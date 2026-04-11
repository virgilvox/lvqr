//! Fragmented MP4 (CMAF) box writer.
//!
//! Generates fMP4 init segments (ftyp + moov) and media segments (moof + mdat)
//! compatible with MSE SourceBuffer and the MoQ ecosystem.
//!
//! Written manually with `bytes::BytesMut` -- no external MP4 crate needed.

use bytes::{BufMut, Bytes, BytesMut};

use super::flv::{AudioConfig, VideoConfig};

/// A single video sample for inclusion in an fMP4 media segment.
#[derive(Debug, Clone)]
pub struct VideoSample {
    /// AVCC-format NALU data (length-prefixed, same as FLV).
    pub data: Bytes,
    /// Duration in timescale ticks (90kHz for video).
    pub duration: u32,
    /// Composition time offset in timescale ticks.
    pub cts_offset: i32,
    /// Whether this is a sync sample (keyframe).
    pub keyframe: bool,
}

// --- Box writing helpers ---

fn write_box(buf: &mut BytesMut, box_type: &[u8; 4], f: impl FnOnce(&mut BytesMut)) {
    let start = buf.len();
    buf.put_u32(0); // placeholder for size
    buf.put_slice(box_type);
    f(buf);
    let size = (buf.len() - start) as u32;
    let start_bytes = size.to_be_bytes();
    buf[start..start + 4].copy_from_slice(&start_bytes);
}

fn write_full_box(buf: &mut BytesMut, box_type: &[u8; 4], version: u8, flags: u32, f: impl FnOnce(&mut BytesMut)) {
    write_box(buf, box_type, |buf| {
        buf.put_u8(version);
        buf.put_u8((flags >> 16) as u8);
        buf.put_u8((flags >> 8) as u8);
        buf.put_u8(flags as u8);
        f(buf);
    });
}

// --- Init segment generation ---

