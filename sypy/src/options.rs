//! Python wrapper for sync options

use crate::config::{PyGcsConfig, PyS3Config, PySshConfig};
use pyo3::prelude::*;

/// Sync options configuration
///
/// Use this class to configure sync behavior when calling sync_with_options().
/// All options have sensible defaults.
#[pyclass(name = "SyncOptions")]
#[derive(Clone, Debug)]
pub struct PySyncOptions {
    /// Dry run mode - show changes without applying
    #[pyo3(get, set)]
    pub dry_run: bool,

    /// Delete files in destination not present in source
    #[pyo3(get, set)]
    pub delete: bool,

    /// Maximum percentage of files that can be deleted (0-100)
    #[pyo3(get, set)]
    pub delete_threshold: u8,

    /// Move deleted files to trash instead of permanent deletion
    #[pyo3(get, set)]
    pub trash: bool,

    /// Skip deletion safety checks
    #[pyo3(get, set)]
    pub force_delete: bool,

    /// Number of parallel file transfers
    #[pyo3(get, set)]
    pub parallel: usize,

    /// Maximum number of errors before aborting (0 = unlimited)
    #[pyo3(get, set)]
    pub max_errors: usize,

    /// Minimum file size to sync (e.g., "1MB", "500KB")
    #[pyo3(get, set)]
    pub min_size: Option<String>,

    /// Maximum file size to sync (e.g., "100MB", "1GB")
    #[pyo3(get, set)]
    pub max_size: Option<String>,

    /// Exclude patterns
    #[pyo3(get, set)]
    pub exclude: Vec<String>,

    /// Include patterns
    #[pyo3(get, set)]
    pub include: Vec<String>,

    /// Bandwidth limit (e.g., "1MB", "500KB")
    #[pyo3(get, set)]
    pub bwlimit: Option<String>,

    /// Enable resume support for interrupted transfers
    #[pyo3(get, set)]
    pub resume: bool,

    /// Verify file integrity after write using xxHash3
    #[pyo3(get, set)]
    pub verify: bool,

    /// Enable compression for network transfers
    #[pyo3(get, set)]
    pub compress: bool,

    /// Preserve extended attributes (xattrs)
    #[pyo3(get, set)]
    pub preserve_xattrs: bool,

    /// Preserve hard links
    #[pyo3(get, set)]
    pub preserve_hardlinks: bool,

    /// Preserve access control lists (ACLs)
    #[pyo3(get, set)]
    pub preserve_acls: bool,

    /// Preserve permissions
    #[pyo3(get, set)]
    pub preserve_permissions: bool,

    /// Preserve modification times
    #[pyo3(get, set)]
    pub preserve_times: bool,

    /// Ignore modification times, always compare checksums
    #[pyo3(get, set)]
    pub ignore_times: bool,

    /// Only compare file size, skip mtime checks
    #[pyo3(get, set)]
    pub size_only: bool,

    /// Always compare checksums instead of size+mtime
    #[pyo3(get, set)]
    pub checksum: bool,

    /// Skip files where destination is newer than source
    #[pyo3(get, set)]
    pub update: bool,

    /// Skip files that already exist in destination
    #[pyo3(get, set)]
    pub ignore_existing: bool,

    /// Apply .gitignore rules
    #[pyo3(get, set)]
    pub gitignore: bool,

    /// Exclude .git directories
    #[pyo3(get, set)]
    pub exclude_vcs: bool,

    /// Bidirectional sync mode
    #[pyo3(get, set)]
    pub bidirectional: bool,

    /// Conflict resolution strategy for bidirectional sync
    /// Options: "newer", "larger", "smaller", "source", "dest", "rename"
    #[pyo3(get, set)]
    pub conflict_resolve: String,

    /// Use daemon mode for fast repeated syncs
    #[pyo3(get, set)]
    pub daemon_auto: bool,

    /// Maximum retry attempts for network operations
    #[pyo3(get, set)]
    pub retry: u32,

    /// Initial delay between retries in seconds
    #[pyo3(get, set)]
    pub retry_delay: u64,

    /// S3 configuration for S3/S3-compatible storage
    #[pyo3(get, set)]
    pub s3: Option<PyS3Config>,

