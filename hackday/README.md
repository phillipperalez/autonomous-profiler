# Hack-day submission — autonomous-profiler

**One-line:** an autonomous performance engineer for Rust — it profiles a codebase, hands an
LLM a token-budgeted brief of the hot paths, then runs a gated loop that **optimizes the code
and commits only changes that are proven correct and measurably faster**, with a live
dashboard of every attempt.

🔴 **Live dashboard:** https://claude.autoperf.run · 📦 **Repo:** https://github.com/phillipperalez/autonomous-profiler

## What it achieved (all gated, all reversible)

| Target | Result | Correctness gate |
|---|---|---|
| **analyzer** (internal polars/ndarray analytics lib) | **−39% flights / −16% transactions**, 7 commits | full `cargo test` suite |
| **polars** (the OSS dataframe library itself) | **−4.0%** on the sort/gather path, 1 commit ([writeup](../highlights/polars-resolve-chunked-idx.md)) | output fingerprint over 2.96M rows |

The reverts matter as much as the wins: the loop refused every regression and every
unverified change — it even tried to out-hand-tune OpenBLAS/SIMD and *correctly failed*. No
fake wins.

## How it was built (this transcript)

[`build-transcript-2026-06-13.txt`](build-transcript-2026-06-13.txt) is the full, unedited
build session (home paths normalized to `~/`). It shows the whole arc:

1. Designing the profiler (`ap` CLI + MCP, samply/dhat collectors, backend-neutral IR, the
   token-budgeted context compiler).
2. The live SolidJS dashboard (benchmark/commit timeline + RAM + activity feed).
3. The **auto-improve loop** (profile → edit → `cargo test` → quiesced benchmark → commit/revert).
4. The **swarm tournament** (parallel diverse-lens proposals → serial trustworthy evaluation →
   commit winner) — including the multi-round "until-dry" meta-loop and the 5 optimization
   lenses (algorithmic, data-locality, dedup, data-shape, branch-flattening).
5. Hardening the correctness gates, the RAM-budget model, and deploying to GitHub Pages.

See the [top-level README](../README.md) for architecture + UML diagrams, and
[`highlights/`](../highlights/) for individual win writeups.