/// Generate a video init segment (ftyp + moov) for H.264/AVC.
pub fn video_init_segment(config: &VideoConfig) -> Bytes {
    let mut buf = BytesMut::with_capacity(512);

    // ftyp
    write_box(&mut buf, b"ftyp", |buf| {
        buf.put_slice(b"isom"); // major_brand
        buf.put_u32(0); // minor_version
        buf.put_slice(b"isom");
        buf.put_slice(b"iso6");
        buf.put_slice(b"msdh");
        buf.put_slice(b"msix");
    });

    // moov
    write_box(&mut buf, b"moov", |buf| {
        // mvhd
        write_full_box(buf, b"mvhd", 0, 0, |buf| {
            buf.put_u32(0); // creation_time
            buf.put_u32(0); // modification_time
            buf.put_u32(90000); // timescale
            buf.put_u32(0); // duration
            buf.put_u32(0x00010000); // rate = 1.0
            buf.put_u16(0x0100); // volume = 1.0
            buf.put_bytes(0, 10); // reserved
            // identity matrix (9 x u32)
            for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
                buf.put_u32(v);
            }
            buf.put_bytes(0, 24); // pre_defined
            buf.put_u32(2); // next_track_ID
        });

        // trak
        write_box(buf, b"trak", |buf| {
            // tkhd
            write_full_box(buf, b"tkhd", 0, 0x03, |buf| {
                buf.put_u32(0); // creation_time
                buf.put_u32(0); // modification_time
                buf.put_u32(1); // track_ID
                buf.put_u32(0); // reserved
                buf.put_u32(0); // duration
                buf.put_bytes(0, 8); // reserved
                buf.put_u16(0); // layer
                buf.put_u16(0); // alternate_group
                buf.put_u16(0); // volume (0 for video)
                buf.put_u16(0); // reserved
                // identity matrix
                for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
                    buf.put_u32(v);
                }
                buf.put_u32(0); // width (not known from SPS without parsing)
                buf.put_u32(0); // height
            });

            // mdia
            write_box(buf, b"mdia", |buf| {
                // mdhd
                write_full_box(buf, b"mdhd", 0, 0, |buf| {
                    buf.put_u32(0); // creation_time
                    buf.put_u32(0); // modification_time
                    buf.put_u32(90000); // timescale
                    buf.put_u32(0); // duration
                    buf.put_u32(0x55C40000); // language = "und"
                });

                // hdlr
                write_full_box(buf, b"hdlr", 0, 0, |buf| {
                    buf.put_u32(0); // pre_defined
                    buf.put_slice(b"vide"); // handler_type
                    buf.put_bytes(0, 12); // reserved
                    buf.put_slice(b"LVQR Video\0");
                });

                // minf
                write_box(buf, b"minf", |buf| {
                    // vmhd
                    write_full_box(buf, b"vmhd", 0, 1, |buf| {
                        buf.put_u16(0); // graphicsmode
                        buf.put_bytes(0, 6); // opcolor
                    });

                    // dinf
                    write_box(buf, b"dinf", |buf| {
                        write_full_box(buf, b"dref", 0, 0, |buf| {
                            buf.put_u32(1); // entry_count
                            write_full_box(buf, b"url ", 0, 1, |_buf| {
                                // self-contained flag set, no URL
                            });
                        });
                    });

                    // stbl
                    write_box(buf, b"stbl", |buf| {
                        // stsd
                        write_full_box(buf, b"stsd", 0, 0, |buf| {
                            buf.put_u32(1); // entry_count

                            // avc1 sample entry
                            write_box(buf, b"avc1", |buf| {
                                buf.put_bytes(0, 6); // reserved
                                buf.put_u16(1); // data_reference_index
                                buf.put_bytes(0, 16); // pre_defined + reserved
                                buf.put_u16(0); // width (unknown)
                                buf.put_u16(0); // height (unknown)
                                buf.put_u32(0x00480000); // horizresolution = 72 dpi
                                buf.put_u32(0x00480000); // vertresolution = 72 dpi
                                buf.put_u32(0); // reserved
                                buf.put_u16(1); // frame_count
                                buf.put_bytes(0, 32); // compressorname
                                buf.put_u16(0x0018); // depth = 24
                                buf.put_i16(-1); // pre_defined

                                // avcC box (AVCDecoderConfigurationRecord)
                                write_box(buf, b"avcC", |buf| {
                                    buf.put_u8(1); // configurationVersion
                                    buf.put_u8(config.profile);
                                    buf.put_u8(config.compat);
                                    buf.put_u8(config.level);
                                    buf.put_u8(0xFF); // lengthSizeMinusOne=3 | reserved
                                    buf.put_u8(0xE1); // numSPS=1 | reserved
                                    buf.put_u16(config.sps.len() as u16);
                                    buf.put_slice(&config.sps);
                                    buf.put_u8(1); // numPPS
                                    buf.put_u16(config.pps.len() as u16);
                                    buf.put_slice(&config.pps);
                                });
                            });
                        });

                        // Empty required boxes
                        write_full_box(buf, b"stts", 0, 0, |buf| buf.put_u32(0));
                        write_full_box(buf, b"stsc", 0, 0, |buf| buf.put_u32(0));
                        write_full_box(buf, b"stsz", 0, 0, |buf| {
                            buf.put_u32(0); // sample_size
                            buf.put_u32(0); // sample_count
                        });
                        write_full_box(buf, b"stco", 0, 0, |buf| buf.put_u32(0));
                    });
                });
            });
        });

        // mvex
        write_box(buf, b"mvex", |buf| {
            write_full_box(buf, b"trex", 0, 0, |buf| {
                buf.put_u32(1); // track_ID
                buf.put_u32(1); // default_sample_description_index
                buf.put_u32(0); // default_sample_duration
                buf.put_u32(0); // default_sample_size
                buf.put_u32(0); // default_sample_flags
            });
        });
    });

    buf.freeze()
}

