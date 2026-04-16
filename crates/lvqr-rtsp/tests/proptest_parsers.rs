//! Property tests for the RTSP/RTP parsers.
//!
//! Proptest slot of the 5-artifact contract for `lvqr-rtsp`. Every
//! parser exposed by the crate handles untrusted network input, so
//! the primary invariant is: **never panic on arbitrary bytes.**
//!
//! Covered attack surfaces:
//! - `proto::parse_request` -- RTSP text protocol from TCP
//! - `proto::parse_transport` -- Transport header value
//! - `rtp::parse_interleaved_frame` -- $-prefixed binary framing
//! - `rtp::parse_rtp_header` -- RTP fixed header (12+ bytes)
//! - `rtp::H264Depacketizer::depacketize` -- H.264 NAL reassembly
//! - `rtp::HevcDepacketizer::depacketize` -- HEVC NAL reassembly

use lvqr_rtsp::proto;
use lvqr_rtsp::rtp;
use proptest::prelude::*;

// ----------------------------------------------------------------
// RTSP text protocol parsers
// ----------------------------------------------------------------

proptest! {
    /// parse_request must never panic on arbitrary bytes.
    #[test]
    fn parse_request_never_panics(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = proto::parse_request(&data);
    }

    /// parse_transport must never panic on arbitrary strings.
    #[test]
    fn parse_transport_never_panics(s in "\\PC{0,512}") {
        let _ = proto::parse_transport(&s);
    }
}

// ----------------------------------------------------------------
// RTP binary parsers
// ----------------------------------------------------------------

proptest! {
    /// parse_interleaved_frame must never panic on arbitrary bytes.
    #[test]
    fn parse_interleaved_frame_never_panics(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let _ = rtp::parse_interleaved_frame(&data);
    }

    /// parse_rtp_header must never panic on arbitrary bytes.
    #[test]
    fn parse_rtp_header_never_panics(data in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = rtp::parse_rtp_header(&data);
    }

    /// H264Depacketizer must never panic on arbitrary RTP payloads.
    #[test]
    fn h264_depack_never_panics(payload in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let header = rtp::RtpHeader {
            payload_type: 96,
            sequence: 0,
            timestamp: 0,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        let mut depack = rtp::H264Depacketizer::new();
        let _ = depack.depacketize(&payload, &header);
    }

    /// HevcDepacketizer must never panic on arbitrary RTP payloads.
    #[test]
    fn hevc_depack_never_panics(payload in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let header = rtp::RtpHeader {
            payload_type: 96,
            sequence: 0,
            timestamp: 0,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        let mut depack = rtp::HevcDepacketizer::new();
        let _ = depack.depacketize(&payload, &header);
    }
}

// ----------------------------------------------------------------
// H.264 FU-A multi-packet reassembly
// ----------------------------------------------------------------

proptest! {
    /// Feeding a random sequence of FU-A-shaped packets must never
    /// panic, even when S/E flags are nonsensical.
    #[test]
    fn h264_fu_a_random_sequence_never_panics(
        packets in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 2..128),
            1..16
        )
    ) {
        let mut depack = rtp::H264Depacketizer::new();
        for (i, pkt) in packets.iter().enumerate() {
            let header = rtp::RtpHeader {
                payload_type: 96,
                sequence: i as u16,
                timestamp: 90000,
                ssrc: 0,
                marker: i == packets.len() - 1,
                header_len: 12,
            };
            let _ = depack.depacketize(pkt, &header);
        }
    }

    /// Feeding a random sequence of HEVC FU packets must never panic.
    #[test]
    fn hevc_fu_random_sequence_never_panics(
        packets in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 3..128),
            1..16
        )
    ) {
        let mut depack = rtp::HevcDepacketizer::new();
        for (i, pkt) in packets.iter().enumerate() {
            let header = rtp::RtpHeader {
                payload_type: 96,
                sequence: i as u16,
                timestamp: 90000,
                ssrc: 0,
                marker: i == packets.len() - 1,
                header_len: 12,
            };
            let _ = depack.depacketize(pkt, &header);
        }
    }
}

// ----------------------------------------------------------------
// Interleaved framing round-trip
// ----------------------------------------------------------------

proptest! {
    /// A correctly framed interleaved packet must round-trip through
    /// parse_interleaved_frame.
    #[test]
    fn interleaved_frame_roundtrip(
        channel in any::<u8>(),
        payload in proptest::collection::vec(any::<u8>(), 0..1024)
    ) {
        let len = payload.len() as u16;
        let mut buf = vec![0x24, channel];
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&payload);
        let (frame, consumed) = rtp::parse_interleaved_frame(&buf).unwrap();
        prop_assert_eq!(frame.channel, channel);
        let payload_len = payload.len();
        prop_assert_eq!(frame.payload, payload);
        prop_assert_eq!(consumed, 4 + payload_len);
    }
}
