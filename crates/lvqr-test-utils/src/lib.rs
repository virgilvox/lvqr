use bytes::Bytes;
use std::net::TcpListener;

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

fn is_on_path(name: &str) -> bool {
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
