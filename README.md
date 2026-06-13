# autonomous-profiler

An autonomous performance loop for Rust: profile a target, find the hot
functions and memory-heavy call sites, compress them into a token-budgeted
context bundle for an LLM, and drive an auto-improve loop that optimizes the
target and gates each change on correctness + benchmarks (time and RAM) before
committing.

## Layout

- **[autonomous-profiler/](autonomous-profiler/)** — the tool. A Rust workspace:
  - `ap` CLI + `ap-mcp` (local MCP server) over a backend-neutral core
  - collectors: **samply** (CPU) and **dhat** (memory) wired; perf/xctrace scaffolded
  - `ap bench` records per-commit time + peak RAM (RSS, optional dhat heap)
  - See [autonomous-profiler/README.md](autonomous-profiler/README.md) for usage.
- **[findings-viz/](findings-viz/)** — live SolidJS dashboard. Benchmark/commit
  timeline (time + RAM per commit, marked by improvement), runs list, hotspot
  table with source. Serves the profiler's `data/` dir; polls live.

## The bench target

The loop's target in development is a copy of an internal analytics library
(`analyzer/`, polars/ndarray heavy). It is **not** included here — it is a
separate repo and the dogfooding target. Point the profiler at any cargo
project: `ap run <project> --example <name> --args ...`.

## Quick start

```bash
cd autonomous-profiler && cargo install samply && cargo build
./target/debug/ap run <target> --example <name> --args <input>   # -> context bundle
cd ../findings-viz && pnpm install && pnpm dev                    # dashboard on :5180
```