/// Generate an audio init segment (ftyp + moov) for AAC.
pub fn audio_init_segment(config: &AudioConfig) -> Bytes {
    let mut buf = BytesMut::with_capacity(512);

    // ftyp
    write_box(&mut buf, b"ftyp", |buf| {
        buf.put_slice(b"isom");
        buf.put_u32(0);
        buf.put_slice(b"isom");
        buf.put_slice(b"iso6");
        buf.put_slice(b"msdh");
        buf.put_slice(b"msix");
    });

    // moov
    write_box(&mut buf, b"moov", |buf| {
        // mvhd
        write_full_box(buf, b"mvhd", 0, 0, |buf| {
            buf.put_u32(0);
            buf.put_u32(0);
            buf.put_u32(config.sample_rate);
            buf.put_u32(0);
            buf.put_u32(0x00010000);
            buf.put_u16(0x0100);
            buf.put_bytes(0, 10);
            for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
                buf.put_u32(v);
            }
            buf.put_bytes(0, 24);
            buf.put_u32(2);
        });

        // trak
        write_box(buf, b"trak", |buf| {
            write_full_box(buf, b"tkhd", 0, 0x03, |buf| {
                buf.put_u32(0);
                buf.put_u32(0);
                buf.put_u32(1);
                buf.put_u32(0);
                buf.put_u32(0);
                buf.put_bytes(0, 8);
                buf.put_u16(0);
                buf.put_u16(0);
                buf.put_u16(0x0100); // volume = 1.0 (audio)
                buf.put_u16(0);
                for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
                    buf.put_u32(v);
                }
                buf.put_u32(0);
                buf.put_u32(0);
            });

            write_box(buf, b"mdia", |buf| {
                write_full_box(buf, b"mdhd", 0, 0, |buf| {
                    buf.put_u32(0);
                    buf.put_u32(0);
                    buf.put_u32(config.sample_rate);
                    buf.put_u32(0);
                    buf.put_u32(0x55C40000);
                });

                write_full_box(buf, b"hdlr", 0, 0, |buf| {
                    buf.put_u32(0);
                    buf.put_slice(b"soun");
                    buf.put_bytes(0, 12);
                    buf.put_slice(b"LVQR Audio\0");
                });

                write_box(buf, b"minf", |buf| {
                    write_full_box(buf, b"smhd", 0, 0, |buf| {
                        buf.put_u16(0); // balance
                        buf.put_u16(0); // reserved
                    });

                    write_box(buf, b"dinf", |buf| {
                        write_full_box(buf, b"dref", 0, 0, |buf| {
                            buf.put_u32(1);
                            write_full_box(buf, b"url ", 0, 1, |_buf| {});
                        });
                    });

                    write_box(buf, b"stbl", |buf| {
                        write_full_box(buf, b"stsd", 0, 0, |buf| {
                            buf.put_u32(1);

                            // mp4a sample entry
                            write_box(buf, b"mp4a", |buf| {
                                buf.put_bytes(0, 6); // reserved
                                buf.put_u16(1); // data_reference_index
                                buf.put_bytes(0, 8); // reserved
                                buf.put_u16(config.channels as u16);
                                buf.put_u16(16); // sampleSize
                                buf.put_u16(0); // pre_defined
                                buf.put_u16(0); // reserved
                                buf.put_u32(config.sample_rate << 16); // sampleRate (fixed-point 16.16)

                                // esds box
                                write_full_box(buf, b"esds", 0, 0, |buf| {
                                    // ES_Descriptor
                                    buf.put_u8(0x03); // ES_DescrTag
                                    let asc_len = config.asc.len();
                                    let decoder_config_len = 13 + 2 + asc_len;
                                    let es_desc_len = 3 + 2 + decoder_config_len + 3;
                                    buf.put_u8(es_desc_len as u8); // length
                                    buf.put_u16(1); // ES_ID
                                    buf.put_u8(0); // streamDependenceFlag, URL_Flag, OCRstreamFlag, streamPriority

                                    // DecoderConfigDescriptor
                                    buf.put_u8(0x04); // DecoderConfigDescrTag
                                    buf.put_u8(decoder_config_len as u8);
                                    buf.put_u8(0x40); // objectTypeIndication = Audio ISO/IEC 14496-3
                                    buf.put_u8(0x15); // streamType=5 (audio) | upStream=0 | reserved=1
                                    buf.put_u8(0); // bufferSizeDB (24 bits)
                                    buf.put_u16(0);
                                    buf.put_u32(0); // maxBitrate
                                    buf.put_u32(0); // avgBitrate

                                    // DecoderSpecificInfo
                                    buf.put_u8(0x05); // DecoderSpecificInfoTag
                                    buf.put_u8(asc_len as u8);
                                    buf.put_slice(&config.asc);

                                    // SLConfigDescriptor
                                    buf.put_u8(0x06); // SLConfigDescrTag
                                    buf.put_u8(1); // length
                                    buf.put_u8(0x02); // predefined = MP4
                                });
                            });
                        });

                        write_full_box(buf, b"stts", 0, 0, |buf| buf.put_u32(0));
                        write_full_box(buf, b"stsc", 0, 0, |buf| buf.put_u32(0));
                        write_full_box(buf, b"stsz", 0, 0, |buf| {
                            buf.put_u32(0);
                            buf.put_u32(0);
                        });
                        write_full_box(buf, b"stco", 0, 0, |buf| buf.put_u32(0));
                    });
                });
            });
        });

        write_box(buf, b"mvex", |buf| {
            write_full_box(buf, b"trex", 0, 0, |buf| {
                buf.put_u32(1);
                buf.put_u32(1);
                buf.put_u32(0);
                buf.put_u32(0);
                buf.put_u32(0);
            });
        });
    });

    buf.freeze()
}

