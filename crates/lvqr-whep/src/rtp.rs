//! H.264 RTP packetizer (RFC 6184).
//!
//! Consumes AVCC length-prefixed NAL unit bytes — the same layout
//! `lvqr_cmaf::RawSample::payload` carries for AVC — and emits a
//! sequence of RTP payloads ready for a WebRTC sender to wrap in
//! RTP headers.
//!
//! Two modes are implemented:
//!
//! * **Single NAL unit mode** (RFC 6184 §5.6). When a NAL fits
//!   within the configured MTU budget, the NAL body (without its
//!   AVCC length prefix) becomes a single RTP payload verbatim. The
//!   first byte of the payload is the NAL header byte, which
//!   doubles as the RFC 6184 type selector (values 1..23).
//!
//! * **FU-A fragmentation** (RFC 6184 §5.8). When a NAL exceeds the
//!   budget, the packetizer splits the NAL body (minus its header
//!   byte) into chunks of at most `fu_payload_budget()` bytes. Each
//!   chunk is prefixed with a two-byte FU header: the FU indicator
//!   (`F | NRI | type=28`) and the FU header (`S | E | R | type`)
//!   where `S` marks the first fragment, `E` the last, and `type` is
//!   the original NAL unit type. The original NAL header is not
//!   carried in any FU-A fragment other than via the reconstructed
//!   `F | NRI | type` fields.
//!
//! STAP-A (§5.7) aggregation is intentionally omitted. It is a
//! micro-optimization for tiny NALs (parameter sets, SEIs) that
//! browser clients accept in single-NAL form anyway.
//!
//! The input payload is expected to be AVCC-formatted: a sequence of
//! `[4-byte big-endian length][body...]` tuples. Inputs that are
//! malformed (truncated length prefixes, length fields that point
//! past the buffer end, zero-length bodies) are skipped silently so
//! the packetizer is safe to call on arbitrary attacker-shaped
//! bytes. The proptest slot at `tests/proptest_packetizer.rs`
//! enforces the never-panic property.

use bytes::{BufMut, Bytes, BytesMut};

/// Default maximum RTP payload size in bytes.
///
/// Matches the "safe" MTU that every major WebRTC stack (`str0m`,
/// Pion, libwebrtc) uses by default. WebRTC Ethernet MTU is 1500,
/// minus the IPv4 (20) + UDP (8) + SRTP auth tag (10) + RTP header
/// (12) + room for a few RTP header extensions. 1200 bytes leaves
/// plenty of slack.
pub const DEFAULT_MTU: usize = 1200;

/// A single RTP payload emitted by the packetizer.
///
/// The `payload` is the bytes placed after the RTP fixed header
/// (the header itself is written by the sender, not here).
/// `is_start_of_frame` is true for the first payload belonging to a
/// given input NAL sequence, and `is_end_of_frame` is true for the
/// last. A WebRTC sender typically maps `is_end_of_frame` onto the
/// RTP marker bit for video streams.
#[derive(Debug, Clone)]
pub struct H264RtpPayload {
    /// RTP packet payload bytes (ready to concatenate with an RTP
    /// header written by the caller).
    pub payload: Bytes,
    /// Whether this is the first RTP packet carrying this input
    /// NAL sequence.
    pub is_start_of_frame: bool,
    /// Whether this is the last RTP packet carrying this input NAL
    /// sequence. Maps onto the RTP marker bit for video.
    pub is_end_of_frame: bool,
}

/// Stateless H.264 RTP packetizer.
///
/// The packetizer holds its MTU budget and nothing else. Callers
/// invoke [`H264Packetizer::packetize`] once per sample, receive a
/// `Vec<H264RtpPayload>`, and push the payloads through an RTP
/// sender.
#[derive(Debug, Clone, Copy)]
pub struct H264Packetizer {
    mtu: usize,
}

impl Default for H264Packetizer {
    fn default() -> Self {
        Self::new(DEFAULT_MTU)
    }
}

impl H264Packetizer {
    /// Build a new packetizer with the given MTU budget in bytes.
    /// The MTU is clamped to a minimum of `FU_HEADER_SIZE + 1` so a
    /// single-byte fragment is always representable.
    pub fn new(mtu: usize) -> Self {
        let mtu = mtu.max(FU_HEADER_SIZE + 1);
        Self { mtu }
    }

    /// Budget for a single RTP payload, excluding the RTP fixed
    /// header. Equal to the MTU.
    pub const fn mtu(&self) -> usize {
        self.mtu
    }

    /// FU-A payload budget: MTU minus the two-byte FU header.
    pub const fn fu_payload_budget(&self) -> usize {
        self.mtu - FU_HEADER_SIZE
    }

