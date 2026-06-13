export const meta = {
  name: 'polars-swarm-tournament',
  description: '3 diverse holistic optimization approaches to the polars sort/gather path, generated in parallel (read-only, no build), then evaluated SERIALLY on the warm clone (apply->build->fingerprint->quiesced bench->revert), winner committed on top of e30ab42. Hardened correctness oracle.',
  phases: [
    { title: 'Generate', detail: '3 parallel agents propose patches (distinct lenses), read-only' },
    { title: 'Evaluate', detail: 'serial: apply -> build -> fingerprint -> quiesced bench -> revert' },
    { title: 'Commit', detail: 'winner committed + dashboard refresh' },
  ],
}

const POLARS = '/Users/YOUR_USER/Source2/polars'
const BENCH = '/Users/YOUR_USER/Source2/polars-bench'
const AP_DIR = '/Users/YOUR_USER/Source2/autonomous-profiler'
const DATA = `${AP_DIR}/data`
const AP = `${AP_DIR}/target/debug/ap`
const BIN = `${BENCH}/target/profiling/examples/polars_workload`
const PARQUET = '/Users/YOUR_USER/Source2/analyzer/parquet/nyc-taxi.parquet'
const LABEL = 'polars-taxi-sort'
const RAM_BUDGET_MB = (args && args.ram_budget_mb) || 8192
const BASE_MS = (args && args.baseline_ms) || 474
const ANCHOR = { sum: 4683913106.640002, rows: 2964624, sortfp: 103271781267229.53125 }

const LENSES = [
  { key: 'locality', focus: 'REGISTER/CACHE LOCALITY: keep hot loop data in registers, cut redundant memory loads/stores, improve access patterns and data layout in the post-sort gather/take kernels so the inner loop stays cache- and register-resident.' },
  { key: 'dedup', focus: 'DEDUP COPIES & LOCALIZE WORK: find redundant buffer copies / clones / intermediate allocations across the sort+gather pipeline; fuse passes; borrow instead of clone; localize work so values are produced and consumed in one pass.' },
  { key: 'algorithmic', focus: 'ALGORITHMIC RESTRUCTURE: rethink the sort/gather approach itself — branchless or fewer-branch comparator, better small-run/partition thresholds, radix vs comparison tradeoffs, batched gather. A larger rewrite is fine as long as output is bit-for-bit identical.' },
]

const GEN_SCHEMA = {
  type: 'object',
  properties: {
    ok: { type: 'boolean' },
    lens: { type: 'string' },
    function: { type: 'string' },
    summary: { type: 'string' },
    scratch_dir: { type: 'string' },
    file_count: { type: 'number' },
    reason: { type: 'string' },
  },
  required: ['ok'],
}
const EVAL_SCHEMA = {
  type: 'object',
  properties: {
    built: { type: 'boolean' },
    fingerprint_ok: { type: 'boolean' },
    min_ms: { type: 'number' },
    rss_mb: { type: 'number' },
    pct: { type: 'number' },
    function: { type: 'string' },
    summary: { type: 'string' },
    reason: { type: 'string' },
  },
  required: ['built', 'fingerprint_ok'],
}

