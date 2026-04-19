//! C2PA provenance signing primitives for archived assets.
//!
//! **Tier 4 item 4.3, sessions A (session 91) + B1 (session 92).**
//! Compiled only when `lvqr-archive` is built with `--features c2pa`.
//! Pulls the `c2pa` crate (pinned 0.80; `default-features = false`,
//! `features = ["rust_native_crypto"]` at the workspace level so the
//! crypto closure stays pure-Rust and the remote-manifest HTTP stacks
//! are not in the graph).
//!
//! # What this module owns
//!
//! Session A (91) landed:
//!
//! * [`C2paConfig`]: operator-facing configuration bag -- signing cert
//!   path, private key path, creator-assertion name, signature
//!   algorithm, optional RFC 3161 timestamp authority URL, optional
//!   PEM trust anchor bundle.
//! * [`C2paSigningAlg`]: LVQR-owned enum that maps 1:1 to the c2pa-rs
//!   `SigningAlg` enum so downstream consumers do not need a direct
//!   dep on `c2pa-rs` to build a [`C2paConfig`].
//! * [`SignedAsset`]: the sign result. Keeps the asset bytes and the
//!   manifest bytes separate so the caller chooses embed vs. sidecar
//!   semantics on disk; the primitive itself runs
//!   `Builder::set_no_embed(true)` so the asset passes through
//!   unchanged.
//! * [`sign_asset_bytes`]: bytes-in / bytes-out signing primitive.
//!   Loads the cert + key PEMs from disk, wires `trust_anchor_pem`
//!   through `Context::with_settings`, constructs a `c2pa::Builder`
//!   with a minimal manifest carrying the creator assertion, signs
//!   against an in-memory cursor of the asset, and returns the
//!   `SignedAsset` pair.
//!
//! Session B1 (92) added two composition primitives that session B2
//! (93) wires into the drain-terminated finalize path:
//!
//! * [`concat_assets`]: reads a caller-supplied ordered list of paths
//!   into one `Vec<u8>`. Session B2 walks the redb segment index in
//!   `start_dts` order, collects `PathBuf`s, and feeds them to this
//!   helper to produce the bytes-to-sign. Decoupling the concat from
//!   the index walk keeps this primitive pure (testable without redb).
//! * [`write_signed_pair`]: writes a [`SignedAsset`] pair to two
//!   caller-supplied paths, creating parent directories as needed.
//!   Session B2 uses this to land
//!   `<archive>/<broadcast>/<track>/finalized.<ext>` +
//!   `<archive>/<broadcast>/<track>/finalized.c2pa` together.
//!
//! # What this module is NOT
//!
//! * Not a finalize-asset orchestrator. The archive is a stream of
//!   CMAF segments, not a single finalized MP4. Wiring the broadcast-
//!   end lifecycle hook onto `FragmentBroadcasterRegistry` +
//!   persisting init bytes on disk at first-segment-write time +
//!   invoking `concat_assets` / `sign_asset_bytes` /
//!   `write_signed_pair` from the drain task's termination path is
//!   session B2 (93)'s problem because it touches the cross-crate
//!   surface between `lvqr-fragment` / `lvqr-archive` / `lvqr-cli`.
//! * Not a c2pa reader / verifier. Session B2 adds the admin verify
//!   route + E2E that parses the manifest back.
//! * Not an operator-supplied PKI manager. The MVP accepts whatever
//!   cert the operator points at. Trust-root validation happens at
//!   read time via `c2pa::Reader`, not here.
//!
//! # Why the sign primitive takes bytes, not a path
//!
//! c2pa-rs 0.80 exposes `Builder::sign(R: Read+Seek+Send, W: Write+Read
//! +Seek+Send)` against in-memory cursors. Taking bytes lets the
//! caller decide whether the asset lives in memory (typical for
//! finalized MP4 construction, which is a concat step that already
//! holds the bytes), on disk, or behind a reader. Session B2 builds
//! the on-disk bytes via [`concat_assets`] before calling
//! [`sign_asset_bytes`]; if that buffer ever gets too large to hold
//! in memory we introduce a streaming `impl Read + Seek` variant
//! then. Today's archive segment sizes are <= 1 MiB so hundreds of
//! them fit in memory without issue.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::ArchiveError;

