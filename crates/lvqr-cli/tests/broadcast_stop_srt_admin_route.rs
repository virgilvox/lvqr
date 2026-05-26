//! Slice 6 (SRT half): `DELETE /api/v1/broadcasts/{name}` truly disconnects a
//! live SRT publisher. Mirrors the RTMP integration test pattern -- publish
//! a real SRT session via `srt_tokio::SrtSocket`, wait for the entry to
//! surface in the broadcast registry, kill it, and assert the SRT socket
//! sees the disconnect on its next send/recv.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use lvqr_test_utils::http::{HttpGetOptions, http_delete, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const SYNC_BYTE: u8 = 0x47;

/// Build a minimal MPEG-TS packet for a single PID.
fn ts_packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
    let mut pkt = vec![0xFFu8; 188];
    pkt[0] = SYNC_BYTE;
    pkt[1] = if pusi { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F);
    pkt[2] = pid as u8;
    pkt[3] = 0x10;
    let copy = payload.len().min(184);
    pkt[4..4 + copy].copy_from_slice(&payload[..copy]);
    pkt
}

fn minimal_pat(pmt_pid: u16) -> Vec<u8> {
    let mut data = vec![0x00, 0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01];
    data.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
    data.push(pmt_pid as u8);
    data.extend_from_slice(&[0x00; 4]);
    data
}

async fn broadcast_present(admin: SocketAddr, name: &str) -> bool {
    let resp = http_get_with(admin, "/api/v1/broadcasts", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    body["sessions"]
        .as_array()
        .map(|arr| arr.iter().any(|s| s["broadcast"].as_str() == Some(name)))
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broadcast_stop_kicks_a_live_srt_publisher() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_srt())
        .await
        .expect("start TestServer with SRT");
    let admin_addr = server.admin_addr();
    let srt_addr = server.srt_addr();

    // Connect as an SRT caller. Without a streamid the server defaults the
    // broadcast name to "srt/default" (see `lvqr_srt::ingest`'s extract
    // fallback). That's the name we kill below.
    let mut srt: srt_tokio::SrtSocket = srt_tokio::SrtSocket::builder()
        .call(srt_addr, None)
        .await
        .expect("SRT connect");

    // Push one PAT so the demuxer wakes up (the registrar fires at accept
    // time regardless, but a real publisher always sends something).
    let pmt_pid = 0x1000u16;
    let mut ts_data = Vec::new();
    ts_data.extend_from_slice(&ts_packet(0, true, &minimal_pat(pmt_pid)));
    srt.send((Instant::now(), Bytes::from(ts_data))).await.unwrap();

    // Poll until "srt/default" surfaces. The SRT registrar fires before the
    // handler task is spawned, so this should land in the first tick.
    let mut seen = false;
    for _ in 0..40 {
        if broadcast_present(admin_addr, "srt/default").await {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seen, "broadcast srt/default must surface in /api/v1/broadcasts");

    let resp = http_get_with(admin_addr, "/api/v1/broadcasts", HttpGetOptions::default()).await;
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let row = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["broadcast"].as_str() == Some("srt/default"))
        .unwrap();
    assert_eq!(row["protocol"], "srt");
    assert!(row["peer"].as_str().is_some(), "peer addr must be captured for SRT");

    // KILL the live publisher.
    let resp = http_delete(admin_addr, "/api/v1/broadcasts/srt%2Fdefault", None).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["result"], "killed");
    assert_eq!(body["protocol"], "srt");

    // Drain the SRT socket: after the server cancels the per-connection
    // token, the handler task exits and the SRT socket closes. The client's
    // `next()` either returns `None` (clean close) or errors -- either is a
    // disconnect from our perspective. We give the SRT teardown ~2s as it
    // can take several poll ticks.
    let mut closed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), srt.next()).await {
            Ok(None) | Ok(Some(Err(_))) => {
                closed = true;
                break;
            }
            Ok(Some(Ok(_))) | Err(_) => continue,
        }
    }
    assert!(closed, "SRT socket must close after broadcast stop");

    // Inventory now omits the entry.
    let mut gone = false;
    for _ in 0..40 {
        if !broadcast_present(admin_addr, "srt/default").await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "broadcast srt/default must be gone from inventory after stop");

    server.shutdown().await.expect("shutdown");
}
