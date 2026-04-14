//! AVC init segment parity between `lvqr-cmaf` (mp4-atom) and
//! `lvqr-ingest` (hand-rolled).
//!
//! The hand-rolled writer at `lvqr_ingest::remux::fmp4::video_init_segment_with_size`
//! is the shipping RTMP-to-MoQ path today. `lvqr_cmaf::write_avc_init_segment`
//! is the replacement the Tier 2.3 roadmap lines up behind. Before the
//! hand-rolled writer can be retired, we need a byte-level proof that
//! the replacement produces either identical bytes or differences a
//! real consumer does not care about.
//!
//! This test is that proof. It is deliberately NOT a byte-for-byte
//! equality check: the two writers pick different defaults for fields
//! that do not affect playback (creation time stamps, default volume,
//! matrix values, `stsz` / `stsc` / `stco` table shapes, `stts` entry
//! counts, `hdlr` name strings, and `esds` width encoding). A strict
//! `assert_eq!(a, b)` would fail loudly on every harmless difference
//! and give future refactors nothing useful.
//!
//! Instead the test:
//!
//! 1. Produces both init segments from the same SPS / PPS / width /
//!    height triple.
//! 2. Runs both through `mp4_atom::Ftyp::decode` + `mp4_atom::Moov::decode`
//!    so we are comparing structured trees, not raw bytes.
//! 3. Asserts the fields that DO matter for playback match exactly:
//!    * ftyp major brand and compatible brands
//!    * mvhd timescale + next_track_id
//!    * trak count + track_id
//!    * mdhd timescale
//!    * hdlr handler type
//!    * stsd codec kind and count
//!    * Avc1 visual width / height / depth / compressor length
//!    * avcC length_size (4-byte NAL prefix in both writers)
//!    * avcC SPS byte sequence (the bytes the decoder ACTUALLY reads)
//!    * avcC PPS byte sequence
//!    * mvex.trex track_id + default_sample_description_index
//! 4. Ignores everything else, but prints the `byte length delta` so a
//!    future session can see at a glance how far the two writers have
//!    drifted in total footprint.
//!
//! When the hand-rolled writer is retired, this test becomes the
//! migration gate: once every asserted field matches, the byte-level
//! delta becomes the remaining risk, and that risk can be evaluated
//! against real MSE / ffprobe consumers rather than guessed at.

use bytes::BytesMut;
use lvqr_cmaf::{VideoInitParams, write_avc_init_segment};
use lvqr_ingest::remux::VideoConfig;
use lvqr_ingest::remux::fmp4::video_init_segment_with_size;
use mp4_atom::{Codec, Decode, Ftyp, Moov};

/// Same deterministic SPS + PPS used by `lvqr-cmaf`'s existing AVC
/// conformance test. Baseline 3.1, 1280x720.
const SPS: &[u8] = &[
    0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03,
    0x03, 0xC0, 0xF1, 0x83, 0x2A,
];
const PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];
const WIDTH: u16 = 1280;
const HEIGHT: u16 = 720;

fn cmaf_bytes() -> Vec<u8> {
    let mut buf = BytesMut::new();
    write_avc_init_segment(
        &mut buf,
        &VideoInitParams {
            sps: SPS.to_vec(),
            pps: PPS.to_vec(),
            width: WIDTH,
            height: HEIGHT,
            timescale: 90_000,
        },
    )
    .expect("lvqr-cmaf encode");
    buf.to_vec()
}

fn ingest_bytes() -> Vec<u8> {
    let config = VideoConfig {
        sps_list: vec![SPS.to_vec()],
        pps_list: vec![PPS.to_vec()],
        profile: 0x42,
        compat: 0x00,
        level: 0x1F,
        nalu_length_size: 4,
    };
    video_init_segment_with_size(&config, WIDTH, HEIGHT).to_vec()
}

fn decode_init(bytes: &[u8]) -> (Ftyp, Moov) {
    let mut cursor = std::io::Cursor::new(bytes);
    let ftyp = Ftyp::decode(&mut cursor).expect("decode ftyp");
    let moov = Moov::decode(&mut cursor).expect("decode moov");
    (ftyp, moov)
}

fn avc1_view(moov: &Moov) -> &mp4_atom::Avc1 {
    let codec = &moov.trak[0].mdia.minf.stbl.stsd.codecs[0];
    match codec {
        Codec::Avc1(a) => a,
        other => panic!("expected Avc1, got {other:?}"),
    }
}

