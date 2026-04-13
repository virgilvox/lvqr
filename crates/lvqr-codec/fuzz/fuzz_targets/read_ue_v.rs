#![no_main]
//! libfuzzer target for `lvqr_codec::bit_reader::BitReader::read_ue_v`.
//!
//! The unsigned exp-Golomb decoder is the single bit-level routine
//! that every H.26x parser in LVQR funnels through. A panic here
//! would cascade into every HEVC / H.264 call site. The in-crate
//! unit tests already cover the canonical overflow guard; this fuzz
//! target ensures the same invariant holds on arbitrary byte streams
//! with arbitrary starting bit alignments.

use libfuzzer_sys::fuzz_target;
use lvqr_codec::bit_reader::BitReader;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // Use the first byte as a bit offset in [0, 8) so the fuzzer
    // exercises every starting alignment. The remaining bytes are
    // the backing buffer.
    let offset = (data[0] & 0x07) as usize;
    let bytes = &data[1..];
    let mut r = BitReader::new(bytes);
    let _ = r.skip_bits(offset);
    // Drain up to 64 consecutive exp-Golomb codes. Bounded so the
    // target terminates on every input and libfuzzer can move on.
    for _ in 0..64 {
        if r.read_ue_v().is_err() {
            break;
        }
    }
});
