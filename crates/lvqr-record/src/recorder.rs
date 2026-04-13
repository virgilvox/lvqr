use crate::error::RecordError;
use bytes::Bytes;
use moq_lite::{BroadcastConsumer, Track};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Options for `BroadcastRecorder::record`.
#[derive(Debug, Clone)]
pub struct RecordOptions {
    /// Track names to record. Typically `["0.mp4", "1.mp4"]` for video+audio.
    pub tracks: Vec<String>,
}

impl Default for RecordOptions {
    fn default() -> Self {
        Self {
            tracks: vec!["0.mp4".to_string(), "1.mp4".to_string()],
        }
    }
}

/// Records MoQ broadcasts to disk as fMP4 init + media segments.
///
/// One `BroadcastRecorder` instance can record many broadcasts concurrently;
/// each `record_broadcast` call spawns its own task tree (one per track).
#[derive(Debug, Clone)]
pub struct BroadcastRecorder {
    base_dir: PathBuf,
}

impl BroadcastRecorder {
    /// Construct a recorder writing into `base_dir`. The directory is created
    /// on first use.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Record a broadcast until either the broadcast ends or `cancel` fires.
    ///
    /// This method spawns one task per requested track. Each task reads MoQ
    /// groups, treating the first frame of each group as either an init
    /// segment (if it begins with `ftyp`) or a media segment.
    pub async fn record_broadcast(
        &self,
        broadcast_name: &str,
        broadcast: BroadcastConsumer,
        opts: RecordOptions,
        cancel: CancellationToken,
    ) -> Result<(), RecordError> {
        let dir = self.base_dir.join(sanitize_name(broadcast_name));
        fs::create_dir_all(&dir).await?;
        info!(broadcast = %broadcast_name, dir = %dir.display(), "recording started");

        let mut handles = Vec::new();
        for track_name in opts.tracks {
            let track_consumer = match broadcast.subscribe_track(&Track::new(track_name.as_str())) {
                Ok(t) => t,
                Err(e) => {
                    debug!(track = %track_name, error = ?e, "track unavailable; skipping");
                    continue;
                }
            };
            let track_dir = dir.clone();
            let cancel = cancel.clone();
            let name = track_name.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = record_track(&track_dir, &name, track_consumer, cancel).await {
                    warn!(track = %name, error = %e, "track recording error");
                }
            }));
        }

        for h in handles {
            let _ = h.await;
        }

        info!(broadcast = %broadcast_name, "recording stopped");
        Ok(())
    }
}

async fn record_track(
    dir: &Path,
    track_name: &str,
    mut track: moq_lite::TrackConsumer,
    cancel: CancellationToken,
) -> Result<(), RecordError> {
    let prefix = track_prefix(track_name);
    let mut segment_seq: u64 = 0;
    let mut init_written = false;

    loop {
        let group = tokio::select! {
            res = track.next_group() => res,
            _ = cancel.cancelled() => return Ok(()),
        };

        let mut group = match group {
            Ok(Some(g)) => g,
            Ok(None) => return Ok(()),
            Err(e) => {
                debug!(track = %track_name, error = ?e, "track ended");
                return Ok(());
            }
        };

        loop {
            let frame = tokio::select! {
                res = group.read_frame() => res,
                _ = cancel.cancelled() => return Ok(()),
            };
            match frame {
                Ok(Some(bytes)) => {
                    if !init_written && looks_like_init(&bytes) {
                        let path = dir.join(format!("{prefix}.init.mp4"));
                        fs::write(&path, &bytes).await?;
                        init_written = true;
                        debug!(track = %track_name, path = %path.display(), "init segment written");
                    } else {
                        segment_seq += 1;
                        let path = dir.join(format!("{prefix}.{segment_seq:04}.m4s"));
                        let mut file = fs::File::create(&path).await?;
                        file.write_all(&bytes).await?;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    debug!(track = %track_name, error = ?e, "group read error");
                    break;
                }
            }
        }
    }
}

/// Detect an fMP4 init segment by looking for the `ftyp` box at offset 4.
fn looks_like_init(bytes: &Bytes) -> bool {
    bytes.len() >= 8 && &bytes[4..8] == b"ftyp"
}

/// Convert a track name like "0.mp4" into a filename prefix "0".
fn track_prefix(track_name: &str) -> String {
    track_name.split('.').next().unwrap_or(track_name).to_string()
}

/// Sanitize a broadcast name for use as a filesystem directory.
/// Strips path traversal and replaces slashes with underscores.
fn sanitize_name(name: &str) -> String {
    name.replace(['/', '\\'], "_")
        .replace("..", "_")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_init_segment() {
        let init = Bytes::from(vec![
            0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm',
        ]);
        assert!(looks_like_init(&init));
        let media = Bytes::from(vec![0x00, 0x00, 0x00, 0x10, b'm', b'o', b'o', b'f', 0, 0, 0, 0]);
        assert!(!looks_like_init(&media));
        assert!(!looks_like_init(&Bytes::new()));
    }

    #[test]
    fn track_prefix_strips_extension() {
        assert_eq!(track_prefix("0.mp4"), "0");
        assert_eq!(track_prefix("1.mp4"), "1");
        assert_eq!(track_prefix(".catalog"), "");
        assert_eq!(track_prefix("video"), "video");
    }

    #[test]
    fn sanitize_strips_path_traversal() {
        assert_eq!(sanitize_name("live/test"), "live_test");
        // ".." is replaced with "_" before slashes are translated
        let cleaned = sanitize_name("../etc/passwd");
        assert!(!cleaned.contains(".."));
        assert!(!cleaned.contains('/'));
        assert_eq!(sanitize_name("normal-name"), "normal-name");
    }
}
