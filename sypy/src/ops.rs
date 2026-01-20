//! Python bindings for file operations (get, put, rm)

use crate::config::{PyGcsConfig, PyS3Config, PySshConfig};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::path::Path;
use sy::ops::{
    DownloadOptions, DownloadResult, RemoveOptions, RemoveResult, UploadOptions, UploadResult,
};
use sy::path::SyncPath;

/// Options for get (download) operations
#[pyclass(name = "GetOptions")]
#[derive(Clone, Debug, Default)]
pub struct PyGetOptions {
    /// Recursive download (for directories)
    #[pyo3(get, set)]
    pub recursive: bool,
    /// Maximum depth for recursive download
    #[pyo3(get, set)]
    pub max_depth: Option<usize>,
    /// Preview changes without actually downloading
    #[pyo3(get, set)]
    pub dry_run: bool,
    /// Number of parallel downloads
    #[pyo3(get, set)]
    pub parallel: usize,
    /// Use SFTP instead of server protocol for SSH
    #[pyo3(get, set)]
    pub sftp: bool,
    /// Include patterns for filtering
    #[pyo3(get, set)]
    pub include: Vec<String>,
    /// Exclude patterns for filtering
    #[pyo3(get, set)]
    pub exclude: Vec<String>,
    /// Daemon socket path (for daemon:/ paths)
    #[pyo3(get, set)]
    pub daemon_socket: Option<String>,
}

#[pymethods]
impl PyGetOptions {
    #[new]
    #[pyo3(signature = (
        recursive = false,
        max_depth = None,
        dry_run = false,
        parallel = 8,
        sftp = false,
        include = Vec::new(),
        exclude = Vec::new(),
        daemon_socket = None
    ))]
    pub fn new(
        recursive: bool,
        max_depth: Option<usize>,
        dry_run: bool,
        parallel: usize,
        sftp: bool,
        include: Vec<String>,
        exclude: Vec<String>,
        daemon_socket: Option<String>,
    ) -> Self {
        Self {
            recursive,
            max_depth,
            dry_run,
            parallel,
            sftp,
            include,
            exclude,
            daemon_socket,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "GetOptions(recursive={}, dry_run={}, parallel={})",
            self.recursive, self.dry_run, self.parallel
        )
    }
}

impl PyGetOptions {
    pub fn to_rust(&self) -> DownloadOptions {
        let mut opts = DownloadOptions::new().with_parallel(self.parallel);
        if self.recursive {
            opts = opts.recursive();
        }
        if let Some(depth) = self.max_depth {
            opts = opts.with_max_depth(depth);
        }
        if self.dry_run {
            opts = opts.dry_run();
        }
        if self.sftp {
            opts = opts.use_sftp();
        }
        opts = opts.with_include(self.include.clone());
        opts = opts.with_exclude(self.exclude.clone());
        if let Some(ref socket) = self.daemon_socket {
            opts = opts.with_daemon_socket(socket.clone());
        }
        opts
    }
}

/// Options for put (upload) operations
#[pyclass(name = "PutOptions")]
#[derive(Clone, Debug, Default)]
pub struct PyPutOptions {
    /// Recursive upload (for directories)
    #[pyo3(get, set)]
    pub recursive: bool,
    /// Maximum depth for recursive upload
    #[pyo3(get, set)]
    pub max_depth: Option<usize>,
    /// Preview changes without actually uploading
    #[pyo3(get, set)]
    pub dry_run: bool,
    /// Number of parallel uploads
    #[pyo3(get, set)]
    pub parallel: usize,
    /// Use SFTP instead of server protocol for SSH
    #[pyo3(get, set)]
    pub sftp: bool,
    /// Include patterns for filtering
    #[pyo3(get, set)]
    pub include: Vec<String>,
    /// Exclude patterns for filtering
    #[pyo3(get, set)]
    pub exclude: Vec<String>,
    /// Daemon socket path (for daemon:/ paths)
    #[pyo3(get, set)]
    pub daemon_socket: Option<String>,
}

