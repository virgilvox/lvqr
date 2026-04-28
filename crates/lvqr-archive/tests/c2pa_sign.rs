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

use std::sync::Arc;

use lvqr_archive::provenance::{
    C2paConfig, C2paSignerSource, C2paSigningAlg, SignOptions, finalize_broadcast_signed,
    finalize_broadcast_signed_with_signer, sign_asset_bytes, sign_asset_with_signer,
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
fn sign_asset_bytes_with_custom_signer_source_delegates_to_ephemeral_signer() {
    // Session 94 B3: the high-level `sign_asset_bytes` path must
    // accept a pre-constructed c2pa::Signer via C2paSignerSource::
    // Custom so integration tests (and operators with HSM-/KMS-
    // backed keys) do not need to serialize PEMs to disk. This
    // exercises the enum branching inside sign_asset_bytes without
    // going through the lower-level sign_asset_with_signer primitive.
    let config = C2paConfig {
        signer_source: C2paSignerSource::Custom(Arc::new(ephemeral_signer())),
        assertion_creator: "Custom source test".to_string(),
        trust_anchor_pem: None,
    };

    let signed = sign_asset_bytes(&config, "image/jpeg", MINIMAL_JPEG)
        .expect("sign_asset_bytes via Custom signer source must succeed");
    assert_eq!(signed.asset_bytes, MINIMAL_JPEG);
    assert!(signed.manifest_bytes.len() > 64);
}

#[test]
fn finalize_broadcast_signed_with_custom_signer_source_writes_pair_to_disk() {
    // Mirror of the _with_signer test above but routed through the
    // high-level `finalize_broadcast_signed(&C2paConfig, ..)` entry
    // point. This is the call shape that
    // `lvqr_cli::archive::BroadcasterArchiveIndexer::drain` invokes
    // on broadcast-end; cover it here so the drain integration is
    // unit-regression-protected regardless of the E2E layer.
    let tmp = TempDir::new().expect("create tempdir");
    let asset_path = tmp.path().join("live/bench/finalized.jpg");
    let manifest_path = tmp.path().join("live/bench/finalized.c2pa");
    let config = C2paConfig {
        signer_source: C2paSignerSource::Custom(Arc::new(ephemeral_signer())),
        assertion_creator: "finalize Custom test".to_string(),
        trust_anchor_pem: None,
    };

    let segment_paths: &[std::path::PathBuf] = &[];
    let signed = finalize_broadcast_signed(
        &config,
        MINIMAL_JPEG,
        segment_paths,
        "image/jpeg",
        &asset_path,
        &manifest_path,
    )
    .expect("finalize_broadcast_signed via Custom signer source must succeed");

    assert_eq!(fs::read(&asset_path).unwrap(), signed.asset_bytes);
    assert_eq!(fs::read(&manifest_path).unwrap(), signed.manifest_bytes);
    assert_eq!(signed.asset_bytes, MINIMAL_JPEG);
    assert!(signed.manifest_bytes.len() > 64);
}

/// Sign an asset, verify the manifest validates against the
/// signed bytes (round-trip), then mutate one byte of the asset
/// and assert the same manifest no longer validates. Closes the
/// audit gap that the existing c2pa-sign tests only assert
/// "manifest_bytes is non-empty"; the headline provenance claim
/// of LVQR is *integrity over time*, which requires the signed
/// asset + manifest pair to actually fail validation under
/// post-signing tampering.
///
/// Uses the same `c2pa::Reader` shape as the production verify
/// route at `crates/lvqr-cli/src/archive.rs::handle_verify` --
/// `Reader::from_context(Context::new()).with_manifest_data_and_stream`
/// then read `reader.validation_state()`, which is the enum the
/// verify route stringifies for the `/playback/verify/{broadcast}`
/// JSON response. A clean signed pair must return
/// `ValidationState::Valid`; a tampered pair must return
/// `ValidationState::Invalid` (or fail to parse outright).
#[test]
fn signed_asset_bytes_round_trip_validates_clean_and_rejects_one_byte_tamper() {
    let signer = ephemeral_signer();
    let options = default_options();

    let signed = sign_asset_with_signer(&signer, &options, "image/jpeg", MINIMAL_JPEG)
        .expect("sign_asset_with_signer must succeed");

    // 1. Clean round-trip: the signed manifest must validate
    //    against the un-mutated asset bytes as Valid (or Trusted
    //    if the operator wired a trust anchor; the EphemeralSigner
    //    CA is not in c2pa-rs's default trust list so the
    //    expectation is Valid). If this fails, the test harness
    //    itself is wrong -- we'd be unable to tell apart "tamper
    //    detected" from "library never validates anything".
    let clean_reader = c2pa::Reader::from_context(c2pa::Context::new())
        .with_manifest_data_and_stream(
            &signed.manifest_bytes,
            "image/jpeg",
            std::io::Cursor::new(&signed.asset_bytes),
        )
        .expect("clean manifest+asset must parse");
    let clean_state = clean_reader.validation_state();
    assert!(
        matches!(
            clean_state,
            c2pa::ValidationState::Valid | c2pa::ValidationState::Trusted
        ),
        "clean signed asset must validate as Valid or Trusted; got {clean_state:?}",
    );

    // 2. Tampered round-trip: flip one byte well inside the JPEG
    //    payload (after the JFIF APP0 header, before the EOI
    //    marker). The asset hash must no longer match the
    //    manifest's recorded hash, so validation must downgrade
    //    to Invalid (or the parse must fail outright with an
    //    integrity error).
    let mut tampered = signed.asset_bytes.clone();
    let tamper_index = tampered.len() / 2;
    tampered[tamper_index] ^= 0xFF;
    assert_ne!(
        tampered, signed.asset_bytes,
        "tampered bytes must differ from clean (sanity check on the mutation)"
    );

    match c2pa::Reader::from_context(c2pa::Context::new()).with_manifest_data_and_stream(
        &signed.manifest_bytes,
        "image/jpeg",
        std::io::Cursor::new(&tampered),
    ) {
        Err(_) => {
            // c2pa::Reader rejected outright (e.g., asset hash
            // mismatch surfaced as a parse-time error). Tamper
            // detected.
        }
        Ok(reader) => {
            // c2pa::Reader parsed but the validation_state must
            // downgrade to Invalid -- the asset's binary hash no
            // longer matches the recorded one in the manifest.
            let state = reader.validation_state();
            assert!(
                matches!(state, c2pa::ValidationState::Invalid),
                "tampered asset must report ValidationState::Invalid; \
                 got {state:?} (validator may be broken if Valid/Trusted)",
            );
        }
    }
}

#[test]
fn sign_asset_bytes_reports_c2pa_error_on_missing_cert_file() {
    let tmp = TempDir::new().expect("create tempdir");
    let config = C2paConfig {
        signer_source: C2paSignerSource::CertKeyFiles {
            signing_cert_path: tmp.path().join("does-not-exist.pem"),
            private_key_path: tmp.path().join("does-not-exist.key"),
            signing_alg: C2paSigningAlg::Es256,
            timestamp_authority_url: None,
        },
        assertion_creator: "missing-file test".to_string(),
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
