//! ISO/IEC 23000-19 `styp` Segment Type box for CMAF chunk delivery.
//!
//! Per ISO/IEC 23000-19 §7.4, every CMAF Chunk SHALL begin with a `styp`
//! box, and a CMAF Segment composed of multiple chunks naturally
//! contains one `styp` per constituent chunk. The LL-HLS partial handler
//! and the DASH segment handler prepend [`cmaf_chunk_styp`] onto the
//! cached body so a chunk on the wire is a standalone CMAF deliverable
//! that mediastreamvalidator, MP4Box, hls.js, dash.js, and Shaka all
//! accept without complaint.
//!
//! Architectural boundary: `build_moof_mdat` (the coalescer-side
//! producer of chunk payloads) stays byte-identical, the fragment
//! broadcaster's `Fragment.payload` stays byte-identical, the archive
//! recorder writes raw `moof + mdat` to disk, and the `/playback/*`
//! DVR replay path serves those raw bytes verbatim. Only the HLS and
//! DASH HTTP serving layers stamp the `styp` prefix, which is exactly
//! the surface the spec calls out for chunk-format compatibility.

use bytes::{BufMut, Bytes, BytesMut};

/// Pre-baked 24-byte CMAF chunk `styp` box.
///
/// Kept as a `static [u8; 24]` so [`cmaf_chunk_styp`] can return a
/// zero-allocation `Bytes::from_static` view, and so callers that just
/// need to inspect the prefix bytes (tests, assertions) don't pay for
/// a `Bytes` indirection.
///
/// Layout:
///
/// | Offset | Field | Value |
/// |---|---|---|
/// | 0..4   | `size`              | 24 (`0x00000018`)         |
/// | 4..8   | `type`              | `styp`                     |
/// | 8..12  | `major_brand`       | `cmfc` (CMAF Chunk Format) |
/// | 12..16 | `minor_version`     | 0                          |
/// | 16..20 | `compatible_brand[0]` | `cmfc`                   |
/// | 20..24 | `compatible_brand[1]` | `iso6` (ISOBMFF 2015 ed.) |
///
/// Brand choice rationale: `cmfc` is the explicit CMAF chunk-format
/// brand from ISO/IEC 23000-19 §7.4 and is what mediastreamvalidator
/// keys off when classifying a chunk as CMAF-conformant. `iso6` is the
/// 2015 ISO Base Media File Format revision; including it as a
/// compatible brand keeps players that don't yet recognise `cmfc` (a
/// thin tail today, but free to support) on the happy path. The
/// 24-byte fixed layout covers both AVC and HEVC chunks without
/// branding-by-codec: per the spec the major brand identifies the
/// *segment* shape, not the codec, and `cmfc` is codec-agnostic.
pub static CMAF_CHUNK_STYP_BYTES: [u8; 24] = [
    // size = 24
    0x00, 0x00, 0x00, 0x18, //
    // type = 'styp'
    b's', b't', b'y', b'p', //
    // major_brand = 'cmfc'
    b'c', b'm', b'f', b'c', //
    // minor_version = 0
    0x00, 0x00, 0x00, 0x00, //
    // compatible_brand[0] = 'cmfc'
    b'c', b'm', b'f', b'c', //
    // compatible_brand[1] = 'iso6'
    b'i', b's', b'o', b'6', //
];

/// A zero-allocation `Bytes` view over the canonical CMAF chunk `styp`
/// box. Cheap to call repeatedly; rides on a static slice.
pub fn cmaf_chunk_styp() -> Bytes {
    Bytes::from_static(&CMAF_CHUNK_STYP_BYTES)
}

/// Concatenate the canonical CMAF chunk `styp` prefix with `body` into
/// a single contiguous `Bytes`. Used by the HLS partial cache and the
/// DASH segment cache to stamp a wire-ready CMAF chunk once at insert
/// time so every subsequent HTTP read serves the cached `Bytes`
/// directly.
///
/// This allocates 24 + `body.len()` bytes once per chunk push; the
/// previous shape (zero-allocation `Bytes::clone`) is gone, but chunk
/// pushes are not on the per-fragment hot path and the response-side
/// I/O cost dwarfs the prepend in any realistic workload.
pub fn prepend_cmaf_chunk_styp(body: &Bytes) -> Bytes {
    let mut out = BytesMut::with_capacity(CMAF_CHUNK_STYP_BYTES.len() + body.len());
    out.put_slice(&CMAF_CHUNK_STYP_BYTES);
    out.put_slice(body);
    out.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmaf_chunk_styp_is_24_bytes() {
        assert_eq!(cmaf_chunk_styp().len(), 24);
        assert_eq!(CMAF_CHUNK_STYP_BYTES.len(), 24);
    }

    #[test]
    fn cmaf_chunk_styp_size_field_equals_box_length() {
        let size = u32::from_be_bytes([
            CMAF_CHUNK_STYP_BYTES[0],
            CMAF_CHUNK_STYP_BYTES[1],
            CMAF_CHUNK_STYP_BYTES[2],
            CMAF_CHUNK_STYP_BYTES[3],
        ]);
        assert_eq!(size as usize, CMAF_CHUNK_STYP_BYTES.len());
    }

    #[test]
    fn cmaf_chunk_styp_type_is_styp_at_offset_4() {
        assert_eq!(&CMAF_CHUNK_STYP_BYTES[4..8], b"styp");
    }

    #[test]
    fn cmaf_chunk_styp_major_brand_is_cmfc() {
        assert_eq!(&CMAF_CHUNK_STYP_BYTES[8..12], b"cmfc");
    }

    #[test]
    fn cmaf_chunk_styp_minor_version_is_zero() {
        let minor = u32::from_be_bytes([
            CMAF_CHUNK_STYP_BYTES[12],
            CMAF_CHUNK_STYP_BYTES[13],
            CMAF_CHUNK_STYP_BYTES[14],
            CMAF_CHUNK_STYP_BYTES[15],
        ]);
        assert_eq!(minor, 0);
    }

    #[test]
    fn cmaf_chunk_styp_compatible_brands_are_cmfc_and_iso6() {
        assert_eq!(&CMAF_CHUNK_STYP_BYTES[16..20], b"cmfc");
        assert_eq!(&CMAF_CHUNK_STYP_BYTES[20..24], b"iso6");
    }

    #[test]
    fn cmaf_chunk_styp_returns_same_view_across_calls() {
        // Both views ride on the same `'static` slice; comparing
        // contents (rather than pointers) is what callers care about.
        let a = cmaf_chunk_styp();
        let b = cmaf_chunk_styp();
        assert_eq!(a, b);
    }

    #[test]
    fn prepend_cmaf_chunk_styp_preserves_body_after_prefix() {
        let body = Bytes::from_static(b"\x00\x00\x00\x10moof....data....");
        let stamped = prepend_cmaf_chunk_styp(&body);
        assert_eq!(stamped.len(), 24 + body.len());
        assert_eq!(&stamped[..24], &CMAF_CHUNK_STYP_BYTES[..]);
        assert_eq!(&stamped[24..], &body[..]);
    }

    #[test]
    fn prepend_cmaf_chunk_styp_handles_empty_body() {
        let stamped = prepend_cmaf_chunk_styp(&Bytes::new());
        assert_eq!(stamped.len(), 24);
        assert_eq!(&stamped[..], &CMAF_CHUNK_STYP_BYTES[..]);
    }
}