    /// Packetize an AVCC length-prefixed NAL sequence into a list
    /// of RTP payloads. Returns an empty vec when the input contains
    /// no usable NAL units.
    ///
    /// Malformed NAL units (truncated length prefix, length field
    /// overruns the buffer, zero-length body) are skipped silently.
    /// The packetizer never panics on malformed input.
    pub fn packetize(&self, avcc: &[u8]) -> Vec<H264RtpPayload> {
        let nals = split_avcc(avcc);
        if nals.is_empty() {
            return Vec::new();
        }

        // First, compute the full sequence of RTP payloads for every
        // NAL in the input. We flag the first payload of the first
        // NAL as start-of-frame and the last payload of the last NAL
        // as end-of-frame. This matches how WebRTC senders set the
        // RTP marker bit for video: once per access unit, not once
        // per NAL.
        let mut all: Vec<H264RtpPayload> = Vec::with_capacity(nals.len());
        for nal in nals.iter() {
            self.packetize_one_nal(nal, &mut all);
        }
        if let Some(first) = all.first_mut() {
            first.is_start_of_frame = true;
        }
        if let Some(last) = all.last_mut() {
            last.is_end_of_frame = true;
        }
        all
    }

    /// Packetize a single NAL into either one single-NAL payload or
    /// a sequence of FU-A fragments, appending to `out`.
    fn packetize_one_nal(&self, nal: &[u8], out: &mut Vec<H264RtpPayload>) {
        debug_assert!(
            !nal.is_empty(),
            "empty NALs must be filtered before reaching packetize_one_nal"
        );
        if nal.len() <= self.mtu {
            out.push(H264RtpPayload {
                payload: Bytes::copy_from_slice(nal),
                is_start_of_frame: false,
                is_end_of_frame: false,
            });
            return;
        }

        // Fragment the NAL body (minus the header byte) into FU-A
        // payloads. `header` keeps the F / NRI bits from the
        // original NAL header; `nal_type` is the type selector we
        // stuff into the FU header.
        let header = nal[0];
        let f_nri = header & 0b1110_0000;
        let nal_type = header & 0b0001_1111;
        let body = &nal[1..];
        let budget = self.fu_payload_budget();

        let mut offset = 0;
        let total = body.len();
        while offset < total {
            let end = (offset + budget).min(total);
            let chunk = &body[offset..end];
            let is_first = offset == 0;
            let is_last = end == total;

            let mut buf = BytesMut::with_capacity(FU_HEADER_SIZE + chunk.len());
            // FU indicator: F | NRI | type=28 (FU-A)
            buf.put_u8(f_nri | FU_A_TYPE);
            // FU header: S | E | R(0) | original type
            let mut fu_header: u8 = nal_type & 0b0001_1111;
            if is_first {
                fu_header |= 0b1000_0000;
            }
            if is_last {
                fu_header |= 0b0100_0000;
            }
            buf.put_u8(fu_header);
            buf.put_slice(chunk);
            out.push(H264RtpPayload {
                payload: buf.freeze(),
                is_start_of_frame: false,
                is_end_of_frame: false,
            });

            offset = end;
        }
    }
}

/// NAL unit type for FU-A fragmentation units (RFC 6184 §5.8).
const FU_A_TYPE: u8 = 28;

/// Size of the FU-A header prefix that replaces the NAL unit's own
/// header byte across fragments: 1 byte FU indicator + 1 byte FU
/// header.
const FU_HEADER_SIZE: usize = 2;

