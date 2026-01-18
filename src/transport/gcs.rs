use super::cloud::CloudClientOptions;
use super::{FileInfo, TransferResult, Transport};
use crate::error::{Result, SyncError};
use crate::sync::scanner::{FileEntry, ScanOptions};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// GCS configuration for explicit credential and client management
///
/// This struct allows configuring GCS credentials and HTTP client behavior
/// explicitly from code, rather than relying solely on environment variables.
#[derive(Debug, Clone, Default)]
pub struct GcsConfig {
    // === Credentials ===
    /// Path to service account JSON key file
    pub credentials_file: Option<String>,
    /// GCP project ID (usually derived from credentials)
    pub project_id: Option<String>,

    // === Client Options ===
    /// HTTP client and retry configuration
    pub client_options: Option<CloudClientOptions>,
}

/// GCS transport for Google Cloud Storage operations
///
/// Supports Google Cloud Storage with service account authentication.
/// Authentication is handled via:
/// - GOOGLE_APPLICATION_CREDENTIALS environment variable (path to service account JSON)
/// - Application Default Credentials (gcloud auth)
/// - GCE metadata service (when running on Google Cloud)
pub struct GcsTransport {
    store: Arc<dyn ObjectStore>,
    prefix: String, // Key prefix for all operations
}

impl GcsTransport {
    /// Create a new GCS transport
    ///
    /// # Arguments
    /// * `bucket` - GCS bucket name
    /// * `prefix` - Key prefix (e.g., "backups/")
    /// * `project_id` - Optional GCP project ID (unused, derived from credentials)
    /// * `service_account_path` - Optional path to service account JSON key
    ///
    /// # Authentication
    /// The transport uses `from_env()` to automatically pick up credentials from:
    /// - GOOGLE_APPLICATION_CREDENTIALS environment variable
    /// - Application Default Credentials
    /// - GCE metadata service
    pub async fn new(
        bucket: String,
        prefix: String,
        project_id: Option<String>,
        service_account_path: Option<String>,
    ) -> Result<Self> {
        Self::with_config(bucket, prefix, project_id, service_account_path, None, 10).await
    }

    /// Create a new GCS transport with explicit configuration
    ///
    /// # Arguments
    /// * `bucket` - GCS bucket name
    /// * `prefix` - Key prefix (e.g., "backups/")
    /// * `project_id` - Optional GCP project ID
    /// * `service_account_path` - Optional path to service account JSON key
    /// * `config` - Optional GcsConfig with explicit credentials and client options
    /// * `max_connections` - Maximum number of concurrent HTTP connections
    ///
    /// # Client Options
    /// If `config.client_options` is provided, those settings are used.
    /// Otherwise, sensible defaults optimized for sync operations are applied.
    pub async fn with_config(
        bucket: String,
        prefix: String,
        project_id: Option<String>,
        service_account_path: Option<String>,
        config: Option<GcsConfig>,
        max_connections: usize,
    ) -> Result<Self> {
        // Get client options from config or use defaults
        let cloud_options = config
            .as_ref()
            .and_then(|c| c.client_options.clone())
            .unwrap_or_default();

        // Build object_store client options
        let client_options = cloud_options.to_client_options(max_connections);
        let retry_config = cloud_options.to_retry_config();

        // Build GCS store with object_store
        let mut builder = GoogleCloudStorageBuilder::from_env()
            .with_bucket_name(&bucket)
            .with_client_options(client_options)
            .with_retry(retry_config);

        // Apply explicit config if provided
        if let Some(cfg) = &config {
            if let Some(sa_path) = &cfg.credentials_file {
                builder = builder.with_service_account_path(sa_path);
            }
            if let Some(pid) = &cfg.project_id {
                // Note: project_id is usually derived from credentials
                // but can be set explicitly for cross-project access
                let _ = pid; // Currently not directly settable in GoogleCloudStorageBuilder
            }
        }

        // Apply service account path from argument (backward compatibility)
        if let Some(sa_path) = service_account_path {
            builder = builder.with_service_account_path(&sa_path);
        }

        // Apply project ID from argument (backward compatibility)
        if let Some(_pid) = project_id {
            // Currently not directly settable in GoogleCloudStorageBuilder
        }

        let pool_size = max_connections.max(cloud_options.pool_max_idle_per_host);
        let store = Arc::new(builder.build().map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to create GCS client: {}",
                e
            )))
        })?);

        tracing::info!(
            "GCS transport initialized: pool_size={}, timeout={}s, retries={}",
            pool_size,
            cloud_options.request_timeout_secs,
            cloud_options.max_retries
        );

        Ok(Self { store, prefix })
    }

    /// Convert a local path to an object store path
    ///
    /// If the path already starts with the prefix, use it as-is.
    /// Otherwise, prepend the prefix.
    fn path_to_object_path(&self, path: &Path) -> ObjectPath {
        let path_str = path.to_string_lossy();
        let path_str = path_str.trim_start_matches('/');

        let key = if self.prefix.is_empty() {
            path_str.to_string()
        } else {
            // Don't double-prefix: if path already starts with prefix, use it as-is
            let prefix_trimmed = self.prefix.trim_end_matches('/');
            if path_str.starts_with(prefix_trimmed) {
                path_str.to_string()
            } else {
                format!("{}/{}", prefix_trimmed, path_str)
            }
        };

        ObjectPath::from(key)
    }

    /// Convert an object store path to a local path
    fn object_path_to_path(&self, object_path: &ObjectPath) -> PathBuf {
        let key = object_path.as_ref();
        let key = if !self.prefix.is_empty() {
            key.strip_prefix(&self.prefix)
                .unwrap_or(key)
                .trim_start_matches('/')
        } else {
            key
        };
        PathBuf::from(key)
    }
}

