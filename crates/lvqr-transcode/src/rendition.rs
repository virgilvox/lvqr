//! [`RenditionSpec`] plus preset constructors for the default
//! LVQR ABR ladder.

use serde::{Deserialize, Serialize};

/// One rendition in an ABR ladder. Carries the target geometry +
/// bitrates a downstream encoder uses to produce output fragments.
///
/// Session 104 A captures only the minimum set every software +
/// hardware encoder consumes. Session 105 B extends this with
/// codec-specific knobs (x264 profile / tune / keyint, NVENC
/// quality preset, VideoToolbox pixel format) layered on
/// rather than replacing these fields, so existing consumers stay
/// source-compatible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenditionSpec {
    /// Short human-readable identifier (`"720p"` / `"480p"` /
    /// `"240p"`). Used as:
    ///
    /// * The rendition suffix on the output broadcast name
    ///   (`<source>/<name>`).
    /// * A Prometheus metric label
    ///   (`lvqr_transcode_fragments_total{rendition="720p"}`).
    /// * The HLS master-playlist `NAME=` attribute (landed in
    ///   session 106 C).
    ///
    /// Pick something short, lowercase, no slashes. Validation is
    /// the operator's responsibility for now; session 106 C's
    /// CLI flag will enforce the character set.
    pub name: String,

    /// Target frame width in pixels. Downstream encoders use this
    /// to configure the `videoscale` element (or the hardware
    /// encoder's equivalent).
    pub width: u32,

    /// Target frame height in pixels.
    pub height: u32,

    /// Target video bitrate in kilobits / second. Upstream to the
    /// encoder's `bitrate` property. Typical 720p h264 lands at
    /// 2-3 Mb/s; 480p at 1-1.5 Mb/s; 240p at 300-500 kb/s.
    pub video_bitrate_kbps: u32,

    /// Target audio bitrate in kilobits / second. Upstream to the
    /// audio encoder (AAC in 105 B) or passed through when the
    /// rendition reuses the source audio track. 96-128 kb/s at
    /// 48 kHz stereo is the typical range.
    pub audio_bitrate_kbps: u32,
}

impl RenditionSpec {
    /// Construct a custom rendition with the supplied fields.
    pub fn new(
        name: impl Into<String>,
        width: u32,
        height: u32,
        video_bitrate_kbps: u32,
        audio_bitrate_kbps: u32,
    ) -> Self {
        Self {
            name: name.into(),
            width,
            height,
            video_bitrate_kbps,
            audio_bitrate_kbps,
        }
    }

    /// 720p preset: `1280x720` at 2.5 Mb/s video + 128 kb/s audio.
    /// Matches the `tracking/TIER_4_PLAN.md` section 4.6 default.
    pub fn preset_720p() -> Self {
        Self::new("720p", 1280, 720, 2_500, 128)
    }

    /// 480p preset: `854x480` at 1.2 Mb/s video + 96 kb/s audio.
    pub fn preset_480p() -> Self {
        Self::new("480p", 854, 480, 1_200, 96)
    }

    /// 240p preset: `426x240` at 400 kb/s video + 64 kb/s audio.
    pub fn preset_240p() -> Self {
        Self::new("240p", 426, 240, 400, 64)
    }

    /// Default 3-rung LVQR ladder, ordered highest-to-lowest so
    /// operators reading logs or admin output see the ladder's
    /// top rung first. HLS master-playlist composition in
    /// session 106 C sorts independently by `BANDWIDTH`.
    pub fn default_ladder() -> Vec<Self> {
        vec![Self::preset_720p(), Self::preset_480p(), Self::preset_240p()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_plan_defaults() {
        let r = RenditionSpec::preset_720p();
        assert_eq!(r.name, "720p");
        assert_eq!((r.width, r.height), (1280, 720));
        assert_eq!(r.video_bitrate_kbps, 2_500);
        assert_eq!(r.audio_bitrate_kbps, 128);

        let r = RenditionSpec::preset_480p();
        assert_eq!((r.width, r.height), (854, 480));

        let r = RenditionSpec::preset_240p();
        assert_eq!((r.width, r.height), (426, 240));
    }

    #[test]
    fn default_ladder_is_highest_to_lowest() {
        let ladder = RenditionSpec::default_ladder();
        assert_eq!(ladder.len(), 3);
        // Monotonically decreasing video bitrate from top rung to
        // bottom: the ordering convention operators rely on when
        // scanning admin output.
        for pair in ladder.windows(2) {
            assert!(
                pair[0].video_bitrate_kbps > pair[1].video_bitrate_kbps,
                "ladder must be highest-to-lowest; got {} before {}",
                pair[0].name,
                pair[1].name,
            );
        }
    }

    #[test]
    fn rendition_spec_round_trips_through_json() {
        let r = RenditionSpec::new("custom", 1920, 1080, 5_000, 192);
        let j = serde_json::to_string(&r).expect("serialize");
        let parsed: RenditionSpec = serde_json::from_str(&j).expect("deserialize");
        assert_eq!(parsed, r);
    }
}
