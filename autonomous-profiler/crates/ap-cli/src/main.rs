//! `ap` — the autonomous profiler CLI. Thin wrapper over `ap_collectors::pipeline`
//! and `ap_core::compile`.

use anyhow::{bail, Result};
use ap_collectors::pipeline::{self, ProfileRecord, ProfileRequest};
use ap_core::collector::{Mode, Target};
use ap_core::compile::{compile, CompileOpts};
use ap_core::model::{ProfileKind, Unit};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "ap", about = "Autonomous profiler: find hot paths, compress them for an LLM")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Profile a target and cache the result.
    Profile(ProfileArgs),
    /// Profile + emit a compressed context bundle in one shot (the demo command).
    Run(RunArgs),
    /// List the hottest functions from a cached profile.
    Hot(HotArgs),
    /// List the heaviest allocation sites from a cached alloc profile.
    Mem(MemArgs),
    /// Emit the token-budgeted context bundle from a cached profile.
    Context(ContextArgs),
    /// Benchmark the workload (no profiler attached) and append a commit-tagged
    /// point to bench.json. The commit-gating metric for the auto-improve loop.
    Bench(BenchArgs),
    /// Append an event to the dashboard activity feed (what the loop is doing).
    Activity(ActivityArgs),
    /// Disassemble a hot function from a binary + show an instruction-mix "why-hot"
    /// read (scalar-vs-NEON, memory-bound, branch-heavy, fdiv).
    Asm(AsmArgs),
    /// Write a starter `autoperf.toml` so the improve loop can run on this project.
    Init(InitArgs),
    /// Parse + validate `autoperf.toml` and print it normalized (paths resolved).
    /// `--json` feeds the workflow/skill; default is a human summary.
    Config(ConfigArgs),
}

#[derive(Args)]
struct InitArgs {
    /// Directory to write autoperf.toml into (default: current directory).
    #[arg(default_value = ".")]
    dir: PathBuf,
    /// Overwrite an existing autoperf.toml.
    #[arg(long, default_value_t = false)]
    force: bool,
}

#[derive(Args)]
struct ConfigArgs {
    /// Path to autoperf.toml or a directory containing it (default: ./autoperf.toml).
    path: Option<PathBuf>,
    /// Emit normalized JSON (for the workflow/skill) instead of a human summary.
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Copy, Clone, ValueEnum)]
enum ModeArg {
    Cpu,
    Alloc,
}
impl From<ModeArg> for Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Cpu => Mode::Cpu,
            ModeArg::Alloc => Mode::Alloc,
        }
    }
}

#[derive(Copy, Clone, ValueEnum)]
enum FormatArg {
    Md,
    Json,
}

#[derive(Args)]
struct ProfileArgs {
    /// Target directory (cargo project) or, with --bin, ignored.
    target: PathBuf,
    #[arg(long, default_value = "rust")]
    lang: String,
    #[arg(long, value_enum, default_value = "cpu")]
    mode: ModeArg,
    /// Profile a cargo example by name (built with the `profiling` profile).
    #[arg(long)]
    example: Option<String>,
    /// Profile a prebuilt binary instead of building an example.
    #[arg(long)]
    bin: Option<PathBuf>,
    /// Extra args passed to the target binary. Stops at the next --flag; use a
    /// trailing `--` if the target itself needs hyphenated args.
    #[arg(long, num_args = 0..)]
    args: Vec<String>,
    /// cargo features to enable when building the example.
    #[arg(long, num_args = 0..)]
    features: Vec<String>,
    /// Sampling rate (Hz) for CPU backends.
    #[arg(long, default_value_t = 1000)]
    rate: u32,
    /// CPU backend id.
    #[arg(long, default_value = "samply")]
    backend: String,
    /// For --mode alloc: the dhat-heap.json to ingest.
    #[arg(long)]
    dhat_json: Option<PathBuf>,
    /// Explicit cache id (default: derived from name + timestamp).
    #[arg(long)]
    id: Option<String>,
    /// Git repo to stamp the run with (default: the target dir). Use when the
    /// build dir differs from the edited repo (e.g. a polars harness).
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Where to write dashboard findings (runs/<id>.json + index.json).
    #[arg(long, default_value = "data")]
    findings_dir: PathBuf,
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    profile: ProfileArgs,
    #[arg(long, default_value_t = 8000)]
    token_budget: usize,
    #[arg(long)]
    focus: Option<String>,
    #[arg(long, default_value_t = 6)]
    ctx_lines: usize,
}