#[async_trait]
impl Transport for GcsTransport {
    fn set_scan_options(&mut self, _options: ScanOptions) {
        // GCS transport currently ignores scan options
    }

    async fn scan(&self, _path: &Path) -> Result<Vec<FileEntry>> {
        use futures::stream::StreamExt;

        let prefix = if self.prefix.is_empty() {
            None
        } else {
            Some(ObjectPath::from(self.prefix.clone()))
        };

        let mut entries = Vec::new();
        let mut list_stream = self.store.list(prefix.as_ref());

        while let Some(meta) = list_stream.next().await {
            let meta = meta.map_err(|e| {
                SyncError::Io(std::io::Error::other(format!(
                    "Failed to retrieve object metadata: {}",
                    e
                )))
            })?;

            let key = meta.location.as_ref();
            let size = meta.size;
            let modified = meta.last_modified.into();

            // Check if this is a directory marker (ends with /)
            let is_dir = key.ends_with('/');

            entries.push(FileEntry {
                path: Arc::new(PathBuf::from(key)),
                relative_path: Arc::new(self.object_path_to_path(&meta.location)),
                size,
                modified,
                is_dir,
                is_symlink: false, // GCS doesn't have symlinks
                symlink_target: None,
                is_sparse: false,
                allocated_size: size,
                xattrs: None,
                inode: None,
                nlink: 1,
                acls: None,
                bsd_flags: None,
            });
        }

        Ok(entries)
    }

    async fn scan_streaming(&self, _path: &Path) -> Result<BoxStream<'static, Result<FileEntry>>> {
        use futures::stream::StreamExt;

        let prefix = if self.prefix.is_empty() {
            None
        } else {
            Some(ObjectPath::from(self.prefix.clone()))
        };

        // Clone for closure
        let prefix_str = self.prefix.clone();

        let stream = self.store.list(prefix.as_ref());

        let mapped = stream.map(move |meta_res| {
            let meta = meta_res.map_err(|e| {
                SyncError::Io(std::io::Error::other(format!(
                    "Failed to retrieve object metadata: {}",
                    e
                )))
            })?;

            let key = meta.location.as_ref();
            let size = meta.size;
            let modified = meta.last_modified.into();

            // Check if this is a directory marker (ends with /)
            let is_dir = key.ends_with('/');

            // Replicate object_path_to_path logic locally to avoid self capture
            let relative_key = if !prefix_str.is_empty() {
                key.strip_prefix(&prefix_str)
                    .unwrap_or(key)
                    .trim_start_matches('/')
            } else {
                key
            };
            let relative_path = PathBuf::from(relative_key);

            Ok(FileEntry {
                path: Arc::new(PathBuf::from(key)),
                relative_path: Arc::new(relative_path),
                size,
                modified,
                is_dir,
                is_symlink: false, // GCS doesn't have symlinks
                symlink_target: None,
                is_sparse: false,
                allocated_size: size,
                xattrs: None,
                inode: None,
                nlink: 1,
                acls: None,
                bsd_flags: None,
            })
        });

