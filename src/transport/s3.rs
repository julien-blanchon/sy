use super::cloud::CloudClientOptions;
use super::{FileInfo, TransferResult, Transport};
use crate::error::{Result, SyncError};
use crate::sync::scanner::{FileEntry, ScanOptions};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// S3 configuration for explicit credential and client management
///
/// This struct allows configuring S3 credentials and HTTP client behavior
/// explicitly from code, rather than relying solely on environment variables.
#[derive(Debug, Clone, Default)]
pub struct S3Config {
    // === Credentials ===
    /// AWS access key ID
    pub access_key_id: Option<String>,
    /// AWS secret access key
    pub secret_access_key: Option<String>,
    /// AWS region (e.g., "us-east-1")
    pub region: Option<String>,
    /// Custom S3-compatible endpoint URL (e.g., for DigitalOcean Spaces, MinIO)
    pub endpoint: Option<String>,
    /// AWS profile name to use from ~/.aws/credentials
    pub profile: Option<String>,

    // === Client Options ===
    /// HTTP client and retry configuration
    pub client_options: Option<CloudClientOptions>,
}

/// S3 transport for cloud storage operations
///
/// Supports AWS S3 and S3-compatible services (Cloudflare R2, Backblaze B2, Wasabi)
pub struct S3Transport {
    store: Arc<dyn ObjectStore>,
    prefix: String, // Key prefix for all operations
}

impl S3Transport {
    /// Create a new S3 transport
    ///
    /// # Arguments
    /// * `bucket` - S3 bucket name
    /// * `prefix` - Key prefix (e.g., "backups/")
    /// * `region` - Optional AWS region (defaults to config/env)
    /// * `endpoint` - Optional custom endpoint (for R2, B2, DigitalOcean Spaces, etc.)
    ///
    /// # Environment Variables
    /// Credentials are read from:
    /// - `AWS_ACCESS_KEY_ID` - Access key
    /// - `AWS_SECRET_ACCESS_KEY` - Secret key
    /// - `AWS_REGION` or `AWS_DEFAULT_REGION` - Region (if not specified)
    /// - `AWS_ENDPOINT_URL` - Custom endpoint (if not specified)
    pub async fn new(
        bucket: String,
        prefix: String,
        region: Option<String>,
        endpoint: Option<String>,
    ) -> Result<Self> {
        Self::with_config(bucket, prefix, region, endpoint, None, 10).await
    }

    /// Create a new S3 transport with explicit configuration
    ///
    /// # Arguments
    /// * `bucket` - S3 bucket name
    /// * `prefix` - Key prefix (e.g., "backups/")
    /// * `region` - Optional AWS region (defaults to config/env)
    /// * `endpoint` - Optional custom endpoint
    /// * `config` - Optional S3Config with explicit credentials and client options
    /// * `max_connections` - Maximum number of concurrent HTTP connections (default: 10)
    ///
    /// # Client Options
    /// If `config.client_options` is provided, those settings are used.
    /// Otherwise, sensible defaults optimized for sync operations are applied:
    /// - Connection pool sized to `max_connections`
    /// - 30 second idle timeout
    /// - 5 second connect timeout
    /// - 60 second request timeout
    /// - 3 retries with 15 second retry timeout
    pub async fn with_config(
        bucket: String,
        prefix: String,
        region: Option<String>,
        endpoint: Option<String>,
        config: Option<S3Config>,
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

        // Build S3 store with object_store
        let mut builder = AmazonS3Builder::from_env()
            .with_bucket_name(&bucket)
            .with_client_options(client_options)
            .with_retry(retry_config);

        // Apply explicit config if provided
        if let Some(cfg) = config {
            if let Some(key) = cfg.access_key_id {
                builder = builder.with_access_key_id(&key);
            }
            if let Some(secret) = cfg.secret_access_key {
                builder = builder.with_secret_access_key(&secret);
            }
            if let Some(r) = cfg.region {
                builder = builder.with_region(&r);
            }
            if let Some(ep) = cfg.endpoint {
                builder = builder.with_endpoint(&ep);
                builder = builder.with_virtual_hosted_style_request(false);
            }
        }

        // Apply region from argument or environment (if not set by config)
        if let Some(r) = region {
            builder = builder.with_region(&r);
        } else if let Ok(r) = std::env::var("AWS_REGION") {
            builder = builder.with_region(&r);
        } else if let Ok(r) = std::env::var("AWS_DEFAULT_REGION") {
            builder = builder.with_region(&r);
        }

        // Apply endpoint from argument or environment (if not set by config)
        if let Some(ep) = endpoint {
            builder = builder.with_endpoint(&ep);
            builder = builder.with_virtual_hosted_style_request(false);
        } else if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            builder = builder.with_endpoint(&ep);
            builder = builder.with_virtual_hosted_style_request(false);
        }