#[derive(Args)]
struct BenchArgs {
    /// Target directory (cargo project).
    target: PathBuf,
    #[arg(long)]
    example: Option<String>,
    #[arg(long)]
    bin: Option<PathBuf>,
    /// Args passed to the workload (e.g. the parquet path). Keep these fixed
    /// across commits so the benchmark is comparable.
    #[arg(long, num_args = 0..)]
    args: Vec<String>,
    #[arg(long, num_args = 0..)]
    features: Vec<String>,
    /// Number of timed runs; the minimum is reported as the benchmark.
    #[arg(long, default_value_t = 5)]
    runs: u32,
    /// Series label (default: example/bin name). Same label = same benchmark.
    #[arg(long)]
    label: Option<String>,
    /// Also measure detailed heap peak via dhat (slower; peak RSS is always measured).
    #[arg(long, default_value_t = false)]
    dhat: bool,
    /// Git repo to stamp the benchmark with (default: the target dir).
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long, default_value = "data")]
    findings_dir: PathBuf,
}

#[derive(Args)]
struct AsmArgs {
    /// Binary to disassemble (built with the `profiling` profile for symbols).
    bin: PathBuf,
    /// Function to find — matched as a substring of the demangled name.
    #[arg(long = "fn")]
    func: String,
    /// Max disassembly lines to print (use --full for everything).
    #[arg(long, default_value_t = 60)]
    max_lines: usize,
    /// Print the entire function disassembly.
    #[arg(long, default_value_t = false)]
    full: bool,
}

#[derive(Args)]
struct ActivityArgs {
    /// working | accepted | rejected | error | info
    #[arg(long)]
    status: String,
    #[arg(long)]
    function: String,
    /// Free text: what's being tried, the verdict + reason, deltas, tradeoff.
    #[arg(long, default_value = "")]
    note: String,
    #[arg(long, default_value = "loop")]
    run: String,
    #[arg(long, default_value_t = 0)]
    iter: u32,
    #[arg(long, default_value = "")]
    commit: String,
    #[arg(long, default_value = "data")]
    findings_dir: PathBuf,
}

#[derive(Args)]
struct HotArgs {
    /// Cached profile id (default: most recent).
    id: Option<String>,
    #[arg(long, default_value_t = 15)]
    top: usize,
    /// Restrict to a crate substring.
    #[arg(long)]
    crate_filter: Option<String>,
}

#[derive(Args)]
struct MemArgs {
    id: Option<String>,
    #[arg(long, default_value_t = 15)]
    top: usize,
}

