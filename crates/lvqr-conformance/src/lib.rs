//! Reference fixtures and conformance harnesses for LVQR.
//!
//! This crate hosts:
//!
//! 1. **Reference fixtures** under `fixtures/` at crate root: real encoder
//!    captures (OBS, ffmpeg, Larix Broadcaster), spec test vectors (Apple
//!    HLS, DASH-IF, Pion WHIP), and deliberate edge cases (broken encoders,
//!    truncated segments, malformed tags). Every fixture ships with a
//!    sidecar `.toml` describing its provenance and the codec profile it
//!    exercises.
//!
//! 2. **Conformance helpers** that other crates' tests can import to:
//!      - Load a named fixture as `Bytes`.
//!      - Iterate the full fixture corpus as proptest seeds.
//!      - Invoke external validators (Apple `mediastreamvalidator`,
//!        `ffprobe`, DASH-IF conformance tool) with a common interface
//!        that returns `Ok(())` on validator success, `Err(Skipped)`
//!        if the validator is not installed, and `Err(Failed { ... })`
//!        on real failures so CI can flag regressions.
//!
//! 3. **Cross-implementation comparison harness** (planned) that feeds
//!    the same input into LVQR and MediaMTX, then structurally diffs
//!    the HLS playlist output. This graduates from Tier 1 to a CI gate
//!    at the start of Tier 2.5 per the 2026-04-13 audit.
//!
//! The crate intentionally has `publish = false`. It is not shipped to
//! crates.io; it is an internal test dependency.

use bytes::Bytes;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Root directory for reference fixtures. Paths are resolved relative to
/// the crate manifest so tests work regardless of where cargo is invoked.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Load a named fixture file into a `Bytes` buffer.
///
/// The `name` argument is a slash-delimited path relative to `fixtures/`,
/// e.g. `"rtmp/obs-30-macos-h264-aac.flv"` or
/// `"fmp4/cmaf-h264-baseline-360p-1s.mp4"`.
pub fn load_fixture(name: &str) -> std::io::Result<Bytes> {
    let path = fixtures_dir().join(name);
    let bytes = std::fs::read(&path)?;
    Ok(Bytes::from(bytes))
}

/// Return the absolute path for a named fixture without reading it.
pub fn fixture_path(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

pub mod codec {
    //! Typed access to the codec-parser fixture corpus.
    //!
    //! Every file under `fixtures/codec/` pairs a raw parser-input
    //! byte blob with a `.toml` sidecar that names the expected
    //! decoded values. Parser conformance tests in `lvqr-codec`
    //! iterate this corpus via [`list`] and [`load`] so adding a new
    //! fixture + sidecar automatically extends coverage without
    //! touching test code.

    use super::*;

    /// Sidecar metadata for a codec fixture.
    ///
    /// Only the fields relevant to parser conformance are modeled.
    /// Free-form TOML keys like `source`, `container`, `license` are
    /// accepted at the top level but not surfaced because the tests
    /// do not assert on them.
    #[derive(Debug, Clone, Deserialize)]
    pub struct CodecFixtureMeta {
        /// Human-readable codec string, e.g. `"hev1.1.6.L93.B0"`.
        pub codec: String,
        /// Expected decoded values for an HEVC SPS fixture. Present
        /// iff the fixture is an HEVC SPS byte blob.
        pub expected: Expected,
    }

    /// Parser-specific expectation discriminators. Only one branch is
    /// present per fixture; the TOML `[expected.hevc_sps]` vs
    /// `[expected.aac_asc]` section names select which.
    #[derive(Debug, Clone, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Expected {
        pub hevc_sps: Option<HevcSpsExpected>,
        pub aac_asc: Option<AacAscExpected>,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct HevcSpsExpected {
        pub general_profile_space: u8,
        pub general_tier_flag: bool,
        pub general_profile_idc: u8,
        pub general_profile_compatibility_flags: u32,
        pub general_level_idc: u8,
        pub chroma_format_idc: u32,
        pub pic_width_in_luma_samples: u32,
        pub pic_height_in_luma_samples: u32,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct AacAscExpected {
        pub object_type: u8,
        pub sample_rate: u32,
        pub channel_config: u8,
        pub sbr_present: bool,
        pub ps_present: bool,
    }

    /// One loaded codec fixture: the byte blob plus its parsed metadata.
    #[derive(Debug, Clone)]
    pub struct CodecFixture {
        /// File stem (no extension), e.g. `hevc-sps-x265-main-320x240`.
        pub name: String,
        /// Raw parser-input bytes.
        pub bytes: Bytes,
        /// Parsed sidecar metadata.
        pub meta: CodecFixtureMeta,
    }

    /// List every codec fixture on disk. Returns the fixtures sorted
    /// by file name so iteration order is deterministic across
    /// platforms and filesystems.
    pub fn list() -> std::io::Result<Vec<CodecFixture>> {
        let dir = super::fixtures_dir().join("codec");
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("bin") {
                continue;
            }
            let name = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let bytes = std::fs::read(&path)?;
            let toml_path = path.with_extension("toml");
            let toml_text = std::fs::read_to_string(&toml_path)
                .map_err(|e| std::io::Error::other(format!("missing sidecar {}: {e}", toml_path.display())))?;
            let meta: CodecFixtureMeta = toml::from_str(&toml_text).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("malformed sidecar {}: {e}", toml_path.display()),
                )
            })?;
            out.push(CodecFixture {
                name,
                bytes: Bytes::from(bytes),
                meta,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Load a single codec fixture by its file stem.
    pub fn load(name: &str) -> std::io::Result<CodecFixture> {
        let list = list()?;
        list.into_iter()
            .find(|f| f.name == name)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("no codec fixture {name}")))
    }
}

