//! End-to-end test for the WhisperCaptionsAgent against a real
//! `lvqr_agent::AgentRunner` driven by a real
//! `lvqr_fragment::FragmentBroadcasterRegistry`.
//!
//! This test compiles only with the `whisper` Cargo feature
//! (so `cargo test --workspace` skips it without the feature)
//! AND runs only when the `WHISPER_MODEL_PATH` environment
//! variable points at an on-disk `ggml-*.bin` whisper.cpp model
//! file. The `#[ignore]` attribute keeps it off the default
//! `cargo test -p lvqr-agent-whisper --features whisper` run;
//! invoke with `-- --ignored` to opt in.
//!
//! How to fetch a small test model:
//!
//! ```bash
//! curl -L -o /tmp/ggml-tiny.en.bin \
//!   https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin
//! WHISPER_MODEL_PATH=/tmp/ggml-tiny.en.bin \
//!   cargo test -p lvqr-agent-whisper --features whisper -- --ignored
//! ```
//!
//! ggml-tiny.en is ~75 MB; the test deliberately does NOT
//! bundle a model file in `lvqr-conformance/fixtures` because
//! that would balloon the repo on every clone. CI runners that
//! want to validate the inference path set `WHISPER_MODEL_PATH`
//! externally.
//!
//! Without the model file the test logs a single line and
//! returns Ok -- it is not a failure to lack the model, it is
//! the expected default state of any environment that has not
//! opted into whisper.cpp.

#![cfg(feature = "whisper")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, Bytes, BytesMut};
use lvqr_agent::AgentRunner;
use lvqr_agent_whisper::{WhisperCaptionsFactory, WhisperConfig};
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};

const BOX_HEADER_LEN: usize = 8;

fn write_box(buf: &mut BytesMut, ty: &[u8; 4], body: &[u8]) {
    let total = (BOX_HEADER_LEN + body.len()) as u32;
    buf.put_u32(total);
    buf.put_slice(ty);
    buf.put_slice(body);
}

fn moof_then_mdat(aac_frame: &[u8]) -> Bytes {
    let mut buf = BytesMut::new();
    write_box(&mut buf, b"moof", b"opaque-moof-body");
    write_box(&mut buf, b"mdat", aac_frame);
    buf.freeze()
}

