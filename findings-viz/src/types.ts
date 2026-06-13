export interface GitInfo {
  commit: string;
  short: string;
  subject: string;
  dirty: boolean;
}

export interface IndexEntry {
  id: string;
  workload: string;
  backend: string;
  kind: string;
  unit: string;
  total_weight: number;
  duration_ms: number;
  created_at_ms: number;
  hotspot_count: number;
  top_function: string | null;
  top_crate: string | null;
  git?: GitInfo;
}

export interface BenchRecord {
  label: string;
  runs: number;
  min_ms: number;
  median_ms: number;
  mean_ms: number;
  samples_ms: number[];
  peak_rss_bytes?: number;
  heap_peak_bytes?: number;
  created_at_ms: number;
  git: GitInfo;
}

export interface CrateShare {
  name: string;
  pct: number;
}

export interface SourceLoc {
  file: string;
  line: number;
}

export interface Hotspot {
  rank: number;
  function: string;
  crate_name: string;
  self_pct: number;
  total_pct: number;
  source?: SourceLoc | null;
  snippet?: string | null;
  neighbors: string[];
  tags: string[];
}

export interface RunFindings {
  id: string;
  workload: string;
  backend: string;
  kind: string;
  unit: string;
  total_weight: number;
  function_count: number;
  duration_ms: number;
  created_at_ms: number;
  git?: GitInfo;
  crate_rollup: CrateShare[];
  hot_path: string[];
  hotspots: Hotspot[];
}
