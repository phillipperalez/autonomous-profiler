//! Profiling backends. Each lowers a native profiler's output into the
//! backend-neutral folded-stack IR (or a finished model). Adding a backend = add
//! a module here and register it in [`collector_by_id`].

pub mod dhat;
pub mod perf;
pub mod pipeline;
pub mod samply;
pub mod xctrace;

use ap_core::collector::Collector;

/// Look up a collector by id. Unknown ids return `None`.
pub fn collector_by_id(id: &str) -> Option<Box<dyn Collector>> {
    match id {
        "samply" => Some(Box::new(samply::SamplyCollector)),
        "perf" => Some(Box::new(perf::PerfCollector)),
        "xctrace" => Some(Box::new(xctrace::XctraceCollector)),
        // dhat is configured with a path, so it isn't constructed here.
        _ => None,
    }
}

/// Ids of every registered CPU-sampling backend, in preference order.
pub fn cpu_backends() -> &'static [&'static str] {
    &["samply", "perf", "xctrace"]
}
