//! `lvqr-moq` is the single point of contact between LVQR and the `moq-lite`
//! crate. Every other crate in the workspace that touches MoQ types imports
//! them from here rather than from `moq-lite` directly.
//!
//! This is roadmap decision 2 (see `tracking/ROADMAP.md`). The value is:
//!
//! * If `moq-lite` renames a type, moves a method, or bumps its major version,
//!   the edit lives in this file and nowhere else.
//! * If LVQR needs to swap the underlying MoQ implementation (e.g. from
//!   `moq-lite` to `moq-transport`), every downstream crate keeps compiling
//!   by updating this one re-export surface.
//! * Tests that want a "LVQR-flavored" MoQ type for future hardening
//!   (rate limits, quotas, audit hooks) have a single place to extend.
//!
//! The current implementation is a pure re-export layer. LVQR does not wrap
//! the types in newtypes today because `moq-lite` 0.15 is stable on crates.io
//! and the wrappers would force us to re-expose every method on `Track`,
//! `GroupProducer`, and friends -- that is its own form of drift. When we
//! actually need to alter behavior (e.g. to emit metrics on every frame
//! write) the facade is the place to introduce newtypes, and downstream
//! call sites will not have to change their imports.
//!
//! # Types re-exported
//!
//! | `lvqr_moq`             | `moq_lite` source                     |
//! |------------------------|---------------------------------------|
//! | `Track`                | `moq_lite::Track`                     |
//! | `Origin`               | `moq_lite::Origin`                    |
//! | `OriginProducer`       | `moq_lite::OriginProducer`            |
//! | `BroadcastProducer`    | `moq_lite::BroadcastProducer`         |
//! | `BroadcastConsumer`    | `moq_lite::BroadcastConsumer`         |
//! | `TrackProducer`        | `moq_lite::TrackProducer`             |
//! | `TrackConsumer`        | `moq_lite::TrackConsumer`             |
//! | `GroupProducer`        | `moq_lite::GroupProducer`             |
//! | `GroupConsumer`        | `moq_lite::GroupConsumer`             |
//!
//! Anything not on this table is intentionally not yet in scope. Add it here
//! when a downstream crate needs it, not by reaching into `moq_lite` directly.

pub use moq_lite::{
    BroadcastConsumer, BroadcastProducer, GroupConsumer, GroupProducer, Origin, OriginProducer, Track, TrackConsumer,
    TrackProducer,
};

/// The pinned `moq-lite` version this facade was built against. Bumping this
/// string is the mechanical signal that the facade's tests must be re-run.
pub const MOQ_LITE_VERSION: &str = "0.15";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_new_constructs_through_facade() {
        // Smoke test: the re-exported Track is the same surface as moq_lite::Track,
        // constructible by name and comparable by name.
        let t = Track::new("0.mp4");
        assert_eq!(t.name, "0.mp4");
    }

    #[tokio::test]
    async fn origin_producer_creates_broadcast_through_facade() {
        // The control plane flow we actually use: construct an OriginProducer,
        // open a broadcast, add one track. If this compiles and runs, the
        // facade is wired correctly.
        let origin = OriginProducer::new();
        let mut broadcast = origin.create_broadcast("facade-smoke").expect("create broadcast");
        let track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
        assert_eq!(track.info.name, "0.mp4");
    }

    #[test]
    fn moq_lite_version_constant_is_set() {
        assert!(!MOQ_LITE_VERSION.is_empty());
    }
}