#[pymethods]
impl PyPutOptions {
    #[new]
    #[pyo3(signature = (
        recursive = false,
        max_depth = None,
        dry_run = false,
        parallel = 8,
        sftp = false,
        include = Vec::new(),
        exclude = Vec::new(),
        daemon_socket = None
    ))]
    pub fn new(
        recursive: bool,
        max_depth: Option<usize>,
        dry_run: bool,
        parallel: usize,
        sftp: bool,
        include: Vec<String>,
        exclude: Vec<String>,
        daemon_socket: Option<String>,
    ) -> Self {
        Self {
            recursive,
            max_depth,
            dry_run,
            parallel,
            sftp,
            include,
            exclude,
            daemon_socket,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PutOptions(recursive={}, dry_run={}, parallel={})",
            self.recursive, self.dry_run, self.parallel
        )
    }
}

impl PyPutOptions {
    pub fn to_rust(&self) -> UploadOptions {
        let mut opts = UploadOptions::new().with_parallel(self.parallel);
        if self.recursive {
            opts = opts.recursive();
        }
        if let Some(depth) = self.max_depth {
            opts = opts.with_max_depth(depth);
        }
        if self.dry_run {
            opts = opts.dry_run();
        }
        if self.sftp {
            opts = opts.use_sftp();
        }
        opts = opts.with_include(self.include.clone());
        opts = opts.with_exclude(self.exclude.clone());
        if let Some(ref socket) = self.daemon_socket {
            opts = opts.with_daemon_socket(socket.clone());
        }
        opts
    }
}

/// Options for rm (remove) operations
#[pyclass(name = "RemoveOptions")]
#[derive(Clone, Debug, Default)]
pub struct PyRemoveOptions {
    /// Recursive removal (for directories)
    #[pyo3(get, set)]
    pub recursive: bool,
    /// Maximum depth for recursive removal
    #[pyo3(get, set)]
    pub max_depth: Option<usize>,
    /// Preview changes without actually removing
    #[pyo3(get, set)]
    pub dry_run: bool,
    /// Remove empty directories after removing files
    #[pyo3(get, set)]
    pub rmdirs: bool,
    /// Use SFTP instead of server protocol for SSH
    #[pyo3(get, set)]
    pub sftp: bool,
    /// Include patterns for filtering
    #[pyo3(get, set)]
    pub include: Vec<String>,
    /// Exclude patterns for filtering
    #[pyo3(get, set)]
    pub exclude: Vec<String>,
    /// Daemon socket path (for daemon:/ paths)
    #[pyo3(get, set)]
    pub daemon_socket: Option<String>,
}

#[pymethods]
impl PyRemoveOptions {
    #[new]
    #[pyo3(signature = (
        recursive = false,
        max_depth = None,
        dry_run = false,
        rmdirs = false,
        sftp = true,
        include = Vec::new(),
        exclude = Vec::new(),
        daemon_socket = None
    ))]
    pub fn new(
        recursive: bool,
        max_depth: Option<usize>,
        dry_run: bool,
        rmdirs: bool,
        sftp: bool,
        include: Vec<String>,
        exclude: Vec<String>,
        daemon_socket: Option<String>,
    ) -> Self {
        Self {
            recursive,
            max_depth,
            dry_run,
            rmdirs,
            sftp,
            include,
            exclude,
            daemon_socket,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "RemoveOptions(recursive={}, dry_run={}, rmdirs={}, sftp={})",
            self.recursive, self.dry_run, self.rmdirs, self.sftp
        )
    }
}

impl PyRemoveOptions {
    pub fn to_rust(&self) -> RemoveOptions {
        let mut opts = RemoveOptions::new();
        if self.recursive {
            opts = opts.recursive();
        }
        if let Some(depth) = self.max_depth {
            opts = opts.with_max_depth(depth);
        }
        if self.dry_run {
            opts = opts.dry_run();
        }
        if self.rmdirs {
            opts = opts.remove_dirs();
        }
        if self.sftp {
            opts = opts.use_sftp();
        }
        opts = opts.with_include(self.include.clone());
        opts = opts.with_exclude(self.exclude.clone());
        if let Some(ref socket) = self.daemon_socket {
            opts = opts.with_daemon_socket(socket.clone());
        }
        opts
    }
}

/// Result of a get (download) operation
#[pyclass(name = "GetResult")]
#[derive(Clone)]
pub struct PyGetResult {
    #[pyo3(get)]
    pub source: String,
    #[pyo3(get)]
    pub destination: String,
    #[pyo3(get)]
    pub downloaded_files: usize,
    #[pyo3(get)]
    pub downloaded_bytes: u64,
    #[pyo3(get)]
    pub created_dirs: usize,
    #[pyo3(get)]
    pub skipped_files: usize,
    #[pyo3(get)]
    pub failed: Vec<PyFailedTransfer>,
    #[pyo3(get)]
    pub dry_run: bool,
}

