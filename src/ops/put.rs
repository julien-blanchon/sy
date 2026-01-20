//! Upload operations - put files to remote storage
//!
//! Supports uploading files from local filesystem to SSH, S3, GCS, and daemon.

use crate::filter::FilterEngine;
use crate::path::SyncPath;
use crate::sync::scanner::Scanner;
use crate::transport::Transport;
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(feature = "ssh")]
use crate::compress::{compress, is_compressed_extension, Compression};
#[cfg(feature = "ssh")]
use crate::server::protocol::{FileListEntry, DATA_FLAG_COMPRESSED};
#[cfg(feature = "ssh")]
use crate::ssh::config::SshConfig;
#[cfg(feature = "ssh")]
use crate::transport::server::ServerSession;

#[cfg(unix)]
use crate::transport::server::DaemonSession;

/// Options for upload operations
#[derive(Debug, Clone, Default)]
pub struct UploadOptions {
    /// Recursive upload (for directories)
    pub recursive: bool,
    /// Maximum depth for recursive upload
    pub max_depth: Option<usize>,
    /// Preview changes without actually uploading
    pub dry_run: bool,
    /// Number of parallel uploads
    pub parallel: usize,
    /// Use SFTP instead of server protocol for SSH
    pub sftp: bool,
    /// Include patterns for filtering
    pub include: Vec<String>,
    /// Exclude patterns for filtering
    pub exclude: Vec<String>,
    /// Daemon socket path (for daemon mode operations)
    pub daemon_socket: Option<String>,
}

impl UploadOptions {
    pub fn new() -> Self {
        Self {
            parallel: 8,
            ..Default::default()
        }
    }

    pub fn recursive(mut self) -> Self {
        self.recursive = true;
        self
    }

    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = Some(depth);
        self
    }

    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    pub fn with_parallel(mut self, parallel: usize) -> Self {
        self.parallel = parallel.max(1);
        self
    }

    pub fn with_include(mut self, patterns: Vec<String>) -> Self {
        self.include = patterns;
        self
    }

    pub fn with_exclude(mut self, patterns: Vec<String>) -> Self {
        self.exclude = patterns;
        self
    }

    pub fn use_sftp(mut self) -> Self {
        self.sftp = true;
        self
    }

    pub fn with_daemon_socket(mut self, socket: String) -> Self {
        self.daemon_socket = Some(socket);
        self
    }

    /// Build a filter engine from include/exclude patterns
    pub fn build_filter(&self) -> Result<FilterEngine> {
        let mut filter = FilterEngine::new();
        for pattern in &self.include {
            filter.add_include(pattern)?;
        }
        for pattern in &self.exclude {
            filter.add_exclude(pattern)?;
        }
        Ok(filter)
    }
}

/// Result of an upload operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResult {
    pub source: String,
    pub destination: String,
    pub uploaded_files: usize,
    pub uploaded_bytes: u64,
    pub created_dirs: usize,
    pub skipped_files: usize,
    pub failed: Vec<FailedUpload>,
    pub dry_run: bool,
}

impl UploadResult {
    pub fn new(source: &str, destination: &str, dry_run: bool) -> Self {
        Self {
            source: source.to_string(),
            destination: destination.to_string(),
            uploaded_files: 0,
            uploaded_bytes: 0,
            created_dirs: 0,
            skipped_files: 0,
            failed: Vec::new(),
            dry_run,
        }
    }

