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

/// Instruction-set architecture of the disassembled binary. Drives which
/// instruction-classifier (and which SIMD vocabulary) we use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// ARM64 — Apple Silicon AND Linux aarch64 (same ISA: NEON, fadd, ldr/ldp…).
    Aarch64,
    /// x86-64 — SSE/AVX: xmm/ymm/zmm, packed *pd/*ps, scalar *sd/*ss, j*/cmp.
    X86_64,
    Other,
}

impl Arch {
    fn detect(disasm: &str) -> Arch {
        let head = disasm.lines().take(5).collect::<String>().to_lowercase();
        if head.contains("arm64") || head.contains("aarch64") {
            Arch::Aarch64
        } else if head.contains("x86-64") || head.contains("x86_64") || head.contains("i386") {
            Arch::X86_64
        } else {
            Arch::Other
        }
    }
    fn simd_name(&self) -> &'static str {
        match self {
            Arch::Aarch64 => "NEON",
            Arch::X86_64 => "SSE/AVX",
            Arch::Other => "SIMD",
        }
    }
}

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
    pub fn why_hot(&self, arch: Arch) -> String {
        if self.total == 0 {
            return "no instructions parsed".into();
        }
        if arch == Arch::Other {
            return format!(
                "{} instructions; mix classification not available for this architecture (ARM64 + x86-64 supported)",
                self.total
            );
        }
        let mut notes = Vec::new();
        if self.fdiv > 0 {
            notes.push(format!(
                "{} floating-point divide(s) (expensive; consider reciprocal/precompute)",
                self.fdiv
            ));
        }
        if self.simd == 0 && self.scalar_fp > 0 {
            notes.push(format!(
                "scalar FP, NO {} SIMD ({}% scalar-FP) — the hot loop did NOT vectorize; a SIMD rewrite (core::simd / intrinsics) may help",
                arch.simd_name(),
                self.pct(self.scalar_fp)
            ));
        } else if self.simd > 0 {
            notes.push(format!(
                "already {}-vectorized ({} vector ops)",
                arch.simd_name(),
                self.simd
            ));
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
    pub arch: Arch,
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
    let arch = Arch::detect(&dis);
    let mut lines = Vec::new();
    let mut mix = InstrMix::default();
    for l in dis.lines() {
        if is_insn_line(l) {
            match arch {
                Arch::Aarch64 => classify_arm(l, &mut mix),
                Arch::X86_64 => classify_x86(l, &mut mix),
                Arch::Other => mix.total += 1,
            }
            lines.push(l.to_string());
        }
    }
    if lines.is_empty() {
        bail!("objdump returned no instructions for {demangled}");
    }
    Ok(AsmReport {
        mangled,
        demangled,
        arch,
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

/// ARM64 (Apple Silicon + Linux aarch64): NEON vector operands, f-prefixed FP,
/// ldr/ldp loads, b./cbz branches.
fn classify_arm(l: &str, mix: &mut InstrMix) {
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

/// x86-64 (SSE/AVX). Best-effort heuristic — syntax varies (AT&T vs Intel):
/// - SIMD: ymm/zmm operands (always vector) or packed mnemonics (`addpd`,
///   `vmulps`, `paddd`).
/// - scalar-FP: scalar SSE (`addsd`, `mulss`, `divsd`, `cvtsi2sd`).
/// - mem: a mov-family op touching a memory operand (`(%…)` AT&T or `[…]` Intel).
/// - branch: `j*` / `cmp` / `test` / `call`.
fn classify_x86(l: &str, mix: &mut InstrMix) {
    mix.total += 1;
    let op = mnemonic(l);
    let div = op.contains("div");
    let packed = op.ends_with("pd") || op.ends_with("ps")
        || (op.starts_with('p') && op.len() > 2)
        || op.starts_with("vp");
    let scalar_sse = op.ends_with("sd") || op.ends_with("ss");
    let vec_reg = l.contains("ymm") || l.contains("zmm");
    if div && (packed || scalar_sse) {
        mix.fdiv += 1;
    }
    if vec_reg || (packed && (op.contains("add") || op.contains("mul") || op.contains("sub")
        || op.contains("div") || op.contains("fmadd") || op.contains("max") || op.contains("min")
        || op.contains("sqrt") || op.starts_with('p') || op.starts_with("vp")))
    {
        mix.simd += 1;
    } else if scalar_sse
        && (op.contains("add") || op.contains("mul") || op.contains("sub") || op.contains("div")
            || op.contains("sqrt") || op.contains("cvt") || op.contains("com"))
    {
        mix.scalar_fp += 1;
    } else if op.starts_with("mov") && (l.contains("(%") || l.contains('[')) {
        mix.mem += 1;
    } else if op.starts_with('j') || op == "call" || op == "cmp" || op == "test" {
        mix.branch += 1;
    }
}

/// Extract the instruction mnemonic (first token after the address/bytes).
fn mnemonic(l: &str) -> String {
    // objdump line: "<addr>:\t<bytes>\t<mnemonic> <operands>"
    l.split('\t')
        .nth(2)
        .or_else(|| l.split('\t').nth(1))
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase()
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
