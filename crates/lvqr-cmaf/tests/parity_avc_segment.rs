//! Parity gate: `TrackCoalescer` vs the hand-rolled
//! `lvqr-ingest::remux::fmp4::video_segment` writer.
//!
//! Sibling of `parity_avc_init.rs` for the media-segment side of
//! the Tier 2.3 migration. The session-7 init-segment parity gate
//! proved the two init-segment writers produce structurally
//! equivalent output; session 9 landed a coalescer that emits its
//! own `moof + mdat` via `mp4-atom`; this test pins that those
//! bytes are structurally equivalent to what the hand-rolled
//! writer produces for the same sample sequence.
//!
//! The assertion style matches `parity_avc_init.rs`: this is NOT a
//! byte-for-byte equality check. The two writers pick different
//! encode paths (mp4-atom goes through typed structs; the
//! hand-rolled path writes box headers directly into a BytesMut)
//! and produce different byte footprints in fields that do not
//! affect playback. The parity gate asserts the fields that DO
//! matter:
//!
//! * `mfhd.sequence_number`
//! * `traf.tfhd.track_id`
//! * `traf.tfdt.base_media_decode_time`
//! * `traf.trun.entries.len()`
//! * per-sample duration, size, flags, and cts offset
//! * `trun.data_offset != 0` (both writers emit a real offset,
//!   though the numeric value differs because the two moof
//!   sizes differ)
//!
//! When the hand-rolled writer is eventually retired behind a
//! feature flag, this test becomes the gate that catches drift
//! between the two sides.

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{CmafPolicy, RawSample, TrackCoalescer};
use lvqr_ingest::remux::fmp4::{VideoSample, video_segment};
use mp4_atom::{Decode, Moof};

fn avcc_nal(total_len: u32, nal_header: u8) -> Bytes {
    assert!(total_len >= 5);
    let body_len = total_len - 4;
    let mut buf = BytesMut::with_capacity(total_len as usize);
    buf.extend_from_slice(&body_len.to_be_bytes());
    buf.extend_from_slice(&[nal_header]);
    buf.extend_from_slice(&vec![0u8; (body_len - 1) as usize]);
    buf.freeze()
}

fn decode_moof(bytes: &[u8]) -> Moof {
    let mut cursor = std::io::Cursor::new(bytes);
    Moof::decode(&mut cursor).expect("decode moof")
}

