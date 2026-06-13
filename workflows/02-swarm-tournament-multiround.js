export const meta = {
  name: 'analyzer-swarm-until-dry',
  description: 'Multi-round 5-lens swarm on philtool_core: each round generates 5 diverse patches in parallel (algorithmic/locality/dedup/data-shape/branch-flatten), evaluates SERIALLY (cargo test -> quiesced bench flights+txn -> revert), commits the winner, updates baseline, repeats until a round finds no winner (cap 3 rounds).',
  phases: [{ title: 'Rounds', detail: 'per round: 5 parallel proposals -> serial gated eval -> commit winner' }],
}

const AN = '/Users/YOUR_USER/Source2/analyzer'
const AP_DIR = '/Users/YOUR_USER/Source2/autonomous-profiler'
const DATA = `${AP_DIR}/data`
const AP = `${AP_DIR}/target/debug/ap`
const PF = `${AN}/parquet/flights-1m.parquet`
const PT = `${AN}/parquet/transactions_train.parquet`
const RAM = (args && args.ram_budget_mb) || 8192
const MAX_ROUNDS = (args && args.max_rounds) || 3
let baseF = (args && args.baseline_flights) || 235
let baseT = (args && args.baseline_txn) || 1296
const done = ['build_top_anomalies', 'calculate_covariance_matrix', 'compute_pearson_correlation', 'detect_multivariate_outliers', 'detect_univariate_outliers', 'ExactQuantileCalculator::quantiles', 'extract_numeric_matrix']

const LENSES = [
  { key: 'algorithmic', focus: 'ALGORITHMIC: reduce asymptotic/constant work — avoid O(n^2) where O(n log n)/O(n) suffices, hoist loop invariants, short-circuit, batch/vectorize. Larger rewrites OK (tests verify).' },
  { key: 'locality', focus: 'DATA LOCALITY: keep hot numeric loops register/cache-resident; cut redundant loads/stores; sequential access; reduce working-set size in the inner loop.' },
  { key: 'dedup', focus: 'DEDUP COPIES: remove redundant Series/DataFrame clones, intermediate Vec allocs, repeated column materialization/recompute; fuse passes; borrow not clone; compute-once.' },
  { key: 'datashape', focus: 'DATA SHAPE: restructure layout for the hot path — struct-of-arrays vs array-of-structs, pack fields, use smaller/aligned numeric types, remove pointer-chasing/indirection, contiguous buffers.' },
  { key: 'branchflat', focus: 'BRANCH FLATTENING: make hot inner loops branchless — replace per-element if/else with arithmetic/predication (e.g. `acc += (cond) as T`), hoist conditionals out of loops, remove bounds/null checks from the steady-state path where provably safe.' },
]

const GEN = { type: 'object', properties: { ok: { type: 'boolean' }, lens: { type: 'string' }, function: { type: 'string' }, summary: { type: 'string' }, scratch_dir: { type: 'string' }, file_count: { type: 'number' }, reason: { type: 'string' } }, required: ['ok'] }
const EV = { type: 'object', properties: { built: { type: 'boolean' }, tests_ok: { type: 'boolean' }, min_f: { type: 'number' }, min_t: { type: 'number' }, rss_mb: { type: 'number' }, pct_f: { type: 'number' }, pct_t: { type: 'number' }, function: { type: 'string' }, summary: { type: 'string' }, reason: { type: 'string' } }, required: ['built', 'tests_ok'] }

function genP(round, i, lens, doneList, bF, bT) {
  return `Round ${round} swarm agent (lens=${lens.key}) proposing ONE holistic optimization to philtool_core (pure-Rust polars/ndarray analytics). READ-ONLY on ${AN} — DO NOT modify it; write proposed files to scratch. ABSOLUTE paths. Only target ${AN}/philtool-core/src/**.

Lens: ${lens.focus}

Baselines: flights ${bF}ms, transactions ${bT}ms. OFF-LIMITS (already optimized — do NOT touch): ${doneList.join(', ')}. Pick a DIFFERENT hot, editable function where your lens applies.

STEP 1 (read-only): the newest ${DATA}/runs/*.json has ranked philtool_core hotspots (file:line). Read it; explore ${AN}/philtool-core/src/** (analysis/anomaly, bivariate, segmentation, temporal, univariate, etc.). Understand inputs/outputs — output MUST stay identical (88-test suite verifies).
STEP 2: design ONE behavior-preserving change through your lens (may span files).
STEP 3: write FULL new content of each changed file to /tmp/swarm-an-r${round}-${i}/ + a manifest /tmp/swarm-an-r${round}-${i}/manifest.json = JSON array of {"dest":"<abs path under ${AN}/philtool-core/src/...>","src":"<abs scratch file>"}.

Return {"ok":true,"lens":"${lens.key}","function":"<fn>","summary":"<what+why>","scratch_dir":"/tmp/swarm-an-r${round}-${i}","file_count":<n>} or {"ok":false,"reason":"<why none>"}.`
}

