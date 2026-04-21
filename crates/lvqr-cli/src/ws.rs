//! WebSocket relay + ingest handlers + recorder event bridge.
//!
//! Extracted out of `lib.rs` in the session-111-B1 follow-up refactor
//! so the composition root stays focused on wiring. Three public-to-
//! the-crate surfaces:
//!
//! - [`WsRelayState`] is the shared handler state threaded through
//!   both `/ws/*` and `/ingest/*` axum routes.
//! - [`ws_relay_handler`] / [`ws_ingest_handler`] are the
//!   authenticated upgrade points. Both reuse
//!   [`resolve_ws_token`] to honor the
//!   `Sec-WebSocket-Protocol: lvqr.bearer.<token>` header and the
//!   legacy `?token=<token>` query parameter.
//! - [`spawn_recordings`] drains the shared [`lvqr_core::EventBus`]
//!   and starts one `lvqr_record::BroadcastRecorder` session per
//!   broadcast. Lives here because WS-ingested broadcasts must be
//!   recorded identically to RTMP-ingested ones, and the event-bus
//!   glue is the fanout point that makes that work.
//!
//! Internal helpers (`ws_relay_session`, `ws_ingest_session`,
//! `relay_track`, `parse_avcc_record`, `WsTokenResolution`,
//! `resolve_ws_token`) stay private to this module.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use lvqr_auth::{AuthContext, AuthDecision, SharedAuth, extract};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_moq::Track;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

/// Monotonic counter backing the per-session mesh peer_id that
/// `ws_relay_session` generates when the mesh coordinator is
/// wired. Format: `ws-{counter}`. Safe across all active
/// sessions because the counter is process-global and
/// `AtomicU64` increments are race-free. Session 111-B2.
static MESH_PEER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Shared state for WebSocket relay and ingest handlers.
#[derive(Clone)]
pub(crate) struct WsRelayState {
    pub(crate) origin: lvqr_moq::OriginProducer,
    /// Stored init segments per broadcast, so viewers get them immediately
    /// on connect.
    pub(crate) init_segments: Arc<dashmap::DashMap<String, Bytes>>,
    /// Authentication provider applied to WS subscribe and ingest sessions.
    pub(crate) auth: SharedAuth,
    /// Lifecycle event bus.
    pub(crate) events: EventBus,
    /// Shared `FragmentBroadcasterRegistry` handle used by the WS relay
    /// session to open an auxiliary `BroadcasterStream` subscription per
    /// `(broadcast, track)` for Tier 4 item 4.7 SLO sampling under the
    /// `transport="ws"` label. The MoQ-side drain that feeds the wire is
    /// unchanged; the aux stream exists purely to read
    /// `Fragment::ingest_time_ms` without touching the MoQ wire.
    pub(crate) registry: lvqr_fragment::FragmentBroadcasterRegistry,
    /// Optional shared latency tracker. `None` disables WS SLO sampling
    /// for compat with tests that boot a bare `WsRelayState`.
    pub(crate) slo: Option<lvqr_admin::LatencyTracker>,
    /// Optional mesh coordinator. `Some` when
    /// [`ServeConfig::mesh_enabled`] was `true` at `start()`,
    /// `None` otherwise. When present, every `ws_relay_session`
    /// generates a server-side peer_id via `MESH_PEER_COUNTER`,
    /// calls `add_peer` at connect time, sends the resulting
    /// [`MeshAssignment`] as a leading text frame on the WS so
    /// the client knows its tree position, and calls
    /// `remove_peer` when the session ends. Session 111-B2.
    pub(crate) mesh: Option<Arc<lvqr_mesh::MeshCoordinator>>,
}

/// Leading-frame shape sent by `ws_relay_session` to every new
/// subscriber when the mesh coordinator is wired. Encodes the
/// server-generated `peer_id`, the peer's tree `role` (Root or
/// Relay), the `parent_id` assigned (`null` for root peers), and
/// the `depth` from the server. Clients that want to participate
/// in the WebRTC DataChannel data plane use the `peer_id` to
/// open `/signal` with the same identity and negotiate SDP with
/// their assigned parent. Clients that do not care about mesh
/// can ignore the leading text frame and continue reading
/// binary MoQ frames as usual. Session 111-B2.
#[derive(serde::Serialize)]
pub(crate) struct MeshAssignment {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) peer_id: String,
    pub(crate) role: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) depth: u32,
}

