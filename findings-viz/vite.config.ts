import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import { resolve } from "node:path";
import { existsSync } from "node:fs";

// The profiler writes findings to ../autonomous-profiler/data. Serve that dir as
// the public root so the app fetches /index.json and /runs/<id>.json directly —
// one source of truth, no copying. Override with AP_DATA_DIR.
//
// On CI (GitHub Pages) the live data dir is absent (gitignored), so fall back to
// the committed `demo-data/` snapshot. Refresh it with `pnpm snapshot`.
const liveData = resolve(__dirname, "../autonomous-profiler/data");
const dataDir =
  process.env.AP_DATA_DIR ??
  (existsSync(liveData) ? liveData : resolve(__dirname, "demo-data"));

export default defineConfig({
  // Served at the apex custom domain https://autoperf.run/, so base is root.
  // (For the bare github.io/<repo>/ project page, set BASE_PATH=/autonomous-profiler/.)
  base: process.env.BASE_PATH ?? "/",
  plugins: [solid()],
  publicDir: dataDir,
  server: { port: 5180, fs: { allow: [resolve(__dirname), dataDir] } },
});
