//! fMP4 demux helpers for the RTSP PLAY egress path.
//!
//! The broadcaster-native PLAY drain task receives `moof + mdat`
//! fragment payloads produced by `lvqr_cmaf::build_moof_mdat`. To
//! packetize those into RTP the drain needs the raw NAL / AAC bytes
//! back, which means walking past the `moof` box, locating the
//! `mdat` box body, and (for video) splitting the AVCC
//! length-prefixed contents into individual NAL units.
//!
//! This module is deliberately scoped to the two helpers the PLAY
//! path needs. It does not attempt to be a general fMP4 reader; LVQR
//! controls the producer side of the fragment payload so the layout
//! is known:
//!
//! * The top-level boxes are always `moof` followed by `mdat`.
//! * `mdat` bodies are AVCC length-prefixed for video (4-byte
//!   `u32` size) and raw access-unit bytes for audio (AAC / Opus).
//! * Fragment payloads are bounded (one access unit per fragment
//!   today), so allocating a `Vec<&[u8]>` for the NAL list is
//!   cheap.
//!
//! Functions here are pure over their inputs and do not allocate
//! beyond the returned containers. They are safe to call from the
//! broadcaster drain task without yielding.

/// Walk top-level fMP4 boxes in `payload` and return the body bytes
/// of the first `mdat` box found. Returns `None` when:
///
/// * the buffer is too short to hold even an 8-byte box header,
/// * the first ill-formed box size or a missing `mdat` entry,
/// * the declared box extends past the end of the buffer.
///
/// Supports the three wire forms of a `size` field:
/// * `size >= 8`: standard 32-bit size including the 8-byte header;
/// * `size == 1`: a 64-bit `largesize` follows the type, header is
///   then 16 bytes;
/// * `size == 0`: the box extends to the end of the buffer (only
///   valid on the last box, which is always `mdat` in our output).
pub fn extract_mdat_body(payload: &[u8]) -> Option<&[u8]> {
    let mut offset = 0;
    while offset + 8 <= payload.len() {
        let size_field = u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        let box_type = &payload[offset + 4..offset + 8];

        let (header_len, body_len) = match size_field {
            0 => {
                // Extends to end of buffer.
                (8usize, payload.len().checked_sub(offset + 8)?)
            }
            1 => {
                if offset + 16 > payload.len() {
                    return None;
                }
                let large = u64::from_be_bytes([
                    payload[offset + 8],
                    payload[offset + 9],
                    payload[offset + 10],
                    payload[offset + 11],
                    payload[offset + 12],
                    payload[offset + 13],
                    payload[offset + 14],
                    payload[offset + 15],
                ]);
                let large = large as usize;
                (16usize, large.checked_sub(16)?)
            }
            n => {
                let n = n as usize;
                // `size` counts the header; anything below 8 is malformed.
                if n < 8 {
                    return None;
                }
                (8usize, n - 8)
            }
        };

        let body_start = offset + header_len;
        let body_end = body_start.checked_add(body_len)?;
        if body_end > payload.len() {
            return None;
        }

        if box_type == b"mdat" {
            return Some(&payload[body_start..body_end]);
        }
        offset = body_end;
    }
    None
}

