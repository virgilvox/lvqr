//! RTCP Sender Report generation for the PLAY egress path.
//!
//! RFC 3550 section 6.4.1 defines the SR packet: a 20-byte fixed
//! block (version / length / SSRC / NTP / RTP timestamp / packet
//! count / octet count) optionally followed by reception reports.
//! LVQR's PLAY path is send-only, so we never attach reception
//! reports; the packet is a flat 28 bytes on the wire.
//!
//! Two pieces land here:
//!
//! * [`RtpStats`]: lock-free sender-side counters updated once per
//!   emitted RTP packet. Atomic so the drain (writer) and the SR
//!   timer task (reader) never block each other.
//! * [`spawn_sr_task`]: a per-drain tokio task that wakes on a fixed
//!   interval, snapshots the stats, and pushes an RTSP interleaved
//!   RTCP frame onto the connection's writer channel.
//!
//! The drain's only contract is to call [`RtpStats::record_packet`]
//! after a successful RTP send. Terminations flow through the shared
//! [`CancellationToken`] and the mpsc writer channel: when either
//! trips, the SR task exits cleanly without holding any reference
//! to the broadcaster.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Interleaved-frame magic byte (RFC 2326 section 10.12).
const INTERLEAVED_MAGIC: u8 = 0x24;

/// Size on the wire of a bare SR packet with no reception reports.
/// Matches RFC 3550 section 6.4.1: 4-byte header + 24-byte sender info.
pub const SR_PACKET_LEN: usize = 28;

/// RTCP Sender Report payload type per IANA allocation.
const RTCP_PT_SR: u8 = 200;

/// Offset between the NTP epoch (1900-01-01) and the UNIX epoch
/// (1970-01-01) in seconds. RFC 5905 gives this constant directly.
const NTP_UNIX_OFFSET_SECS: u64 = 2_208_988_800;

/// Sender-side RTP stream counters. One instance per PLAY drain.
///
/// Packet and octet counts are cumulative `u32` that wrap on 32-bit
/// overflow, matching the SR wire format. `last_ts_state` packs the
/// most recently emitted RTP timestamp with a validity flag: the
/// high bit is set by the first `record_packet` call, and the low
/// 32 bits carry the timestamp itself. A fresh stats record with no
/// packets yet snapshots to `None`.
#[derive(Debug, Default)]
pub struct RtpStats {
    packet_count: AtomicU32,
    octet_count: AtomicU32,
    last_ts_state: AtomicU64,
}

const LAST_TS_VALID_BIT: u64 = 1u64 << 63;

impl RtpStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one RTP packet's worth of payload octets plus the RTP
    /// timestamp it carried. `payload_octets` is the RTP packet size
    /// minus the fixed 12-byte header (LVQR packetizers never emit
    /// CSRC lists or extensions, so the header is always 12 bytes).
    pub fn record_packet(&self, payload_octets: u32, rtp_ts: u32) {
        self.packet_count.fetch_add(1, Ordering::Relaxed);
        self.octet_count.fetch_add(payload_octets, Ordering::Relaxed);
        self.last_ts_state
            .store(LAST_TS_VALID_BIT | u64::from(rtp_ts), Ordering::Release);
    }

    /// Snapshot the current counters. Returns `None` when the drain
    /// has not emitted a packet yet -- the SR timer uses that to
    /// suppress an opening SR with all-zero sender info.
    pub fn snapshot(&self) -> Option<RtpStatsSnapshot> {
        let state = self.last_ts_state.load(Ordering::Acquire);
        if state & LAST_TS_VALID_BIT == 0 {
            return None;
        }
        Some(RtpStatsSnapshot {
            packet_count: self.packet_count.load(Ordering::Relaxed),
            octet_count: self.octet_count.load(Ordering::Relaxed),
            last_rtp_ts: state as u32,
        })
    }
}

/// Point-in-time view of an [`RtpStats`] record suitable for direct
/// rendering into an SR packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpStatsSnapshot {
    pub packet_count: u32,
    pub octet_count: u32,
    pub last_rtp_ts: u32,
}

/// Convert `SystemTime::now()` into a 64-bit NTP timestamp. The high
/// 32 bits carry seconds since 1900; the low 32 bits carry a
/// fractional second in `1 / 2^32` units. RFC 5905 section 6.
///
/// Pulled out as a separate fn so tests can freeze the clock by
/// calling [`write_sender_report`] with a fixed NTP value.
pub fn system_time_to_ntp(now: SystemTime) -> u64 {
    let dur = now.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let secs = dur.as_secs().saturating_add(NTP_UNIX_OFFSET_SECS);
    // (nanos / 1e9) * 2^32, rearranged to keep precision inside u64.
    let frac = (u64::from(dur.subsec_nanos()) * (1u64 << 32)) / 1_000_000_000;
    (secs << 32) | frac
}

