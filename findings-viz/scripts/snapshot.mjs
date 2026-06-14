// Refresh the committed demo-data snapshot from the live profiler data, SAFELY:
// copy ../autonomous-profiler/data, then strip embedded source-code snippets and
// normalize local/registry file paths. This is what the public GitHub Pages build
// serves — it must never contain internal source bodies or home paths.
//
//   pnpm snapshot
import { cpSync, rmSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";

const here = new URL(".", import.meta.url).pathname;
const live = resolve(here, "../../autonomous-profiler/data");
const dest = resolve(here, "../demo-data");

rmSync(dest, { recursive: true, force: true });
cpSync(live, dest, { recursive: true });

const cleanPath = (p) =>
  typeof p === "string"
    ? p
        .replace(/.*\/registry\/src\/[^/]+\//, "") // cargo registry -> crate-rel
        .replace(/.*\/Source2\/analyzer\//, "") // target repo -> repo-rel
        .replace(/\/Users\/[^/]+\//, "~/") // any other home -> ~/
    : p;

// Scrub absolute home paths from free text (git patches are repo-relative already,
// this is just defensive) without altering the diff content itself.
const scrubText = (s) =>
  typeof s === "string"
    ? s.replace(/\/Users\/[^/\s]+\//g, "~/").replace(/.*\/Source2\//g, "")
    : s;

const walk = (o) => {
  if (Array.isArray(o)) return o.map(walk);
  if (o && typeof o === "object") {
    for (const k of Object.keys(o)) {
      if (k === "snippet") o[k] = null; // drop profiler source-code dumps
      else if (k === "patch") o[k] = scrubText(o[k]); // KEEP win diffs (the highlight)
      else if (k === "file") o[k] = cleanPath(o[k]);
      else o[k] = walk(o[k]);
    }
  }
  return o;
};

let files = 0;
const sanitize = (dir) => {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) sanitize(p);
    else if (e.name.endsWith(".json")) {
      writeFileSync(p, JSON.stringify(walk(JSON.parse(readFileSync(p, "utf8"))), null, 2));
      files++;
    }
  }
};
sanitize(dest);
console.log(`snapshot: sanitized ${files} json files into demo-data/ (no source, no home paths)`);