function evP(round, i, c, bF, bT) {
  return `Round ${round}: serially EVALUATE candidate (lens=${c.lens}, fn=${c.function}) on the warm analyzer clone. You're the only thing running — timing trustworthy. ABSOLUTE paths.
Apply: read /tmp/swarm-an-r${round}-${i}/manifest.json; copy each src->dest into ${AN} (overwrite).
Correctness: cd ${AN} && cargo test -p philtool-core. You MAY make MINIMAL compile fixes preserving intent (retry up to twice). tests_ok = all pass; if build broken or any test fails -> tests_ok=false (skip bench).
Bench (only if tests_ok): ${AP} bench ${AN} --example run_phase --label flights-1m --runs 5 --findings-dir /tmp/ap-trial-r${round}-${i} --args ${PF} (min_f, rss); ${AP} bench ${AN} --example run_phase --label transactions_train --runs 3 --findings-dir /tmp/ap-trial-r${round}-${i} --args ${PT} (min_t). pct_f=100*(min_f-${bF})/${bF}; pct_t=100*(min_t-${bT})/${bT}.
ALWAYS revert: cd ${AN} && git checkout -- philtool-core && git clean -fd philtool-core >/dev/null 2>&1.
Activity: ${AP} activity --status info --run swarm-r${round} --iter ${i} --function "${c.function}" --note "lens=${c.lens}: tests=<ok> flights=<pct_f>% txn=<pct_t>%" --findings-dir ${DATA}
Return {"built":<b>,"tests_ok":<b>,"min_f":<N|0>,"min_t":<N|0>,"rss_mb":<R|0>,"pct_f":<p|0>,"pct_t":<p|0>,"function":"${c.function}","summary":"${c.summary}","reason":"<verdict>"}.`
}

phase('Rounds')
const roundResults = []
for (let round = 1; round <= MAX_ROUNDS; round++) {
  log(`round ${round}: baselines flights ${baseF}ms / txn ${baseT}ms | done=[${done.join(', ')}]`)
  const cands = (await parallel(
    LENSES.map((lens, idx) => () =>
      agent(genP(round, idx + 1, lens, done, baseF, baseT), { label: `r${round}:gen:${lens.key}`, phase: 'Rounds', agentType: 'general-purpose', schema: GEN })
        .then((r) => (r ? { ...r, i: idx + 1, lens: lens.key } : null))
    )
  )).filter((c) => c && c.ok && c.file_count > 0 && !done.includes(c.function))
  log(`round ${round}: ${cands.length} viable: ${cands.map((c) => c.lens + '(' + (c.function || '?') + ')').join(', ')}`)

  const evald = []
  for (const c of cands) {
    const r = await agent(evP(round, c.i, c, baseF, baseT), { label: `r${round}:eval:${c.lens}`, phase: 'Rounds', agentType: 'general-purpose', schema: EV })
    if (r) { evald.push({ ...r, i: c.i, lens: c.lens }); log(`r${round} eval ${c.lens}: tests=${r.tests_ok} flights=${r.pct_f}% txn=${r.pct_t}%`) }
  }

  const passing = evald.filter((r) => r.tests_ok && typeof r.min_f === 'number' && r.min_f > 0 && (r.rss_mb || 0) <= RAM && (r.pct_t || 0) < 2.0 && r.pct_f <= -3.0)
  passing.sort((a, b) => a.min_f - b.min_f)
  const w = passing[0]
  let committed = null
  if (w) {
    const cp = `Commit round ${round} WINNER (lens=${w.lens}, fn=${w.function}, flights ${w.pct_f}%, txn ${w.pct_t}%) to ${AN}. ABSOLUTE paths.
Re-apply /tmp/swarm-an-r${round}-${w.i}/manifest.json (copy src->dest). cd ${AN} && cargo test -p philtool-core MUST pass (redo minimal compile fix if needed); if fail, git checkout -- philtool-core and return committed=false.
Commit: cd ${AN} && git add -A && git commit -m "perf(philtool_core): ${w.function} — swarm r${round} [${w.lens}] (flights ${w.pct_f}%) [autoperf]"; sha=$(git rev-parse --short HEAD).
Canonical bench -> dashboard: ${AP} bench ${AN} --example run_phase --label flights-1m --runs 5 --findings-dir ${DATA} --args ${PF}; ${AP} bench ${AN} --example run_phase --label transactions_train --runs 3 --findings-dir ${DATA} --args ${PT}.
Refresh findings: ${AP} run ${AN} --example run_phase --focus philtool_core --token-budget 6000 --findings-dir ${DATA} --args ${PF}.
Activity: ${AP} activity --status accepted --run swarm-r${round} --iter ${w.i} --function "${w.function}" --commit <sha> --note "r${round} winner [${w.lens}] flights ${w.pct_f}%" --findings-dir ${DATA}.
Return {"committed":true,"commit":"<sha>","min_f":<canonical flights min>,"min_t":<canonical txn min>} or {"committed":false,"reason":"..."}.`
    committed = await agent(cp, { label: `r${round}:commit:${w.lens}`, phase: 'Rounds', agentType: 'general-purpose', schema: { type: 'object', properties: { committed: { type: 'boolean' }, commit: { type: 'string' }, min_f: { type: 'number' }, min_t: { type: 'number' }, reason: { type: 'string' } }, required: ['committed'] } })
  }

  if (committed && committed.committed) {
    done.push(w.function)
    if (typeof committed.min_f === 'number' && committed.min_f > 0) baseF = committed.min_f
    if (typeof committed.min_t === 'number' && committed.min_t > 0) baseT = committed.min_t
    roundResults.push({ round, winner: { lens: w.lens, function: w.function, pct_f: w.pct_f, pct_t: w.pct_t }, commit: committed.commit })
    log(`round ${round}: COMMIT ${committed.commit} — ${w.function} [${w.lens}] flights ${w.pct_f}%; new baseline flights ${baseF}ms`)
  } else {
    roundResults.push({ round, winner: null, dry: true })
    log(`round ${round}: DRY — no candidate beat baseline by >=3% without regression. Stopping.`)
    break
  }
}

const wins = roundResults.filter((r) => r.winner)
return {
  target: 'analyzer',
  rounds_run: roundResults.length,
  wins: wins.map((r) => ({ round: r.round, lens: r.winner.lens, function: r.winner.function, pct_f: r.winner.pct_f, pct_t: r.winner.pct_t, commit: r.commit })),
  final_baselines: { flights: baseF, txn: baseT },
}
