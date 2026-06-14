import { For, Show, createMemo } from "solid-js";
import type { BenchRecord, DiffMap } from "./types";
import { fmtTime, fmtMB } from "./util";

// Group bench points by series label, each sorted oldest -> newest.
function groupByLabel(records: BenchRecord[]): [string, BenchRecord[]][] {
  const map = new Map<string, BenchRecord[]>();
  for (const r of records) {
    const arr = map.get(r.label) ?? [];
    arr.push(r);
    map.set(r.label, arr);
  }
  for (const arr of map.values())
    arr.sort((a, b) => a.created_at_ms - b.created_at_ms);
  return [...map.entries()];
}

export function BenchTimeline(props: {
  records: BenchRecord[];
  diffs?: DiffMap;
  onOpen?: (sha: string) => void;
}) {
  const groups = createMemo(() => groupByLabel(props.records));
  return (
    <div class="win timeline">
      <div class="timeline-head">
        <h2>benchmark timeline</h2>
        <span class="muted small">min ms per commit · lower is better · click a win bar for its diff</span>
      </div>
      <For each={groups()}>
        {([label, recs]) => (
          <Series label={label} recs={recs} diffs={props.diffs} onOpen={props.onOpen} />
        )}
      </For>
    </div>
  );
}

function Series(props: {
  label: string;
  recs: BenchRecord[];
  diffs?: DiffMap;
  onOpen?: (sha: string) => void;
}) {
  const recs = () => props.recs;
  const max = createMemo(() => Math.max(...recs().map((r) => r.min_ms), 1));
  const first = () => recs()[0];
  const last = () => recs()[recs().length - 1];
  const best = createMemo(() => Math.min(...recs().map((r) => r.min_ms)));
  const totalPct = createMemo(() => {
    const f = first().min_ms;
    return f > 0 ? (100 * (last().min_ms - f)) / f : 0;
  });

  return (
    <div class="series">
      <div class="series-head">
        <span class="series-label mono">{props.label}</span>
        <span class="series-now mono">{last().min_ms} ms</span>
        <Show when={recs().length > 1}>
          <span
            class="series-trend"
            classList={{
              good: totalPct() < 0,
              bad: totalPct() > 0,
            }}
          >
            {totalPct() <= 0 ? "▼" : "▲"} {Math.abs(totalPct()).toFixed(1)}% vs baseline
          </span>
        </Show>
        <Show when={last().peak_rss_bytes}>
          <span class="series-ram mono">RAM {fmtMB(last().peak_rss_bytes)}</span>
        </Show>
        <span class="muted small">best {best()} ms · {recs().length} commits</span>
      </div>

      <div class="bars">
        <For each={recs()}>
          {(r, i) => {
            const prev = i() > 0 ? recs()[i() - 1] : null;
            const delta = prev ? r.min_ms - prev.min_ms : 0;
            const deltaPct = prev && prev.min_ms ? (100 * delta) / prev.min_ms : 0;
            const state = !prev
              ? "first"
              : delta < 0
                ? "good"
                : delta > 0
                  ? "bad"
                  : "flat";
            const h = Math.max(8, (r.min_ms / max()) * 100);
            const isBest = r.min_ms === best();
            const ramDelta =
              prev && prev.peak_rss_bytes && r.peak_rss_bytes
                ? r.peak_rss_bytes - prev.peak_rss_bytes
                : 0;
            const sha = r.git?.short;
            const hasDiff = !!(sha && props.diffs && props.diffs[sha]);
            return (
              <div class="bar-col" classList={{ best: isBest, "has-diff": hasDiff }}>
                <div class="bar-wrap">
                  <div
                    class={`tbar ${state}`}
                    classList={{ clickable: hasDiff }}
                    style={{ height: `${h}%` }}
                    onClick={() => hasDiff && props.onOpen?.(sha!)}
                    title={
                      `${r.git?.short ?? "?"}${r.git?.dirty ? "*" : ""} — ${r.git?.subject ?? ""}\n` +
                      `min ${r.min_ms} / median ${r.median_ms} / mean ${r.mean_ms} ms (${r.runs} runs)\n` +
                      `peak RSS ${fmtMB(r.peak_rss_bytes)}` +
                      (r.heap_peak_bytes ? ` · heap ${fmtMB(r.heap_peak_bytes)}` : "") +
                      `\n${fmtTime(r.created_at_ms)}` +
                      (hasDiff ? "\n\nclick for diff" : "")
                    }
                  >
                    <span class="tbar-ms mono">{r.min_ms}</span>
                    <Show when={hasDiff}>
                      <span class="tbar-diff" title="has diff">⊕</span>
                    </Show>
                  </div>
                </div>
                <div class="bar-foot">
                  <span class="bar-sha mono" classList={{ link: hasDiff }} onClick={() => hasDiff && props.onOpen?.(sha!)}>{r.git?.short || "—"}</span>
                  <Show when={prev}>
                    <span class={`bar-delta ${state}`}>
                      {delta <= 0 ? "" : "+"}
                      {deltaPct.toFixed(0)}%
                    </span>
                  </Show>
                  <Show when={r.peak_rss_bytes}>
                    <span
                      class="bar-ram mono"
                      classList={{ good: ramDelta < 0, bad: ramDelta > 0 }}
                    >
                      {fmtMB(r.peak_rss_bytes)}
                    </span>
                  </Show>
                </div>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
