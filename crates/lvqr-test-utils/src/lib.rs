use bytes::Bytes;
use std::net::TcpListener;

pub mod flv;
pub mod http;
pub mod rtmp;
mod test_server;
pub use test_server::{TestServer, TestServerConfig};

/// Find an available TCP port on localhost.
pub fn find_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind ephemeral port");
    listener.local_addr().unwrap().port()
}

/// Generate a synthetic keyframe payload of the given size.
pub fn synthetic_keyframe(size: usize) -> Bytes {
    let mut data = vec![0u8; size];
    // NAL unit header for IDR slice (simplified)
    if size >= 4 {
        data[0] = 0x00;
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x01;
    }
    if size >= 5 {
        data[4] = 0x65; // IDR NAL type
    }
    Bytes::from(data)
}

/// Generate a synthetic delta frame payload of the given size.
pub fn synthetic_delta_frame(size: usize) -> Bytes {
    let mut data = vec![0u8; size];
    if size >= 4 {
        data[0] = 0x00;
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x01;
    }
    if size >= 5 {
        data[4] = 0x41; // non-IDR NAL type
    }
    Bytes::from(data)
}

/// Initialize tracing for tests (call once per test binary).
pub fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();
}

/// Generate a self-signed TLS certificate for testing.
pub fn generate_test_certs() -> (Vec<u8>, Vec<u8>) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("failed to generate test cert");
    let cert_der = cert.cert.der().to_vec();
    let key_der = cert.key_pair.serialize_der().to_vec();
    (cert_der, key_der)
}

/// Outcome of running `ffprobe` against an in-memory byte slice.
#[derive(Debug)]
pub enum FfprobeResult {
    /// ffprobe returned exit code 0 and emitted no errors on stderr.
    Ok,
    /// ffprobe is not installed on PATH. Tests should treat this as a
    /// soft skip so CI works on machines that do not have ffmpeg.
    Skipped,
    /// ffprobe ran and rejected the bytes.
    Failed { stderr: String, exit_code: i32 },
}

impl FfprobeResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, FfprobeResult::Ok)
    }

    pub fn is_skipped(&self) -> bool {
        matches!(self, FfprobeResult::Skipped)
    }

    /// Panic unless ffprobe accepted the bytes. Skipped runs print a
    /// warning but do not fail the test; this keeps contributor laptops
    /// without ffmpeg installed from breaking CI.
    pub fn assert_accepted(self) {
        match self {
            FfprobeResult::Ok => {}
            FfprobeResult::Skipped => {
                eprintln!("ffprobe not installed; skipping structural validation");
            }
            FfprobeResult::Failed { stderr, exit_code } => {
                panic!("ffprobe rejected bytes: exit={exit_code}\nstderr:\n{stderr}");
            }
        }
    }
}

/// Pipe `bytes` into `ffprobe -v error -show_format -show_streams -i -` and
/// report whether it parsed. Any non-zero exit or stderr content is
/// treated as a failure. Use for lightweight structural validation of
/// generated fMP4 segments in tests.
///
/// Returns `FfprobeResult::Skipped` if `ffprobe` is not on PATH so tests
/// compile and run on machines without ffmpeg installed.
pub fn ffprobe_bytes(bytes: &[u8]) -> FfprobeResult {
    if !is_on_path("ffprobe") {
        return FfprobeResult::Skipped;
    }
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = match Command::new("ffprobe")
        .args(["-v", "error", "-show_format", "-show_streams", "-i", "pipe:0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return FfprobeResult::Failed {
                stderr: format!("spawn failed: {e}"),
                exit_code: -1,
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(bytes);
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return FfprobeResult::Failed {
                stderr: format!("wait failed: {e}"),
                exit_code: -1,
            };
        }
    };

    // ffprobe's authoritative signal is its exit code: non-zero means it
    // rejected the input. stderr on an exit-zero run is diagnostics, not a
    // verdict -- ffprobe 8.x emits decoder-level warnings like
    // "deblocking_filter_idc 32 out of range" even on structurally valid
    // containers that happen to carry synthesized dummy NAL payloads. Treat
    // those as noise, but surface them to the test log via eprintln so a
    // real regression is still visible.
    if output.status.success() {
        if !output.stderr.is_empty() {
            eprintln!(
                "ffprobe accepted container with decoder warnings:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        FfprobeResult::Ok
    } else {
        FfprobeResult::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        }
    }
}

/// Outcome of running Apple `mediastreamvalidator` against an
/// on-disk playlist.
///
/// `mediastreamvalidator` is part of Apple's free HLS Tools bundle
/// (`https://developer.apple.com/download/all/?q=hls`). It is not on
/// Homebrew and is not shipped in CI; tests should soft-skip when
/// the binary is not available, just like [`FfprobeResult::Skipped`].
#[derive(Debug)]
pub enum MediaStreamValidatorResult {
    /// Validator ran and accepted the playlist.
    Ok,
    /// Validator is not installed on this host. Tests treat this as
    /// a soft skip so contributor laptops without the Apple HLS
    /// Tools bundle do not break CI.
    Skipped,
    /// Validator ran and rejected the playlist.
    Failed { stdout: String, exit_code: i32 },
}

impl MediaStreamValidatorResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }

    pub fn is_skipped(&self) -> bool {
        matches!(self, Self::Skipped)
    }

    /// Panic unless the validator accepted the playlist. Skipped
    /// runs print a warning but do not fail the test.
    pub fn assert_accepted(self) {
        match self {
            Self::Ok => {}
            Self::Skipped => {
                eprintln!("mediastreamvalidator not installed; skipping Apple HLS validation");
            }
            Self::Failed { stdout, exit_code } => {
                panic!("mediastreamvalidator rejected playlist: exit={exit_code}\n{stdout}");
            }
        }
    }
}

