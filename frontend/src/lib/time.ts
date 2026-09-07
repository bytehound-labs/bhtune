const MINUTE_MS = 60 * 1000;

function parseTimestamp(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return null;

  if (/^\d+$/.test(trimmed)) {
    const numeric = Number(trimmed);
    if (!Number.isFinite(numeric)) return null;

    // The gateway currently sends epoch milliseconds. Accept epoch seconds too so
    // the display remains useful if an older or alternate API emits them.
    return numeric < 100_000_000_000 ? numeric * 1000 : numeric;
  }

  const parsed = Date.parse(trimmed);
  return Number.isNaN(parsed) ? null : parsed;
}

function pluralize(value: number, unit: string): string {
  return `${value} ${unit}${value === 1 ? "" : "s"}`;
}

export function formatTimeUntil(value: string, now = Date.now()): string {
  const timestamp = parseTimestamp(value);
  if (timestamp === null) return "time unavailable";

  const remainingMs = timestamp - now;
  if (remainingMs <= 0) return "due now";

  const totalMinutes = Math.max(1, Math.ceil(remainingMs / MINUTE_MS));
  if (totalMinutes < 60) {
    return `in ${pluralize(totalMinutes, "minute")}`;
  }

  const totalHours = Math.ceil(totalMinutes / 60);
  if (totalHours < 24) {
    return `in ${pluralize(totalHours, "hour")}`;
  }

  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  const parts = [pluralize(days, "day")];
  if (hours > 0) parts.push(pluralize(hours, "hour"));
  return `in ${parts.join(" ")}`;
}

export function formatExactTime(value: string): string | null {
  const timestamp = parseTimestamp(value);
  return timestamp === null ? null : new Date(timestamp).toLocaleString();
}
