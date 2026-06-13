export const meta = {
  name: 'auto-improve-analyzer-until-dry',
  description: 'Open-ended: optimize hot philtool_core fns for flights-1m, gate on cargo test + >3% time (no >2% transactions regression) + peak RSS under RAM budget. Loop until no editable win remains (bottleneck in a dependency). Streams activity to the dashboard.',
  phases: [{ title: 'Improve', detail: 'loop until dry: profile -> edit -> test -> bench -> commit/revert -> activity' }],
}

const AN = '/Users/YOUR_USER/Source2/analyzer'
const AP_DIR = '/Users/YOUR_USER/Source2/autonomous-profiler'
const DATA = '/Users/YOUR_USER/Source2/autonomous-profiler/data'
const AP = `${AP_DIR}/target/debug/ap`
const PRIMARY = 'flights-1m'
const GUARD = 'transactions_train'
const RAM_BUDGET_MB = (args && args.ram_budget_mb) || 8192
const MAX_ITERS = (args && args.max_iters) || 12

let baselines = { 'flights-1m': 384, 'transactions_train': 1542 }
let baselineRssMb = 540

const SCHEMA = {
  type: 'object',
  properties: {
    committed: { type: 'boolean' },
    stop: { type: 'boolean' },
    commit: { type: 'string' },
    function: { type: 'string' },
    file: { type: 'string' },
    summary: { type: 'string' },
    tradeoff: { type: 'string' },
    reason: { type: 'string' },
    primary_rss_mb: { type: 'number' },
    deltas: {
      type: 'object',
      properties: {
        'flights-1m': { type: 'number' },
        'transactions_train': { type: 'number' },
      },
    },
    new_mins: {
      type: 'object',
      properties: {
        'flights-1m': { type: 'number' },
        'transactions_train': { type: 'number' },
      },
    },
  },
  required: ['committed'],
}

function prompt(i, base, baseRss, budget) {
  return `You are iteration ${i} of an OPEN-ENDED autonomous performance loop on a Rust library. Make ONE behavior-preserving optimization to the hottest function you OWN, keep it only if it passes the gates, and report progress to the dashboard activity feed. NEVER push. Use ABSOLUTE paths. Only ever edit files under ${AN}/philtool-core/src.

Paths: TARGET repo=${AN} (your code: philtool-core/src/**) · AP=${AP} · dashboard data dir=${DATA}
Datasets: primary=${PRIMARY}, guard=${GUARD} at ${AN}/parquet/<name>.parquet
Baselines to beat — min ms: ${PRIMARY}=${base[PRIMARY]}, ${GUARD}=${base[GUARD]}. Baseline ${PRIMARY} peak RSS=${baseRss} MB. RAM BUDGET: ${PRIMARY} peak RSS MUST stay <= ${budget} MB (you may trade memory for speed under that ceiling; state it).

STEP 1 — find a NEW hotspot:
  ${AP} run ${AN} --example run_phase --token-budget 6000 --focus philtool_core --findings-dir ${DATA} --args ${AN}/parquet/${PRIMARY}.parquet
  Pick the highest self% hotspot whose crate is philtool_core AND source is under philtool-core/src. Deps (polars*, ndarray, core, alloc, hashbrown, ryu) are NOT editable.
  Run: cd ${AN} && git log --oneline   — do NOT re-optimize a function changed in a prior commit; pick the next unoptimized one.
  STOP CONDITION: if the top philtool_core hotspots are all already-optimized OR the remaining hot self-time is dominated by non-editable dependencies (e.g. polars cast/collect/arrow, ndarray), then there is no editable win left. Emit:
    ${AP} activity --status info --run analyzer --iter ${i} --function "<area>" --note "no editable win left; bottleneck now in <dependency> (e.g. polars) — recommend switching to that target" --findings-dir ${DATA}
  and return {"committed":false,"stop":true,"reason":"no editable hotspot; bottleneck in <dependency>"}.

STEP 2 — announce + optimize:
  ${AP} activity --status working --run analyzer --iter ${i} --function "<fn>" --note "optimizing <fn>: <approach>" --findings-dir ${DATA}
  Apply ONE safe edit (outputs identical). Techniques: null_count()==0 fast paths; rechunk()+cont_slice() contiguous reads instead of per-element ChunkedArray::get; with_capacity; hoist allocations/format! out of loops; drop clones; reuse buffers.

STEP 3 — correctness gate: cd ${AN} && cargo test -p philtool-core
  On failure: cd ${AN} && git checkout -- philtool-core ; emit  ${AP} activity --status rejected --run analyzer --iter ${i} --function "<fn>" --note "tests failed" --findings-dir ${DATA} ; return {"committed":false,"function":...,"reason":"tests failed: <detail>"}.

STEP 4 — trial benches to a TEMP dir (keep dashboard clean). For ${PRIMARY} and ${GUARD}:
  ${AP} bench ${AN} --example run_phase --label <ds> --runs 5 --findings-dir /tmp/ap-trial-dry-${i} --args ${AN}/parquet/<ds>.parquet
  Output: "min <N> ms ... peak RSS <R> MB". pct[ds]=100*(N-baseline[ds])/baseline[ds]; record ${PRIMARY} rss=R.

STEP 5 — decision:
  ACCEPT iff (R <= ${budget}) AND pct[${PRIMARY}] <= -3.0 AND pct[${GUARD}] < 2.0
  - ACCEPT:
      cd ${AN} && git add -A && git commit -m "perf(philtool_core): <fn> — <what+why> (${PRIMARY} <pct>%, RAM <R>MB)"
      sha=(cd ${AN} && git rev-parse --short HEAD)
      Canonical post-commit benches (clean HEAD) -> dashboard: for ${PRIMARY} and ${GUARD}: ${AP} bench ${AN} --example run_phase --label <ds> --runs 5 --findings-dir ${DATA} --args ${AN}/parquet/<ds>.parquet  (record new_mins[ds])
      Refresh findings: ${AP} run ${AN} --example run_phase --token-budget 6000 --focus philtool_core --findings-dir ${DATA} --args ${AN}/parquet/${PRIMARY}.parquet
      Emit: ${AP} activity --status accepted --run analyzer --iter ${i} --function "<fn>" --commit <sha> --note "${PRIMARY} <pct>% · RAM <R>MB · <tradeoff>" --findings-dir ${DATA}
      Return {"committed":true,"commit":sha,"function":...,"file":...,"summary":...,"tradeoff":...,"primary_rss_mb":R,"deltas":{...},"new_mins":{...}}.
  - REJECT:
      cd ${AN} && git checkout -- philtool-core
      Emit: ${AP} activity --status rejected --run analyzer --iter ${i} --function "<fn>" --note "<reason e.g. flights only -1.2%>" --findings-dir ${DATA}
      Return {"committed":false,"function":...,"file":...,"summary":"<tried>","primary_rss_mb":R,"deltas":{...},"reason":"<why>"}.

Return STRICT JSON for the schema.`
}

