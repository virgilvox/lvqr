// Pure SCTE-35 marker arithmetic for the LVQR DVR player.
//
// Consumes hls.js's `LevelDetails.dateRanges` shape (the
// pre-parsed `#EXT-X-DATERANGE` entries from session 152's
// SCTE-35 ad-marker passthrough surface) and produces a normalised
// marker list the seek-bar renderer draws on top of the played
// fill. All functions in this module are total and side-effect
// free; the seek-bar render layer in `index.ts` handles the DOM
// and event surfaces.
//
// `DvrMarker` is the public shape (re-exported from index.ts)
// integrators consume via the new `getMarkers()` programmatic API
// and via the `lvqr-dvr-markers-changed` / `lvqr-dvr-marker-crossed`
// event detail payloads.

import { type SeekableRange, timeToFraction } from './seekbar.js';

/** Which `SCTE35-*` attribute carried the marker on the wire. */
export type DvrMarkerKind = 'out' | 'in' | 'cmd' | 'unknown';

/**
 * One normalised SCTE-35 ad-marker derived from a single
 * `#EXT-X-DATERANGE` entry in the served HLS playlist.
 */
export interface DvrMarker {
  /** DATERANGE `ID` attribute. Stable across playlist refreshes. */
  id: string;
  /** Which SCTE35-* attribute the wire carried; see DvrMarkerKind. */
  kind: DvrMarkerKind;
  /**
   * `currentTime` offset (seconds) inside the seekable range,
   * pre-computed by hls.js from the playlist's
   * `#EXT-X-PROGRAM-DATE-TIME` anchor. NaN when the playlist has
   * no PDT (LVQR's relay always emits PDT, but a hand-crafted
   * test playlist may not).
   */
  startTime: number;
  /** Wall-clock RFC 3339 anchor straight from the DATERANGE. */
  startDate: Date;
  /** DURATION attribute value in seconds, when set. */
  durationSecs: number | null;
  /** CLASS attribute, when set. */
  class: string | null;
  /** Raw SCTE35-* hex value (`0x...`), when present. */
  scte35Hex: string | null;
}

/**
 * Pairing status for a daterange ID after OUT/IN reduction.
 *
 * * `pair` -- both an OUT and an IN with the same ID. Renders as
 *   a coloured break-range span between the two start times.
 * * `open` -- an OUT without a matching IN (the IN has not yet
 *   arrived). Renders as an in-flight overlay from the OUT's
 *   start time to the live edge.
 * * `in-only` -- an IN without a matching OUT (the OUT has aged
 *   out of the playlist's sliding window). Renders as a tick at
 *   the IN's start time.
 * * `singleton` -- a CMD or unknown entry with no pairing
 *   semantics. Renders as a tick at the marker's start time.
 */
export type DvrMarkerPairKind = 'pair' | 'open' | 'in-only' | 'singleton';

export interface DvrMarkerPair {
  id: string;
  out: DvrMarker | null;
  in: DvrMarker | null;
  kind: DvrMarkerPairKind;
}

/**
 * Structural shape the marker pipeline consumes. hls.js's
 * `DateRange` class satisfies this without explicit import; the
 * internal `AttrList` is index-accessed as a string-keyed
 * record. Keeping the interface structural lets callers feed
 * stub data into the helpers from unit tests without depending
 * on hls.js types.
 */
export interface HlsDateRangeLike {
  id: string;
  class?: string;
  startTime: number;
  startDate: Date | null;
  duration: number | null;
  attr: Record<string, string | undefined>;
}

function hasAttr(a: Record<string, string | undefined>, key: string): boolean {
  return typeof a[key] === 'string' && a[key]!.length > 0;
}

/**
 * Map a daterange to a single primary kind. When BOTH SCTE35-OUT
 * and SCTE35-IN are present on the same merged DateRange (the
 * usual outcome when hls.js merges separate OUT + IN playlist
 * entries with the same ID), this returns `out`; the
 * `dvrMarkersFromHlsDateRanges` adapter then emits two markers
 * for that single DateRange so both sides render. Callers that
 * want to know whether the merged DateRange has both can use
 * `dvrMarkersFromHlsDateRanges` directly and inspect the
 * resulting marker list.
 */
