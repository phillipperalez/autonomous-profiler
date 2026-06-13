# Autonomous Profiler

Point it at a Rust target, get back the **hot functions and memory-heavy call
sites** as a **token-budgeted, source-attributed bundle** an LLM can act on
directly — instead of a raw flamegraph it would burn thousands of tokens
decoding. Runs as a terminal tool (`ap`) and a local MCP server (`ap-mcp`), and
ships a fast SolidJS dashboard (`../findings-viz`) to present runs.

## Why

A flamegraph is for humans with a mouse. This produces a compressed context
bundle: ranked hotspots with cost %, crate rollup ("how much time is *inside
polars*"), the dominant hot path, source snippets, and why-hot tags — sized to a
token budget so an optimizer LLM gets signal, not noise.

## Quickstart

```bash
cargo install samply                 # CPU sampler (unprivileged on macOS arm64)
cargo build                          # build the workspace

# CPU: profile a cargo example and print a 6k-token bundle
./target/debug/ap run ../analyzer --example run_phase \
  --token-budget 6000 \
  --args ../analyzer/parquet/flights-1m.parquet

# inspect a cached run
./target/debug/ap hot  flights-1m-cpu --top 15 --crate-filter polars
./target/debug/ap context flights-1m-cpu --focus polars --format md

# Memory (dhat): build the target instrumented, run it, ingest the heap json
cd ../analyzer && cargo run --profile profiling --example run_phase \
  --features dhat-heap -- parquet/flights-1m.parquet            # writes dhat-heap.json
cd ../autonomous-profiler && ./target/debug/ap profile ../analyzer \
  --mode alloc --id flights-1m-alloc --dhat-json ../analyzer/dhat-heap.json
./target/debug/ap mem flights-1m-alloc --top 10
```

Each run caches to `.ap/profiles/<id>.json` and writes dashboard findings to
`data/{index.json,runs/<id>.json}` (override with `--findings-dir`). Every run is
stamped with the target repo's HEAD commit.

## Benchmark + auto-improve loop

`ap bench` runs the workload K times **without** the profiler and appends a
commit-tagged point to `data/bench.json`. `min_ms` is the low-noise stat — the
signal to gate "did it get faster" on.

```bash
ap bench ../analyzer --example run_phase --label flights-1m --runs 5 \
  --args ../analyzer/parquet/flights-1m.parquet
# prints: bench flights-1m @ 10e3625: min 365 ms ... | vs <prev>: -8.1% FASTER
```

The intended Claude-workflow loop (the target `analyzer/` is its own git repo so
commits land locally, never pushed):

```text
profile  → ap context  (find the hottest function + source)
edit      → optimize that function in ../analyzer
benchmark → ap bench    (same label/args → comparable)
gate      → if min_ms improved: git -C ../analyzer commit -am "..."   else revert
repeat
```

The dashboard's benchmark timeline marks each commit green (faster) / pink
(regressed) so the perf trend over commits is visible at a glance.

## Dashboard (live)

```bash
cd ../findings-viz && pnpm install && pnpm dev   # serves ../autonomous-profiler/data, port 5180
```

Polls every 2.5s, so runs and benchmarks appear **as the loop produces them**.
Top: the **benchmark timeline** (per-commit bars, colored by improvement). Below:
runs list (compare across runs) + per-run crate rollup, hot path, and an
expandable hotspot table with source snippets. Monokai, sharp/minimalist.

## MCP

```bash
cargo install --path crates/ap-mcp
claude mcp add -s user autonomous-profiler ap-mcp
```

Tools: `profile_target`, `list_hot_functions`, `memory_hotspots`, `context_bundle`.

## Architecture

```text
target ──▶ Collector ──▶ FoldedStacks ──▶ analyze ──▶ ProfileModel ──▶ compile ──▶ ContextBundle
          (backend)      (neutral IR)     (rank,       (functions,      (token-     (md / json /
                                           rollup,      graph)           budgeted)    findings)
                                           hot path)
```

- **`crates/ap-core`** — the backend-neutral brain: `model` (IR), `collector`
  (trait), `analyze` (ranking, crate rollup, hot path, tags), `compile` (the
  context compiler), `language`, `symbolize`.
- **`crates/ap-collectors`** — backends behind the `Collector` trait. **samply**
  (real, CPU) and **dhat** (real, memory) are wired; **perf** (x86/Linux) and
  **xctrace** (macOS-native) are scaffolded. `pipeline` is the shared
  orchestration + findings export.
- **`crates/ap-cli`** — `ap`.  **`crates/ap-mcp`** — `ap-mcp` (stdio JSON-RPC).

The backend-neutral folded-stack IR is what makes this portable: every collector
lowers into it, so adding `perf` for an x86 Linux box changes nothing downstream.

### Notes on the CPU path (samply)

samply symbolicates at load time, so we pass `--unstable-presymbolicate` to get a
`.syms.json` sidecar resolved offline (symbol + file:line per address). Samples
are weighted by `threadCPUDelta` and parked/blocking leaves (`__psynch_cvwait`,
`mach_msg`, …) are dropped, so idle worker threads don't masquerade as hot.

## Hack-day fan-out (next)

Each is an isolated module = clean parallel work: ① perf collector (x86/Linux)
· ② xctrace collector (mac CPU+alloc) · ③ addr2line source mapping for frames the
sampler left bare · ④ richer why-hot heuristics · ⑤ swap the hand-rolled MCP for
the `rmcp` SDK · ⑥ flamegraph/timeline view in the dashboard · ⑦ more languages
behind the `Language` trait.
