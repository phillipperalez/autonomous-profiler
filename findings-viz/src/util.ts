// Small display helpers shared across the dashboard.

/** Shorten a long demangled symbol for table display (keeps the tail). */
export function shortFn(name: string): string {
  // Drop generic params to cut noise: `Foo<A,B>::bar` -> `Foo::bar`.
  let s = name.replace(/<[^<>]*>/g, "");
  // Collapse repeated `<... as ...>` qualifiers left over.
  s = s.replace(/<[^<>]*>/g, "");
  const parts = s.split("::").filter(Boolean);
  if (parts.length <= 3) return s;
  return parts.slice(-3).join("::");
}

export function basename(path: string): string {
  const i = path.lastIndexOf("/");
  return i >= 0 ? path.slice(i + 1) : path;
}

export function fmtDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

export function fmtWeight(n: number, unit: string): string {
  const v =
    n >= 1_000_000
      ? `${(n / 1_000_000).toFixed(1)}M`
      : n >= 1000
        ? `${(n / 1000).toFixed(1)}k`
        : `${n}`;
  return `${v} ${unit}`;
}

export function fmtMB(bytes: number | undefined): string {
  if (!bytes) return "—";
  const mb = bytes / (1024 * 1024);
  return mb >= 1024 ? `${(mb / 1024).toFixed(2)} GB` : `${mb.toFixed(0)} MB`;
}

export function fmtTime(ms: number): string {
  if (!ms) return "";
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return "";
  }
}

// Stable color per crate name (HSL by hash) for bars/badges.
export function crateColor(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) % 360;
  return `hsl(${h} 70% 55%)`;
}
