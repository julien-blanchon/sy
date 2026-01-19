//! Python wrapper for SyncStats and SyncError

use crate::dryrun::PyDryRunDetails;
use pyo3::prelude::*;

/// Error information from a sync operation
#[pyclass(name = "SyncError")]
#[derive(Clone, Debug)]
pub struct PySyncError {
    /// Path that caused the error
    #[pyo3(get)]
    pub path: String,
    /// Error message
    #[pyo3(get)]
    pub error: String,
    /// Action that was being performed
    #[pyo3(get)]
    pub action: String,
}

#[pymethods]
impl PySyncError {
    fn __str__(&self) -> String {
        format!("[{}] {}: {}", self.action, self.path, self.error)
    }

    fn __repr__(&self) -> String {
        format!(
            "SyncError(path='{}', action='{}', error='{}')",
            self.path, self.action, self.error
        )
    }
}

impl From<&sy::sync::SyncError> for PySyncError {
    fn from(err: &sy::sync::SyncError) -> Self {
        PySyncError {
            path: err.path.to_string_lossy().to_string(),
            error: err.error.clone(),
            action: err.action.clone(),
        }
    }
}

/// Statistics from a sync operation
#[pyclass(name = "SyncStats")]
#[derive(Clone, Debug)]
pub struct PySyncStats {
    /// Number of files scanned
    #[pyo3(get)]
    pub files_scanned: u64,
    /// Number of files created
    #[pyo3(get)]
    pub files_created: u64,
    /// Number of files updated
    #[pyo3(get)]
    pub files_updated: u64,
    /// Number of files skipped (already up-to-date)
    #[pyo3(get)]
    pub files_skipped: usize,
    /// Number of files deleted
    #[pyo3(get)]
    pub files_deleted: usize,
    /// Total bytes transferred
    #[pyo3(get)]
    pub bytes_transferred: u64,
    /// Number of files synced using delta algorithm
    #[pyo3(get)]
    pub files_delta_synced: usize,
    /// Bytes saved by delta sync
    #[pyo3(get)]
    pub delta_bytes_saved: u64,
    /// Number of files compressed during transfer
    #[pyo3(get)]
    pub files_compressed: usize,
    /// Bytes saved by compression
    #[pyo3(get)]
    pub compression_bytes_saved: u64,
    /// Number of files verified
    #[pyo3(get)]
    pub files_verified: usize,
    /// Number of verification failures
    #[pyo3(get)]
    pub verification_failures: usize,
    /// Duration of the sync operation in seconds
    #[pyo3(get)]
    pub duration_secs: f64,
    /// Bytes that would be added (dry-run)
    #[pyo3(get)]
    pub bytes_would_add: u64,
    /// Bytes that would change (dry-run)
    #[pyo3(get)]
    pub bytes_would_change: u64,
    /// Bytes that would be deleted (dry-run)
    #[pyo3(get)]
    pub bytes_would_delete: u64,
    /// Number of directories created
    #[pyo3(get)]
    pub dirs_created: u64,
    /// Number of symlinks created
    #[pyo3(get)]
    pub symlinks_created: u64,
    /// List of errors encountered
    errors: Vec<PySyncError>,

    /// Detailed dry-run information (only present when dry_run=True)
    dry_run_details_data: Option<PyDryRunDetails>,
}

#[pymethods]
impl PySyncStats {
    /// Get the list of errors
    #[getter]
    pub fn errors(&self) -> Vec<PySyncError> {
        self.errors.clone()
    }

    /// Check if the sync completed without errors
    #[getter]
    pub fn success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Get the transfer rate in bytes per second
    #[getter]
    pub fn transfer_rate(&self) -> f64 {
        if self.duration_secs > 0.0 {
            self.bytes_transferred as f64 / self.duration_secs
        } else {
            0.0
        }
    }

    /// Get a clean, typed summary for dry-run results
    /// Returns a dict with 'would_create', 'would_update', 'would_delete' keys
    #[getter]
    pub fn dry_run_summary(&self, py: Python<'_>) -> PyResult<PyObject> {
        use pyo3::types::PyDict;

        let dict = PyDict::new_bound(py);

        // would_create
        let create = PyDict::new_bound(py);
        create.set_item("count", self.files_created)?;
        create.set_item("bytes", self.bytes_would_add)?;
        dict.set_item("would_create", create)?;

        // would_update
        let update = PyDict::new_bound(py);
        update.set_item("count", self.files_updated)?;
        update.set_item("bytes", self.bytes_would_change)?;
        dict.set_item("would_update", update)?;

        // would_delete
        let delete = PyDict::new_bound(py);
        delete.set_item("count", self.files_deleted)?;
        delete.set_item("bytes", self.bytes_would_delete)?;
        dict.set_item("would_delete", delete)?;

        // totals
        dict.set_item(
            "total_files",
            self.files_created + self.files_updated + self.files_deleted as u64,
        )?;
        dict.set_item(
            "total_bytes",
            self.bytes_would_add + self.bytes_would_change + self.bytes_would_delete,
        )?;

        Ok(dict.into())
    }

    /// Get detailed dry-run information with file-level changes
    /// Returns None if not a dry-run, otherwise returns DryRunDetails object
    #[getter]
    pub fn dry_run_details(&self) -> Option<PyDryRunDetails> {
        self.dry_run_details_data.clone()
    }

    fn __str__(&self) -> String {
        format!(
            "SyncStats(scanned={}, created={}, updated={}, skipped={}, deleted={}, bytes={}, duration={:.2}s)",
            self.files_scanned,
            self.files_created,
            self.files_updated,
            self.files_skipped,
            self.files_deleted,
            self.bytes_transferred,
            self.duration_secs
        )
    }

    fn __repr__(&self) -> String {
        self.__str__()
    }
}

impl From<sy::sync::SyncStats> for PySyncStats {
    fn from(stats: sy::sync::SyncStats) -> Self {
        PySyncStats {
            files_scanned: stats.files_scanned,
            files_created: stats.files_created,
            files_updated: stats.files_updated,
            files_skipped: stats.files_skipped,
            files_deleted: stats.files_deleted,
            bytes_transferred: stats.bytes_transferred,
            files_delta_synced: stats.files_delta_synced,
            delta_bytes_saved: stats.delta_bytes_saved,
            files_compressed: stats.files_compressed,
            compression_bytes_saved: stats.compression_bytes_saved,
            files_verified: stats.files_verified,
            verification_failures: stats.verification_failures,
            duration_secs: stats.duration.as_secs_f64(),
            bytes_would_add: stats.bytes_would_add,
            bytes_would_change: stats.bytes_would_change,
            bytes_would_delete: stats.bytes_would_delete,
            dirs_created: stats.dirs_created,
            symlinks_created: stats.symlinks_created,
            errors: stats.errors.iter().map(PySyncError::from).collect(),
            dry_run_details_data: stats.dry_run_details.as_ref().map(PyDryRunDetails::from),
        }
    }
}
