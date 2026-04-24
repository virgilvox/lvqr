//! End-to-end fixture for the `--c2pa-signing-cert` +
//! `--c2pa-signing-key` CLI path. Tier 4 item 4.3 / session 121.
//!
//! This test was initially drafted in session 120 but reverted
//! after c2pa-rs rejected the rcgen-generated cert chain with
//! "the certificate is invalid" at sign time. Session 121
//! audited c2pa-rs's `check_certificate_profile` implementation
//! at `crates/c2pa-0.80.0/src/crypto/cose/certificate_profile.rs`
//! and traced the failure to a single missing extension on the
//! leaf cert: the `AuthorityKeyIdentifier` (AKI). rcgen's
//! `CertificateParams::default()` sets
//! `use_authority_key_identifier_extension: false` (see
//! `rcgen-0.13.2/src/certificate.rs:111`); without it the
//! `aki_good` flag in c2pa-rs's validation loop never flips
//! true and the final `aki_good && ski_good && key_usage_good
//! && extended_key_usage_good && handled_all_critical` check
//! fails. Setting the flag to `true` on the leaf params flips
//! rcgen into emitting AKI + matching SKI on the issuer, which
//! is what c2pa-rs's profile check actually needs.
//!
//! Shape:
//!
//! 1. Mint a CA + leaf + key triple in-process via [`rcgen`].
//!    CA is self-signed with `is_ca = Ca(Unconstrained)` +
//!    `KeyCertSign + CrlSign` key usage. Leaf is signed by the
//!    CA with `DigitalSignature` key usage, `EmailProtection`
//!    extended key usage, and -- the session-121 fix --
//!    `use_authority_key_identifier_extension = true` so the
//!    AKI extension actually lands on the DER output. Both
//!    keys are ECDSA P-256 (rcgen's default), matching c2pa-rs's
//!    `Es256` signing-alg variant.
//! 2. Write the leaf chain (leaf first, then CA) as
//!    `<tmp>/signing.pem` and the leaf's PKCS#8 private key as
//!    `<tmp>/signing.key`. These are the two files an operator
//!    would hand to `--c2pa-signing-cert` +
//!    `--c2pa-signing-key`.
//! 3. Boot a `TestServer` with an archive dir + a `C2paConfig`
//!    whose `signer_source` is
//!    `C2paSignerSource::CertKeyFiles` pointing at the two
//!    files. `TestServerConfig` accepts a programmatic
//!    `C2paConfig` via `with_c2pa`, which is what the CLI
//!    would construct from the parsed flags in
//!    `build_c2pa_config`.
//! 4. Publish two keyframes via a real `rml_rtmp` client + drop
//!    the stream. That triggers the archive indexer's drain-
//!    termination C2PA finalize path, which reads the same
//!    PEMs from disk, constructs a c2pa-rs signer, and writes
//!    `finalized.mp4` + `finalized.c2pa` next to the segment
//!    files.
//! 5. `GET /playback/verify/live/dvr` returns the JSON shape:
//!    `valid = true`, `validation_state = "Valid"` (crypto
//!    integrity -- our test CA is not in c2pa-rs's default
//!    trust list, matching the `c2pa_verify_e2e.rs` ephemeral
//!    posture), non-empty `signer`, empty `errors`.
//!
//! This is the first happy-path test exercising the
//! `CertKeyFiles` signer source end to end;
//! `c2pa_verify_e2e.rs` covers the programmatic `Custom(Arc<dyn
//! Signer>)` variant via `c2pa::EphemeralSigner`. Together they
//! lock the two operator-facing signer code paths.

#![cfg(feature = "c2pa")]

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use lvqr_archive::provenance::{C2paConfig, C2paSignerSource, C2paSigningAlg};
use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};
use lvqr_test_utils::rtmp::rtmp_client_handshake;
use lvqr_test_utils::{TestServer, TestServerConfig};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);
const FINALIZE_POLL_BUDGET: Duration = Duration::from_secs(10);
const FINALIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);

// =====================================================================
// rcgen-based test PKI. In-process cert minting avoids shipping
// expiring on-disk fixtures in the repo.
// =====================================================================

