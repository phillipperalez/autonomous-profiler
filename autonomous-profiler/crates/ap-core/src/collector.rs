//! The collector extension seam. Add a backend = implement [`Collector`].

use crate::model::RawProfile;
use anyhow::Result;
use std::path::PathBuf;

/// What to profile.
#[derive(Clone, Debug)]
pub enum Target {
    /// A prebuilt executable + its args.
    Binary { path: PathBuf, args: Vec<String> },
    /// A cargo example to build then profile. The CLI resolves this to a
    /// `Binary` via [`crate::language`] before handing it to a collector.
    CargoExample {
        dir: PathBuf,
        name: String,
        features: Vec<String>,
        args: Vec<String>,
    },
    /// Attach to a running process.
    Pid(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Cpu,
    Alloc,
}

#[derive(Clone, Debug)]
pub struct CollectOpts {
    pub mode: Mode,
    pub duration_secs: Option<u64>,
    pub rate_hz: u32,
    /// Substring hint for the backend (e.g. a crate name). Advisory only —
    /// hot-path focusing happens in the context compiler so callers stay in the
    /// frame for context.
    pub frame_filter: Option<String>,
}

impl Default for CollectOpts {
    fn default() -> Self {
        CollectOpts {
            mode: Mode::Cpu,
            duration_secs: None,
            rate_hz: 1000,
            frame_filter: None,
        }
    }
}

/// A profiling backend. Implementors only have to produce folded stacks (or a
/// finished model); everything downstream is backend-agnostic.
pub trait Collector {
    fn id(&self) -> &'static str;
    /// Is this backend usable on this machine right now (tool installed, platform
    /// supported, privileges available)?
    fn available(&self) -> bool;
    fn collect(&self, target: &Target, opts: &CollectOpts) -> Result<RawProfile>;
}