/// Outcome of running an external validator.
#[derive(Debug)]
pub enum ValidatorResult {
    /// Validator ran and accepted the input.
    Ok,
    /// Validator is not installed on this host. Tests should treat this
    /// as a soft skip rather than a failure so CI works on machines that
    /// do not have the tool available.
    Skipped { reason: String },
    /// Validator ran and rejected the input.
    Failed { stderr: String, exit_code: i32 },
}

impl ValidatorResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, ValidatorResult::Ok)
    }

    pub fn is_skipped(&self) -> bool {
        matches!(self, ValidatorResult::Skipped { .. })
    }

    /// Panic unless the validator accepted the input. Skipped runs are
    /// treated as success so optional tooling does not break CI.
    pub fn assert_accepted(self) {
        match self {
            ValidatorResult::Ok => {}
            ValidatorResult::Skipped { reason } => {
                eprintln!("validator skipped: {reason}");
            }
            ValidatorResult::Failed { stderr, exit_code } => {
                panic!("validator rejected input: exit={exit_code}\n{stderr}");
            }
        }
    }
}

/// Check whether a named executable is on PATH.
pub fn has_tool(name: &str) -> bool {
    which_on_path(name).is_some()
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    match std::fs::metadata(p) {
        Ok(md) => md.is_file(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_dir_is_under_crate_manifest() {
        let dir = fixtures_dir();
        assert!(dir.ends_with("fixtures"));
    }

    #[test]
    fn codec_fixture_list_loads_bundled_blobs() {
        let list = codec::list().expect("list codec fixtures");
        assert!(
            !list.is_empty(),
            "codec fixture corpus should not be empty after session 5 bootstrap"
        );
        // Every fixture's sidecar must name exactly one parser
        // expectation (hevc_sps XOR aac_asc). Catches a malformed
        // sidecar before any downstream test loads it.
        for f in &list {
            let has_hevc = f.meta.expected.hevc_sps.is_some();
            let has_aac = f.meta.expected.aac_asc.is_some();
            assert!(
                has_hevc ^ has_aac,
                "fixture {} must name exactly one of hevc_sps / aac_asc",
                f.name
            );
        }
    }

    #[test]
    fn validator_result_predicates() {
        assert!(ValidatorResult::Ok.is_ok());
        assert!(
            ValidatorResult::Skipped {
                reason: "no tool".into(),
            }
            .is_skipped()
        );
        assert!(
            !ValidatorResult::Failed {
                stderr: String::new(),
                exit_code: 1,
            }
            .is_ok()
        );
    }
}
