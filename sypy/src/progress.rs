//! Progress callback bridge for Python
//!
//! Provides real-time progress reporting during sync operations with configurable
//! callback frequency. The callback receives a ProgressSnapshot with statistics
//! about the current sync/transfer state.

use pyo3::prelude::*;
use pyo3::types::PyList;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sy::sync::live_progress::{ProgressSnapshot, ProgressState};

/// A point-in-time snapshot of sync progress, exposed to Python.
///
/// This class provides real-time statistics about an ongoing sync operation
/// including bytes transferred, transfer count, speed, and currently
/// transferring files.
#[pyclass(name = "ProgressSnapshot")]
#[derive(Clone, Debug)]
pub struct PyProgressSnapshot {
    /// Estimated total bytes to process
    #[pyo3(get)]
    pub total_bytes: u64,

    /// Bytes completed so far
    #[pyo3(get)]
    pub bytes: u64,

    /// Instantaneous speed in bytes per second
    #[pyo3(get)]
    pub bytes_per_sec: u64,

    /// Number of transfers (files) completed
    #[pyo3(get)]
    pub transfers: usize,

    /// Total number of transfers planned
    #[pyo3(get)]
    pub total_transfers: usize,

    /// Number of currently active (in-flight) transfers
    #[pyo3(get)]
    pub active_transfers: usize,

    /// Percentage complete (0.0 to 100.0), None if total_bytes is 0
    #[pyo3(get)]
    pub percentage: Option<f64>,

    /// Elapsed time in seconds since sync started
    #[pyo3(get)]
    pub elapsed_secs: f64,

    /// List of currently transferring file paths
    transferring_paths: Vec<String>,
}

#[pymethods]
impl PyProgressSnapshot {
    /// Get the list of currently transferring file paths
    #[getter]
    pub fn transferring(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::new_bound(py, &self.transferring_paths);
        Ok(list.into())
    }

    /// Get the current file being transferred (first in list, or None)
    #[getter]
    pub fn current_file(&self) -> Option<String> {
        self.transferring_paths.first().cloned()
    }

    /// Get speed as a human-readable string (e.g., "10.5 MB/s")
    #[getter]
    pub fn speed_human(&self) -> String {
        format_bytes_per_sec(self.bytes_per_sec)
    }

    /// Get bytes as a human-readable string (e.g., "1.5 GB")
    #[getter]
    pub fn bytes_human(&self) -> String {
        format_bytes(self.bytes)
    }

    /// Get total_bytes as a human-readable string (e.g., "10.2 GB")
    #[getter]
    pub fn total_bytes_human(&self) -> String {
        format_bytes(self.total_bytes)
    }

    /// Get estimated time remaining in seconds (None if speed is 0 or unknown)
    #[getter]
    pub fn eta_secs(&self) -> Option<f64> {
        if self.bytes_per_sec > 0 && self.total_bytes > self.bytes {
            let remaining = self.total_bytes - self.bytes;
            Some(remaining as f64 / self.bytes_per_sec as f64)
        } else {
            None
        }
    }

    fn __str__(&self) -> String {
        let pct = self
            .percentage
            .map(|p| format!("{:.1}%", p))
            .unwrap_or_else(|| "?%".to_string());
        format!(
            "ProgressSnapshot({}/{} bytes, {}, {}/{} transfers, {})",
            self.bytes,
            self.total_bytes,
            pct,
            self.transfers,
            self.total_transfers,
            self.speed_human()
        )
    }

    fn __repr__(&self) -> String {
        self.__str__()
    }
}

impl From<&ProgressSnapshot> for PyProgressSnapshot {
    fn from(snapshot: &ProgressSnapshot) -> Self {
        PyProgressSnapshot {
            total_bytes: snapshot.total_bytes,
            bytes: snapshot.bytes,
            bytes_per_sec: snapshot.bytes_per_sec,
            transfers: snapshot.transfers,
            total_transfers: snapshot.total_transfers,
            active_transfers: snapshot.active_transfers,
            percentage: snapshot.percentage,
            elapsed_secs: snapshot.elapsed.as_secs_f64(),
            transferring_paths: snapshot
                .transferring
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
        }
    }
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format bytes per second as human-readable string
fn format_bytes_per_sec(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}

/// Progress callback wrapper that can be called from Rust
///
/// The callback receives a ProgressSnapshot object with all current statistics.
#[derive(Clone)]
pub struct ProgressCallback {
    callback: Arc<PyObject>,
    frequency: Duration,
    last_call: Arc<std::sync::Mutex<Instant>>,
}

impl ProgressCallback {
    /// Create a new progress callback from a Python callable
    ///
    /// # Arguments
    /// * `callback` - Python callable that accepts a ProgressSnapshot
    /// * `frequency_ms` - Minimum time between callbacks in milliseconds
    pub fn new(callback: PyObject, frequency_ms: u64) -> Self {
        ProgressCallback {
            callback: Arc::new(callback),
            frequency: Duration::from_millis(frequency_ms),
            last_call: Arc::new(std::sync::Mutex::new(
                Instant::now() - Duration::from_secs(10),
            )), // Allow immediate first call
        }
    }