/// Operator-facing C2PA signing configuration. Constructed by the
/// caller (CLI config, API consumer) and passed to
/// [`sign_asset_bytes`].
#[derive(Debug, Clone)]
pub struct C2paConfig {
    /// Path to a PEM-encoded signing certificate chain. The leaf
    /// certificate MUST carry an extended key usage from c2pa-rs's
    /// allow-list (`emailProtection` 1.3.6.1.5.5.7.3.4,
    /// `documentSigning` 1.3.6.1.5.5.7.3.36, `timeStamping`
    /// 1.3.6.1.5.5.7.3.8, `OCSPSigning` 1.3.6.1.5.5.7.3.9, MS C2PA
    /// 1.3.6.1.4.1.311.76.59.1.9, or C2PA 1.3.6.1.4.1.62558.2.1) plus
    /// the `digitalSignature` key usage bit and must chain to a CA
    /// (self-signed leaves are rejected per C2PA spec §14.5.1). The
    /// PEM concatenates the leaf cert first, then the CA.
    pub signing_cert_path: PathBuf,
    /// Path to a PEM-encoded PKCS#8 private key matching the leaf
    /// cert's subject public key.
    pub private_key_path: PathBuf,
    /// Human-readable creator name embedded in the
    /// `stds.schema-org.CreativeWork` author assertion on every
    /// signed asset. Typical value is the operator's org name or a
    /// broadcast identifier.
    pub assertion_creator: String,
    /// Digital signature algorithm. Must match the private key:
    /// `Es256` + ECDSA P-256 key, `Ed25519` + Ed25519 key, etc.
    pub signing_alg: C2paSigningAlg,
    /// Optional RFC 3161 Timestamp Authority URL. When set, the
    /// signer contacts the TSA during `sign` to embed a timestamp
    /// countersignature in the manifest so the signing moment
    /// survives cert expiry. `None` leaves the manifest without a
    /// trusted timestamp -- acceptable for internal archives but not
    /// for evidentiary use.
    pub timestamp_authority_url: Option<String>,
    /// Optional PEM-encoded trust anchor bundle. Surfaces directly to
    /// `c2pa::Context::with_settings({"trust": {"user_anchors": ...}})`
    /// so c2pa-rs's chain validator accepts certs signed by this CA.
    /// Required in any deployment that uses a private CA (the c2pa-rs
    /// default trust list is the public C2PA conformance list); leave
    /// `None` only when signing with a cert chained to a public C2PA
    /// trust anchor.
    pub trust_anchor_pem: Option<String>,
}

/// LVQR-owned signing algorithm enum. 1:1 with `c2pa::SigningAlg`; the
/// mapping is an implementation detail so upstream API churn in
/// `c2pa::SigningAlg` does not leak into lvqr-archive's public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum C2paSigningAlg {
    /// ECDSA with SHA-256.
    Es256,
    /// ECDSA with SHA-384.
    Es384,
    /// ECDSA with SHA-512.
    Es512,
    /// RSASSA-PSS using SHA-256 + MGF1 SHA-256.
    Ps256,
    /// RSASSA-PSS using SHA-384 + MGF1 SHA-384.
    Ps384,
    /// RSASSA-PSS using SHA-512 + MGF1 SHA-512.
    Ps512,
    /// Edwards-curve DSA on Curve25519.
    Ed25519,
}

impl C2paSigningAlg {
    fn to_c2pa(self) -> c2pa::SigningAlg {
        match self {
            Self::Es256 => c2pa::SigningAlg::Es256,
            Self::Es384 => c2pa::SigningAlg::Es384,
            Self::Es512 => c2pa::SigningAlg::Es512,
            Self::Ps256 => c2pa::SigningAlg::Ps256,
            Self::Ps384 => c2pa::SigningAlg::Ps384,
            Self::Ps512 => c2pa::SigningAlg::Ps512,
            Self::Ed25519 => c2pa::SigningAlg::Ed25519,
        }
    }
}

