export const meta = {
  name: 'autoperf-improve',
  description: 'Generic, config-driven autonomous improve loop for ANY Rust project. Reads an autoperf.toml (via `ap config --json`, passed in as args.config), profiles the primary workload, then runs a multi-round swarm: each round generates N diverse-lens patches in parallel (read-only), evaluates them SERIALLY and quiesced (correctness gate -> bench primary + guards -> revert), commits the single winner that clears the thresholds, and repeats until a round finds no winner. No hardcoded paths — everything comes from the config.',
  phases: [
    { title: 'Baseline', detail: 'profile + benchmark primary & guards on clean HEAD' },
    { title: 'Rounds', detail: 'per round: N parallel proposals -> serial gated eval -> commit winner' },
  ],
}

// ---- args (built by the /autoperf skill from `ap config --json`) -------------
// args = { ap_bin, data_dir, mode?, max_rounds?, config: <normalized autoperf.toml> }
if (!args || !args.config || !args.ap_bin || !args.data_dir) {
  throw new Error('autoperf-improve requires args.config (from `ap config --json`), args.ap_bin, args.data_dir')
}
const AP = args.ap_bin
const DATA = args.data_dir
const MODE = args.mode || 'swarm' // 'swarm' (N lenses/round) or 'serial' (1 proposer/round)
const MAX_ROUNDS = args.max_rounds || 3
const cfg = args.config
const DIR = cfg.target.dir
const REPO = cfg.target.repo || cfg.target.dir
const RAM = cfg.improve.ram_budget_mb || 8192
const MIN_WIN = cfg.improve.min_improvement_pct ?? 3.0 // % faster required on primary
const MAX_GUARD = cfg.improve.guard_regression_pct ?? 2.0 // % a guard may regress
const ALL_LENSES = cfg.improve.lenses && cfg.improve.lenses.length ? cfg.improve.lenses : ['algorithmic']
const LENSES = MODE === 'serial' ? [ALL_LENSES[0]] : ALL_LENSES
const off = new Set(cfg.improve.off_limits || [])
const workloads = cfg.workload || cfg.workloads || []
const primaryIdx = Math.max(0, workloads.findIndex((w) => w.primary))
const primary = workloads[primaryIdx === -1 ? 0 : primaryIdx]
const guards = workloads.filter((_, i) => i !== (primaryIdx === -1 ? 0 : primaryIdx))
const useFingerprint = !!(cfg.gate && cfg.gate.fingerprint)
const testCmd = cfg.gate && cfg.gate.test

const LENS_FOCUS = {
  algorithmic: 'ALGORITHMIC: cut asymptotic/constant work — avoid O(n^2) where O(n log n)/O(n) suffices, hoist invariants, short-circuit, batch. Larger rewrites OK (the gate verifies).',
  locality: 'DATA LOCALITY: keep hot loops register/cache-resident; cut redundant loads/stores; sequential access; shrink the inner working set.',
  dedup: 'DEDUP COPIES: remove redundant clones, intermediate Vec allocs, repeated materialization/recompute; borrow not clone; compute-once.',
  datashape: 'DATA SHAPE: restructure layout for the hot path — SoA vs AoS, pack fields, smaller/aligned numeric types, contiguous buffers, remove pointer-chasing.',
  branchflat: 'BRANCH FLATTENING: make hot inner loops branchless — arithmetic/predication (`acc += (cond) as T`), hoist conditionals out of loops, drop provably-safe checks from the steady state.',
}

