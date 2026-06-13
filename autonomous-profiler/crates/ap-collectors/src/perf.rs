//! perf backend (stub) — the x86 / Linux extension path.
//!
//! Plan: `perf record -g -- <bin>` then `perf script` -> collapse to folded
//! stacks (inferno-style) -> [`FoldedStacks`]. This is the "Linux box at home"
//! collector; the backend-neutral IR means nothing downstream changes once it
//! lands. Hack-day fan-out item #5.

use ap_core::collector::{Collector, CollectOpts, Target};
use ap_core::model::RawProfile;
use anyhow::{bail, Result};

pub struct PerfCollector;

impl Collector for PerfCollector {
    fn id(&self) -> &'static str {
        "perf"
    }

    fn available(&self) -> bool {
        cfg!(target_os = "linux") && which::which("perf").is_ok()
    }

    fn collect(&self, _target: &Target, _opts: &CollectOpts) -> Result<RawProfile> {
        bail!("perf collector not implemented yet (fan-out item: x86/Linux path)")
    }
}
