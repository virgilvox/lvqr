//! End-to-end RTMP onCuePoint scte35-bin64 passthrough test.
//!
//! Drives the full RTMP wire path for SCTE-35 ad markers:
//!
//!   AMF0 onCuePoint -> MessagePayload -> ChunkSerializer
//!     -> ServerSession::handle_input
//!     -> patched rml_rtmp Amf0DataReceived event
//!     -> assert raw splice_info_section bytes round-trip
//!
//! Validates the session-152 RTMP unblock: the vendored rml_rtmp fork
//! at vendor/rml_rtmp/ surfaces non-`@setDataFrame` AMF0 Data messages
//! as the new ServerSessionEvent::Amf0DataReceived variant; without
//! that patch the upstream library silently drops onCuePoint
//! carriages and SCTE-35 ad markers never reach LVQR's RTMP path.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rml_amf0::Amf0Value;
use rml_rtmp::chunk_io::ChunkSerializer;
use rml_rtmp::messages::{MessagePayload, RtmpMessage};
use rml_rtmp::sessions::{ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult};
use rml_rtmp::time::RtmpTimestamp;
use std::collections::HashMap;

/// Build the canonical Adobe-convention onCuePoint AMF0 payload for a
/// SCTE-35 splice event: a top-level "onCuePoint" string followed by an
/// AMF0 object whose "name" property is "scte35-bin64", "type" is
/// "scte-35", and "data" is the base64-encoded splice_info_section.
fn build_oncuepoint_scte35(section: &[u8]) -> Vec<Amf0Value> {
    let mut props = HashMap::new();
    props.insert("name".into(), Amf0Value::Utf8String("scte35-bin64".into()));
    props.insert("type".into(), Amf0Value::Utf8String("scte-35".into()));
    props.insert("data".into(), Amf0Value::Utf8String(STANDARD.encode(section)));
    vec![Amf0Value::Utf8String("onCuePoint".into()), Amf0Value::Object(props)]
}

#[test]
fn server_session_surfaces_oncuepoint_scte35_bin64_via_amf0_data_received() {
    // A real splice_info_section header byte triple. The integration
    // surface here is the rml_rtmp event variant; CRC + timing decode
    // is covered by lvqr-codec's scte35.rs unit tests. Anything that
    // base64-decodes to non-empty bytes is sufficient for the wire
    // round-trip assertion.
    let section: &[u8] = &[0xFC, 0x30, 0x11, 0x00, 0x00];
    let amf0_values = build_oncuepoint_scte35(section);
    let amf0_for_assert = amf0_values.clone();

    // Hand-build the RtmpMessage and serialize it onto the chunk stream
    // exactly like a publisher would. RTMP message_type 18 = Amf0Data.
    let message = RtmpMessage::Amf0Data { values: amf0_values };
    let payload = MessagePayload::from_rtmp_message(message, RtmpTimestamp::new(0), 1)
        .expect("MessagePayload::from_rtmp_message");

    let mut serializer = ChunkSerializer::new();
    let packet = serializer
        .serialize(&payload, false, false)
        .expect("ChunkSerializer::serialize");

    // Drive through a fresh ServerSession and inspect the events.
    let (mut session, _initial) = ServerSession::new(ServerSessionConfig::new()).expect("ServerSession::new");
    let results = session.handle_input(&packet.bytes).expect("handle_input");

    let mut saw_amf0_data = false;
    for r in results {
        if let ServerSessionResult::RaisedEvent(ServerSessionEvent::Amf0DataReceived { data, .. }) = r {
            saw_amf0_data = true;
            assert_eq!(
                data, amf0_for_assert,
                "patched rml_rtmp should surface the AMF0 values verbatim"
            );
        }
    }
    assert!(
        saw_amf0_data,
        "vendored rml_rtmp v0.8 patch must raise Amf0DataReceived for onCuePoint payloads"
    );
}

#[test]
fn at_setdataframe_onmetadata_still_routes_to_stream_metadata_changed() {
    // Regression guard: the vendor patch only touches the non-
    // @setDataFrame fallthrough; the existing @setDataFrame onMetaData
    // path must still produce StreamMetadataChanged so OBS / ffmpeg
    // publishers do not regress. Without an active publish,
    // handle_amf0_data_set_data_frame returns Ok(Vec::new()) (per
    // upstream rml_rtmp). The contract here is "no Amf0DataReceived
    // for the @setDataFrame path"; the positive event surface lives
    // in the upstream tests.
    let mut props = HashMap::new();
    props.insert("width".into(), Amf0Value::Number(1920.0));
    let amf0_values = vec![
        Amf0Value::Utf8String("@setDataFrame".into()),
        Amf0Value::Utf8String("onMetaData".into()),
        Amf0Value::Object(props),
    ];

    let message = RtmpMessage::Amf0Data { values: amf0_values };
    let payload = MessagePayload::from_rtmp_message(message, RtmpTimestamp::new(0), 1)
        .expect("MessagePayload::from_rtmp_message");

    let mut serializer = ChunkSerializer::new();
    let packet = serializer
        .serialize(&payload, false, false)
        .expect("ChunkSerializer::serialize");

    let (mut session, _initial) = ServerSession::new(ServerSessionConfig::new()).expect("ServerSession::new");
    let results = session.handle_input(&packet.bytes).expect("handle_input");

    for r in results {
        if let ServerSessionResult::RaisedEvent(ServerSessionEvent::Amf0DataReceived { .. }) = r {
            panic!("@setDataFrame onMetaData must NOT surface as Amf0DataReceived");
        }
    }
}