phase('Improve')
const results = []
let consecutiveReverts = 0
for (let i = 1; i <= MAX_ITERS; i++) {
  log(`iter ${i}: baselines ${JSON.stringify(baselines)} | RAM<=${RAM_BUDGET_MB}MB`)
  const r = await agent(prompt(i, baselines, baselineRssMb, RAM_BUDGET_MB), {
    label: `iter-${i}`,
    phase: 'Improve',
    agentType: 'general-purpose',
    schema: SCHEMA,
  })
  results.push(r)
  if (!r) { log(`iter ${i}: null`); consecutiveReverts++; if (consecutiveReverts >= 3) break; continue }
  if (r.committed && r.new_mins) {
    if (typeof r.new_mins[PRIMARY] === 'number') baselines[PRIMARY] = r.new_mins[PRIMARY]
    if (typeof r.new_mins[GUARD] === 'number') baselines[GUARD] = r.new_mins[GUARD]
    if (typeof r.primary_rss_mb === 'number' && r.primary_rss_mb > 0) baselineRssMb = r.primary_rss_mb
    consecutiveReverts = 0
    log(`iter ${i}: COMMIT ${r.commit} — ${r.function} | ${r.tradeoff || ''}`)
  } else {
    consecutiveReverts++
    log(`iter ${i}: ${r.stop ? 'STOP (dry)' : 'reverted'} — ${r.reason || ''}`)
  }
  if (r.stop) break
  if (consecutiveReverts >= 3) { log(`stopping: ${consecutiveReverts} consecutive reverts (treating as dry)`); break }
}

const committed = results.filter((r) => r && r.committed)
const lastStop = results.length ? results[results.length - 1] : null
return {
  ram_budget_mb: RAM_BUDGET_MB,
  iterations_run: results.length,
  committed_count: committed.length,
  commits: committed.map((r) => ({ commit: r.commit, function: r.function, tradeoff: r.tradeoff, deltas: r.deltas })),
  dry: !!(lastStop && lastStop.stop),
  final_baselines: baselines,
}