// --- Media segment generation ---

/// Generate a video media segment (moof + mdat) containing one or more samples.
pub fn video_segment(sequence: u32, base_dts: u64, samples: &[VideoSample]) -> Bytes {
    if samples.is_empty() {
        return Bytes::new();
    }

    let total_data_size: usize = samples.iter().map(|s| s.data.len()).sum();
    let mut buf = BytesMut::with_capacity(256 + total_data_size);

    // moof
    let moof_start = buf.len();
    write_box(&mut buf, b"moof", |buf| {
        // mfhd
        write_full_box(buf, b"mfhd", 0, 0, |buf| {
            buf.put_u32(sequence);
        });

        // traf
        write_box(buf, b"traf", |buf| {
            // tfhd: default-base-is-moof flag (0x020000)
            write_full_box(buf, b"tfhd", 0, 0x020000, |buf| {
                buf.put_u32(1); // track_ID
            });

            // tfdt: baseMediaDecodeTime
            write_full_box(buf, b"tfdt", 1, 0, |buf| {
                buf.put_u64(base_dts);
            });

            // trun: sample_count, data_offset, per-sample: duration, size, flags, cts_offset
            // flags: 0x000001 (data-offset) | 0x000100 (duration) | 0x000200 (size)
            //      | 0x000400 (flags) | 0x000800 (cts offset)
            let trun_flags: u32 = 0x000001 | 0x000100 | 0x000200 | 0x000400 | 0x000800;
            write_full_box(buf, b"trun", 0, trun_flags, |buf| {
                buf.put_u32(samples.len() as u32);
                // data_offset placeholder -- we'll fix this after writing moof
                let data_offset_pos = buf.len();
                buf.put_i32(0); // placeholder

                for sample in samples {
                    buf.put_u32(sample.duration);
                    buf.put_u32(sample.data.len() as u32);
                    let flags: u32 = if sample.keyframe {
                        0x02000000 // is_leading=0, depends_on=2 (does NOT depend), is_depended_on=0, is_sync
                    } else {
                        0x01010000 // depends_on=1 (does depend), not sync
                    };
                    buf.put_u32(flags);
                    buf.put_i32(sample.cts_offset);
                }

                // Fix data_offset: offset from moof_start to start of mdat payload
                // We don't know mdat header size yet, but it's always 8 bytes (size + type)
                // data_offset = (moof size) + 8 (mdat header)
                // We'll fix this after writing the moof box
                let _ = data_offset_pos; // used below after moof is complete
            });
        });
    });

    let moof_size = buf.len() - moof_start;
    // data_offset = moof_size + 8 (mdat header)
    let data_offset = (moof_size + 8) as i32;

    // Find and fix the data_offset in trun
    // The data_offset is at a known position within the trun box.
    // trun structure: [4 size][4 type][4 full_box_header][4 sample_count][4 data_offset]...
    // We need to find it. Let's search for the placeholder.
    // Since we control the layout, the data_offset is the first i32 after sample_count in trun.
    // We'll patch it by scanning for the trun box.
    patch_trun_data_offset(&mut buf, moof_start, data_offset);

    // mdat
    write_box(&mut buf, b"mdat", |buf| {
        for sample in samples {
            buf.put_slice(&sample.data);
        }
    });

    buf.freeze()
}

