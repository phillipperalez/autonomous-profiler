//! `autoperf.toml` — the config contract a user drops in their Rust project so the
//! autonomous-profiler improve loop can run on *their* code with zero hardcoding.
//!
//! This is the single source of truth for what the loop needs: which cargo project to
//! profile, how to prove a change is correct (test suite or output fingerprint), which
//! workloads to benchmark (the first is the optimization target; the rest are guards),
//! and the gating thresholds. `ap init` writes a starter file; `ap config --json`
//! parses + validates + normalizes it (resolving paths to absolute) for the workflow,
//! the skill, or a human to consume.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The whole `autoperf.toml`, after parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoperfConfig {
    pub target: TargetCfg,
    #[serde(default)]
    pub gate: GateCfg,
    #[serde(default)]
    pub improve: ImproveCfg,
    /// One `[[workload]]` per benchmark. The first (or the one marked `primary`) is the
    /// optimization target; the rest are guard datasets that must not regress.
    #[serde(default, rename = "workload")]
    pub workloads: Vec<WorkloadCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetCfg {
    /// Path to the cargo project to profile/optimize (relative to the config file, or absolute).
    pub dir: String,
    /// Git repo to stamp commits/benchmarks against, if it differs from `dir`
    /// (e.g. a separate bench harness that builds the real library). Defaults to `dir`.
    #[serde(default)]
    pub repo: Option<String>,
    /// Source language. Only `rust` is supported today.
    #[serde(default = "default_lang")]
    pub lang: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GateCfg {
    /// Correctness gate for an internal target you own: a shell command that must exit 0
    /// (e.g. `cargo test -p mycrate`). Run from `target.dir`.
    #[serde(default)]
    pub test: Option<String>,
    /// Correctness gate for an external/library target where you can't run the suite:
    /// the workload prints a deterministic `FINGERPRINT=...` line that the loop must
    /// match against the baseline bit-for-bit.
    #[serde(default)]
    pub fingerprint: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImproveCfg {
    /// Peak-RSS ceiling (MB). A change may trade memory for speed only under this.
    pub ram_budget_mb: u64,
    /// A winner must beat the baseline on the primary workload by at least this percent.
    pub min_improvement_pct: f64,
    /// Any guard workload regressing by more than this percent fails the change.
    pub guard_regression_pct: f64,
    /// Optimization lenses the swarm rotates through (one diverse proposal each).
    pub lenses: Vec<String>,
    /// Functions already optimized / off-limits — the loop won't re-touch these.
    #[serde(default)]
    pub off_limits: Vec<String>,
}

impl Default for ImproveCfg {
    fn default() -> Self {
        ImproveCfg {
            ram_budget_mb: 8192,
            min_improvement_pct: 3.0,
            guard_regression_pct: 2.0,
            lenses: default_lenses(),
            off_limits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadCfg {
    /// Stable series label — same label == same benchmark across commits.
    pub label: String,
    /// Cargo example name to build+run (mutually exclusive with `bin`).
    #[serde(default)]
    pub example: Option<String>,
    /// Prebuilt binary to run instead of building an example.
    #[serde(default)]
    pub bin: Option<String>,
    /// Args passed to the workload (e.g. an input file). Kept fixed across commits.
    #[serde(default)]
    pub args: Vec<String>,
    /// Cargo features to enable when building the example.
    #[serde(default)]
    pub features: Vec<String>,
    /// Timed runs; the minimum is reported as the benchmark.
    #[serde(default = "default_runs")]
    pub runs: u32,
    /// Mark the optimization-target workload. If none is marked, the first is primary.
    #[serde(default)]
    pub primary: bool,
}

fn default_lang() -> String {
    "rust".into()
}
fn default_runs() -> u32 {
    5
}
fn default_lenses() -> Vec<String> {
    ["algorithmic", "locality", "dedup", "datashape", "branchflat"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

impl AutoperfConfig {
    /// Parse + validate `autoperf.toml` at `path`, resolving `dir`/`repo`/file-like args
    /// to absolute paths relative to the config file's directory.
    pub fn load(path: &Path) -> Result<AutoperfConfig> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut cfg: AutoperfConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        cfg.resolve_paths(&base);
        cfg.validate()?;
        Ok(cfg)
    }

    /// Locate `autoperf.toml`: an explicit path, else `./autoperf.toml`.
    pub fn locate(explicit: Option<&Path>) -> Result<PathBuf> {
        if let Some(p) = explicit {
            if p.is_dir() {
                let f = p.join("autoperf.toml");
                if f.exists() {
                    return Ok(f);
                }
                bail!("no autoperf.toml in {}", p.display());
            }
            return Ok(p.to_path_buf());
        }
        let cwd = PathBuf::from("autoperf.toml");
        if cwd.exists() {
            return Ok(cwd);
        }
        bail!("no autoperf.toml found (pass a path, or run `ap init` to create one)")
    }

    fn resolve_paths(&mut self, base: &Path) {
        let abs = |p: &str| -> String {
            let pb = PathBuf::from(p);
            let joined = if pb.is_absolute() { pb } else { base.join(&pb) };
            // Canonicalize when the path exists; otherwise keep the lexical join.
            std::fs::canonicalize(&joined)
                .unwrap_or(joined)
                .to_string_lossy()
                .to_string()
        };
        self.target.dir = abs(&self.target.dir);
        self.target.repo = Some(self.target.repo.clone().map(|r| abs(&r)).unwrap_or_else(|| self.target.dir.clone()));
        // Best-effort: make file-like args absolute relative to the config dir so the
        // workflow can pass them through regardless of cwd.
        for w in &mut self.workloads {
            for a in &mut w.args {
                let candidate = if PathBuf::from(&*a).is_absolute() {
                    PathBuf::from(&*a)
                } else {
                    base.join(&*a)
                };
                if candidate.exists() {
                    *a = std::fs::canonicalize(&candidate)
                        .unwrap_or(candidate)
                        .to_string_lossy()
                        .to_string();
                }
            }
            if let Some(b) = &w.bin {
                let pb = PathBuf::from(b);
                if !pb.is_absolute() {
                    *w.bin.as_mut().unwrap() = base.join(&pb).to_string_lossy().to_string();
                }
            }
        }
    }

    fn validate(&self) -> Result<()> {
        if self.target.lang != "rust" {
            bail!("target.lang = '{}' unsupported (only 'rust' today)", self.target.lang);
        }
        if !Path::new(&self.target.dir).exists() {
            bail!("target.dir does not exist: {}", self.target.dir);
        }
        if self.gate.test.is_none() && !self.gate.fingerprint {
            bail!("gate: set either `test = \"<cmd>\"` (internal target) or `fingerprint = true` (external target)");
        }
        if self.workloads.is_empty() {
            bail!("define at least one [[workload]] (the first is the optimization target)");
        }
        let primaries = self.workloads.iter().filter(|w| w.primary).count();
        if primaries > 1 {
            bail!("only one [[workload]] may set primary = true");
        }
        for w in &self.workloads {
            if w.example.is_none() && w.bin.is_none() {
                bail!("workload '{}': set either `example` or `bin`", w.label);
            }
            if w.example.is_some() && w.bin.is_some() {
                bail!("workload '{}': set only one of `example` / `bin`", w.label);
            }
        }
        if self.improve.lenses.is_empty() {
            bail!("improve.lenses must list at least one lens");
        }
        Ok(())
    }

    /// Index of the optimization-target workload (explicit `primary`, else the first).
    pub fn primary_index(&self) -> usize {
        self.workloads.iter().position(|w| w.primary).unwrap_or(0)
    }
}

/// The starter `autoperf.toml` written by `ap init`, with inline documentation.
pub fn starter_toml() -> &'static str {
    r#"# autoperf.toml — config for the autonomous-profiler improve loop.
# Drop this in the root of the Rust project you want to optimize, then run the
# `/autoperf` skill (or invoke the workflow). See README "Run it on your own project".

[target]
dir  = "."          # cargo project to profile/optimize (relative to this file, or absolute)
# repo = "."        # git repo to commit/stamp against, if it differs from `dir`
lang = "rust"

[gate]
# Correctness gate — choose ONE:
#  (a) internal target you own: a command that must exit 0.
test = "cargo test"
#  (b) external/library target: the workload prints a deterministic `FINGERPRINT=...`
#      line the loop matches bit-for-bit against the baseline.
# fingerprint = true

[improve]
ram_budget_mb       = 8192   # peak-RSS ceiling; trade RAM for speed only under this
min_improvement_pct = 3.0    # a winner must beat baseline by at least this on the primary
guard_regression_pct = 2.0   # any guard workload regressing more than this fails the change
lenses = ["algorithmic", "locality", "dedup", "datashape", "branchflat"]
# off_limits = ["already_optimized_fn"]   # functions the loop should not re-touch

# One [[workload]] per benchmark. The FIRST (or the one with primary = true) is the
# optimization target; the rest are guards that must not regress.
[[workload]]
label   = "primary"
example = "my_bench"            # cargo example name  (or:  bin = "target/release/foo")
args    = ["data/input.parquet"]
runs    = 5
primary = true

# [[workload]]
# label   = "guard-small"
# example = "my_bench"
# args    = ["data/small.parquet"]
# runs    = 3
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_parses_and_validates_against_cwd() {
        // The starter targets "." which exists; it should round-trip through the parser
        // (validation of dir existence uses ".", always present).
        let cfg: AutoperfConfig = toml::from_str(starter_toml()).expect("starter parses");
        assert_eq!(cfg.target.dir, ".");
        assert_eq!(cfg.workloads.len(), 1);
        assert!(cfg.workloads[0].primary);
        assert_eq!(cfg.primary_index(), 0);
        assert_eq!(cfg.gate.test.as_deref(), Some("cargo test"));
        assert_eq!(cfg.improve.lenses.len(), 5);
    }

    #[test]
    fn rejects_missing_gate_and_no_workload() {
        let t = r#"
[target]
dir = "."
[improve]
ram_budget_mb = 8192
min_improvement_pct = 3.0
guard_regression_pct = 2.0
lenses = ["algorithmic"]
"#;
        let cfg: AutoperfConfig = toml::from_str(t).unwrap();
        assert!(cfg.validate().is_err(), "missing gate + workloads must fail");
    }

    #[test]
    fn primary_defaults_to_first() {
        let t = r#"
[target]
dir = "."
[gate]
test = "cargo test"
[[workload]]
label = "a"
example = "x"
[[workload]]
label = "b"
example = "y"
"#;
        let cfg: AutoperfConfig = toml::from_str(t).unwrap();
        assert_eq!(cfg.primary_index(), 0);
        assert_eq!(cfg.workloads[1].runs, 5, "runs defaults to 5");
    }
}
