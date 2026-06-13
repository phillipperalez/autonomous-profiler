//! dhat backend (memory). Not a sampler: it ingests a `dhat-heap.json` produced
//! by a target instrumented with dhat's global allocator (see the analyzer copy's
//! `dhat-heap` feature). Lowers allocation program-points into folded stacks
//! weighted by bytes, so the same ranking/compile machinery applies.

use ap_core::collector::{Collector, CollectOpts, Target};
use ap_core::model::{FoldedStack, FoldedStacks, Frame, RawProfile, SourceLoc};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::PathBuf;

pub struct DhatCollector {
    pub json_path: PathBuf,
}

impl Collector for DhatCollector {
    fn id(&self) -> &'static str {
        "dhat"
    }

    fn available(&self) -> bool {
        self.json_path.exists()
    }

    fn collect(&self, _target: &Target, _opts: &CollectOpts) -> Result<RawProfile> {
        let bytes = std::fs::read(&self.json_path)
            .with_context(|| format!("reading {}", self.json_path.display()))?;
        Ok(RawProfile::Folded(parse_dhat(&bytes)?))
    }
}

/// Parse a dhat (`dhatFileVersion: 2`) heap file into byte-weighted folded
/// stacks. Each program-point's `tb` (total bytes) becomes the stack weight; its
/// `fs` frame indices map into `ftbl`.
pub fn parse_dhat(bytes: &[u8]) -> Result<FoldedStacks> {
    let root: Value = serde_json::from_slice(bytes).context("parsing dhat JSON")?;
    let ftbl = root
        .get("ftbl")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("dhat file has no `ftbl`"))?;
    let pps = root
        .get("pps")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("dhat file has no `pps`"))?;

    let mut out = FoldedStacks::default();
    for pp in pps {
        let tb = pp.get("tb").and_then(Value::as_u64).unwrap_or(0);
        if tb == 0 {
            continue;
        }
        let fs = match pp.get("fs").and_then(Value::as_array) {
            Some(a) => a,
            None => continue,
        };
        // fs[0] is the innermost (allocation) frame. Build root->leaf so the
        // allocation site owns the self bytes. Drop the allocator shim frames
        // (Global::allocate, dhat::Alloc, malloc, ...) so the real user call site
        // becomes the leaf rather than the allocator itself.
        let mut frames: Vec<Frame> = Vec::new();
        for cell in fs.iter().rev() {
            let idx = match cell.as_u64() {
                Some(i) => i as usize,
                None => continue,
            };
            let entry = match ftbl.get(idx).and_then(Value::as_str) {
                Some(s) => s,
                None => continue,
            };
            if entry == "[root]" {
                continue;
            }
            let frame = parse_ftbl_entry(entry);
            if is_alloc_internal(&frame.symbol) {
                continue;
            }
            frames.push(frame);
        }
        if frames.is_empty() {
            continue;
        }
        out.stacks.push(FoldedStack { frames, weight: tb });
        out.total_weight += tb;
    }

    if out.stacks.is_empty() {
        return Err(anyhow!("dhat file had no non-empty allocation points"));
    }
    Ok(out)
}

/// Allocator/runtime shim frames that sit between user code and the OS. Dropping
/// them re-roots the allocation cost onto the code that actually requested memory.
fn is_alloc_internal(sym: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "alloc::alloc::",
        "core::alloc::",
        "dhat::",
        "__rust_alloc",
        "__rust_realloc",
        "__rdl_alloc",
        "malloc",
        "realloc",
        "calloc",
        "Allocator>::allocate",
        "GlobalAlloc>::alloc",
        "RawVec",
        "raw_vec",
        // Generic container plumbing — peel to the user code that grows them.
        "alloc::vec::Vec",
        "alloc::boxed::Box",
        "alloc::collections::",
        "spec_from_elem",
        "SpecFromElem",
        "hashbrown::raw::alloc",
        "RawTableInner::fallible_with_capacity",
        "RawTableInner::new_uninitialized",
        "compact_str::repr::heap",
    ];
    PATTERNS.iter().any(|p| sym.contains(p))
}

/// Parse a `ftbl` entry like `"0x10abcd: polars::foo (src/lib.rs:42:5)"` into a
/// frame. Tolerates missing address and missing source.
fn parse_ftbl_entry(entry: &str) -> Frame {
    // Split off a trailing "(file:line:col)" if present.
    let (head, source) = match (entry.rfind(" ("), entry.ends_with(')')) {
        (Some(open), true) => {
            let src = &entry[open + 2..entry.len() - 1];
            (&entry[..open], parse_source(src))
        }
        _ => (entry, None),
    };
    // Strip a leading "0x...: " address prefix.
    let symbol = match head.find(": ") {
        Some(p) if head.starts_with("0x") => head[p + 2..].to_string(),
        _ => head.to_string(),
    };
    Frame { symbol, source }
}

fn parse_source(src: &str) -> Option<SourceLoc> {
    // file:line[:col]
    let mut parts = src.rsplitn(3, ':');
    let _col = parts.next();
    let line = parts.next();
    let file = parts.next();
    match (file, line.and_then(|l| l.parse::<u32>().ok())) {
        (Some(f), Some(l)) => Some(SourceLoc {
            file: f.to_string(),
            line: l,
        }),
        // Handle the `file:line` (no col) shape.
        _ => {
            let mut p = src.rsplitn(2, ':');
            let l = p.next().and_then(|l| l.parse::<u32>().ok());
            let f = p.next();
            match (f, l) {
                (Some(f), Some(l)) => Some(SourceLoc {
                    file: f.to_string(),
                    line: l,
                }),
                _ => None,
            }
        }
    }
}
