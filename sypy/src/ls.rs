//! Python bindings for directory listing functionality

use pyo3::prelude::*;
use pyo3::types::PyDict;
use sy::integrity::{ChecksumType, IntegrityVerifier};
use sy::ls::{list_directory, ListEntry, ListOptions};
use sy::path::SyncPath;
use sy::retry::RetryConfig;
use sy::transport::local::LocalTransport;

#[cfg(feature = "ssh")]
use sy::ssh::config::{parse_ssh_config, SshConfig};
#[cfg(feature = "ssh")]
use sy::transport::ssh::SshTransport;

#[cfg(feature = "s3")]
use sy::transport::s3::S3Transport;

#[cfg(feature = "gcs")]
use sy::transport::gcs::GcsTransport;

/// Python class representing a directory listing entry
#[pyclass(name = "ListEntry")]
#[derive(Clone)]
pub struct PyListEntry {
    #[pyo3(get)]
    pub path: String,
    #[pyo3(get)]
    pub size: u64,
    #[pyo3(get)]
    pub mod_time: String,
    #[pyo3(get)]
    pub is_dir: bool,
    #[pyo3(get)]
    pub entry_type: String,
    #[pyo3(get)]
    pub mime_type: Option<String>,
    #[pyo3(get)]
    pub symlink_target: Option<String>,
    #[pyo3(get)]
    pub is_sparse: Option<bool>,
    #[pyo3(get)]
    pub allocated_size: Option<u64>,
    #[pyo3(get)]
    pub inode: Option<u64>,
    #[pyo3(get)]
    pub num_links: Option<u64>,
}

impl From<ListEntry> for PyListEntry {
    fn from(entry: ListEntry) -> Self {
        Self {
            path: entry.path,
            size: entry.size,
            mod_time: entry.mod_time,
            is_dir: entry.is_dir,
            entry_type: entry.entry_type,
            mime_type: entry.mime_type,
            symlink_target: entry.symlink_target,
            is_sparse: entry.is_sparse,
            allocated_size: entry.allocated_size,
            inode: entry.inode,
            num_links: entry.num_links,
        }
    }
}

#[pymethods]
impl PyListEntry {
    /// Convert to a Python dictionary (TypedDict-compatible)
    fn to_dict(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new_bound(py);
        dict.set_item("path", &self.path)?;
        dict.set_item("size", self.size)?;
        dict.set_item("mod_time", &self.mod_time)?;
        dict.set_item("is_dir", self.is_dir)?;
        dict.set_item("entry_type", &self.entry_type)?;

        if let Some(ref mime) = self.mime_type {
            dict.set_item("mime_type", mime)?;
        }
        if let Some(ref target) = self.symlink_target {
            dict.set_item("symlink_target", target)?;
        }
        if let Some(sparse) = self.is_sparse {
            dict.set_item("is_sparse", sparse)?;
        }
        if let Some(alloc) = self.allocated_size {
            dict.set_item("allocated_size", alloc)?;
        }
        if let Some(inode) = self.inode {
            dict.set_item("inode", inode)?;
        }
        if let Some(nlinks) = self.num_links {
            dict.set_item("num_links", nlinks)?;
        }

        Ok(dict.into())
    }

    fn __repr__(&self) -> String {
        format!(
            "ListEntry(path='{}', size={}, type='{}')",
            self.path, self.size, self.entry_type
        )
    }
}

/// List directory contents
///
/// Args:
///     path: Path to list (local, SSH, S3, GCS)
///           Examples: "/path", "user@host:/path", "s3://bucket/path", "gs://bucket/path"
///     recursive: Whether to recurse into subdirectories (default: False)
///     max_depth: Maximum recursion depth (default: unlimited)
///     files_only: Only list files, exclude directories (default: False)
///     dirs_only: Only list directories, exclude files (default: False)
///
/// Returns:
///     List of ListEntry objects
///
/// Example:
///     >>> entries = sypy.ls("/path/to/dir")
///     >>> for entry in entries:
///     ...     print(f"{entry.path}: {entry.size} bytes")
///
///     >>> entries = sypy.ls("s3://bucket/path", recursive=True)
///     >>> dicts = [e.to_dict() for e in entries]
#[pyfunction]
#[pyo3(signature = (path, recursive=false, max_depth=None, files_only=false, dirs_only=false))]
pub fn ls(
    path: String,
    recursive: bool,
    max_depth: Option<usize>,
    files_only: bool,
    dirs_only: bool,
) -> PyResult<Vec<PyListEntry>> {
    // Validate options
    if files_only && dirs_only {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "files_only and dirs_only are mutually exclusive",
        ));
    }

    // Parse path
    let sync_path = SyncPath::parse(&path);

    // Create list options
    let mut list_opts = if recursive {
        ListOptions::recursive()
    } else {
        ListOptions::flat()
    };

    if let Some(depth) = max_depth {
        list_opts = list_opts.with_max_depth(depth);
    }

    list_opts.include_files = !dirs_only;
    list_opts.include_dirs = !files_only;

    // Run async listing
    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to create runtime: {}", e))
    })?;

    let entries = runtime
        .block_on(async { ls_async(sync_path, list_opts).await })
        .map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to list directory: {}", e))
        })?;

    Ok(entries.into_iter().map(PyListEntry::from).collect())
}