/// Walk an AVCC-formatted byte slice and return the NAL bodies.
///
/// AVCC format is `[u32-be length][body...]` repeated. Malformed
/// entries (truncated, length overflows the buffer, zero-length
/// body) are skipped. Parsing stops at the first unparseable entry
/// rather than trying to resync, which matches how browser decoders
/// handle torn AVCC streams.
fn split_avcc(mut data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while data.len() >= 4 {
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if len == 0 {
            // zero-length NAL: skip this entry but keep walking so
            // a second valid entry in the same buffer is still
            // returned.
            data = &data[4..];
            continue;
        }
        if len > data.len() - 4 {
            // length field overruns the buffer — stop cleanly
            // rather than slice out of bounds.
            break;
        }
        out.push(&data[4..4 + len]);
        data = &data[4 + len..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an AVCC buffer from one or more NAL bodies.
    fn avcc(nals: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        for nal in nals {
            buf.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            buf.extend_from_slice(nal);
        }
        buf
    }

    #[test]
    fn empty_input_produces_nothing() {
        let p = H264Packetizer::default();
        assert!(p.packetize(&[]).is_empty());
    }

    #[test]
    fn single_small_nal_single_packet() {
        let nal = vec![0x65, 0xAA, 0xBB, 0xCC, 0xDD];
        let buf = avcc(&[&nal]);
        let p = H264Packetizer::default();
        let out = p.packetize(&buf);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload.as_ref(), nal.as_slice());
        assert!(out[0].is_start_of_frame);
        assert!(out[0].is_end_of_frame);
    }

    #[test]
    fn oversized_nal_is_fragmented_with_fua() {
        // MTU small enough to force FU-A on a short NAL.
        let mtu = 8;
        let p = H264Packetizer::new(mtu);
        let body: Vec<u8> = (0..30u8).collect();
        let mut nal = vec![0x65]; // NAL header: IDR slice, nal_ref_idc=3
        nal.extend_from_slice(&body);

        let buf = avcc(&[&nal]);
        let out = p.packetize(&buf);
        assert!(out.len() > 1, "NAL should have been fragmented, got {}", out.len());

        // Every fragment must carry the FU indicator + FU header
        // prefix and fit within the configured MTU.
        for frag in &out {
            assert!(frag.payload.len() >= FU_HEADER_SIZE);
            assert!(frag.payload.len() <= mtu);
            assert_eq!(frag.payload[0] & 0b0001_1111, FU_A_TYPE);
        }

        // Exactly one fragment has the Start bit set, exactly one
        // has the End bit set, and they must be the first and last
        // respectively.
        let starts: Vec<_> = out.iter().filter(|f| f.payload[1] & 0b1000_0000 != 0).collect();
        let ends: Vec<_> = out.iter().filter(|f| f.payload[1] & 0b0100_0000 != 0).collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(ends.len(), 1);
        assert!(out.first().unwrap().payload[1] & 0b1000_0000 != 0);
        assert!(out.last().unwrap().payload[1] & 0b0100_0000 != 0);

        // Reassembling the FU-A payloads reproduces the original
        // NAL body (header byte is reconstructed from F|NRI + type).
        let mut reassembled = Vec::new();
        reassembled.push((out[0].payload[0] & 0b1110_0000) | (out[0].payload[1] & 0b0001_1111));
        for frag in &out {
            reassembled.extend_from_slice(&frag.payload[FU_HEADER_SIZE..]);
        }
        assert_eq!(reassembled, nal);

        // Start-of-frame / end-of-frame flags line up with the
        // first and last emitted packet.
        assert!(out.first().unwrap().is_start_of_frame);
        assert!(out.last().unwrap().is_end_of_frame);
    }

    #[test]
    fn two_small_nals_emit_two_single_nal_packets() {
        let a = vec![0x67, 0x01, 0x02];
        let b = vec![0x68, 0x03, 0x04];
        let buf = avcc(&[&a, &b]);
        let p = H264Packetizer::default();
        let out = p.packetize(&buf);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].payload.as_ref(), a.as_slice());
        assert_eq!(out[1].payload.as_ref(), b.as_slice());
        assert!(out[0].is_start_of_frame);
        assert!(!out[0].is_end_of_frame);
        assert!(!out[1].is_start_of_frame);
        assert!(out[1].is_end_of_frame);
    }

    #[test]
    fn truncated_length_prefix_is_tolerated() {
        let p = H264Packetizer::default();
        // 3 bytes is not enough for a length prefix; walker stops
        // cleanly and produces nothing.
        assert!(p.packetize(&[0, 0, 0]).is_empty());
    }

    #[test]
    fn length_overruns_buffer_stops_cleanly() {
        let p = H264Packetizer::default();
        // length = 1000 but body is only 3 bytes: walker stops.
        let buf = vec![0, 0, 0x03, 0xE8, 1, 2, 3];
        assert!(p.packetize(&buf).is_empty());
    }

    #[test]
    fn zero_length_nal_is_skipped() {
        // Two entries: first has length 0, second is a normal NAL.
        // Walker must skip the zero entry and return the second.
        let mut buf = vec![0, 0, 0, 0];
        let real = vec![0x65, 1, 2, 3];
        buf.extend_from_slice(&(real.len() as u32).to_be_bytes());
        buf.extend_from_slice(&real);
        let p = H264Packetizer::default();
        let out = p.packetize(&buf);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload.as_ref(), real.as_slice());
    }

    #[test]
    fn mtu_floor_keeps_fu_header_representable() {
        // Caller passes mtu=0; packetizer clamps so FU fragments
        // can still exist with a 1-byte payload.
        let p = H264Packetizer::new(0);
        assert!(p.mtu() > FU_HEADER_SIZE);
        assert!(p.fu_payload_budget() >= 1);
    }
}
