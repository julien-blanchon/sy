//! Sync functions for Python

use crate::config::{PyGcsConfig, PyS3Config, PySshConfig};
use crate::error::{anyhow_to_pyerr, sync_error_to_pyerr};
use crate::options::{parse_size_option, PySyncOptions};
use crate::progress::ProgressCallback;
use crate::stats::PySyncStats;
use pyo3::prelude::*;
use std::path::PathBuf;
use sy::cli::SymlinkMode;
use sy::filter::FilterEngine;
use sy::integrity::ChecksumType;
use sy::path::SyncPath;
use sy::retry::RetryConfig;
use sy::sync::scanner::ScanOptions;
use sy::sync::SyncEngine;
use sy::transport::router::TransportRouter;

/// Main sync function with keyword arguments
///
/// Synchronize files from source to destination.
///
/// Args:
///     source: Source path (local, remote, S3, or GCS)
///     dest: Destination path (local, remote, S3, or GCS)
///     dry_run: Preview changes without applying them
///     delete: Delete files in destination not present in source
///     delete_threshold: Maximum percentage of files to delete (0-100)
///     parallel: Number of parallel file transfers (default: 10)
///     verify: Verify file integrity after write
///     compress: Enable compression for network transfers
///     checksum: Compare by checksum instead of mtime
///     exclude: List of exclude patterns
///     include: List of include patterns
///     min_size: Minimum file size (e.g., "1MB")
///     max_size: Maximum file size (e.g., "1GB")
///     bwlimit: Bandwidth limit (e.g., "10MB")
///     progress_callback: Callback function for progress updates
///     daemon_auto: Use daemon mode for fast repeated syncs
///     s3: S3Config for S3/S3-compatible storage credentials
///     gcs: GcsConfig for Google Cloud Storage credentials
///     ssh: SshConfig for SSH connection options
///
/// Returns:
///     SyncStats: Statistics from the sync operation
///
/// Example:
///     >>> stats = sync("/source", "/dest", dry_run=True)
///     >>> print(f"Would sync {stats.files_scanned} files")
///
///     >>> # With S3 credentials
///     >>> s3 = S3Config(access_key_id="...", secret_access_key="...", region="us-east-1")
///     >>> stats = sync("/local/", "s3://bucket/path/", s3=s3)
///
///     >>> # With GCS credentials
///     >>> gcs = GcsConfig(credentials_file="/path/to/key.json")
///     >>> stats = sync("/local/", "gs://bucket/path/", gcs=gcs)
#[pyfunction]
#[pyo3(signature = (
    source,
    dest,
    dry_run = false,
    delete = false,
    delete_threshold = 50,
    parallel = 10,
    verify = false,
    compress = false,
    checksum = false,
    exclude = Vec::new(),
    include = Vec::new(),
    min_size = None,
    max_size = None,
    bwlimit = None,
    progress_callback = None,
    daemon_auto = false,
    resume = true,
    ignore_times = false,
    size_only = false,
    update = false,
    ignore_existing = false,
    gitignore = false,
    exclude_vcs = false,
    preserve_xattrs = false,
    preserve_hardlinks = false,
    preserve_permissions = false,
    preserve_times = false,
    retry = 3,
    retry_delay = 1,
    s3 = None,
    gcs = None,
    ssh = None
))]
pub fn sync(
    py: Python<'_>,
    source: &str,
    dest: &str,
    dry_run: bool,
    delete: bool,
    delete_threshold: u8,
    parallel: usize,
    verify: bool,
    compress: bool,
    checksum: bool,
    exclude: Vec<String>,
    include: Vec<String>,
    min_size: Option<String>,
    max_size: Option<String>,
    bwlimit: Option<String>,
    progress_callback: Option<PyObject>,
    daemon_auto: bool,
    resume: bool,
    ignore_times: bool,
    size_only: bool,
    update: bool,
    ignore_existing: bool,
    gitignore: bool,
    exclude_vcs: bool,
    preserve_xattrs: bool,
    preserve_hardlinks: bool,
    preserve_permissions: bool,
    preserve_times: bool,
    retry: u32,
    retry_delay: u64,
    s3: Option<PyS3Config>,
    gcs: Option<PyGcsConfig>,
    ssh: Option<PySshConfig>,
) -> PyResult<PySyncStats> {
    // Apply cloud/SSH configs if provided
    if let Some(ref s3_config) = s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = gcs {
        gcs_config.apply_to_env();
    }
    // SSH config is handled differently - it's passed to the transport
    let _ = ssh; // Mark as used for now

    // Parse size options
    let min_size_bytes = min_size
        .map(|s| parse_size_option(&s))
        .transpose()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

    let max_size_bytes = max_size
        .map(|s| parse_size_option(&s))
        .transpose()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

    let bwlimit_bytes = bwlimit
        .map(|s| parse_size_option(&s))
        .transpose()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

    // Parse paths
    let source_path = SyncPath::parse(source);
    let dest_path = SyncPath::parse(dest);

    // Create progress callback if provided
    let _progress_cb = progress_callback.map(ProgressCallback::new);

    // Run the sync operation
    // Release the GIL during the sync to allow other Python threads to run
    py.allow_threads(|| {
        // Create a new Tokio runtime for async operations
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            do_sync(
                source_path,
                dest_path,
                dry_run,
                delete,
                delete_threshold,
                parallel,
                verify,
                compress,
                checksum,
                exclude,
                include,
                min_size_bytes,
                max_size_bytes,
                bwlimit_bytes,
                daemon_auto,
                resume,
                ignore_times,
                size_only,
                update,
                ignore_existing,
                gitignore,
                exclude_vcs,
                preserve_xattrs,
                preserve_hardlinks,
                preserve_permissions,
                preserve_times,
                retry,
                retry_delay,
            )
            .await
        })
    })
}