pub(crate) async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsRelayState>,
    Path(broadcast): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    tracing::info!(broadcast = %broadcast, "WebSocket relay request");
    let resolved = resolve_ws_token(&headers, &params, "ws_subscribe");
    let decision = state.auth.check(&AuthContext::Subscribe {
        token: resolved.token,
        broadcast: broadcast.clone(),
    });
    if let AuthDecision::Deny { reason } = decision {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WS relay denied");
        metrics::counter!("lvqr_auth_failures_total", "entry" => "ws").increment(1);
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    metrics::counter!("lvqr_ws_connections_total", "direction" => "subscribe").increment(1);
    let ws = match resolved.offered_subprotocol {
        Some(ref p) => ws.protocols(std::iter::once(p.clone())),
        None => ws,
    };
    ws.on_upgrade(move |socket| ws_relay_session(socket, state, broadcast))
        .into_response()
}

async fn ws_relay_session(mut socket: WebSocket, state: WsRelayState, broadcast: String) {
    let consumer = state.origin.consume();
    let Some(bc) = consumer.consume_broadcast(&broadcast) else {
        tracing::warn!(broadcast = %broadcast, "broadcast not found for WS relay");
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4404,
                reason: "broadcast not found".into(),
            })))
            .await;
        return;
    };

    tracing::info!(broadcast = %broadcast, "WS relay session started");

    // Session 111-B2: register this subscriber with the mesh
    // coordinator (when mesh is enabled) and send the resulting
    // assignment as a leading text frame so the client can open
    // `/signal` later with the same server-generated peer_id.
    // `mesh_peer_id` is `Some` only when the registration
    // succeeded; on failure we log and proceed without mesh
    // state so the subscriber still gets served MoQ frames.
    let mesh_peer_id = if let Some(mesh) = state.mesh.as_ref() {
        let peer_id = format!("ws-{}", MESH_PEER_COUNTER.fetch_add(1, Ordering::Relaxed));
        match mesh.add_peer(peer_id.clone(), broadcast.clone()) {
            Ok(assignment) => {
                let payload = MeshAssignment {
                    kind: "peer_assignment",
                    peer_id: peer_id.clone(),
                    role: format!("{:?}", assignment.role),
                    parent_id: assignment.parent.clone(),
                    depth: assignment.depth,
                };
                match serde_json::to_string(&payload) {
                    Ok(json) => {
                        if let Err(e) = socket.send(Message::Text(json.into())).await {
                            tracing::debug!(error = ?e, "WS send of mesh assignment failed");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "serializing MeshAssignment failed");
                    }
                }
                Some(peer_id)
            }
            Err(e) => {
                tracing::warn!(error = ?e, peer = %peer_id, "mesh add_peer failed for WS subscriber");
                None
            }
        }
    } else {
        None
    };

    let video_track = bc.subscribe_track(&Track::new("0.mp4")).ok();
    let audio_track = bc.subscribe_track(&Track::new("1.mp4")).ok();

    if video_track.is_none() && audio_track.is_none() {
        tracing::warn!(broadcast = %broadcast, "no playable tracks for WS relay");
        return;
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u8, Bytes)>(64);
    let cancel = CancellationToken::new();

    if let Some(track) = video_track {
        let tx = tx.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            relay_track(track, 0u8, tx, cancel).await;
        });
    }
    if let Some(track) = audio_track {
        let tx = tx.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            relay_track(track, 1u8, tx, cancel).await;
        });
    }
    drop(tx);

    // Tier 4 item 4.7 session 110 A: auxiliary fragment-registry drain
    // per (broadcast, track) so the WS relay records one SLO sample
    // per fragment under `transport="ws"`. The MoQ-side drain above
    // still feeds the wire byte-identically; this aux drain never
    // touches the socket. Decision baked in-commit: stamp the sample
    // at the point where the fragment becomes available on the
    // fanout (essentially sub-ms before the WS socket.send) to keep
    // the MoQ wire pure (no payload-prefix ingest-time propagation)
    // and avoid a correlation channel with the MoQ-side drain's
    // phantom init frames (which are written once per group by
    // `MoqTrackSink::push` but have no matching Fragment).
    if let Some(tracker) = state.slo.clone() {
        for track_id in ["0.mp4", "1.mp4"] {
            if let Some(mut sub) = state.registry.get(&broadcast, track_id).map(|bc| bc.subscribe()) {
                let tracker = tracker.clone();
                let broadcast = broadcast.clone();
                let cancel_aux = cancel.clone();
                tokio::spawn(async move {
                    use lvqr_fragment::FragmentStream;
                    loop {
                        let frag = tokio::select! {
                            res = sub.next_fragment() => res,
                            _ = cancel_aux.cancelled() => return,
                        };
                        let Some(frag) = frag else { return };
                        if frag.ingest_time_ms > 0 {
                            let now_ms = lvqr_core::now_unix_ms();
                            let latency = now_ms.saturating_sub(frag.ingest_time_ms);
                            tracker.record(&broadcast, "ws", latency);
                        }
                    }
                });
            }
        }
    }

    // Session 111-B2: watch both the MoQ-side rx and the
    // client-side socket so idle subscribers (no frames flowing)
    // still exit promptly when the client closes the WS.
    // Pre-111-B2 the loop only polled rx.recv(), which meant a
    // subscriber that never saw a frame never got cleaned up;
    // that pinned the mesh peer entry forever when mesh was
    // enabled, and held the relay-side state otherwise.
    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Some((track_id, payload)) => {
                        let mut framed = Vec::with_capacity(1 + payload.len());
                        framed.push(track_id);
                        framed.extend_from_slice(&payload);
                        let len = framed.len() as u64;
                        if let Err(e) = socket.send(Message::Binary(framed.into())).await {
                            tracing::debug!(error = ?e, "WS send error");
                            break;
                        }
                        metrics::counter!("lvqr_frames_relayed_total", "transport" => "ws").increment(1);
                        metrics::counter!("lvqr_bytes_relayed_total", "transport" => "ws").increment(len);
                    }
                    None => break,
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    None => break,
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(e)) => {
                        tracing::debug!(error = ?e, "WS recv error");
                        break;
                    }
                    Some(Ok(_)) => {
                        // Subscribers may send Ping or small Text frames
                        // over the life of the session; we do not
                        // interpret them today, so ignore and keep
                        // polling.
                    }
                }
            }
        }
    }

    cancel.cancel();

    // Session 111-B2: release the mesh peer_id when the
    // subscriber disconnects. `remove_peer` returns the list of
    // orphaned children (peers that had this peer as parent);
    // each is reassigned immediately so the tree does not leak
    // orphans. Symmetric to the `add_peer` call at session
    // start.
    if let (Some(peer_id), Some(mesh)) = (mesh_peer_id, state.mesh.as_ref()) {
        let orphans = mesh.remove_peer(&peer_id);
        for orphan in orphans {
            let _ = mesh.reassign_peer(&orphan);
        }
    }

    tracing::info!(broadcast = %broadcast, "WS relay session ended");
}

