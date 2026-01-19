//! Live progress tracking for sync/transfer operations.
//!
//! This is a low-overhead, backend-agnostic progress state that can be updated
//! from any sync path (SyncEngine, server mode, daemon mode) and sampled on a timer
//! (e.g., from Python) to drive progress bars/logging.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A point-in-time snapshot of current sync progress.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    /// Estimated total bytes to process (sum of source file sizes for transfers).
    pub total_bytes: u64,
    /// Bytes completed so far (best-effort; may be exact for streaming transfers).
    pub bytes: u64,
    /// Estimated instantaneous speed in bytes/sec (computed when sampling).
    pub bytes_per_sec: u64,
    /// Transfers completed so far (files/symlinks created/updated).
    pub transfers: usize,
    /// Total number of transfers planned.
    pub total_transfers: usize,
    /// Number of active in-flight transfers.
    pub active_transfers: usize,
    /// Currently transferring paths (may be multiple when parallel).
    pub transferring: Vec<PathBuf>,
    /// Percentage complete (0-100). `None` if total_bytes is 0.
    pub percentage: Option<f64>,
    /// Elapsed time since progress tracking started.
    pub elapsed: Duration,
}

/// Shared, thread-safe progress state.
///
/// This is intentionally transport-agnostic and uses atomics + small mutex-protected
/// maps for per-file streaming deltas.
#[allow(dead_code)]
#[derive(Debug)]
pub struct ProgressState {
    started_at: Instant,

    total_bytes: AtomicU64,
    bytes: AtomicU64,

    transfers: AtomicUsize,
    total_transfers: AtomicUsize,
    active_transfers: AtomicUsize,

    // Track currently transferring files (for UI). Also used for delta accounting.
    in_flight: Mutex<HashMap<PathBuf, InFlightFile>>,

    // Used to stop external sampling loops cleanly.
    finished: AtomicBool,
}

#[derive(Debug, Clone)]
struct InFlightFile {
    last_bytes: u64,
}

#[allow(dead_code)]
impl ProgressState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started_at: Instant::now(),
            total_bytes: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            transfers: AtomicUsize::new(0),
            total_transfers: AtomicUsize::new(0),
            active_transfers: AtomicUsize::new(0),
            in_flight: Mutex::new(HashMap::new()),
            finished: AtomicBool::new(false),
        })
    }

    pub fn mark_finished(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    pub fn set_totals(&self, total_bytes: u64, total_transfers: usize) {
        self.total_bytes.store(total_bytes, Ordering::Relaxed);
        self.total_transfers
            .store(total_transfers, Ordering::Relaxed);
    }

    pub fn start_transfer(&self, path: PathBuf) {
        self.active_transfers.fetch_add(1, Ordering::Relaxed);
        let mut map = self.in_flight.lock().expect("progress in_flight poisoned");
        map.entry(path).or_insert(InFlightFile { last_bytes: 0 });
    }

    pub fn finish_transfer(&self, path: &PathBuf, final_bytes: u64) {
        // Ensure we account for any missing bytes for streaming transfers.
        let mut map = self.in_flight.lock().expect("progress in_flight poisoned");
        if let Some(state) = map.remove(path) {
            if final_bytes > state.last_bytes {
                let delta = final_bytes - state.last_bytes;
                self.bytes.fetch_add(delta, Ordering::Relaxed);
            }
        }
        self.active_transfers.fetch_sub(1, Ordering::Relaxed);
        self.transfers.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment bytes when we only know a final count (non-streaming transfer).
    pub fn add_bytes(&self, bytes: u64) {
        if bytes > 0 {
            self.bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Update bytes for a specific in-flight file using an absolute position.
    ///
    /// This is designed to be called from streaming callbacks.
    pub fn update_file_position(&self, path: &PathBuf, current_bytes: u64) {
        let mut map = self.in_flight.lock().expect("progress in_flight poisoned");
        if let Some(state) = map.get_mut(path) {
            if current_bytes > state.last_bytes {
                let delta = current_bytes - state.last_bytes;
                state.last_bytes = current_bytes;
                self.bytes.fetch_add(delta, Ordering::Relaxed);
            }
        }
    }

    /// Build a snapshot. `bytes_per_sec` is computed relative to `prev_bytes/prev_at`.
    pub fn snapshot_with_speed(
        &self,
        prev_bytes: u64,
        prev_at: Instant,
        now: Instant,
    ) -> ProgressSnapshot {
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);
        let bytes = self.bytes.load(Ordering::Relaxed);
        let transfers = self.transfers.load(Ordering::Relaxed);
        let total_transfers = self.total_transfers.load(Ordering::Relaxed);
        let active_transfers = self.active_transfers.load(Ordering::Relaxed);
        let elapsed = now.duration_since(self.started_at);

        let dt = now.saturating_duration_since(prev_at);
        let bytes_per_sec = if dt.as_secs_f64() > 0.0 && bytes >= prev_bytes {
            ((bytes - prev_bytes) as f64 / dt.as_secs_f64()) as u64
        } else {
            0
        };

        let transferring = {
            let map = self.in_flight.lock().expect("progress in_flight poisoned");
            map.keys().cloned().collect::<Vec<_>>()
        };

        let percentage = if total_bytes > 0 {
            Some((bytes as f64 / total_bytes as f64) * 100.0)
        } else {
            None
        };

        ProgressSnapshot {
            total_bytes,
            bytes,
            bytes_per_sec,
            transfers,
            total_transfers,
            active_transfers,
            transferring,
            percentage,
            elapsed,
        }
    }
}
