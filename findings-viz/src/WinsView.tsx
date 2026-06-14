import { For, Show, createMemo } from "solid-js";
import type { ActivityEntry, DiffMap, BenchRecord } from "./types";
import { shortFn } from "./util";

export interface Win {
  function: string;
  note: string;
  commit: string;
  repo: string;
  pct: number | null; // parsed headline % (negative = faster)
  ts_ms: number;
  hasDiff: boolean;
}

// Pull the headline percent out of a note like "flights-1m -9.6% (canonical) · …".
function parsePct(note: string): number | null {
  const m = note.match(/-?\d+(?:\.\d+)?\s*%/);
  if (!m) return null;
  const n = parseFloat(m[0]);
  return Number.isFinite(n) ? n : null;
}

export function deriveWins(activity: ActivityEntry[], diffs: DiffMap): Win[] {
  const wins: Win[] = [];
  for (const e of activity) {
    if (e.status !== "accepted" || !e.commit) continue;
    const d = diffs[e.commit];
    wins.push({
      function: e.function,
      note: e.note,
      commit: e.commit,
      repo: d?.repo ?? "",
      pct: parsePct(e.note),
      ts_ms: e.ts_ms,
      hasDiff: !!d,
    });
  }
  // Newest first.
  return wins.sort((a, b) => b.ts_ms - a.ts_ms);
}

// Per-series cumulative improvement (first -> best) for the headline band.
function seriesGains(bench: BenchRecord[]): { label: string; pct: number; from: number; to: number }[] {
  const map = new Map<string, BenchRecord[]>();
  for (const r of bench) {
    const a = map.get(r.label) ?? [];
    a.push(r);
    map.set(r.label, a);
  }
  const out: { label: string; pct: number; from: number; to: number }[] = [];
  for (const [label, recs] of map) {
    recs.sort((a, b) => a.created_at_ms - b.created_at_ms);
    const from = recs[0].min_ms;
    const to = Math.min(...recs.map((r) => r.min_ms));
    if (recs.length > 1 && from > 0 && to < from)
      out.push({ label, pct: (100 * (to - from)) / from, from, to });
  }
  return out.sort((a, b) => a.pct - b.pct);
}

export function WinsView(props: {
  activity: ActivityEntry[];
  diffs: DiffMap;
  bench: BenchRecord[];
  onOpen: (sha: string) => void;
}) {
  const wins = createMemo(() => deriveWins(props.activity, props.diffs));
  const gains = createMemo(() => seriesGains(props.bench));
  const repos = createMemo(() => new Set(wins().map((w) => w.repo).filter(Boolean)).size);

  return (
    <div class="wins-view">
      <header class="wins-hero">
        <h1>
          <span class="hero-accent">{wins().length}</span> proven optimizations,
          <span class="hero-accent"> committed</span> — each correct &amp; faster
        </h1>
        <p class="muted">
          autonomously found, gated on tests + quiesced benchmarks, reverted unless measurably
          better. Click any card for the diff.
        </p>
        <div class="gain-band">
          <For each={gains()}>
            {(g) => (
              <div class="gain">
                <div class="gain-pct">{g.pct.toFixed(1)}%</div>
                <div class="gain-label mono">{g.label}</div>
                <div class="gain-sub muted small">
                  {g.from} → {g.to} ms
                </div>
              </div>
            )}
          </For>
        </div>
      </header>

      <div class="wins-grid">
        <For each={wins()}>
          {(w) => (
            <button
              class="wincard"
              classList={{ clickable: w.hasDiff }}
              onClick={() => w.hasDiff && props.onOpen(w.commit)}
              disabled={!w.hasDiff}
            >
              <div class="wincard-top">
                <Show when={w.pct !== null}>
                  <span class="wincard-pct" classList={{ good: (w.pct ?? 0) < 0 }}>
                    {(w.pct ?? 0) < 0 ? "▼" : "▲"} {Math.abs(w.pct ?? 0).toFixed(1)}%
                  </span>
                </Show>
                <Show when={w.repo}>
                  <span class={`repo-badge ${w.repo}`}>{w.repo}</span>
                </Show>
              </div>
              <div class="wincard-fn mono" title={w.function}>
                {shortFn(w.function)}
              </div>
              <div class="wincard-note">{w.note}</div>
              <div class="wincard-foot">
                <span class="chip commit">{w.commit}</span>
                <Show when={w.hasDiff} fallback={<span class="muted small">no diff</span>}>
                  <span class="view-diff">view diff →</span>
                </Show>
              </div>
            </button>
          )}
        </For>
      </div>
    </div>
  );
}
