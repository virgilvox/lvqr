//! Extract the AAC `AudioSpecificConfig` (ASC) bytes from a
//! CMAF audio init segment.
//!
//! The init segment produced by `lvqr_ingest::remux::fmp4::
//! audio_init_segment` carries the ASC inside the chain:
//!
//! ```text
//! moov / trak / mdia / minf / stbl / stsd / mp4a / esds
//!   ESDescriptor (0x03)
//!     DecoderConfigDescriptor (0x04)
//!       DecoderSpecificInfo (0x05)  <-- the ASC bytes
//! ```
//!
//! For the whisper agent we only need the ASC payload (so
//! symphonia's AAC decoder can configure itself with the
//! correct profile / sample rate / channel layout). This
//! module walks down the box chain, then walks the
//! MPEG-4 descriptor list inside `esds` to find the
//! DecoderSpecificInfo (descriptor tag 0x05) and returns its
//! payload.
//!
//! The descriptor parser accepts both single-byte and
//! variable-length-encoded (VLE) descriptor lengths per
//! ISO/IEC 14496-1 §8.3.3 -- the LVQR fmp4 writer emits VLE
//! when the body is >127 bytes (e.g. xHE-AAC, HE-AAC SBR/PS
//! with explicit-frequency escape).
//!
//! Always available regardless of the `whisper` Cargo feature.

use bytes::Bytes;

const BOX_HEADER_LEN: usize = 8;

/// Recursively walk top-level BMFF boxes inside `payload`,
/// descending into `target_path` in order. Returns the body
/// bytes of the leaf box.
///
/// Each path component is a 4-byte FourCC. The walker descends
/// only the first matching child at each level (no
/// branching) -- enough for the ASC chain because a CMAF audio
/// init has exactly one mp4a entry.
fn descend<'a>(mut payload: &'a [u8], target_path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    for fourcc in target_path {
        let mut cursor = 0usize;
        let mut found: Option<(&'a [u8], usize)> = None;
        while cursor + BOX_HEADER_LEN <= payload.len() {
            let size = u32::from_be_bytes([
                payload[cursor],
                payload[cursor + 1],
                payload[cursor + 2],
                payload[cursor + 3],
            ]) as usize;
            let box_type = &payload[cursor + 4..cursor + BOX_HEADER_LEN];
            if size < BOX_HEADER_LEN || size > payload.len() - cursor {
                return None;
            }
            if box_type == fourcc.as_slice() {
                let body_start = cursor + BOX_HEADER_LEN;
                let body_end = cursor + size;
                found = Some((&payload[body_start..body_end], cursor));
                break;
            }
            cursor += size;
        }
        let (body, _) = found?;
        payload = body;
    }
    Some(payload)
}

/// Read an MPEG-4 variable-length-encoded descriptor length per
/// ISO/IEC 14496-1 §8.3.3. Returns `(length, bytes_consumed)`.
///
/// Each byte contributes 7 bits of length; the high bit
/// indicates "more bytes follow". Bounded to 4 bytes max
/// (28-bit length, well past anything an ASC needs).
fn read_descriptor_length(buf: &[u8]) -> Option<(usize, usize)> {
    let mut length: usize = 0;
    for (idx, byte) in buf.iter().take(4).enumerate() {
        length = (length << 7) | (*byte as usize & 0x7F);
        if *byte & 0x80 == 0 {
            return Some((length, idx + 1));
        }
    }
    None
}

/// Walk the MPEG-4 descriptor list in `body` and return the
/// payload bytes of the first descriptor whose tag matches
/// `target_tag`. Used to descend ESDescriptor (0x03) ->
/// DecoderConfigDescriptor (0x04) -> DecoderSpecificInfo (0x05).
fn find_descriptor(mut body: &[u8], target_tag: u8, skip_prefix: usize) -> Option<&[u8]> {
    if body.len() < skip_prefix {
        return None;
    }
    body = &body[skip_prefix..];
    while !body.is_empty() {
        let tag = body[0];
        let (len, consumed) = read_descriptor_length(&body[1..])?;
        let payload_start = 1 + consumed;
        let payload_end = payload_start + len;
        if payload_end > body.len() {
            return None;
        }
        if tag == target_tag {
            return Some(&body[payload_start..payload_end]);
        }
        // The descriptors we care about are siblings under their
        // parent body, so just continue past this one.
        body = &body[payload_end..];
    }
    None
}