/// Generate an audio media segment (moof + mdat) containing a single AAC frame.
pub fn audio_segment(sequence: u32, base_dts: u64, duration: u32, data: &Bytes) -> Bytes {
    let mut buf = BytesMut::with_capacity(128 + data.len());

    let moof_start = buf.len();
    write_box(&mut buf, b"moof", |buf| {
        write_full_box(buf, b"mfhd", 0, 0, |buf| {
            buf.put_u32(sequence);
        });

        write_box(buf, b"traf", |buf| {
            write_full_box(buf, b"tfhd", 0, 0x020000, |buf| {
                buf.put_u32(1);
            });

            write_full_box(buf, b"tfdt", 1, 0, |buf| {
                buf.put_u64(base_dts);
            });

            // trun: data_offset + duration + size
            let trun_flags: u32 = 0x000001 | 0x000100 | 0x000200;
            write_full_box(buf, b"trun", 0, trun_flags, |buf| {
                buf.put_u32(1); // sample_count
                buf.put_i32(0); // data_offset placeholder
                buf.put_u32(duration);
                buf.put_u32(data.len() as u32);
            });
        });
    });

    let moof_size = buf.len() - moof_start;
    let data_offset = (moof_size + 8) as i32;
    patch_trun_data_offset(&mut buf, moof_start, data_offset);

    write_box(&mut buf, b"mdat", |buf| {
        buf.put_slice(data);
    });

    buf.freeze()
}