async fn relay_track(
    mut track: lvqr_moq::TrackConsumer,
    track_id: u8,
    tx: tokio::sync::mpsc::Sender<(u8, Bytes)>,
    cancel: CancellationToken,
) {
    loop {
        let group = tokio::select! {
            res = track.next_group() => res,
            _ = cancel.cancelled() => return,
        };
        let mut group = match group {
            Ok(Some(g)) => g,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(track_id, error = ?e, "track error");
                return;
            }
        };
        loop {
            let frame = tokio::select! {
                res = group.read_frame() => res,
                _ = cancel.cancelled() => return,
            };
            match frame {
                Ok(Some(bytes)) => {
                    if tx.send((track_id, bytes)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(track_id, error = ?e, "group read error");
                    return;
                }
            }
        }
    }
}

pub(crate) async fn ws_ingest_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsRelayState>,
    Path(broadcast): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    tracing::info!(broadcast = %broadcast, "WebSocket ingest request");
    let resolved = resolve_ws_token(&headers, &params, "ws_ingest");
    let decision = state
        .auth
        .check(&extract::extract_ws_ingest(resolved.token.as_deref(), &broadcast));
    if let AuthDecision::Deny { reason } = decision {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WS ingest denied");
        metrics::counter!("lvqr_auth_failures_total", "entry" => "ws_ingest").increment(1);
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    metrics::counter!("lvqr_ws_connections_total", "direction" => "publish").increment(1);
    let ws = match resolved.offered_subprotocol {
        Some(ref p) => ws.protocols(std::iter::once(p.clone())),
        None => ws,
    };
    ws.on_upgrade(move |socket| ws_ingest_session(socket, state, broadcast))
        .into_response()
}

