//! C2PA provenance signing primitive for archived assets.
//!
//! **Tier 4 item 4.3 session A.** Compiled only when `lvqr-archive` is
//! built with `--features c2pa`. Pulls the `c2pa` crate (pinned 0.80;
//! `default-features = false`, `features = ["rust_native_crypto"]` at the
//! workspace level so the crypto closure stays pure-Rust and the remote-
//! manifest HTTP stacks are not in the graph).
//!
//! # What this module owns
//!
//! * [`C2paConfig`]: operator-facing configuration bag -- signing cert
//!   path, private key path, creator-assertion name, signature algorithm,
//!   optional RFC 3161 timestamp authority URL.
//! * [`C2paSigningAlg`]: LVQR-owned enum that maps 1:1 to the c2pa-rs
//!   `SigningAlg` enum so downstream consumers do not need a direct dep
//!   on `c2pa-rs` to build a [`C2paConfig`].
//! * [`SignedAsset`]: the sign result. Keeps the asset bytes and the
//!   manifest bytes separate so the caller chooses embed vs. sidecar
//!   semantics on disk; the primitive itself runs `Builder::set_no_embed(
//!   true)` so the asset passes through unchanged.
//! * [`sign_asset_bytes`]: bytes-in / bytes-out signing primitive. Loads
//!   the cert + key PEMs from disk, constructs a `c2pa::Builder` with a
//!   minimal manifest carrying the creator assertion, signs against an
//!   in-memory cursor of the asset, and returns the `SignedAsset` pair.
//!
//! # What this module is NOT
//!
//! * Not a finalize-asset builder. The archive is a stream of CMAF
//!   segments, not a single finalized MP4. Constructing the bytes-to-sign
//!   (concatenated init.mp4 + segments ordered by dts, or a tree-hash
//!   over segment digests) is session B's problem because it requires
//!   persisting init bytes on disk and wiring a broadcast-end lifecycle
//!   hook into `FragmentBroadcasterRegistry` -- both out of scope for a
//!   pure crate-local primitive.
//! * Not a c2pa reader / verifier. Session B adds the admin verify
//!   route + E2E that parses the manifest back.
//! * Not an operator-supplied PKI manager. The MVP accepts whatever
//!   cert the operator points at. Trust-root validation happens at read
//!   time via `c2pa::Reader`, not here.
//!
//! # Why the primitive takes bytes, not a path
//!
//! c2pa-rs 0.80 exposes `Builder::sign(R: Read+Seek+Send, W: Write+Read
//! +Seek+Send)` against in-memory cursors. Taking bytes lets the caller
//! decide whether the asset lives in memory (typical for finalized MP4
//! construction, which is a concat step that already holds the bytes),
//! on disk, or behind a reader. Session B wires the on-disk path by
//! reading + concatenating segments into a `Vec<u8>` before calling
//! this primitive; if that buffer ever gets too large to hold in memory,
//! we introduce a streaming variant then. Today's archive segment sizes
//! are <= 1 MiB so hundreds of them fit in memory without issue.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

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

/// Sign an asset with the operator's configured cert + key, returning
/// the (unchanged) asset bytes plus the sidecar C2PA manifest.
///
/// `asset_format` is an IANA MIME type (`"image/jpeg"`, `"video/mp4"`,
/// etc.) or a c2pa-rs known extension alias. The handler is selected
/// by `c2pa-rs`'s `asset_handlers` dispatch; unsupported formats
/// return [`ArchiveError::C2pa`].
///
/// The manifest carries:
/// * A `ClaimGeneratorInfo` naming `"lvqr"` + this crate's version.
/// * One `stds.schema-org.CreativeWork` assertion with a single
///   `Person` author whose `name` is `config.assertion_creator`.
/// * No ingredients. Ingredient chains are meaningful when an asset
///   is derived from another C2PA-signed asset; an archive's source
///   is an RTMP ingest which has no upstream manifest, so there is
///   nothing to ingredient.
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
                    "name": config.assertion_creator,
                }],
            },
        }],
    })
    .to_string();

    // `Builder::from_json` was deprecated in c2pa-rs 0.80 in favor of
    // `Builder::from_context(ctx).with_definition(json)` so the manifest
    // definition is carried alongside a Context rather than through
    // thread-local settings. We construct a fresh `Context::new()` per
    // call -- the primitive is stateless and the Context's only setting
    // we care about (the signer) is passed explicitly to `sign` below.
    let mut builder = c2pa::Builder::from_context(c2pa::Context::new())
        .with_definition(manifest_json.as_str())
        .map_err(|e| ArchiveError::C2pa(format!("builder with_definition: {e}")))?;
    builder.set_intent(c2pa::BuilderIntent::Edit);
    builder.set_no_embed(true);

    let mut source = Cursor::new(asset_bytes.to_vec());
    let mut dest = Cursor::new(Vec::<u8>::new());
    let manifest_bytes = builder
        .sign(&*signer, asset_format, &mut source, &mut dest)
        .map_err(|e| ArchiveError::C2pa(format!("sign: {e}")))?;

    Ok(SignedAsset {
        asset_bytes: dest.into_inner(),
        manifest_bytes,
    })
}
