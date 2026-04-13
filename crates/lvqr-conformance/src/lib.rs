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
/// `"fmp4/init-h264-baseline-720p.mp4"`.
pub fn load_fixture(name: &str) -> std::io::Result<Bytes> {
    let path = fixtures_dir().join(name);
    let bytes = std::fs::read(&path)?;
    Ok(Bytes::from(bytes))
}

/// Return the absolute path for a named fixture without reading it.
pub fn fixture_path(name: &str) -> PathBuf {
    fixtures_dir().join(name)
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
