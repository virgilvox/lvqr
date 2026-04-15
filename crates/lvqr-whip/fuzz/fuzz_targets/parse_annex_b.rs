#![no_main]
//! libfuzzer target for the WHIP Annex B -> AVCC depack path.
//!
//! WebRTC `Event::MediaData` hands `lvqr-whip` an opaque byte buffer
//! that the ingest bridge then walks as an Annex B access unit:
//! `split_annex_b` splits the buffer into NAL unit slices at
//! 3- / 4-byte start codes, and `annex_b_to_avcc` re-frames the same
//! bytes as an AVCC length-prefixed payload that `mp4-atom` consumes.
//! Both functions run on attacker-controlled bytes from the
//! browser's RTP depacketizer; they must never panic, never
//! read out of bounds, and never produce an AVCC that is longer than
//! the input + its own length prefixes (a bound the bridge relies on
//! when sizing downstream buffers).
//!
//! The proptest harness at `tests/proptest_depack.rs` covers a
//! structured never-panic property for the same functions; this
//! libfuzzer target exercises the same code with unstructured
//! mutation, which historically catches corner cases that structured
//! generators miss (e.g. start codes that straddle the input
//! boundary, emulation-prevention byte patterns at exact byte
//! offsets, zero-length trailing NAL units).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Primary split: identify NAL boundaries. The walker must
    // tolerate any input, including shorter-than-start-code
    // buffers and strings of zero bytes.
    let nals = lvqr_whip::split_annex_b(data);

    // Every reported NAL slice must be a real sub-slice of `data`.
    // Bounds-check the slice by pointer arithmetic to catch any
    // would-be out-of-range return without heap-allocating.
    let base = data.as_ptr() as usize;
    let end = base + data.len();
    for nal in &nals {
        let nal_start = nal.as_ptr() as usize;
        let nal_end = nal_start + nal.len();
        assert!(nal_start >= base, "nal starts before input buffer");
        assert!(nal_end <= end, "nal ends past input buffer");
    }

    // Secondary pass: Annex B -> AVCC. Must never panic on any
    // input; the output must be at most `input_len + 4 * nal_count`
    // bytes (one 4-byte length prefix per NAL). This loose bound
    // is enough to catch runaway growth; the tighter byte-for-byte
    // equivalence is already covered by the proptest.
    let avcc = lvqr_whip::annex_b_to_avcc(data);
    let max_expected = data.len() + 4 * nals.len().max(1);
    assert!(avcc.len() <= max_expected, "avcc output larger than upper bound");

    // HEVC NAL type decoder is the other untrusted-input surface in
    // depack; run it against every NAL the splitter produced so the
    // fuzzer exercises the 6-bit field extraction on crafted first
    // bytes.
    for nal in &nals {
        let _ = lvqr_whip::hevc_nal_type(nal);
    }
});
