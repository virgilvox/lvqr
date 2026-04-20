//! Minimal `moof + mdat` parser: extract the first `mdat`
//! payload bytes from a CMAF audio fragment.
//!
//! The audio fragment payload produced by `lvqr_ingest::remux::
//! fmp4::audio_segment` is `moof + mdat` where `mdat` contains a
//! single raw AAC-LC access unit (1024 samples). The whisper
//! agent only needs the AAC frame bytes, not the wrapping CMAF
//! boxes; this module strips the wrapper without pulling in a
//! full ISO BMFF parser.
//!
//! The parser is deliberately tiny and trust-no-one: it walks
//! BMFF boxes by `(size, type)` headers, skips anything that
//! is not `mdat`, returns `None` on any malformed length /
//! truncation. There is no recursion: callers only need the
//! top-level `mdat`, never nested boxes.
//!
//! Always available regardless of the `whisper` Cargo feature.

use bytes::Bytes;

/// Minimum BMFF box header size: 4-byte size + 4-byte type.
const BOX_HEADER_LEN: usize = 8;

/// Walk top-level BMFF boxes in `payload` and return the first
/// `mdat` box's payload bytes (the bytes between the box header
/// and the next box, exclusive of the header).
///
/// Returns `None` for malformed input: truncated header,
/// declared box size shorter than the header, declared box size
/// extending past the buffer, or no `mdat` box found.
///
/// Cheap: returns a sliced `Bytes` view, no copy.
pub fn extract_first_mdat(payload: &Bytes) -> Option<Bytes> {
    let mut cursor = 0usize;
    while cursor + BOX_HEADER_LEN <= payload.len() {
        let size = u32::from_be_bytes([
            payload[cursor],
            payload[cursor + 1],
            payload[cursor + 2],
            payload[cursor + 3],
        ]) as usize;
        let box_type = &payload[cursor + 4..cursor + BOX_HEADER_LEN];
        // Reject zero-size or sub-header sizes; reject
        // 64-bit large-size sentinel (size=1, only used for
        // boxes >4 GiB, which audio fragments never are).
        if size < BOX_HEADER_LEN || size > payload.len() - cursor {
            return None;
        }
        if box_type == b"mdat" {
            let start = cursor + BOX_HEADER_LEN;
            let end = cursor + size;
            return Some(payload.slice(start..end));
        }
        cursor += size;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{BufMut, BytesMut};

    fn write_box(buf: &mut BytesMut, ty: &[u8; 4], body: &[u8]) {
        let total = (BOX_HEADER_LEN + body.len()) as u32;
        buf.put_u32(total);
        buf.put_slice(ty);
        buf.put_slice(body);
    }

    fn moof_then_mdat(aac_frame: &[u8]) -> Bytes {
        let mut buf = BytesMut::new();
        // Skeletal moof. Real fragments carry tfhd + tfdt + trun
        // inside, but the parser only walks top-level boxes so
        // the body bytes are opaque.
        write_box(&mut buf, b"moof", b"\x00\x00\x00\x00fake-moof-body");
        write_box(&mut buf, b"mdat", aac_frame);
        buf.freeze()
    }

    #[test]
    fn extracts_mdat_skipping_moof() {
        let frame: &[u8] = &[0x21, 0x12, 0x34, 0xAB, 0xCD]; // pretend AAC bytes
        let payload = moof_then_mdat(frame);
        let got = extract_first_mdat(&payload).expect("mdat present");
        assert_eq!(got.as_ref(), frame);
    }

    #[test]
    fn returns_none_for_buffer_with_no_mdat() {
        let mut buf = BytesMut::new();
        write_box(&mut buf, b"moof", b"only-moof-here");
        assert!(extract_first_mdat(&buf.freeze()).is_none());
    }

    #[test]
    fn returns_none_for_truncated_header() {
        // Only 6 bytes; a header is 8.
        let payload = Bytes::from_static(&[0x00, 0x00, 0x00, 0x10, b'm', b'd']);
        assert!(extract_first_mdat(&payload).is_none());
    }

    #[test]
    fn returns_none_for_box_size_lying_about_length() {
        // Declared size 256, actual buffer 16. Must NOT slice past end.
        let mut buf = BytesMut::new();
        buf.put_u32(256);
        buf.put_slice(b"mdat");
        buf.put_slice(b"short!!");
        assert!(extract_first_mdat(&buf.freeze()).is_none());
    }

    #[test]
    fn returns_none_for_zero_size_box() {
        let mut buf = BytesMut::new();
        buf.put_u32(0);
        buf.put_slice(b"mdat");
        assert!(extract_first_mdat(&buf.freeze()).is_none());
    }

    #[test]
    fn handles_empty_buffer() {
        assert!(extract_first_mdat(&Bytes::new()).is_none());
    }

    #[test]
    fn handles_empty_mdat_payload() {
        let mut buf = BytesMut::new();
        write_box(&mut buf, b"mdat", &[]);
        let got = extract_first_mdat(&buf.freeze()).expect("mdat present");
        assert!(got.is_empty(), "empty mdat payload is legal");
    }
}