        // Skip credential loading retries on non-AWS environments
        builder = builder.with_skip_signature(false);

        let pool_size = max_connections.max(cloud_options.pool_max_idle_per_host);
        let store = Arc::new(builder.build().map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to create S3 client: {}. \
                 Ensure AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are set.",
                e
            )))
        })?);

        tracing::info!(
            "S3 transport initialized: pool_size={}, timeout={}s, retries={}",
            pool_size,
            cloud_options.request_timeout_secs,
            cloud_options.max_retries
        );

        Ok(Self { store, prefix })
    }

    /// Convert a local path to an object store path
    fn path_to_object_path(&self, path: &Path) -> ObjectPath {
        let path_str = path.to_string_lossy();
        let path_str = path_str.trim_start_matches('/');

        let key = if self.prefix.is_empty() {
            path_str.to_string()
        } else {
            let prefix_normalized = self.prefix.trim_end_matches('/');
            // Check if path already starts with the prefix (avoid double-prefixing)
            if path_str.starts_with(prefix_normalized)
                && (path_str.len() == prefix_normalized.len()
                    || path_str.chars().nth(prefix_normalized.len()) == Some('/'))
            {
                // Path already has prefix, use as-is
                path_str.to_string()
            } else {
                // Add prefix
                format!("{}/{}", prefix_normalized, path_str)
            }
        };

        ObjectPath::from(key)
    }

    /// Convert an object store path to a local path (strips prefix)
    fn object_path_to_path(&self, object_path: &ObjectPath) -> PathBuf {
        let key = object_path.as_ref();
        let key = if !self.prefix.is_empty() {
            // Try stripping with trailing slash first, then without
            let prefix_with_slash = format!("{}/", self.prefix.trim_end_matches('/'));
            let prefix_without_slash = self.prefix.trim_end_matches('/');

            key.strip_prefix(&prefix_with_slash)
                .or_else(|| key.strip_prefix(prefix_without_slash))
                .unwrap_or(key)
                .trim_start_matches('/')
        } else {
            key
        };
        PathBuf::from(key)
    }
}

#[async_trait]
impl Transport for S3Transport {
    fn set_scan_options(&mut self, _options: ScanOptions) {
        // S3 transport currently ignores scan options
    }

    async fn scan(&self, _path: &Path) -> Result<Vec<FileEntry>> {
        use futures::stream::StreamExt;

        let prefix = if self.prefix.is_empty() {
            None
        } else {
            Some(ObjectPath::from(self.prefix.clone()))
        };

        tracing::debug!(
            "Starting S3 listing with prefix: {:?} (full recursive scan)",
            prefix
        );
        let mut entries = Vec::new();
        let mut list_stream = self.store.list(prefix.as_ref());
        let start = std::time::Instant::now();

        while let Some(meta) = list_stream.next().await {
            let meta = meta.map_err(|e| {
                tracing::error!(
                    "S3 list error after {} entries, {:?}: {}",
                    entries.len(),
                    start.elapsed(),
                    e
                );
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
                is_symlink: false, // S3 doesn't have symlinks
                symlink_target: None,
                is_sparse: false,
                allocated_size: size,
                xattrs: None,
                inode: None,
                nlink: 1,
                acls: None,
                bsd_flags: None,
            });

            if entries.len() % 1000 == 0 {
                tracing::info!(
                    "Listed {} objects so far ({:?})",
                    entries.len(),
                    start.elapsed()
                );
            }
        }

        tracing::info!(
            "S3 listing complete: {} entries in {:?}",
            entries.len(),
            start.elapsed()
        );
        Ok(entries)
    }

