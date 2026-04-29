# `@lvqr/player` Changelog

User-visible changes to the LVQR live-playback web component
published on npm. The head of `main` is always the source of
truth; this file summarises shipped + unreleased work between
npm releases. For session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../../../tracking/HANDOFF.md).

## [1.0.0] - 2026-04-28

First major release. Stability commitment for the live-playback
component surface; the entire SDK family moves to 1.0.0 together
(`@lvqr/core`, `@lvqr/dvr-player`, `@lvqr/admin-ui`, Python `lvqr`,
and the Rust workspace).

### Changed

* **Renamed 0.3.2 -> 1.0.0.** No component shape changes vs. 0.3.2;
  every attribute, event, and shadow part keeps its existing shape.
* **`@lvqr/core` dependency pinned exact at `1.0.0`** (was exact
  `0.3.2`). Matches the pre-existing exact-pin pattern that prevents
  semver drift on the player surface; consumers wanting the floating
  pin can override in their own package.json.

## [0.3.2] - 2026-04-24

Tracker-only republish alongside `@lvqr/core` 0.3.2 + Rust
0.4.1. No component shape changes.
