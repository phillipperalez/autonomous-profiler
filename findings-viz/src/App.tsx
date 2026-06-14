import {
  createResource,
  createSignal,
  For,
  Show,
  createMemo,
  onMount,
  onCleanup,
} from "solid-js";
import type {
  IndexEntry,
  RunFindings,
  Hotspot,
  BenchRecord,
  ActivityEntry,
  DiffMap,
} from "./types";
import {
  shortFn,
  basename,
  fmtDuration,
  fmtWeight,
  fmtTime,
  crateColor,
} from "./util";
import { BenchTimeline } from "./BenchTimeline";
import { ActivityFeed } from "./ActivityFeed";
import { WinsView } from "./WinsView";
import { DiffModal } from "./DiffModal";

const base = import.meta.env.BASE_URL;

async function getJSON<T>(path: string, fallback: T): Promise<T> {
  try {
    const r = await fetch(`${base}${path}`, { cache: "no-store" });
    if (!r.ok) return fallback;
    return (await r.json()) as T;
  } catch {
    return fallback;
  }
}

const POLL_MS = 2500;
type View = "wins" | "timeline" | "activity";

export default function App() {
  // `tick` drives live polling: every resource keyed on it refetches.
  const [tick, setTick] = createSignal(0);
  onMount(() => {
    const t = setInterval(() => setTick((v) => v + 1), POLL_MS);
    onCleanup(() => clearInterval(t));
  });

  const [index] = createResource(tick, () =>
    getJSON<IndexEntry[]>("index.json", []),
  );
  const [bench] = createResource(tick, () =>
    getJSON<BenchRecord[]>("bench.json", []),
  );
  const [activity] = createResource(tick, () =>
    getJSON<ActivityEntry[]>("activity.json", []),
  );
  const [diffs] = createResource(tick, () => getJSON<DiffMap>("diffs.json", {}));

  const [view, setView] = createSignal<View>("wins");
  const [present, setPresent] = createSignal(false);
  const [diffSha, setDiffSha] = createSignal<string | null>(null);
  const openDiff = (sha: string) => setDiffSha(sha);
  const currentDiff = createMemo(() => {
    const s = diffSha();
    const d = diffs();
    return s && d && d[s] ? d[s] : null;
  });

  // Loop is "working" if the most recent activity event is a working status.
  const working = () => {
    const a = activity();
    return !!a && a.length > 0 && a[a.length - 1].status === "working";
  };

  const [selected, setSelected] = createSignal<string | null>(null);
  const currentId = createMemo(() => {
    const sel = selected();
    if (sel) return sel;
    const idx = index();
    return idx && idx.length ? idx[0].id : null;
  });

  const [run] = createResource(
    () => ({ id: currentId(), t: tick() }),
    ({ id }) => (id ? getJSON<RunFindings | null>(`runs/${id}.json`, null) : null),
  );

  const runCount = () => index()?.length ?? 0;
  const latestCommit = () => index()?.[0]?.git;
  const winCount = () =>
    (activity() ?? []).filter((e) => e.status === "accepted" && e.commit).length;

  // Keyboard: p = presentation, f = fullscreen, 1/2/3 = tabs.
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      if (e.key === "p") setPresent((v) => !v);
      else if (e.key === "f") toggleFullscreen();
      else if (e.key === "1") setView("wins");
      else if (e.key === "2") setView("timeline");
      else if (e.key === "3") setView("activity");
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const toggleFullscreen = () => {
    if (!document.fullscreenElement) document.documentElement.requestFullscreen?.();
    else document.exitFullscreen?.();
  };

  return (
    <div class="app" classList={{ present: present() }}>
      <div class="bar">
        <div class="bar-left">
          <span class="ws">◈</span>
          <span class="bar-title">autonomous-profiler</span>
          <span class="bar-sep">/</span>
          <span class="muted">findings</span>
        </div>
        <div class="bar-center">
          <button class="tab" classList={{ active: view() === "wins" }} onClick={() => setView("wins")}>
            wins
            <Show when={winCount() > 0}>
              <span class="tab-count">{winCount()}</span>
            </Show>
          </button>
          <button
            class="tab"
            classList={{ active: view() === "timeline" }}
            onClick={() => setView("timeline")}
          >
            timeline
          </button>
          <button
            class="tab"
            classList={{ active: view() === "activity" }}
            onClick={() => setView("activity")}
          >
            activity
            <Show when={(activity()?.length ?? 0) > 0}>
              <span class="tab-count">{activity()!.length}</span>
            </Show>
          </button>
        </div>
        <div class="bar-right">
          <Show when={working()}>
            <span class="running">
              <span class="live-dot run" />
              running
            </span>
          </Show>
          <span class="live">
            <span class="live-dot" />
            live
          </span>
          <span class="chip">{runCount()} runs</span>
          <span class="chip">{bench()?.length ?? 0} bench</span>
          <Show when={latestCommit()?.short}>
            <span class="chip commit">
              {latestCommit()!.short}
              {latestCommit()!.dirty ? "*" : ""}
            </span>
          </Show>
          <button
            class="iconbtn"
            classList={{ on: present() }}
            title="presentation mode (p)"
            onClick={() => setPresent((v) => !v)}
          >
            ▣
          </button>
          <button class="iconbtn" title="fullscreen (f)" onClick={toggleFullscreen}>
            ⛶
          </button>
        </div>
      </div>

      <div class="body">
        <Show when={view() === "timeline" && !present()}>
          <aside class="sidebar">
            <div class="sidebar-head">runs</div>
            <div class="runs">
              <Show when={index()} fallback={<div class="muted pad">loading…</div>}>
                <For each={index()} fallback={<div class="muted pad">no runs yet</div>}>
                  {(e) => (
                    <button
                      class="run-card win"
                      classList={{ active: e.id === currentId() }}
                      onClick={() => setSelected(e.id)}
                    >
                      <div class="run-head">
                        <span class={`pill ${e.kind}`}>{e.kind}</span>
                        <span class="muted small">{fmtDuration(e.duration_ms)}</span>
                      </div>
                      <div class="run-workload">{e.workload}</div>
                      <div class="run-meta small muted">
                        {fmtWeight(e.total_weight, e.unit)} · {e.backend}
                      </div>
                      <div class="run-foot">
                        <Show when={e.top_crate}>
                          <span class="dot" style={{ background: crateColor(e.top_crate!) }} />
                          <span class="small mono">{e.top_crate}</span>
                        </Show>
                        <Show when={e.git?.short}>
                          <span class="small commit-tag">
                            {e.git!.short}
                            {e.git!.dirty ? "*" : ""}
                          </span>
                        </Show>
                      </div>
                    </button>
                  )}
                </For>
              </Show>
            </div>
          </aside>
        </Show>

        <main class="main">
          <Show when={view() === "wins"}>
            <WinsView
              activity={activity() ?? []}
              diffs={diffs() ?? {}}
              bench={bench() ?? []}
              onOpen={openDiff}
            />
          </Show>

          <Show when={view() === "activity"}>
            <ActivityFeed entries={activity() ?? []} diffs={diffs() ?? {}} onOpen={openDiff} />
          </Show>

          <Show when={view() === "timeline"}>
            <Show when={bench() && bench()!.length > 0}>
              <BenchTimeline records={bench()!} diffs={diffs() ?? {}} onOpen={openDiff} />
            </Show>

            <Show
              when={run()}
              fallback={
                <div class="win pad muted">
                  <Show when={runCount() === 0} fallback={<>select a run</>}>
                    <p>no findings yet — run the profiler:</p>
                    <pre class="snippet">
                      ap run ../analyzer --example run_phase \{"\n"} --args
                      parquet/flights-1m.parquet
                    </pre>
                    <p class="small">this view updates live as runs land.</p>
                  </Show>
                </div>
              }
            >
              {(r) => <RunView run={r()!} />}
            </Show>
          </Show>
        </main>
      </div>

      <DiffModal diff={currentDiff()} onClose={() => setDiffSha(null)} />
    </div>
  );
}

