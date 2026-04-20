//! Unit tests for [`lvqr_cluster::FederationLink`] + [`FederationRunner`]
//! (Tier 4 item 4.4 session A).
//!
//! These tests cover the config shape (TOML / JSON serde), the
//! `subscription_url()` + `forwards()` helpers, and the runner
//! lifecycle with unreachable remote URLs (the runner must handle
//! connect errors gracefully without leaking tasks or panicking).
//!
//! A two-cluster integration test that actually moves fragments
//! across a federation link lands in session 102 B
//! (`crates/lvqr-cli/tests/federation_two_cluster.rs`). This file
//! stays deliberately network-free so the default
//! `cargo test --workspace` run exercises it in <1 s even on
//! constrained runners.

use std::time::Duration;

use lvqr_cluster::{FederationLink, FederationRunner};
use tokio_util::sync::CancellationToken;

#[test]
fn federation_link_json_round_trip() {
    let link = FederationLink::new(
        "https://peer.us-west.example:4443/",
        "jwt-abc",
        vec!["live/room1".into(), "live/room2".into()],
    );
    let json = serde_json::to_string(&link).expect("serialize");
    let parsed: FederationLink = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, link);
}

#[test]
fn federation_link_toml_round_trip() {
    // Operators will carry federation links through a TOML config
    // file in many deployments. Verify the TOML shape is stable.
    let link = FederationLink::new(
        "https://peer.us-east.example:4443/",
        "tok",
        vec!["live/featured".into()],
    );
    let toml_str = toml::to_string(&link).expect("serialize toml");
    let parsed: FederationLink = toml::from_str(&toml_str).expect("deserialize toml");
    assert_eq!(parsed, link);
}

#[test]
fn federation_link_deserializes_without_forwarded_broadcasts_key() {
    // `forwarded_broadcasts` carries serde's default annotation; an
    // operator who writes only remote_url + auth_token should get an
    // empty forward list rather than a parse error.
    let json = r#"{"remote_url":"https://peer:4443/","auth_token":"t"}"#;
    let parsed: FederationLink = serde_json::from_str(json).expect("deserialize with missing field");
    assert_eq!(parsed.remote_url, "https://peer:4443/");
    assert_eq!(parsed.auth_token, "t");
    assert!(parsed.forwarded_broadcasts.is_empty());
}

#[test]
fn federation_link_forwards_by_exact_match() {
    let link = FederationLink::new("https://peer:4443/", "t", vec!["live/alpha".into(), "live/beta".into()]);
    assert!(link.forwards("live/alpha"));
    assert!(link.forwards("live/beta"));
    assert!(!link.forwards("live/gamma"));
    assert!(!link.forwards("live/alpha/extra"));
    assert!(!link.forwards(""));
}

#[test]
fn subscription_url_appends_token_query() {
    let link = FederationLink::new("https://peer:4443/", "abc", Vec::new());
    let url = link.subscription_url().expect("valid url");
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(pairs, vec![("token".into(), "abc".into())]);
}

#[test]
fn subscription_url_errors_on_invalid_remote_url() {
    let link = FederationLink::new("not a url", "t", Vec::new());
    assert!(link.subscription_url().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_starts_and_shuts_down_with_no_links() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();
    let origin = lvqr_moq::OriginProducer::new();
    let shutdown = CancellationToken::new();
    let runner = FederationRunner::start(Vec::new(), origin, shutdown);
    assert_eq!(runner.configured_links(), 0);
    assert_eq!(runner.active_links(), 0);
    runner.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_exits_cleanly_on_shutdown_even_with_unreachable_remote() {
    // The remote URL points at a reserved TEST-NET-1 address (RFC
    // 5737). Connect will loop-and-time-out; the per-link task must
    // surface the error and the runner's shutdown must still complete
    // in bounded time. Tests the shutdown-before-connect race path.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let origin = lvqr_moq::OriginProducer::new();
    let shutdown = CancellationToken::new();
    let link = FederationLink::new("https://192.0.2.1:4443/", "t", vec!["live/x".into()]);
    let runner = FederationRunner::start(vec![link], origin, shutdown);
    assert_eq!(runner.configured_links(), 1);
    // Give the connect future a moment to get underway.
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Now trigger the shutdown and verify the runner winds down
    // within 2 s (the per-link task's select arm sees the cancel and
    // returns).
    let shutdown_fut = runner.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown_fut)
        .await
        .expect("runner must shut down within 2 s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_drops_clean_even_without_explicit_shutdown() {
    // The Drop impl aborts tasks so leaked handles do not outlive the
    // Cluster. Exercise the drop path explicitly.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let origin = lvqr_moq::OriginProducer::new();
    let shutdown = CancellationToken::new();
    let link = FederationLink::new("https://192.0.2.2:4443/", "t", vec![]);
    let runner = FederationRunner::start(vec![link], origin, shutdown);
    assert_eq!(runner.configured_links(), 1);
    // Dropping `runner` should trigger the cancel + abort path.
    drop(runner);
    // No assert beyond "this did not panic". Any leak of the
    // per-link task would be caught by tokio's shutdown at test
    // teardown.
}
