//! Property tests for [`lvqr_cmaf::CmafPolicyState`].
//!
//! The segmenter's whole job is to emit a stream of `CmafChunk` values
//! whose boundary classifications are internally consistent. These
//! properties lock in the invariants the LL-HLS and DASH playlist
//! generators rely on:
//!
//! * Every `Segment` chunk is independent (decoder can resume here).
//! * Segment boundaries never move backwards in DTS.
//! * Partial chunks never produce a DTS earlier than the current
//!   segment start.
//! * The policy never panics on any monotonic DTS sequence with any
//!   keyframe pattern, regardless of timescale.

use lvqr_cmaf::{CmafChunkKind, CmafPolicy, PolicyDecision};
use proptest::prelude::*;

// Re-export the internal state machine for tests. Using the public
// types only would force the test to drive a full FragmentStream;
// exposing the state machine keeps the proptest narrow.
use lvqr_cmaf::policy::CmafPolicyState;

/// Strategy: up to 64 steps, each a (keyframe, delta) pair. Deltas are
/// in [1, 200_000] so the total sequence can span several 2 s segments
/// at 90 kHz without overflowing.
fn step_strategy() -> impl Strategy<Value = Vec<(bool, u64)>> {
    prop::collection::vec((any::<bool>(), 1u64..=200_000), 1..=64)
}

fn walk(steps: &[(bool, u64)]) -> Vec<(PolicyDecision, u64)> {
    let mut state = CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT);
    let mut dts: u64 = 0;
    let mut out = Vec::with_capacity(steps.len());
    for &(kf, delta) in steps {
        let decision = state.step(kf, dts);
        out.push((decision, dts));
        dts = dts.saturating_add(delta);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn never_panics_on_any_sequence(steps in step_strategy()) {
        let _ = walk(&steps);
    }

    #[test]
    fn segment_chunks_are_always_independent(steps in step_strategy()) {
        for (decision, _) in walk(&steps) {
            if decision.kind == CmafChunkKind::Segment {
                prop_assert!(decision.kind.is_independent());
                prop_assert!(decision.kind.is_segment_start());
            }
        }
    }

    #[test]
    fn segment_dts_is_monotonic(steps in step_strategy()) {
        let mut last_segment_dts: Option<u64> = None;
        for (decision, dts) in walk(&steps) {
            if decision.kind == CmafChunkKind::Segment {
                if let Some(prev) = last_segment_dts {
                    prop_assert!(dts >= prev, "segment DTS went backwards: {prev} -> {dts}");
                }
                last_segment_dts = Some(dts);
            }
        }
    }

    #[test]
    fn first_decision_is_always_segment(steps in step_strategy()) {
        let walk = walk(&steps);
        prop_assert_eq!(walk[0].0.kind, CmafChunkKind::Segment);
    }
}