function genPrompt(i, lens) {
  return `Swarm agent ${i} proposing ONE holistic optimization to the POLARS sort/gather hot path. You are READ-ONLY on the polars clone — DO NOT modify ${POLARS}. Write your proposed changed files to a scratch dir instead. Use ABSOLUTE paths.

Your lens: ${lens.focus}

Context: workload = read nyc-taxi parquet, cast numerics to f64+sum, then SORT a Float64 column over ~3M rows; the post-sort GATHER dominates. Already-optimized (do NOT touch): resolve_chunked_idx (committed e30ab42). Other hot functions seen: take_values_and_validity_unchecked, gather_idx_array_unchecked, PrimitiveArray arr_from_iter_trusted, and the comparison sort core.

STEP 1 — study (read-only): explore ${POLARS}/crates/polars-core/src/chunked_array/ops/sort/** and the take/gather kernels (grep for the functions above under ${POLARS}/crates/polars-core and ${POLARS}/crates/polars-arrow). Understand inputs/outputs of the function(s) you'll change. Output MUST stay bit-for-bit identical (a downstream fingerprint of sum+rows+order will verify).

STEP 2 — design ONE change through your lens. It may span multiple functions/files (holistic is encouraged) but must be behavior-preserving.

STEP 3 — write the FULL new content of each file you change to scratch dir /tmp/swarm-cand-${i}/ , and a manifest at /tmp/swarm-cand-${i}/manifest.json that is a JSON array of {"dest":"<absolute path under ${POLARS}/crates/...>","src":"<absolute scratch file path you wrote>"}. Do NOT edit the polars clone itself.

Return {"ok":true,"lens":"${lens.key}","function":"<main fn>","summary":"<what+why, 1-2 sentences>","scratch_dir":"/tmp/swarm-cand-${i}","file_count":<n>} — or {"ok":false,"reason":"<why no viable change>"} if you can't find a safe one.`
}

function evalPrompt(i, cand) {
  return `Serially EVALUATE swarm candidate ${i} (lens=${cand.lens}, fn=${cand.function}) on the warm polars clone. Trustworthy timing — you are the only thing running. ABSOLUTE paths.

Apply: read /tmp/swarm-cand-${i}/manifest.json; for each {dest,src} copy src -> dest (overwrite the file in ${POLARS}). 
Build: cd ${BENCH} && cargo build --profile profiling --example polars_workload  (warm incremental). If it fails to compile, you MAY make MINIMAL edits to the applied files to fix compile errors WITHOUT changing the intended optimization or behavior; retry up to twice. If still failing → built=false.
Correctness: run ${BIN} ${PARQUET} 4 → parse "FINGERPRINT sum=<S> rows=<R> sortfp=<F>". PASS iff R==${ANCHOR.rows} AND |S-${ANCHOR.sum}|/${ANCHOR.sum}<1e-9 AND |F-${ANCHOR.sortfp}|/${ANCHOR.sortfp}<1e-9. If fail → fingerprint_ok=false.
Bench (only if built & fingerprint_ok): ${AP} bench ${BENCH} --example polars_workload --repo ${POLARS} --label ${LABEL} --runs 5 --findings-dir /tmp/ap-trial-swarm-${i} --args ${PARQUET} → read "min <N> ms ... peak RSS <R> MB". pct = 100*(N-${BASE_MS})/${BASE_MS}.
ALWAYS revert when done so the next candidate starts clean: cd ${POLARS} && git checkout -- crates && git clean -fd crates >/dev/null 2>&1.
Report activity: ${AP} activity --status info --run swarm --iter ${i} --function "${cand.function}" --note "lens=${cand.lens}: built=<b> fp=<ok> min=<N>ms pct=<pct>%" --findings-dir ${DATA}

Return {"built":<bool>,"fingerprint_ok":<bool>,"min_ms":<N or 0>,"rss_mb":<R or 0>,"pct":<pct or 0>,"function":"${cand.function}","summary":"${cand.summary}","reason":"<short verdict>"}.`
}

// PHASE 1 — parallel idea generation (read-only, no builds)
phase('Generate')
const cands = await parallel(
  LENSES.map((lens, idx) => () =>
    agent(genPrompt(idx + 1, lens), { label: `gen:${lens.key}`, phase: 'Generate', agentType: 'general-purpose', schema: GEN_SCHEMA })
      .then((r) => (r ? { ...r, i: idx + 1, lens: lens.key } : null))
  )
)
const viable = cands.filter((c) => c && c.ok && c.file_count > 0)
log(`generated ${viable.length}/${LENSES.length} viable candidates: ${viable.map((c) => c.lens + '(' + (c.function || '?') + ')').join(', ')}`)

