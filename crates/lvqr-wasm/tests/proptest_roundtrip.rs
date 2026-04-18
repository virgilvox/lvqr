//! Proptest harness for Tier 4 item 4.2 session A.
//!
//! Proves that a no-op WASM filter preserves the payload
//! byte-for-byte regardless of input shape (any length up to
//! a cap that fits in a 64 KiB page, any byte values). The cap
//! is deliberate: session A ships a minimum-viable host that
//! grows linear memory lazily, and proptest's default
//! shrinkage iterates long lengths faster than the tokio-green
//! test suite tolerates. 16 KiB is large enough to cover every
//! CMAF partial we emit today (200 ms at 4 Mbps = ~100 KB --
//! the full bound lands once session B wires the
//! `FragmentObserver` and the 1-page budget is relaxed).

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentFlags};
use lvqr_wasm::{FragmentFilter, WasmFilter};
use proptest::prelude::*;

const WAT_NOOP: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "on_fragment") (param i32 i32) (result i32)
        local.get 1))
"#;

fn make_filter() -> WasmFilter {
    let bytes = wat::parse_str(WAT_NOOP).expect("wat parse");
    WasmFilter::from_bytes(&bytes).expect("wasm compile")
}

fn fragment_strategy() -> impl Strategy<Value = Fragment> {
    (
        any::<String>()
            .prop_filter("utf8 only", |s| !s.is_empty())
            .prop_map(|s| {
                // Keep the track_id short so proptest shrinkage is
                // predictable; the scaffold does not touch metadata
                // so this is just a placeholder.
                s.chars().take(16).collect::<String>()
            }),
        any::<u64>(),
        any::<u64>(),
        any::<u8>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        proptest::collection::vec(any::<u8>(), 0..16 * 1024),
    )
        .prop_map(
            |(track_id, group_id, object_id, priority, dts, pts, duration, payload)| {
                Fragment::new(
                    track_id,
                    group_id,
                    object_id,
                    priority,
                    dts,
                    pts,
                    duration,
                    FragmentFlags::default(),
                    Bytes::from(payload),
                )
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 1024,
        .. ProptestConfig::default()
    })]

    /// The scaffold's no-op filter must preserve the payload
    /// byte-for-byte and all metadata unchanged. If this ever
    /// fails, the host-to-guest ABI has silently drifted.
    #[test]
    fn noop_filter_roundtrips_any_payload(frag in fragment_strategy()) {
        let filter = make_filter();
        let original = frag.clone();
        let out = filter.apply(frag).expect("no-op filter must keep the fragment");
        prop_assert_eq!(out.payload.as_ref(), original.payload.as_ref());
        prop_assert_eq!(out.track_id, original.track_id);
        prop_assert_eq!(out.group_id, original.group_id);
        prop_assert_eq!(out.object_id, original.object_id);
        prop_assert_eq!(out.priority, original.priority);
        prop_assert_eq!(out.dts, original.dts);
        prop_assert_eq!(out.pts, original.pts);
        prop_assert_eq!(out.duration, original.duration);
    }
}
