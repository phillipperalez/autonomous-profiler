//! xctrace backend (stub) — macOS-native, richer (CPU + allocations in one run).
//!
//! Plan: `xctrace record --template 'Time Profiler' --launch -- <bin>` then parse
//! the `.trace` bundle into folded stacks. Heavier to drive than samply, so it is
//! scaffolded rather than wired for v0. Hack-day fan-out item #4.

use ap_core::collector::{Collector, CollectOpts, Target};
use ap_core::model::RawProfile;
use anyhow::{bail, Result};

pub struct XctraceCollector;

impl Collector for XctraceCollector {
    fn id(&self) -> &'static str {
        "xctrace"
    }

    fn available(&self) -> bool {
        cfg!(target_os = "macos") && which::which("xctrace").is_ok()
    }

    fn collect(&self, _target: &Target, _opts: &CollectOpts) -> Result<RawProfile> {
        bail!("xctrace collector not implemented yet (fan-out item: mac-native CPU+alloc)")
    }
}