        Ok(mapped.boxed())
    }

    async fn exists(&self, path: &Path) -> Result<bool> {
        let object_path = self.path_to_object_path(path);
        let result = self.store.head(&object_path).await;
        Ok(result.is_ok())
    }

    async fn metadata(&self, _path: &Path) -> Result<std::fs::Metadata> {
        // GCS doesn't have std::fs::Metadata, this method shouldn't be used
        Err(SyncError::Io(std::io::Error::other(
            "metadata() not supported for GCS, use file_info() instead",
        )))
    }

    async fn file_info(&self, path: &Path) -> Result<FileInfo> {
        let object_path = self.path_to_object_path(path);

        let meta = self.store.head(&object_path).await.map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to get GCS object metadata: {}",
                e
            )))
        })?;

        Ok(FileInfo {
            size: meta.size,
            modified: meta.last_modified.into(),
        })
    }

    async fn create_dir_all(&self, path: &Path) -> Result<()> {
        // GCS doesn't have directories in the traditional sense
        // We can create a directory marker object (key ending with /)
        let mut key_str = self.path_to_object_path(path).to_string();
        if !key_str.ends_with('/') {
            key_str.push('/');
        }
        let object_path = ObjectPath::from(key_str);

        self.store
            .put(&object_path, Bytes::new().into())
            .await
            .map_err(|e| {
                SyncError::Io(std::io::Error::other(format!(
                    "Failed to create GCS directory marker: {}",
                    e
                )))
            })?;

        Ok(())
    }

    async fn copy_file(&self, source: &Path, dest: &Path) -> Result<TransferResult> {
        use tokio::io::AsyncReadExt;

        let metadata = tokio::fs::metadata(source).await?;
        let size = metadata.len();
        let object_path = self.path_to_object_path(dest);

        // Use streaming multipart upload for large files to avoid loading into memory
        // For small files (<5MB), use simple put for efficiency
        const MULTIPART_THRESHOLD: u64 = 5 * 1024 * 1024; // 5MB

        if size < MULTIPART_THRESHOLD {
            // Small file: use simple put (one API call)
            let data = tokio::fs::read(source).await?;
            self.store
                .put(&object_path, Bytes::from(data).into())
                .await
                .map_err(|e| {
                    SyncError::Io(std::io::Error::other(format!(
                        "Failed to upload to GCS: {}",
                        e
                    )))
                })?;
        } else {
            // Large file: use multipart upload (streaming, no memory buffering)
            use object_store::WriteMultipart;

            let mut file = tokio::fs::File::open(source).await?;
            let upload = self.store.put_multipart(&object_path).await.map_err(|e| {
                SyncError::Io(std::io::Error::other(format!(
                    "Failed to initiate multipart upload: {}",
                    e
                )))
            })?;

            // WriteMultipart handles chunking automatically (5MB chunks)
            let mut writer = WriteMultipart::new(upload);

            // Stream file in chunks
            const BUFFER_SIZE: usize = 5 * 1024 * 1024;
            let mut buffer = vec![0u8; BUFFER_SIZE];

            loop {
                let bytes_read = file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }

                // write() is synchronous by design - it buffers data and starts uploads automatically
                // Errors are reported via finish()
                writer.write(&buffer[..bytes_read]);
            }

            // finish() waits for all uploads to complete and reports any errors
            writer.finish().await.map_err(|e| {
                SyncError::Io(std::io::Error::other(format!(
                    "Failed to complete multipart upload: {}",
                    e
                )))
            })?;
        }

        Ok(TransferResult::new(size))
    }

    async fn remove(&self, path: &Path, _is_dir: bool) -> Result<()> {
        let object_path = self.path_to_object_path(path);

        self.store.delete(&object_path).await.map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to delete GCS object: {}",
                e
            )))
        })?;

        Ok(())
    }

    async fn create_hardlink(&self, _source: &Path, _dest: &Path) -> Result<()> {
        Err(SyncError::Io(std::io::Error::other(
            "Hardlinks not supported on GCS",
        )))
    }

    async fn create_symlink(&self, _target: &Path, _dest: &Path) -> Result<()> {
        Err(SyncError::Io(std::io::Error::other(
            "Symlinks not supported on GCS",
        )))
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let object_path = self.path_to_object_path(path);

        let result = self.store.get(&object_path).await.map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to download from GCS: {}",
                e
            )))
        })?;

        let bytes = result.bytes().await.map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to read GCS object body: {}",
                e
            )))
        })?;

        Ok(bytes.to_vec())
    }

    async fn write_file(&self, path: &Path, data: &[u8], _mtime: SystemTime) -> Result<()> {
        let object_path = self.path_to_object_path(path);

        self.store
            .put(&object_path, Bytes::copy_from_slice(data).into())
            .await
            .map_err(|e| {
                SyncError::Io(std::io::Error::other(format!(
                    "Failed to upload to GCS: {}",
                    e
                )))
            })?;

        Ok(())
    }

    async fn get_mtime(&self, path: &Path) -> Result<SystemTime> {
        let info = self.file_info(path).await?;
        Ok(info.modified)
    }

    async fn check_disk_space(&self, _path: &Path, _bytes_needed: u64) -> Result<()> {
        // GCS has virtually unlimited storage, so no disk space check needed
        Ok(())
    }

    async fn set_bsd_flags(&self, _path: &Path, _flags: u32) -> Result<()> {
        // BSD flags are not supported on GCS - silently ignore
        Ok(())
    }

    async fn set_xattrs(&self, _path: &Path, _xattrs: &[(String, Vec<u8>)]) -> Result<()> {
        // Extended attributes are not supported on GCS - silently ignore
        Ok(())
    }

    async fn set_acls(&self, _path: &Path, _acls_text: &str) -> Result<()> {
        // ACLs are not supported on GCS - silently ignore
        Ok(())
    }
}
