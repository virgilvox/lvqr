//! Focused MPEG-TS demuxer for SRT and file-based ingest.
//!
//! Parses a byte stream of 188-byte TS packets, extracts PAT and
//! PMT tables to discover elementary stream PIDs and types, and
//! reassembles PES packets across TS packet boundaries. The
//! caller feeds arbitrary byte chunks via [`TsDemuxer::feed`];
//! the demuxer handles sync-byte recovery internally.
//!
//! Scope: PAT, single-program PMT, PES reassembly with PTS/DTS
//! extraction for H.264 (0x1B), HEVC (0x24), and AAC (0x0F).
//! Multi-program TS, SCTE-35, DVB descriptors, and PCR recovery
//! are out of scope for this first cut; the SRT ingest path
//! (Tier 2.8) only needs single-program demux from broadcast
//! encoders.

use std::collections::HashMap;

const TS_PACKET_SIZE: usize = 188;
const SYNC_BYTE: u8 = 0x47;
const PAT_PID: u16 = 0;

/// Elementary stream type codes from ISO/IEC 13818-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    H264,
    H265,
    Aac,
    Unknown(u8),
}

impl StreamType {
    fn from_byte(b: u8) -> Self {
        match b {
            0x1B => Self::H264,
            0x24 => Self::H265,
            0x0F | 0x11 => Self::Aac,
            other => Self::Unknown(other),
        }
    }
}

/// One reassembled PES packet yielded by [`TsDemuxer::feed`].
#[derive(Debug, Clone)]
pub struct PesPacket {
    pub pid: u16,
    pub stream_type: StreamType,
    /// Presentation timestamp in 90 kHz ticks. `None` when the
    /// PES header does not carry a PTS (uncommon for video/audio).
    pub pts: Option<u64>,
    /// Decode timestamp in 90 kHz ticks. `None` when PTS == DTS
    /// (most audio, non-B-frame video).
    pub dts: Option<u64>,
    /// Raw elementary stream bytes (Annex B for video, raw AAC
    /// frame for audio after ADTS stripping if present).
    pub payload: Vec<u8>,
}

/// Per-PID reassembly buffer.
#[derive(Debug)]
struct PesBuffer {
    stream_type: StreamType,
    buf: Vec<u8>,
    started: bool,
}

/// MPEG-TS demuxer with sync recovery and PES reassembly.
#[derive(Debug)]
pub struct TsDemuxer {
    /// Leftover bytes from the previous `feed` call that did not
    /// align to a 188-byte boundary.
    remainder: Vec<u8>,
    /// PMT PID discovered from the PAT.
    pmt_pid: Option<u16>,
    /// Elementary stream PID -> stream type, populated from PMT.
    streams: HashMap<u16, StreamType>,
    /// Per-PID PES reassembly buffers.
    pes_bufs: HashMap<u16, PesBuffer>,
}

impl Default for TsDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl TsDemuxer {
    pub fn new() -> Self {
        Self {
            remainder: Vec::new(),
            pmt_pid: None,
            streams: HashMap::new(),
            pes_bufs: HashMap::new(),
        }
    }

    /// Feed an arbitrary byte slice into the demuxer. Returns
    /// zero or more fully reassembled PES packets. The demuxer
    /// handles sync-byte recovery and cross-call buffering
    /// internally; callers may pass any chunk size.
    pub fn feed(&mut self, data: &[u8]) -> Vec<PesPacket> {
        self.remainder.extend_from_slice(data);
        let mut out = Vec::new();

        loop {
            // Find the next sync byte.
            let sync_pos = match self.remainder.iter().position(|&b| b == SYNC_BYTE) {
                Some(p) => p,
                None => {
                    self.remainder.clear();
                    break;
                }
            };
            if sync_pos > 0 {
                self.remainder.drain(..sync_pos);
            }
            if self.remainder.len() < TS_PACKET_SIZE {
                break;
            }

            let pkt: [u8; TS_PACKET_SIZE] = self.remainder[..TS_PACKET_SIZE]
                .try_into()
                .expect("slice is exactly 188 bytes");
            self.remainder.drain(..TS_PACKET_SIZE);
            self.process_packet(&pkt, &mut out);
        }

        out
    }

