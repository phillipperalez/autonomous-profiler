//! Function-level disassembly + an instruction-mix "why-hot" heuristic.
//!
//! Deterministic: shells out to `nm` (resolve a symbol by its demangled name) and
//! `objdump` (disassemble just that one symbol — efficient, no full-binary dump).
//! Works for Mach-O (macOS, LLVM objdump `--disassemble-symbols=`) and ELF (Linux,
//! GNU objdump `--disassemble=`). Turns "this function is hot" into "…and here is
//! *why*: scalar (un-vectorized) FP / memory-bound / branch-heavy / has fdiv".

use crate::symbolize::demangle_symbol;
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Default, Clone)]
pub struct InstrMix {
    pub total: usize,
    pub simd: usize,
    pub scalar_fp: usize,
    pub mem: usize,
    pub branch: usize,
    pub fdiv: usize,
}

impl InstrMix {
    fn pct(&self, n: usize) -> usize {
        100 * n / self.total.max(1)
    }

    /// Heuristic read of what likely makes the function expensive.
    pub fn why_hot(&self) -> String {
        if self.total == 0 {
            return "no instructions parsed".into();
        }
        let mut notes = Vec::new();
        if self.fdiv > 0 {
            notes.push(format!(
                "{} floating-point divide(s) (expensive on ARM; consider reciprocal/precompute)",
                self.fdiv
            ));
        }
        if self.simd == 0 && self.scalar_fp > 0 {
            notes.push(format!(
                "scalar FP, NO NEON SIMD ({}% scalar-FP) — the hot loop did NOT vectorize; a SIMD rewrite (core::simd / NEON intrinsics) may help",
                self.pct(self.scalar_fp)
            ));
        } else if self.simd > 0 {
            notes.push(format!("already NEON-vectorized ({} vector ops)", self.simd));
        }
        if self.pct(self.mem) >= 40 {
            notes.push(format!(
                "memory-bound: {}% loads/stores — likely cache/bandwidth limited, not compute",
                self.pct(self.mem)
            ));
        }
        if self.pct(self.branch) >= 20 {
            notes.push(format!(
                "branch-heavy: {}% branches/compares — consider branchless/predicated code",
                self.pct(self.branch)
            ));
        }
        if notes.is_empty() {
            notes.push("balanced instruction mix".into());
        }
        notes.join("; ")
    }
}

pub struct AsmReport {
    pub mangled: String,
    pub demangled: String,
    pub lines: Vec<String>,
    pub mix: InstrMix,
}

/// Disassemble the first function whose demangled name contains `needle`.
pub fn disassemble_fn(bin: &Path, needle: &str) -> Result<AsmReport> {
    if !bin.exists() {
        bail!("binary not found: {}", bin.display());
    }
    let nm = Command::new("nm")
        .arg(bin)
        .output()
        .context("running `nm` (install binutils/llvm tools)")?;
    if !nm.status.success() {
        bail!("nm failed on {}", bin.display());
    }
    let nm_out = String::from_utf8_lossy(&nm.stdout);
    let needle_l = needle.to_lowercase();

    let mangled = nm_out
        .lines()
        .filter_map(|l| l.split_whitespace().nth(2)) // <addr> <type> <symbol>
        .find(|sym| demangle_one(sym).to_lowercase().contains(&needle_l))
        .ok_or_else(|| anyhow!("no function symbol matching '{needle}' in {}", bin.display()))?
        .to_string();
    let demangled = demangle_one(&mangled);

    let dis = run_objdump(bin, &mangled)?;
    let mut lines = Vec::new();
    let mut mix = InstrMix::default();
    for l in dis.lines() {
        if is_insn_line(l) {
            classify(l, &mut mix);
            lines.push(l.to_string());
        }
    }
    if lines.is_empty() {
        bail!("objdump returned no instructions for {demangled}");
    }
    Ok(AsmReport {
        mangled,
        demangled,
        lines,
        mix,
    })
}

fn run_objdump(bin: &Path, sym: &str) -> Result<String> {
    for flag in [
        format!("--disassemble-symbols={sym}"), // LLVM objdump
        format!("--disassemble={sym}"),         // GNU objdump
    ] {
        if let Ok(o) = Command::new("objdump").arg("-d").arg(&flag).arg(bin).output() {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout).to_string();
                if s.lines().any(is_insn_line) {
                    return Ok(s);
                }
            }
        }
    }
    bail!("objdump produced no disassembly for {sym} (is objdump installed?)")
}

/// An instruction line starts (after trimming) with a hex address then ':'.
fn is_insn_line(l: &str) -> bool {
    let t = l.trim_start();
    match t.split_once(':') {
        Some((addr, _)) => !addr.is_empty() && addr.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

fn classify(l: &str, mix: &mut InstrMix) {
    mix.total += 1;
    if l.contains("\tfdiv") || l.contains(" fdiv ") {
        mix.fdiv += 1;
    }
    if has_neon_operand(l) {
        mix.simd += 1;
    } else if has_any(
        l,
        &[
            "fadd", "fsub", "fmul", "fdiv", "fmadd", "fmsub", "fmla", "fnmul", "fcvt", "fcmp",
            "scvtf", "ucvtf",
        ],
    ) {
        mix.scalar_fp += 1;
    } else if has_any(l, &["ldr", "ldp", "ldur", "str", "stp", "stur"]) {
        mix.mem += 1;
    } else if has_any(l, &["b.", "cbz", "cbnz", "tbz", "tbnz", "cmp", "ccmp"]) {
        mix.branch += 1;
    }
}

fn has_any(l: &str, ops: &[&str]) -> bool {
    ops.iter()
        .any(|op| l.contains(&format!("\t{op}")) || l.contains(&format!(" {op} ")) || l.contains(&format!("\t{op} ")))
}

/// Detect a NEON vector operand like `v3.2d`, `v10.4s`, `v0.16b`.
fn has_neon_operand(l: &str) -> bool {
    let b = l.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'v' && b[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && b[j] == b'.' {
                let mut k = j + 1;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k < b.len() && matches!(b[k], b'b' | b'h' | b's' | b'd') {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// Demangle, handling Mach-O's extra leading `_`.
fn demangle_one(sym: &str) -> String {
    let d = demangle_symbol(sym);
    if d != sym {
        return d;
    }
    if let Some(stripped) = sym.strip_prefix('_') {
        let d2 = demangle_symbol(stripped);
        if d2 != stripped {
            return d2;
        }
    }
    sym.to_string()
}