// ---- shared command fragments -----------------------------------------------
function benchCmd(w, findingsDir, extraRepo) {
  const tgt = w.bin ? `--bin ${w.bin}` : `--example ${w.example}`
  const feats = w.features && w.features.length ? ` --features ${w.features.join(' ')}` : ''
  const argsStr = w.args && w.args.length ? ` --args ${w.args.join(' ')}` : ''
  return `${AP} bench ${DIR} ${tgt} --label ${w.label} --runs ${w.runs || 5} --findings-dir ${findingsDir} --repo ${extraRepo || REPO}${feats}${argsStr}`
}
function gateText(slot) {
  if (useFingerprint) {
    return `Correctness gate (FINGERPRINT): build+run the PRIMARY workload and capture the line matching /FINGERPRINT=/ from stdout. It MUST equal the baseline fingerprint \`${slot}\` byte-for-byte. If it differs or the run errors -> gate FAILS (tests_ok=false, skip bench). Run: cd ${DIR} && cargo run --profile profiling ${primary.bin ? '' : '--example ' + primary.example} -- ${(primary.args || []).join(' ')} 2>/dev/null | grep -o 'FINGERPRINT=[^ ]*'.`
  }
  return `Correctness gate (TESTS): cd ${DIR} && ${testCmd}. You MAY make MINIMAL compile fixes that preserve intent (retry up to twice). tests_ok = command exits 0 / all pass; any failure -> tests_ok=false (skip bench).`
}
// Surgical revert: undo only the files this candidate touched (never harms autoperf.toml
// or other untracked files). manifest.json lists {dest} entries.
function revertText(scratch) {
  return `ALWAYS revert before finishing: for each {dest} in ${scratch}/manifest.json run \`cd ${REPO} && git checkout -- "<dest>" 2>/dev/null || rm -f "<dest>"\`.`
}

// ---- schemas ----------------------------------------------------------------
const GEN = { type: 'object', properties: { ok: { type: 'boolean' }, lens: { type: 'string' }, function: { type: 'string' }, summary: { type: 'string' }, scratch_dir: { type: 'string' }, file_count: { type: 'number' }, reason: { type: 'string' } }, required: ['ok'] }
const EV = { type: 'object', properties: { built: { type: 'boolean' }, tests_ok: { type: 'boolean' }, min_primary: { type: 'number' }, rss_mb: { type: 'number' }, pct_primary: { type: 'number' }, worst_guard_pct: { type: 'number' }, function: { type: 'string' }, summary: { type: 'string' }, reason: { type: 'string' } }, required: ['built', 'tests_ok'] }
const COMMIT = { type: 'object', properties: { committed: { type: 'boolean' }, commit: { type: 'string' }, min_primary: { type: 'number' }, reason: { type: 'string' } }, required: ['committed'] }

// ---- prompts ----------------------------------------------------------------
function genP(round, i, lens, baseP) {
  return `Round ${round} ${MODE} agent (lens=${lens}) proposing ONE optimization to the Rust project at ${DIR}. READ-ONLY — DO NOT modify ${DIR}; write proposed files to scratch. ABSOLUTE paths.

Lens: ${LENS_FOCUS[lens] || lens}

Primary workload baseline: ${primary.label} = ${baseP}ms. OFF-LIMITS (already optimized — do NOT touch): ${[...off].join(', ') || '(none)'}. Pick a DIFFERENT hot, editable function where your lens applies.

STEP 1 (read-only): the newest ${DATA}/runs/*.json has ranked hotspots with file:line for this project. Read it; explore ${DIR}/**/src/**. Understand inputs/outputs — behavior MUST stay identical (the gate verifies).
STEP 2: design ONE behavior-preserving change through your lens (may span files).
STEP 3: write the FULL new content of each changed file to /tmp/autoperf-r${round}-${i}/ + a manifest /tmp/autoperf-r${round}-${i}/manifest.json = JSON array of {"dest":"<abs path under ${DIR}>","src":"<abs scratch file>"}.

Return {"ok":true,"lens":"${lens}","function":"<fn>","summary":"<what+why>","scratch_dir":"/tmp/autoperf-r${round}-${i}","file_count":<n>} or {"ok":false,"reason":"<why none>"}.`
}