/// Build a minimum-viable CA + leaf + key triple that c2pa-rs's
/// `check_certificate_profile` accepts. Writes three files into
/// `tmp`:
///
/// * `signing.pem` -- leaf cert followed by CA cert, PEM-encoded.
///   Leaf-first is the convention operators follow when handing
///   the chain to `--c2pa-signing-cert`.
/// * `signing.key` -- leaf PKCS#8 private key PEM.
/// * `ca.pem` -- CA cert alone, PEM-encoded. Returned so callers
///   can optionally feed it to `--c2pa-trust-anchor` to flip
///   `validation_state` from `"Valid"` to `"Trusted"` in a
///   future variant of the test.
///
/// The session-121 fix over the session-120 draft: leaf sets
/// `use_authority_key_identifier_extension = true`. Without it
/// rcgen elides the AKI extension and c2pa-rs's validation
/// rejects the cert as "invalid" even though every documented
/// EKU / KU / CA-chain requirement is otherwise met.
fn mint_c2pa_test_pki(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    // CA: self-signed, key-cert-sign + CRL-sign key usages, CA
    // basic-constraint. Subject is arbitrary but helpful in logs.
    let ca_key = KeyPair::generate().expect("rcgen: generate CA key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("rcgen: CA params");
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "LVQR Test CA");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params.self_signed(&ca_key).expect("rcgen: self-sign CA");

    // Leaf: signed by the CA. `is_ca = NoCa` because c2pa-rs's
    // `check_end_entity_certificate_profile` explicitly rejects
    // CA-flagged leaves. KU `digital_signature` is required;
    // `key_cert_sign` is forbidden on EE (would trip the
    // `ku.key_cert_sign() && !tbscert.is_ca()` branch). EKU
    // `emailProtection` is on c2pa-rs's default allow-list in
    // `has_allowed_eku`.
    //
    // The session-121 audit found TWO issues with the initial
    // draft:
    //
    // 1. Missing `AuthorityKeyIdentifier` extension. rcgen's
    //    `CertificateParams::default()` sets
    //    `use_authority_key_identifier_extension: false`; without
    //    the flag flipped true the leaf DER has no AKI, and
    //    c2pa-rs's `check_certificate_profile` aki_good flag
    //    never flips true, so the final gate fails with a
    //    generic "certificate is invalid" error that does not
    //    name the missing AKI.
    //
    // 2. Missing `Organization` (O) attribute in the subject DN.
    //    c2pa-rs's COSE verifier (`crypto/cose/verifier.rs:159`)
    //    calls `sign_cert.subject().iter_organization().last()`
    //    to populate `CertificateInfo::issuer_org` AFTER the
    //    signature itself has validated successfully. If the
    //    subject has no O attribute the extraction returns
    //    `MissingSigningCertificateChain`, which claim.rs:3023
    //    folds into the generic "claim signature is not valid"
    //    failure with a NULL signer in the verify response.
    //    The signature itself is fine; the subject shape is
    //    what trips the check.
    let leaf_key = KeyPair::generate().expect("rcgen: generate leaf key");
    let mut leaf_params = CertificateParams::new(Vec::<String>::new()).expect("rcgen: leaf params");
    let mut leaf_dn = DistinguishedName::new();
    leaf_dn.push(DnType::CommonName, "lvqr test signer");
    leaf_dn.push(DnType::OrganizationName, "LVQR Test Operator");
    leaf_params.distinguished_name = leaf_dn;
    leaf_params.is_ca = IsCa::NoCa;
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::EmailProtection];
    leaf_params.use_authority_key_identifier_extension = true;
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .expect("rcgen: CA-sign leaf");

    // Write the three PEMs.
    let leaf_pem = leaf_cert.pem();
    let ca_pem = ca_cert.pem();
    let chain_pem = format!("{leaf_pem}{ca_pem}");
    let key_pem = leaf_key.serialize_pem();

    let cert_path = tmp.join("signing.pem");
    let key_path = tmp.join("signing.key");
    let ca_path = tmp.join("ca.pem");
    std::fs::write(&cert_path, chain_pem).expect("write signing.pem");
    std::fs::write(&key_path, key_pem).expect("write signing.key");
    std::fs::write(&ca_path, ca_pem).expect("write ca.pem");
    (cert_path, key_path, ca_path)
}

// =====================================================================
// RTMP client (mirror c2pa_verify_e2e.rs). FLV + HTTP helpers now
// live in `lvqr_test_utils::flv` + `lvqr_test_utils::http`.
// =====================================================================

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with(
        addr,
        path,
        HttpGetOptions {
            timeout: TIMEOUT,
            ..Default::default()
        },
    )
    .await
}

async fn send_results(stream: &mut TcpStream, results: &[ClientSessionResult]) {
    for result in results {
        if let ClientSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await.unwrap();
        }
    }
}

async fn send_result(stream: &mut TcpStream, result: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = result {
        stream.write_all(&packet.bytes).await.unwrap();
    }
}

