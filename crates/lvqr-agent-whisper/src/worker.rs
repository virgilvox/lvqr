//! Worker thread that owns the whisper.cpp `WhisperContext`,
//! the symphonia AAC decoder, and the PCM buffer ring. Runs
//! inference on a windowed schedule and publishes captions to
//! a [`crate::CaptionStream`].
//!
//! Spawned from [`crate::WhisperCaptionsAgent::on_start`] when
//! the `whisper` Cargo feature is enabled. The worker is an OS
//! thread (not a tokio task) because whisper.cpp inference is
//! a CPU-bound blocking call that would starve the tokio
//! runtime if scheduled on it. Communication is via
//! `std::sync::mpsc::sync_channel` with a bounded depth so a
//! slow worker drops AAC frames into a `warn!` rather than
//! back-pressuring the per-broadcast drain task.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentBroadcaster, FragmentFlags};
use thiserror::Error;
use tracing::{debug, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::caption::{CaptionStream, TranscribedCaption};
use crate::decode::{AacToMonoF32, WHISPER_SAMPLE_RATE};
use crate::factory::WhisperConfig;

#[derive(Debug, Error)]
pub(crate) enum SpawnError {
    #[error("whisper context init failed: {0}")]
    ContextInit(String),

    #[error("symphonia AAC decoder init failed: {0}")]
    DecoderInit(String),
}

/// Message from the agent's `on_fragment` to the worker thread.
enum Message {
    /// One raw AAC frame extracted from a fragment's mdat box.
    Frame { dts: u64, aac: Bytes },
}

/// Handle the agent holds to send frames to the worker and to
/// trigger an orderly shutdown when `on_stop` fires.
pub(crate) struct WorkerHandle {
    tx: SyncSender<Message>,
    join: Option<thread::JoinHandle<()>>,
    broadcast: String,
}

impl WorkerHandle {
    /// Try to enqueue an AAC frame. On a full channel logs a
    /// warning and drops the frame -- caption gaps beat
    /// back-pressuring the broadcast.
    pub fn send_frame(&self, dts: u64, aac: Bytes) {
        if let Err(_e) = self.tx.try_send(Message::Frame { dts, aac }) {
            warn!(
                broadcast = %self.broadcast,
                "WhisperCaptionsAgent: worker queue full; dropping AAC frame",
            );
        }
    }

    /// Close the worker's input channel and wait for it to
    /// drain its remaining PCM, run a final inference pass,
    /// and exit. Called from
    /// [`crate::WhisperCaptionsAgent::on_stop`].
    pub fn shutdown(mut self) {
        // Drop the sender to signal end-of-stream to the worker.
        drop(self.tx);
        if let Some(join) = self.join.take()
            && let Err(e) = join.join()
        {
            warn!(
                broadcast = %self.broadcast,
                error = ?e,
                "WhisperCaptionsAgent: worker thread panicked during shutdown",
            );
        }
    }
}

/// Spawn a worker thread for one `(broadcast, "1.mp4")` pair.
///
/// Returns a [`WorkerHandle`] the agent uses to enqueue frames
/// and to trigger graceful shutdown. Panicking inside the
/// worker thread is caught at `join()` time and logged; it does
/// not propagate to the caller (the per-broadcast drain task
/// stays alive).
pub(crate) fn spawn(
    config: Arc<WhisperConfig>,
    captions: CaptionStream,
    caption_broadcaster: Option<Arc<FragmentBroadcaster>>,
    broadcast: String,
    source_sample_rate: u32,
    asc: Bytes,
    queue_depth: usize,
) -> Result<WorkerHandle, SpawnError> {
    // Construct heavy state on the spawning thread so that
    // failures (model file missing, bad ASC) surface eagerly to
    // the agent's on_start instead of getting buried in the
    // worker thread's stderr.
    let model_path = config.model_path.clone();
    let model_path_str = model_path
        .to_str()
        .ok_or_else(|| SpawnError::ContextInit(format!("model path not valid UTF-8: {}", model_path.display())))?
        .to_string();
    let context = WhisperContext::new_with_params(&model_path_str, WhisperContextParameters::default())
        .map_err(|e| SpawnError::ContextInit(format!("{e}")))?;

    let asc_arc = Arc::new(asc);
    let mut decoder = AacToMonoF32::new(Arc::clone(&asc_arc), source_sample_rate)
        .map_err(|e| SpawnError::DecoderInit(format!("{e}")))?;

    let (tx, rx) = sync_channel::<Message>(queue_depth);
    let window_samples = (config.window_ms as u64 * WHISPER_SAMPLE_RATE as u64 / 1000) as usize;
    let broadcast_for_thread = broadcast.clone();

    let join = thread::Builder::new()
        .name(format!("lvqr-whisper:{}", broadcast))
        .spawn(move || {
            run(
                context,
                &mut decoder,
                captions,
                caption_broadcaster,
                broadcast_for_thread,
                source_sample_rate,
                window_samples,
                rx,
            );
        })
        .map_err(|e| SpawnError::ContextInit(format!("worker thread spawn failed: {e}")))?;

    Ok(WorkerHandle {
        tx,
        join: Some(join),
        broadcast,
    })
}

/// Worker main loop. Owns the `WhisperContext`, the decoder,
/// and the PCM ring. Runs inference whenever the ring crosses
/// `window_samples`; runs a final pass on channel close.
#[allow(clippy::too_many_arguments)]
fn run(
    context: WhisperContext,
    decoder: &mut AacToMonoF32,
    captions: CaptionStream,
    caption_broadcaster: Option<Arc<FragmentBroadcaster>>,
    broadcast: String,
    source_sample_rate: u32,
    window_samples: usize,
    rx: Receiver<Message>,
) {
    let mut state = match context.create_state() {
        Ok(s) => s,
        Err(e) => {
            warn!(broadcast = %broadcast, error = %e, "whisper create_state failed; worker exiting");
            return;
        }
    };

    let mut pcm: Vec<f32> = Vec::with_capacity(window_samples * 2);
    let mut window_start_dts: Option<u64> = None;
    let mut total_inferences: u64 = 0;
    let mut total_captions: u64 = 0;

    loop {
        match rx.recv() {
            Ok(Message::Frame { dts, aac }) => {
                if window_start_dts.is_none() {
                    window_start_dts = Some(dts);
                }
                match decoder.decode_frame(dts, &aac) {
                    Ok(samples) => pcm.extend_from_slice(&samples),
                    Err(e) => {
                        warn!(broadcast = %broadcast, error = %e, "AAC decode failed; skipping frame");
                        continue;
                    }
                }

                if pcm.len() >= window_samples
                    && let Some(start_dts) = window_start_dts.take()
                {
                    let segments_emitted = run_inference(
                        &mut state,
                        &captions,
                        caption_broadcaster.as_ref(),
                        &broadcast,
                        source_sample_rate,
                        start_dts,
                        &pcm,
                    );
                    total_inferences += 1;
                    total_captions += segments_emitted as u64;
                    pcm.clear();
                }
            }
            Err(_recv_err) => {
                // Sender dropped -> graceful shutdown. Run a
                // final inference pass on whatever's left in
                // the ring so the last few seconds of audio do
                // not get silently dropped.
                if let Some(start_dts) = window_start_dts.take()
                    && !pcm.is_empty()
                {
                    let segments_emitted = run_inference(
                        &mut state,
                        &captions,
                        caption_broadcaster.as_ref(),
                        &broadcast,
                        source_sample_rate,
                        start_dts,
                        &pcm,
                    );
                    total_inferences += 1;
                    total_captions += segments_emitted as u64;
                }
                break;
            }
        }
    }

    info!(
        broadcast = %broadcast,
        inferences = total_inferences,
        captions = total_captions,
        "WhisperCaptionsAgent: worker exited",
    );
}

/// Run one inference pass on the buffered PCM and publish
/// resulting segments to the captions channel and (when
/// installed) the shared captions broadcaster on the
/// fragment registry. Returns the segment count published.
fn run_inference(
    state: &mut whisper_rs::WhisperState,
    captions: &CaptionStream,
    caption_broadcaster: Option<&Arc<FragmentBroadcaster>>,
    broadcast: &str,
    source_sample_rate: u32,
    window_start_dts: u64,
    pcm: &[f32],
) -> usize {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);

    if let Err(e) = state.full(params, pcm) {
        warn!(broadcast = %broadcast, error = ?e, "whisper full() failed");
        return 0;
    }

    let n_segments = state.full_n_segments();
    let mut emitted = 0usize;
    for i in 0..n_segments {
        let Some(seg) = state.get_segment(i) else {
            continue;
        };
        let text = match seg.to_str_lossy() {
            Ok(t) => t.into_owned(),
            Err(e) => {
                warn!(broadcast = %broadcast, segment = i, error = ?e, "skip segment text");
                continue;
            }
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Whisper segment timestamps are in centiseconds
        // (1/100 sec). Convert to source timescale and offset
        // by the window's starting fragment DTS so consumers
        // can align captions against the source DTS axis.
        let t0_cs = seg.start_timestamp().max(0) as u64;
        let t1_cs = seg.end_timestamp().max(0) as u64;
        let start_ts = window_start_dts + t0_cs * source_sample_rate as u64 / 100;
        let end_ts = window_start_dts + t1_cs * source_sample_rate as u64 / 100;
        debug!(broadcast = %broadcast, start_ts, end_ts, text = %trimmed, "caption emitted");
        captions.publish(TranscribedCaption {
            broadcast: broadcast.to_string(),
            start_ts,
            end_ts,
            text: trimmed.to_string(),
        });
        if let Some(bc) = caption_broadcaster {
            // Convert source-track ticks to wall-clock UNIX ms
            // for the captions Fragment so the HLS bridge can
            // place cues on the PROGRAM-DATE-TIME axis without
            // re-deriving the broadcast anchor. `now()` at
            // publish time is a reasonable proxy for the cue's
            // wall-clock; cues lag inference by at most one
            // window, which the HLS bridge documents as a v1
            // limitation.
            let duration_ms = end_ts.saturating_sub(start_ts) * 1000 / source_sample_rate.max(1) as u64;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let frag = Fragment::new(
                "captions",
                emitted as u64,
                0,
                0,
                now_ms,
                now_ms,
                duration_ms,
                FragmentFlags::KEYFRAME,
                Bytes::from(trimmed.to_string()),
            );
            bc.emit(frag);
        }
        emitted += 1;
    }
    emitted
}
