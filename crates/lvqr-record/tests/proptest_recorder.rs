//! Property tests for `lvqr-record`'s pure helpers.
//!
//! The recorder is a sink, not a parser, so the proptest slot in the
//! 5-artifact test contract (tests/CONTRACT.md) targets the three pure
//! helpers exposed via `lvqr_record::internals` rather than trying to
//! fuzz the full `record_broadcast` pipeline. What we want these tests
//! to catch:
//!
//!   - `sanitize_name` must never let a path-traversal or backslash
//!     component leak into the produced directory name, regardless of
//!     attacker-controlled input.
//!   - `track_prefix` must be a safe (non-panicking) function over
//!     arbitrary strings; the recorder calls it on every track name
//!     that comes off MoQ, which is a trust boundary.
//!   - `looks_like_init` must never panic on arbitrary byte slices; the
//!     recorder's per-frame classifier calls it on every frame, so a
//!     crash here is a remote crash vector.

use bytes::Bytes;
use lvqr_record::internals::{looks_like_init_bytes, sanitize_name, track_prefix};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        .. ProptestConfig::default()
    })]

    /// `sanitize_name` must strip every slash, backslash, and `..`
    /// sequence from its output, no matter what the input looks like.
    /// The output is used as a filesystem directory name, so any
    /// escape is a path-traversal bug.
    #[test]
    fn sanitize_name_strips_traversal_and_slashes(raw in ".{0,128}") {
        let cleaned = sanitize_name(&raw);
        prop_assert!(!cleaned.contains('/'), "sanitized name contains / : {cleaned:?}");
        prop_assert!(!cleaned.contains('\\'), "sanitized name contains \\ : {cleaned:?}");
        prop_assert!(!cleaned.contains(".."), "sanitized name contains .. : {cleaned:?}");
        // Control characters (\x00 .. \x1f, \x7f) must also be absent,
        // since the recorder uses the sanitized name as a log field
        // and a filesystem path component.
        prop_assert!(
            cleaned.chars().all(|c| !c.is_control()),
            "sanitized name contains control chars: {cleaned:?}"
        );
    }

    /// Wide-character strategy variant: proves the sanitizer is
    /// unicode-aware, not just ASCII-aware. Latin-1 plus a pile of
    /// random control bytes and astral-plane runs shakes out any
    /// `[u8]`-indexing assumption in the helper.
    #[test]
    fn sanitize_name_handles_arbitrary_unicode(raw in "\\PC{0,64}") {
        let cleaned = sanitize_name(&raw);
        prop_assert!(!cleaned.contains('/'));
        prop_assert!(!cleaned.contains('\\'));
        prop_assert!(!cleaned.contains(".."));
    }

    /// `track_prefix` must never panic and must return a prefix that
    /// is free of the `.` separator, because the recorder uses the
    /// output directly as a filename stem.
    #[test]
    fn track_prefix_never_panics_and_strips_extension(name in "\\PC{0,64}") {
        let prefix = track_prefix(&name);
        prop_assert!(!prefix.contains('.'), "prefix contains . : {prefix:?}");
        // A dotless input round-trips verbatim.
        if !name.contains('.') {
            prop_assert_eq!(prefix, name);
        }
    }

    /// `looks_like_init` must accept or reject any byte slice without
    /// panicking. Inputs under 8 bytes are always rejected; inputs
    /// with `ftyp` at offset 4..8 are always accepted; no crash is
    /// allowed for any input length.
    #[test]
    fn looks_like_init_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let b = Bytes::from(bytes.clone());
        let result = looks_like_init_bytes(&b);

        // Cross-check the result against a byte-level reconstruction
        // so we catch any off-by-one drift in the helper.
        let expected = bytes.len() >= 8 && &bytes[4..8] == b"ftyp";
        prop_assert_eq!(result, expected);
    }

    /// Targeted "fMP4 init shape" strategy: a 4-byte size field, the
    /// literal `ftyp`, and a random compatibility-brand tail. This
    /// exercises the positive branch of `looks_like_init` the way
    /// real recorder inputs do.
    #[test]
    fn looks_like_init_accepts_ftyp_shaped_bytes(
        size in 8u32..4096,
        tail in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let mut buf = Vec::with_capacity(8 + tail.len());
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(b"ftyp");
        buf.extend_from_slice(&tail);
        prop_assert!(looks_like_init_bytes(&Bytes::from(buf)));
    }
}
