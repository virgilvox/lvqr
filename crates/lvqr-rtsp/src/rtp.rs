//! RTP interleaved frame parsing and H.264 depacketization.
//!
//! RTSP interleaved TCP transport (RFC 2326 Section 10.12) wraps
//! RTP/RTCP packets in a 4-byte header: `$` (0x24), channel (u8),
//! length (u16 big-endian), then payload. Even channels carry RTP;
//! odd channels carry RTCP.
//!
//! H.264 RTP depacketization follows RFC 6184: single NAL unit
//! packets (type 1-23), FU-A fragmentation (type 28), and STAP-A
//! aggregation (type 24).

/// Parsed interleaved frame from the TCP stream.
#[derive(Debug)]
pub struct InterleavedFrame {
    pub channel: u8,
    pub payload: Vec<u8>,
}

/// Try to parse one interleaved frame from the buffer.
/// Returns `Some((frame, consumed_bytes))` if complete,
/// `None` if more data is needed.
pub fn parse_interleaved_frame(buf: &[u8]) -> Option<(InterleavedFrame, usize)> {
    if buf.len() < 4 {
        return None;
    }
    if buf[0] != 0x24 {
        return None;
    }
    let channel = buf[1];
    let length = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let total = 4 + length;
    if buf.len() < total {
        return None;
    }
    Some((
        InterleavedFrame {
            channel,
            payload: buf[4..total].to_vec(),
        },
        total,
    ))
}

/// Minimal RTP header fields extracted from an RTP packet.
#[derive(Debug)]
pub struct RtpHeader {
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub marker: bool,
    pub header_len: usize,
}

/// Parse the fixed RTP header (12 bytes minimum) plus CSRC and
/// extension headers to find the payload offset.
pub fn parse_rtp_header(data: &[u8]) -> Option<RtpHeader> {
    if data.len() < 12 {
        return None;
    }
    let version = (data[0] >> 6) & 0x03;
    if version != 2 {
        return None;
    }
    let padding = (data[0] >> 5) & 0x01 != 0;
    let extension = (data[0] >> 4) & 0x01 != 0;
    let csrc_count = (data[0] & 0x0F) as usize;
    let marker = (data[1] >> 7) & 0x01 != 0;
    let payload_type = data[1] & 0x7F;
    let sequence = u16::from_be_bytes([data[2], data[3]]);
    let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    let mut offset = 12 + csrc_count * 4;
    if offset > data.len() {
        return None;
    }

    if extension {
        if offset + 4 > data.len() {
            return None;
        }
        // Extension header: 2 bytes profile + 2 bytes length (in 32-bit words)
        let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4 + ext_len * 4;
        if offset > data.len() {
            return None;
        }
    }

    let payload_end = if padding && !data.is_empty() {
        let pad_len = data[data.len() - 1] as usize;
        data.len().saturating_sub(pad_len)
    } else {
        data.len()
    };

    if offset > payload_end {
        return None;
    }

    Some(RtpHeader {
        payload_type,
        sequence,
        timestamp,
        ssrc,
        marker,
        header_len: offset,
    })
}

/// H.264 NAL unit type constants for RTP depacketization.
const NAL_TYPE_STAP_A: u8 = 24;
const NAL_TYPE_FU_A: u8 = 28;

/// H.264 RTP depacketization result: one or more NAL units
/// extracted from an RTP packet.
#[derive(Debug)]
pub struct DepackResult {
    pub nalus: Vec<Vec<u8>>,
    pub keyframe: bool,
    pub timestamp: u32,
    pub marker: bool,
}

/// State for reassembling FU-A fragmented NAL units.
#[derive(Debug, Default)]
pub struct H264Depacketizer {
    fu_buf: Vec<u8>,
    fu_active: bool,
}