    pub fn success(&self) -> bool {
        self.failed.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedUpload {
    pub path: String,
    pub error: String,
}

/// Upload files to a transport from local source
pub async fn upload_to_transport<T: Transport + Send + Sync + 'static>(
    transport: Arc<T>,
    source: &Path,
    dest: &Path,
    options: &UploadOptions,
    filter: &FilterEngine,
) -> Result<UploadResult> {
    let result = Arc::new(Mutex::new(UploadResult::new(
        &source.display().to_string(),
        &dest.display().to_string(),
        options.dry_run,
    )));

    // Check if source exists
    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    let source_is_file = source.is_file();

    if source_is_file {
        // Single file upload
        let file_size = std::fs::metadata(source)?.len();

        // Determine the actual destination path
        // If dest looks like a directory (ends with / or is empty), append the source filename
        let dest_str = dest.to_string_lossy();
        let actual_dest = if dest_str.ends_with('/') || dest_str.is_empty() {
            // Append source filename to directory destination
            let filename = source.file_name().unwrap_or_default();
            dest.join(filename)
        } else {
            dest.to_path_buf()
        };

        if options.dry_run {
            let mut r = result.lock().await;
            r.uploaded_files = 1;
            r.uploaded_bytes = file_size;
        } else {
            let data = tokio::fs::read(source).await?;
            let mtime = std::fs::metadata(source)?.modified()?;

            if let Some(parent) = actual_dest.parent() {
                if !parent.as_os_str().is_empty() {
                    transport.create_dir_all(parent).await.ok();
                }
            }

            transport
                .write_file(&actual_dest, &data, mtime)
                .await
                .context(format!("Failed to upload {}", source.display()))?;

            let mut r = result.lock().await;
            r.uploaded_files = 1;
            r.uploaded_bytes = file_size;
        }
    } else if options.recursive {
        // Directory upload
        let scanner = Scanner::new(source.to_path_buf());
        let entries = scanner.scan().context("Failed to scan source directory")?;

        // Filter entries and apply max depth
        let filtered_entries: Vec<_> = entries
            .iter()
            .filter(|e| {
                if !filter.should_include(&e.relative_path, e.is_dir) {
                    return false;
                }
                if let Some(max_depth) = options.max_depth {
                    let depth = e.relative_path.components().count();
                    if depth > max_depth {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Collect directories to create
        let mut dirs: Vec<_> = filtered_entries.iter().filter(|e| e.is_dir).collect();
        dirs.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        // Create directories first
        for entry in &dirs {
            let dest_path = dest.join(&*entry.relative_path);
            if options.dry_run {
                let mut r = result.lock().await;
                r.created_dirs += 1;
            } else {
                match transport.create_dir_all(&dest_path).await {
                    Ok(()) => {
                        let mut r = result.lock().await;
                        r.created_dirs += 1;
                    }
                    Err(_) => {
                        // Ignore directory creation errors for cloud storage
                    }
                }
            }
        }

        // Collect files to upload
        let files: Vec<_> = filtered_entries
            .iter()
            .filter(|e| !e.is_dir && !e.is_symlink)
            .map(|e| {
                (
                    source.join(&*e.relative_path),
                    dest.join(&*e.relative_path),
                    e.size,
                )
            })
            .collect();

        if options.dry_run {
            let mut r = result.lock().await;
            r.uploaded_files = files.len();
            r.uploaded_bytes = files.iter().map(|(_, _, s)| s).sum();
        } else {
            let concurrency = options.parallel.max(1);

            let upload_tasks: Vec<_> = files
                .into_iter()
                .map(|(source_path, dest_path, _)| {
                    let transport = Arc::clone(&transport);
                    let result = Arc::clone(&result);

                    async move {
                        match upload_single_file(&source_path, &dest_path, &*transport).await {
                            Ok(bytes) => {
                                let mut r = result.lock().await;
                                r.uploaded_files += 1;
                                r.uploaded_bytes += bytes;
                            }
                            Err(e) => {
                                let mut r = result.lock().await;
                                r.failed.push(FailedUpload {
                                    path: source_path.display().to_string(),
                                    error: e.to_string(),
                                });
                            }
                        }
                    }
                })
                .collect();

            stream::iter(upload_tasks)
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await;
        }
    } else {
        anyhow::bail!("Source is a directory. Use recursive option to upload directories.");
    }

    let final_result = Arc::try_unwrap(result)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap result"))?
        .into_inner();

    Ok(final_result)
}

/// Upload a single file
pub async fn upload_single_file<T: Transport + ?Sized>(
    source: &Path,
    dest: &Path,
    transport: &T,
) -> Result<u64> {
    let data = tokio::fs::read(source).await?;
    let file_size = data.len() as u64;
    let mtime = std::fs::metadata(source)?.modified()?;

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            transport.create_dir_all(parent).await.ok();
        }
    }

    transport
        .write_file(dest, &data, mtime)
        .await
        .context(format!("Failed to upload {}", source.display()))?;

    Ok(file_size)
}

/// Minimum size for compression (1MB)
#[cfg(feature = "ssh")]
const COMPRESS_MIN_SIZE: u64 = 1024 * 1024;

/// Upload using server protocol (spawns sy --server on remote for pipelined transfers)
#[cfg(feature = "ssh")]
pub async fn upload_via_server_protocol(
    source: &Path,
    config: &SshConfig,
    dest: &Path,
    options: &UploadOptions,
    filter: &FilterEngine,
) -> Result<UploadResult> {
    use std::sync::Arc as StdArc;

    let mut result = UploadResult::new(
        &source.display().to_string(),
        &dest.display().to_string(),
        options.dry_run,
    );

    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    let source_is_file = source.is_file();

    if source_is_file {
        // Single file upload via server protocol
        let file_size = std::fs::metadata(source)?.len();
        let mtime = std::fs::metadata(source)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Determine server root and relative path based on destination format:
        // - If dest ends with / (directory): connect to dest, use source filename
        // - If dest doesn't end with / (file): connect to parent, use dest filename
        let dest_str = dest.to_string_lossy();
        let (server_root, rel_path) = if dest_str.ends_with('/') || dest_str.is_empty() {
            // Destination is a directory - file goes inside it with original name
            let filename = source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            (dest.to_path_buf(), filename)
        } else {
            // Destination is a file path - rename file to dest's filename
            let parent = dest.parent().unwrap_or(dest);
            let filename = dest
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            (parent.to_path_buf(), filename)
        };

        if options.dry_run {
            result.uploaded_files = 1;
            result.uploaded_bytes = file_size;
            return Ok(result);
        }

        let mut session = ServerSession::connect_ssh(config, &server_root)
            .await
            .context(
                "Failed to connect to sy --server on remote. Is 'sy' installed on the remote host?",
            )?;

        let entry = FileListEntry {
            path: rel_path.clone(),
            size: file_size,
            mtime,
            mode: 0o644,
            flags: 0,
            symlink_target: None,
        };
        session.send_file_list(vec![entry]).await?;
        let ack = session.read_ack().await?;

        let needs_transfer = ack
            .decisions
            .first()
            .map(|d| d.action != crate::server::protocol::Action::Skip)
            .unwrap_or(false);

        if needs_transfer {
            let data = tokio::fs::read(source).await?;
            let (send_data, flags) =
                if file_size >= COMPRESS_MIN_SIZE && !is_compressed_extension(&rel_path) {
                    match compress(&data, Compression::Zstd) {
                        Ok(compressed) if compressed.len() < data.len() => {
                            (compressed, DATA_FLAG_COMPRESSED)
                        }
                        _ => (data, 0),
                    }
                } else {
                    (data, 0)
                };

            session
                .send_file_data_with_flags(0, 0, flags, send_data.clone())
                .await?;
            session.flush().await?;

            let done = session.read_file_done().await?;
            if done.status == 0 {
                result.uploaded_files = 1;
                result.uploaded_bytes = file_size;
            } else {
                result.failed.push(FailedUpload {
                    path: source.display().to_string(),
                    error: format!("Server returned status {}", done.status),
                });
            }
        } else {
            result.skipped_files = 1;
        }

        session.close().await?;
        return Ok(result);
    }

    if !options.recursive {
        anyhow::bail!("Source is a directory. Use recursive option to upload directories.");
    }

    // Directory upload via server protocol
    let scanner = Scanner::new(source.to_path_buf());
    let entries = scanner.scan().context("Failed to scan source directory")?;

    // Filter and apply max depth
    let filtered_entries: Vec<_> = entries
        .iter()
        .filter(|e| {
            if !filter.should_include(&e.relative_path, e.is_dir) {
                return false;
            }
            if let Some(max_depth) = options.max_depth {
                let depth = e.relative_path.components().count();
                if depth > max_depth {
                    return false;
                }
            }
            true
        })
        .collect();

    // Separate directories and files
    let mut directories: Vec<String> = Vec::new();
    let mut files: Vec<(StdArc<std::path::PathBuf>, String, u64, i64)> = Vec::new();

    for entry in filtered_entries {
        if entry.is_dir {
            directories.push(entry.relative_path.to_string_lossy().to_string());
        } else if !entry.is_symlink {
            let mtime = entry
                .modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            files.push((
                entry.path.clone(),
                entry.relative_path.to_string_lossy().to_string(),
                entry.size,
                mtime,
            ));
        }
    }

    directories.sort();

    let total_files = files.len();
    let total_dirs = directories.len();
    let total_bytes: u64 = files.iter().map(|(_, _, s, _)| s).sum();

    if options.dry_run {
        result.created_dirs = total_dirs;
        result.uploaded_files = total_files;
        result.uploaded_bytes = total_bytes;
        return Ok(result);
    }

    // Connect to server
    let mut session = ServerSession::connect_ssh(config, dest).await.context(
        "Failed to connect to sy --server on remote. Is 'sy' installed on the remote host?",
    )?;

    // Step 1: Create directories
    if !directories.is_empty() {
        session.send_mkdir_batch(directories).await?;
        let ack = session.read_mkdir_ack().await?;
        result.created_dirs = ack.created as usize;
    }

    // Step 2: Send file list
    let proto_entries: Vec<FileListEntry> = files
        .iter()
        .map(|(_, rel_path, size, mtime)| FileListEntry {
            path: rel_path.clone(),
            size: *size,
            mtime: *mtime,
            mode: 0o644,
            flags: 0,
            symlink_target: None,
        })
        .collect();

    session.send_file_list(proto_entries).await?;
    let ack = session.read_ack().await?;

    // Categorize files by action
    let creates: Vec<(u32, &(StdArc<std::path::PathBuf>, String, u64, i64))> = ack
        .decisions
        .iter()
        .filter_map(|d| {
            if d.action == crate::server::protocol::Action::Create {
                Some((d.index, &files[d.index as usize]))
            } else {
                None
            }
        })
        .collect();

    let updates: Vec<(u32, &(StdArc<std::path::PathBuf>, String, u64, i64))> = ack
        .decisions
        .iter()
        .filter_map(|d| {
            if d.action == crate::server::protocol::Action::Update {
                Some((d.index, &files[d.index as usize]))
            } else {
                None
            }
        })
        .collect();

    result.skipped_files = total_files - creates.len() - updates.len();

    // Step 3: Transfer files
    let mut files_sent = Vec::new();

    for (idx, (abs_path, rel_path, size, _)) in creates.iter().chain(updates.iter()) {
        let data = tokio::fs::read(&**abs_path).await?;
        let (send_data, flags) = if *size >= COMPRESS_MIN_SIZE && !is_compressed_extension(rel_path)
        {
            match compress(&data, Compression::Zstd) {
                Ok(compressed) if compressed.len() < data.len() => {
                    (compressed, DATA_FLAG_COMPRESSED)
                }
                _ => (data, 0),
            }
        } else {
            (data, 0)
        };

        result.uploaded_bytes += send_data.len() as u64;
        session
            .send_file_data_with_flags(*idx, 0, flags, send_data)
            .await?;
        files_sent.push((*idx, rel_path.clone()));
    }

    session.flush().await?;

    for (_idx, rel_path) in files_sent {
        let done = session.read_file_done().await?;
        if done.status == 0 {
            result.uploaded_files += 1;
        } else {
            result.failed.push(FailedUpload {
                path: rel_path,
                error: format!("Server returned status {}", done.status),
            });
        }
    }

    session.close().await?;

    Ok(result)
}

/// Upload using daemon protocol (connects to daemon via Unix socket)
#[cfg(unix)]
pub async fn upload_via_daemon(
    source: &Path,
    socket_path: &str,
    dest: &Path,
    options: &UploadOptions,
    filter: &FilterEngine,
) -> Result<UploadResult> {
    use crate::compress::{compress, is_compressed_extension, Compression};
    use crate::server::protocol::{FileListEntry, DATA_FLAG_COMPRESSED};
    use std::sync::Arc as StdArc;

    let mut result = UploadResult::new(
        &source.display().to_string(),
        &dest.display().to_string(),
        options.dry_run,
    );

    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    let source_is_file = source.is_file();

    if source_is_file {
        // Single file upload via daemon
        let file_size = std::fs::metadata(source)?.len();
        let rel_path = source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mtime = std::fs::metadata(source)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        if options.dry_run {
            result.uploaded_files = 1;
            result.uploaded_bytes = file_size;
            return Ok(result);
        }

        // For single file upload, dest could be:
        // 1. A directory path (e.g., /remote/) - file goes into that directory
        // 2. A file path (e.g., /remote/newname.txt) - file gets renamed
        // We use dest directly as the root for the daemon connection
        let mut session = DaemonSession::connect(socket_path, dest)
            .await
            .context("Failed to connect to daemon")?;

        let entry = FileListEntry {
            path: rel_path.clone(),
            size: file_size,
            mtime,
            mode: 0o644,
            flags: 0,
            symlink_target: None,
        };
        session.send_file_list(vec![entry]).await?;
        let ack = session.read_ack().await?;

        let needs_transfer = ack
            .decisions
            .first()
            .map(|d| d.action != crate::server::protocol::Action::Skip)
            .unwrap_or(false);

        if needs_transfer {
            let data = tokio::fs::read(source).await?;
            let (send_data, flags) =
                if file_size >= COMPRESS_MIN_SIZE && !is_compressed_extension(&rel_path) {
                    match compress(&data, Compression::Zstd) {
                        Ok(compressed) if compressed.len() < data.len() => {
                            (compressed, DATA_FLAG_COMPRESSED)
                        }
                        _ => (data, 0),
                    }
                } else {
                    (data, 0)
                };

            session
                .send_file_data_with_flags(0, 0, flags, send_data.clone())
                .await?;
            session.flush().await?;

            let done = session.read_file_done().await?;
            if done.status == 0 {
                result.uploaded_files = 1;
                result.uploaded_bytes = file_size;
            } else {
                result.failed.push(FailedUpload {
                    path: source.display().to_string(),
                    error: format!("Daemon returned status {}", done.status),
                });
            }
        } else {
            result.skipped_files = 1;
        }

        session.close().await?;
        return Ok(result);
    }

    if !options.recursive {
        anyhow::bail!("Source is a directory. Use recursive option to upload directories.");
    }

    // Directory upload via daemon
    let scanner = Scanner::new(source.to_path_buf());
    let entries = scanner.scan().context("Failed to scan source directory")?;

    // Filter and apply max depth
    let filtered_entries: Vec<_> = entries
        .iter()
        .filter(|e| {
            if !filter.should_include(&e.relative_path, e.is_dir) {
                return false;
            }
            if let Some(max_depth) = options.max_depth {
                let depth = e.relative_path.components().count();
                if depth > max_depth {
                    return false;
                }
            }
            true
        })
        .collect();

    // Separate directories and files
    let mut directories: Vec<String> = Vec::new();
    let mut files: Vec<(StdArc<std::path::PathBuf>, String, u64, i64)> = Vec::new();

    for entry in filtered_entries {
        if entry.is_dir {
            directories.push(entry.relative_path.to_string_lossy().to_string());
        } else if !entry.is_symlink {
            let mtime = entry
                .modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            files.push((
                entry.path.clone(),
                entry.relative_path.to_string_lossy().to_string(),
                entry.size,
                mtime,
            ));
        }
    }

    directories.sort();

    let total_files = files.len();
    let total_dirs = directories.len();
    let total_bytes: u64 = files.iter().map(|(_, _, s, _)| s).sum();

    if options.dry_run {
        result.created_dirs = total_dirs;
        result.uploaded_files = total_files;
        result.uploaded_bytes = total_bytes;
        return Ok(result);
    }

    // Connect to daemon
    let mut session = DaemonSession::connect(socket_path, dest)
        .await
        .context("Failed to connect to daemon")?;

    // Step 1: Create directories
    if !directories.is_empty() {
        session.send_mkdir_batch(directories).await?;
        let ack = session.read_mkdir_ack().await?;
        result.created_dirs = ack.created as usize;
    }

    // Step 2: Send file list
    let proto_entries: Vec<FileListEntry> = files
        .iter()
        .map(|(_, rel_path, size, mtime)| FileListEntry {
            path: rel_path.clone(),
            size: *size,
            mtime: *mtime,
            mode: 0o644,
            flags: 0,
            symlink_target: None,
        })
        .collect();

    session.send_file_list(proto_entries).await?;
    let ack = session.read_ack().await?;

    // Categorize files by action
    let creates: Vec<(u32, &(StdArc<std::path::PathBuf>, String, u64, i64))> = ack
        .decisions
        .iter()
        .filter_map(|d| {
            if d.action == crate::server::protocol::Action::Create {
                Some((d.index, &files[d.index as usize]))
            } else {
                None
            }
        })
        .collect();

    let updates: Vec<(u32, &(StdArc<std::path::PathBuf>, String, u64, i64))> = ack
        .decisions
        .iter()
        .filter_map(|d| {
            if d.action == crate::server::protocol::Action::Update {
                Some((d.index, &files[d.index as usize]))
            } else {
                None
            }
        })
        .collect();

    result.skipped_files = total_files - creates.len() - updates.len();

    // Step 3: Transfer files
    let mut files_sent = Vec::new();

    for (idx, (abs_path, rel_path, size, _)) in creates.iter().chain(updates.iter()) {
        let data = tokio::fs::read(&**abs_path).await?;
        let (send_data, flags) = if *size >= COMPRESS_MIN_SIZE && !is_compressed_extension(rel_path)
        {
            match compress(&data, Compression::Zstd) {
                Ok(compressed) if compressed.len() < data.len() => {
                    (compressed, DATA_FLAG_COMPRESSED)
                }
                _ => (data, 0),
            }
        } else {
            (data, 0)
        };

        result.uploaded_bytes += send_data.len() as u64;
        session
            .send_file_data_with_flags(*idx, 0, flags, send_data)
            .await?;
        files_sent.push((*idx, rel_path.clone()));
    }

    session.flush().await?;

    for (_idx, rel_path) in files_sent {
        let done = session.read_file_done().await?;
        if done.status == 0 {
            result.uploaded_files += 1;
        } else {
            result.failed.push(FailedUpload {
                path: rel_path,
                error: format!("Daemon returned status {}", done.status),
            });
        }
    }

    session.close().await?;

    Ok(result)
}

/// Main upload function that dispatches to appropriate transport
pub async fn upload(
    source: &Path,
    dest: &SyncPath,
    options: &UploadOptions,
) -> Result<UploadResult> {
    use crate::retry::RetryConfig;

    if dest.is_local() {
        anyhow::bail!(
            "Destination must be a remote path (SSH, S3, or GCS). Use sy or cp for local copies."
        );
    }

    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    let filter = options.build_filter()?;

    match dest {
        SyncPath::Local { .. } => unreachable!(),

        #[cfg(feature = "ssh")]
        SyncPath::Remote { host, user, .. } => {
            use crate::ssh::config::parse_ssh_config;
            use crate::transport::ssh::SshTransport;

            // Always parse SSH config first to get port, identity files, etc.
            // Then override user if provided in the path
            let mut config =
                parse_ssh_config(host).unwrap_or_else(|_| crate::ssh::config::SshConfig::new(host));
            if let Some(user) = user {
                config.user = user.clone();
            }

            if options.sftp {
                let retry_config = RetryConfig::default();
                let pool_size = options.parallel.max(1);
                let transport = Arc::new(
                    SshTransport::with_retry_config(&config, pool_size, retry_config)
                        .await
                        .context("Failed to create SSH transport")?,
                );

                transport
                    .prepare_for_transfer(1000)
                    .await
                    .context("Failed to expand SSH connection pool")?;

                upload_to_transport(transport, source, dest.path(), options, &filter).await
            } else {
                upload_via_server_protocol(source, &config, dest.path(), options, &filter).await
            }
        }

        #[cfg(not(feature = "ssh"))]
        SyncPath::Remote { .. } => {
            anyhow::bail!(
                "SSH support not enabled. Reinstall with: cargo install sy --features ssh"
            );
        }

        #[cfg(feature = "s3")]
        SyncPath::S3 {
            bucket,
            key,
            region,
            endpoint,
            ..
        } => {
            use crate::transport::s3::S3Transport;

            let transport = Arc::new(
                S3Transport::new(
                    bucket.clone(),
                    key.clone(),
                    region.clone(),
                    endpoint.clone(),
                )
                .await
                .context("Failed to create S3 transport")?,
            );

            upload_to_transport(transport, source, dest.path(), options, &filter).await
        }

        #[cfg(not(feature = "s3"))]
        SyncPath::S3 { .. } => {
            anyhow::bail!("S3 support not enabled. Reinstall with: cargo install sy --features s3");
        }

        #[cfg(feature = "gcs")]
        SyncPath::Gcs {
            bucket,
            key,
            project_id,
            service_account_path,
            ..
        } => {
            use crate::transport::gcs::GcsTransport;

            let transport = Arc::new(
                GcsTransport::new(
                    bucket.clone(),
                    key.clone(),
                    project_id.clone(),
                    service_account_path.clone(),
                )
                .await
                .context("Failed to create GCS transport")?,
            );

            upload_to_transport(transport, source, dest.path(), options, &filter).await
        }

        #[cfg(not(feature = "gcs"))]
        SyncPath::Gcs { .. } => {
            anyhow::bail!(
                "GCS support not enabled. Reinstall with: cargo install sy --features gcs"
            );
        }

        #[cfg(unix)]
        SyncPath::Daemon { path, .. } => {
            let socket_path = options.daemon_socket.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Daemon socket path required for daemon:/ paths. Use with_daemon_socket() or set daemon_socket option."
                )
            })?;

            upload_via_daemon(source, socket_path, path, options, &filter).await
        }

        #[cfg(not(unix))]
        SyncPath::Daemon { .. } => {
            anyhow::bail!("Daemon mode is only supported on Unix systems");
        }
    }
}
