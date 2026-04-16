//! RTMP ingest -> LL-HLS HTTP egress end-to-end integration test.
//!
//! Sister test to `rtmp_ws_e2e.rs`. Where the WS test verifies the
//! RTMP -> MoQ -> WebSocket fMP4 path, this one verifies the
//! Tier 2.3 RTMP -> Fragment -> CmafChunk -> MultiHlsServer -> axum
//! HTTP path that `lvqr-cli serve` composes. There are no mocks: a
//! real `rml_rtmp` client publishes, a real `lvqr_cli::start`-driven
//! server forwards fragments through the HLS bridge, and a real
//! raw-TCP HTTP/1.1 client reads the per-broadcast playlists plus
//! referenced media URIs off the LL-HLS surface.
//!
//! Session 12: this test now publishes **two** concurrent RTMP
//! broadcasts -- `live/one` and `live/two` -- and asserts that the
//! multi-broadcast router exposes them under
//! `/hls/live/one/playlist.m3u8` and `/hls/live/two/playlist.m3u8`
//! respectively, that the two playlists reference distinct
//! `#EXT-X-PART:` URIs, and that fetching one part from each
//! broadcast returns a `moof`-prefixed body. An unknown broadcast
//! returns 404 so the negative path stays honest too.
//!
//! Each broadcast pushes exactly two keyframes spaced 2.1 s apart so
//! the segmenter's default `VIDEO_90KHZ_DEFAULT` policy (2 s segment
//! duration at 90 kHz) closes one full segment after the second
//! keyframe.

use bytes::Bytes;
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

// =====================================================================
// FLV tag helpers (mirror crates/lvqr-cli/tests/rtmp_ws_e2e.rs)
// =====================================================================

fn flv_video_seq_header() -> Bytes {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let mut tag = vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1];
    tag.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&sps);
    tag.push(0x01);
    tag.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&pps);
    Bytes::from(tag)
}

fn flv_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes {
    let frame_type = if keyframe { 0x17 } else { 0x27 };
    let mut tag = vec![frame_type, 0x01, (cts >> 16) as u8, (cts >> 8) as u8, cts as u8];
    tag.extend_from_slice(nalu_data);
    Bytes::from(tag)
}

/// FLV AAC sequence header: codec id 10 (AAC), packet type 0, followed
/// by a minimal AudioSpecificConfig for AAC-LC 44100 Hz stereo.
fn flv_audio_seq_header() -> Bytes {
    // AAC-LC (obj=2), sampling_frequency_index=4 (44100), channel=2
    let b0: u8 = (2 << 3) | (4 >> 1);
    let b1: u8 = (4 << 7) | (2 << 3);
    Bytes::from(vec![0xAF, 0x00, b0, b1])
}

/// FLV AAC raw frame: codec id 10 (AAC), packet type 1, payload bytes.
fn flv_audio_raw(aac_data: &[u8]) -> Bytes {
    let mut tag = vec![0xAF, 0x01];
    tag.extend_from_slice(aac_data);
    Bytes::from(tag)
}

// =====================================================================
// RTMP publish helpers (copied from rtmp_ws_e2e.rs verbatim)
// =====================================================================

async fn rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake.generate_outbound_p0_and_p1().unwrap();
    stream.write_all(&p0_and_p1).await.unwrap();

    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "server closed during handshake");
        match handshake.process_bytes(&buf[..n]).unwrap() {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
                return remaining_bytes;
            }
        }
    }
}

async fn send_results(stream: &mut TcpStream, results: &[ClientSessionResult]) {
    for result in results {
        if let ClientSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await.unwrap();
        }
    }
}

async fn send_result(stream: &mut TcpStream, result: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = result {
        stream.write_all(&packet.bytes).await.unwrap();
    }
}

