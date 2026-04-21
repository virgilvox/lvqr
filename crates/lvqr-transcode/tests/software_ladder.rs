//! Tier 4 item 4.6 session 105 B integration test.
//!
//! Drives the default 720p / 480p / 240p ABR ladder against a real
//! GStreamer software pipeline. Loads the CMAF H.264 baseline 360p
//! fixture from `crates/lvqr-conformance/fixtures/fmp4/`, splits it
//! into init (`ftyp+moov`) + fragment (`moof+mdat+mfra`), emits the
//! fragment onto a source [`FragmentBroadcaster`] in a shared
//! [`FragmentBroadcasterRegistry`], then asserts the transcoder
//! republishes three `<source>/<rendition>` broadcasts carrying
//! real x264-encoded output bytes.
//!
//! The test skips with a log when the GStreamer plugin set is not
//! available on the host -- CI runners without the full plugin
//! install surface as a green test with a clear diagnostic rather
//! than a hard fail, matching the factory-side opt-out shape.

#![cfg(feature = "transcode")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta, FragmentStream};
use lvqr_transcode::{RenditionSpec, SoftwareTranscoderFactory, TranscodeRunner};

const FIXTURE_REL: &str = "../lvqr-conformance/fixtures/fmp4/cmaf-h264-baseline-360p-1s.mp4";