#[pymethods]
impl PyGetResult {
    /// Check if the operation was successful (no failures)
    #[getter]
    pub fn success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Convert to a dictionary
    fn to_dict(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new_bound(py);
        dict.set_item("source", &self.source)?;
        dict.set_item("destination", &self.destination)?;
        dict.set_item("downloaded_files", self.downloaded_files)?;
        dict.set_item("downloaded_bytes", self.downloaded_bytes)?;
        dict.set_item("created_dirs", self.created_dirs)?;
        dict.set_item("skipped_files", self.skipped_files)?;
        dict.set_item("dry_run", self.dry_run)?;
        dict.set_item("success", self.success())?;

        let failed_list: Vec<_> = self
            .failed
            .iter()
            .map(|f| {
                let d = PyDict::new_bound(py);
                d.set_item("path", &f.path).unwrap();
                d.set_item("error", &f.error).unwrap();
                d
            })
            .collect();
        dict.set_item("failed", failed_list)?;

        Ok(dict.into())
    }

    fn __repr__(&self) -> String {
        format!(
            "GetResult(downloaded_files={}, downloaded_bytes={}, dry_run={})",
            self.downloaded_files, self.downloaded_bytes, self.dry_run
        )
    }
}

impl From<DownloadResult> for PyGetResult {
    fn from(r: DownloadResult) -> Self {
        Self {
            source: r.source,
            destination: r.destination,
            downloaded_files: r.downloaded_files,
            downloaded_bytes: r.downloaded_bytes,
            created_dirs: r.created_dirs,
            skipped_files: r.skipped_files,
            failed: r
                .failed
                .into_iter()
                .map(|f| PyFailedTransfer {
                    path: f.path,
                    error: f.error,
                })
                .collect(),
            dry_run: r.dry_run,
        }
    }
}

/// Result of a put (upload) operation
#[pyclass(name = "PutResult")]
#[derive(Clone)]
pub struct PyPutResult {
    #[pyo3(get)]
    pub source: String,
    #[pyo3(get)]
    pub destination: String,
    #[pyo3(get)]
    pub uploaded_files: usize,
    #[pyo3(get)]
    pub uploaded_bytes: u64,
    #[pyo3(get)]
    pub created_dirs: usize,
    #[pyo3(get)]
    pub skipped_files: usize,
    #[pyo3(get)]
    pub failed: Vec<PyFailedTransfer>,
    #[pyo3(get)]
    pub dry_run: bool,
}

#[pymethods]
impl PyPutResult {
    /// Check if the operation was successful (no failures)
    #[getter]
    pub fn success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Convert to a dictionary
    fn to_dict(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new_bound(py);
        dict.set_item("source", &self.source)?;
        dict.set_item("destination", &self.destination)?;
        dict.set_item("uploaded_files", self.uploaded_files)?;
        dict.set_item("uploaded_bytes", self.uploaded_bytes)?;
        dict.set_item("created_dirs", self.created_dirs)?;
        dict.set_item("skipped_files", self.skipped_files)?;
        dict.set_item("dry_run", self.dry_run)?;
        dict.set_item("success", self.success())?;

        let failed_list: Vec<_> = self
            .failed
            .iter()
            .map(|f| {
                let d = PyDict::new_bound(py);
                d.set_item("path", &f.path).unwrap();
                d.set_item("error", &f.error).unwrap();
                d
            })
            .collect();
        dict.set_item("failed", failed_list)?;

        Ok(dict.into())
    }

    fn __repr__(&self) -> String {
        format!(
            "PutResult(uploaded_files={}, uploaded_bytes={}, dry_run={})",
            self.uploaded_files, self.uploaded_bytes, self.dry_run
        )
    }
}

impl From<UploadResult> for PyPutResult {
    fn from(r: UploadResult) -> Self {
        Self {
            source: r.source,
            destination: r.destination,
            uploaded_files: r.uploaded_files,
            uploaded_bytes: r.uploaded_bytes,
            created_dirs: r.created_dirs,
            skipped_files: r.skipped_files,
            failed: r
                .failed
                .into_iter()
                .map(|f| PyFailedTransfer {
                    path: f.path,
                    error: f.error,
                })
                .collect(),
            dry_run: r.dry_run,
        }
    }
}