function RunView(props: { run: RunFindings }) {
  const r = () => props.run;
  return (
    <div class="run-view">
      <header class="win run-header">
        <div class="rh-top">
          <h1>{r().workload}</h1>
          <Show when={r().git?.short}>
            <span class="chip commit">
              {r().git!.short}
              {r().git!.dirty ? "*" : ""} · {r().git!.subject}
            </span>
          </Show>
        </div>
        <div class="stats">
          <Stat label="backend" value={r().backend} accent="blue" />
          <Stat label="kind" value={r().kind} accent="pink" />
          <Stat label={r().unit} value={fmtWeight(r().total_weight, "")} accent="green" />
          <Stat label="duration" value={fmtDuration(r().duration_ms)} accent="orange" />
          <Stat label="functions" value={String(r().function_count)} accent="purple" />
          <Stat label="hotspots" value={String(r().hotspots.length)} accent="yellow" />
        </div>
        <Show when={r().created_at_ms}>
          <div class="muted small">{fmtTime(r().created_at_ms)}</div>
        </Show>
      </header>

      <section class="panels">
        <div class="win panel">
          <h2>where the cost is</h2>
          <div class="rollup">
            <For each={r().crate_rollup}>
              {(c) => (
                <div class="rollup-row">
                  <div class="rollup-label mono" title={c.name}>{c.name}</div>
                  <div class="bar-track">
                    <div
                      class="bar"
                      style={{
                        width: `${Math.max(2, c.pct)}%`,
                        background: crateColor(c.name),
                      }}
                    />
                  </div>
                  <div class="rollup-pct mono">{c.pct.toFixed(1)}%</div>
                </div>
              )}
            </For>
          </div>
        </div>

        <div class="win panel">
          <h2>dominant hot path</h2>
          <ol class="hotpath">
            <For each={r().hot_path}>
              {(f, i) => (
                <li classList={{ leaf: i() === r().hot_path.length - 1 }}>
                  <span class="hp-fn mono" title={f}>{shortFn(f)}</span>
                </li>
              )}
            </For>
          </ol>
        </div>
      </section>

      <section class="win panel">
        <h2>hotspots</h2>
        <div class="hotspots">
          <For each={r().hotspots}>{(h) => <HotspotRow h={h} />}</For>
        </div>
      </section>
    </div>
  );
}