    /// GCS configuration for Google Cloud Storage
    #[pyo3(get, set)]
    pub gcs: Option<PyGcsConfig>,

    /// SSH configuration for remote connections
    #[pyo3(get, set)]
    pub ssh: Option<PySshConfig>,
}

#[pymethods]
impl PySyncOptions {
    /// Create new sync options with default values
    #[new]
    #[pyo3(signature = (
        dry_run = false,
        delete = false,
        delete_threshold = 50,
        trash = false,
        force_delete = false,
        parallel = 10,
        max_errors = 100,
        min_size = None,
        max_size = None,
        exclude = Vec::new(),
        include = Vec::new(),
        bwlimit = None,
        resume = true,
        verify = false,
        compress = false,
        preserve_xattrs = false,
        preserve_hardlinks = false,
        preserve_acls = false,
        preserve_permissions = false,
        preserve_times = false,
        ignore_times = false,
        size_only = false,
        checksum = false,
        update = false,
        ignore_existing = false,
        gitignore = false,
        exclude_vcs = false,
        bidirectional = false,
        conflict_resolve = "newer".to_string(),
        daemon_auto = false,
        retry = 3,
        retry_delay = 1,
        s3 = None,
        gcs = None,
        ssh = None
    ))]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dry_run: bool,
        delete: bool,
        delete_threshold: u8,
        trash: bool,
        force_delete: bool,
        parallel: usize,
        max_errors: usize,
        min_size: Option<String>,
        max_size: Option<String>,
        exclude: Vec<String>,
        include: Vec<String>,
        bwlimit: Option<String>,
        resume: bool,
        verify: bool,
        compress: bool,
        preserve_xattrs: bool,
        preserve_hardlinks: bool,
        preserve_acls: bool,
        preserve_permissions: bool,
        preserve_times: bool,
        ignore_times: bool,
        size_only: bool,
        checksum: bool,
        update: bool,
        ignore_existing: bool,
        gitignore: bool,
        exclude_vcs: bool,
        bidirectional: bool,
        conflict_resolve: String,
        daemon_auto: bool,
        retry: u32,
        retry_delay: u64,
        s3: Option<PyS3Config>,
        gcs: Option<PyGcsConfig>,
        ssh: Option<PySshConfig>,
    ) -> Self {
        PySyncOptions {
            dry_run,
            delete,
            delete_threshold,
            trash,
            force_delete,
            parallel,
            max_errors,
            min_size,
            max_size,
            exclude,
            include,
            bwlimit,
            resume,
            verify,
            compress,
            preserve_xattrs,
            preserve_hardlinks,
            preserve_acls,
            preserve_permissions,
            preserve_times,
            ignore_times,
            size_only,
            checksum,
            update,
            ignore_existing,
            gitignore,
            exclude_vcs,
            bidirectional,
            conflict_resolve,
            daemon_auto,
            retry,
            retry_delay,
            s3,
            gcs,
            ssh,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SyncOptions(dry_run={}, delete={}, parallel={}, verify={})",
            self.dry_run, self.delete, self.parallel, self.verify
        )
    }
}

impl Default for PySyncOptions {
    fn default() -> Self {
        PySyncOptions {
            dry_run: false,
            delete: false,
            delete_threshold: 50,
            trash: false,
            force_delete: false,
            parallel: 10,
            max_errors: 100,
            min_size: None,
            max_size: None,
            exclude: Vec::new(),
            include: Vec::new(),
            bwlimit: None,
            resume: true,
            verify: false,
            compress: false,
            preserve_xattrs: false,
            preserve_hardlinks: false,
            preserve_acls: false,
            preserve_permissions: false,
            preserve_times: false,
            ignore_times: false,
            size_only: false,
            checksum: false,
            update: false,
            ignore_existing: false,
            gitignore: false,
            exclude_vcs: false,
            bidirectional: false,
            conflict_resolve: "newer".to_string(),
            daemon_auto: false,
            retry: 3,
            retry_delay: 1,
            s3: None,
            gcs: None,
            ssh: None,
        }
    }
}

/// Parse a size string (e.g., "1MB", "500KB") into bytes
pub fn parse_size_option(s: &str) -> Result<u64, String> {
    sy::cli::parse_size(s)
}