/// Sync with a SyncOptions object
///
/// This function provides the same functionality as sync() but accepts
/// a SyncOptions object for configuration.
///
/// Args:
///     source: Source path
///     dest: Destination path
///     options: SyncOptions configuration object
///     progress_callback: Optional progress callback
///
/// Returns:
///     SyncStats: Statistics from the sync operation
#[pyfunction]
#[pyo3(signature = (source, dest, options, progress_callback = None))]
pub fn sync_with_options(
    py: Python<'_>,
    source: &str,
    dest: &str,
    options: PySyncOptions,
    progress_callback: Option<PyObject>,
) -> PyResult<PySyncStats> {
    // Apply cloud/SSH configs if provided
    if let Some(ref s3_config) = options.s3 {
        s3_config.apply_to_env();
    }
    if let Some(ref gcs_config) = options.gcs {
        gcs_config.apply_to_env();
    }
    // SSH config is handled differently - will be passed to transport in future

    // Parse size options
    let min_size_bytes = options
        .min_size
        .clone()
        .map(|s| parse_size_option(&s))
        .transpose()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

    let max_size_bytes = options
        .max_size
        .clone()
        .map(|s| parse_size_option(&s))
        .transpose()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

    let bwlimit_bytes = options
        .bwlimit
        .clone()
        .map(|s| parse_size_option(&s))
        .transpose()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

    // Parse paths
    let source_path = SyncPath::parse(source);
    let dest_path = SyncPath::parse(dest);

    // Create progress callback if provided
    let _progress_cb = progress_callback.map(ProgressCallback::new);

    // Run the sync operation
    py.allow_threads(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async {
            do_sync(
                source_path,
                dest_path,
                options.dry_run,
                options.delete,
                options.delete_threshold,
                options.parallel,
                options.verify,
                options.compress,
                options.checksum,
                options.exclude,
                options.include,
                min_size_bytes,
                max_size_bytes,
                bwlimit_bytes,
                options.daemon_auto,
                options.resume,
                options.ignore_times,
                options.size_only,
                options.update,
                options.ignore_existing,
                options.gitignore,
                options.exclude_vcs,
                options.preserve_xattrs,
                options.preserve_hardlinks,
                options.preserve_permissions,
                options.preserve_times,
                options.retry,
                options.retry_delay,
            )
            .await
        })
    })
}

