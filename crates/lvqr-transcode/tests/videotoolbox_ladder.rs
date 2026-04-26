//! Tier 4 item 4.6 session 156 integration test.
//!
//! Drives the default 720p / 480p / 240p ABR ladder against a real
//! GStreamer VideoToolbox HW encoder pipeline. Mirrors session 105 B's
//! `software_ladder.rs` shape: same conformance fixture, same
//! init / fragment split, same drain-and-collect assertions; only the
//! factory swaps from [`SoftwareTranscoderFactory`] to
//! [`VideoToolboxTranscoderFactory`].
//!
//! The test skips with a log when the GStreamer plugin set is not
//! available on the host (the factory's `is_available()` probe
//! consolidates the missing-element list). CI runners without the
//! `applemedia` plugin from `gst-plugins-bad` see a green test with
//! a clear diagnostic rather than a hard fail. The whole module is
//! gated on `target_os = "macos"` because `vtenc_h264_hw` is the
//! Apple-only HW path.

#![cfg(all(target_os = "macos", feature = "hw-videotoolbox"))]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta, FragmentStream};
use lvqr_transcode::{RenditionSpec, TranscodeRunner, VideoToolboxTranscoderFactory};

const FIXTURE_REL: &str = "../lvqr-conformance/fixtures/fmp4/cmaf-h264-baseline-360p-1s.mp4";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn videotoolbox_ladder_emits_three_renditions() {
    // Skip-with-log when the applemedia plugin is missing.
    let probe_registry = FragmentBroadcasterRegistry::new();
    let probe = VideoToolboxTranscoderFactory::new(RenditionSpec::preset_720p(), probe_registry);
    if !probe.is_available() {
        eprintln!(
            "skipping videotoolbox_ladder: required GStreamer elements missing {:?}. \
             Install gstreamer + plugin set (base, good, bad, ugly, libav) and re-run; \
             vtenc_h264_hw lives in the applemedia plugin from gst-plugins-bad.",
            probe.missing_elements()
        );
        return;
    }
    drop(probe);

    let registry = FragmentBroadcasterRegistry::new();

    let output_registry = registry.clone();
    let _handle = TranscodeRunner::new()
        .with_ladder(RenditionSpec::default_ladder(), move |spec| {
            VideoToolboxTranscoderFactory::new(spec, output_registry.clone())
        })
        .install(&registry);

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_REL);
    let bytes = std::fs::read(&fixture_path).expect("read cmaf-h264-baseline-360p-1s.mp4");
    let (init, frag_body) = split_init_and_remainder(&bytes);
    assert!(!init.is_empty(), "fixture must have ftyp+moov");
    assert!(!frag_body.is_empty(), "fixture must have moof+mdat");

    let source_meta = FragmentMeta::new("avc1.42c01f", 12_800).with_init_segment(Bytes::copy_from_slice(init));
    let source_bc = registry.get_or_create("live/demo", "0.mp4", source_meta);

    source_bc.emit(Fragment::new(
        "0.mp4",
        0,
        0,
        0,
        0,
        0,
        12_800,
        FragmentFlags::KEYFRAME,
        Bytes::copy_from_slice(frag_body),
    ));

    let rendition_names = ["720p", "480p", "240p"];
    let expected_broadcasts = rendition_names.map(|r| format!("live/demo/{r}"));

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let all_present = expected_broadcasts
            .iter()
            .all(|name| registry.get(name, "0.mp4").is_some());
        if all_present {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "output broadcasts did not appear on the registry within 15s; saw keys: {:?}",
                registry.keys()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut subs: Vec<_> = expected_broadcasts
        .iter()
        .map(|name| {
            let bc = registry
                .get(name, "0.mp4")
                .unwrap_or_else(|| panic!("output broadcast {name} missing"));
            (name.clone(), bc.subscribe())
        })
        .collect();

    drop(source_bc);
    registry.remove("live/demo", "0.mp4");

    let mut per_rendition: Vec<(String, usize, usize)> = Vec::new();
    for (name, mut sub) in subs.drain(..) {
        let mut count = 0usize;
        let mut total_bytes = 0usize;
        let drain_deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if Instant::now() >= drain_deadline {
                break;
            }
            match tokio::time::timeout(Duration::from_secs(8), sub.next_fragment()).await {
                Ok(Some(f)) => {
                    count += 1;
                    total_bytes += f.payload.len();
                }
                Ok(None) => break,
                Err(_) => break,
            }
            if count >= 32 {
                break;
            }
        }
        let has_init = registry
            .get(&name, "0.mp4")
            .and_then(|bc| bc.meta().init_segment.clone())
            .map(|b| !b.is_empty())
            .unwrap_or(false);
        eprintln!("rendition {name}: fragments={count}, bytes={total_bytes}, has_init={has_init}",);
        per_rendition.push((name, count, total_bytes));
    }

    for name in &expected_broadcasts {
        registry.remove(name, "0.mp4");
    }

    // Primary assertion: every rendition produced at least one
    // non-header output fragment + non-zero bytes. VT-encoded output
    // bytes are valid H.264 in fMP4 wrapping; the assertions stay
    // generic so the test passes regardless of whether the source
    // fixture happened to align with VT's exact rate-control window.
    for (name, count, bytes) in &per_rendition {
        assert!(
            *count >= 1,
            "rendition {name}: expected >= 1 output fragment, got {count}",
        );
        assert!(*bytes > 0, "rendition {name}: expected > 0 bytes, got {bytes}",);
    }

    // Ordering sanity: 720p output bytes must exceed 240p output
    // bytes for the same 1 s source. Catches a miswired
    // factory / rendition pairing.
    let bytes_for = |suffix: &str| -> usize {
        per_rendition
            .iter()
            .find(|(n, _, _)| n.ends_with(suffix))
            .map(|(_, _, b)| *b)
            .unwrap_or(0)
    };
    let b720 = bytes_for("720p");
    let b240 = bytes_for("240p");
    assert!(
        b720 > b240,
        "720p output ({b720} bytes) must exceed 240p output ({b240} bytes); the ladder rungs may be miswired",
    );
}

/// Scan top-level ISO-BMFF boxes in `file` and return `(init,
/// remainder)` split at the first `moof`. Identical to the helper in
/// `software_ladder.rs`; duplicated here so the two integration tests
/// stay independently maintainable. Path B (no shared scaffolding)
/// per the session 156 brief decision 1.
fn split_init_and_remainder(file: &[u8]) -> (&[u8], &[u8]) {
    let mut offset = 0usize;
    while offset + 8 <= file.len() {
        let size_word = u32::from_be_bytes(file[offset..offset + 4].try_into().unwrap());
        let box_type = &file[offset + 4..offset + 8];
        let box_size = if size_word == 1 {
            if offset + 16 > file.len() {
                return (file, &[]);
            }
            u64::from_be_bytes(file[offset + 8..offset + 16].try_into().unwrap()) as usize
        } else if size_word == 0 {
            return (file, &[]);
        } else {
            size_word as usize
        };
        if box_type == b"moof" {
            return (&file[..offset], &file[offset..]);
        }
        offset = offset.saturating_add(box_size);
    }
    (file, &[])
}