#[derive(Args)]
struct ContextArgs {
    id: Option<String>,
    #[arg(long, default_value_t = 8000)]
    token_budget: usize,
    #[arg(long)]
    focus: Option<String>,
    #[arg(long, value_enum, default_value = "md")]
    format: FormatArg,
    #[arg(long, default_value_t = 6)]
    ctx_lines: usize,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Profile(a) => {
            let (id, _) = do_profile(&a)?;
            println!("cached profile: {id}");
        }
        Cmd::Run(a) => {
            let (_, record) = do_profile(&a.profile)?;
            let opts = CompileOpts {
                token_budget: a.token_budget,
                focus: a.focus,
                source_ctx_lines: a.ctx_lines,
                source_roots: record.source_roots.clone(),
                ..Default::default()
            };
            println!("{}", compile(&record.model, &opts).to_markdown());
        }
        Cmd::Hot(a) => {
            let record = load(a.id)?;
            print_hot(&record, a.top, a.crate_filter.as_deref());
        }
        Cmd::Mem(a) => {
            let record = load(a.id)?;
            if record.model.kind != ProfileKind::Alloc {
                eprintln!("note: this profile is CPU, not alloc; showing self-time anyway");
            }
            print_hot(&record, a.top, None);
        }
        Cmd::Context(a) => {
            let record = load(a.id)?;
            let opts = CompileOpts {
                token_budget: a.token_budget,
                focus: a.focus,
                source_ctx_lines: a.ctx_lines,
                source_roots: record.source_roots.clone(),
                ..Default::default()
            };
            let bundle = compile(&record.model, &opts);
            match a.format {
                FormatArg::Md => println!("{}", bundle.to_markdown()),
                FormatArg::Json => println!("{}", serde_json::to_string_pretty(&bundle)?),
            }
        }
        Cmd::Bench(a) => do_bench(&a)?,
        Cmd::Activity(a) => {
            let entry = pipeline::ActivityEntry {
                ts_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
                run: a.run.clone(),
                iteration: a.iter,
                status: a.status.clone(),
                function: a.function.clone(),
                note: a.note.clone(),
                commit: a.commit.clone(),
            };
            pipeline::append_activity(&a.findings_dir, entry)?;
        }
        Cmd::Asm(a) => {
            let r = ap_core::disasm::disassemble_fn(&a.bin, &a.func)?;
            println!("# {}  [{:?}]", r.demangled, r.arch);
            let m = &r.mix;
            println!(
                "instructions: {} · SIMD: {} · scalar-FP: {} · mem(ld/st): {} · branch/cmp: {} · fdiv: {}",
                m.total, m.simd, m.scalar_fp, m.mem, m.branch, m.fdiv
            );
            println!("why-hot: {}\n", m.why_hot(r.arch));
            let n = if a.full { r.lines.len() } else { a.max_lines.min(r.lines.len()) };
            for line in &r.lines[..n] {
                println!("{}", line.trim_end());
            }
            if n < r.lines.len() {
                println!("… {} more lines (--full for all)", r.lines.len() - n);
            }
        }
        Cmd::Init(a) => do_init(&a)?,
        Cmd::Config(a) => do_config(&a)?,
    }
    Ok(())
}

fn do_init(a: &InitArgs) -> Result<()> {
    let dest = a.dir.join("autoperf.toml");
    if dest.exists() && !a.force {
        bail!("{} already exists (use --force to overwrite)", dest.display());
    }
    std::fs::write(&dest, ap_core::config::starter_toml())?;
    println!("wrote {}", dest.display());
    println!("Edit it (gate + at least one [[workload]]), then `ap config` to validate.");
    Ok(())
}

fn do_config(a: &ConfigArgs) -> Result<()> {
    let path = ap_core::config::AutoperfConfig::locate(a.path.as_deref())?;
    let cfg = ap_core::config::AutoperfConfig::load(&path)?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
        return Ok(());
    }
    let p = cfg.primary_index();
    let gate = match (&cfg.gate.test, cfg.gate.fingerprint) {
        (Some(t), _) => format!("test: `{t}`"),
        (None, true) => "fingerprint (deterministic output match)".to_string(),
        _ => "(none)".to_string(),
    };
    println!("autoperf.toml ✓  ({})", path.display());
    println!("  target : {} (lang {})", cfg.target.dir, cfg.target.lang);
    if let Some(r) = &cfg.target.repo {
        if r != &cfg.target.dir {
            println!("  repo   : {r}");
        }
    }
    println!("  gate   : {gate}");
    println!(
        "  budget : RAM {} MB · win ≥ {:.1}% · guard regress < {:.1}%",
        cfg.improve.ram_budget_mb, cfg.improve.min_improvement_pct, cfg.improve.guard_regression_pct
    );
    println!("  lenses : {}", cfg.improve.lenses.join(", "));
    if !cfg.improve.off_limits.is_empty() {
        println!("  off-limits: {}", cfg.improve.off_limits.join(", "));
    }
    println!("  workloads:");
    for (i, w) in cfg.workloads.iter().enumerate() {
        let kind = w
            .example
            .as_ref()
            .map(|e| format!("example {e}"))
            .or_else(|| w.bin.as_ref().map(|b| format!("bin {b}")))
            .unwrap_or_default();
        let role = if i == p { "PRIMARY (optimize)" } else { "guard" };
        println!(
            "    - {} [{role}] · {kind} · runs {} · args {:?}",
            w.label, w.runs, w.args
        );
    }
    Ok(())
}