/// Result of extracting a bearer token from a WebSocket upgrade request.
///
/// The preferred transport is the `Sec-WebSocket-Protocol` header with a
/// value of `lvqr.bearer.<token>`. When the client offers that, the
/// matching subprotocol string is echoed back so axum's upgrade handshake
/// accepts it. The legacy `?token=` query parameter is still accepted as
/// a fallback but logs a deprecation warning.
struct WsTokenResolution {
    token: Option<String>,
    offered_subprotocol: Option<String>,
}

fn resolve_ws_token(headers: &HeaderMap, params: &HashMap<String, String>, entry: &'static str) -> WsTokenResolution {
    if let Some(hv) = headers.get("sec-websocket-protocol")
        && let Ok(raw) = hv.to_str()
    {
        for item in raw.split(',') {
            let proto = item.trim();
            if let Some(tok) = proto.strip_prefix("lvqr.bearer.")
                && !tok.is_empty()
            {
                return WsTokenResolution {
                    token: Some(tok.to_string()),
                    offered_subprotocol: Some(proto.to_string()),
                };
            }
        }
    }

    if let Some(tok) = params.get("token").filter(|t| !t.is_empty()) {
        tracing::warn!(
            entry = entry,
            "deprecated: ?token= query parameter; migrate to Sec-WebSocket-Protocol: lvqr.bearer.<token>"
        );
        return WsTokenResolution {
            token: Some(tok.clone()),
            offered_subprotocol: None,
        };
    }

    WsTokenResolution {
        token: None,
        offered_subprotocol: None,
    }
}

