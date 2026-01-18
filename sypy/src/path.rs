//! Python wrapper for SyncPath

use pyo3::prelude::*;

/// Represents a sync path that can be local, remote (SSH), S3, GCS, or daemon
#[pyclass(name = "SyncPath")]
#[derive(Clone, Debug)]
pub struct PySyncPath {
    inner: sy::path::SyncPath,
}

#[pymethods]
impl PySyncPath {
    /// Create a new SyncPath by parsing a path string
    ///
    /// Supported formats:
    /// - Local: `/path/to/dir`, `./relative/path`
    /// - Remote: `user@host:/path`, `host:/path`
    /// - S3: `s3://bucket/key`
    /// - GCS: `gs://bucket/key`
    /// - Daemon: `daemon:/path`
    #[new]
    pub fn new(path: &str) -> Self {
        PySyncPath {
            inner: sy::path::SyncPath::parse(path),
        }
    }

    /// Get the path component as a string
    #[getter]
    pub fn path(&self) -> String {
        self.inner.path().to_string_lossy().to_string()
    }

    /// Check if this is a local path
    #[getter]
    pub fn is_local(&self) -> bool {
        self.inner.is_local()
    }

    /// Check if this is a remote SSH path
    #[getter]
    pub fn is_remote(&self) -> bool {
        self.inner.is_remote()
    }

    /// Check if this is an S3 path
    #[getter]
    pub fn is_s3(&self) -> bool {
        self.inner.is_s3()
    }

    /// Check if this is a GCS path
    #[getter]
    pub fn is_gcs(&self) -> bool {
        self.inner.is_gcs()
    }

    /// Check if this is a daemon path
    #[getter]
    pub fn is_daemon(&self) -> bool {
        self.inner.is_daemon()
    }

    /// Check if the path has a trailing slash
    #[getter]
    pub fn has_trailing_slash(&self) -> bool {
        self.inner.has_trailing_slash()
    }

    /// Get the host for remote paths (None for local paths)
    #[getter]
    pub fn host(&self) -> Option<String> {
        match &self.inner {
            sy::path::SyncPath::Remote { host, .. } => Some(host.clone()),
            _ => None,
        }
    }

    /// Get the user for remote paths (None for local paths or paths without user)
    #[getter]
    pub fn user(&self) -> Option<String> {
        match &self.inner {
            sy::path::SyncPath::Remote { user, .. } => user.clone(),
            _ => None,
        }
    }

    /// Get the bucket for S3/GCS paths (None for other path types)
    #[getter]
    pub fn bucket(&self) -> Option<String> {
        match &self.inner {
            sy::path::SyncPath::S3 { bucket, .. } => Some(bucket.clone()),
            sy::path::SyncPath::Gcs { bucket, .. } => Some(bucket.clone()),
            _ => None,
        }
    }

    fn __str__(&self) -> String {
        self.inner.to_string()
    }

    fn __repr__(&self) -> String {
        format!("SyncPath('{}')", self.inner)
    }
}

impl PySyncPath {
    /// Get the inner SyncPath
    pub fn inner(&self) -> &sy::path::SyncPath {
        &self.inner
    }

    /// Create from an inner SyncPath
    pub fn from_inner(inner: sy::path::SyncPath) -> Self {
        PySyncPath { inner }
    }
}

/// Parse a path string into a SyncPath
#[pyfunction]
pub fn parse_path(path: &str) -> PySyncPath {
    PySyncPath::new(path)
}