function HotspotRow(props: { h: Hotspot }) {
  const [open, setOpen] = createSignal(false);
  const h = () => props.h;
  return (
    <div class="hotspot" classList={{ open: open() }}>
      <button class="hs-row" onClick={() => setOpen(!open())}>
        <span class="hs-rank mono">{h().rank}</span>
        <span class="hs-bar-cell">
          <span class="hs-bar-track">
            <span
              class="hs-bar"
              style={{
                width: `${Math.max(2, h().self_pct)}%`,
                background: crateColor(h().crate_name),
              }}
            />
          </span>
          <span class="hs-pct mono">{h().self_pct.toFixed(1)}%</span>
        </span>
        <span class="hs-fn mono" title={h().function}>{shortFn(h().function)}</span>
        <span class="hs-crate mono" style={{ color: crateColor(h().crate_name) }}>
          {h().crate_name}
        </span>
        <span class="hs-src small muted mono">
          <Show when={h().source} fallback={"—"}>
            {basename(h().source!.file)}:{h().source!.line}
          </Show>
        </span>
      </button>
      <Show when={open()}>
        <div class="hs-detail">
          <div class="hs-tags">
            <For each={h().tags}>{(t) => <span class="tag">{t}</span>}</For>
            <span class="muted small">
              {h().self_pct.toFixed(1)}% self · {h().total_pct.toFixed(1)}% total
            </span>
          </div>
          <Show when={h().neighbors.length}>
            <div class="hs-neighbors small">
              <For each={h().neighbors}>{(n) => <code class="mono">{n}</code>}</For>
            </div>
          </Show>
          <Show
            when={h().snippet}
            fallback={
              <div class="muted small">
                no source mapped (registry crate without local debug line info)
              </div>
            }
          >
            <pre class="snippet">{h().snippet}</pre>
          </Show>
        </div>
      </Show>
    </div>
  );
}

function Stat(props: { label: string; value: string; accent: string }) {
  return (
    <div class="stat" data-accent={props.accent}>
      <div class="stat-value mono">{props.value}</div>
      <div class="stat-label">{props.label}</div>
    </div>
  );
}
