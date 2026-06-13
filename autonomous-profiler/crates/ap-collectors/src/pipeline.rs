//! End-to-end orchestration shared by the CLI and the MCP server: resolve a
//! target, pick a backend, collect, fold into a model, and cache it.

use crate::collector_by_id;
use crate::dhat::DhatCollector;
use ap_core::analyze;
use ap_core::collector::{CollectOpts, Collector, Mode, Target};
use ap_core::compile::{compile, CompileOpts};
use ap_core::language;
use ap_core::model::{ProfileModel, RawProfile};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A cached profile: the model, the roots used to resolve source snippets, and
/// run metadata (wall-clock duration + creation time + target git commit).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub model: ProfileModel,
    pub source_roots: Vec<PathBuf>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub created_at_ms: u128,
    #[serde(default)]
    pub git: GitInfo,
}

/// The target repo's HEAD when a run/bench happened, so the dashboard can tie
/// findings + performance to commits.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GitInfo {
    pub commit: String,
    pub short: String,
    pub subject: String,
    pub dirty: bool,
}

impl GitInfo {
    pub fn is_empty(&self) -> bool {
        self.commit.is_empty()
    }
}

/// Best-effort git HEAD info for a directory. Empty fields if not a repo.
pub fn git_info(dir: &Path) -> GitInfo {
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let commit = run(&["rev-parse", "HEAD"]).unwrap_or_default();
    let short = run(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let subject = run(&["log", "-1", "--format=%s"]).unwrap_or_default();
    let dirty = run(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    GitInfo {
        commit,
        short,
        subject,
        dirty,
    }
}

pub struct ProfileRequest {
    pub workload: String,
    pub target: Target,
    pub mode: Mode,
    pub rate_hz: u32,
    /// CPU backend id (ignored for alloc).
    pub backend_id: String,
    /// Required for alloc mode: a dhat-heap.json to ingest.
    pub dhat_json: Option<PathBuf>,
    pub source_roots: Vec<PathBuf>,
    /// Target repo to stamp the run with (its HEAD commit).
    pub repo_dir: Option<PathBuf>,
}

pub fn run_profile(req: ProfileRequest) -> Result<ProfileRecord> {
    let opts = CollectOpts {
        mode: req.mode,
        rate_hz: req.rate_hz,
        ..Default::default()
    };
    let started = Instant::now();

    let (raw, backend) = match req.mode {
        Mode::Alloc => {
            let path = req
                .dhat_json
                .clone()
                .ok_or_else(|| anyhow!("alloc mode needs --dhat-json <dhat-heap.json>"))?;
            let c = DhatCollector { json_path: path };
            if !c.available() {
                bail!("dhat json not found");
            }
            (c.collect(&Target::Pid(0), &opts)?, "dhat".to_string())
        }
        Mode::Cpu => {
            let target = language::resolve_target(req.target)?;
            let c = collector_by_id(&req.backend_id)
                .ok_or_else(|| anyhow!("unknown backend '{}'", req.backend_id))?;
            if !c.available() {
                bail!(
                    "backend '{}' is not available on this machine",
                    req.backend_id
                );
            }
            let id = c.id().to_string();
            (c.collect(&target, &opts)?, id)
        }
    };

    let model = match raw {
        RawProfile::Folded(folded) => {
            analyze::build_model(folded, req.mode, &backend, &req.workload)
        }
        RawProfile::Model(m) => *m,
    };

    let git = req
        .repo_dir
        .as_deref()
        .map(git_info)
        .unwrap_or_default();

    Ok(ProfileRecord {
        model,
        source_roots: req.source_roots,
        duration_ms: started.elapsed().as_millis() as u64,
        created_at_ms: now_ms(),
        git,
    })
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// --- caching --------------------------------------------------------------

pub fn cache_dir() -> PathBuf {
    PathBuf::from(".ap").join("profiles")
}

pub fn save(id: &str, record: &ProfileRecord) -> Result<PathBuf> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).context("creating .ap/profiles")?;
    let path = dir.join(format!("{id}.json"));
    let json = serde_json::to_string_pretty(record)?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn load(id: &str) -> Result<ProfileRecord> {
    let path = cache_dir().join(format!("{id}.json"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("no cached profile '{id}' at {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parsing cached profile")
}

// --- findings export (for the findings-viz dashboard) --------------------

/// A run, flattened for the dashboard. Self-contained so the frontend needs no
/// extra lookups.
#[derive(Serialize, Deserialize)]
pub struct RunFindings {
    pub id: String,
    pub workload: String,
    pub backend: String,
    pub kind: String,
    pub unit: String,
    pub total_weight: u64,
    pub function_count: usize,
    pub duration_ms: u64,
    pub created_at_ms: u128,
    pub git: GitInfo,
    pub crate_rollup: Vec<CrateShare>,
    pub hot_path: Vec<String>,
    pub hotspots: Vec<ap_core::compile::Hotspot>,
}

#[derive(Serialize, Deserialize)]
pub struct CrateShare {
    pub name: String,
    pub pct: f64,
}

/// One row in the runs index the dashboard lists.
#[derive(Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: String,
    pub workload: String,
    pub backend: String,
    pub kind: String,
    pub unit: String,
    pub total_weight: u64,
    pub duration_ms: u64,
    pub created_at_ms: u128,
    pub hotspot_count: usize,
    pub top_function: Option<String>,
    pub top_crate: Option<String>,
    #[serde(default)]
    pub git: GitInfo,
}

/// Write `<dir>/runs/<id>.json` and upsert `<dir>/index.json`. The dashboard
/// reads these directly (point a Vite `public/data` dir here).
pub fn write_findings(dir: &Path, id: &str, record: &ProfileRecord) -> Result<PathBuf> {
    let runs = dir.join("runs");
    std::fs::create_dir_all(&runs).with_context(|| format!("creating {}", runs.display()))?;

    let opts = CompileOpts {
        token_budget: 100_000, // findings file is for the UI, not an LLM budget
        source_ctx_lines: 8,
        source_roots: record.source_roots.clone(),
        max_hotspots: 50,
        ..Default::default()
    };
    let bundle = compile(&record.model, &opts);

    let findings = RunFindings {
        id: id.to_string(),
        workload: record.model.workload.clone(),
        backend: record.model.backend.clone(),
        kind: format!("{:?}", record.model.kind).to_lowercase(),
        unit: record.model.unit.label().to_string(),
        total_weight: record.model.total_weight,
        function_count: record.model.functions.len(),
        duration_ms: record.duration_ms,
        created_at_ms: record.created_at_ms,
        git: record.git.clone(),
        crate_rollup: bundle
            .crate_rollup
            .iter()
            .map(|(name, pct)| CrateShare {
                name: name.clone(),
                pct: *pct,
            })
            .collect(),
        hot_path: bundle.hot_path.clone(),
        hotspots: bundle.hotspots.clone(),
    };

    let run_path = runs.join(format!("{id}.json"));
    std::fs::write(&run_path, serde_json::to_string_pretty(&findings)?)
        .with_context(|| format!("writing {}", run_path.display()))?;

    // Upsert the index.
    let index_path = dir.join("index.json");
    let mut index: Vec<IndexEntry> = std::fs::read(&index_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    index.retain(|e| e.id != id);
    index.push(IndexEntry {
        id: id.to_string(),
        workload: findings.workload.clone(),
        backend: findings.backend.clone(),
        kind: findings.kind.clone(),
        unit: findings.unit.clone(),
        total_weight: findings.total_weight,
        duration_ms: findings.duration_ms,
        created_at_ms: findings.created_at_ms,
        hotspot_count: findings.hotspots.len(),
        top_function: findings.hotspots.first().map(|h| h.function.clone()),
        top_crate: findings.crate_rollup.first().map(|c| c.name.clone()),
        git: findings.git.clone(),
    });
    index.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    std::fs::write(&index_path, serde_json::to_string_pretty(&index)?)
        .with_context(|| format!("writing {}", index_path.display()))?;

    Ok(run_path)
}

// --- benchmark (the commit-gating metric) --------------------------------

pub struct BenchRequest {
    pub label: String,
    pub target: Target,
    pub runs: u32,
    pub repo_dir: Option<PathBuf>,
    /// Also measure detailed heap peak via dhat (slower: a second instrumented
    /// build + run). Peak RSS is always measured regardless.
    pub with_dhat: bool,
}

/// One benchmark point: wall-clock + peak RAM of the workload, tied to a commit.
/// `min_ms` is the low-noise time stat; `peak_rss_bytes` is the RAM-budget stat.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchRecord {
    pub label: String,
    pub runs: u32,
    pub min_ms: u64,
    pub median_ms: u64,
    pub mean_ms: u64,
    pub samples_ms: Vec<u64>,
    /// Peak resident set size across runs (real process RAM), via /usr/bin/time.
    #[serde(default)]
    pub peak_rss_bytes: u64,
    /// Optional dhat heap peak (at-t-gmax), when `--dhat` was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_peak_bytes: Option<u64>,
    pub created_at_ms: u128,
    pub git: GitInfo,
}

/// Build the target, run it `runs` times (output suppressed), timing each run and
/// capturing peak RSS. The profiler is NOT attached — this is the clean
/// comparable metric. Optionally also captures dhat heap peak.
pub fn run_bench(req: BenchRequest) -> Result<BenchRecord> {
    // Remember how to build a dhat-instrumented variant before consuming target.
    let dhat_target = if req.with_dhat {
        Some(with_dhat_feature(&req.target))
    } else {
        None
    };

    let target = language::resolve_target(req.target)?;
    let (path, args) = match &target {
        Target::Binary { path, args } => (path, args),
        _ => bail!("bench needs a binary or cargo example target"),
    };

    let time_bin = "/usr/bin/time";
    let use_time = std::path::Path::new(time_bin).exists();

    let mut samples_ms: Vec<u64> = Vec::new();
    let mut peak_rss_bytes: u64 = 0;
    for _ in 0..req.runs.max(1) {
        let start = Instant::now();
        let (success, rss) = if use_time {
            // `/usr/bin/time -l <bin> <args>` reports peak RSS on stderr (macOS);
            // wall-clock comes from our own timer to exclude time's overhead.
            let out = std::process::Command::new(time_bin)
                .arg("-l")
                .arg(path)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .output()
                .with_context(|| format!("running benchmark binary {}", path.display()))?;
            (
                out.status.success(),
                parse_rss(&String::from_utf8_lossy(&out.stderr)),
            )
        } else {
            let s = std::process::Command::new(path)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .with_context(|| format!("running benchmark binary {}", path.display()))?;
            (s.success(), 0)
        };
        if !success {
            bail!("benchmark run failed");
        }
        samples_ms.push(start.elapsed().as_millis() as u64);
        peak_rss_bytes = peak_rss_bytes.max(rss);
    }

    let mut sorted = samples_ms.clone();
    sorted.sort_unstable();
    let min_ms = sorted[0];
    let median_ms = sorted[sorted.len() / 2];
    let mean_ms = (samples_ms.iter().sum::<u64>() as f64 / samples_ms.len() as f64) as u64;

    let heap_peak_bytes = dhat_target.and_then(|t| run_dhat_peak(t).ok());

    let git = req.repo_dir.as_deref().map(git_info).unwrap_or_default();

    Ok(BenchRecord {
        label: req.label,
        runs: samples_ms.len() as u32,
        min_ms,
        median_ms,
        mean_ms,
        samples_ms,
        peak_rss_bytes,
        heap_peak_bytes,
        created_at_ms: now_ms(),
        git,
    })
}

/// Add the `dhat-heap` feature to a CargoExample target (for the optional heap
/// measurement). Non-example targets pass through unchanged.
fn with_dhat_feature(target: &Target) -> Target {
    match target {
        Target::CargoExample {
            dir,
            name,
            features,
            args,
        } => {
            let mut features = features.clone();
            if !features.iter().any(|f| f == "dhat-heap") {
                features.push("dhat-heap".to_string());
            }
            Target::CargoExample {
                dir: dir.clone(),
                name: name.clone(),
                features,
                args: args.clone(),
            }
        }
        other => other.clone(),
    }
}

/// Build the dhat-instrumented variant, run once, and parse the `At t-gmax: N
/// bytes` peak it prints on stderr.
fn run_dhat_peak(target: Target) -> Result<u64> {
    let built = language::resolve_target(target)?;
    let (path, args) = match &built {
        Target::Binary { path, args } => (path, args),
        _ => bail!("dhat target did not resolve to a binary"),
    };
    let out = std::process::Command::new(path)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("running dhat-instrumented binary")?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    parse_dhat_gmax(&stderr).ok_or_else(|| anyhow!("could not parse dhat t-gmax from output"))
}

/// First integer on the "maximum resident set size" line (`/usr/bin/time -l`,
/// macOS reports bytes).
fn parse_rss(stderr: &str) -> u64 {
    for line in stderr.lines() {
        if line.contains("maximum resident set size") {
            for tok in line.split_whitespace() {
                if let Ok(n) = tok.parse::<u64>() {
                    return n;
                }
            }
        }
    }
    0
}

/// Parse `dhat: At t-gmax: 237,557,994 bytes in ...` -> bytes.
fn parse_dhat_gmax(stderr: &str) -> Option<u64> {
    let line = stderr.lines().find(|l| l.contains("t-gmax"))?;
    let after = line.split("t-gmax:").nth(1)?;
    let digits: String = after
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(|c| *c != ',')
        .collect();
    digits.parse().ok()
}

/// Append a benchmark point to `<dir>/bench.json` (chronological series the
/// dashboard plots per commit).
pub fn append_bench(dir: &Path, rec: &BenchRecord) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("bench.json");
    let mut series: Vec<BenchRecord> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    series.push(rec.clone());
    std::fs::write(&path, serde_json::to_string_pretty(&series)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

// --- activity feed (what the auto-improve loop is doing) -----------------

/// One event in the loop's activity log: an iteration starting, or its verdict.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub ts_ms: u128,
    pub run: String,
    pub iteration: u32,
    /// working | accepted | rejected | error | info
    pub status: String,
    pub function: String,
    pub note: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
}

/// Append an activity event to `<dir>/activity.json` (capped to the last 300).
pub fn append_activity(dir: &Path, entry: ActivityEntry) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("activity.json");
    let mut log: Vec<ActivityEntry> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    log.push(entry);
    let n = log.len();
    if n > 300 {
        log.drain(0..n - 300);
    }
    std::fs::write(&path, serde_json::to_string_pretty(&log)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Newest cached profile id by mtime, if any.
pub fn latest_id() -> Option<String> {
    let dir = cache_dir();
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path.file_stem()?.to_string_lossy().to_string();
        let mtime = entry.metadata().ok()?.modified().ok()?;
        if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, stem));
        }
    }
    best.map(|(_, id)| id)
}