    fn process_packet(&mut self, pkt: &[u8; TS_PACKET_SIZE], out: &mut Vec<PesPacket>) {
        let pid = (((pkt[1] & 0x1F) as u16) << 8) | pkt[2] as u16;
        let pusi = pkt[1] & 0x40 != 0;
        let afc = (pkt[3] >> 4) & 0x03;

        let payload_offset = match afc {
            0b01 => 4,
            0b11 => {
                let af_len = pkt[4] as usize;
                5 + af_len
            }
            _ => return,
        };
        if payload_offset >= TS_PACKET_SIZE {
            return;
        }
        let payload = &pkt[payload_offset..];

        if pid == PAT_PID {
            self.parse_pat(payload, pusi);
        } else if Some(pid) == self.pmt_pid {
            self.parse_pmt(payload, pusi);
        } else if self.streams.contains_key(&pid) {
            self.push_pes(pid, payload, pusi, out);
        }
    }

    fn parse_pat(&mut self, payload: &[u8], pusi: bool) {
        let data = if pusi && !payload.is_empty() {
            let pointer = payload[0] as usize;
            if 1 + pointer >= payload.len() {
                return;
            }
            &payload[1 + pointer..]
        } else {
            payload
        };
        // table_id(1) + flags/length(2) + ts_id(2) + version(1) +
        // section/last(2) = 8 bytes header before the program loop.
        if data.len() < 12 {
            return;
        }
        let section_length = (((data[1] & 0x0F) as usize) << 8) | data[2] as usize;
        let table_end = 3 + section_length;
        if table_end > data.len() || section_length < 9 {
            return;
        }
        // Program loop starts at byte 8, ends 4 bytes before CRC.
        let loop_end = table_end.saturating_sub(4);
        let mut i = 8;
        while i + 4 <= loop_end {
            let prog_num = ((data[i] as u16) << 8) | data[i + 1] as u16;
            let map_pid = (((data[i + 2] & 0x1F) as u16) << 8) | data[i + 3] as u16;
            if prog_num != 0 {
                self.pmt_pid = Some(map_pid);
                break;
            }
            i += 4;
        }
    }

    fn parse_pmt(&mut self, payload: &[u8], pusi: bool) {
        let data = if pusi && !payload.is_empty() {
            let pointer = payload[0] as usize;
            if 1 + pointer >= payload.len() {
                return;
            }
            &payload[1 + pointer..]
        } else {
            payload
        };
        if data.len() < 16 {
            return;
        }
        let section_length = (((data[1] & 0x0F) as usize) << 8) | data[2] as usize;
        let table_end = 3 + section_length;
        if table_end > data.len() || section_length < 13 {
            return;
        }
        let prog_info_len = (((data[10] & 0x0F) as usize) << 8) | data[11] as usize;
        let mut i = 12 + prog_info_len;
        let loop_end = table_end.saturating_sub(4);
        self.streams.clear();
        while i + 5 <= loop_end {
            let st = data[i];
            let es_pid = (((data[i + 1] & 0x1F) as u16) << 8) | data[i + 2] as u16;
            let es_info_len = (((data[i + 3] & 0x0F) as usize) << 8) | data[i + 4] as usize;
            self.streams.insert(es_pid, StreamType::from_byte(st));
            i += 5 + es_info_len;
        }
    }

    fn push_pes(&mut self, pid: u16, payload: &[u8], pusi: bool, out: &mut Vec<PesPacket>) {
        let stream_type = *self.streams.get(&pid).unwrap_or(&StreamType::Unknown(0));

        if pusi {
            if let Some(buf) = self.pes_bufs.get_mut(&pid) {
                if buf.started && !buf.buf.is_empty() {
                    if let Some(pkt) = Self::finish_pes(pid, buf) {
                        out.push(pkt);
                    }
                }
            }
            let entry = self.pes_bufs.entry(pid).or_insert_with(|| PesBuffer {
                stream_type,
                buf: Vec::with_capacity(64 * 1024),
                started: false,
            });
            entry.buf.clear();
            entry.buf.extend_from_slice(payload);
            entry.started = true;
            entry.stream_type = stream_type;
        } else if let Some(buf) = self.pes_bufs.get_mut(&pid) {
            if buf.started {
                buf.extend(payload);
            }
        }
    }