/// Result of a rm (remove) operation
#[pyclass(name = "RemoveResult")]
#[derive(Clone)]
pub struct PyRemoveResult {
    #[pyo3(get)]
    pub path: String,
    #[pyo3(get)]
    pub removed_files: usize,
    #[pyo3(get)]
    pub removed_dirs: usize,
    #[pyo3(get)]
    pub failed: Vec<PyFailedTransfer>,
    #[pyo3(get)]
    pub dry_run: bool,
}

#[pymethods]
impl PyRemoveResult {
    /// Check if the operation was successful (no failures)
    #[getter]
    pub fn success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Convert to a dictionary
    fn to_dict(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new_bound(py);
        dict.set_item("path", &self.path)?;
        dict.set_item("removed_files", self.removed_files)?;
        dict.set_item("removed_dirs", self.removed_dirs)?;
        dict.set_item("dry_run", self.dry_run)?;
        dict.set_item("success", self.success())?;

        let failed_list: Vec<_> = self
            .failed
            .iter()
            .map(|f| {
                let d = PyDict::new_bound(py);
                d.set_item("path", &f.path).unwrap();
                d.set_item("error", &f.error).unwrap();
                d
            })
            .collect();
        dict.set_item("failed", failed_list)?;

        Ok(dict.into())
    }

    fn __repr__(&self) -> String {
        format!(
            "RemoveResult(removed_files={}, removed_dirs={}, dry_run={})",
            self.removed_files, self.removed_dirs, self.dry_run
        )
    }
}

impl From<RemoveResult> for PyRemoveResult {
    fn from(r: RemoveResult) -> Self {
        Self {
            path: r.path,
            removed_files: r.removed_files,
            removed_dirs: r.removed_dirs,
            failed: r
                .failed
                .into_iter()
                .map(|f| PyFailedTransfer {
                    path: f.path,
                    error: f.error,
                })
                .collect(),
            dry_run: r.dry_run,
        }
    }
}

/// Represents a failed file transfer
#[pyclass(name = "FailedTransfer")]
#[derive(Clone)]
pub struct PyFailedTransfer {
    #[pyo3(get)]
    pub path: String,
    #[pyo3(get)]
    pub error: String,
}

#[pymethods]
impl PyFailedTransfer {
    fn __repr__(&self) -> String {
        format!(
            "FailedTransfer(path='{}', error='{}')",
            self.path, self.error
        )
    }
}

/// Download files from remote storage to local filesystem
///
/// Args:
///     source: Remote source path (SSH, S3, GCS)
///         Examples: "user@host:/path", "s3://bucket/path", "gs://bucket/path"
///     destination: Local destination path
///     recursive: Download directories recursively (default: False)
///     max_depth: Maximum depth for recursive download
///     dry_run: Preview changes without downloading (default: False)
///     parallel: Number of parallel downloads (default: 8)
///     sftp: Use SFTP instead of server protocol for SSH (default: False)
///     include: Include patterns for filtering
///     exclude: Exclude patterns for filtering
///     s3: S3Config for S3/S3-compatible storage credentials
///     gcs: GcsConfig for Google Cloud Storage credentials
///     ssh: SshConfig for SSH connection options
///
/// Returns:
///     GetResult: Statistics from the download operation
///
/// Example:
///     >>> result = sypy.get("s3://bucket/path/", "/local/dest/", recursive=True)
///     >>> print(f"Downloaded {result.downloaded_files} files")
///
///     >>> # With S3 config
///     >>> s3 = sypy.S3Config(access_key_id="...", secret_access_key="...")
///     >>> result = sypy.get("s3://bucket/data/", "/local/", recursive=True, s3=s3)
#[pyfunction]
#[pyo3(signature = (
    source,
    destination,
    recursive = false,
    max_depth = None,
    dry_run = false,
    parallel = 8,
    sftp = false,
    include = Vec::new(),
    exclude = Vec::new(),
    s3 = None,
    gcs = None,
    _ssh = None,
    daemon_socket = None
))]
#[allow(clippy::too_many_arguments)]
pub fn get(
    py: Python<'_>,
    source: &str,
    destination: &str,
    recursive: bool,
    max_depth: Option<usize>,
    dry_run: bool,
    parallel: usize,
    sftp: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    s3: Option<PyS3Config>,
    gcs: Option<PyGcsConfig>,
    _ssh: Option<PySshConfig>,
    daemon_socket: Option<String>,
) -> PyResult<PyGetResult> {
    // Apply cloud configs
    if let Some(ref s3_config) = s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = gcs {
        gcs_config.apply_to_env();
    }
    // Note: SSH config is not yet used - the ops module uses SSH config from the path

    let options = PyGetOptions {
        recursive,
        max_depth,
        dry_run,
        parallel,
        sftp,
        include,
        exclude,
        daemon_socket,
    };

    py.allow_threads(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            let sync_path = SyncPath::parse(source);
            let dest_path = Path::new(destination);
            let rust_options = options.to_rust();

            sy::ops::download(&sync_path, dest_path, &rust_options)
                .await
                .map(PyGetResult::from)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })
    })
}