/// Async implementation of ls (internal)
async fn ls_async(
    sync_path: SyncPath,
    list_opts: ListOptions,
) -> Result<Vec<ListEntry>, Box<dyn std::error::Error + Send + Sync>> {
    let path_buf = sync_path.path().to_path_buf();

    let entries = match sync_path {
        SyncPath::Local { .. } => {
            let verifier = IntegrityVerifier::new(ChecksumType::None, false);
            let transport = LocalTransport::with_verifier(verifier);
            list_directory(&transport, &path_buf, &list_opts).await?
        }
        #[cfg(feature = "ssh")]
        SyncPath::Remote { host, user, .. } => {
            let config = if let Some(user) = user {
                SshConfig {
                    hostname: host.clone(),
                    user: user.clone(),
                    ..Default::default()
                }
            } else {
                parse_ssh_config(&host)?
            };

            let retry_config = RetryConfig::default();
            let transport = SshTransport::with_retry_config(&config, 1, retry_config).await?;
            list_directory(&transport, &path_buf, &list_opts).await?
        }
        #[cfg(not(feature = "ssh"))]
        SyncPath::Remote { .. } => {
            return Err("SSH support not enabled".into());
        }
        #[cfg(feature = "s3")]
        SyncPath::S3 {
            bucket,
            key,
            region,
            endpoint,
            ..
        } => {
            use sy::transport::{CloudClientOptions, S3Config};

            // Use optimized settings for listing
            let client_options = CloudClientOptions {
                pool_max_idle_per_host: 50,
                pool_idle_timeout_secs: 60,
                connect_timeout_secs: 5,
                request_timeout_secs: 120,
                max_retries: 1,
                retry_timeout_secs: 30,
                allow_http: false,
            };

            let config = S3Config {
                access_key_id: None,
                secret_access_key: None,
                region: region.clone(),
                endpoint: endpoint.clone(),
                profile: None,
                client_options: Some(client_options),
            };

            let transport = S3Transport::with_config(
                bucket.clone(),
                key.clone(),
                region.clone(),
                endpoint.clone(),
                Some(config),
                50,
            )
            .await?;
            list_directory(&transport, &path_buf, &list_opts).await?
        }
        #[cfg(not(feature = "s3"))]
        SyncPath::S3 { .. } => {
            return Err("S3 support not enabled".into());
        }
        #[cfg(feature = "gcs")]
        SyncPath::Gcs {
            bucket,
            key,
            project_id,
            service_account_path,
            ..
        } => {
            use sy::transport::{CloudClientOptions, GcsConfig};

            // Use optimized settings for listing
            let client_options = CloudClientOptions {
                pool_max_idle_per_host: 50,
                pool_idle_timeout_secs: 60,
                connect_timeout_secs: 5,
                request_timeout_secs: 120,
                max_retries: 1,
                retry_timeout_secs: 30,
                allow_http: false,
            };

            let config = GcsConfig {
                credentials_file: service_account_path.clone(),
                project_id: project_id.clone(),
                client_options: Some(client_options),
            };

            let transport = GcsTransport::with_config(
                bucket.clone(),
                key.clone(),
                project_id.clone(),
                service_account_path.clone(),
                Some(config),
                50,
            )
            .await?;
            list_directory(&transport, &path_buf, &list_opts).await?
        }
        #[cfg(not(feature = "gcs"))]
        SyncPath::Gcs { .. } => {
            return Err("GCS support not enabled".into());
        }
        SyncPath::Daemon { .. } => {
            return Err("Daemon paths are not supported for listing".into());
        }
    };

    Ok(entries)
}