/// Serialize one Sender Report into `buf`. RFC 3550 section 6.4.1.
///
/// No reception reports are appended. The header length field encodes
/// the total size in 32-bit words minus one, so `SR_PACKET_LEN / 4 - 1
/// = 6`.
pub fn write_sender_report(
    buf: &mut Vec<u8>,
    ssrc: u32,
    ntp: u64,
    rtp_timestamp: u32,
    packet_count: u32,
    octet_count: u32,
) {
    let start = buf.len();
    buf.push(0x80); // V=2, P=0, RC=0
    buf.push(RTCP_PT_SR);
    buf.extend_from_slice(&6u16.to_be_bytes());
    buf.extend_from_slice(&ssrc.to_be_bytes());
    buf.extend_from_slice(&ntp.to_be_bytes());
    buf.extend_from_slice(&rtp_timestamp.to_be_bytes());
    buf.extend_from_slice(&packet_count.to_be_bytes());
    buf.extend_from_slice(&octet_count.to_be_bytes());
    debug_assert_eq!(buf.len() - start, SR_PACKET_LEN);
}

/// Wrap a serialized RTCP packet in an RTSP interleaved TCP frame on
/// the given channel. The length field is 16-bit big-endian, matching
/// RFC 2326 section 10.12.
pub fn wrap_interleaved(channel: u8, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.push(INTERLEAVED_MAGIC);
    frame.push(channel);
    let len = u16::try_from(body.len()).expect("RTCP frame exceeds 16-bit length field");
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Spawn a per-drain SR timer.
///
/// The task fires every `interval` starting at `now + interval` (so
/// the first SR is never a zero-packet report), snapshots `stats`,
/// and pushes an interleaved SR frame onto `writer_tx` at
/// `rtcp_channel`. Exits when `cancel` fires, when `stats.snapshot()`
/// is still `None` _and_ the channel closes under the first send, or
/// when `writer_tx` is dropped.
///
/// The returned `JoinHandle` is owned by the drain; the caller is
/// expected to `.await` or `.abort()` it on drain termination so the
/// task does not outlive its logical session.
pub fn spawn_sr_task(
    ssrc: u32,
    stats: Arc<RtpStats>,
    rtcp_channel: u8,
    writer_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
    interval: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let start = tokio::time::Instant::now() + interval;
        let mut ticker = tokio::time::interval_at(start, interval);
        // Delay on missed ticks rather than bursting catch-up SRs.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    let Some(snap) = stats.snapshot() else {
                        // No RTP packets emitted yet. Skip this tick;
                        // the next will pick up once the drain starts
                        // producing.
                        continue;
                    };
                    let mut sr = Vec::with_capacity(SR_PACKET_LEN);
                    write_sender_report(
                        &mut sr,
                        ssrc,
                        system_time_to_ntp(SystemTime::now()),
                        snap.last_rtp_ts,
                        snap.packet_count,
                        snap.octet_count,
                    );
                    let frame = wrap_interleaved(rtcp_channel, &sr);
                    if writer_tx.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_stats_snapshot_none_before_first_packet() {
        let stats = RtpStats::new();
        assert!(stats.snapshot().is_none());
    }

    #[test]
    fn rtp_stats_counters_accumulate() {
        let stats = RtpStats::new();
        stats.record_packet(100, 0);
        stats.record_packet(250, 960);
        stats.record_packet(75, 1920);
        let snap = stats.snapshot().expect("snapshot");
        assert_eq!(snap.packet_count, 3);
        assert_eq!(snap.octet_count, 100 + 250 + 75);
        assert_eq!(snap.last_rtp_ts, 1920);
    }

    #[test]
    fn rtp_stats_zero_timestamp_still_reads_valid_after_record() {
        // Re-injection packets from the H.264 / HEVC drains carry
        // rtp_ts = 0. Ensure the valid-bit path is encoded by record
        // flag, not by the timestamp value itself.
        let stats = RtpStats::new();
        stats.record_packet(42, 0);
        let snap = stats.snapshot().expect("snapshot valid despite ts=0");
        assert_eq!(snap.last_rtp_ts, 0);
        assert_eq!(snap.packet_count, 1);
    }

    #[test]
    fn write_sender_report_matches_rfc_3550_layout() {
        let mut buf = Vec::new();
        write_sender_report(&mut buf, 0xDEADBEEF, 0x0123_4567_89AB_CDEF, 0xCAFEBABE, 7, 1234);
        assert_eq!(buf.len(), SR_PACKET_LEN);
        // V=2, P=0, RC=0.
        assert_eq!(buf[0], 0x80);
        // PT = 200 (SR).
        assert_eq!(buf[1], 200);
        // Length in 32-bit words - 1 = (28/4) - 1 = 6.
        assert_eq!(u16::from_be_bytes([buf[2], buf[3]]), 6);
        // SSRC.
        assert_eq!(&buf[4..8], &0xDEADBEEFu32.to_be_bytes());
        // NTP timestamp.
        assert_eq!(&buf[8..16], &0x0123_4567_89AB_CDEFu64.to_be_bytes());
        // RTP timestamp.
        assert_eq!(&buf[16..20], &0xCAFEBABEu32.to_be_bytes());
        // Packet count.
        assert_eq!(&buf[20..24], &7u32.to_be_bytes());
        // Octet count.
        assert_eq!(&buf[24..28], &1234u32.to_be_bytes());
    }

    #[test]
    fn system_time_to_ntp_epoch_maps_to_ntp_offset() {
        let ntp = system_time_to_ntp(UNIX_EPOCH);
        // Fractional should be zero; seconds equal the UNIX->NTP offset.
        assert_eq!(ntp >> 32, NTP_UNIX_OFFSET_SECS);
        assert_eq!(ntp as u32, 0);
    }

    #[test]
    fn system_time_to_ntp_monotonic_across_arbitrary_instant() {
        let a = system_time_to_ntp(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let b = system_time_to_ntp(UNIX_EPOCH + Duration::from_secs(1_700_000_001));
        assert!(b > a);
        assert_eq!((b >> 32) - (a >> 32), 1, "one second delta");
    }

    #[test]
    fn wrap_interleaved_encodes_magic_channel_and_length() {
        let sr = [0xAAu8; SR_PACKET_LEN];
        let frame = wrap_interleaved(3, &sr);
        assert_eq!(frame[0], INTERLEAVED_MAGIC);
        assert_eq!(frame[1], 3);
        assert_eq!(u16::from_be_bytes([frame[2], frame[3]]) as usize, SR_PACKET_LEN);
        assert_eq!(&frame[4..], &sr[..]);
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_sr_task_emits_on_interval_after_packets_recorded() {
        let stats = Arc::new(RtpStats::new());
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
        let cancel = CancellationToken::new();
        let handle = spawn_sr_task(
            0x1234_5678,
            stats.clone(),
            1,
            tx,
            cancel.clone(),
            Duration::from_secs(5),
        );

        // Let the spawned task reach its first poll so the interval is
        // armed before advance() drives timers.
        tokio::task::yield_now().await;
        stats.record_packet(100, 0x8000_0000);
        tokio::time::sleep(Duration::from_secs(6)).await;

        let frame = rx.try_recv().expect("SR available after 6s sleep");
        assert_eq!(frame[0], INTERLEAVED_MAGIC);
        assert_eq!(frame[1], 1, "RTCP on the odd channel");
        let sr = &frame[4..];
        assert_eq!(sr[1], RTCP_PT_SR);
        // SSRC.
        assert_eq!(&sr[4..8], &0x1234_5678u32.to_be_bytes());
        // RTP timestamp = last recorded.
        assert_eq!(&sr[16..20], &0x8000_0000u32.to_be_bytes());
        // Packet count = 1, octet count = 100.
        assert_eq!(&sr[20..24], &1u32.to_be_bytes());
        assert_eq!(&sr[24..28], &100u32.to_be_bytes());

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_sr_task_skips_tick_when_no_packets_yet() {
        let stats = Arc::new(RtpStats::new());
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
        let cancel = CancellationToken::new();
        let handle = spawn_sr_task(1, stats, 1, tx, cancel.clone(), Duration::from_secs(5));

        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;

        // No packets recorded; expect no SR, then timeout.
        let res = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(res.is_err(), "no SR emitted before the first recorded packet");

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_sr_task_exits_when_cancel_fires() {
        let stats = Arc::new(RtpStats::new());
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(4);
        let cancel = CancellationToken::new();
        let handle = spawn_sr_task(1, stats, 1, tx, cancel.clone(), Duration::from_secs(5));
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("sr task exits on cancel")
            .expect("task join");
    }
}
