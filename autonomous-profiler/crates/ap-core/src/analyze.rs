//! Fold raw stacks into a ranked model, then derive the higher-level signals the
//! context compiler needs: crate rollup, dominant hot path, why-hot tags.

use crate::collector::Mode;
use crate::model::*;
use crate::symbolize::{crate_of, demangle_symbol};
use std::collections::HashMap;

/// Lower folded stacks into a ranked [`ProfileModel`].
pub fn build_model(
    folded: FoldedStacks,
    mode: Mode,
    backend: &str,
    workload: &str,
) -> ProfileModel {
    let kind = match mode {
        Mode::Cpu => ProfileKind::Cpu,
        Mode::Alloc => ProfileKind::Alloc,
    };
    let unit = match mode {
        Mode::Cpu => Unit::Samples,
        Mode::Alloc => Unit::Bytes,
    };

    // Aggregate per demangled function.
    struct Acc {
        symbol: String,
        demangled: String,
        crate_name: String,
        source: Option<SourceLoc>,
        self_weight: u64,
        total_weight: u64,
    }
    let mut by_func: HashMap<String, Acc> = HashMap::new();
    let mut edges: HashMap<(String, String), u64> = HashMap::new();

    for stack in &folded.stacks {
        if stack.frames.is_empty() {
            continue;
        }
        // Each function appears at most once per stack for `total` (avoid
        // double-counting recursion within one stack).
        let mut seen_in_stack: HashMap<&str, ()> = HashMap::new();
        let demangled: Vec<String> = stack
            .frames
            .iter()
            .map(|f| demangle_symbol(&f.symbol))
            .collect();

        for (i, frame) in stack.frames.iter().enumerate() {
            let dem = &demangled[i];
            let acc = by_func.entry(dem.clone()).or_insert_with(|| Acc {
                symbol: frame.symbol.clone(),
                demangled: dem.clone(),
                crate_name: crate_of(dem),
                source: frame.source.clone(),
                self_weight: 0,
                total_weight: 0,
            });
            if acc.source.is_none() {
                acc.source = frame.source.clone();
            }
            if seen_in_stack.insert(dem.as_str(), ()).is_none() {
                acc.total_weight += stack.weight;
            }
        }
        // Leaf (last frame, root->leaf order) owns the self time.
        if let Some(leaf) = demangled.last() {
            if let Some(acc) = by_func.get_mut(leaf) {
                acc.self_weight += stack.weight;
            }
        }
        // Edges: caller -> callee for adjacent frames.
        for w in demangled.windows(2) {
            *edges.entry((w[0].clone(), w[1].clone())).or_insert(0) += stack.weight;
        }
    }

    let mut functions: Vec<FunctionStat> = by_func
        .into_values()
        .map(|a| FunctionStat {
            symbol: a.symbol,
            demangled: a.demangled,
            crate_name: a.crate_name,
            source: a.source,
            self_weight: a.self_weight,
            total_weight: a.total_weight,
        })
        .collect();
    functions.sort_by(|a, b| b.self_weight.cmp(&a.self_weight));

    let mut edges: Vec<CallEdge> = edges
        .into_iter()
        .map(|((caller, callee), weight)| CallEdge {
            caller,
            callee,
            weight,
        })
        .collect();
    edges.sort_by(|a, b| b.weight.cmp(&a.weight));

    ProfileModel {
        kind,
        unit,
        total_weight: folded.total_weight,
        functions,
        graph: CallGraph { edges },
        workload: workload.to_string(),
        backend: backend.to_string(),
    }
}

/// Weight grouped by crate (using `self_weight`), as `(crate, pct)` sorted
/// descending. This is the "how much time is actually inside polars" answer.
pub fn crate_rollup(model: &ProfileModel) -> Vec<(String, f64)> {
    let mut by_crate: HashMap<&str, u64> = HashMap::new();
    for f in &model.functions {
        *by_crate.entry(f.crate_name.as_str()).or_insert(0) += f.self_weight;
    }
    let mut rows: Vec<(String, f64)> = by_crate
        .into_iter()
        .map(|(c, w)| (c.to_string(), pct(w, model.total_weight)))
        .collect();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    rows
}

/// Dominant call chain: start from the heaviest caller edge and greedily follow
/// the heaviest callee. Bounded to avoid cycles running away.
pub fn hot_path(model: &ProfileModel) -> Vec<String> {
    let edges = &model.graph.edges;
    if edges.is_empty() {
        // Degenerate (single-frame stacks): just the hottest function.
        return model
            .functions
            .first()
            .map(|f| vec![f.demangled.clone()])
            .unwrap_or_default();
    }
    let start = edges[0].caller.clone();
    let mut path = vec![start.clone()];
    let mut current = start;
    let mut guard = 0;
    while guard < 32 {
        guard += 1;
        let next = edges
            .iter()
            .filter(|e| e.caller == current)
            .max_by_key(|e| e.weight)
            .map(|e| e.callee.clone());
        match next {
            Some(n) if !path.contains(&n) => {
                path.push(n.clone());
                current = n;
            }
            _ => break,
        }
    }
    path
}

/// Top callers and callees of a function, as display strings.
pub fn neighbors(model: &ProfileModel, demangled: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    for e in model.graph.edges.iter().filter(|e| e.callee == demangled).take(limit) {
        out.push(format!("<- {}", short(&e.caller)));
    }
    for e in model.graph.edges.iter().filter(|e| e.caller == demangled).take(limit) {
        out.push(format!("-> {}", short(&e.callee)));
    }
    out
}

/// Heuristic "why is this hot" tags from the function name + self/total ratio.
pub fn tags(model: &ProfileModel, f: &FunctionStat) -> Vec<String> {
    let mut tags = Vec::new();
    let d = f.demangled.to_lowercase();
    const ALLOC_HINTS: &[&str] = &[
        "clone", "to_vec", "to_owned", "reserve", "with_capacity", "collect",
        "format", "::vec", "hashmap", "memcpy", "memmove", "alloc",
    ];
    if ALLOC_HINTS.iter().any(|h| d.contains(h)) {
        tags.push("alloc/copy-heavy".to_string());
    }
    if f.total_weight > 0 && f.self_weight * 100 / f.total_weight.max(1) >= 80 {
        tags.push("tight self-time (compute leaf)".to_string());
    }
    if model.total_pct(f) >= 50.0 {
        tags.push("dominates the profile".to_string());
    }
    tags
}

fn short(demangled: &str) -> String {
    // Trailing two path segments keep neighbor lists readable.
    let parts: Vec<&str> = demangled.split("::").collect();
    if parts.len() <= 2 {
        demangled.to_string()
    } else {
        parts[parts.len() - 2..].join("::")
    }
}
