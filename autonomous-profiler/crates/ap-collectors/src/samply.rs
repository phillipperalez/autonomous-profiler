//! samply backend (v0 reference, CPU). Unprivileged on macOS arm64 and identical
//! on x86 Linux/Windows, so it doubles as the x86 extension path.
//!
//! samply defers symbolication to load time, so we pass `--unstable-presymbolicate`
//! to get a `<out>.syms.json` sidecar that resolves every sampled address to a
//! symbol + file:line offline. We weight samples by `threadCPUDelta` (CPU time)
//! so idle/blocked threads don't masquerade as hot.
//!
//! Parsing is tolerant (navigates `serde_json::Value`) so samply format drift
//! degrades gracefully rather than panicking.

use ap_core::collector::{CollectOpts, Collector, Target};
use ap_core::model::{FoldedStack, FoldedStacks, Frame, RawProfile, SourceLoc};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;

pub struct SamplyCollector;

impl Collector for SamplyCollector {
    fn id(&self) -> &'static str {
        "samply"
    }

    fn available(&self) -> bool {
        which::which("samply").is_ok()
    }

    fn collect(&self, target: &Target, opts: &CollectOpts) -> Result<RawProfile> {
        let (path, args) = match target {
            Target::Binary { path, args } => (path, args),
            Target::CargoExample { .. } => {
                bail!("samply needs a built binary; resolve CargoExample via ap_core::language first")
            }
            Target::Pid(_) => bail!("samply pid attach not wired in v0"),
        };

        let dir = tempfile::tempdir().context("creating temp dir for samply output")?;
        let out = dir.path().join("profile.json");

        let mut cmd = Command::new("samply");
        cmd.arg("record")
            .arg("--save-only")
            .arg("--unstable-presymbolicate")
            .arg("--output")
            .arg(&out)
            .arg("--rate")
            .arg(opts.rate_hz.to_string())
            .arg("--")
            .arg(path)
            .args(args);

        let status = cmd
            .status()
            .context("failed to spawn samply (is it installed? `cargo install samply`)")?;
        if !status.success() {
            bail!("samply record exited with {status}");
        }

        let profile = std::fs::read(&out).with_context(|| format!("reading {}", out.display()))?;
        // Sidecar: samply replaces the `.json` extension with `.syms.json`.
        let sidecar_path = out.with_extension("syms.json");
        let symbols = std::fs::read(&sidecar_path)
            .ok()
            .map(|b| parse_sidecar(&b))
            .transpose()?
            .unwrap_or_default();

        let folded = parse_samply_profile(&profile, &symbols)?;
        Ok(RawProfile::Folded(folded))
    }
}

#[derive(Clone, Default)]
struct SymInfo {
    name: String,
    source: Option<SourceLoc>,
}

/// addr -> resolved symbol, built from the presymbolicate sidecar.
type SymIndex = HashMap<u64, SymInfo>;

fn parse_sidecar(bytes: &[u8]) -> Result<SymIndex> {
    let v: Value = serde_json::from_slice(bytes).context("parsing samply syms sidecar")?;
    let strings = v
        .get("string_table")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("sidecar missing string_table"))?;
    let get_str = |i: usize| strings.get(i).and_then(Value::as_str);

    let mut map: SymIndex = HashMap::new();
    let libs = v.get("data").and_then(Value::as_array);
    for lib in libs.into_iter().flatten() {
        let symtab = match lib.get("symbol_table").and_then(Value::as_array) {
            Some(s) => s,
            None => continue,
        };
        let known = match lib.get("known_addresses").and_then(Value::as_array) {
            Some(k) => k,
            None => continue,
        };
        for pair in known {
            let Some(pair) = pair.as_array() else { continue };
            let (Some(addr), Some(sym_idx)) = (
                pair.first().and_then(Value::as_u64),
                pair.get(1).and_then(Value::as_u64),
            ) else {
                continue;
            };
            let Some(entry) = symtab.get(sym_idx as usize) else {
                continue;
            };
            let name = entry
                .get("symbol")
                .and_then(Value::as_u64)
                .and_then(|i| get_str(i as usize))
                .unwrap_or("<unknown>")
                .to_string();
            // Inlined frame chain: frames[0] carries this symbol's file:line.
            let source = entry
                .get("frames")
                .and_then(Value::as_array)
                .and_then(|f| f.first())
                .and_then(|frame| {
                    let file = frame
                        .get("file")
                        .and_then(Value::as_u64)
                        .and_then(|i| get_str(i as usize))?;
                    let line = frame.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
                    Some(SourceLoc {
                        file: file.to_string(),
                        line,
                    })
                });
            // First lib wins on the rare cross-lib RVA collision.
            map.entry(addr).or_insert(SymInfo { name, source });
        }
    }
    Ok(map)
}