    /// Check if enough time has passed since the last call
    pub fn should_call(&self) -> bool {
        let last = self.last_call.lock().expect("progress lock poisoned");
        last.elapsed() >= self.frequency
    }

    /// Call the progress callback with a snapshot
    ///
    /// Returns Ok(()) if the callback was called successfully.
    /// Returns Err if the callback raised an exception.
    pub fn call(&self, snapshot: &ProgressSnapshot) -> PyResult<()> {
        // Check frequency
        {
            let mut last = self.last_call.lock().expect("progress lock poisoned");
            if last.elapsed() < self.frequency {
                return Ok(()); // Skip this call, too soon
            }
            *last = Instant::now();
        }

        let py_snapshot = PyProgressSnapshot::from(snapshot);
        Python::with_gil(|py| self.callback.call1(py, (py_snapshot,)).map(|_| ()))
    }

    /// Call the progress callback, ignoring any errors
    pub fn call_ignore_errors(&self, snapshot: &ProgressSnapshot) {
        let _ = self.call(snapshot);
    }

    /// Force call the callback regardless of frequency (for final update)
    pub fn force_call(&self, snapshot: &ProgressSnapshot) -> PyResult<()> {
        let py_snapshot = PyProgressSnapshot::from(snapshot);
        Python::with_gil(|py| self.callback.call1(py, (py_snapshot,)).map(|_| ()))
    }
}

/// Progress action types (kept for backwards compatibility)
pub enum ProgressAction {
    Scanning,
    Creating,
    Updating,
    Deleting,
    Verifying,
    Skipping,
}

impl ProgressAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProgressAction::Scanning => "scanning",
            ProgressAction::Creating => "creating",
            ProgressAction::Updating => "updating",
            ProgressAction::Deleting => "deleting",
            ProgressAction::Verifying => "verifying",
            ProgressAction::Skipping => "skipping",
        }
    }
}

/// Spawns a background thread that samples ProgressState at a given frequency
/// and calls the Python callback. Returns a handle to stop the thread.
pub struct ProgressSampler {
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ProgressSampler {
    /// Start sampling progress state and calling the callback
    ///
    /// # Arguments
    /// * `state` - Shared progress state to sample
    /// * `callback` - Python callback to invoke
    /// * `frequency_ms` - Sampling interval in milliseconds
    pub fn start(state: Arc<ProgressState>, callback: PyObject, frequency_ms: u64) -> Self {
        let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag_clone = Arc::clone(&stop_flag);
        let frequency = Duration::from_millis(frequency_ms);

        let handle = std::thread::spawn(move || {
            let mut prev_bytes = 0u64;
            let mut prev_at = Instant::now();

            while !stop_flag_clone.load(std::sync::atomic::Ordering::Relaxed) {
                // Check if sync is finished
                if state.is_finished() {
                    // Send final snapshot
                    let now = Instant::now();
                    let snapshot = state.snapshot_with_speed(prev_bytes, prev_at, now);
                    let py_snapshot = PyProgressSnapshot::from(&snapshot);
                    let _ = Python::with_gil(|py| callback.call1(py, (py_snapshot,)));
                    break;
                }

                // Take snapshot
                let now = Instant::now();
                let snapshot = state.snapshot_with_speed(prev_bytes, prev_at, now);

                // Update for next iteration
                prev_bytes = snapshot.bytes;
                prev_at = now;

                // Call Python callback
                let py_snapshot = PyProgressSnapshot::from(&snapshot);
                let result = Python::with_gil(|py| callback.call1(py, (py_snapshot,)));

                // If callback raises an exception, log and continue
                if let Err(e) = result {
                    eprintln!("Progress callback error: {}", e);
                }

                // Sleep until next sample
                std::thread::sleep(frequency);
            }
        });

        ProgressSampler {
            stop_flag,
            handle: Some(handle),
        }
    }

    /// Stop the sampling thread and send a final snapshot
    pub fn stop(&mut self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    /// Stop the sampling thread and send a final snapshot with the given state
    pub fn stop_with_final(mut self, state: &Arc<ProgressState>, callback: &PyObject) {
        // First stop the sampling thread
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }

        // Now send a final snapshot with the completed stats
        let now = Instant::now();
        let snapshot = state.snapshot_with_speed(0, now, now);
        let py_snapshot = PyProgressSnapshot::from(&snapshot);
        let _ = Python::with_gil(|py| callback.call1(py, (py_snapshot,)));
    }
}

impl Drop for ProgressSampler {
    fn drop(&mut self) {
        self.stop();
    }
}