function evP(round, c, baseP, baseFp) {
  const guardBench = guards
    .map((g) => `  - ${g.label}: ${benchCmd(g, `/tmp/ap-trial-r${round}-${c.i}`)} -> read its min_ms; guard_pct = 100*(min-baseline)/baseline (baselines: ${guards.map((x) => x.label + '=' + (baseGuards[x.label] || 0)).join(', ')}).`)
    .join('\n')
  return `Round ${round}: serially EVALUATE candidate (lens=${c.lens}, fn=${c.function}). You are the ONLY thing running — timing is trustworthy. ABSOLUTE paths.
Apply: read /tmp/autoperf-r${round}-${c.i}/manifest.json; copy each src->dest into ${DIR} (overwrite).
${gateText(baseFp)}
Bench (only if tests_ok): ${benchCmd(primary, `/tmp/ap-trial-r${round}-${c.i}`)} -> min_primary, rss_mb. pct_primary = 100*(min_primary-${baseP})/${baseP}.
Guards (only if tests_ok), must not regress > ${MAX_GUARD}%:
${guardBench || '  (none)'}
${revertText(`/tmp/autoperf-r${round}-${c.i}`)}
Activity: ${AP} activity --status info --run autoperf-r${round} --iter ${c.i} --function "${c.function}" --note "lens=${c.lens}: tests=<ok> primary=<pct_primary>% worst_guard=<worst_guard_pct>%" --findings-dir ${DATA}
Return {"built":<b>,"tests_ok":<b>,"min_primary":<N|0>,"rss_mb":<R|0>,"pct_primary":<p|0>,"worst_guard_pct":<p|0>,"function":"${c.function}","summary":"${c.summary}","reason":"<verdict>"}.`
}

function commitP(round, w, baseP, baseFp) {
  return `Commit round ${round} WINNER (lens=${w.lens}, fn=${w.function}, primary ${w.pct_primary}%) to ${REPO}. ABSOLUTE paths.
Re-apply /tmp/autoperf-r${round}-${w.i}/manifest.json (copy src->dest). ${gateText(baseFp)} MUST pass; if it fails, ${revertText(`/tmp/autoperf-r${round}-${w.i}`)} and return committed=false.
Commit ONLY the changed source files (not autoperf.toml): cd ${REPO} && git add $(python3 -c "import json,sys; print(' '.join(json.dumps(e['dest']) for e in json.load(open('/tmp/autoperf-r${round}-${w.i}/manifest.json'))))") && git commit -m "perf: ${w.function} — autoperf r${round} [${w.lens}] (${primary.label} ${w.pct_primary}%) [autoperf]"; sha=$(git rev-parse --short HEAD).
Canonical bench -> dashboard: ${benchCmd(primary, DATA)}${guards.map((g) => '; ' + benchCmd(g, DATA)).join('')}.
Refresh findings: ${AP} run ${DIR} ${primary.bin ? '--bin ' + primary.bin : '--example ' + primary.example} --token-budget 6000 --findings-dir ${DATA} --repo ${REPO} --args ${(primary.args || []).join(' ')}.
Activity: ${AP} activity --status accepted --run autoperf-r${round} --iter ${w.i} --function "${w.function}" --commit <sha> --note "r${round} winner [${w.lens}] ${primary.label} ${w.pct_primary}%" --findings-dir ${DATA}.
Return {"committed":true,"commit":"<sha>","min_primary":<canonical primary min>} or {"committed":false,"reason":"..."}.`
}

// ---- baseline ----------------------------------------------------------------
phase('Baseline')
log(`autoperf-improve [${MODE}] on ${DIR} | primary=${primary.label} | guards=[${guards.map((g) => g.label).join(', ')}] | lenses=[${LENSES.join(', ')}] | gate=${useFingerprint ? 'fingerprint' : 'tests'}`)