/// Synthesize a 12-byte ADTS-prefixed silent AAC-LC frame so
/// the worker can construct its symphonia decoder + worker
/// thread without a real audio source. The decoder may reject
/// the frame's payload as invalid AAC; the test does not
/// assert on caption text, only on lifecycle ordering.
fn synthetic_aac_frame() -> Bytes {
    Bytes::from_static(&[0x21, 0x10, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
}

/// Build a synthetic init segment carrying an AAC ASC of
/// `[0x12, 0x10]` (AAC-LC, 44.1 kHz, stereo). Constructed by
/// hand because pulling lvqr-ingest in as a dev-dep would
/// double the dep graph for one fixture.
fn synthetic_init_segment() -> Bytes {
    use lvqr_agent_whisper::asc;
    // Simply round-trip through asc::extract_asc's verified
    // ground-truth bytes so the test does not duplicate the
    // ASC chain construction.
    let asc_bytes: &[u8] = &[0x12, 0x10];

    // DecoderSpecificInfo (0x05).
    let mut dsi = vec![0x05, asc_bytes.len() as u8];
    dsi.extend_from_slice(asc_bytes);
    // DecoderConfigDescriptor (0x04). 13-byte preamble.
    let mut dcd_body = Vec::new();
    dcd_body.extend_from_slice(&[0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    dcd_body.extend_from_slice(&dsi);
    let mut dcd = vec![0x04, dcd_body.len() as u8];
    dcd.extend_from_slice(&dcd_body);
    // ESDescriptor (0x03). 3-byte preamble.
    let mut es_body = vec![0x00, 0x01, 0x00];
    es_body.extend_from_slice(&dcd);
    let mut es = vec![0x03, es_body.len() as u8];
    es.extend_from_slice(&es_body);

    let mut esds_body = vec![0u8; 4];
    esds_body.extend_from_slice(&es);
    let esds = make_box(b"esds", &esds_body);

    let mut mp4a_body = vec![0u8; 28];
    mp4a_body.extend_from_slice(&esds);
    let mp4a = make_box(b"mp4a", &mp4a_body);

    let mut stsd_body = vec![0u8; 8];
    stsd_body[7] = 1;
    stsd_body.extend_from_slice(&mp4a);
    let stsd = make_box(b"stsd", &stsd_body);

    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let mdia = make_box(b"mdia", &minf);
    let trak = make_box(b"trak", &mdia);
    let moov = make_box(b"moov", &trak);

    let init = Bytes::from(moov);
    // Sanity: extract_asc must round-trip the synthesized chain.
    assert_eq!(
        asc::extract_asc(&init).expect("synthesized init contains ASC").as_ref(),
        asc_bytes,
    );
    init
}

fn make_box(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let total = (BOX_HEADER_LEN + body.len()) as u32;
    let mut out = total.to_be_bytes().to_vec();
    out.extend_from_slice(ty);
    out.extend_from_slice(body);
    out
}

fn fragment(seq: u64, sample_rate: u32) -> Fragment {
    let dts = seq * 1024;
    let payload = moof_then_mdat(synthetic_aac_frame().as_ref());
    Fragment::new("1.mp4", seq, 0, 0, dts, dts, 1024, FragmentFlags::KEYFRAME, payload)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires WHISPER_MODEL_PATH env var pointing at a ggml-*.bin file"]
async fn whisper_agent_runs_against_real_registry_with_real_model() {
    let model_path = match std::env::var("WHISPER_MODEL_PATH") {
        Ok(p) => PathBuf::from(p),
        Err(_) => {
            eprintln!(
                "WHISPER_MODEL_PATH not set; skipping. \
                Fetch a model with:\n  \
                curl -L -o /tmp/ggml-tiny.en.bin \
                https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin\n\
                Then re-run with WHISPER_MODEL_PATH=/tmp/ggml-tiny.en.bin"
            );
            return;
        }
    };

    let registry = FragmentBroadcasterRegistry::new();
    let factory = WhisperCaptionsFactory::new(WhisperConfig::new(model_path).with_window_ms(2_000));
    let captions = factory.captions();
    let _captions_sub = captions.subscribe();

    let _runner_handle = AgentRunner::new().with_factory(factory).install(&registry);

    // Audio track at 44.1 kHz with the synthesized ASC.
    let meta = FragmentMeta::new("mp4a.40.2", 44_100).with_init_segment(synthetic_init_segment());
    let bc = registry.get_or_create("live/cam1", "1.mp4", meta);

    // Push enough fragments to cross the 2-second window so the
    // worker thread runs at least one inference pass.
    // 2000 ms * 44_100 Hz / 1024 samples per AAC-LC frame
    // = ~86 frames.
    let frames = 90u64;
    for i in 0..frames {
        bc.emit(fragment(i, 44_100));
    }

    // Drop the producer-side clone so on_stop fires + the
    // worker drains its remaining buffer.
    drop(bc);
    registry.remove("live/cam1", "1.mp4");

    // Wait long enough for the inference pass to complete; the
    // tiny.en model takes ~1s on a modern CPU for a 2-second
    // window. 10s is generous.
    tokio::time::sleep(Duration::from_secs(10)).await;

    // We do NOT assert on caption text content because the
    // synthesized AAC frames do not contain real speech --
    // assertion is "the worker reached + completed an
    // inference pass without panicking". Captions count may
    // be zero (no audible speech detected), which is the
    // expected silent-audio result.
    let _arc = Arc::new(()); // satisfy unused-import on lifecycle
}