/// Sign result. The asset bytes pass through unchanged (the primitive
/// runs in sidecar mode via `Builder::set_no_embed(true)`); the
/// manifest bytes are the COSE-signed JUMBF container the caller
/// stores alongside the asset (conventionally at
/// `<asset>.c2pa`).
#[derive(Debug, Clone)]
pub struct SignedAsset {
    /// The asset bytes. Identical to the input in sidecar mode;
    /// retained on the return type so the caller does not have to
    /// keep the original buffer alive when persisting the pair.
    pub asset_bytes: Vec<u8>,
    /// The signed manifest bytes. Write to `<asset>.c2pa` (or the
    /// layout of the caller's choice); parsed back via
    /// `c2pa::Reader::from_manifest_data_and_stream`.
    pub manifest_bytes: Vec<u8>,
}

/// Low-level sign options: the subset of [`C2paConfig`] that is
/// independent of PEM paths + signing algorithm. [`sign_asset_with_signer`]
/// takes these alongside any [`c2pa::Signer`] impl so tests can use the
/// in-process [`c2pa::EphemeralSigner`] (which produces C2PA-spec-
/// compliant certs in-memory, no disk PEMs) and operators with bespoke
/// key-storage backends (HSMs, KMS, etc.) can bring their own signer.
/// Production config flows through [`sign_asset_bytes`] which reads
/// PEMs from disk and constructs a signer via
/// `c2pa::create_signer::from_keys`.
#[derive(Debug, Clone)]
pub struct SignOptions {
    /// Human-readable creator name embedded in the
    /// `stds.schema-org.CreativeWork` author assertion. Mirrors
    /// [`C2paConfig::assertion_creator`].
    pub assertion_creator: String,
    /// Optional PEM trust anchor bundle. Mirrors
    /// [`C2paConfig::trust_anchor_pem`]. When set, wired through
    /// `c2pa::Context::with_settings({"trust": {"user_anchors": ...}})`
    /// so c2pa-rs's chain validator accepts certs signed by this CA.
    pub trust_anchor_pem: Option<String>,
}

impl SignOptions {
    fn from_config(config: &C2paConfig) -> Self {
        Self {
            assertion_creator: config.assertion_creator.clone(),
            trust_anchor_pem: config.trust_anchor_pem.clone(),
        }
    }
}

/// Sign an asset with the caller-supplied [`c2pa::Signer`], returning
/// the (unchanged) asset bytes plus the sidecar C2PA manifest. Lower-
/// level variant of [`sign_asset_bytes`] -- the path-based flow reads
/// PEMs then delegates here.
///
/// Session 93 B2 added this signature to let tests use
/// [`c2pa::EphemeralSigner`] (which generates C2PA-spec-compliant
/// ephemeral certs in memory) without the PEM-fixture problem that
/// blocked the happy-path test through sessions 91-92. Operators
/// running with HSM-backed or KMS-backed keys can also call this
/// primitive directly with their own [`c2pa::Signer`] implementation.
///
/// `asset_format` is an IANA MIME type (`"image/jpeg"`, `"video/mp4"`,
/// etc.) or a c2pa-rs known extension alias. The handler is selected
/// by `c2pa-rs`'s `asset_handlers` dispatch; unsupported formats
/// return [`ArchiveError::C2pa`].
///
/// The manifest carries:
/// * A `ClaimGeneratorInfo` naming `"lvqr"` + this crate's version.
/// * One `stds.schema-org.CreativeWork` assertion with a single
///   `Person` author whose `name` is `options.assertion_creator`.
/// * No ingredients. Ingredient chains are meaningful when an asset
///   is derived from another C2PA-signed asset; an archive's source
///   is an RTMP ingest which has no upstream manifest, so there is
///   nothing to ingredient.
pub fn sign_asset_with_signer(
    signer: &dyn c2pa::Signer,
    options: &SignOptions,
    asset_format: &str,
    asset_bytes: &[u8],
) -> Result<SignedAsset, ArchiveError> {
    // Minimal manifest. Built via serde_json::json! so the operator-
    // supplied creator name is JSON-escaped correctly; embedding it via
    // `format!` would break on any creator containing `"` or `\`.
    let manifest_json = serde_json::json!({
        "claim_generator_info": [{
            "name": "lvqr",
            "version": env!("CARGO_PKG_VERSION"),
        }],
        "format": asset_format,
        "assertions": [{
            "label": "stds.schema-org.CreativeWork",
            "data": {
                "@context": "http://schema.org/",
                "@type": "CreativeWork",
                "author": [{
                    "@type": "Person",
                    "name": options.assertion_creator,
                }],
            },
        }],
    })
    .to_string();

    // `Builder::from_json` was deprecated in c2pa-rs 0.80 in favor of
    // `Builder::from_context(ctx).with_definition(json)` so the manifest
    // definition is carried alongside a Context rather than through
    // thread-local settings. We construct a per-call `Context::new()`;
    // if the operator supplied a trust anchor PEM, wire it through
    // `with_settings({"trust": {"user_anchors": ...}})` so c2pa-rs's
    // chain validator treats the custom CA as a trust root. Without
    // this, c2pa-rs's default trust list (the public C2PA conformance
    // roots) rejects any cert signed by a private CA.
    let context = if let Some(anchor_pem) = options.trust_anchor_pem.as_deref() {
        let settings_json = serde_json::json!({
            "trust": {
                "user_anchors": anchor_pem,
            },
        })
        .to_string();
        c2pa::Context::new()
            .with_settings(settings_json.as_str())
            .map_err(|e| ArchiveError::C2pa(format!("context with_settings: {e}")))?
    } else {
        c2pa::Context::new()
    };
    let mut builder = c2pa::Builder::from_context(context)
        .with_definition(manifest_json.as_str())
        .map_err(|e| ArchiveError::C2pa(format!("builder with_definition: {e}")))?;
    builder.set_intent(c2pa::BuilderIntent::Edit);
    builder.set_no_embed(true);

    let mut source = Cursor::new(asset_bytes.to_vec());
    let mut dest = Cursor::new(Vec::<u8>::new());
    let manifest_bytes = builder
        .sign(signer, asset_format, &mut source, &mut dest)
        .map_err(|e| ArchiveError::C2pa(format!("sign: {e}")))?;

    Ok(SignedAsset {
        asset_bytes: dest.into_inner(),
        manifest_bytes,
    })
}