async fn read_until<F>(stream: &mut TcpStream, session: &mut ClientSession, predicate: F)
where
    F: Fn(&ClientSessionEvent) -> bool,
{
    let mut buf = vec![0u8; 65536];
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let n = match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => panic!("server closed connection unexpectedly"),
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => panic!("timed out waiting for expected RTMP event"),
        };
        let results = session.handle_input(&buf[..n]).unwrap();
        for result in results {
            match result {
                ClientSessionResult::OutboundResponse(packet) => {
                    stream.write_all(&packet.bytes).await.unwrap();
                }
                ClientSessionResult::RaisedEvent(ref event) if predicate(event) => {
                    return;
                }
                _ => {}
            }
        }
    }
}

async fn connect_and_publish(addr: SocketAddr, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .unwrap()
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial_results) = ClientSession::new(config).unwrap();
    send_results(&mut stream, &initial_results).await;
    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect_result = session.request_connection(app.to_string()).unwrap();
    send_result(&mut stream, &connect_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

async fn publish_two_keyframes(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    let seq = flv_video_seq_header();
    let r = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let r = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let kf1 = flv_video_nalu(true, 0, &nalu);
    let r = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &r).await;

    (rtmp_stream, session)
}

async fn wait_for_finalize(manifest_path: &Path) {
    let deadline = tokio::time::Instant::now() + FINALIZE_POLL_BUDGET;
    loop {
        if manifest_path.exists() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "finalize manifest did not appear at {} within {:?}",
                manifest_path.display(),
                FINALIZE_POLL_BUDGET
            );
        }
        tokio::time::sleep(FINALIZE_POLL_INTERVAL).await;
    }
}

// =====================================================================
// The test: on-disk CertKeyFiles signer source, end to end.
// =====================================================================

#[tokio::test]
async fn certkeyfiles_signer_source_yields_valid_c2pa_manifest() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let pki_tmp = TempDir::new().expect("pki tmp");
    let (cert_path, key_path, _ca_path) = mint_c2pa_test_pki(pki_tmp.path());

    let archive_tmp = TempDir::new().expect("archive tmp");
    let archive_path = archive_tmp.path().to_path_buf();

    let c2pa_config = C2paConfig {
        signer_source: C2paSignerSource::CertKeyFiles {
            signing_cert_path: cert_path.clone(),
            private_key_path: key_path.clone(),
            signing_alg: C2paSigningAlg::Es256,
            timestamp_authority_url: None,
        },
        assertion_creator: "LVQR Session 121 E2E".to_string(),
        // Leaving trust_anchor_pem at None mirrors the
        // c2pa_verify_e2e.rs posture: our test CA is not in
        // c2pa-rs's default trust list, so validation_state
        // comes back as `"Valid"` (crypto integrity) rather
        // than `"Trusted"`. Supplying the CA PEM here would
        // flip it to `"Trusted"`; a follow-up variant of this
        // test could cover that path.
        trust_anchor_pem: None,
    };

    let server = TestServer::start(
        TestServerConfig::default()
            .with_archive_dir(&archive_path)
            .with_c2pa(c2pa_config),
    )
    .await
    .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (rtmp_stream, rtmp_session) = publish_two_keyframes(rtmp_addr, "live", "dvr").await;

    // Let the bridge drain every fragment to disk before the
    // drop-triggered finalize runs. 500 ms matches the
    // c2pa_verify_e2e / rtmp_archive_e2e pattern.
    tokio::time::sleep(Duration::from_millis(500)).await;

    drop(rtmp_stream);
    drop(rtmp_session);

    let manifest_path = archive_path.join("live/dvr/0.mp4/finalized.c2pa");
    let asset_path = archive_path.join("live/dvr/0.mp4/finalized.mp4");
    wait_for_finalize(&manifest_path).await;
    assert!(
        asset_path.exists(),
        "finalize manifest landed but finalized.mp4 is missing at {}",
        asset_path.display()
    );

    let resp = http_get(admin_addr, "/playback/verify/live/dvr").await;
    assert_eq!(
        resp.status,
        200,
        "GET /playback/verify/live/dvr returned {} with body {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    let body = std::str::from_utf8(&resp.body).expect("verify body utf-8");
    eprintln!("--- /playback/verify/live/dvr ---\n{body}\n--- end ---");

    let v: serde_json::Value = serde_json::from_str(body).expect("verify body is JSON");
    assert_eq!(
        v["valid"].as_bool(),
        Some(true),
        "expected valid=true; manifest failed verification: {body}"
    );
    assert_eq!(
        v["validation_state"].as_str(),
        Some("Valid"),
        "expected validation_state=Valid (test CA not in c2pa-rs trust list), got {body}"
    );
    let signer = v["signer"].as_str().expect("signer field is a string");
    assert!(
        !signer.is_empty(),
        "signer string must be non-empty; manifest has no issuer"
    );
    let errors = v["errors"].as_array().expect("errors field is array");
    assert!(errors.is_empty(), "expected empty errors array; got {errors:?}");

    server.shutdown().await.expect("shutdown");
}