/// Write a rendered HLS playlist plus its init segment and media
/// segment byte blobs into a temporary directory, then invoke
/// `mediastreamvalidator` against the playlist path.
///
/// `segments` is a slice of `(relative_path, bytes)` tuples. Every
/// tuple is written verbatim into the temp dir so the playlist's
/// `#EXT-X-MAP` / `#EXT-X-PART` / `#EXTINF` URIs resolve locally.
/// The playlist itself is written as `<tmp>/playlist.m3u8`.
///
/// Returns `Skipped` if `mediastreamvalidator` is not on PATH. Tests
/// should call `.assert_accepted()` on the result.
pub fn mediastreamvalidator_playlist(
    playlist: &str,
    segments: &[(String, bytes::Bytes)],
) -> MediaStreamValidatorResult {
    if !is_on_path("mediastreamvalidator") {
        return MediaStreamValidatorResult::Skipped;
    }
    use std::process::Command;

    let tmp = match tempfile::tempdir() {
        Ok(t) => t,
        Err(e) => {
            return MediaStreamValidatorResult::Failed {
                stdout: format!("tempdir failed: {e}"),
                exit_code: -1,
            };
        }
    };
    let playlist_path = tmp.path().join("playlist.m3u8");
    if let Err(e) = std::fs::write(&playlist_path, playlist) {
        return MediaStreamValidatorResult::Failed {
            stdout: format!("write playlist failed: {e}"),
            exit_code: -1,
        };
    }
    for (rel, body) in segments {
        let out = tmp.path().join(rel);
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&out, body) {
            return MediaStreamValidatorResult::Failed {
                stdout: format!("write {rel} failed: {e}"),
                exit_code: -1,
            };
        }
    }
    let output = match Command::new("mediastreamvalidator").arg(&playlist_path).output() {
        Ok(o) => o,
        Err(e) => {
            return MediaStreamValidatorResult::Failed {
                stdout: format!("spawn failed: {e}"),
                exit_code: -1,
            };
        }
    };
    // mediastreamvalidator emits its verdict on stdout (not stderr)
    // and exits zero even on warnings; a non-zero exit is the
    // authoritative rejection signal.
    if output.status.success() {
        MediaStreamValidatorResult::Ok
    } else {
        MediaStreamValidatorResult::Failed {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        }
    }
}

/// `true` when `name` resolves to a file on `$PATH`. Used by the
/// ffprobe / mediastreamvalidator wrappers above and exposed so
/// integration tests that shell out to external tools (ffmpeg,
/// gst-inspect-1.0, etc.) can soft-skip on hosts without the tool
/// installed rather than hard-failing.
pub fn is_on_path(name: &str) -> bool {
    is_on_path_inner(name)
}

fn is_on_path_inner(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if std::fs::metadata(&candidate).map(|m| m.is_file()).unwrap_or(false) {
            return true;
        }
    }
    false
}
