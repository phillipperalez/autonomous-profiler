# Workflow examples — how to drive the autonomous-profiler

These are the real [Claude Code **Workflow**](https://docs.claude.com/en/docs/claude-code)
scripts that produced the results in this repo (the analyzer **−39%** and the polars
**−4%** wins). A Workflow script is JavaScript that orchestrates subagents deterministically
(`agent()`, `parallel()`, `phase()`, `log()`); you run one by passing the script to the
`Workflow` tool in Claude Code.

They're included as **reference templates** — anonymized to `/Users/YOUR_USER/...`. To run
one, edit the path/baseline constants at the top (marked below) for your machine, then hand
the script to the Workflow tool.

## The three patterns

### `01-improve-loop-until-dry.js` — the core loop
Single-edit serial loop on an **internal target** (your own crate, gated by its real test
suite). Each iteration: **profile → pick one hot function → edit → `cargo test` → quiesced
benchmark → commit if >3% faster (no guard-dataset regression, under RAM budget), else
revert.** Repeats until a round finds no win ("dry") or an iteration cap.
> The foundational pattern. Start here.

### `02-swarm-tournament-multiround.js` — the swarm meta-loop
Multi-round **tournament**: each round spawns **N agents in parallel**, each pursuing a
*different optimization lens* (algorithmic · data-locality · dedup-copies · data-shape ·
branch-flattening), proposing a patch **read-only**. Then the candidates are evaluated
**serially** (apply → test → quiesced bench → revert) so the deciding benchmark is never
skewed by parallel contention. The round's winner is committed, the baseline is raised, and
it goes again — until a round is dry.
> Best for finding wins a single-edit loop misses. The diverse lenses explore in parallel;
> the serial evaluation keeps the timing trustworthy.

### `03-swarm-external-target-fingerprint.js` — optimizing a dependency
Same swarm shape, but for an **external target you can't run a full test suite against**
(here: polars). Correctness is gated by a **deterministic output fingerprint** — the
workload prints `sum + row-count + order-sensitive checksum`, and a candidate is rejected
unless that matches the baseline bit-for-bit. Commits land in a local clone, never pushed.
> Use when the target is huge / third-party. The fingerprint is the correctness backstop
> that lets agents attempt bold rewrites safely.

## What to edit (top-of-file constants)

| constant | meaning |
|---|---|
| `AN` / `POLARS` / target dir | path to the git repo being optimized (commits land here, locally) |
| `BENCH` | the harness/build dir whose example the profiler runs |
| `AP_DIR` / `AP` | path to the built `ap` CLI (`autonomous-profiler/target/debug/ap`) |
| `DATA` | dashboard data dir the dashboard polls |
| `PARQUET*` / args | the workload input(s) |
| `baseline_*` (via `args`) | starting min-ms per dataset to beat |
| `ANCHOR` (script 03) | the baseline correctness fingerprint |

## The design principles these encode

- **Correctness is ground truth.** Internal target → the real test suite. External target →
  an output fingerprint that must match bit-for-bit. The baseline is assumed correct unless a
  bug is *proven*; output-changing edits are rejected, never silently committed.
- **Benchmark serially / quiesced.** Ideas may be *generated* in parallel, but the timing
  that decides a commit is always taken with nothing else running.
- **Every change is gated and reversible.** Commit only on a real improvement past a noise
  threshold, with no guard-dataset regression and under an explicit RAM budget; otherwise
  `git checkout` and move on.
- **Honest reverts > fake wins.** A "dry" round or a pile of reverts is the system working.

See the [top-level README](../README.md) for the architecture + UML diagrams, and
[`../highlights/`](../highlights/) for individual win writeups.