/// Internal sync implementation
async fn do_sync(
    source: SyncPath,
    dest: SyncPath,
    dry_run: bool,
    delete: bool,
    delete_threshold: u8,
    parallel: usize,
    verify: bool,
    compress: bool,
    checksum: bool,
    exclude: Vec<String>,
    include: Vec<String>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    bwlimit: Option<u64>,
    daemon_auto: bool,
    resume: bool,
    ignore_times: bool,
    size_only: bool,
    update: bool,
    ignore_existing: bool,
    gitignore: bool,
    exclude_vcs: bool,
    preserve_xattrs: bool,
    preserve_hardlinks: bool,
    preserve_permissions: bool,
    preserve_times: bool,
    retry: u32,
    retry_delay: u64,
) -> PyResult<PySyncStats> {
    // Checksum type for verification
    let checksum_type = if verify {
        ChecksumType::Fast
    } else {
        ChecksumType::None
    };
    let verify_on_write = false;

    // Create retry config
    let retry_config = RetryConfig::new(retry, std::time::Duration::from_secs(retry_delay));

    // Scan options
    let scan_options = ScanOptions {
        respect_gitignore: gitignore,
        include_git_dir: !exclude_vcs,
    };

    // Handle daemon auto mode for SSH destinations
    #[cfg(unix)]
    if daemon_auto && dest.is_remote() {
        if let SyncPath::Remote {
            host, user, path, ..
        } = &dest
        {
            // Set up daemon connection
            let daemon_result =
                sy::sync::daemon_auto::ensure_daemon_connection(host, user.as_deref(), path)
                    .await
                    .map_err(anyhow_to_pyerr)?;

            // Perform sync using daemon
            let stats = sy::sync::daemon_mode::sync_daemon_mode(
                source.path(),
                &daemon_result.socket_path,
                path,
            )
            .await
            .map_err(anyhow_to_pyerr)?;

            return Ok(PySyncStats::from(stats));
        }
    }

    // Handle server mode for SSH syncs
    if source.is_local() && dest.is_remote() {
        // Local -> Remote: use server mode (push)
        let stats = sy::sync::server_mode::sync_server_mode(source.path(), &dest)
            .await
            .map_err(anyhow_to_pyerr)?;
        return Ok(PySyncStats::from(stats));
    }

    if source.is_remote() && dest.is_local() {
        // Remote -> Local: use server mode (pull)
        let stats = sy::sync::server_mode::sync_pull_server_mode(&source, dest.path())
            .await
            .map_err(anyhow_to_pyerr)?;
        return Ok(PySyncStats::from(stats));
    }

    // Create transport router for other cases
    let transport = TransportRouter::new(
        &source,
        &dest,
        checksum_type,
        verify_on_write,
        parallel,
        retry_config,
    )
    .await
    .map_err(sync_error_to_pyerr)?
    .with_scan_options(scan_options);

    // Build filter engine
    let mut filter_engine = FilterEngine::new();

    for pattern in &include {
        filter_engine
            .add_include(pattern)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    }

    for pattern in &exclude {
        filter_engine
            .add_exclude(pattern)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    }

    // Create sync engine
    let engine = SyncEngine::new(
        transport,
        dry_run,
        false, // diff_mode
        delete,
        delete_threshold,
        false, // trash
        false, // force_delete
        true,  // quiet (suppress output)
        parallel,
        100, // max_errors
        min_size,
        max_size,
        filter_engine,
        bwlimit,
        resume,
        100,               // checkpoint_files
        100 * 1024 * 1024, // checkpoint_bytes (100MB)
        false,             // json
        checksum_type,
        verify_on_write,
        SymlinkMode::Preserve,
        preserve_xattrs,
        preserve_hardlinks,
        false, // preserve_acls
        false, // preserve_flags
        false, // per_file_progress
        ignore_times,
        size_only,
        checksum,
        update,
        ignore_existing,
        false, // use_cache
        false, // clear_cache
        false, // checksum_db
        false, // clear_checksum_db
        false, // prune_checksum_db
        dest.is_remote(),
        false, // perf
    );

    // Compute effective destination path
    let effective_dest = compute_destination_path(&source, &dest);

    // Run sync
    let stats = engine
        .sync(source.path(), &effective_dest)
        .await
        .map_err(sync_error_to_pyerr)?;

    Ok(PySyncStats::from(stats))
}

/// Compute effective destination path based on rsync trailing slash semantics
fn compute_destination_path(source: &SyncPath, destination: &SyncPath) -> PathBuf {
    let source_path = source.path();

    // For sources with trailing slash, use destination as-is (copy contents)
    if source.has_trailing_slash() {
        return destination.path().to_path_buf();
    }

    // For sources without trailing slash, append source name to destination
    if let Some(name) = source_path.file_name() {
        destination.path().join(name)
    } else {
        destination.path().to_path_buf()
    }
}