const baseGuards = {}
const baseline = await agent(
  `Establish the CLEAN baseline for ${DIR} (current HEAD, no edits). ABSOLUTE paths.
1. Profile the primary so the swarm has hotspots: ${AP} run ${DIR} ${primary.bin ? '--bin ' + primary.bin : '--example ' + primary.example} --token-budget 7000 --findings-dir ${DATA} --repo ${REPO} --args ${(primary.args || []).join(' ')}.
2. Benchmark primary: ${benchCmd(primary, DATA)} -> min_primary.
3. Benchmark each guard: ${guards.map((g) => benchCmd(g, DATA)).join(' ; ') || '(none)'} -> {label:min} for each.
${useFingerprint ? `4. Capture baseline fingerprint: cd ${DIR} && cargo run --profile profiling ${primary.bin ? '' : '--example ' + primary.example} -- ${(primary.args || []).join(' ')} 2>/dev/null | grep -o 'FINGERPRINT=[^ ]*' -> fingerprint.` : ''}
Do NOT edit any source. Return the numbers.`,
  { label: 'baseline', phase: 'Baseline', agentType: 'general-purpose', schema: { type: 'object', properties: { min_primary: { type: 'number' }, guards: { type: 'object' }, fingerprint: { type: 'string' } }, required: ['min_primary'] } }
)
let baseP = baseline.min_primary
const baseFp = baseline.fingerprint || ''
Object.assign(baseGuards, baseline.guards || {})
log(`baseline: ${primary.label}=${baseP}ms ${Object.entries(baseGuards).map(([k, v]) => k + '=' + v + 'ms').join(' ')} ${baseFp ? '| fp=' + baseFp : ''}`)

// ---- rounds ------------------------------------------------------------------
phase('Rounds')
const roundResults = []
for (let round = 1; round <= MAX_ROUNDS; round++) {
  log(`round ${round}: baseline ${primary.label}=${baseP}ms | off-limits=${off.size}`)

  const cands = (await parallel(
    LENSES.map((lens, idx) => () =>
      agent(genP(round, idx + 1, lens, baseP), { label: `r${round}:gen:${lens}`, phase: 'Rounds', agentType: 'general-purpose', schema: GEN })
        .then((r) => (r ? { ...r, i: idx + 1, lens } : null))
    )
  )).filter((c) => c && c.ok && c.file_count > 0 && !off.has(c.function))
  log(`round ${round}: ${cands.length} viable: ${cands.map((c) => c.lens + '(' + (c.function || '?') + ')').join(', ')}`)

  const evald = []
  for (const c of cands) {
    const r = await agent(evP(round, c, baseP, baseFp), { label: `r${round}:eval:${c.lens}`, phase: 'Rounds', agentType: 'general-purpose', schema: EV })
    if (r) { evald.push({ ...r, i: c.i, lens: c.lens }); log(`r${round} eval ${c.lens}: tests=${r.tests_ok} primary=${r.pct_primary}% worst_guard=${r.worst_guard_pct}%`) }
  }

  const passing = evald.filter(
    (r) => r.tests_ok && typeof r.min_primary === 'number' && r.min_primary > 0 && (r.rss_mb || 0) <= RAM && (r.worst_guard_pct || 0) <= MAX_GUARD && r.pct_primary <= -MIN_WIN
  )
  passing.sort((a, b) => a.min_primary - b.min_primary)
  const w = passing[0]

  let committed = null
  if (w) committed = await agent(commitP(round, w, baseP, baseFp), { label: `r${round}:commit:${w.lens}`, phase: 'Rounds', agentType: 'general-purpose', schema: COMMIT })

  if (committed && committed.committed) {
    off.add(w.function)
    if (typeof committed.min_primary === 'number' && committed.min_primary > 0) baseP = committed.min_primary
    roundResults.push({ round, winner: { lens: w.lens, function: w.function, pct_primary: w.pct_primary }, commit: committed.commit })
    log(`round ${round}: COMMIT ${committed.commit} — ${w.function} [${w.lens}] ${primary.label} ${w.pct_primary}%; new baseline ${baseP}ms`)
  } else {
    roundResults.push({ round, winner: null, dry: true })
    log(`round ${round}: DRY — no candidate beat baseline by >=${MIN_WIN}% without regression. Stopping.`)
    break
  }
}

const wins = roundResults.filter((r) => r.winner)
return {
  target: DIR,
  mode: MODE,
  rounds_run: roundResults.length,
  wins: wins.map((r) => ({ round: r.round, lens: r.winner.lens, function: r.winner.function, pct_primary: r.winner.pct_primary, commit: r.commit })),
  final_primary_ms: baseP,
}