/// Download files with a GetOptions object
#[pyfunction]
#[pyo3(signature = (source, destination, options, s3 = None, gcs = None, _ssh = None))]
pub fn get_with_options(
    py: Python<'_>,
    source: &str,
    destination: &str,
    options: PyGetOptions,
    s3: Option<PyS3Config>,
    gcs: Option<PyGcsConfig>,
    _ssh: Option<PySshConfig>,
) -> PyResult<PyGetResult> {
    if let Some(ref s3_config) = s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = gcs {
        gcs_config.apply_to_env();
    }
    // Note: SSH config is not yet used - the ops module uses SSH config from the path

    py.allow_threads(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            let sync_path = SyncPath::parse(source);
            let dest_path = Path::new(destination);
            let rust_options = options.to_rust();

            sy::ops::download(&sync_path, dest_path, &rust_options)
                .await
                .map(PyGetResult::from)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })
    })
}

/// Upload files from local filesystem to remote storage
///
/// Args:
///     source: Local source path
///     destination: Remote destination path (SSH, S3, GCS)
///         Examples: "user@host:/path", "s3://bucket/path", "gs://bucket/path"
///     recursive: Upload directories recursively (default: False)
///     max_depth: Maximum depth for recursive upload
///     dry_run: Preview changes without uploading (default: False)
///     parallel: Number of parallel uploads (default: 8)
///     sftp: Use SFTP instead of server protocol for SSH (default: False)
///     include: Include patterns for filtering
///     exclude: Exclude patterns for filtering
///     s3: S3Config for S3/S3-compatible storage credentials
///     gcs: GcsConfig for Google Cloud Storage credentials
///     ssh: SshConfig for SSH connection options
///
/// Returns:
///     PutResult: Statistics from the upload operation
///
/// Example:
///     >>> result = sypy.put("/local/data/", "s3://bucket/backup/", recursive=True)
///     >>> print(f"Uploaded {result.uploaded_files} files")
///
///     >>> # With GCS config
///     >>> gcs = sypy.GcsConfig(credentials_file="/path/to/key.json")
///     >>> result = sypy.put("/local/", "gs://bucket/prefix/", recursive=True, gcs=gcs)
#[pyfunction]
#[pyo3(signature = (
    source,
    destination,
    recursive = false,
    max_depth = None,
    dry_run = false,
    parallel = 8,
    sftp = false,
    include = Vec::new(),
    exclude = Vec::new(),
    s3 = None,
    gcs = None,
    _ssh = None,
    daemon_socket = None
))]
#[allow(clippy::too_many_arguments)]
pub fn put(
    py: Python<'_>,
    source: &str,
    destination: &str,
    recursive: bool,
    max_depth: Option<usize>,
    dry_run: bool,
    parallel: usize,
    sftp: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    s3: Option<PyS3Config>,
    gcs: Option<PyGcsConfig>,
    _ssh: Option<PySshConfig>,
    daemon_socket: Option<String>,
) -> PyResult<PyPutResult> {
    // Apply cloud configs
    if let Some(ref s3_config) = s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = gcs {
        gcs_config.apply_to_env();
    }
    // Note: SSH config is not yet used - the ops module uses SSH config from the path

    let options = PyPutOptions {
        recursive,
        max_depth,
        dry_run,
        parallel,
        sftp,
        include,
        exclude,
        daemon_socket,
    };

    py.allow_threads(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            let source_path = Path::new(source);
            let sync_path = SyncPath::parse(destination);
            let rust_options = options.to_rust();

            sy::ops::upload(source_path, &sync_path, &rust_options)
                .await
                .map(PyPutResult::from)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })
    })
}