async fn ws_ingest_session(mut socket: WebSocket, state: WsRelayState, broadcast: String) {
    use lvqr_ingest::remux;

    tracing::info!(broadcast = %broadcast, "WS ingest session started");

    let Some(mut bc) = state.origin.create_broadcast(&broadcast) else {
        tracing::warn!(broadcast = %broadcast, "broadcast creation failed");
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4409,
                reason: "broadcast already exists".into(),
            })))
            .await;
        return;
    };

    state.events.emit(RelayEvent::BroadcastStarted {
        name: broadcast.clone(),
    });

    let Ok(mut video_track) = bc.create_track(Track::new("0.mp4")) else {
        tracing::warn!("failed to create video track");
        return;
    };
    let Ok(mut catalog_track) = bc.create_track(Track::new(".catalog")) else {
        tracing::warn!("failed to create catalog track");
        return;
    };

    let mut _video_config: Option<remux::VideoConfig> = None;
    let mut video_init: Option<Bytes> = None;
    let mut video_group: Option<lvqr_moq::GroupProducer> = None;
    let mut video_seq: u32 = 0;
    let _ = socket.send(Message::Text(r#"{"status":"ready"}"#.into())).await;

    while let Some(msg) = socket.recv().await {
        let data = match msg {
            Ok(Message::Binary(data)) => data,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                tracing::debug!(error = ?e, "WS ingest recv error");
                break;
            }
        };

        if data.len() < 5 {
            continue;
        }

        let msg_type = data[0];
        let timestamp = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        let payload = Bytes::from(data[5..].to_vec());

        match msg_type {
            0 => {
                if payload.len() < 6 {
                    continue;
                }
                let vid_width = u16::from_be_bytes([payload[0], payload[1]]);
                let vid_height = u16::from_be_bytes([payload[2], payload[3]]);
                let avcc_data = &payload[4..];

                match parse_avcc_record(avcc_data) {
                    Some(config) => {
                        tracing::info!(
                            broadcast = %broadcast,
                            codec = %config.codec_string(),
                            width = vid_width,
                            height = vid_height,
                            "WS ingest: video config received"
                        );
                        let init = remux::video_init_segment_with_size(&config, vid_width, vid_height);
                        _video_config = Some(config.clone());
                        video_init = Some(init.clone());
                        state.init_segments.insert(broadcast.clone(), init);

                        let json = remux::generate_catalog(Some(&config), None);
                        if let Ok(mut group) = catalog_track.append_group() {
                            let _ = group.write_frame(Bytes::from(json));
                            let _ = group.finish();
                        }
                    }
                    None => {
                        tracing::warn!("invalid AVCC record from browser");
                    }
                }
            }
            1 => {
                let Some(ref init) = video_init else { continue };
                if let Some(mut g) = video_group.take() {
                    let _ = g.finish();
                }
                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = lvqr_cmaf::RawSample {
                    track_id: 1,
                    dts: base_dts,
                    cts_offset: 0,
                    duration: 3000,
                    payload,
                    keyframe: true,
                };
                if let Ok(mut group) = video_track.append_group() {
                    let _ = group.write_frame(init.clone());
                    let seg = lvqr_cmaf::build_moof_mdat(video_seq, 1, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                    video_group = Some(group);
                }
            }
            2 => {
                if video_init.is_none() {
                    continue;
                }
                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = lvqr_cmaf::RawSample {
                    track_id: 1,
                    dts: base_dts,
                    cts_offset: 0,
                    duration: 3000,
                    payload,
                    keyframe: false,
                };
                if let Some(ref mut group) = video_group {
                    let seg = lvqr_cmaf::build_moof_mdat(video_seq, 1, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                }
            }
            _ => {}
        }
    }

    if let Some(mut g) = video_group.take() {
        let _ = g.finish();
    }
    state.events.emit(RelayEvent::BroadcastStopped {
        name: broadcast.clone(),
    });
    tracing::info!(broadcast = %broadcast, "WS ingest session ended");
}

/// Background task that listens on the event bus for new broadcasts and
/// starts a recorder for each one. Event-driven so WS-ingested broadcasts
/// are recorded identically to RTMP-ingested ones.
pub(crate) async fn spawn_recordings(
    recorder: lvqr_record::BroadcastRecorder,
    origin: lvqr_moq::OriginProducer,
    mut events: tokio::sync::broadcast::Receiver<RelayEvent>,
    shutdown: CancellationToken,
) {
    let mut active: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let event = tokio::select! {
            res = events.recv() => res,
            _ = shutdown.cancelled() => return,
        };
        let event = match event {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "recorder event stream lagged");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        match event {
            RelayEvent::BroadcastStarted { name } => {
                if !active.insert(name.clone()) {
                    continue;
                }
                let consumer = origin.consume();
                let Some(broadcast) = consumer.consume_broadcast(&name) else {
                    tracing::warn!(broadcast = %name, "recorder: broadcast not resolvable yet");
                    active.remove(&name);
                    continue;
                };
                let recorder = recorder.clone();
                let cancel = shutdown.clone();
                tracing::info!(broadcast = %name, "starting recording");
                let name_clone = name.clone();
                tokio::spawn(async move {
                    let _ = recorder
                        .record_broadcast(&name_clone, broadcast, lvqr_record::RecordOptions::default(), cancel)
                        .await;
                });
            }
            RelayEvent::BroadcastStopped { name } => {
                active.remove(&name);
            }
            RelayEvent::ViewerJoined { .. } | RelayEvent::ViewerLeft { .. } => {}
        }
    }
}

/// Parse an AVCDecoderConfigurationRecord from a WS ingest `type=0` payload.
fn parse_avcc_record(data: &[u8]) -> Option<lvqr_ingest::remux::VideoConfig> {
    if data.len() < 6 {
        return None;
    }
    let profile = data[1];
    let compat = data[2];
    let level = data[3];
    let nalu_length_size = (data[4] & 0x03) + 1;

    let num_sps = (data[5] & 0x1F) as usize;
    let mut offset = 6;
    let mut sps_list = Vec::with_capacity(num_sps);
    for _ in 0..num_sps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        sps_list.push(data[offset..offset + len].to_vec());
        offset += len;
    }

    if offset >= data.len() {
        return None;
    }
    let num_pps = data[offset] as usize;
    offset += 1;
    let mut pps_list = Vec::with_capacity(num_pps);
    for _ in 0..num_pps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        pps_list.push(data[offset..offset + len].to_vec());
        offset += len;
    }

    if sps_list.is_empty() || pps_list.is_empty() {
        return None;
    }

    Some(lvqr_ingest::remux::VideoConfig {
        sps_list,
        pps_list,
        profile,
        compat,
        level,
        nalu_length_size,
    })
}