/// Find the trun box within a moof and patch its data_offset field.
fn patch_trun_data_offset(buf: &mut BytesMut, moof_start: usize, data_offset: i32) {
    let mut pos = moof_start + 8; // skip moof header
    while pos + 8 <= buf.len() {
        let box_size = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        let box_type = &buf[pos + 4..pos + 8];

        if box_type == b"traf" {
            // Recurse into traf
            let traf_end = pos + box_size;
            let mut inner = pos + 8;
            while inner + 8 <= traf_end {
                let inner_size =
                    u32::from_be_bytes([buf[inner], buf[inner + 1], buf[inner + 2], buf[inner + 3]]) as usize;
                let inner_type = &buf[inner + 4..inner + 8];

                if inner_type == b"trun" {
                    // trun: [4 size][4 type][4 full_box_header][4 sample_count][4 data_offset]
                    let offset_pos = inner + 8 + 4 + 4; // after header + full_box + sample_count
                    let bytes = data_offset.to_be_bytes();
                    buf[offset_pos..offset_pos + 4].copy_from_slice(&bytes);
                    return;
                }
                inner += inner_size;
            }
        }
        pos += box_size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remux::flv;

    fn read_box_at(data: &[u8], offset: usize) -> Option<(usize, &[u8; 4], &[u8])> {
        if offset + 8 > data.len() {
            return None;
        }
        let size = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]) as usize;
        if offset + size > data.len() || size < 8 {
            return None;
        }
        let box_type: &[u8; 4] = data[offset + 4..offset + 8].try_into().ok()?;
        let payload = &data[offset + 8..offset + size];
        Some((size, box_type, payload))
    }

    fn find_box<'a>(data: &'a [u8], target: &[u8; 4]) -> Option<(usize, &'a [u8])> {
        let mut pos = 0;
        while let Some((size, box_type, payload)) = read_box_at(data, pos) {
            if box_type == target {
                return Some((pos, payload));
            }
            pos += size;
        }
        None
    }

    fn test_video_config() -> flv::VideoConfig {
        flv::VideoConfig {
            sps: vec![0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9],
            pps: vec![0x68, 0xEE, 0x3C, 0x80],
            profile: 0x64,
            compat: 0x00,
            level: 0x1F,
            nalu_length_size: 4,
        }
    }

    fn test_audio_config() -> flv::AudioConfig {
        flv::AudioConfig {
            asc: vec![0x12, 0x10], // AAC-LC, 44100 Hz, stereo
            sample_rate: 44100,
            channels: 2,
            object_type: 2,
        }
    }

    #[test]
    fn video_init_starts_with_ftyp() {
        let init = video_init_segment(&test_video_config());
        assert!(init.len() > 8);
        assert_eq!(&init[4..8], b"ftyp");
    }

    #[test]
    fn video_init_contains_moov() {
        let init = video_init_segment(&test_video_config());
        assert!(find_box(&init, b"moov").is_some());
    }

    #[test]
    fn video_init_contains_avcc_with_sps_pps() {
        let config = test_video_config();
        let init = video_init_segment(&config);

        // Search for avcC box by scanning the bytes
        let init_bytes = &init[..];
        let avcc_needle = b"avcC";
        let pos = init_bytes
            .windows(4)
            .position(|w| w == avcc_needle)
            .expect("avcC box not found");

        // avcC payload starts after the box header (4 bytes before the type + 4 type bytes = skip 4 after type)
        let avcc_start = pos + 4; // past "avcC" type
        assert_eq!(init_bytes[avcc_start], 1); // configurationVersion
        assert_eq!(init_bytes[avcc_start + 1], config.profile);
        assert_eq!(init_bytes[avcc_start + 2], config.compat);
        assert_eq!(init_bytes[avcc_start + 3], config.level);
    }

    #[test]
    fn audio_init_starts_with_ftyp() {
        let init = audio_init_segment(&test_audio_config());
        assert_eq!(&init[4..8], b"ftyp");
    }

    #[test]
    fn audio_init_contains_esds() {
        let init = audio_init_segment(&test_audio_config());
        let init_bytes = &init[..];
        let esds_pos = init_bytes.windows(4).position(|w| w == b"esds");
        assert!(esds_pos.is_some(), "esds box not found in audio init");
    }

    #[test]
    fn video_segment_structure() {
        let samples = vec![VideoSample {
            data: Bytes::from(vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00]),
            duration: 3000,
            cts_offset: 0,
            keyframe: true,
        }];

        let seg = video_segment(1, 0, &samples);
        assert!(!seg.is_empty());

        // Should start with moof
        assert_eq!(&seg[4..8], b"moof");

        // Should have mdat after moof
        let moof_size = u32::from_be_bytes([seg[0], seg[1], seg[2], seg[3]]) as usize;
        assert_eq!(&seg[moof_size + 4..moof_size + 8], b"mdat");

        // mdat should contain our NALU data
        let mdat_size = u32::from_be_bytes([
            seg[moof_size],
            seg[moof_size + 1],
            seg[moof_size + 2],
            seg[moof_size + 3],
        ]) as usize;
        let mdat_payload = &seg[moof_size + 8..moof_size + mdat_size];
        assert_eq!(mdat_payload, &[0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00]);
    }

    #[test]
    fn video_segment_data_offset_correct() {
        let samples = vec![VideoSample {
            data: Bytes::from(vec![0x65, 0x88]),
            duration: 3000,
            cts_offset: 0,
            keyframe: true,
        }];

        let seg = video_segment(1, 0, &samples);
        let moof_size = u32::from_be_bytes([seg[0], seg[1], seg[2], seg[3]]) as usize;

        // data_offset in trun should point to mdat payload (moof_size + 8)
        // Find trun in the segment
        let trun_needle = b"trun";
        let trun_pos = seg.windows(4).position(|w| w == trun_needle).unwrap();
        // data_offset is at trun_pos + 4 (past type) + 4 (full_box) + 4 (sample_count) = +12
        let do_pos = trun_pos + 4 + 4 + 4;
        let data_offset = i32::from_be_bytes([seg[do_pos], seg[do_pos + 1], seg[do_pos + 2], seg[do_pos + 3]]);
        assert_eq!(data_offset as usize, moof_size + 8);
    }

    #[test]
    fn audio_segment_structure() {
        let data = Bytes::from(vec![0x01, 0x02, 0x03, 0x04]);
        let seg = audio_segment(1, 0, 1024, &data);

        assert_eq!(&seg[4..8], b"moof");
        let moof_size = u32::from_be_bytes([seg[0], seg[1], seg[2], seg[3]]) as usize;
        assert_eq!(&seg[moof_size + 4..moof_size + 8], b"mdat");
    }

    #[test]
    fn video_segment_multiple_samples() {
        let samples = vec![
            VideoSample {
                data: Bytes::from(vec![0x65, 0x88]),
                duration: 3000,
                cts_offset: 0,
                keyframe: true,
            },
            VideoSample {
                data: Bytes::from(vec![0x41, 0x9A, 0x00]),
                duration: 3000,
                cts_offset: 0,
                keyframe: false,
            },
        ];

        let seg = video_segment(1, 0, &samples);
        let moof_size = u32::from_be_bytes([seg[0], seg[1], seg[2], seg[3]]) as usize;
        let mdat_payload = &seg[moof_size + 8..];
        // mdat should contain both samples' data concatenated
        assert_eq!(mdat_payload, &[0x65, 0x88, 0x41, 0x9A, 0x00]);
    }

    #[test]
    fn empty_samples_returns_empty() {
        let seg = video_segment(1, 0, &[]);
        assert!(seg.is_empty());
    }
}