/// Sign an asset with the operator's configured cert + key PEM files,
/// returning the (unchanged) asset bytes plus the sidecar C2PA
/// manifest. High-level convenience over [`sign_asset_with_signer`]:
/// reads the cert chain + private key from disk, constructs a
/// [`c2pa::Signer`] via `c2pa::create_signer::from_keys`, and
/// delegates. Production operators who keep cert + key on the
/// filesystem call this; testers and advanced operators with custom
/// signers call [`sign_asset_with_signer`] directly.
pub fn sign_asset_bytes(
    config: &C2paConfig,
    asset_format: &str,
    asset_bytes: &[u8],
) -> Result<SignedAsset, ArchiveError> {
    let cert_pem = fs::read(&config.signing_cert_path)
        .map_err(|e| ArchiveError::Io(format!("read c2pa cert {}: {e}", config.signing_cert_path.display())))?;
    let key_pem = fs::read(&config.private_key_path)
        .map_err(|e| ArchiveError::Io(format!("read c2pa key {}: {e}", config.private_key_path.display())))?;

    let signer = c2pa::create_signer::from_keys(
        &cert_pem,
        &key_pem,
        config.signing_alg.to_c2pa(),
        config.timestamp_authority_url.clone(),
    )
    .map_err(|e| ArchiveError::C2pa(format!("create_signer: {e}")))?;

    sign_asset_with_signer(&*signer, &SignOptions::from_config(config), asset_format, asset_bytes)
}

/// Concatenate the byte contents of the given paths, in the caller-
/// supplied order, into a single `Vec<u8>`. This is the bytes-to-sign
/// builder for session 93's finalize flow: the caller (session 93's
/// `BroadcasterArchiveIndexer`-side glue code) walks the redb segment
/// index in `start_dts` order, collects the resulting [`PathBuf`]s,
/// and hands them to this helper. Decoupling the read + concat step
/// from the index walk keeps this primitive pure -- testable without
/// spinning up redb -- and lets future variants (BMFF `init` prefix,
/// streaming readers) swap the input without changing the signature
/// primitive downstream.
///
/// The current implementation reads every file fully into memory then
/// grows one contiguous `Vec<u8>`. At LVQR's per-segment size bound
/// (<= 1 MiB) and the archive's per-broadcast duration cap, the total
/// concat even for a multi-hour broadcast fits in memory comfortably
/// on the servers LVQR targets; if that ever stops being true we
/// swap to a streaming `impl Read + Seek` variant.
///
/// # Errors
///
/// [`ArchiveError::Io`] if any read fails; the error message names
/// the offending path.
pub fn concat_assets(paths: &[impl AsRef<Path>]) -> Result<Vec<u8>, ArchiveError> {
    let mut total = 0usize;
    let mut buffers: Vec<Vec<u8>> = Vec::with_capacity(paths.len());
    for p in paths {
        let bytes = fs::read(p.as_ref())
            .map_err(|e| ArchiveError::Io(format!("concat_assets read {}: {e}", p.as_ref().display())))?;
        total += bytes.len();
        buffers.push(bytes);
    }
    let mut out = Vec::with_capacity(total);
    for b in buffers {
        out.extend_from_slice(&b);
    }
    Ok(out)
}

