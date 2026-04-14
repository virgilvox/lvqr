//! Property tests for the H.264 RTP packetizer.
//!
//! This is the proptest slot of the 5-artifact contract for
//! `lvqr-whep`. The fuzz / integration / e2e / conformance slots
//! land alongside the signaling layer in a later session; see
//! `crates/lvqr-whep/docs/design.md` for the full plan.
//!
//! Two invariants:
//!
//! 1. **Never panic on arbitrary input.** `H264Packetizer::packetize`
//!    must handle attacker-shaped byte sequences — truncated length
//!    prefixes, giant length fields, zero-length NALs, interleaved
//!    garbage — without panicking. A crash here would let any WHEP
//!    client take the server down by sending a malformed RTMP publish
//!    upstream that flows through the `RawSampleObserver` hook.
//!
//! 2. **FU-A round-trip.** For every input NAL that exceeds the MTU,
//!    the FU-A fragments the packetizer emits must reassemble back
//!    to the original NAL body byte-for-byte (header reconstructed
//!    from the F|NRI bits in the FU indicator plus the type field
//!    in the FU header). Browser decoders rely on this contract.

use bytes::Bytes;
use lvqr_whep::{H264Packetizer, H264RtpPayload};
use proptest::prelude::*;

/// Build an AVCC-formatted byte sequence from one or more NAL
/// bodies. Each entry is `[u32-be length][body]`.
fn avcc(nals: &[Vec<u8>]) -> Vec<u8> {
    let mut buf = Vec::new();
    for nal in nals {
        buf.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        buf.extend_from_slice(nal);
    }
    buf
}

/// Strategy: a plausible NAL unit body. The first byte is a NAL
/// header (forbidden_zero_bit=0, nal_ref_idc in 0..=3, type in
/// 1..=23 — type 28 clashes with FU-A, so we stay below it). The
/// rest of the body is arbitrary bytes.
fn nal_strategy() -> impl Strategy<Value = Vec<u8>> {
    (0u8..=3, 1u8..=23, proptest::collection::vec(any::<u8>(), 0..512)).prop_map(|(nri, ty, body)| {
        let mut v = Vec::with_capacity(body.len() + 1);
        let header = (nri << 5) | (ty & 0b0001_1111);
        v.push(header);
        v.extend_from_slice(&body);
        v
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        .. ProptestConfig::default()
    })]

    /// `packetize` must never panic on fully arbitrary bytes. MTU is
    /// drawn from the same arbitrary space so the packetizer's MTU
    /// clamp is also exercised.
    #[test]
    fn packetize_never_panics_on_arbitrary_bytes(
        buf in proptest::collection::vec(any::<u8>(), 0..2048),
        mtu in 0usize..4096,
    ) {
        let p = H264Packetizer::new(mtu);
        let _ = p.packetize(&buf);
    }

    /// `packetize` on well-formed AVCC input emits at least one
    /// payload per non-empty input NAL, and every emitted payload is
    /// bounded by the configured MTU.
    #[test]
    fn packetize_respects_mtu_budget(
        nals in proptest::collection::vec(nal_strategy(), 1..6),
        mtu in 16usize..1500,
    ) {
        let buf = avcc(&nals);
        let p = H264Packetizer::new(mtu);
        let out = p.packetize(&buf);

        prop_assert!(!out.is_empty(), "non-empty input must produce at least one packet");
        for pk in &out {
            prop_assert!(pk.payload.len() <= p.mtu(),
                "payload length {} exceeds MTU {}", pk.payload.len(), p.mtu());
            prop_assert!(!pk.payload.is_empty(), "payload must be non-empty");
        }

        // The very first packet is start-of-frame, the very last is
        // end-of-frame. Every other packet is neither.
        prop_assert!(out.first().unwrap().is_start_of_frame);
        prop_assert!(out.last().unwrap().is_end_of_frame);
        if out.len() > 2 {
            for pk in &out[1..out.len() - 1] {
                prop_assert!(!pk.is_start_of_frame);
                prop_assert!(!pk.is_end_of_frame);
            }
        }
    }

    /// FU-A round-trip: pick a NAL that is guaranteed to exceed the
    /// MTU, packetize, reassemble, and assert byte equality with the
    /// original NAL. This is the contract a WHEP client's depacketizer
    /// will rely on when reconstructing frames on the receiver side.
    #[test]
    fn fua_fragments_round_trip(
        // NAL body is sized so that header + body > mtu is guaranteed.
        body in proptest::collection::vec(any::<u8>(), 200..2000),
        nri in 0u8..=3,
        nal_type in 1u8..=23,
        mtu in 16usize..128,
    ) {
        let mut nal = Vec::with_capacity(body.len() + 1);
        nal.push((nri << 5) | (nal_type & 0b0001_1111));
        nal.extend_from_slice(&body);
        let buf = avcc(&[nal.clone()]);

        let p = H264Packetizer::new(mtu);
        let out: Vec<H264RtpPayload> = p.packetize(&buf);

        // With mtu small and body large, FU-A is guaranteed.
        prop_assert!(out.len() > 1, "FU-A should produce > 1 fragment, got {}", out.len());

        // Every fragment carries the FU indicator type = 28.
        for frag in &out {
            prop_assert!(frag.payload.len() >= 2, "fragment missing FU header");
            prop_assert_eq!(frag.payload[0] & 0b0001_1111, 28, "FU indicator type must be 28 (FU-A)");
            prop_assert!(frag.payload.len() <= p.mtu(), "fragment exceeds MTU");
        }

        // Exactly one Start, exactly one End, first and last.
        let starts = out.iter().filter(|f| f.payload[1] & 0b1000_0000 != 0).count();
        let ends = out.iter().filter(|f| f.payload[1] & 0b0100_0000 != 0).count();
        prop_assert_eq!(starts, 1);
        prop_assert_eq!(ends, 1);
        prop_assert!(out.first().unwrap().payload[1] & 0b1000_0000 != 0);
        prop_assert!(out.last().unwrap().payload[1] & 0b0100_0000 != 0);

        // Reassemble: header from (F|NRI bits of FU indicator) | type
        // from FU header low 5 bits; body from the concatenation of
        // every fragment payload past the 2-byte FU header.
        let header = (out[0].payload[0] & 0b1110_0000) | (out[0].payload[1] & 0b0001_1111);
        let mut reassembled = vec![header];
        for frag in &out {
            reassembled.extend_from_slice(&frag.payload[2..]);
        }
        prop_assert_eq!(Bytes::from(reassembled), Bytes::from(nal));
    }

    /// Single-NAL-unit mode round-trip: a NAL that fits within the
    /// MTU must emit exactly one packet whose payload is the NAL
    /// body byte-for-byte (header included).
    #[test]
    fn single_nal_packet_is_verbatim(
        nal in nal_strategy().prop_filter("small enough", |v| v.len() <= 512),
    ) {
        let buf = avcc(std::slice::from_ref(&nal));
        let p = H264Packetizer::new(1200);
        let out = p.packetize(&buf);
        prop_assert_eq!(out.len(), 1);
        prop_assert_eq!(out[0].payload.as_ref(), nal.as_slice());
    }
}