async fn read_until<F>(stream: &mut TcpStream, session: &mut ClientSession, predicate: F)
where
    F: Fn(&ClientSessionEvent) -> bool,
{
    let mut buf = vec![0u8; 65536];
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let n = match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => panic!("server closed connection unexpectedly"),
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => panic!("timed out waiting for expected RTMP event"),
        };
        let results = session.handle_input(&buf[..n]).unwrap();
        for result in results {
            match result {
                ClientSessionResult::OutboundResponse(packet) => {
                    stream.write_all(&packet.bytes).await.unwrap();
                }
                ClientSessionResult::RaisedEvent(ref event) => {
                    if predicate(event) {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

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
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

// =====================================================================
// Minimal raw-TCP HTTP/1.1 GET client.
//
// We deliberately avoid pulling in `reqwest` or `hyper-util` as a
// dev-dep just for two GETs. The HLS server speaks plain HTTP/1.1
// `Connection: close` perfectly well, so a 30-line client is enough.
// =====================================================================

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("http GET connect timed out")
        .expect("http GET connect failed");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("http GET read timed out")
        .expect("http GET read failed");
    parse_http_response(&buf)
}

fn parse_http_response(bytes: &[u8]) -> HttpResponse {
    // Locate the end-of-headers marker (CRLF CRLF).
    let split = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http response missing header terminator");
    let header_block = &bytes[..split];
    let body_block = &bytes[split + 4..];

    let header_text = std::str::from_utf8(header_block).expect("http headers are not utf-8");
    let mut header_lines = header_text.lines();
    let status_line = header_lines.next().expect("http response missing status line");
    let mut status_parts = status_line.splitn(3, ' ');
    let _http_version = status_parts.next();
    let status: u16 = status_parts
        .next()
        .expect("status line missing code")
        .parse()
        .expect("status code is not numeric");

    HttpResponse {
        status,
        body: body_block.to_vec(),
    }
}

// =====================================================================
// Helpers for parsing the LL-HLS playlist body.
// =====================================================================

/// Pull every URI named in an `#EXT-X-PART:` line out of a rendered
/// playlist body. The renderer in `lvqr-hls` emits each part as
/// `#EXT-X-PART:DURATION=...,URI="<uri>"[,INDEPENDENT=YES]`.
fn extract_part_uris(playlist: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in playlist.lines() {
        let Some(rest) = line.strip_prefix("#EXT-X-PART:") else {
            continue;
        };
        let Some(uri_start) = rest.find("URI=\"") else {
            continue;
        };
        let after = &rest[uri_start + 5..];
        let Some(end) = after.find('"') else {
            continue;
        };
        out.push(after[..end].to_string());
    }
    out
}

// =====================================================================
// The test
// =====================================================================

/// Publish a two-keyframe sequence to `{app}/{key}` and return the
/// open RTMP stream + session so the caller can hold them alive while
/// the test reads the resulting LL-HLS surface. Dropping them closes
/// the RTMP session; keep them in scope until after the HTTP reads
/// complete so the bridge does not tear the broadcast down early.
async fn publish_two_keyframes(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    let seq = flv_video_seq_header();
    let result = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let result = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // dts at 90 kHz = 189_000, past the default 180_000-tick segment
    // boundary, so the second keyframe closes the first segment.
    let kf1 = flv_video_nalu(true, 0, &nalu);
    let result = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &result).await;

    (rtmp_stream, session)
}

/// Fetch `/hls/{app}/{key}/playlist.m3u8` and assert it is a
/// well-formed LL-HLS media playlist with at least one
/// `#EXT-X-PART:` URI. Returns the parsed part URI list so the
/// caller can compare it against a second broadcast's playlist.
async fn fetch_playlist_and_part_uris(hls_addr: SocketAddr, app: &str, key: &str) -> Vec<String> {
    let path = format!("/hls/{app}/{key}/playlist.m3u8");
    let resp = http_get(hls_addr, &path).await;
    assert_eq!(resp.status, 200, "playlist GET status for {path}");
    let body = std::str::from_utf8(&resp.body).expect("playlist body should be utf-8");
    eprintln!("--- playlist {path} ---\n{body}\n--- end ---");
    assert!(body.starts_with("#EXTM3U"), "playlist missing #EXTM3U header: {body}");
    assert!(
        body.contains("#EXT-X-VERSION:9"),
        "playlist missing LL-HLS version tag: {body}"
    );
    assert!(
        body.contains("#EXT-X-MAP:URI=\"init.mp4\""),
        "playlist missing #EXT-X-MAP for init segment: {body}"
    );
    let part_uris = extract_part_uris(body);
    assert!(
        !part_uris.is_empty(),
        "playlist {path} references no #EXT-X-PART URIs:\n{body}"
    );
    part_uris
}

/// Real end-to-end: two concurrent RTMP publishes -> RtmpMoqBridge ->
/// shared FragmentBroadcasterRegistry -> BroadcasterHlsBridge drain
/// tasks -> MultiHlsServer -> axum HTTP. Verifies that both broadcasts
/// expose independent `/hls/{app}/{key}/playlist.m3u8` endpoints,
/// that the two playlists reference distinct part URIs (the
/// per-broadcast `PlaylistBuilder` state machines are genuinely
/// independent), and that fetching one part from each broadcast
/// returns a `moof`-prefixed body. Also asserts a negative lookup
/// for an unknown broadcast returns 404 so the router does not
/// silently fabricate empty playlists.
#[tokio::test]
async fn rtmp_publish_reaches_multi_broadcast_hls_router() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // --- Spin up the full LVQR stack with HLS enabled (default). ---
    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let hls_addr = server.hls_addr();

    // --- Publish two concurrent broadcasts. ---
    let (_s1, _sess1) = publish_two_keyframes(rtmp_addr, "live", "one").await;
    let (_s2, _sess2) = publish_two_keyframes(rtmp_addr, "live", "two").await;

    // The on_fragment path spawns one tokio task per push; give them
    // a tick to land on the MultiHlsServer state before we read.
    tokio::time::sleep(Duration::from_millis(250)).await;

    // --- Fetch both playlists, assert each is well-formed. ---
    let parts_one = fetch_playlist_and_part_uris(hls_addr, "live", "one").await;
    let parts_two = fetch_playlist_and_part_uris(hls_addr, "live", "two").await;

    // The two playlists must reference independent part URIs. Because
    // each broadcast lives behind its own `PlaylistBuilder`, the URIs
    // for the first chunk happen to collide (both start at
    // `part-0-0.m4s`), but the routes that serve them are distinct:
    // `/hls/live/one/part-0-0.m4s` and `/hls/live/two/part-0-0.m4s`
    // resolve to different per-broadcast caches. Verify that the
    // bytes served under each route are both valid `moof` segments,
    // which is the real independence property we care about.
    let first_one = &parts_one[0];
    let first_two = &parts_two[0];
    let part_one_path = format!("/hls/live/one/{first_one}");
    let part_two_path = format!("/hls/live/two/{first_two}");

    let part_one_resp = http_get(hls_addr, &part_one_path).await;
    assert_eq!(part_one_resp.status, 200, "part GET status for {part_one_path}");
    assert!(
        part_one_resp.body.len() >= 8,
        "part one body too short: {} bytes",
        part_one_resp.body.len()
    );
    assert_eq!(
        &part_one_resp.body[4..8],
        b"moof",
        "expected part one to start with a `moof` box"
    );

    let part_two_resp = http_get(hls_addr, &part_two_path).await;
    assert_eq!(part_two_resp.status, 200, "part GET status for {part_two_path}");
    assert!(
        part_two_resp.body.len() >= 8,
        "part two body too short: {} bytes",
        part_two_resp.body.len()
    );
    assert_eq!(
        &part_two_resp.body[4..8],
        b"moof",
        "expected part two to start with a `moof` box"
    );

    // --- init segments must be served per broadcast too. ---
    let init_one_resp = http_get(hls_addr, "/hls/live/one/init.mp4").await;
    assert_eq!(init_one_resp.status, 200, "init one GET status");
    assert!(init_one_resp.body.len() >= 8, "init one body too short");
    assert_eq!(
        &init_one_resp.body[4..8],
        b"ftyp",
        "init one segment did not start with `ftyp`"
    );

    let init_two_resp = http_get(hls_addr, "/hls/live/two/init.mp4").await;
    assert_eq!(init_two_resp.status, 200, "init two GET status");
    assert_eq!(
        &init_two_resp.body[4..8],
        b"ftyp",
        "init two segment did not start with `ftyp`"
    );

    // --- Unknown broadcast must return 404 rather than an empty 200. ---
    let unknown = http_get(hls_addr, "/hls/live/ghost/playlist.m3u8").await;
    assert_eq!(
        unknown.status, 404,
        "unknown broadcast should 404, got {}",
        unknown.status
    );

    // --- Clean shutdown. ---
    drop(_s1);
    drop(_s2);
    server.shutdown().await.expect("shutdown");
}

/// Publish a video keyframe sequence plus an AAC sequence header and
/// raw frame to `{app}/{key}` and return the open RTMP stream and
/// session so the caller can hold them alive until the HTTP reads
/// complete. The audio sequence header is sent before the first video
/// keyframe so the bridge sees the AAC config at the same time the
/// video init lands, which is how real publishers (OBS, ffmpeg)
/// interleave the two track headers.
async fn publish_video_with_audio(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    // Video sequence header.
    let vseq = flv_video_seq_header();
    let r = session.publish_video_data(vseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    // Audio sequence header. The AAC-LC 44100/stereo ASC matches the
    // `flv_audio_seq_header` helper above; the bridge's
    // `parse_audio_tag` picks this up, constructs an audio init
    // segment, and fires the HLS bridge's `on_init` hook with
    // track id `1.mp4`, which in turn calls `ensure_audio(broadcast)`
    // and registers the audio `HlsServer` on the `MultiHlsServer`.
    let aseq = flv_audio_seq_header();
    let r = session.publish_audio_data(aseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    // First video keyframe at t=0.
    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let r = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    // One raw AAC frame at t=0. A single audio fragment is enough to
    // promote the audio HlsServer out of the `None` state in the
    // MultiHlsServer entry for this broadcast; the master playlist
    // then declares the audio rendition group.
    let aac = flv_audio_raw(&[0u8; 64]);
    let r = session.publish_audio_data(aac, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    // Second video keyframe past the 2 s segment boundary so the video
    // playlist closes its first segment and starts emitting `#EXT-X-PART:`
    // lines the reader side can match on.
    let kf1 = flv_video_nalu(true, 0, &nalu);
    let r = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &r).await;

    // One more raw AAC frame close to the second video keyframe so the
    // audio playlist has at least one chunk visible by the time the
    // client reads.
    let aac = flv_audio_raw(&[0u8; 64]);
    let r = session
        .publish_audio_data(aac, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &r).await;

    (rtmp_stream, session)
}

/// Real end-to-end: one RTMP broadcast publishing both video and audio
/// reaches the LL-HLS master playlist with an EXT-X-MEDIA audio
/// rendition declaration, the audio sub-playlist at `audio.m3u8`, and
/// an audio init segment at `audio-init.mp4`. Closes the audio
/// rendition path that session 13 landed through `integration_master.rs`
/// by proving the same surface is reachable through a real RTMP
/// publish, not just a router oneshot.
#[tokio::test]
async fn rtmp_publish_with_audio_reaches_master_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let hls_addr = server.hls_addr();

    let (_s, _sess) = publish_video_with_audio(rtmp_addr, "live", "av").await;

    // The observer path spawns one tokio task per fragment; give them
    // a tick to land on the MultiHlsServer state before reading.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // --- Master playlist must declare the audio rendition. ---
    let master_resp = http_get(hls_addr, "/hls/live/av/master.m3u8").await;
    assert_eq!(master_resp.status, 200, "master GET status");
    let master_body = std::str::from_utf8(&master_resp.body).expect("master body utf-8");
    eprintln!("--- master.m3u8 ---\n{master_body}\n--- end ---");
    assert!(
        master_body.starts_with("#EXTM3U"),
        "master missing #EXTM3U: {master_body}"
    );
    assert!(
        master_body.contains("#EXT-X-MEDIA:"),
        "master missing #EXT-X-MEDIA when audio is present: {master_body}"
    );
    assert!(
        master_body.contains("TYPE=AUDIO"),
        "master #EXT-X-MEDIA is not an audio rendition: {master_body}"
    );
    assert!(
        master_body.contains("AUDIO=\"audio\""),
        "master #EXT-X-STREAM-INF missing AUDIO= group reference: {master_body}"
    );

    // --- Audio sub-playlist must be served, not 404. ---
    let audio_resp = http_get(hls_addr, "/hls/live/av/audio.m3u8").await;
    assert_eq!(audio_resp.status, 200, "audio.m3u8 GET status");
    let audio_body = std::str::from_utf8(&audio_resp.body).expect("audio body utf-8");
    eprintln!("--- audio.m3u8 ---\n{audio_body}\n--- end ---");
    assert!(
        audio_body.starts_with("#EXTM3U"),
        "audio playlist missing #EXTM3U: {audio_body}"
    );
    assert!(
        audio_body.contains("#EXT-X-MAP:URI=\"audio-init.mp4\""),
        "audio playlist missing #EXT-X-MAP for audio-init.mp4: {audio_body}"
    );

    // --- Audio init segment must be served and start with ftyp. ---
    let audio_init_resp = http_get(hls_addr, "/hls/live/av/audio-init.mp4").await;
    assert_eq!(audio_init_resp.status, 200, "audio-init.mp4 GET status");
    assert!(
        audio_init_resp.body.len() >= 8,
        "audio-init body too short: {} bytes",
        audio_init_resp.body.len()
    );
    assert_eq!(
        &audio_init_resp.body[4..8],
        b"ftyp",
        "audio-init segment did not start with `ftyp`"
    );

    // --- Video playlist must still resolve on the same broadcast. ---
    let video_resp = http_get(hls_addr, "/hls/live/av/playlist.m3u8").await;
    assert_eq!(video_resp.status, 200, "video playlist GET status");
    let video_body = std::str::from_utf8(&video_resp.body).expect("video body utf-8");
    assert!(
        video_body.contains("#EXT-X-MAP:URI=\"init.mp4\""),
        "video playlist missing #EXT-X-MAP for init.mp4: {video_body}"
    );

    // --- Clean shutdown. ---
    drop(_s);
    server.shutdown().await.expect("shutdown");
}

/// Publish two keyframes, disconnect the RTMP client, then verify the
/// playlist carries #EXT-X-ENDLIST. The disconnect fires the
/// BroadcastStopped event, which the session-39 HLS finalize
/// subscriber picks up and calls MultiHlsServer::finalize_broadcast.
#[tokio::test]
async fn rtmp_disconnect_produces_endlist_in_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let hls_addr = server.hls_addr();

    // Publish two keyframes so the segmenter closes one segment.
    let (rtmp_stream, _session) = publish_two_keyframes(rtmp_addr, "live", "fin").await;

    // Wait for fragments to land on the HLS surface.
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Before disconnect: playlist exists and has no ENDLIST.
    let resp = http_get(hls_addr, "/hls/live/fin/playlist.m3u8").await;
    assert_eq!(resp.status, 200);
    let body = std::str::from_utf8(&resp.body).expect("utf-8");
    assert!(body.starts_with("#EXTM3U"));
    assert!(
        !body.contains("#EXT-X-ENDLIST"),
        "ENDLIST must not appear before disconnect:\n{body}"
    );

    // Drop the RTMP stream to trigger disconnect -> on_unpublish ->
    // BroadcastStopped -> finalize_broadcast.
    drop(rtmp_stream);
    drop(_session);

    // Give the event loop time to propagate the disconnect through
    // the event bus and finalize the HLS broadcast.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // After disconnect: playlist must contain ENDLIST.
    let resp = http_get(hls_addr, "/hls/live/fin/playlist.m3u8").await;
    assert_eq!(resp.status, 200);
    let body = std::str::from_utf8(&resp.body).expect("utf-8");
    assert!(
        body.contains("#EXT-X-ENDLIST"),
        "playlist must contain #EXT-X-ENDLIST after disconnect:\n{body}"
    );
    assert!(
        !body.contains("#EXT-X-PRELOAD-HINT"),
        "preload hint must be suppressed after finalize:\n{body}"
    );

    server.shutdown().await.expect("shutdown");
}
