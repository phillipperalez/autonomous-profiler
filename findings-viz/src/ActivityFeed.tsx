import { For, Show } from "solid-js";
import type { ActivityEntry, DiffMap } from "./types";
import { fmtTime } from "./util";

const ICON: Record<string, string> = {
  working: "▷",
  accepted: "✓",
  rejected: "✕",
  error: "!",
  info: "·",
};

export function ActivityFeed(props: {
  entries: ActivityEntry[];
  diffs?: DiffMap;
  onOpen?: (sha: string) => void;
}) {
  const rows = () => props.entries.slice().reverse(); // newest first
  const diffFor = (e: ActivityEntry) =>
    e.commit && props.diffs && props.diffs[e.commit] ? e.commit : null;
  return (
    <div class="win activity">
      <div class="timeline-head">
        <h2>loop activity</h2>
        <span class="muted small">what the improver is doing · newest first · ✓ rows open their diff</span>
      </div>
      <Show
        when={rows().length}
        fallback={<div class="muted pad">no activity yet — run the improve loop</div>}
      >
        <div class="feed">
          <For each={rows()}>
            {(e) => {
              const sha = diffFor(e);
              return (
                <div
                  class={`feed-row ${e.status}`}
                  classList={{ clickable: !!sha }}
                  onClick={() => sha && props.onOpen?.(sha)}
                >
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
                    <span class="feed-commit mono" classList={{ link: !!sha }}>
                      {e.commit}
                    </span>
                  </Show>
                  <span class="feed-note">{e.note}</span>
                  <Show when={sha}>
                    <span class="view-diff small">diff →</span>
                  </Show>
                  <span class="feed-time muted small">{fmtTime(e.ts_ms)}</span>
                </div>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}