/// Split an AVCC byte buffer into individual NAL unit slices.
///
/// Each NAL is prefixed with a 4-byte big-endian `u32` length,
/// matching the encoding `lvqr_cmaf::build_moof_mdat` produces and
/// `lvqr_codec::write_avc_init_segment` declares via its
/// `lengthSizeMinusOne = 3` in the `avcC` box. Callers that need
/// HEVC also get the right answer: both codecs use the same
/// length-prefix convention in the `mdat` body.
///
/// Returns an empty `Vec` for an empty input. Malformed input
/// (a length header pointing past the buffer end) is treated as
/// end-of-stream and the walk terminates silently; every valid NAL
/// scanned so far is returned.
pub fn split_avcc_nalus(body: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset + 4 <= body.len() {
        let len = u32::from_be_bytes([body[offset], body[offset + 1], body[offset + 2], body[offset + 3]]) as usize;
        offset += 4;
        if len == 0 {
            continue;
        }
        let end = match offset.checked_add(len) {
            Some(e) => e,
            None => break,
        };
        if end > body.len() {
            break;
        }
        out.push(&body[offset..end]);
        offset = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use lvqr_cmaf::{RawSample, build_moof_mdat};

    fn build_box(box_type: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let size = (8 + body.len()) as u32;
        let mut v = Vec::with_capacity(8 + body.len());
        v.extend_from_slice(&size.to_be_bytes());
        v.extend_from_slice(box_type);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn extract_mdat_body_reads_after_moof() {
        let moof = build_box(b"moof", &[0x11, 0x22, 0x33, 0x44]);
        let mdat_body: Vec<u8> = (0..16).collect();
        let mdat = build_box(b"mdat", &mdat_body);
        let mut payload = moof.clone();
        payload.extend_from_slice(&mdat);

        let body = extract_mdat_body(&payload).expect("mdat extracted");
        assert_eq!(body, &mdat_body[..]);
    }

    #[test]
    fn extract_mdat_body_handles_empty_mdat() {
        let moof = build_box(b"moof", &[0xAA]);
        let mdat = build_box(b"mdat", &[]);
        let mut payload = moof;
        payload.extend_from_slice(&mdat);
        let body = extract_mdat_body(&payload).expect("empty mdat extracted");
        assert!(body.is_empty());
    }

    #[test]
    fn extract_mdat_body_returns_none_without_mdat() {
        let moof = build_box(b"moof", &[0xAA, 0xBB]);
        assert!(extract_mdat_body(&moof).is_none());
    }

    #[test]
    fn extract_mdat_body_returns_none_on_short_header() {
        assert!(extract_mdat_body(&[]).is_none());
        assert!(extract_mdat_body(&[0, 0, 0, 8]).is_none());
    }

    #[test]
    fn extract_mdat_body_rejects_size_less_than_header() {
        // size=4 < 8 header minimum
        let bad = [0u8, 0, 0, 4, b'm', b'd', b'a', b't'];
        assert!(extract_mdat_body(&bad).is_none());
    }

    #[test]
    fn extract_mdat_body_rejects_size_past_buffer() {
        let bad = [0u8, 0, 0, 64, b'm', b'd', b'a', b't', 0, 0, 0];
        assert!(extract_mdat_body(&bad).is_none());
    }

    #[test]
    fn extract_mdat_body_handles_size_zero_last_box() {
        // size=0 means "extends to end of buffer".
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(b"mdat");
        payload.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let body = extract_mdat_body(&payload).expect("size=0 mdat extracted");
        assert_eq!(body, &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn extract_mdat_body_handles_largesize() {
        // size=1 -> 64-bit largesize follows.
        let body = vec![0x01, 0x02, 0x03];
        let large_size: u64 = 16 + body.len() as u64;
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(b"mdat");
        payload.extend_from_slice(&large_size.to_be_bytes());
        payload.extend_from_slice(&body);
        let extracted = extract_mdat_body(&payload).expect("largesize mdat extracted");
        assert_eq!(extracted, &body[..]);
    }

    #[test]
    fn extract_mdat_body_round_trips_build_moof_mdat_single_sample() {
        // The real thing: LVQR's ingest calls build_moof_mdat to produce the
        // wire-ready fragment payload. The PLAY drain must recover the raw
        // sample bytes from it, byte-perfect.
        let sample_payload = Bytes::from_static(&[0, 0, 0, 5, 0x65, 0xAA, 0xBB, 0xCC, 0xDD]);
        let sample = RawSample {
            track_id: 1,
            dts: 0,
            cts_offset: 0,
            duration: 3000,
            payload: sample_payload.clone(),
            keyframe: true,
        };
        let fragment = build_moof_mdat(1, 1, 0, std::slice::from_ref(&sample));
        let body = extract_mdat_body(&fragment).expect("mdat body present");
        assert_eq!(body, sample_payload.as_ref(), "mdat body equals sample payload");
    }

    #[test]
    fn extract_mdat_body_round_trips_multi_sample_fragment() {
        // Two concatenated AVCC NAL units. The ingest path currently emits
        // one sample per fragment, but the extractor must still work if a
        // future coalescer packs multiple samples into a single mdat.
        let nalu1: Vec<u8> = {
            let n = vec![0x41, 0x11, 0x22];
            let mut v = (n.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(&n);
            v
        };
        let nalu2: Vec<u8> = {
            let n = vec![0x41, 0x33, 0x44, 0x55];
            let mut v = (n.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(&n);
            v
        };
        let mut combined = Vec::new();
        combined.extend_from_slice(&nalu1);
        combined.extend_from_slice(&nalu2);
        let sample = RawSample {
            track_id: 1,
            dts: 0,
            cts_offset: 0,
            duration: 3000,
            payload: Bytes::from(combined.clone()),
            keyframe: false,
        };
        let fragment = build_moof_mdat(1, 1, 0, std::slice::from_ref(&sample));
        let body = extract_mdat_body(&fragment).expect("mdat body present");
        assert_eq!(body, &combined[..]);
    }

    #[test]
    fn split_avcc_nalus_empty_input() {
        assert!(split_avcc_nalus(&[]).is_empty());
    }

    #[test]
    fn split_avcc_nalus_single_nal() {
        let nal = [0x65, 0xAA, 0xBB, 0xCC];
        let mut buf = (nal.len() as u32).to_be_bytes().to_vec();
        buf.extend_from_slice(&nal);
        let nalus = split_avcc_nalus(&buf);
        assert_eq!(nalus.len(), 1);
        assert_eq!(nalus[0], &nal[..]);
    }

    #[test]
    fn split_avcc_nalus_multiple_nals() {
        let n1 = [0x67, 0x42, 0x00, 0x1F];
        let n2 = [0x68, 0xCE, 0x38, 0x80];
        let n3 = [0x65, 0xAA, 0xBB, 0xCC];
        let mut buf = Vec::new();
        for nal in [&n1[..], &n2[..], &n3[..]] {
            buf.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            buf.extend_from_slice(nal);
        }
        let nalus = split_avcc_nalus(&buf);
        assert_eq!(nalus.len(), 3);
        assert_eq!(nalus[0], &n1[..]);
        assert_eq!(nalus[1], &n2[..]);
        assert_eq!(nalus[2], &n3[..]);
    }

    #[test]
    fn split_avcc_nalus_truncated_length_returns_partial() {
        // Second NAL's declared length points past the buffer: the walker
        // returns the first NAL and stops rather than panicking.
        let n1 = [0x41, 0x11];
        let mut buf = (n1.len() as u32).to_be_bytes().to_vec();
        buf.extend_from_slice(&n1);
        buf.extend_from_slice(&100u32.to_be_bytes()); // claims 100 bytes follow
        buf.extend_from_slice(&[0xAA]); // but only 1 byte is there
        let nalus = split_avcc_nalus(&buf);
        assert_eq!(nalus.len(), 1);
        assert_eq!(nalus[0], &n1[..]);
    }

    #[test]
    fn split_avcc_nalus_zero_length_entry_skipped() {
        // A length=0 entry advances the cursor by 4 bytes without emitting
        // a NAL. Real producers never emit zero-length entries, but the
        // walker should be tolerant.
        let mut buf = 0u32.to_be_bytes().to_vec();
        let nal = [0x41, 0x11];
        buf.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        buf.extend_from_slice(&nal);
        let nalus = split_avcc_nalus(&buf);
        assert_eq!(nalus.len(), 1);
        assert_eq!(nalus[0], &nal[..]);
    }
}
