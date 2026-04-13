//! Disk-based fMP4 recording for LVQR broadcasts.
//!
//! `BroadcastRecorder` subscribes to a MoQ broadcast and writes its tracks to
//! disk as fMP4 init segments and media segments. The recorder is designed to
//! be optional: it acts as just another MoQ subscriber, so it never affects the
//! live data path.
//!
//! Layout for each recorded broadcast:
//! ```text
//! {record_dir}/{broadcast}/0.init.mp4   # video init segment
//! {record_dir}/{broadcast}/0.0001.m4s   # video media segments
//! {record_dir}/{broadcast}/0.0002.m4s
//! {record_dir}/{broadcast}/1.init.mp4   # audio init segment
//! {record_dir}/{broadcast}/1.0001.m4s   # audio media segments
//! ```

mod error;
mod recorder;

pub use error::RecordError;
pub use recorder::{BroadcastRecorder, RecordOptions};
