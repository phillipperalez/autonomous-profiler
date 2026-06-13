//! Language abstraction. Rust is the only implementation today; Go/C++/Python
//! are future variants behind the same surface (demangler, default backend,
//! build command, crate-filter rules).

use crate::collector::Target;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
}

impl Language {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Ok(Language::Rust),
            other => bail!("unsupported language '{other}' (only 'rust' for now)"),
        }
    }

    pub fn default_collector_id(&self) -> &'static str {
        match self {
            // samply: unprivileged on macOS arm64 and identical on x86 Linux.
            Language::Rust => "samply",
        }
    }
}

/// The cargo profile we build targets under: release codegen + line tables +
/// frame pointers so the sampler can symbolize. Defined in the target's
/// `Cargo.toml` (see analyzer's `[profile.profiling]`).
pub const PROFILE: &str = "profiling";

/// Build a cargo example with the profiling profile and return the binary path.
/// Resolves a [`Target::CargoExample`] into a [`Target::Binary`] the collector
/// can record.
pub fn build_cargo_example(
    dir: &PathBuf,
    name: &str,
    features: &[String],
) -> Result<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(dir)
        .arg("build")
        .arg("--profile")
        .arg(PROFILE)
        .arg("--example")
        .arg(name);
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    // The analyzer target links system openblas; make pkg-config find brew's.
    if let Some(pc) = openblas_pkgconfig() {
        let existing = std::env::var("PKG_CONFIG_PATH").unwrap_or_default();
        let joined = if existing.is_empty() {
            pc
        } else {
            format!("{pc}:{existing}")
        };
        cmd.env("PKG_CONFIG_PATH", joined);
    }

    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn cargo build in {}", dir.display()))?;
    if !status.success() {
        bail!("cargo build --example {name} failed");
    }

    let bin = dir
        .join("target")
        .join(PROFILE)
        .join("examples")
        .join(name);
    if !bin.exists() {
        bail!("built example not found at {}", bin.display());
    }
    Ok(bin)
}

fn openblas_pkgconfig() -> Option<String> {
    let p = "/opt/homebrew/opt/openblas/lib/pkgconfig";
    if std::path::Path::new(p).exists() {
        Some(p.to_string())
    } else {
        None
    }
}

/// Resolve any [`Target`] into a runnable binary target, building if needed.
pub fn resolve_target(target: Target) -> Result<Target> {
    match target {
        Target::CargoExample {
            dir,
            name,
            features,
            args,
        } => {
            let path = build_cargo_example(&dir, &name, &features)?;
            Ok(Target::Binary { path, args })
        }
        other => Ok(other),
    }
}
