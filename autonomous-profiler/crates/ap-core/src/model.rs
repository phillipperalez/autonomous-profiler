//! The normalized, backend-neutral profile model + the folded-stack IR every
//! collector emits.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Cpu,
    Alloc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Samples,
    Bytes,
}

impl Unit {
    pub fn label(&self) -> &'static str {
        match self {
            Unit::Samples => "samples",
            Unit::Bytes => "bytes",
        }
    }
}

/// A source location, when symbolization could resolve one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLoc {
    pub file: String,
    pub line: u32,
}

/// One frame inside a folded stack. `symbol` is raw (possibly mangled); the
/// analyzer demangles it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLoc>,
}

/// One sampled stack: frames ordered root -> leaf, plus its weight (sample count
/// for CPU, bytes for alloc).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FoldedStack {
    pub frames: Vec<Frame>,
    pub weight: u64,
}

/// The backend-neutral intermediate representation. Every collector lowers its
/// native format into this; everything downstream consumes only this.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FoldedStacks {
    pub stacks: Vec<FoldedStack>,
    pub total_weight: u64,
}

/// What a collector hands back: raw folded stacks (CPU sampling) or, for backends
/// like dhat that already aggregate, a finished model.
pub enum RawProfile {
    Folded(FoldedStacks),
    Model(Box<ProfileModel>),
}

/// Per-function aggregate. `self_weight` excludes callees; `total_weight`
/// includes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionStat {
    pub symbol: String,
    pub demangled: String,
    pub crate_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLoc>,
    pub self_weight: u64,
    pub total_weight: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallEdge {
    pub caller: String,
    pub callee: String,
    pub weight: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CallGraph {
    pub edges: Vec<CallEdge>,
}

/// The normalized model: the single thing the analyzer, the context compiler, the
/// CLI, and the MCP server all read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileModel {
    pub kind: ProfileKind,
    pub unit: Unit,
    pub total_weight: u64,
    /// Sorted by `self_weight` descending.
    pub functions: Vec<FunctionStat>,
    pub graph: CallGraph,
    pub workload: String,
    pub backend: String,
}

impl ProfileModel {
    /// `self_weight` of `f` as a fraction (0..1) of total profiled weight.
    pub fn self_pct(&self, f: &FunctionStat) -> f64 {
        pct(f.self_weight, self.total_weight)
    }

    pub fn total_pct(&self, f: &FunctionStat) -> f64 {
        pct(f.total_weight, self.total_weight)
    }
}

pub fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        100.0 * (part as f64) / (whole as f64)
    }
}
