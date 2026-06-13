---
name: autoperf
description: Run the autonomous-profiler improve loop on a Rust project. Profiles the real hot paths, then runs a gated, reversible swarm that commits only changes that are provably correct and measurably faster. Use when the user asks to "optimize", "speed up", "profile + improve", or "run autoperf" on a Rust crate/workspace that has (or can have) an autoperf.toml.
---

# autoperf — autonomous performance engineer for Rust

This skill drives the `ap` tool + a config-driven workflow to optimize **any** Rust project.
Everything it needs comes from an `autoperf.toml` in the target project — nothing is hardcoded.

## What it does
Profiles the primary workload → hands each swarm agent a ranked, source-attributed hotspot
brief → agents propose diverse-lens optimizations in parallel (read-only) → each is evaluated
**serially and quiesced** (correctness gate → benchmark primary + guards → revert) → the single
winner that clears the thresholds is committed **locally** (never pushed). Repeats until a round
finds nothing. A live dashboard (findings-viz) shows every attempt.

## Steps

1. **Locate the autonomous-profiler tool.**
   - Find the `ap` binary: try `which ap`; else look for `<autonomous-profiler>/target/release/ap`
     then `.../target/debug/ap`. If none exists, build it: `cargo build --release` in the
     autonomous-profiler checkout (and suggest `cargo install samply` if samply is missing).
   - Note the tool's data dir for the dashboard: `<autonomous-profiler>/data` (this is the
     `--findings-dir` the dashboard polls). Call it `DATA`.

2. **Locate + validate the target's config.** From the target project (or a path the user gives):
   - Run `ap config <path-or-dir> --json`. This parses + validates `autoperf.toml` and prints
     normalized JSON with absolute paths.
   - If it errors with "no autoperf.toml", run `ap init <dir>`, then tell the user to fill in the
     `gate` and at least one `[[workload]]`, and stop. Do **not** invent a config silently.
   - Parse the JSON. This object is `config` for the workflow.

3. **Pick run parameters** (from the user's request, with these defaults):
   - `mode`: `"swarm"` (N lenses per round) — use `"serial"` if the user wants a single proposer.
   - `max_rounds`: `3` (the loop stops early when a round is dry).

4. **Confirm scope before spending tokens.** A swarm spawns many agents and re-runs the
   workload many times. Briefly tell the user: target dir, primary workload, guard count, gate
   type, mode, max_rounds — then run it. If the target repo is dirty, warn first (the loop
   commits locally).

5. **Run the workflow.** Invoke the **Workflow** tool with:
   - `scriptPath`: `<autonomous-profiler>/../workflows/autoperf-improve.js` (the generic script
     committed at the repo's top-level `workflows/` dir).
   - `args`: `{ ap_bin: "<abs path to ap>", data_dir: "<DATA>", mode, max_rounds, config }`.

6. **Report.** When it returns, summarize: each committed win (round, lens, function, % faster,
   short SHA) and the final primary timing. Remind the user commits are **local only** — nothing
   was pushed. Point them at the dashboard (`findings-viz`, or the deployed URL) to see the
   timeline + activity feed update.

## Guardrails (the loop already enforces these — restate them to the user)
- **Correctness is ground truth.** Internal target → the `gate.test` command must pass.
  External/library target → a deterministic `FINGERPRINT=` line must match the baseline
  bit-for-bit. The baseline output is assumed correct unless a bug is *proven*.
- **Honest timing.** Benchmarks are min-of-N, taken serially/quiesced. A winner must beat the
  baseline by `min_improvement_pct`, no guard may regress past `guard_regression_pct`, and peak
  RSS must stay under `ram_budget_mb`.
- **Reversible.** Every candidate is reverted after evaluation; only the winner is committed, and
  only its changed source files are staged (never `autoperf.toml`). Nothing is ever pushed.

## Notes
- Only Rust is supported today (`target.lang = "rust"`).
- For a multi-crate workspace, set `target.dir` to the **workspace root** (examples build into
  `<root>/target`), and use `gate.test = "cargo test -p <crate>"`.
- See `workflows/README.md` for the example workflows and `examples/autoperf.toml` for an
  annotated config.
