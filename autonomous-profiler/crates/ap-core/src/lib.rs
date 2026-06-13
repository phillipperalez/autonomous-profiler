//! ap-core: the backend-neutral brain of the autonomous profiler.
//!
//! Pipeline: a [`collector::Collector`] produces a [`model::RawProfile`] (folded
//! stacks for CPU, or a pre-built model for alloc); [`analyze`] folds that into a
//! ranked [`model::ProfileModel`]; [`compile`] turns the model into a
//! token-budgeted [`compile::ContextBundle`] an LLM can act on directly.
//!
//! Nothing here knows which profiler produced the data — that is the whole point.
//! Backends live in `ap-collectors` and only have to emit folded stacks.

pub mod analyze;
pub mod collector;
pub mod compile;
pub mod language;
pub mod model;
pub mod symbolize;

pub use model::{ProfileKind, ProfileModel, Unit};
