// Pure formatting helpers (unit-tested without a browser).

/** `1536` → `1.5 KB`. */
export function bytes(n) {
  if (!Number.isFinite(n) || n < 0) return "";
  const units = ["B", "KB", "MB", "GB"];
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? value : value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** Percentage change as a whole number, negative when smaller. */
export function percentChange(before, after) {
  if (!before) return 0;
  return Math.round(((after - before) / before) * 100);
}

/** Collapse a sorted list of page numbers into ranges: `[1,2,3,5]` → `1-3, 5`. */
export function ranges(numbers) {
  const sorted = [...new Set(numbers)].sort((a, b) => a - b);
  const out = [];
  let start = null;
  let prev = null;
  for (const n of sorted) {
    if (start === null) start = prev = n;
    else if (n === prev + 1) prev = n;
    else {
      out.push(start === prev ? `${start}` : `${start}-${prev}`);
      start = prev = n;
    }
  }
  if (start !== null) out.push(start === prev ? `${start}` : `${start}-${prev}`);
  return out.join(", ");
}

export function clamp(value, lo, hi) {
  return Math.min(hi, Math.max(lo, value));
}