// =====================================================================
// Second happy-path test: openssl-generated cert material.
// =====================================================================
//
// The rcgen-based test above proves the `CertKeyFiles` wire works
// against an in-process PKI. The typical operator path, though, is
// `openssl` CLI -- the `examples/tier4-demos/demo-01.sh` script
// uses exactly these commands when its `--c2pa` opt-in is on.
// Verifying that openssl output also passes c2pa-rs's profile
// checks locks the demo's code path into CI.
//
// Skips gracefully when `openssl` is not on $PATH so the default
// gate stays portable; c2pa / rcgen tests continue to cover the
// logical end-to-end surface.

fn have_openssl() -> bool {
    std::process::Command::new("openssl")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_or_panic(desc: &str, cmd: &mut std::process::Command) {
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("openssl cmd spawn failed ({desc}): {e}"));
    if !output.status.success() {
        panic!(
            "openssl cmd failed ({desc}): status={:?} stdout=<<{}>> stderr=<<{}>>",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// Mint a CA + leaf + PKCS#8 key triple via `openssl` shelling
/// out. Mirrors the `examples/tier4-demos/demo-01.sh` recipe
/// verbatim so demo users and CI see the same cert material
/// shape. The resulting chain passes c2pa-rs's
/// `check_certificate_profile` and
/// `check_end_entity_certificate_profile` because the v3
/// extensions match rcgen's verified-working layout:
/// critical `BasicConstraints: CA:FALSE`, critical `KeyUsage:
/// digitalSignature`, `ExtendedKeyUsage: emailProtection`,
/// `SubjectKeyIdentifier: hash`, `AuthorityKeyIdentifier:
/// keyid:always`, plus CN + O in the subject DN.
fn mint_c2pa_test_pki_openssl(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let ca_key = tmp.join("ca.key");
    let ca_pem = tmp.join("ca.pem");
    let ca_cfg = tmp.join("ca.cfg");
    let leaf_key_sec1 = tmp.join("leaf.sec1.key");
    let leaf_key_pkcs8 = tmp.join("signing.key");
    let leaf_csr = tmp.join("leaf.csr");
    let leaf_pem = tmp.join("leaf.pem");
    let leaf_cfg = tmp.join("leaf.cfg");
    let chain_pem = tmp.join("signing.pem");

    // CA config + key + self-signed cert.
    std::fs::write(
        &ca_cfg,
        "[req]\n\
         distinguished_name = req_dn\n\
         x509_extensions = v3_ca\n\
         prompt = no\n\
         [req_dn]\n\
         CN = LVQR openssl Test CA\n\
         O = LVQR openssl Test\n\
         [v3_ca]\n\
         basicConstraints = critical, CA:TRUE\n\
         keyUsage = critical, keyCertSign, cRLSign\n\
         subjectKeyIdentifier = hash\n",
    )
    .expect("write ca.cfg");

    run_or_panic(
        "gen CA key",
        std::process::Command::new("openssl")
            .args(["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out"])
            .arg(&ca_key),
    );

    run_or_panic(
        "self-sign CA",
        std::process::Command::new("openssl")
            .args(["req", "-x509", "-new", "-key"])
            .arg(&ca_key)
            .args(["-out"])
            .arg(&ca_pem)
            .args(["-days", "30", "-config"])
            .arg(&ca_cfg),
    );

    // Leaf key (SEC1) + convert to PKCS#8 (c2pa-rs reads PKCS#8).
    run_or_panic(
        "gen leaf key",
        std::process::Command::new("openssl")
            .args(["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out"])
            .arg(&leaf_key_sec1),
    );
    run_or_panic(
        "sec1 -> pkcs8",
        std::process::Command::new("openssl")
            .args(["pkcs8", "-topk8", "-nocrypt", "-in"])
            .arg(&leaf_key_sec1)
            .args(["-out"])
            .arg(&leaf_key_pkcs8),
    );

    // CSR: subject must include CN + O (session-121 audit note
    // on verifier.rs:159 -- missing O yields a null signer +
    // false claim-signature validity).
    run_or_panic(
        "gen leaf csr",
        std::process::Command::new("openssl")
            .args(["req", "-new", "-key"])
            .arg(&leaf_key_sec1)
            .args(["-out"])
            .arg(&leaf_csr)
            .args(["-subj", "/CN=lvqr openssl demo signer/O=LVQR openssl Demo Operator"]),
    );

    // Leaf extensions. AKI:keyid:always is the session-121
    // audit fix -- without AKI c2pa-rs's aki_good flag never
    // flips true and the cert-profile check rejects with a
    // generic "certificate is invalid".
    std::fs::write(
        &leaf_cfg,
        "basicConstraints = critical, CA:FALSE\n\
         keyUsage = critical, digitalSignature\n\
         extendedKeyUsage = emailProtection\n\
         subjectKeyIdentifier = hash\n\
         authorityKeyIdentifier = keyid:always\n",
    )
    .expect("write leaf.cfg");

    run_or_panic(
        "CA-sign leaf",
        std::process::Command::new("openssl")
            .args(["x509", "-req", "-in"])
            .arg(&leaf_csr)
            .args(["-CA"])
            .arg(&ca_pem)
            .args(["-CAkey"])
            .arg(&ca_key)
            .args(["-CAcreateserial", "-out"])
            .arg(&leaf_pem)
            .args(["-days", "30", "-extfile"])
            .arg(&leaf_cfg),
    );

    // Leaf cert followed by CA cert, the order operators and
    // c2pa-rs expect when walking the chain PEM.
    let leaf_bytes = std::fs::read(&leaf_pem).expect("read leaf.pem");
    let ca_bytes = std::fs::read(&ca_pem).expect("read ca.pem");
    let mut chain = leaf_bytes;
    chain.extend_from_slice(&ca_bytes);
    std::fs::write(&chain_pem, chain).expect("write signing.pem");

    (chain_pem, leaf_key_pkcs8, ca_pem)
}

/// Same end-to-end flow as the rcgen test above, but with cert
/// material minted via the `openssl` CLI. Locks the
/// `examples/tier4-demos/demo-01.sh` `--c2pa` code path into CI
/// so a future rcgen/openssl divergence in rust-crypto land does
/// not silently break the demo.
#[tokio::test]
async fn openssl_generated_certkeyfiles_also_yields_valid_manifest() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    if !have_openssl() {
        eprintln!("openssl not on PATH; skipping openssl_generated_certkeyfiles_also_yields_valid_manifest");
        return;
    }

    let pki_tmp = TempDir::new().expect("pki tmp");
    let (cert_path, key_path, _ca_path) = mint_c2pa_test_pki_openssl(pki_tmp.path());

    let archive_tmp = TempDir::new().expect("archive tmp");
    let archive_path = archive_tmp.path().to_path_buf();

    let c2pa_config = C2paConfig {
        signer_source: C2paSignerSource::CertKeyFiles {
            signing_cert_path: cert_path.clone(),
            private_key_path: key_path.clone(),
            signing_alg: C2paSigningAlg::Es256,
            timestamp_authority_url: None,
        },
        assertion_creator: "LVQR Session 121 E2E (openssl)".to_string(),
        trust_anchor_pem: None,
    };

    let server = TestServer::start(
        TestServerConfig::default()
            .with_archive_dir(&archive_path)
            .with_c2pa(c2pa_config),
    )
    .await
    .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (rtmp_stream, rtmp_session) = publish_two_keyframes(rtmp_addr, "live", "dvr").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(rtmp_stream);
    drop(rtmp_session);

    let manifest_path = archive_path.join("live/dvr/0.mp4/finalized.c2pa");
    wait_for_finalize(&manifest_path).await;

    let resp = http_get(admin_addr, "/playback/verify/live/dvr").await;
    assert_eq!(resp.status, 200, "verify status");
    let body = std::str::from_utf8(&resp.body).expect("verify body utf-8");
    let v: serde_json::Value = serde_json::from_str(body).expect("verify body is JSON");
    assert_eq!(
        v["valid"].as_bool(),
        Some(true),
        "openssl cert: expected valid=true; got {body}"
    );
    assert_eq!(
        v["validation_state"].as_str(),
        Some("Valid"),
        "openssl cert: expected validation_state=Valid; got {body}"
    );
    let signer = v["signer"].as_str().expect("signer field is a string");
    assert!(!signer.is_empty(), "openssl cert: signer must be non-empty");
    let errors = v["errors"].as_array().expect("errors field is array");
    assert!(errors.is_empty(), "openssl cert: expected empty errors; got {errors:?}");

    server.shutdown().await.expect("shutdown");
}