/// Leaf symbols that mean the thread was parked/blocked (off-CPU), not working.
fn is_idle_leaf(sym: &str) -> bool {
    const IDLE: &[&str] = &[
        "__psynch_cvwait",
        "__psynch_mutexwait",
        "__ulock_wait",
        "__ulock_wait2",
        "_pthread_cond_wait",
        "mach_msg",
        "mach_msg2_trap",
        "mach_msg_trap",
        "semaphore_wait_trap",
        "__workq_kernreturn",
        "kevent",
        "kevent_id",
        "__select",
        "poll",
        "__wait4",
    ];
    IDLE.iter().any(|i| sym.contains(i))
}

fn arr<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    v.get(key).and_then(Value::as_array)
}

fn as_index(v: &Value) -> Option<usize> {
    v.as_u64().map(|n| n as usize)
}

fn strings_for<'a>(thread: &'a Value, root: &'a Value) -> Option<&'a Vec<Value>> {
    thread
        .get("stringArray")
        .and_then(Value::as_array)
        .or_else(|| thread.get("stringTable").and_then(Value::as_array))
        .or_else(|| {
            root.get("shared")
                .and_then(|s| s.get("stringArray"))
                .and_then(Value::as_array)
        })
}

/// Lower the Firefox profiler JSON into folded stacks, resolving each frame via
/// the sidecar symbol index and weighting by CPU time.
fn parse_samply_profile(bytes: &[u8], symbols: &SymIndex) -> Result<FoldedStacks> {
    let root: Value = serde_json::from_slice(bytes).context("parsing samply JSON")?;
    let threads = root
        .get("threads")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("samply profile has no `threads` array"))?;

    let mut out = FoldedStacks::default();

    for thread in threads {
        let Some(strings) = strings_for(thread, &root) else {
            continue;
        };
        let get_str = |idx: usize| -> Option<&str> { strings.get(idx).and_then(Value::as_str) };

        let (Some(samples), Some(stack_table), Some(frame_table), Some(func_table)) = (
            thread.get("samples"),
            thread.get("stackTable"),
            thread.get("frameTable"),
            thread.get("funcTable"),
        ) else {
            continue;
        };

        let (Some(s_stack), Some(st_prefix), Some(st_frame), Some(ft_func)) = (
            arr(samples, "stack"),
            arr(stack_table, "prefix"),
            arr(stack_table, "frame"),
            arr(frame_table, "func"),
        ) else {
            continue;
        };
        let ft_address = arr(frame_table, "address");
        let s_cpu = arr(samples, "threadCPUDelta");
        let s_weight = arr(samples, "weight");
        let fn_name = match arr(func_table, "name") {
            Some(a) => a,
            None => continue,
        };

        for (i, stack_cell) in s_stack.iter().enumerate() {
            // Weight by CPU time. When the profile carries threadCPUDelta, a zero
            // delta means the thread was off-CPU (blocked/idle) for that sample —
            // drop it, so condvar waits don't masquerade as hot. Only when there
            // is no CPU-delta data at all do we fall back to a unit sample count.
            let weight = match s_cpu {
                Some(cpu) => cpu.get(i).and_then(Value::as_u64).unwrap_or(0),
                None => s_weight
                    .and_then(|w| w.get(i))
                    .and_then(Value::as_u64)
                    .unwrap_or(1),
            };
            if weight == 0 {
                continue;
            }

            let mut cursor = as_index(stack_cell);
            let mut frames: Vec<Frame> = Vec::new();
            let mut guard = 0;
            while let Some(sidx) = cursor {
                guard += 1;
                if guard > 4096 {
                    break;
                }
                if let Some(frame_idx) = st_frame.get(sidx).and_then(as_index) {
                    let addr = ft_address
                        .and_then(|a| a.get(frame_idx))
                        .and_then(Value::as_u64);
                    let resolved = addr.and_then(|a| symbols.get(&a));

                    let (symbol, source) = match resolved {
                        Some(s) => (s.name.clone(), s.source.clone()),
                        None => {
                            // Fall back to the (likely hex) funcTable name.
                            let name = ft_func
                                .get(frame_idx)
                                .and_then(as_index)
                                .and_then(|f| fn_name.get(f))
                                .and_then(as_index)
                                .and_then(get_str)
                                .unwrap_or("<unknown>")
                                .to_string();
                            (name, None)
                        }
                    };
                    frames.push(Frame { symbol, source });
                }
                cursor = st_prefix.get(sidx).and_then(as_index);
            }
            if frames.is_empty() {
                continue;
            }
            frames.reverse(); // walked leaf->root; store root->leaf
            // Hide off-CPU samples: a parked/blocking syscall at the leaf means the
            // thread was idle (rayon workers waiting for work, main thread joined).
            if frames.last().is_some_and(|f| is_idle_leaf(&f.symbol)) {
                continue;
            }
            out.stacks.push(FoldedStack { frames, weight });
            out.total_weight += weight;
        }
    }

    if out.stacks.is_empty() {
        bail!("samply profile produced no usable stacks (format mismatch?)");
    }
    Ok(out)
}