/// Upload files with a PutOptions object
#[pyfunction]
#[pyo3(signature = (source, destination, options, s3 = None, gcs = None, _ssh = None))]
pub fn put_with_options(
    py: Python<'_>,
    source: &str,
    destination: &str,
    options: PyPutOptions,
    s3: Option<PyS3Config>,
    gcs: Option<PyGcsConfig>,
    _ssh: Option<PySshConfig>,
) -> PyResult<PyPutResult> {
    if let Some(ref s3_config) = s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = gcs {
        gcs_config.apply_to_env();
    }
    // Note: SSH config is not yet used - the ops module uses SSH config from the path

    py.allow_threads(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            let source_path = Path::new(source);
            let sync_path = SyncPath::parse(destination);
            let rust_options = options.to_rust();

            sy::ops::upload(source_path, &sync_path, &rust_options)
                .await
                .map(PyPutResult::from)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })
    })
}

/// Remove files from storage
///
/// Args:
///     path: Path to remove (local, SSH, S3, GCS)
///         Examples: "/path", "user@host:/path", "s3://bucket/path", "gs://bucket/path"
///     recursive: Remove directories recursively (default: False)
///     max_depth: Maximum depth for recursive removal
///     dry_run: Preview changes without removing (default: False)
///     rmdirs: Remove empty directories after removing files (default: False)
///     include: Include patterns for filtering
///     exclude: Exclude patterns for filtering
///     s3: S3Config for S3/S3-compatible storage credentials
///     gcs: GcsConfig for Google Cloud Storage credentials
///     ssh: SshConfig for SSH connection options
///
/// Returns:
///     RemoveResult: Statistics from the remove operation
///
/// Example:
///     >>> result = sypy.rm("s3://bucket/old-data/", recursive=True)
///     >>> print(f"Removed {result.removed_files} files")
///
///     >>> # Dry run to preview
///     >>> result = sypy.rm("/path/to/dir/", recursive=True, dry_run=True)
///     >>> print(f"Would remove {result.removed_files} files")
#[pyfunction]
#[pyo3(signature = (
    path,
    recursive = false,
    max_depth = None,
    dry_run = false,
    rmdirs = false,
    sftp = true,
    include = Vec::new(),
    exclude = Vec::new(),
    s3 = None,
    gcs = None,
    _ssh = None,
    daemon_socket = None
))]
#[allow(clippy::too_many_arguments)]
pub fn rm(
    py: Python<'_>,
    path: &str,
    recursive: bool,
    max_depth: Option<usize>,
    dry_run: bool,
    rmdirs: bool,
    sftp: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    s3: Option<PyS3Config>,
    gcs: Option<PyGcsConfig>,
    _ssh: Option<PySshConfig>,
    daemon_socket: Option<String>,
) -> PyResult<PyRemoveResult> {
    // Apply cloud configs
    if let Some(ref s3_config) = s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = gcs {
        gcs_config.apply_to_env();
    }
    // Note: SSH config is not yet used - the ops module uses SSH config from the path

    let options = PyRemoveOptions {
        recursive,
        max_depth,
        dry_run,
        rmdirs,
        sftp,
        include,
        exclude,
        daemon_socket,
    };

    py.allow_threads(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            let sync_path = SyncPath::parse(path);
            let rust_options = options.to_rust();

            sy::ops::remove(&sync_path, &rust_options)
                .await
                .map(PyRemoveResult::from)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })
    })
}

/// Remove files with a RemoveOptions object
#[pyfunction]
#[pyo3(signature = (path, options, s3 = None, gcs = None, _ssh = None))]
pub fn rm_with_options(
    py: Python<'_>,
    path: &str,
    options: PyRemoveOptions,
    s3: Option<PyS3Config>,
    gcs: Option<PyGcsConfig>,
    _ssh: Option<PySshConfig>,
) -> PyResult<PyRemoveResult> {
    if let Some(ref s3_config) = s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = gcs {
        gcs_config.apply_to_env();
    }
    // Note: SSH config is not yet used - the ops module uses SSH config from the path

    py.allow_threads(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            let sync_path = SyncPath::parse(path);
            let rust_options = options.to_rust();

            sy::ops::remove(&sync_path, &rust_options)
                .await
                .map(PyRemoveResult::from)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })
    })
}