impl H264Depacketizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one RTP packet and return any completed NAL units.
    pub fn depacketize(&mut self, rtp_payload: &[u8], header: &RtpHeader) -> Option<DepackResult> {
        if rtp_payload.is_empty() {
            return None;
        }

        let nal_type = rtp_payload[0] & 0x1F;
        let nri = rtp_payload[0] & 0x60;

        match nal_type {
            1..=23 => {
                // Single NAL unit packet.
                let keyframe = nal_type == 5;
                Some(DepackResult {
                    nalus: vec![rtp_payload.to_vec()],
                    keyframe,
                    timestamp: header.timestamp,
                    marker: header.marker,
                })
            }
            NAL_TYPE_STAP_A => {
                // Aggregation packet: multiple NALs with 2-byte length prefix each.
                let mut nalus = Vec::new();
                let mut keyframe = false;
                let mut offset = 1; // skip STAP-A header byte
                while offset + 2 <= rtp_payload.len() {
                    let nalu_len = u16::from_be_bytes([rtp_payload[offset], rtp_payload[offset + 1]]) as usize;
                    offset += 2;
                    if offset + nalu_len > rtp_payload.len() {
                        break;
                    }
                    let nalu = &rtp_payload[offset..offset + nalu_len];
                    if !nalu.is_empty() && (nalu[0] & 0x1F) == 5 {
                        keyframe = true;
                    }
                    nalus.push(nalu.to_vec());
                    offset += nalu_len;
                }
                if nalus.is_empty() {
                    return None;
                }
                Some(DepackResult {
                    nalus,
                    keyframe,
                    timestamp: header.timestamp,
                    marker: header.marker,
                })
            }
            NAL_TYPE_FU_A => {
                // Fragmentation unit: FU indicator (1 byte) + FU header (1 byte) + payload.
                if rtp_payload.len() < 2 {
                    return None;
                }
                let fu_header = rtp_payload[1];
                let start = fu_header & 0x80 != 0;
                let end = fu_header & 0x40 != 0;
                let fu_nal_type = fu_header & 0x1F;

                if start {
                    // First fragment: reconstruct NAL header byte.
                    self.fu_buf.clear();
                    self.fu_buf.push(nri | fu_nal_type);
                    self.fu_buf.extend_from_slice(&rtp_payload[2..]);
                    self.fu_active = true;
                } else if self.fu_active {
                    // Continuation or end fragment.
                    self.fu_buf.extend_from_slice(&rtp_payload[2..]);
                } else {
                    // Out-of-order fragment without a start.
                    return None;
                }

                if end {
                    self.fu_active = false;
                    let nalu = std::mem::take(&mut self.fu_buf);
                    let keyframe = !nalu.is_empty() && (nalu[0] & 0x1F) == 5;
                    Some(DepackResult {
                        nalus: vec![nalu],
                        keyframe,
                        timestamp: header.timestamp,
                        marker: header.marker,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// HEVC NAL unit type constants for RTP depacketization (RFC 7798).
const HEVC_NAL_TYPE_AP: u8 = 48;
const HEVC_NAL_TYPE_FU: u8 = 49;

/// HEVC RTP depacketizer (RFC 7798).
///
/// Handles single NAL unit packets (types 0-47), Aggregation Packets
/// (AP, type 48), and Fragmentation Units (FU, type 49). HEVC NAL
/// headers are 2 bytes; NAL type is `(byte[0] >> 1) & 0x3F`.
#[derive(Debug, Default)]
pub struct HevcDepacketizer {
    fu_buf: Vec<u8>,
    fu_active: bool,
}

impl HevcDepacketizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one RTP packet and return any completed NAL units.
    pub fn depacketize(&mut self, rtp_payload: &[u8], header: &RtpHeader) -> Option<DepackResult> {
        if rtp_payload.len() < 2 {
            return None;
        }

        let nal_type = (rtp_payload[0] >> 1) & 0x3F;

        match nal_type {
            0..=47 => {
                // Single NAL unit packet (the entire RTP payload is one NAL).
                let keyframe = is_hevc_keyframe(nal_type);
                Some(DepackResult {
                    nalus: vec![rtp_payload.to_vec()],
                    keyframe,
                    timestamp: header.timestamp,
                    marker: header.marker,
                })
            }
            HEVC_NAL_TYPE_AP => {
                // Aggregation packet: skip 2-byte PayloadHdr, then
                // repeated [2-byte NALU size][NALU data].
                let mut nalus = Vec::new();
                let mut keyframe = false;
                let mut offset = 2;
                while offset + 2 <= rtp_payload.len() {
                    let nalu_len = u16::from_be_bytes([rtp_payload[offset], rtp_payload[offset + 1]]) as usize;
                    offset += 2;
                    if offset + nalu_len > rtp_payload.len() {
                        break;
                    }
                    let nalu = &rtp_payload[offset..offset + nalu_len];
                    if nalu.len() >= 2 {
                        let inner_type = (nalu[0] >> 1) & 0x3F;
                        if is_hevc_keyframe(inner_type) {
                            keyframe = true;
                        }
                    }
                    nalus.push(nalu.to_vec());
                    offset += nalu_len;
                }
                if nalus.is_empty() {
                    return None;
                }
                Some(DepackResult {
                    nalus,
                    keyframe,
                    timestamp: header.timestamp,
                    marker: header.marker,
                })
            }
            HEVC_NAL_TYPE_FU => {
                // Fragmentation unit: 2-byte PayloadHdr + 1-byte FU header + payload.
                if rtp_payload.len() < 3 {
                    return None;
                }
                let fu_header = rtp_payload[2];
                let start = fu_header & 0x80 != 0;
                let end = fu_header & 0x40 != 0;
                let fu_nal_type = fu_header & 0x3F;

                if start {
                    // Reconstruct the 2-byte HEVC NAL header from the
                    // PayloadHdr (bytes 0-1) with the NAL type replaced
                    // by the FU's original NAL type.
                    self.fu_buf.clear();
                    let byte0 = (rtp_payload[0] & 0x81) | (fu_nal_type << 1);
                    self.fu_buf.push(byte0);
                    self.fu_buf.push(rtp_payload[1]);
                    self.fu_buf.extend_from_slice(&rtp_payload[3..]);
                    self.fu_active = true;
                } else if self.fu_active {
                    self.fu_buf.extend_from_slice(&rtp_payload[3..]);
                } else {
                    return None;
                }

                if end {
                    self.fu_active = false;
                    let nalu = std::mem::take(&mut self.fu_buf);
                    let keyframe = nalu.len() >= 2 && is_hevc_keyframe((nalu[0] >> 1) & 0x3F);
                    Some(DepackResult {
                        nalus: vec![nalu],
                        keyframe,
                        timestamp: header.timestamp,
                        marker: header.marker,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Returns true for HEVC NAL types that indicate a random access point.
fn is_hevc_keyframe(nal_type: u8) -> bool {
    matches!(nal_type, 19..=21) // IDR_W_RADL, IDR_N_LP, CRA_NUT
}

/// Result of AAC RTP depacketization: one or more raw AAC Access Units.
#[derive(Debug)]
pub struct AacDepackResult {
    pub frames: Vec<Vec<u8>>,
    pub timestamp: u32,
    pub marker: bool,
}

/// AAC RTP depacketizer for RFC 3640 AAC-hbr mode.
///
/// In AAC-hbr mode each RTP packet carries:
/// - 2 bytes: AU-headers-length (in bits)
/// - N AU headers (each 16 bits: 13-bit AU-size + 3-bit AU-Index)
/// - Concatenated AU data
///
/// The AU-headers-length field divided by 16 gives the number of
/// AU headers, and each AU-size gives the byte count of the
/// corresponding Access Unit in the data section.
#[derive(Debug, Default)]
pub struct AacDepacketizer;

impl AacDepacketizer {
    pub fn new() -> Self {
        Self
    }

    /// Depacketize one RTP packet containing AAC-hbr data.
    pub fn depacketize(&self, rtp_payload: &[u8], header: &RtpHeader) -> Option<AacDepackResult> {
        if rtp_payload.len() < 2 {
            return None;
        }

        let au_headers_length_bits = u16::from_be_bytes([rtp_payload[0], rtp_payload[1]]) as usize;
        // Each AU header in AAC-hbr is 16 bits (sizelength=13 + indexlength=3).
        let au_header_count = au_headers_length_bits / 16;
        if au_header_count == 0 {
            return None;
        }

        let au_headers_bytes = au_header_count * 2;
        let headers_end = 2 + au_headers_bytes;
        if headers_end > rtp_payload.len() {
            return None;
        }

        // Parse AU sizes from the headers.
        let mut au_sizes = Vec::with_capacity(au_header_count);
        for i in 0..au_header_count {
            let off = 2 + i * 2;
            let h = u16::from_be_bytes([rtp_payload[off], rtp_payload[off + 1]]);
            let au_size = (h >> 3) as usize; // top 13 bits
            au_sizes.push(au_size);
        }

        // Extract AU data.
        let mut frames = Vec::with_capacity(au_sizes.len());
        let mut data_offset = headers_end;
        for size in &au_sizes {
            if data_offset + size > rtp_payload.len() {
                break;
            }
            frames.push(rtp_payload[data_offset..data_offset + size].to_vec());
            data_offset += size;
        }
        if frames.is_empty() {
            return None;
        }

        Some(AacDepackResult {
            frames,
            timestamp: header.timestamp,
            marker: header.marker,
        })
    }
}

/// Parse the hex-encoded AudioSpecificConfig from an RFC 3640 fmtp line.
/// Returns the decoded bytes, e.g. `[0x12, 0x10]` for `config=1210`.
pub fn parse_aac_config_from_fmtp(fmtp: &str) -> Option<Vec<u8>> {
    // fmtp line looks like: "97 streamtype=5;profile-level-id=1;mode=AAC-hbr;...;config=1210"
    // Skip the payload type number at the start.
    let params = fmtp.split_once(' ').map(|(_, rest)| rest).unwrap_or(fmtp);
    for param in params.split(';') {
        let param = param.trim();
        if let Some(hex) = param.strip_prefix("config=") {
            let hex = hex.trim();
            if hex.len() % 2 != 0 {
                return None;
            }
            let mut bytes = Vec::with_capacity(hex.len() / 2);
            for i in (0..hex.len()).step_by(2) {
                let byte = u8::from_str_radix(&hex[i..i + 2], 16).ok()?;
                bytes.push(byte);
            }
            return Some(bytes);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interleaved_frame_basic() {
        let mut data = vec![0x24, 0x00, 0x00, 0x04]; // $ channel=0 length=4
        data.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let (frame, consumed) = parse_interleaved_frame(&data).unwrap();
        assert_eq!(frame.channel, 0);
        assert_eq!(frame.payload, &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(consumed, 8);
    }

    #[test]
    fn parse_interleaved_frame_incomplete() {
        let data = vec![0x24, 0x00, 0x00, 0x10, 0x01, 0x02];
        assert!(parse_interleaved_frame(&data).is_none());
    }

    #[test]
    fn parse_interleaved_frame_not_dollar() {
        let data = b"PLAY rtsp://host RTSP/1.0\r\n";
        assert!(parse_interleaved_frame(data).is_none());
    }

    fn make_rtp_packet(pt: u8, seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut pkt = vec![0u8; 12 + payload.len()];
        pkt[0] = 0x80; // version=2
        pkt[1] = pt | if marker { 0x80 } else { 0x00 };
        pkt[2..4].copy_from_slice(&seq.to_be_bytes());
        pkt[4..8].copy_from_slice(&ts.to_be_bytes());
        pkt[8..12].copy_from_slice(&0x12345678u32.to_be_bytes());
        pkt[12..].copy_from_slice(payload);
        pkt
    }

    #[test]
    fn parse_rtp_header_basic() {
        let pkt = make_rtp_packet(96, 1234, 90000, true, &[0x65, 0xAA]);
        let hdr = parse_rtp_header(&pkt).unwrap();
        assert_eq!(hdr.payload_type, 96);
        assert_eq!(hdr.sequence, 1234);
        assert_eq!(hdr.timestamp, 90000);
        assert!(hdr.marker);
        assert_eq!(hdr.header_len, 12);
    }

    #[test]
    fn depack_single_nal() {
        // IDR slice (nal_type=5)
        let payload = vec![0x65, 0xAA, 0xBB, 0xCC];
        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 1,
            timestamp: 90000,
            ssrc: 0,
            marker: true,
            header_len: 12,
        };
        let mut depack = H264Depacketizer::new();
        let result = depack.depacketize(&payload, &hdr).unwrap();
        assert_eq!(result.nalus.len(), 1);
        assert!(result.keyframe);
        assert_eq!(result.nalus[0], payload);
    }

    #[test]
    fn depack_stap_a() {
        // STAP-A with two NALs: SPS (type 7) and PPS (type 8)
        let sps = vec![0x67, 0x42, 0x00, 0x1F];
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        let mut payload = vec![NAL_TYPE_STAP_A]; // STAP-A header
        payload.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        payload.extend_from_slice(&sps);
        payload.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        payload.extend_from_slice(&pps);

        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 1,
            timestamp: 90000,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        let mut depack = H264Depacketizer::new();
        let result = depack.depacketize(&payload, &hdr).unwrap();
        assert_eq!(result.nalus.len(), 2);
        assert_eq!(result.nalus[0], sps);
        assert_eq!(result.nalus[1], pps);
        assert!(!result.keyframe);
    }

    #[test]
    fn depack_fu_a_reassembly() {
        let mut depack = H264Depacketizer::new();
        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 1,
            timestamp: 90000,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        let hdr_end = RtpHeader {
            marker: true,
            sequence: 3,
            ..hdr
        };

        // FU-A start: FU indicator (NRI=0x60, type=28) + FU header (S=1, type=5/IDR)
        let start = vec![0x7C, 0x85, 0xAA, 0xBB];
        assert!(depack.depacketize(&start, &hdr).is_none());

        // FU-A middle
        let mid = vec![0x7C, 0x05, 0xCC, 0xDD];
        assert!(depack.depacketize(&mid, &hdr).is_none());

        // FU-A end
        let end = vec![0x7C, 0x45, 0xEE, 0xFF];
        let result = depack.depacketize(&end, &hdr_end).unwrap();
        assert_eq!(result.nalus.len(), 1);
        assert!(result.keyframe);
        // Reassembled: NRI(0x60) | type(5) = 0x65, then all fragment payloads
        assert_eq!(result.nalus[0], vec![0x65, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn depack_fu_a_mid_without_start_returns_none() {
        let mut depack = H264Depacketizer::new();
        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 5,
            timestamp: 90000,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        // FU-A continuation without prior start
        let mid = vec![0x7C, 0x05, 0xCC, 0xDD];
        assert!(depack.depacketize(&mid, &hdr).is_none());
    }

    // --- HEVC depacketizer tests ---

    /// Build a 2-byte HEVC NAL header: forbidden(1) | type(6) | layer_id(6) | tid(3).
    fn hevc_nal_header(nal_type: u8, tid: u8) -> [u8; 2] {
        [(nal_type << 1), tid]
    }

    #[test]
    fn hevc_depack_single_nal() {
        // IDR_W_RADL (type 19)
        let mut payload = hevc_nal_header(19, 1).to_vec();
        payload.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 1,
            timestamp: 90000,
            ssrc: 0,
            marker: true,
            header_len: 12,
        };
        let mut depack = HevcDepacketizer::new();
        let result = depack.depacketize(&payload, &hdr).unwrap();
        assert_eq!(result.nalus.len(), 1);
        assert!(result.keyframe);
        assert_eq!(result.nalus[0], payload);
    }

    #[test]
    fn hevc_depack_single_non_keyframe() {
        // TRAIL_R (type 1) -- not a keyframe
        let mut payload = hevc_nal_header(1, 1).to_vec();
        payload.extend_from_slice(&[0xDD, 0xEE]);
        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 1,
            timestamp: 90000,
            ssrc: 0,
            marker: true,
            header_len: 12,
        };
        let mut depack = HevcDepacketizer::new();
        let result = depack.depacketize(&payload, &hdr).unwrap();
        assert!(!result.keyframe);
    }

    #[test]
    fn hevc_depack_ap() {
        // AP with VPS (type 32) + SPS (type 33)
        let vps = {
            let mut v = hevc_nal_header(32, 1).to_vec();
            v.extend_from_slice(&[0x01, 0x02]);
            v
        };
        let sps = {
            let mut v = hevc_nal_header(33, 1).to_vec();
            v.extend_from_slice(&[0x03, 0x04]);
            v
        };
        let mut payload = hevc_nal_header(HEVC_NAL_TYPE_AP, 1).to_vec();
        payload.extend_from_slice(&(vps.len() as u16).to_be_bytes());
        payload.extend_from_slice(&vps);
        payload.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        payload.extend_from_slice(&sps);

        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 1,
            timestamp: 90000,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        let mut depack = HevcDepacketizer::new();
        let result = depack.depacketize(&payload, &hdr).unwrap();
        assert_eq!(result.nalus.len(), 2);
        assert_eq!(result.nalus[0], vps);
        assert_eq!(result.nalus[1], sps);
        assert!(!result.keyframe);
    }

    #[test]
    fn hevc_depack_fu_reassembly() {
        let mut depack = HevcDepacketizer::new();
        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 1,
            timestamp: 90000,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        let hdr_end = RtpHeader {
            marker: true,
            sequence: 3,
            ..hdr
        };

        // FU start: PayloadHdr (type=49, tid=1) + FU header (S=1, type=19/IDR_W_RADL) + data.
        let fu_payload_hdr = hevc_nal_header(HEVC_NAL_TYPE_FU, 1);
        let mut start_pkt = fu_payload_hdr.to_vec();
        start_pkt.push(0x80 | 19); // S=1, E=0, type=19
        start_pkt.extend_from_slice(&[0xAA, 0xBB]);
        assert!(depack.depacketize(&start_pkt, &hdr).is_none());

        // FU middle
        let mut mid_pkt = fu_payload_hdr.to_vec();
        mid_pkt.push(19); // S=0, E=0, type=19
        mid_pkt.extend_from_slice(&[0xCC, 0xDD]);
        assert!(depack.depacketize(&mid_pkt, &hdr).is_none());

        // FU end
        let mut end_pkt = fu_payload_hdr.to_vec();
        end_pkt.push(0x40 | 19); // S=0, E=1, type=19
        end_pkt.extend_from_slice(&[0xEE, 0xFF]);
        let result = depack.depacketize(&end_pkt, &hdr_end).unwrap();
        assert_eq!(result.nalus.len(), 1);
        assert!(result.keyframe);
        // Reassembled: 2-byte HEVC NAL header (type=19) + fragment payloads
        let reassembled = &result.nalus[0];
        assert_eq!((reassembled[0] >> 1) & 0x3F, 19);
        assert_eq!(&reassembled[2..], &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn hevc_depack_fu_mid_without_start_returns_none() {
        let mut depack = HevcDepacketizer::new();
        let hdr = RtpHeader {
            payload_type: 96,
            sequence: 5,
            timestamp: 90000,
            ssrc: 0,
            marker: false,
            header_len: 12,
        };
        let fu_payload_hdr = hevc_nal_header(HEVC_NAL_TYPE_FU, 1);
        let mut mid = fu_payload_hdr.to_vec();
        mid.push(19); // continuation, type=19
        mid.extend_from_slice(&[0xCC, 0xDD]);
        assert!(depack.depacketize(&mid, &hdr).is_none());
    }

    // --- AAC depacketizer tests ---

    #[test]
    fn aac_depack_single_frame() {
        // One AU: AU-headers-length = 16 bits (1 header), size=5, index=0.
        let au_size: u16 = 5;
        let au_header = au_size << 3; // 13-bit size + 3-bit index
        let mut payload = vec![];
        payload.extend_from_slice(&16u16.to_be_bytes()); // AU-headers-length in bits
        payload.extend_from_slice(&au_header.to_be_bytes());
        payload.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE]); // 5 bytes AU data

        let hdr = RtpHeader {
            payload_type: 97,
            sequence: 1,
            timestamp: 44100,
            ssrc: 0,
            marker: true,
            header_len: 12,
        };
        let depack = AacDepacketizer::new();
        let result = depack.depacketize(&payload, &hdr).unwrap();
        assert_eq!(result.frames.len(), 1);
        assert_eq!(result.frames[0], &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
    }

    #[test]
    fn aac_depack_multiple_frames() {
        // Two AUs in one packet.
        let au1_size: u16 = 3;
        let au2_size: u16 = 4;
        let au1_header = au1_size << 3;
        let au2_header = au2_size << 3;
        let mut payload = vec![];
        payload.extend_from_slice(&32u16.to_be_bytes()); // 2 headers * 16 bits each
        payload.extend_from_slice(&au1_header.to_be_bytes());
        payload.extend_from_slice(&au2_header.to_be_bytes());
        payload.extend_from_slice(&[0x11, 0x22, 0x33]); // AU 1
        payload.extend_from_slice(&[0x44, 0x55, 0x66, 0x77]); // AU 2

        let hdr = RtpHeader {
            payload_type: 97,
            sequence: 1,
            timestamp: 44100,
            ssrc: 0,
            marker: true,
            header_len: 12,
        };
        let depack = AacDepacketizer::new();
        let result = depack.depacketize(&payload, &hdr).unwrap();
        assert_eq!(result.frames.len(), 2);
        assert_eq!(result.frames[0], &[0x11, 0x22, 0x33]);
        assert_eq!(result.frames[1], &[0x44, 0x55, 0x66, 0x77]);
    }

    #[test]
    fn aac_depack_too_short() {
        let depack = AacDepacketizer::new();
        let hdr = RtpHeader {
            payload_type: 97,
            sequence: 1,
            timestamp: 44100,
            ssrc: 0,
            marker: true,
            header_len: 12,
        };
        assert!(depack.depacketize(&[0x00], &hdr).is_none());
        assert!(depack.depacketize(&[0x00, 0x00], &hdr).is_none()); // zero headers
    }

    #[test]
    fn parse_aac_config_from_fmtp_basic() {
        let fmtp = "97 streamtype=5;profile-level-id=1;mode=AAC-hbr;sizelength=13;indexlength=3;indexdeltalength=3;config=1210";
        let config = parse_aac_config_from_fmtp(fmtp).unwrap();
        assert_eq!(config, vec![0x12, 0x10]);
    }

    #[test]
    fn parse_aac_config_from_fmtp_missing() {
        let fmtp = "97 streamtype=5;profile-level-id=1;mode=AAC-hbr";
        assert!(parse_aac_config_from_fmtp(fmtp).is_none());
    }

    #[test]
    fn parse_aac_config_48khz_stereo() {
        // 48kHz stereo: object_type=2 (AAC-LC), freq_idx=3 (48000), channels=2
        // ASC: 0x11 0x90 -> (2<<3 | 3>>1) = 0x11, (3<<7 | 2<<3) = 0x90
        let fmtp = "97 mode=AAC-hbr;config=1190";
        let config = parse_aac_config_from_fmtp(fmtp).unwrap();
        assert_eq!(config, vec![0x11, 0x90]);
    }
}