    fn finish_pes(pid: u16, buf: &mut PesBuffer) -> Option<PesPacket> {
        let data = &buf.buf;
        if data.len() < 9 || data[0] != 0 || data[1] != 0 || data[2] != 1 {
            return None;
        }
        let pes_packet_length = ((data[4] as usize) << 8) | data[5] as usize;
        let header_data_len = data[8] as usize;
        let es_start = 9 + header_data_len;
        if es_start > data.len() {
            return None;
        }
        let flags = data[7];
        let pts_flag = flags & 0x80 != 0;
        let dts_flag = flags & 0x40 != 0;

        let pts = if pts_flag && header_data_len >= 5 {
            Some(parse_ts_timestamp(&data[9..14]))
        } else {
            None
        };
        let dts = if dts_flag && header_data_len >= 10 {
            Some(parse_ts_timestamp(&data[14..19]))
        } else {
            None
        };

        // When PES_packet_length is non-zero, it specifies the
        // exact number of bytes after the 6-byte PES header
        // prefix. Use it to trim trailing TS padding. When zero
        // (unbounded, common for video), take everything.
        let es_end = if pes_packet_length > 0 {
            (6 + pes_packet_length).min(data.len())
        } else {
            data.len()
        };
        let payload = data[es_start..es_end].to_vec();
        if payload.is_empty() {
            return None;
        }

        Some(PesPacket {
            pid,
            stream_type: buf.stream_type,
            pts,
            dts,
            payload,
        })
    }
}

impl PesBuffer {
    fn extend(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }
}

