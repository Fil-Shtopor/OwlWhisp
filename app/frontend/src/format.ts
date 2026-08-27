// Presentation helpers shared by the Models and Benchmark panels.
//
// House rule: an estimate is never rendered the way a measurement is. `formatEstimatedRtf` always
// carries the "(estimated)" suffix; `formatRtf` is only ever fed numbers that were measured.

const BYTE_UNITS = ["KiB", "MiB", "GiB", "TiB"] as const;

/** Humanise a byte count, binary units. `null` becomes an explicit "unknown". */
export function formatBytes(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) return "unknown";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < BYTE_UNITS.length - 1) {
    value /= 1024;
    index += 1;
  }
  const unit = BYTE_UNITS[index];
  return `${value < 10 ? value.toFixed(1) : Math.round(value).toString()} ${unit}`;
}

/** A measured real-time factor. Never use this for catalog estimates. */
export function formatRtf(rtf: number): string {
  return rtf.toFixed(4);
}

/** A catalog estimate. The "(estimated)" tail is not optional — it is the point. */
export function formatEstimatedRtf(rtf: number | null): string {
  if (rtf === null) return "not estimated";
  return `~${rtf.toFixed(4)} (estimated)`;
}

/** WER arrives as a fraction (0.054 = 5.4%). */
export function formatWer(wer: number | null): string {
  if (wer === null) return "—";
  return `${(wer * 100).toFixed(1)}%`;
}

/** Milliseconds, rounded; seconds once it gets long enough that ms stop reading well. */
export function formatMs(ms: number): string {
  if (ms >= 10_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.round(ms)} ms`;
}

export function formatSeconds(secs: number): string {
  return `${secs.toFixed(2)} s`;
}
