//! End-to-end test for the `--whisper-model` CLI wiring (Tier 4
//! item 4.5 session D).
//!
//! Requires the `whisper` Cargo feature. Actual whisper.cpp
//! inference is gated on the `WHISPER_MODEL_PATH` env var
//! pointing at a real `ggml-*.bin` file. The test is wired
//! under `#[ignore]` so the default `cargo test --workspace`
//! run skips it; explicit invocations go through
//!
//! ```bash
//! WHISPER_MODEL_PATH=/tmp/ggml-tiny.en.bin \
//!   cargo test -p lvqr-cli --features whisper \
//!     --test whisper_cli_e2e -- --ignored
//! ```
//!
//! The test verifies the wiring itself, not whisper.cpp's
//! transcription quality: it drives a real RTMP publish with
//! synthetic AAC audio frames, waits for the AgentRunner's
//! per-broadcast drain task to observe them, and asserts the
//! `fragments_seen` counter on the returned
//! `AgentRunnerHandle` is non-zero for
//! `(captions, broadcast, 1.mp4)`. That proves the
//! WhisperCaptionsFactory was installed against the shared
//! registry and its WhisperCaptionsAgent is actually receiving
//! the audio fragments through the whisper worker channel.
//!
//! A future session could extend this with a real English
//! audio fixture + assert that the captions playlist carries
//! at least one non-empty cue; for session-100 D the wiring
//! assertion is sufficient because the captions surface itself
//! is already verified by `captions_hls_e2e.rs` (which uses
//! synthetic `captions`-track fragments on the registry and
//! does not need whisper.cpp).

#![cfg(feature = "whisper")]

use lvqr_test_utils::flv::{
    flv_audio_aac_lc_seq_header_44k_stereo, flv_audio_raw, flv_video_nalu, flv_video_seq_header,
};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, ClientSessionEvent, PublishRequestType};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

async fn connect_and_publish(addr: SocketAddr, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .unwrap()
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial_results) = ClientSession::new(config).unwrap();
    send_results(&mut stream, &initial_results).await;
    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect_result = session.request_connection(app.to_string()).unwrap();
    send_result(&mut stream, &connect_result).await;
    read_until(&mut stream, &mut session, TIMEOUT, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish_result).await;
    read_until(&mut stream, &mut session, TIMEOUT, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

/// Publish one video init + audio init + a handful of audio
/// frames. Each audio frame is synthetic 64-byte silence; the
/// whisper agent's AAC decoder will produce (mostly) empty PCM
/// but the wire path from RTMP through the
/// `FragmentBroadcasterRegistry` and into the AgentRunner-spawned
/// drain task is the thing under test here, not the transcription
/// quality.
async fn publish_audio_burst(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    let vseq = flv_video_seq_header();
    let r = session.publish_video_data(vseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let aseq = flv_audio_aac_lc_seq_header_44k_stereo();
    let r = session.publish_audio_data(aseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let r = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    // Push four AAC frames spaced ~25 ms apart. The agent's
    // fragments_seen counter only needs one to tick over; the
    // extra frames give the worker channel a chance to back up
    // if anything on the wire is broken.
    for i in 0..4u32 {
        let aac = flv_audio_raw(&[0u8; 64]);
        let ts = i * 25;
        let r = session.publish_audio_data(aac, RtmpTimestamp::new(ts), false).unwrap();
        send_result(&mut rtmp_stream, &r).await;
    }

    let kf1 = flv_video_nalu(true, 0, &nalu);
    let r = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &r).await;

    (rtmp_stream, session)
}

/// Full CLI wiring check: `ServeConfig.whisper_model = Some(path)`
/// causes `start()` to install a `WhisperCaptionsFactory` on the
/// shared registry, and a real RTMP audio publish on `1.mp4`
/// is observed by the resulting `WhisperCaptionsAgent`.
///
/// Requires `WHISPER_MODEL_PATH` to point at a real `ggml-*.bin`.
/// Without the env var the test logs a single line and exits
/// with success -- the absent model is the expected default
/// state, not a failure.
#[ignore = "requires WHISPER_MODEL_PATH + the whisper feature; run via `-- --ignored`"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whisper_cli_flag_wires_factory_through_start() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let Some(model_path) = std::env::var_os("WHISPER_MODEL_PATH") else {
        eprintln!("WHISPER_MODEL_PATH unset; skipping. Set it to a ggml-*.bin to exercise the full path.");
        return;
    };

    let server = TestServer::start(TestServerConfig::default().with_whisper_model(model_path))
        .await
        .expect("start TestServer with --whisper-model");
    let rtmp_addr = server.rtmp_addr();

    let (_s, _sess) = publish_audio_burst(rtmp_addr, "live", "captions").await;

    // The agent's on_fragment fires on every arriving audio
    // fragment; the worker channel is sync_channel(64), so the
    // counter bumps synchronously inside the drain task. 5 s is
    // enough to cover the RTMP -> ingest -> registry callback ->
    // drain-task spawn path on a cold cache.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut observed = 0u64;
    while std::time::Instant::now() < deadline {
        observed = server
            .agent_runner()
            .expect("agent_runner must be present when --whisper-model is set")
            .fragments_seen("captions", "live/captions", "1.mp4");
        if observed > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(
        observed > 0,
        "WhisperCaptionsAgent did not observe any audio fragments (fragments_seen=0)"
    );

    server.shutdown().await.expect("shutdown");
}