/// Write a [`SignedAsset`] pair to disk at the caller-supplied paths.
/// `asset_path` receives `signed.asset_bytes` (the pass-through asset
/// in sidecar mode, identical to the input bytes); `manifest_path`
/// receives `signed.manifest_bytes` (the COSE-signed JUMBF container).
/// Both parent directories are created if they do not exist, matching
/// [`crate::writer::write_segment`]'s semantics.
///
/// Session 93 wires this call into the drain-terminated finalize
/// task at
/// `<archive_dir>/<broadcast>/<track>/finalized.<asset_ext>` +
/// `<archive_dir>/<broadcast>/<track>/finalized.c2pa`.
///
/// # Errors
///
/// [`ArchiveError::Io`] if any filesystem step fails; the error
/// message names the offending path.
pub fn write_signed_pair(asset_path: &Path, manifest_path: &Path, signed: &SignedAsset) -> Result<(), ArchiveError> {
    if let Some(parent) = asset_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| ArchiveError::Io(format!("create_dir_all {}: {e}", parent.display())))?;
    }
    fs::write(asset_path, &signed.asset_bytes)
        .map_err(|e| ArchiveError::Io(format!("write asset {}: {e}", asset_path.display())))?;
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| ArchiveError::Io(format!("create_dir_all {}: {e}", parent.display())))?;
    }
    fs::write(manifest_path, &signed.manifest_bytes)
        .map_err(|e| ArchiveError::Io(format!("write manifest {}: {e}", manifest_path.display())))?;
    Ok(())
}

/// Finalize one `(broadcast, track)` archive: concatenate
/// `init_bytes + segment_paths[0]..n` in the caller-supplied order,
/// sign the resulting asset via the caller-supplied
/// [`c2pa::Signer`], and write the signed asset + sidecar manifest
/// to disk. Returns the [`SignedAsset`] so the caller can log
/// manifest size or inspect bytes without re-reading from disk.
///
/// This is a pure orchestration helper: it composes
/// [`concat_assets`] + [`sign_asset_with_signer`] +
/// [`write_signed_pair`]. Session 94 B3 wires it into
/// `lvqr_cli::archive::BroadcasterArchiveIndexer::drain`'s
/// termination path inside a `tokio::task::spawn_blocking` closure
/// (the fn is sync; the caller already owns a blocking-friendly
/// execution context per the session 91 A writer refactor).
///
/// Deciding where `init_bytes` comes from is session 94's problem
/// (flat `<archive>/<broadcast>/<track>/init.mp4` vs.
/// `metadata.json` sidecar is still open). This helper stays agnostic
/// by taking the bytes as a parameter.
///
/// # Errors
///
/// Propagates [`ArchiveError::Io`] (concat reads or on-disk writes)
/// and [`ArchiveError::C2pa`] (signing).
pub fn finalize_broadcast_signed_with_signer(
    signer: &dyn c2pa::Signer,
    options: &SignOptions,
    init_bytes: &[u8],
    segment_paths: &[impl AsRef<Path>],
    asset_format: &str,
    asset_path: &Path,
    manifest_path: &Path,
) -> Result<SignedAsset, ArchiveError> {
    let mut concat_bytes = Vec::with_capacity(init_bytes.len());
    concat_bytes.extend_from_slice(init_bytes);
    if !segment_paths.is_empty() {
        let seg_bytes = concat_assets(segment_paths)?;
        concat_bytes.extend_from_slice(&seg_bytes);
    }
    let signed = sign_asset_with_signer(signer, options, asset_format, &concat_bytes)?;
    write_signed_pair(asset_path, manifest_path, &signed)?;
    Ok(signed)
}