/// Extract the AudioSpecificConfig (ASC) payload bytes from a
/// CMAF audio init segment.
///
/// Returns `None` if the segment is not an audio init or the
/// ASC chain is malformed / missing.
pub fn extract_asc(init_segment: &Bytes) -> Option<Bytes> {
    let esds_body = descend(
        init_segment.as_ref(),
        &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"],
    )?;
    // stsd has a 4-byte version+flags + 4-byte entry_count
    // header before its sample-description entries.
    if esds_body.len() < 8 {
        return None;
    }
    let entries = &esds_body[8..];
    // Walk top-level boxes inside stsd; we want the first mp4a.
    let mut cursor = 0usize;
    let mut mp4a_body: Option<&[u8]> = None;
    while cursor + BOX_HEADER_LEN <= entries.len() {
        let size = u32::from_be_bytes([
            entries[cursor],
            entries[cursor + 1],
            entries[cursor + 2],
            entries[cursor + 3],
        ]) as usize;
        let ty = &entries[cursor + 4..cursor + BOX_HEADER_LEN];
        if size < BOX_HEADER_LEN || size > entries.len() - cursor {
            return None;
        }
        if ty == b"mp4a" {
            mp4a_body = Some(&entries[cursor + BOX_HEADER_LEN..cursor + size]);
            break;
        }
        cursor += size;
    }
    let mp4a_body = mp4a_body?;
    // mp4a sample entry has a fixed 28-byte preamble before
    // the esds child box (6 reserved + 2 data_reference_index +
    // 8 reserved + 2 channel_count + 2 sample_size + 2 pre_defined +
    // 2 reserved + 4 sample_rate = 28).
    if mp4a_body.len() < 28 {
        return None;
    }
    let after_preamble = &mp4a_body[28..];
    // Find the esds box.
    let mut cursor = 0usize;
    while cursor + BOX_HEADER_LEN <= after_preamble.len() {
        let size = u32::from_be_bytes([
            after_preamble[cursor],
            after_preamble[cursor + 1],
            after_preamble[cursor + 2],
            after_preamble[cursor + 3],
        ]) as usize;
        let ty = &after_preamble[cursor + 4..cursor + BOX_HEADER_LEN];
        if size < BOX_HEADER_LEN || size > after_preamble.len() - cursor {
            return None;
        }
        if ty == b"esds" {
            // esds is a FullBox: skip 4-byte version+flags before
            // the descriptor body.
            let body_start = cursor + BOX_HEADER_LEN + 4;
            let body_end = cursor + size;
            if body_start > body_end {
                return None;
            }
            let esds_descriptors = &after_preamble[body_start..body_end];
            // ESDescriptor (tag 0x03). Body has a 3-byte preamble
            // (ES_ID 2 bytes + flags 1 byte) before the inner
            // descriptors.
            let es_body = find_descriptor(esds_descriptors, 0x03, 0)?;
            // DecoderConfigDescriptor (tag 0x04) inside ESDescriptor.
            // Its body has a 13-byte preamble (objectTypeIndication +
            // streamType+upStream+reserved + bufferSizeDB(3) +
            // maxBitrate(4) + avgBitrate(4) = 13) before the inner
            // descriptors.
            let dcd_body = find_descriptor(es_body, 0x04, 3)?;
            // DecoderSpecificInfo (tag 0x05). Its body IS the ASC.
            let asc = find_descriptor(dcd_body, 0x05, 13)?;
            return Some(Bytes::copy_from_slice(asc));
        }
        cursor += size;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_length_short_form() {
        // Single byte: length = 0x42 (high bit clear).
        let (len, used) = read_descriptor_length(&[0x42]).unwrap();
        assert_eq!(len, 0x42);
        assert_eq!(used, 1);
    }

    #[test]
    fn descriptor_length_vle_two_bytes() {
        // 0x80 0x42: continuation byte (0x00 << 7) | 0x42.
        let (len, used) = read_descriptor_length(&[0x80, 0x42]).unwrap();
        assert_eq!(len, 0x42);
        assert_eq!(used, 2);
    }

    #[test]
    fn descriptor_length_vle_three_bytes() {
        // 0x80 0x80 0x42 = (0x00 << 14) | (0x00 << 7) | 0x42.
        let (len, used) = read_descriptor_length(&[0x80, 0x80, 0x42]).unwrap();
        assert_eq!(len, 0x42);
        assert_eq!(used, 3);
    }

    #[test]
    fn descriptor_length_truncated_returns_none() {
        // Continuation byte but no terminator byte after.
        assert!(read_descriptor_length(&[0x80, 0x80, 0x80, 0x80]).is_none());
    }

    #[test]
    fn extract_asc_from_real_init_segment() {
        // Build a minimal init segment using the lvqr-ingest
        // writer would be cleanest, but that pulls a hard dep
        // on lvqr-ingest into this crate's dep graph; instead
        // we synthesize the smallest valid moov/trak/.../esds
        // chain by hand. The ASC bytes we expect back are
        // [0x12, 0x10] (AAC-LC, 44100 Hz, stereo).
        let asc: &[u8] = &[0x12, 0x10];

        // Build innermost-out.
        // DecoderSpecificInfo (tag 0x05).
        let mut dsi = vec![0x05, asc.len() as u8];
        dsi.extend_from_slice(asc);
        // DecoderConfigDescriptor (tag 0x04). Body: 13-byte preamble + dsi.
        let mut dcd_body = Vec::new();
        dcd_body.extend_from_slice(&[
            0x40, 0x15, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        dcd_body.extend_from_slice(&dsi);
        let mut dcd = vec![0x04, dcd_body.len() as u8];
        dcd.extend_from_slice(&dcd_body);
        // ESDescriptor (tag 0x03). Body: 3-byte preamble + dcd.
        let mut es_body = vec![0x00, 0x01, 0x00];
        es_body.extend_from_slice(&dcd);
        let mut es = vec![0x03, es_body.len() as u8];
        es.extend_from_slice(&es_body);
        // esds FullBox: 4-byte version+flags then descriptors.
        let mut esds_body = vec![0u8; 4];
        esds_body.extend_from_slice(&es);
        let esds = make_box(b"esds", &esds_body);

        // mp4a sample entry: 28-byte preamble + esds.
        let mut mp4a_body = vec![0u8; 28];
        // Set channel_count + sample_size + sample_rate to plausible
        // values so the asc-extractor's >=28 length check is the
        // operative gate.
        mp4a_body[16..18].copy_from_slice(&2u16.to_be_bytes()); // channels
        mp4a_body[18..20].copy_from_slice(&16u16.to_be_bytes()); // sample_size
        mp4a_body[24..28].copy_from_slice(&(44_100u32 << 16).to_be_bytes());
        mp4a_body.extend_from_slice(&esds);
        let mp4a = make_box(b"mp4a", &mp4a_body);

        // stsd FullBox: 4 version+flags + 4 entry_count + entry.
        let mut stsd_body = vec![0u8; 8];
        stsd_body[7] = 1;
        stsd_body.extend_from_slice(&mp4a);
        let stsd = make_box(b"stsd", &stsd_body);

        // Build up the box chain.
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mdia = make_box(b"mdia", &minf);
        let trak = make_box(b"trak", &mdia);
        let moov = make_box(b"moov", &trak);

        let init = Bytes::from(moov);
        let got = extract_asc(&init).expect("ASC extracted");
        assert_eq!(got.as_ref(), asc);
    }

    fn make_box(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let total = (BOX_HEADER_LEN + body.len()) as u32;
        let mut out = total.to_be_bytes().to_vec();
        out.extend_from_slice(ty);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn extract_asc_returns_none_on_garbage() {
        assert!(extract_asc(&Bytes::from_static(b"this is not an init segment")).is_none());
    }

    #[test]
    fn extract_asc_returns_none_on_empty() {
        assert!(extract_asc(&Bytes::new()).is_none());
    }
}