export function classifyMarker(dr: HlsDateRangeLike): DvrMarkerKind {
  const a = dr.attr ?? {};
  if (hasAttr(a, 'SCTE35-OUT')) return 'out';
  if (hasAttr(a, 'SCTE35-IN')) return 'in';
  if (hasAttr(a, 'SCTE35-CMD')) return 'cmd';
  return 'unknown';
}

function attrScte35Hex(dr: HlsDateRangeLike, kind: DvrMarkerKind): string | null {
  const a = dr.attr ?? {};
  switch (kind) {
    case 'out': return a['SCTE35-OUT'] ?? null;
    case 'in': return a['SCTE35-IN'] ?? null;
    case 'cmd': return a['SCTE35-CMD'] ?? null;
    case 'unknown': return null;
  }
}

/**
 * Convert a record of hls.js DATERANGE entries into the
 * component's normalised marker list. Drops entries whose
 * `startTime` is non-finite (no PDT anchor on the playlist).
 *
 * For a SCTE35-OUT entry that carries a DURATION attribute,
 * this adapter emits TWO markers:
 *
 * * an OUT marker at `startTime` carrying the OUT hex,
 * * an IN marker at `startTime + duration` (the announced end
 *   of the ad break, derived from the splice_insert
 *   `break_duration`).
 *
 * The IN is emitted from the OUT's duration rather than from a
 * second DATERANGE because hls.js (v1.5/1.6) rejects DATERANGE
 * merges when same-ID entries have different `START-DATE`
 * values: per the HLS spec section 4.4.5.1.4, conflicting
 * attribute values for a shared ID flag the entry as invalid
 * (`DateRange.isValid === false`) and the parser drops it.
 * Since LVQR's relay emits OUT and IN as separate playlist
 * entries with different `START-DATE` (so the IN's start time
 * is wire-side-explicit), the second entry is dropped by hls.js
 * and only the OUT survives. Deriving the IN from the OUT's
 * DURATION yields the same visual break range without relying
 * on the merge to succeed. Production publishers (Wirecast,
 * vMix, AWS Elemental) always set `break_duration`, so DURATION
 * is normally present.
 *
 * If the OUT has no DURATION (in-flight break with no announced
 * end), only the OUT marker is emitted; the renderer paints an
 * in-flight overlay running to the live edge instead of a
 * closed span.
 *
 * Sort order: ascending `startTime`, then ascending `id`, then
 * ascending `kind` for ties.
 */
export function dvrMarkersFromHlsDateRanges(
  dateRanges: Record<string, HlsDateRangeLike | undefined>,
): DvrMarker[] {
  const out: DvrMarker[] = [];
  for (const id of Object.keys(dateRanges)) {
    const dr = dateRanges[id];
    if (!dr) continue;
    if (!Number.isFinite(dr.startTime)) continue;
    const a = dr.attr ?? {};
    const durationSecs =
      typeof dr.duration === 'number' && Number.isFinite(dr.duration) ? dr.duration : null;
    const klass = typeof dr.class === 'string' && dr.class.length > 0 ? dr.class : null;
    const startDate = dr.startDate ?? new Date(NaN);
    const kind = classifyMarker(dr);
    out.push({
      id: dr.id,
      kind,
      startTime: dr.startTime,
      startDate,
      durationSecs,
      class: klass,
      scte35Hex: attrScte35Hex(dr, kind),
    });
    // For OUT entries with a known DURATION, synthesize a
    // matching IN marker at startTime + duration. See doc
    // comment above for why we derive the IN rather than
    // expecting hls.js to expose a separate IN entry.
    if (kind === 'out' && durationSecs !== null && durationSecs > 0) {
      out.push({
        id: dr.id,
        kind: 'in',
        startTime: dr.startTime + durationSecs,
        startDate,
        durationSecs: null,
        class: klass,
        // The wire's IN hex is unavailable on the merged DateRange
        // (dropped by hls.js when START-DATE conflicts); we surface
        // the OUT's hex so consumers still get a SCTE-35 reference.
        scte35Hex: a['SCTE35-IN'] ?? a['SCTE35-OUT'] ?? null,
      });
    }
  }
  out.sort((a, b) => {
    if (a.startTime !== b.startTime) return a.startTime - b.startTime;
    if (a.id < b.id) return -1;
    if (a.id > b.id) return 1;
    return a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0;
  });
  return out;
}

