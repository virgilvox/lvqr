//! Annex B ↔ AVCC conversion for inbound H.264 frames.
//!
//! `str0m` emits one `Event::MediaData` per fully depacketized
//! access unit, with the payload byte layout following Annex B
//! (NAL units separated by `0x00 0x00 0x00 0x01` or `0x00 0x00
//! 0x01` start codes). Every downstream consumer in LVQR
//! (`lvqr-cmaf::RawSample`, the MoQ track sink, the LL-HLS
//! segmenter, the disk recorder) speaks AVCC — a sequence of
//! `[u32-be length][nal body]` tuples with no start codes.
//!
//! This module is the inverse of the AVCC → Annex B converter at
//! `crates/lvqr-whep/src/str0m_backend.rs:430`. Both are
//! load-bearing: WebRTC speaks Annex B, LVQR's Unified Fragment
//! Model speaks AVCC, and every crossing between the two must run
//! through an explicit conversion. Feeding a raw Annex B buffer
//! into `build_moof_mdat` would produce a fragment whose `moof`
//! sample size does not match its `mdat` body and no decoder would
//! play it.
//!
//! Malformed inputs (buffers with no start code, truncated NALs,
//! zero-length entries) are handled by skipping the unparseable
//! entry and continuing. The converter never panics — the
//! proptest slot in `tests/proptest_depack.rs` enforces this
//! property against arbitrary attacker-shaped input.

/// AVC NAL unit type: IDR slice (keyframe).
pub const AVC_NAL_TYPE_IDR: u8 = 5;
/// AVC NAL unit type: sequence parameter set.
pub const AVC_NAL_TYPE_SPS: u8 = 7;
/// AVC NAL unit type: picture parameter set.
pub const AVC_NAL_TYPE_PPS: u8 = 8;

/// HEVC NAL unit type: video parameter set.
pub const HEVC_NAL_TYPE_VPS: u8 = 32;
/// HEVC NAL unit type: sequence parameter set.
pub const HEVC_NAL_TYPE_SPS: u8 = 33;
/// HEVC NAL unit type: picture parameter set.
pub const HEVC_NAL_TYPE_PPS: u8 = 34;

/// Extract the HEVC NAL unit type (`nal_unit_type`) from a NAL
/// body. Returns `None` if the slice is shorter than the 2-byte
/// HEVC NAL header.
///
/// HEVC NAL header layout (ITU-T H.265 7.3.1.2):
///   `forbidden_zero_bit(1) nal_unit_type(6) nuh_layer_id(6)
///    nuh_temporal_id_plus1(3)`
///
/// The type lives in bits 6..=1 of the first byte.
pub fn hevc_nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| (b >> 1) & 0x3f)
}

/// Walk an Annex B byte buffer and return the NAL unit bodies
/// (without their start codes).
///
/// The walker recognises both the 3-byte (`00 00 01`) and 4-byte
/// (`00 00 00 01`) start-code forms and tolerates interleaved
/// emulation-prevention bytes inside NAL bodies (those are the
/// decoder's concern, not this function's). A buffer with no
/// start code at all returns an empty vec — the caller should
/// treat that as "no NALs in this frame" rather than "the whole
/// buffer is one NAL", because Annex B by definition requires at
/// least one start code.
pub fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let starts = find_start_codes(data);
    if starts.is_empty() {
        return out;
    }
    for window in starts.windows(2) {
        let begin = window[0].end;
        let end = window[1].start;
        if begin < end {
            out.push(&data[begin..end]);
        }
    }
    // Trailing NAL after the last start code.
    if let Some(last) = starts.last() {
        let begin = last.end;
        if begin < data.len() {
            out.push(&data[begin..data.len()]);
        }
    }
    out
}

/// Convert an Annex B byte buffer into an AVCC length-prefixed NAL
/// sequence suitable for `lvqr_cmaf::RawSample::payload`.
///
/// Zero-length NALs are skipped silently; the output buffer only
/// carries NALs the walker actually recovered. Returns an empty
/// vec when the input contains no parseable NAL units.
pub fn annex_b_to_avcc(annex_b: &[u8]) -> Vec<u8> {
    let nals = split_annex_b(annex_b);
    if nals.is_empty() {
        return Vec::new();
    }
    let total: usize = nals.iter().map(|n| n.len() + 4).sum();
    let mut out = Vec::with_capacity(total);
    for nal in nals {
        if nal.is_empty() {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// One Annex B start-code run in a byte buffer, recorded as the
/// `[start, end)` byte indices so the walker can slice NAL bodies
/// as `data[end_of_prev_start..start_of_next_start]` without
/// having to juggle 3-byte vs 4-byte forms at the consumption
/// site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StartCodeRun {
    start: usize,
    end: usize,
}

fn find_start_codes(data: &[u8]) -> Vec<StartCodeRun> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            out.push(StartCodeRun { start: i, end: i + 3 });
            i += 3;
            continue;
        }
        if i + 4 <= data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
            out.push(StartCodeRun { start: i, end: i + 4 });
            i += 4;
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_annex_b(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn splits_single_nal() {
        let nal: &[u8] = &[0x65, 0xAA, 0xBB];
        let buf = build_annex_b(&[nal]);
        let out = split_annex_b(&buf);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], nal);
    }

    #[test]
    fn splits_three_nals() {
        let sps: &[u8] = &[0x67, 0x42, 0xC0, 0x1E];
        let pps: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];
        let idr: &[u8] = &[0x65, 0x88, 0x84, 0x40];
        let buf = build_annex_b(&[sps, pps, idr]);
        let out = split_annex_b(&buf);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], sps);
        assert_eq!(out[1], pps);
        assert_eq!(out[2], idr);
    }

    #[test]
    fn accepts_three_byte_start_code() {
        // Mix of 4-byte and 3-byte start codes — both must parse.
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42]);
        buf.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xCE]);
        let out = split_annex_b(&buf);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], &[0x67, 0x42]);
        assert_eq!(out[1], &[0x68, 0xCE]);
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(split_annex_b(&[]).is_empty());
        assert!(annex_b_to_avcc(&[]).is_empty());
    }

    #[test]
    fn input_without_start_code_returns_empty() {
        let garbage: &[u8] = &[0xAA, 0xBB, 0xCC, 0xDD];
        assert!(split_annex_b(garbage).is_empty());
        assert!(annex_b_to_avcc(garbage).is_empty());
    }

    #[test]
    fn avcc_round_trip_matches_input_nals() {
        let sps: &[u8] = &[0x67, 0x42, 0xC0, 0x1E];
        let pps: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];
        let buf = build_annex_b(&[sps, pps]);
        let avcc = annex_b_to_avcc(&buf);

        // Expect: [4-byte len][sps][4-byte len][pps]
        let mut expected = Vec::new();
        expected.extend_from_slice(&(sps.len() as u32).to_be_bytes());
        expected.extend_from_slice(sps);
        expected.extend_from_slice(&(pps.len() as u32).to_be_bytes());
        expected.extend_from_slice(pps);
        assert_eq!(avcc, expected);
    }

    #[test]
    fn walker_tolerates_trailing_garbage() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x11, 0x22]);
        // Trailing bytes without a start code are consumed as the last NAL body.
        let out = split_annex_b(&buf);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], &[0x65, 0x11, 0x22]);
    }

    #[test]
    fn walker_never_panics_on_short_input() {
        for len in 0..8 {
            let buf = vec![0u8; len];
            let _ = split_annex_b(&buf);
            let _ = annex_b_to_avcc(&buf);
        }
    }
}