    async fn scan_flat(&self, _path: &Path) -> Result<Vec<FileEntry>> {
        use object_store::path::Path as ObjectPath;

        let prefix = if self.prefix.is_empty() {
            None
        } else {
            Some(ObjectPath::from(self.prefix.clone()))
        };

        tracing::debug!(
            "Starting S3 flat listing with prefix: {:?} (using delimiter)",
            prefix
        );
        let mut entries = Vec::new();

        // Use list_with_delimiter for efficient non-recursive listing
        // This tells S3 to only return objects at the current level, not traverse subdirectories
        let list_result = self
            .store
            .list_with_delimiter(prefix.as_ref())
            .await
            .map_err(|e| {
                SyncError::Io(std::io::Error::other(format!(
                    "Failed to list S3 objects: {}",
                    e
                )))
            })?;

        let object_count = list_result.objects.len();
        let dir_count = list_result.common_prefixes.len();

        // Process regular objects (files at this level)
        for meta in list_result.objects {
            let key = meta.location.as_ref();
            let size = meta.size;
            let modified = meta.last_modified.into();

            entries.push(FileEntry {
                path: Arc::new(PathBuf::from(key)),
                relative_path: Arc::new(self.object_path_to_path(&meta.location)),
                size,
                modified,
                is_dir: false,
                is_symlink: false,
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

        // Process common prefixes (directories at this level)
        for prefix_path in list_result.common_prefixes {
            let key = prefix_path.as_ref();

            entries.push(FileEntry {
                path: Arc::new(PathBuf::from(key)),
                relative_path: Arc::new(self.object_path_to_path(&prefix_path)),
                size: 0,
                modified: std::time::SystemTime::UNIX_EPOCH,
                is_dir: true,
                is_symlink: false,
                symlink_target: None,
                is_sparse: false,
                allocated_size: 0,
                xattrs: None,
                inode: None,
                nlink: 1,
                acls: None,
                bsd_flags: None,
            });
        }

        tracing::info!(
            "S3 flat listing complete: {} entries ({} files, {} dirs)",
            entries.len(),
            object_count,
            dir_count
        );
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
                is_symlink: false, // S3 doesn't have symlinks
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
        // S3 doesn't have std::fs::Metadata, this method shouldn't be used
        Err(SyncError::Io(std::io::Error::other(
            "metadata() not supported for S3, use file_info() instead",
        )))
    }

    async fn file_info(&self, path: &Path) -> Result<FileInfo> {
        let object_path = self.path_to_object_path(path);

        let meta = self.store.head(&object_path).await.map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to get S3 object metadata: {}",
                e
            )))
        })?;

        Ok(FileInfo {
            size: meta.size,
            modified: meta.last_modified.into(),
        })
    }

    async fn create_dir_all(&self, path: &Path) -> Result<()> {
        // S3 doesn't have directories in the traditional sense
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
                    "Failed to create S3 directory marker: {}",
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
                        "Failed to upload to S3: {}",
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
                "Failed to delete S3 object: {}",
                e
            )))
        })?;

        Ok(())
    }

    async fn create_hardlink(&self, _source: &Path, _dest: &Path) -> Result<()> {
        Err(SyncError::Io(std::io::Error::other(
            "Hardlinks not supported on S3",
        )))
    }

    async fn create_symlink(&self, _target: &Path, _dest: &Path) -> Result<()> {
        Err(SyncError::Io(std::io::Error::other(
            "Symlinks not supported on S3",
        )))
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let object_path = self.path_to_object_path(path);

        let result = self.store.get(&object_path).await.map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to download from S3: {}",
                e
            )))
        })?;

        let bytes = result.bytes().await.map_err(|e| {
            SyncError::Io(std::io::Error::other(format!(
                "Failed to read S3 object body: {}",
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
                    "Failed to upload to S3: {}",
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
        // S3 has virtually unlimited storage, so no disk space check needed
        Ok(())
    }

    async fn set_bsd_flags(&self, _path: &Path, _flags: u32) -> Result<()> {
        // BSD flags are not supported on S3 - silently ignore
        Ok(())
    }

    async fn set_xattrs(&self, _path: &Path, _xattrs: &[(String, Vec<u8>)]) -> Result<()> {
        // Extended attributes are not supported on S3 - silently ignore
        Ok(())
    }

    async fn set_acls(&self, _path: &Path, _acls_text: &str) -> Result<()> {
        // ACLs are not supported on S3 - silently ignore
        Ok(())
    }
}