/**
 * Map a marker's `startTime` to a fraction along the seek bar.
 * Returns `null` when the marker falls outside the seekable
 * range or when its start time is non-finite, so the renderer
 * can suppress drawing instead of clamping (clamping would pin
 * the tick at an endpoint, suggesting a marker that is not
 * actually at that time).
 */
export function markerToFraction(marker: DvrMarker, range: SeekableRange): number | null {
  if (!Number.isFinite(marker.startTime)) return null;
  if (marker.startTime < range.start) return null;
  if (marker.startTime > range.end) return null;
  return timeToFraction(marker.startTime, range);
}

/**
 * Reduce a marker list into ID-keyed groups. OUT + IN with the
 * same ID become one `pair`; an orphan OUT becomes an `open`
 * group; an orphan IN becomes an `in-only` group; CMD and
 * unknown markers become `singleton` groups. The `pair` group's
 * out/in are swapped if their start times are reversed (clock
 * skew or hand-crafted test data).
 *
 * Output order: ascending by the group's earliest start time,
 * matching the marker list's sort.
 */
export function groupOutInPairs(markers: ReadonlyArray<DvrMarker>): DvrMarkerPair[] {
  const buckets = new Map<string, { out: DvrMarker | null; in: DvrMarker | null }>();
  const singletons: DvrMarkerPair[] = [];

  for (const m of markers) {
    if (m.kind === 'out') {
      const b = buckets.get(m.id) ?? { out: null, in: null };
      b.out = m;
      buckets.set(m.id, b);
    } else if (m.kind === 'in') {
      const b = buckets.get(m.id) ?? { out: null, in: null };
      b.in = m;
      buckets.set(m.id, b);
    } else {
      // Both 'cmd' and 'unknown' kinds collapse into a singleton
      // group. Renderer uses `out` as the marker reference; the
      // underlying SCTE-35 kind is on `out.kind` (so 'unknown'
      // can render as a neutral tick without an SCTE-35 tooltip
      // body, while 'cmd' carries its hex value for the body).
      singletons.push({ id: m.id, out: m, in: null, kind: 'singleton' });
    }
  }

  const grouped: DvrMarkerPair[] = [];
  for (const [id, b] of buckets) {
    if (b.out && b.in) {
      let outM = b.out;
      let inM = b.in;
      if (outM.startTime > inM.startTime) {
        const tmp = outM;
        outM = inM;
        inM = tmp;
      }
      grouped.push({ id, out: outM, in: inM, kind: 'pair' });
    } else if (b.out) {
      grouped.push({ id, out: b.out, in: null, kind: 'open' });
    } else if (b.in) {
      grouped.push({ id, out: null, in: b.in, kind: 'in-only' });
    }
  }

  const all = [...grouped, ...singletons];
  all.sort((a, b) => groupStartTime(a) - groupStartTime(b));
  return all;
}

function groupStartTime(g: DvrMarkerPair): number {
  if (g.out && Number.isFinite(g.out.startTime)) return g.out.startTime;
  if (g.in && Number.isFinite(g.in.startTime)) return g.in.startTime;
  return Number.POSITIVE_INFINITY;
}

/**
 * Format a duration (seconds) for tooltip display. Values under
 * 60 seconds show three decimal places (e.g. "12.000s"); values
 * 60 seconds and over show "M:SS"; values an hour or longer show
 * "H:MM:SS". Negative values clamp to zero.
 */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, seconds);
  if (s < 60) return `${s.toFixed(3)}s`;
  if (s < 3600) {
    const mm = Math.floor(s / 60);
    const ss = Math.floor(s % 60);
    return `${mm}:${pad2(ss)}`;
  }
  const hh = Math.floor(s / 3600);
  const mm = Math.floor((s % 3600) / 60);
  const ss = Math.floor(s % 60);
  return `${hh}:${pad2(mm)}:${pad2(ss)}`;
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}
