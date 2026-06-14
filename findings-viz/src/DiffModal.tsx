import { For, Show, onMount, onCleanup, createMemo } from "solid-js";
import type { CommitDiff } from "./types";

type Line = { kind: "add" | "del" | "hunk" | "file" | "ctx" | "meta"; text: string };

// Classify each patch line for color-coding. `git show --format=` output starts
// straight at `diff --git ...`, so we only see file headers, hunks, and content.
function parsePatch(patch: string): Line[] {
  const out: Line[] = [];
  for (const raw of patch.split("\n")) {
    if (raw.startsWith("diff --git") || raw.startsWith("+++ ") || raw.startsWith("--- "))
      out.push({ kind: "file", text: raw });
    else if (raw.startsWith("@@")) out.push({ kind: "hunk", text: raw });
    else if (
      raw.startsWith("index ") ||
      raw.startsWith("new file") ||
      raw.startsWith("deleted file") ||
      raw.startsWith("similarity ") ||
      raw.startsWith("rename ")
    )
      out.push({ kind: "meta", text: raw });
    else if (raw.startsWith("+")) out.push({ kind: "add", text: raw });
    else if (raw.startsWith("-")) out.push({ kind: "del", text: raw });
    else out.push({ kind: "ctx", text: raw });
  }
  return out;
}

export function DiffModal(props: { diff: CommitDiff | null; onClose: () => void }) {
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onClose();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const lines = createMemo(() => (props.diff ? parsePatch(props.diff.patch) : []));

  return (
    <Show when={props.diff}>
      {(d) => (
        <div class="modal-backdrop" onClick={props.onClose}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <header class="modal-head">
              <div class="modal-title">
                <span class={`repo-badge ${d().repo}`}>{d().repo}</span>
                <span class="modal-subject mono">{d().subject}</span>
              </div>
              <div class="modal-meta">
                <span class="chip commit">{d().short}</span>
                <span class="muted small">{d().stat}</span>
                <button class="modal-close" onClick={props.onClose} title="close (Esc)">
                  ✕
                </button>
              </div>
            </header>

            <div class="modal-files">
              <For each={d().files}>
                {(f) => (
                  <span class="dfile mono">
                    {f.file}
                    <span class="dfile-add">+{f.add}</span>
                    <span class="dfile-del">−{f.del}</span>
                  </span>
                )}
              </For>
            </div>

            <div class="diff-scroll">
              <pre class="diff">
                <For each={lines()}>
                  {(l) => <div class={`dl ${l.kind}`}>{l.text || " "}</div>}
                </For>
              </pre>
            </div>

            <footer class="modal-foot muted small">
              {d().repo === "polars" ? (
                <span>upstream candidate · proposed to pola-rs/polars</span>
              ) : (
                <span>committed locally · gated on tests + quiesced benchmark</span>
              )}
              <span class="modal-hint">Esc or click outside to close</span>
            </footer>
          </div>
        </div>
      )}
    </Show>
  );
}