/// Parse a 33-bit MPEG-TS timestamp from the 5-byte PTS/DTS
/// encoding with marker bits. The layout is:
/// `0bXXXa_bbbY cccc_cccc YYYY_dddd eeee_eeeY`
/// where a-e are the 33 timestamp bits and X/Y are markers.
fn parse_ts_timestamp(b: &[u8]) -> u64 {
    let a = ((b[0] as u64 >> 1) & 0x07) << 30;
    let bc = ((b[1] as u64) << 7 | (b[2] as u64 >> 1)) << 15;
    let de = (b[3] as u64) << 7 | (b[4] as u64 >> 1);
    a | bc | de
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ts_packet(pid: u16, pusi: bool, payload: &[u8]) -> [u8; 188] {
        let mut pkt = [0xFFu8; 188];
        pkt[0] = SYNC_BYTE;
        pkt[1] = if pusi { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F);
        pkt[2] = pid as u8;
        pkt[3] = 0x10; // payload only, CC=0
        let copy_len = payload.len().min(184);
        pkt[4..4 + copy_len].copy_from_slice(&payload[..copy_len]);
        // Stuff remaining bytes with 0xFF (already done by init).
        pkt
    }

    fn minimal_pat(pmt_pid: u16) -> Vec<u8> {
        // pointer_field(1) + table_id(1) + flags/length(2) +
        // ts_id(2) + version(1) + section(1) + last_section(1)
        // + program_number(2) + reserved/pmt_pid(2) + CRC(4)
        let mut data = vec![
            0x00, // pointer field
            0x00, // table_id = PAT
            0xB0, 0x0D, // section_syntax + length = 13
            0x00, 0x01, // transport_stream_id
            0xC1, // version=0, current
            0x00, 0x00, // section 0 of 0
            0x00, 0x01, // program_number = 1
        ];
        data.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
        data.push(pmt_pid as u8);
        data.extend_from_slice(&[0x00; 4]); // CRC placeholder
        data
    }

    fn minimal_pmt(video_pid: u16, audio_pid: u16) -> Vec<u8> {
        // pointer_field + table_id + flags/length + program_number +
        // version + section + pcr_pid + program_info_length +
        // stream entries + CRC
        let mut data = vec![
            0x00, // pointer field
            0x02, // table_id = PMT
            0xB0, 0x17, // section_syntax + length = 23
            0x00, 0x01, // program_number = 1
            0xC1, // version=0, current
            0x00, 0x00, // section 0 of 0
            0xE1, 0x00, // PCR_PID = 0x100
            0xF0, 0x00, // program_info_length = 0
        ];
        // Video stream entry: H.264
        data.push(0x1B); // stream_type
        data.push(0xE0 | ((video_pid >> 8) as u8 & 0x1F));
        data.push(video_pid as u8);
        data.push(0xF0);
        data.push(0x00); // ES_info_length = 0
        // Audio stream entry: AAC
        data.push(0x0F); // stream_type
        data.push(0xE0 | ((audio_pid >> 8) as u8 & 0x1F));
        data.push(audio_pid as u8);
        data.push(0xF0);
        data.push(0x00); // ES_info_length = 0
        data.extend_from_slice(&[0x00; 4]); // CRC placeholder
        data
    }

    fn minimal_pes(pts_90k: u64, es_payload: &[u8]) -> Vec<u8> {
        // PES_packet_length = header (3 bytes: flags + PTS flag +
        // header_data_length) + PTS (5 bytes) + ES payload.
        let pes_len = (3 + 5 + es_payload.len()) as u16;
        let mut data = vec![
            0x00,
            0x00,
            0x01, // start code
            0xE0, // stream_id (video)
            (pes_len >> 8) as u8,
            pes_len as u8,
            0x80, // marker bits
            0x80, // PTS flag set, no DTS
            0x05, // header_data_length = 5
        ];
        // Encode PTS into 5 bytes with marker bits.
        let pts = pts_90k & 0x1_FFFF_FFFF;
        data.push(0x21 | ((pts >> 29) as u8 & 0x0E));
        data.push((pts >> 22) as u8);
        data.push(0x01 | ((pts >> 14) as u8 & 0xFE));
        data.push((pts >> 7) as u8);
        data.push(0x01 | ((pts << 1) as u8 & 0xFE));
        data.extend_from_slice(es_payload);
        data
    }

    #[test]
    fn demux_discovers_streams_and_yields_pes() {
        let mut demux = TsDemuxer::new();
        let video_pid = 0x100;
        let audio_pid = 0x101;
        let pmt_pid = 0x1000;

        // Feed PAT.
        let pat = make_ts_packet(PAT_PID, true, &minimal_pat(pmt_pid));
        assert!(demux.feed(&pat).is_empty());
        assert_eq!(demux.pmt_pid, Some(pmt_pid));

        // Feed PMT.
        let pmt = make_ts_packet(pmt_pid, true, &minimal_pmt(video_pid, audio_pid));
        assert!(demux.feed(&pmt).is_empty());
        assert_eq!(demux.streams.len(), 2);
        assert_eq!(demux.streams[&video_pid], StreamType::H264);
        assert_eq!(demux.streams[&audio_pid], StreamType::Aac);

        // Feed a PES packet for video.
        let pes = minimal_pes(90_000, b"nalunalunalu");
        let pkt = make_ts_packet(video_pid, true, &pes);
        // PES is not yielded until the next PUSI on the same PID.
        assert!(demux.feed(&pkt).is_empty());

        // Start a new PES on the same PID to flush the previous one.
        let pes2 = minimal_pes(180_000, b"nalu2");
        let pkt2 = make_ts_packet(video_pid, true, &pes2);
        let packets = demux.feed(&pkt2);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].pid, video_pid);
        assert_eq!(packets[0].stream_type, StreamType::H264);
        assert_eq!(packets[0].pts, Some(90_000));
        assert_eq!(packets[0].payload, b"nalunalunalu");
    }

    #[test]
    fn sync_recovery_skips_garbage() {
        let mut demux = TsDemuxer::new();
        let pmt_pid = 0x1000;

        // Feed garbage followed by a valid PAT packet.
        let mut data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        data.extend_from_slice(&make_ts_packet(PAT_PID, true, &minimal_pat(pmt_pid)));
        demux.feed(&data);
        assert_eq!(demux.pmt_pid, Some(pmt_pid));
    }

    #[test]
    fn cross_call_buffering_handles_partial_packets() {
        let mut demux = TsDemuxer::new();
        let pmt_pid = 0x1000;
        let full = make_ts_packet(PAT_PID, true, &minimal_pat(pmt_pid));

        // Feed first half.
        demux.feed(&full[..100]);
        assert_eq!(demux.pmt_pid, None);

        // Feed second half.
        demux.feed(&full[100..]);
        assert_eq!(demux.pmt_pid, Some(pmt_pid));
    }

    #[test]
    fn parse_ts_timestamp_round_trips() {
        let pts: u64 = 123_456_789;
        let mut buf = [0u8; 5];
        buf[0] = 0x21 | ((pts >> 29) as u8 & 0x0E);
        buf[1] = (pts >> 22) as u8;
        buf[2] = 0x01 | ((pts >> 14) as u8 & 0xFE);
        buf[3] = (pts >> 7) as u8;
        buf[4] = 0x01 | ((pts << 1) as u8 & 0xFE);
        assert_eq!(parse_ts_timestamp(&buf), pts);
    }
}
