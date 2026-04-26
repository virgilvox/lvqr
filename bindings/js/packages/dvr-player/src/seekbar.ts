// Pure seek-bar arithmetic for the LVQR DVR player.
//
// Extracted into its own module so the math can be unit-tested
// without a DOM. Every function here is total and side-effect free.

export interface SeekableRange {
  start: number;
  end: number;
}

export interface PercentileLabel {
  /** Position along the seek bar in [0, 1]. */
  fraction: number;
  /** Absolute time in seconds inside the seekable range. */
  time: number;
  /** Pre-formatted label, e.g. "00:30" or "01:23:45". */
  text: string;
}

/**
 * Map a position fraction [0, 1] inside the seek-bar's drawable area
 * back to a media `currentTime` inside the seekable range.
 */
export function fractionToTime(fraction: number, range: SeekableRange): number {
  const f = clamp(fraction, 0, 1);
  return range.start + (range.end - range.start) * f;
}

/**
 * Map a media `currentTime` to a fraction in [0, 1] along the
 * seek-bar's drawable area. Clamps for currentTime outside the
 * range -- the caller can choose to draw a truncated thumb or
 * suppress rendering entirely.
 */
export function timeToFraction(time: number, range: SeekableRange): number {
  const span = range.end - range.start;
  if (span <= 0) return 0;
  return clamp((time - range.start) / span, 0, 1);
}

/**
 * Format seconds as `HH:MM:SS` for ranges spanning at least one
 * hour, otherwise as `MM:SS`. Negative values clamp to zero.
 */
export function formatTime(seconds: number, totalSpanSecs: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const showHours = totalSpanSecs >= 3600;
  const hh = Math.floor(s / 3600);
  const mm = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  if (showHours) {
    return `${pad2(hh)}:${pad2(mm)}:${pad2(ss)}`;
  }
  return `${pad2(mm)}:${pad2(ss)}`;
}

/**
 * Generate percentile labels (0%, 25%, 50%, 75%, 100% by default)
 * across the seekable range. Labels are formatted with `formatTime`
 * using the range span, so a 30-second range gets `MM:SS` and a
 * one-hour range gets `HH:MM:SS`.
 */
export function generatePercentileLabels(
  range: SeekableRange,
  fractions: ReadonlyArray<number> = [0, 0.25, 0.5, 0.75, 1],
): PercentileLabel[] {
  const span = range.end - range.start;
  return fractions.map((fraction) => {
    const time = fractionToTime(fraction, range);
    return {
      fraction,
      time,
      text: formatTime(time - range.start, span),
    };
  });
}

/**
 * Decide whether the player is "at the live edge". `deltaSecs` is
 * `seekableEnd - currentTime`. The live-edge check returns true
 * when the delta is strictly less than the threshold.
 */
export function isAtLiveEdge(deltaSecs: number, thresholdSecs: number): boolean {
  return deltaSecs < thresholdSecs;
}

function clamp(n: number, lo: number, hi: number): number {
  if (n < lo) return lo;
  if (n > hi) return hi;
  return n;
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}
