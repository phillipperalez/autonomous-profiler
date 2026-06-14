// capture-diffs.mjs — collect the git diff for every committed win so the dashboard
// can show a scrollable popup diff per commit.
//
// Reads <data>/activity.json (and bench.json), finds every commit SHA, runs
// `git show` in whichever target repo contains it, and writes <data>/diffs.json:
//   { "<short-sha>": { sha, subject, author, date, stat, files:[{file,add,del}], patch } }
//
// Usage: node scripts/capture-diffs.mjs [dataDir] [repo ...]
//   dataDir defaults to ../autonomous-profiler/data
//   repos   default to ../analyzer and ../polars (the local, gitignored targets)

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const dataDir = resolve(root, process.argv[2] || "../autonomous-profiler/data");
const repos = (process.argv.length > 3 ? process.argv.slice(3) : ["../analyzer", "../polars"]).map(
  (p) => resolve(root, p),
);

function readJSON(p, fallback) {
  try {
    return JSON.parse(readFileSync(p, "utf8"));
  } catch {
    return fallback;
  }
}

// Collect SHAs from ACCEPTED wins only (the real optimizations). We deliberately do
// NOT pull every bench SHA — baselines like the initial import are enormous (100k+
// files) and aren't wins; the timeline simply leaves those bars non-clickable.
const activity = readJSON(resolve(dataDir, "activity.json"), []);
const shas = new Set();
for (const e of activity) if (e.status === "accepted" && e.commit) shas.add(e.commit.trim());
// Safety: skip any commit that touches an unreasonable number of files (not a focused win).
const MAX_FILES = 50;

function gitShow(repo, sha, args) {
  return execFileSync("git", ["-C", repo, "show", ...args, sha], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
}

function repoFor(sha) {
  for (const repo of repos) {
    if (!existsSync(resolve(repo, ".git"))) continue;
    try {
      execFileSync("git", ["-C", repo, "cat-file", "-e", `${sha}^{commit}`], { stdio: "ignore" });
      return repo;
    } catch {
      /* not in this repo */
    }
  }
  return null;
}

const out = {};
let ok = 0;
let miss = 0;
for (const sha of shas) {
  const repo = repoFor(sha);
  if (!repo) {
    miss++;
    console.warn(`! ${sha}: not found in ${repos.map((r) => r.split("/").pop()).join(", ")}`);
    continue;
  }
  try {
    const meta = gitShow(repo, sha, [
      "--no-patch",
      "--format=%H%n%h%n%an%n%aI%n%s",
    ]).trim().split("\n");
    const [full, short, author, date, ...subjectParts] = meta;
    const subject = subjectParts.join("\n");
    const numstat = gitShow(repo, sha, ["--no-patch", "--numstat", "--format="]).trim();
    const files = numstat
      .split("\n")
      .filter(Boolean)
      .map((l) => {
        const [add, del, ...f] = l.split("\t");
        return { file: f.join("\t"), add: Number(add) || 0, del: Number(del) || 0 };
      });
    if (files.length > MAX_FILES) {
      miss++;
      console.warn(`! ${sha}: ${files.length} files (> ${MAX_FILES}) — skipping non-focused commit`);
      continue;
    }
    const stat = gitShow(repo, sha, ["--no-patch", "--shortstat", "--format="]).trim();
    // The patch itself, with the commit header stripped (we render our own header).
    const patch = gitShow(repo, sha, ["--format=", "--unified=4"]).replace(/^\s+/, "");
    out[short] = {
      sha: full,
      short,
      repo: repo.split("/").pop(),
      author,
      date,
      subject,
      stat,
      files,
      patch,
    };
    ok++;
  } catch (e) {
    miss++;
    console.warn(`! ${sha}: git show failed — ${e.message}`);
  }
}

writeFileSync(resolve(dataDir, "diffs.json"), JSON.stringify(out, null, 2));
console.log(`wrote ${resolve(dataDir, "diffs.json")} — ${ok} diffs (${miss} missing)`);
