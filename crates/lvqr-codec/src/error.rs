//! Shared error type for every codec parser in this crate.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("ran out of bits: needed {needed}, have {remaining}")]
    EndOfStream { needed: usize, remaining: usize },

    #[error("exp-Golomb code exceeds 32 bits")]
    GolombOverflow,

    #[error("invalid NAL unit type {0}")]
    InvalidNalType(u8),

    #[error("unsupported SPS feature: {0}")]
    Unsupported(&'static str),

    #[error("malformed SPS: {0}")]
    MalformedSps(&'static str),

    #[error("malformed AudioSpecificConfig: {0}")]
    MalformedAsc(&'static str),

    #[error("malformed SCTE-35 splice_info_section: {0}")]
    Scte35Malformed(&'static str),

    #[error("SCTE-35 CRC mismatch: computed {computed:#010x}, wire {wire:#010x}")]
    Scte35BadCrc { computed: u32, wire: u32 },
}
