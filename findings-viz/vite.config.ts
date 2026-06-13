import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import { resolve } from "node:path";

// The profiler writes findings to ../autonomous-profiler/data. Serve that dir as
// the public root so the app fetches /index.json and /runs/<id>.json directly —
// one source of truth, no copying. Override with AP_DATA_DIR.
const dataDir =
  process.env.AP_DATA_DIR ?? resolve(__dirname, "../autonomous-profiler/data");

export default defineConfig({
  plugins: [solid()],
  publicDir: dataDir,
  server: { port: 5180, fs: { allow: [resolve(__dirname), dataDir] } },
});