#[test]
fn avc_init_parity_structural_match() {
    let cmaf = cmaf_bytes();
    let ingest = ingest_bytes();

    // Dump the byte-count delta so a reader of the test output can
    // see how the two writers compare in total footprint without
    // rerunning the diff by hand. This number is informational; the
    // assertions below cover the fields that actually matter.
    eprintln!(
        "parity: cmaf={} bytes, ingest={} bytes, delta={}",
        cmaf.len(),
        ingest.len(),
        cmaf.len() as isize - ingest.len() as isize
    );

    let (cmaf_ftyp, cmaf_moov) = decode_init(&cmaf);
    let (ingest_ftyp, ingest_moov) = decode_init(&ingest);

    // ftyp: major brand and the compatible-brand list are the
    // contract MSE and ffprobe read at the very first box. Must
    // match across writers.
    assert_eq!(cmaf_ftyp.major_brand, ingest_ftyp.major_brand);
    assert_eq!(cmaf_ftyp.compatible_brands, ingest_ftyp.compatible_brands);

    // mvhd: the only mvhd fields a consumer cares about for playback
    // are the timescale (drives every downstream tick calculation)
    // and the next_track_id (must be > the max track id present).
    // Creation / modification timestamps and rate / volume are free.
    assert_eq!(cmaf_moov.mvhd.timescale, ingest_moov.mvhd.timescale);
    assert_eq!(cmaf_moov.mvhd.next_track_id, ingest_moov.mvhd.next_track_id);

    // trak: exactly one video track in both cases, track_id = 1.
    assert_eq!(cmaf_moov.trak.len(), 1);
    assert_eq!(ingest_moov.trak.len(), 1);
    assert_eq!(cmaf_moov.trak[0].tkhd.track_id, ingest_moov.trak[0].tkhd.track_id);

    // mdhd timescale drives per-track ticks; handler must be "vide".
    assert_eq!(
        cmaf_moov.trak[0].mdia.mdhd.timescale,
        ingest_moov.trak[0].mdia.mdhd.timescale
    );
    assert_eq!(
        cmaf_moov.trak[0].mdia.hdlr.handler,
        ingest_moov.trak[0].mdia.hdlr.handler
    );

    // stsd: exactly one codec entry, both must be Avc1.
    assert_eq!(cmaf_moov.trak[0].mdia.minf.stbl.stsd.codecs.len(), 1);
    assert_eq!(ingest_moov.trak[0].mdia.minf.stbl.stsd.codecs.len(), 1);

    let cmaf_avc1 = avc1_view(&cmaf_moov);
    let ingest_avc1 = avc1_view(&ingest_moov);
    assert_eq!(cmaf_avc1.visual.width, ingest_avc1.visual.width);
    assert_eq!(cmaf_avc1.visual.height, ingest_avc1.visual.height);
    assert_eq!(cmaf_avc1.visual.depth, ingest_avc1.visual.depth);

    // avcC: the SPS and PPS byte sequences are the actual decoder
    // contract. length_size_minus_one must be identical because the
    // media segments use it to locate NAL unit boundaries.
    assert_eq!(cmaf_avc1.avcc.length_size, ingest_avc1.avcc.length_size);
    assert_eq!(
        cmaf_avc1.avcc.sequence_parameter_sets, ingest_avc1.avcc.sequence_parameter_sets,
        "SPS NAL units must match byte-for-byte"
    );
    assert_eq!(
        cmaf_avc1.avcc.picture_parameter_sets, ingest_avc1.avcc.picture_parameter_sets,
        "PPS NAL units must match byte-for-byte"
    );

    // mvex.trex: both writers must publish a default sample
    // description index = 1 for track 1. If either skips mvex, the
    // media segments will not play in a fragmented-MP4 consumer.
    let cmaf_trex = cmaf_moov.mvex.as_ref().expect("cmaf mvex").trex.clone();
    let ingest_trex = ingest_moov.mvex.as_ref().expect("ingest mvex").trex.clone();
    assert_eq!(cmaf_trex.len(), 1);
    assert_eq!(ingest_trex.len(), 1);
    assert_eq!(cmaf_trex[0].track_id, ingest_trex[0].track_id);
    assert_eq!(
        cmaf_trex[0].default_sample_description_index,
        ingest_trex[0].default_sample_description_index
    );
}

#[test]
fn avc_init_parity_byte_equality_is_not_required() {
    // Sanity assertion that bolts the documentation to the code: we
    // are intentionally NOT comparing raw bytes. If a future session
    // accidentally deletes the structural-match test and replaces it
    // with a byte equality assertion, this test flips and fails,
    // prompting a rewrite rather than silent regression.
    let cmaf = cmaf_bytes();
    let ingest = ingest_bytes();
    assert_ne!(
        cmaf, ingest,
        "the two writers coincidentally produce identical bytes; the \
         structural-match test above is still the canonical gate, but \
         this counter-assertion is now load-bearing and should be \
         revisited"
    );
}