fn do_bench(a: &BenchArgs) -> Result<()> {
    let (target, label) = if let Some(bin) = &a.bin {
        (
            Target::Binary {
                path: bin.clone(),
                args: a.args.clone(),
            },
            bin.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "bin".into()),
        )
    } else if let Some(example) = &a.example {
        (
            Target::CargoExample {
                dir: a.target.clone(),
                name: example.clone(),
                features: a.features.clone(),
                args: a.args.clone(),
            },
            example.clone(),
        )
    } else {
        bail!("specify --example <name> or --bin <path>");
    };
    let label = a.label.clone().unwrap_or(label);

    let req = pipeline::BenchRequest {
        label: label.clone(),
        target,
        runs: a.runs,
        repo_dir: Some(a.repo.clone().unwrap_or_else(|| a.target.clone())),
        with_dhat: a.dhat,
    };

    // Previous point for this commit-series label, to report a delta.
    let prev = read_bench(&a.findings_dir)
        .into_iter()
        .filter(|r| r.label == label)
        .next_back();

    let rec = pipeline::run_bench(req)?;
    let path = pipeline::append_bench(&a.findings_dir, &rec)?;

    let commit = if rec.git.short.is_empty() {
        "(no git)".to_string()
    } else {
        format!("{}{}", rec.git.short, if rec.git.dirty { "*" } else { "" })
    };
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    println!(
        "bench {label} @ {commit}: min {} ms · median {} ms · mean {} ms · peak RSS {:.0} MB{} ({} runs)",
        rec.min_ms,
        rec.median_ms,
        rec.mean_ms,
        mb(rec.peak_rss_bytes),
        rec.heap_peak_bytes
            .map(|h| format!(" · heap {:.0} MB", mb(h)))
            .unwrap_or_default(),
        rec.runs
    );
    if let Some(p) = prev {
        let delta = rec.min_ms as f64 - p.min_ms as f64;
        let pct = if p.min_ms > 0 {
            100.0 * delta / p.min_ms as f64
        } else {
            0.0
        };
        let verdict = if delta < 0.0 { "FASTER" } else { "slower" };
        let rss_pct = if p.peak_rss_bytes > 0 {
            100.0 * (rec.peak_rss_bytes as f64 - p.peak_rss_bytes as f64) / p.peak_rss_bytes as f64
        } else {
            0.0
        };
        println!(
            "  vs {} ({} ms, {:.0} MB): {:+.1}% time {} · {:+.1}% RAM",
            if p.git.short.is_empty() { "prev".into() } else { p.git.short.clone() },
            p.min_ms,
            mb(p.peak_rss_bytes),
            pct,
            verdict,
            rss_pct
        );
    }
    eprintln!("appended {}", path.display());
    Ok(())
}

