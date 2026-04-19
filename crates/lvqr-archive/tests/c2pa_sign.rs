//! `cargo test -p lvqr-archive --features c2pa --test c2pa_sign`
//!
//! Tier 4 item 4.3 sessions A (session 91) + B1 (session 92) + B2
//! (session 93). Integration coverage for
//! [`lvqr_archive::provenance`]:
//!
//! * [`sign_asset_with_signer`](lvqr_archive::provenance::sign_asset_with_signer)
//!   end-to-end via [`c2pa::EphemeralSigner`]. Session 93's cert-
//!   fixture breakthrough: c2pa-rs 0.80 publicly exports
//!   `EphemeralSigner` which generates C2PA-spec-compliant Ed25519
//!   cert chains in memory (its own `ephemeral_cert` module builds
//!   them with rasn_pkix and the exact extensions c2pa-rs's
//!   profile check wants). This replaces the session-91 rcgen-based
//!   chain that the profile check kept rejecting with the generic
//!   `CertificateProfileError::InvalidCertificate`. The
//!   EphemeralSigner path is the canonical test-time signer in both
//!   c2pa-rs's own suite and this one.
//!
//! * [`finalize_broadcast_signed_with_signer`](lvqr_archive::provenance::finalize_broadcast_signed_with_signer)
//!   orchestration end-to-end: init-bytes + (empty) segment list →
//!   sign → write_signed_pair on disk → read back and assert.
//!
//! * [`sign_asset_bytes`](lvqr_archive::provenance::sign_asset_bytes)
//!   error path when the configured PEM files are missing.
//!
//! Gated on `feature = "c2pa"` so `cargo test -p lvqr-archive`
//! (default features) compiles this file as an empty binary and
//! skips it, matching the io-uring test pattern.

#![cfg(feature = "c2pa")]

use std::fs;

use lvqr_archive::provenance::{
    C2paConfig, C2paSigningAlg, SignOptions, finalize_broadcast_signed_with_signer, sign_asset_bytes,
    sign_asset_with_signer,
};
use tempfile::TempDir;

/// 155-byte 1x1 black JPEG. SOI + JFIF APP0 + DQT + SOF0 + DHT + SOS +
/// scan + EOI. c2pa-rs's JPEG handler uses `jfifdump` which strictly
/// validates every marker, so a 22-byte SOI/APP0/EOI stub is rejected
/// -- this fixture carries the real DQT / DHT tables a baseline JPEG
/// decoder needs. Smallest structurally-valid JPEG for c2pa's
/// signing path.
const MINIMAL_JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x01, 0x00, 0x60, 0x00, 0x60, 0x00,
    0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08,
    0x0a, 0x0c, 0x14, 0x0d, 0x0c, 0x0b, 0x0b, 0x0c, 0x19, 0x12, 0x13, 0x0f, 0x14, 0x1d, 0x1a, 0x1f, 0x1e, 0x1d, 0x1a,
    0x1c, 0x1c, 0x20, 0x24, 0x2e, 0x27, 0x20, 0x22, 0x2c, 0x23, 0x1c, 0x1c, 0x28, 0x37, 0x29, 0x2c, 0x30, 0x31, 0x34,
    0x34, 0x34, 0x1f, 0x27, 0x39, 0x3d, 0x38, 0x32, 0x3c, 0x2e, 0x33, 0x34, 0x32, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00,
    0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xda, 0x00, 0x08, 0x01, 0x01,
    0x00, 0x00, 0x3f, 0x00, 0x57, 0xff, 0xd9,
];

fn ephemeral_signer() -> c2pa::EphemeralSigner {
    c2pa::EphemeralSigner::new("lvqr-c2pa-test.local").expect("generate ephemeral c2pa signer")
}

fn default_options() -> SignOptions {
    SignOptions {
        assertion_creator: "LVQR Test Operator".to_string(),
        // EphemeralSigner's CA is not in c2pa-rs's default trust list,
        // but sign-time validation does not require trust (post-sign
        // reader-side verification would). Leave `None` and rely on
        // the profile-check-only guarantee.
        trust_anchor_pem: None,
    }
}