// PHASE 2 — SERIAL evaluation on the warm clone (trustworthy timing)
phase('Evaluate')
const evaluated = []
for (const c of viable) {
  const r = await agent(evalPrompt(c.i, c), { label: `eval:${c.lens}`, phase: 'Evaluate', agentType: 'general-purpose', schema: EVAL_SCHEMA })
  if (r) {
    evaluated.push({ ...r, i: c.i, lens: c.lens })
    log(`eval ${c.lens}: built=${r.built} fp=${r.fingerprint_ok} min=${r.min_ms}ms pct=${r.pct}%`)
  }
}

// PHASE 3 — pick + commit winner
phase('Commit')
const passing = evaluated.filter((r) => r.built && r.fingerprint_ok && typeof r.min_ms === 'number' && r.min_ms > 0 && (r.rss_mb || 0) <= RAM_BUDGET_MB)
passing.sort((a, b) => a.min_ms - b.min_ms)
const winner = passing[0]
let committed = null
if (winner && winner.pct <= -3.0) {
  const cw = viable.find((c) => c.i === winner.i)
  const commitPrompt = `Commit the WINNING swarm candidate (lens=${winner.lens}, fn=${winner.function}, ${winner.pct}% faster) to the polars clone on top of e30ab42. ABSOLUTE paths.
Re-apply: read /tmp/swarm-cand-${winner.i}/manifest.json; copy each src->dest into ${POLARS}. (If a prior eval left compile-fix edits, the scratch is the source of truth — re-apply scratch, then if needed re-do the same minimal compile fixes.)
Build: cd ${BENCH} && cargo build --profile profiling --example polars_workload. Verify ${BIN} ${PARQUET} 4 fingerprint still PASSES (rows==${ANCHOR.rows}, sum within 1e-9 of ${ANCHOR.sum}, sortfp within 1e-9 of ${ANCHOR.sortfp}). If build or fingerprint fails, REVERT (git checkout -- crates) and return committed=false.
Commit: cd ${POLARS} && git add -A && git commit -m "perf(sort): ${winner.function} — swarm winner [${winner.lens} lens] (${LABEL} ${winner.pct}%) [autoperf]" ; sha=$(git rev-parse --short HEAD).
Canonical bench: ${AP} bench ${BENCH} --example polars_workload --repo ${POLARS} --label ${LABEL} --runs 5 --findings-dir ${DATA} --args ${PARQUET}.
Refresh findings: ${AP} run ${BENCH} --example polars_workload --repo ${POLARS} --focus polars --token-budget 6000 --findings-dir ${DATA} --args ${PARQUET}.
Activity: ${AP} activity --status accepted --run swarm --iter ${winner.i} --function "${winner.function}" --commit <sha> --note "swarm winner [${winner.lens}] ${LABEL} ${winner.pct}%" --findings-dir ${DATA}.
Return JSON {"committed":true,"commit":"<sha>"} or {"committed":false,"reason":"..."}.`
  const cr = await agent(commitPrompt, { label: `commit:${winner.lens}`, phase: 'Commit', agentType: 'general-purpose', schema: { type: 'object', properties: { committed: { type: 'boolean' }, commit: { type: 'string' }, reason: { type: 'string' } }, required: ['committed'] } })
  committed = cr
  log(committed && committed.committed ? `WINNER committed ${committed.commit} (${winner.lens}, ${winner.pct}%)` : `winner commit failed: ${committed && committed.reason}`)
} else {
  log(`no candidate beat baseline by >=3% (best: ${winner ? winner.pct + '% ' + winner.lens : 'none'})`)
  // record the outcome on the dashboard
}

return {
  baseline_ms: BASE_MS,
  generated: viable.map((c) => ({ lens: c.lens, function: c.function })),
  evaluated: evaluated.map((r) => ({ lens: r.lens, built: r.built, fingerprint_ok: r.fingerprint_ok, min_ms: r.min_ms, pct: r.pct })),
  winner: winner ? { lens: winner.lens, function: winner.function, pct: winner.pct, min_ms: winner.min_ms } : null,
  committed: committed && committed.committed ? committed.commit : null,
}