/// End-to-end test: source fMP4 fragment in, three rendition fMP4
/// streams out, each carrying x264-encoded output. Exercises the
/// full registry-callback + drain + transcoder worker + appsrc /
/// appsink + rendition republish path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_ladder_emits_three_renditions() {
    // Skip-with-log branch if required plugins are missing on the
    // host. The factory's availability flag consolidates the probe
    // result so the test does not replicate the plugin list.
    let probe_registry = FragmentBroadcasterRegistry::new();
    let probe = SoftwareTranscoderFactory::new(RenditionSpec::preset_720p(), probe_registry);
    if !probe.is_available() {
        eprintln!(
            "skipping software_ladder: required GStreamer elements missing {:?}. \
             Install gstreamer + plugin set (base, good, bad, ugly, libav) and re-run.",
            probe.missing_elements()
        );
        return;
    }
    drop(probe);

    // Shared input + output registry; 105 B publishes output
    // broadcasts into the same registry the source lives on so
    // downstream egress (LL-HLS, MoQ) picks them up without any
    // extra wiring.
    let registry = FragmentBroadcasterRegistry::new();

    // Install the default ladder -- three factories, one per rung.
    let output_registry = registry.clone();
    let _handle = TranscodeRunner::new()
        .with_ladder(RenditionSpec::default_ladder(), move |spec| {
            SoftwareTranscoderFactory::new(spec, output_registry.clone())
        })
        .install(&registry);

    // Load the conformance fixture and split into init + fragment.
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_REL);
    let bytes = std::fs::read(&fixture_path).expect("read cmaf-h264-baseline-360p-1s.mp4");
    let (init, frag_body) = split_init_and_remainder(&bytes);
    assert!(!init.is_empty(), "fixture must have ftyp+moov");
    assert!(!frag_body.is_empty(), "fixture must have moof+mdat");

    // Source broadcast carries the init segment on its FragmentMeta;
    // the transcoder's on_start reads it and pushes as a HEADER
    // buffer before any regular fragment.
    let source_meta = FragmentMeta::new("avc1.42c01f", 12_800).with_init_segment(Bytes::copy_from_slice(init));
    let source_bc = registry.get_or_create("live/demo", "0.mp4", source_meta);

    // Emit the fragment body. Reading the test server end-to-end:
    // this Fragment hits every `TranscoderFactory::build`'s opt-in
    // path (the three software factories), the runner spawns three
    // drain tasks, and each pushes bytes through its own worker.
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

    // Let the runner's registry callback install all three factories
    // (synchronous on our thread) and spawn their drain tasks.
    // Subscribe to every rendition before dropping the source so no
    // output is missed by a late-joining subscriber.
    let rendition_names = ["720p", "480p", "240p"];
    let expected_broadcasts = rendition_names.map(|r| format!("live/demo/{r}"));

    // Output broadcasts are created lazily inside
    // SoftwareTranscoder::on_start; poll until they appear on the
    // registry.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let all_present = expected_broadcasts
            .iter()
            .all(|name| registry.get(name, "0.mp4").is_some());
        if all_present {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "output broadcasts did not appear on the registry within 10s; saw keys: {:?}",
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

    // Drop the source broadcaster + remove the registry entry to
    // trigger EOS. The runner's drain task then calls on_stop on
    // every rendition; each worker thread pushes EOS down its
    // pipeline; mp4mux flushes the final fragment; appsink emits
    // the tail buffers; worker joins.
    drop(source_bc);
    registry.remove("live/demo", "0.mp4");

    // Collect output per rendition with a bounded wait. Each
    // rendition's stream terminates when every producer-side clone
    // of its output broadcaster drops; that happens when the
    // transcoder's worker thread exits and the
    // SoftwareTranscoder is dropped.
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
                // safety cap so a runaway encoder cannot hang the test
                break;
            }
        }
        // Re-read the output broadcaster so the test can also
        // introspect `init_segment` on meta after drain.
        let has_init = registry
            .get(&name, "0.mp4")
            .and_then(|bc| bc.meta().init_segment.clone())
            .map(|b| !b.is_empty())
            .unwrap_or(false);
        eprintln!("rendition {name}: fragments={count}, bytes={total_bytes}, has_init={has_init}",);
        per_rendition.push((name, count, total_bytes));
    }

    // Clean up output broadcasters so drain tasks + worker threads exit.
    for name in &expected_broadcasts {
        registry.remove(name, "0.mp4");
    }

    // Primary assertion: every rendition produced at least one
    // non-header output fragment + non-zero bytes.
    for (name, count, bytes) in &per_rendition {
        assert!(
            *count >= 1,
            "rendition {name}: expected >= 1 output fragment, got {count}",
        );
        assert!(*bytes > 0, "rendition {name}: expected > 0 bytes, got {bytes}",);
    }

    // Coarse bitrate check: the fixture is 1 s of content, so
    // `bytes * 8 / 1000` is kbps. x264enc at tune=zerolatency +
    // speed-preset=superfast lands within +/-15% of the target for
    // the default ladder on this fixture; allow +/-40% so CI
    // variance + startup rate-control jitter do not flake the gate.
    // A larger fixture + matching tightening lands in 106 C.
    for (name, _count, bytes) in &per_rendition {
        let target_kbps = if name.ends_with("/720p") {
            2_500
        } else if name.ends_with("/480p") {
            1_200
        } else if name.ends_with("/240p") {
            400
        } else {
            panic!("unexpected rendition {name}");
        };
        let measured_kbps = (*bytes as u64 * 8 / 1_000) as u32;
        let low = (target_kbps as f64 * 0.6).floor() as u32;
        let high = (target_kbps as f64 * 1.4).ceil() as u32;
        assert!(
            (low..=high).contains(&measured_kbps),
            "rendition {name}: measured {measured_kbps} kbps outside coarse \
             window [{low}, {high}] (target {target_kbps})",
        );
    }

    // Ordering sanity: the three rendition bitrates are 2500 / 1200 /
    // 400 kbps, so the higher-rung output must carry more bytes than
    // the lower rung for the same source. This catches a swapped
    // factory / rendition wiring that would otherwise ship a
    // "working" ladder with every rung at the lowest bitrate.
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

/// Scan top-level ISO-BMFF boxes in `file` and return
/// `(init, remainder)` split at the first `moof`.
///
/// The LVQR convention is that `FragmentMeta::init_segment` carries
/// `ftyp + moov` and each `Fragment::payload` carries `moof + mdat`
/// (plus an optional trailing `mfra`). Splitting the conformance
/// fixture the same way exercises both the pipeline's
/// `BufferFlags::HEADER` init push and the regular fragment push
/// path.
fn split_init_and_remainder(file: &[u8]) -> (&[u8], &[u8]) {
    let mut offset = 0usize;
    while offset + 8 <= file.len() {
        let size_word = u32::from_be_bytes(file[offset..offset + 4].try_into().unwrap());
        let box_type = &file[offset + 4..offset + 8];
        let box_size = if size_word == 1 {
            // 64-bit extended size at offset+8..+16.
            if offset + 16 > file.len() {
                return (file, &[]);
            }
            u64::from_be_bytes(file[offset + 8..offset + 16].try_into().unwrap()) as usize
        } else if size_word == 0 {
            // Box extends to EOF; impossible to split.
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
