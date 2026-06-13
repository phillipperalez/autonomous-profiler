import { For, Show } from "solid-js";
import type { ActivityEntry } from "./types";
import { fmtTime } from "./util";

const ICON: Record<string, string> = {
  working: "▷",
  accepted: "✓",
  rejected: "✕",
  error: "!",
  info: "·",
};

export function ActivityFeed(props: { entries: ActivityEntry[] }) {
  const rows = () => props.entries.slice().reverse(); // newest first
  return (
    <div class="win activity">
      <div class="timeline-head">
        <h2>loop activity</h2>
        <span class="muted small">what the improver is doing · newest first</span>
      </div>
      <Show
        when={rows().length}
        fallback={<div class="muted pad">no activity yet — run the improve loop</div>}
      >
        <div class="feed">
          <For each={rows()}>
            {(e) => (
              <div class={`feed-row ${e.status}`}>
                <span class={`feed-status ${e.status}`}>
                  {ICON[e.status] ?? "·"} {e.status}
                </span>
                <span class="feed-iter mono">
                  {e.run} #{e.iteration}
                </span>
                <span class="feed-fn mono" title={e.function}>
                  {e.function}
                </span>
                <Show when={e.commit}>
                  <span class="feed-commit mono">{e.commit}</span>
                </Show>
                <span class="feed-note">{e.note}</span>
                <span class="feed-time muted small">{fmtTime(e.ts_ms)}</span>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
