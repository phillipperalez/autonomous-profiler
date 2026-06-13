//! The context compiler: the differentiator. Turns a [`ProfileModel`] into a
//! token-budgeted, source-attributed bundle an LLM can act on directly, instead
//! of a raw flamegraph it would burn thousands of tokens decoding.

use crate::analyze;
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct CompileOpts {
    /// Approximate token ceiling for the whole bundle.
    pub token_budget: usize,
    /// Restrict hotspots to crates/symbols containing this substring.
    pub focus: Option<String>,
    /// Source context lines on each side of a hotspot's line.
    pub source_ctx_lines: usize,
    /// Hard cap on hotspots regardless of budget.
    pub max_hotspots: usize,
    /// Roots to resolve relative source paths against.
    pub source_roots: Vec<PathBuf>,
}

impl Default for CompileOpts {
    fn default() -> Self {
        CompileOpts {
            token_budget: 8000,
            focus: None,
            source_ctx_lines: 6,
            max_hotspots: 25,
            source_roots: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hotspot {
    pub rank: usize,
    pub function: String,
    pub crate_name: String,
    pub self_pct: f64,
    pub total_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLoc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub neighbors: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextBundle {
    pub workload: String,
    pub backend: String,
    pub kind: ProfileKind,
    pub unit: Unit,
    pub total_weight: u64,
    pub crate_rollup: Vec<(String, f64)>,
    pub hot_path: Vec<String>,
    pub hotspots: Vec<Hotspot>,
    pub suggested_questions: Vec<String>,
    pub notes: Vec<String>,
}

/// Rough token estimate (chars / 4) used for budgeting.
fn est_tokens(s: &str) -> usize {
    s.len() / 4
}

pub fn compile(model: &ProfileModel, opts: &CompileOpts) -> ContextBundle {
    let rollup = analyze::crate_rollup(model);
    let hot_path = analyze::hot_path(model);

    let focus = opts.focus.as_deref().map(|f| f.to_lowercase());
    let mut hotspots = Vec::new();
    let mut notes = Vec::new();
    let mut running_tokens = 600; // header + rollup + hot path overhead

    for f in &model.functions {
        if hotspots.len() >= opts.max_hotspots {
            break;
        }
        if f.self_weight == 0 {
            break; // functions are self-weight sorted; nothing past here matters
        }
        if let Some(focus) = &focus {
            let hit = f.demangled.to_lowercase().contains(focus)
                || f.crate_name.to_lowercase().contains(focus);
            if !hit {
                continue;
            }
        }

        let snippet = f
            .source
            .as_ref()
            .and_then(|loc| read_snippet(loc, opts.source_ctx_lines, &opts.source_roots));

        let hs = Hotspot {
            rank: hotspots.len() + 1,
            function: f.demangled.clone(),
            crate_name: f.crate_name.clone(),
            self_pct: model.self_pct(f),
            total_pct: model.total_pct(f),
            source: f.source.clone(),
            snippet,
            neighbors: analyze::neighbors(model, &f.demangled, 3),
            tags: analyze::tags(model, f),
        };

        // Budget check: degrade (drop snippet) before dropping the hotspot.
        let cost = hotspot_tokens(&hs);
        if running_tokens + cost > opts.token_budget {
            let mut lean = hs.clone();
            lean.snippet = None;
            let lean_cost = hotspot_tokens(&lean);
            if running_tokens + lean_cost > opts.token_budget {
                notes.push(format!(
                    "Truncated at {} hotspots to fit ~{} token budget.",
                    hotspots.len(),
                    opts.token_budget
                ));
                break;
            }
            running_tokens += lean_cost;
            hotspots.push(lean);
        } else {
            running_tokens += cost;
            hotspots.push(hs);
        }
    }

    let suggested_questions = build_questions(model, &rollup, &hotspots);

    ContextBundle {
        workload: model.workload.clone(),
        backend: model.backend.clone(),
        kind: model.kind,
        unit: model.unit,
        total_weight: model.total_weight,
        crate_rollup: rollup.into_iter().take(8).collect(),
        hot_path,
        hotspots,
        suggested_questions,
        notes,
    }
}

fn hotspot_tokens(hs: &Hotspot) -> usize {
    let base = 40;
    base + hs.snippet.as_deref().map(est_tokens).unwrap_or(0)
}

fn read_snippet(loc: &SourceLoc, ctx: usize, roots: &[PathBuf]) -> Option<String> {
    let candidates = std::iter::once(PathBuf::from(&loc.file))
        .chain(roots.iter().map(|r| r.join(&loc.file)))
        .chain(roots.iter().filter_map(|r| {
            // Also try matching just the file's tail under each root.
            PathBuf::from(&loc.file)
                .file_name()
                .map(|n| r.join(n))
        }));

    for path in candidates {
        if let Ok(text) = fs::read_to_string(&path) {
            let lines: Vec<&str> = text.lines().collect();
            if lines.is_empty() {
                continue;
            }
            let center = (loc.line as usize).saturating_sub(1).min(lines.len() - 1);
            let start = center.saturating_sub(ctx);
            let end = (center + ctx + 1).min(lines.len());
            let mut out = String::new();
            for (i, line) in lines[start..end].iter().enumerate() {
                let n = start + i + 1;
                let marker = if n == loc.line as usize { ">" } else { " " };
                out.push_str(&format!("{marker} {n:>5} | {line}\n"));
            }
            return Some(out);
        }
    }
    None
}

fn build_questions(
    model: &ProfileModel,
    rollup: &[(String, f64)],
    hotspots: &[Hotspot],
) -> Vec<String> {
    let mut qs = Vec::new();
    if let Some(top) = hotspots.first() {
        qs.push(format!(
            "`{}` is {:.0}% of self-time. Can it be vectorized, cached, or called less often?",
            top.function, top.self_pct
        ));
    }
    if let Some((c, p)) = rollup.first() {
        if *p >= 25.0 {
            qs.push(format!(
                "{:.0}% of time is inside `{}`. Is there a cheaper API or a way to push work out of it?",
                p, c
            ));
        }
    }
    if hotspots.iter().any(|h| h.tags.iter().any(|t| t.contains("alloc"))) {
        qs.push(
            "Several hotspots look allocation/copy-heavy. Where can we reuse buffers or borrow instead of clone?"
                .to_string(),
        );
    }
    if model.kind == ProfileKind::Alloc {
        qs.push("Which of these allocation sites are on the steady-state path vs one-time setup?".to_string());
    }
    qs
}

impl ContextBundle {
    /// Compact Markdown for humans + LLM paste.
    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("# Profile: {}\n\n", self.workload));
        s.push_str(&format!(
            "backend: `{}` · kind: `{:?}` · total: {} {}\n\n",
            self.backend,
            self.kind,
            self.total_weight,
            self.unit.label()
        ));

        s.push_str("## Where the cost is (by crate)\n\n");
        for (c, p) in &self.crate_rollup {
            s.push_str(&format!("- {:>5.1}%  {}\n", p, c));
        }
        s.push('\n');

        if !self.hot_path.is_empty() {
            s.push_str("## Dominant hot path\n\n");
            s.push_str(&self.hot_path.join("\n  -> "));
            s.push_str("\n\n");
        }

        s.push_str("## Hotspots\n\n");
        for h in &self.hotspots {
            s.push_str(&format!(
                "### {}. {}  ({:.1}% self / {:.1}% total)\n",
                h.rank, h.function, h.self_pct, h.total_pct
            ));
            s.push_str(&format!("- crate: `{}`\n", h.crate_name));
            if let Some(loc) = &h.source {
                s.push_str(&format!("- source: `{}:{}`\n", loc.file, loc.line));
            }
            if !h.tags.is_empty() {
                s.push_str(&format!("- tags: {}\n", h.tags.join(", ")));
            }
            if !h.neighbors.is_empty() {
                s.push_str(&format!("- calls: {}\n", h.neighbors.join("  ")));
            }
            if let Some(snip) = &h.snippet {
                s.push_str("\n```\n");
                s.push_str(snip);
                s.push_str("```\n");
            }
            s.push('\n');
        }

        if !self.suggested_questions.is_empty() {
            s.push_str("## Suggested optimization questions\n\n");
            for q in &self.suggested_questions {
                s.push_str(&format!("- {}\n", q));
            }
            s.push('\n');
        }

        if !self.notes.is_empty() {
            s.push_str("---\n");
            for n in &self.notes {
                s.push_str(&format!("_{}_\n", n));
            }
        }
        s
    }
}