#[test]
fn sign_asset_with_signer_emits_non_empty_c2pa_manifest_for_minimal_jpeg() {
    let signer = ephemeral_signer();
    let options = default_options();

    let signed = sign_asset_with_signer(&signer, &options, "image/jpeg", MINIMAL_JPEG)
        .expect("sign_asset_with_signer must succeed with the c2pa-rs EphemeralSigner");

    assert!(
        !signed.manifest_bytes.is_empty(),
        "manifest bytes must be non-empty; got zero-length output from sign"
    );
    assert!(
        signed.manifest_bytes.len() > 64,
        "manifest bytes look truncated ({} bytes); expected the COSE + JUMBF \
         container to be at least a few hundred bytes even for a minimal asset",
        signed.manifest_bytes.len()
    );
    assert_eq!(
        signed.asset_bytes, MINIMAL_JPEG,
        "sidecar-mode asset passthrough must return identical bytes"
    );
}

#[test]
fn finalize_broadcast_signed_with_signer_writes_asset_and_manifest_pair_to_disk() {
    let tmp = TempDir::new().expect("create tempdir");
    let asset_path = tmp.path().join("live/bench/finalized.jpg");
    let manifest_path = tmp.path().join("live/bench/finalized.c2pa");

    let signer = ephemeral_signer();
    let options = default_options();

    // Init-only "broadcast": the init bytes ARE the whole asset, zero
    // media segments. That is the production shape for a broadcast
    // that disconnects before producing any data; it also keeps the
    // test's concat-path behavior exercised without needing a
    // multi-JPEG fixture (c2pa's JPEG handler needs exactly one
    // syntactically-valid JPEG and multiple concatenated JPEGs would
    // fail structural validation).
    let segment_paths: &[std::path::PathBuf] = &[];
    let signed = finalize_broadcast_signed_with_signer(
        &signer,
        &options,
        MINIMAL_JPEG,
        segment_paths,
        "image/jpeg",
        &asset_path,
        &manifest_path,
    )
    .expect("finalize_broadcast_signed_with_signer must succeed");

    let asset_on_disk = fs::read(&asset_path).expect("asset file must exist");
    let manifest_on_disk = fs::read(&manifest_path).expect("manifest file must exist");
    assert_eq!(
        asset_on_disk, signed.asset_bytes,
        "on-disk asset must match SignedAsset.asset_bytes"
    );
    assert_eq!(
        manifest_on_disk, signed.manifest_bytes,
        "on-disk manifest must match SignedAsset.manifest_bytes"
    );
    assert_eq!(
        signed.asset_bytes, MINIMAL_JPEG,
        "init-only finalize must round-trip the input bytes"
    );
    assert!(
        !signed.manifest_bytes.is_empty() && signed.manifest_bytes.len() > 64,
        "manifest must be a real COSE+JUMBF container, not a stub"
    );
}

#[test]
fn sign_asset_bytes_reports_c2pa_error_on_missing_cert_file() {
    let tmp = TempDir::new().expect("create tempdir");
    let config = C2paConfig {
        signing_cert_path: tmp.path().join("does-not-exist.pem"),
        private_key_path: tmp.path().join("does-not-exist.key"),
        assertion_creator: "missing-file test".to_string(),
        signing_alg: C2paSigningAlg::Es256,
        timestamp_authority_url: None,
        trust_anchor_pem: None,
    };

    let err = sign_asset_bytes(&config, "image/jpeg", MINIMAL_JPEG)
        .expect_err("missing cert must surface as ArchiveError::Io");
    match err {
        lvqr_archive::ArchiveError::Io(msg) => {
            assert!(
                msg.contains("does-not-exist.pem"),
                "error should name the missing path; got: {msg}"
            );
        }
        other => panic!("expected ArchiveError::Io, got {other:?}"),
    }
}
