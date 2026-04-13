//! [`FragmentStream`] trait: the async surface every fragment producer implements.
//!
//! Consumers call [`FragmentStream::next_fragment`] in a loop until it returns
//! `None`. Implementors stash the [`FragmentMeta`] at construction time so
//! consumers can read it via [`FragmentStream::meta`] without waiting.
//!
//! We intentionally do not use `async_trait` here. The trait is narrow and the
//! future is always borrowed from `self`, so returning
//! `Pin<Box<dyn Future<...> + '_>>` keeps the trait object-safe without the
//! macro dependency.

use crate::fragment::{Fragment, FragmentMeta};
use std::future::Future;
use std::pin::Pin;

/// A source of [`Fragment`] values for one logical track.
///
/// Every ingest protocol that lands in `lvqr-ingest` exposes its output as
/// a `FragmentStream`. Every egress in `lvqr-relay`, `lvqr-record`, and the
/// future protocol crates reads from one. This is the single extensibility
/// seam that replaces the ad-hoc "write directly to a MoQ `TrackProducer`"
/// pattern the RTMP bridge used in v0.3.
pub trait FragmentStream: Send {
    /// Metadata describing the stream (codec, timescale, init segment).
    /// Available immediately after construction.
    fn meta(&self) -> &FragmentMeta;

    /// Pull the next fragment. Returns `None` when the stream ends.
    fn next_fragment<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Fragment>> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragment::FragmentFlags;
    use bytes::Bytes;

    /// Minimal in-memory FragmentStream that drains a `Vec<Fragment>`.
    /// Used by the proptest and by downstream crates' unit tests when they
    /// need a deterministic stream without wiring a real parser.
    struct VecStream {
        meta: FragmentMeta,
        remaining: std::collections::VecDeque<Fragment>,
    }

    impl FragmentStream for VecStream {
        fn meta(&self) -> &FragmentMeta {
            &self.meta
        }
        fn next_fragment<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Fragment>> + Send + 'a>> {
            Box::pin(async move { self.remaining.pop_front() })
        }
    }

    #[tokio::test]
    async fn vec_stream_drains_in_order() {
        let meta = FragmentMeta::new("avc1.640028", 90000);
        let mut s = VecStream {
            meta,
            remaining: [0u64, 1, 2]
                .into_iter()
                .map(|i| {
                    Fragment::new(
                        "0.mp4",
                        1,
                        i,
                        0,
                        i * 3000,
                        i * 3000,
                        3000,
                        if i == 0 {
                            FragmentFlags::KEYFRAME
                        } else {
                            FragmentFlags::DELTA
                        },
                        Bytes::from(vec![i as u8; 4]),
                    )
                })
                .collect(),
        };
        assert_eq!(s.meta().codec, "avc1.640028");
        for expected in 0..3u64 {
            let f = s.next_fragment().await.expect("has fragment");
            assert_eq!(f.object_id, expected);
        }
        assert!(s.next_fragment().await.is_none());
    }
}