/// High-level convenience over [`finalize_broadcast_signed_with_signer`]:
/// reads the cert chain + private key from disk per [`C2paConfig`],
/// constructs a [`c2pa::Signer`], and delegates. Operators running
/// with filesystem-backed PKI call this from the drain-terminated
/// finalize path; advanced operators with HSM-backed or KMS-backed
/// keys call the `_with_signer` variant directly.
pub fn finalize_broadcast_signed(
    config: &C2paConfig,
    init_bytes: &[u8],
    segment_paths: &[impl AsRef<Path>],
    asset_format: &str,
    asset_path: &Path,
    manifest_path: &Path,
) -> Result<SignedAsset, ArchiveError> {
    let cert_pem = fs::read(&config.signing_cert_path)
        .map_err(|e| ArchiveError::Io(format!("read c2pa cert {}: {e}", config.signing_cert_path.display())))?;
    let key_pem = fs::read(&config.private_key_path)
        .map_err(|e| ArchiveError::Io(format!("read c2pa key {}: {e}", config.private_key_path.display())))?;
    let signer = c2pa::create_signer::from_keys(
        &cert_pem,
        &key_pem,
        config.signing_alg.to_c2pa(),
        config.timestamp_authority_url.clone(),
    )
    .map_err(|e| ArchiveError::C2pa(format!("create_signer: {e}")))?;
    finalize_broadcast_signed_with_signer(
        &*signer,
        &SignOptions::from_config(config),
        init_bytes,
        segment_paths,
        asset_format,
        asset_path,
        manifest_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn concat_assets_preserves_caller_order() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        let c = dir.path().join("c.bin");
        fs::write(&a, b"aaa").unwrap();
        fs::write(&b, b"bbbb").unwrap();
        fs::write(&c, b"cc").unwrap();

        let out = concat_assets(&[&a, &b, &c]).unwrap();
        assert_eq!(out, b"aaabbbbcc");

        let reversed = concat_assets(&[&c, &b, &a]).unwrap();
        assert_eq!(reversed, b"ccbbbbaaa");
    }

    #[test]
    fn concat_assets_returns_io_error_when_any_path_is_missing() {
        let dir = TempDir::new().unwrap();
        let good = dir.path().join("present.bin");
        let missing = dir.path().join("absent.bin");
        fs::write(&good, b"bytes").unwrap();
        let err = concat_assets(&[&good, &missing]).unwrap_err();
        match err {
            ArchiveError::Io(msg) => assert!(msg.contains("absent.bin"), "should name missing: {msg}"),
            other => panic!("expected ArchiveError::Io, got {other:?}"),
        }
    }

    #[test]
    fn concat_assets_returns_empty_vec_for_empty_input() {
        let paths: &[PathBuf] = &[];
        let out = concat_assets(paths).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn write_signed_pair_creates_missing_parent_dirs_and_writes_bytes() {
        let dir = TempDir::new().unwrap();
        let asset = dir.path().join("nested/live/dvr/finalized.mp4");
        let manifest = dir.path().join("nested/live/dvr/finalized.c2pa");
        let signed = SignedAsset {
            asset_bytes: b"asset-body".to_vec(),
            manifest_bytes: b"manifest-body".to_vec(),
        };
        write_signed_pair(&asset, &manifest, &signed).unwrap();
        assert_eq!(fs::read(&asset).unwrap(), b"asset-body");
        assert_eq!(fs::read(&manifest).unwrap(), b"manifest-body");
    }

    #[test]
    fn write_signed_pair_overwrites_existing_files() {
        let dir = TempDir::new().unwrap();
        let asset = dir.path().join("a.mp4");
        let manifest = dir.path().join("a.c2pa");
        write_signed_pair(
            &asset,
            &manifest,
            &SignedAsset {
                asset_bytes: b"first-asset".to_vec(),
                manifest_bytes: b"first-manifest".to_vec(),
            },
        )
        .unwrap();
        write_signed_pair(
            &asset,
            &manifest,
            &SignedAsset {
                asset_bytes: b"second-asset".to_vec(),
                manifest_bytes: b"second-manifest".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(fs::read(&asset).unwrap(), b"second-asset");
        assert_eq!(fs::read(&manifest).unwrap(), b"second-manifest");
    }
}