/// Build both writers' output for the same input and compare the
/// structural fields.
#[test]
fn coalescer_parity_with_hand_rolled_video_segment() {
    // Six samples at 3000 ticks each: one IDR keyframe followed
    // by five P-slices. Inside one LL-HLS partial window (18_000
    // ticks = 200 ms at 90 kHz), so the coalescer flushes the
    // whole batch as a single chunk and can be compared directly
    // against the hand-rolled output.
    let base_dts: u64 = 0;
    let sample_duration: u32 = 3000;
    let mut cmaf_samples = Vec::new();
    let mut ingest_samples = Vec::new();
    for i in 0..6 {
        let dts = base_dts + (i as u64 * sample_duration as u64);
        let keyframe = i == 0;
        let nal_header = if keyframe { 0x65 } else { 0x41 };
        let size = 48 + (i as u32 * 8);
        let payload = avcc_nal(size, nal_header);
        cmaf_samples.push(RawSample {
            track_id: 1,
            dts,
            cts_offset: 0,
            duration: sample_duration,
            payload: payload.clone(),
            keyframe,
        });
        ingest_samples.push(VideoSample {
            data: payload,
            duration: sample_duration,
            cts_offset: 0,
            keyframe,
        });
    }

    // Drive the cmaf coalescer. The partial window is 18_000
    // ticks; pushing six samples that each take 3000 ticks
    // accumulates exactly 18_000 ticks, which is the boundary.
    // The coalescer only flushes on a push that *crosses* the
    // boundary, so we need one final flush() to drain the
    // pending batch after the six pushes.
    let mut c = TrackCoalescer::new(1, CmafPolicy::VIDEO_90KHZ_DEFAULT);
    let mut emitted = Vec::new();
    for s in cmaf_samples {
        if let Some(chunk) = c.push(s) {
            emitted.push(chunk);
        }
    }
    if let Some(chunk) = c.flush() {
        emitted.push(chunk);
    }
    assert_eq!(
        emitted.len(),
        1,
        "coalescer should emit exactly one chunk for the batch"
    );
    let cmaf_chunk = emitted.into_iter().next().unwrap();

    // Drive the hand-rolled writer with the matching sequence
    // number (1 == the coalescer's first emit) and the same
    // base_dts.
    let ingest_bytes = video_segment(1, base_dts, &ingest_samples);

    eprintln!(
        "parity: cmaf={} bytes, ingest={} bytes, delta={}",
        cmaf_chunk.payload.len(),
        ingest_bytes.len(),
        cmaf_chunk.payload.len() as isize - ingest_bytes.len() as isize
    );

    // Parse both moof boxes and compare structural fields.
    let cmaf_moof = decode_moof(&cmaf_chunk.payload);
    let ingest_moof = decode_moof(&ingest_bytes);

    // mfhd: sequence number.
    assert_eq!(cmaf_moof.mfhd.sequence_number, ingest_moof.mfhd.sequence_number);

    // traf count and track id.
    assert_eq!(cmaf_moof.traf.len(), 1);
    assert_eq!(ingest_moof.traf.len(), 1);
    assert_eq!(cmaf_moof.traf[0].tfhd.track_id, ingest_moof.traf[0].tfhd.track_id);

    // tfdt base media decode time.
    assert_eq!(
        cmaf_moof.traf[0].tfdt.as_ref().map(|t| t.base_media_decode_time),
        ingest_moof.traf[0].tfdt.as_ref().map(|t| t.base_media_decode_time)
    );

    // trun entries: same count, same durations, sizes, flags, cts.
    let cmaf_trun = &cmaf_moof.traf[0].trun[0];
    let ingest_trun = &ingest_moof.traf[0].trun[0];
    assert_eq!(cmaf_trun.entries.len(), ingest_trun.entries.len());
    for (i, (a, b)) in cmaf_trun.entries.iter().zip(ingest_trun.entries.iter()).enumerate() {
        assert_eq!(a.duration, b.duration, "duration[{i}]");
        assert_eq!(a.size, b.size, "size[{i}]");
        assert_eq!(a.flags, b.flags, "flags[{i}]");
        assert_eq!(a.cts, b.cts, "cts[{i}]");
    }

    // data_offset: both writers must publish a positive offset
    // that lands inside the final buffer (moof_size + 8). The
    // numeric values differ because the two moof sizes differ;
    // what we assert is that both are positive and that the
    // offset correctly points past the moof into the mdat.
    let cmaf_offset = cmaf_trun.data_offset.unwrap();
    let ingest_offset = ingest_trun.data_offset.unwrap();
    assert!(cmaf_offset > 0);
    assert!(ingest_offset > 0);
    // Sanity: the offset plus the mdat body length equals the
    // total buffer size plus 8 (the mdat header the coalescer
    // writes by hand and mp4-atom does not re-decode through
    // Moof::decode).
    let mdat_body: u32 = cmaf_trun.entries.iter().map(|e| e.size.unwrap()).sum();
    assert_eq!(
        cmaf_chunk.payload.len() as u32,
        cmaf_offset as u32 + mdat_body,
        "cmaf buffer size must equal data_offset + mdat_body"
    );
    assert_eq!(
        ingest_bytes.len() as u32,
        ingest_offset as u32 + mdat_body,
        "ingest buffer size must equal data_offset + mdat_body"
    );
}

#[test]
fn coalescer_parity_byte_equality_is_not_required() {
    // Counterpart to `parity_avc_init.rs`'s non-equality pin: the
    // parity test above is intentionally structural, not
    // byte-exact. If a future session accidentally replaces it
    // with a byte-equality assertion, this test fails and
    // prompts a rewrite rather than a silent regression.
    let base_dts: u64 = 0;
    let samples_cmaf = vec![
        RawSample::keyframe(1, 0, 3000, avcc_nal(64, 0x65)),
        RawSample::delta(1, 3000, 3000, avcc_nal(48, 0x41)),
    ];
    let samples_ingest = vec![
        VideoSample {
            data: avcc_nal(64, 0x65),
            duration: 3000,
            cts_offset: 0,
            keyframe: true,
        },
        VideoSample {
            data: avcc_nal(48, 0x41),
            duration: 3000,
            cts_offset: 0,
            keyframe: false,
        },
    ];

    let mut c = TrackCoalescer::new(1, CmafPolicy::VIDEO_90KHZ_DEFAULT);
    for s in samples_cmaf {
        let _ = c.push(s);
    }
    let cmaf = c.flush().expect("flush");

    let ingest = video_segment(1, base_dts, &samples_ingest);

    assert_ne!(
        cmaf.payload.as_ref(),
        ingest.as_ref(),
        "the two writers coincidentally produce identical media segment bytes; \
         the structural-match test above is still the canonical gate, but this \
         counter-assertion is now load-bearing and should be revisited"
    );
}