fn read_bench(dir: &std::path::Path) -> Vec<pipeline::BenchRecord> {
    std::fs::read(dir.join("bench.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn do_profile(a: &ProfileArgs) -> Result<(String, ProfileRecord)> {
    let mode: Mode = a.mode.into();

    let (target, label, mut source_roots) = if let Some(bin) = &a.bin {
        let roots = bin
            .parent()
            .map(|p| vec![p.to_path_buf()])
            .unwrap_or_default();
        (
            Target::Binary {
                path: bin.clone(),
                args: a.args.clone(),
            },
            bin.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "bin".into()),
            roots,
        )
    } else if let Some(example) = &a.example {
        (
            Target::CargoExample {
                dir: a.target.clone(),
                name: example.clone(),
                features: a.features.clone(),
                args: a.args.clone(),
            },
            example.clone(),
            vec![a.target.clone()],
        )
    } else if mode == Mode::Alloc {
        // alloc only ingests json; target is unused but roots help snippets.
        (Target::Pid(0), "alloc".into(), vec![a.target.clone()])
    } else {
        bail!("specify --example <name> or --bin <path>");
    };
    source_roots.push(PathBuf::from("."));

    let _lang = ap_core::language::Language::parse(&a.lang)?;

    // Include a dataset hint (last arg's file name) so runs are distinguishable.
    let dataset = a
        .args
        .last()
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string());
    let workload = match dataset {
        Some(d) => format!("{label} · {d} ({})", mode_label(mode)),
        None => format!("{label} ({})", mode_label(mode)),
    };
    let req = ProfileRequest {
        workload,
        target,
        mode,
        rate_hz: a.rate,
        backend_id: a.backend.clone(),
        dhat_json: a.dhat_json.clone(),
        source_roots,
        repo_dir: Some(a.repo.clone().unwrap_or_else(|| a.target.clone())),
    };

    let record = pipeline::run_profile(req)?;
    let id = a.id.clone().unwrap_or_else(|| derive_id(&label));
    let path = pipeline::save(&id, &record)?;
    eprintln!(
        "profiled {} functions ({} {} in {} ms); saved {}",
        record.model.functions.len(),
        record.model.total_weight,
        record.model.unit.label(),
        record.duration_ms,
        path.display()
    );
    match pipeline::write_findings(&a.findings_dir, &id, &record) {
        Ok(p) => eprintln!("findings: {}", p.display()),
        Err(e) => eprintln!("warning: findings export failed: {e:#}"),
    }
    Ok((id, record))
}

fn mode_label(m: Mode) -> &'static str {
    match m {
        Mode::Cpu => "cpu",
        Mode::Alloc => "alloc",
    }
}

fn derive_id(label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        % 1_000_000;
    format!("{slug}-{ms}")
}

fn load(id: Option<String>) -> Result<ProfileRecord> {
    let id = match id {
        Some(id) => id,
        None => pipeline::latest_id()
            .ok_or_else(|| anyhow::anyhow!("no cached profiles in .ap/profiles"))?,
    };
    pipeline::load(&id)
}

fn print_hot(record: &ProfileRecord, top: usize, crate_filter: Option<&str>) {
    let model = &record.model;
    let unit = model.unit;
    println!(
        "{:>6} {:>6}  {:<16} {:<8} function   [{}]",
        "self%", "total%", "crate", "where", unit_header(unit)
    );
    let mut shown = 0;
    for f in &model.functions {
        if shown >= top {
            break;
        }
        if let Some(cf) = crate_filter {
            if !f.crate_name.contains(cf) {
                continue;
            }
        }
        let where_ = f
            .source
            .as_ref()
            .map(|s| {
                let file = s
                    .file
                    .rsplit('/')
                    .next()
                    .unwrap_or(&s.file)
                    .to_string();
                format!("{}:{}", file, s.line)
            })
            .unwrap_or_default();
        println!(
            "{:>6.1} {:>6.1}  {:<16} {:<8} {}  {}",
            model.self_pct(f),
            model.total_pct(f),
            truncate(&f.crate_name, 16),
            f.self_weight,
            truncate(&f.demangled, 70),
            where_
        );
        shown += 1;
    }
}

fn unit_header(u: Unit) -> &'static str {
    match u {
        Unit::Samples => "samples",
        Unit::Bytes => "bytes",
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n.saturating_sub(1)])
    }
}
