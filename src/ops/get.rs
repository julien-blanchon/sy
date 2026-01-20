//! Download operations - get files from remote storage
//!
//! Supports downloading files from SSH, S3, GCS, and daemon to local filesystem.

use crate::filter::FilterEngine;
use crate::ls::{list_directory, ListOptions};
use crate::path::SyncPath;
use crate::transport::Transport;
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(feature = "ssh")]
use crate::server::protocol::Action;
#[cfg(feature = "ssh")]
use crate::ssh::config::SshConfig;
#[cfg(feature = "ssh")]
use crate::transport::server::ServerSession;

#[cfg(unix)]
use crate::transport::server::DaemonSession;

/// Options for download operations
#[derive(Debug, Clone, Default)]
pub struct DownloadOptions {
    /// Recursive download (for directories)
    pub recursive: bool,
    /// Maximum depth for recursive download
    pub max_depth: Option<usize>,
    /// Preview changes without actually downloading
    pub dry_run: bool,
    /// Number of parallel downloads
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

impl DownloadOptions {
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

/// Result of a download operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadResult {
    pub source: String,
    pub destination: String,
    pub downloaded_files: usize,
    pub downloaded_bytes: u64,
    pub created_dirs: usize,
    pub skipped_files: usize,
    pub failed: Vec<FailedDownload>,
    pub dry_run: bool,
}

impl DownloadResult {
    pub fn new(source: &str, destination: &str, dry_run: bool) -> Self {
        Self {
            source: source.to_string(),
            destination: destination.to_string(),
            downloaded_files: 0,
            downloaded_bytes: 0,
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
pub struct FailedDownload {
    pub path: String,
    pub error: String,
}

/// Download files from a transport to local destination
pub async fn download_from_transport<T: Transport + Send + Sync + 'static>(
    transport: Arc<T>,
    source: &Path,
    dest: &Path,
    options: &DownloadOptions,
    filter: &FilterEngine,
) -> Result<DownloadResult> {
    let result = Arc::new(Mutex::new(DownloadResult::new(
        &source.display().to_string(),
        &dest.display().to_string(),
        options.dry_run,
    )));

    // Check if source exists
    if !transport.exists(source).await? {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    if options.recursive {
        // Directory download
        let list_opts = ListOptions {
            recursive: true,
            max_depth: options.max_depth,
            include_dirs: true,
            include_files: true,
        };

        let entries = list_directory(&*transport, source, &list_opts)
            .await
            .context("Failed to list remote directory")?;

        // Filter entries
        let filtered_entries: Vec<_> = entries
            .iter()
            .filter(|e| {
                let entry_path = std::path::Path::new(&e.path);
                filter.should_include(entry_path, e.is_dir)
            })
            .collect();

        // Collect directories to create
        let mut dirs: Vec<_> = filtered_entries.iter().filter(|e| e.is_dir).collect();
        dirs.sort_by(|a, b| a.path.cmp(&b.path));

        // Create local directories first
        for entry in &dirs {
            let dest_path = dest.join(&entry.path);
            if options.dry_run {
                let mut r = result.lock().await;
                r.created_dirs += 1;
            } else {
                match tokio::fs::create_dir_all(&dest_path).await {
                    Ok(()) => {
                        let mut r = result.lock().await;
                        r.created_dirs += 1;
                    }
                    Err(e) => {
                        let mut r = result.lock().await;
                        r.failed.push(FailedDownload {
                            path: dest_path.display().to_string(),
                            error: e.to_string(),
                        });
                    }
                }
            }
        }

        // Collect files to download (skip directories and 0-byte directory markers)
        let files: Vec<_> = filtered_entries
            .iter()
            .filter(|e| {
                if e.is_dir {
                    return false;
                }
                if e.path.ends_with('/') {
                    return false;
                }
                // Skip 0-byte files that look like directory markers
                if e.size == 0 && !e.path.contains('.') {
                    return false;
                }
                true
            })
            .map(|e| (source.join(&e.path), dest.join(&e.path), e.size))
            .collect();

        if options.dry_run {
            let mut r = result.lock().await;
            r.downloaded_files = files.len();
            r.downloaded_bytes = files.iter().map(|(_, _, s)| s).sum();
        } else {
            // Parallel downloads
            let concurrency = options.parallel.max(1);

            let download_tasks: Vec<_> = files
                .into_iter()
                .map(|(source_path, dest_path, _)| {
                    let transport = Arc::clone(&transport);
                    let result = Arc::clone(&result);

                    async move {
                        match download_single_file(&*transport, &source_path, &dest_path).await {
                            Ok(bytes) => {
                                let mut r = result.lock().await;
                                r.downloaded_files += 1;
                                r.downloaded_bytes += bytes;
                            }
                            Err(e) => {
                                let mut r = result.lock().await;
                                r.failed.push(FailedDownload {
                                    path: source_path.display().to_string(),
                                    error: e.to_string(),
                                });
                            }
                        }
                    }
                })
                .collect();

            stream::iter(download_tasks)
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await;
        }
    } else {
        // Single file download
        let file_info = transport.file_info(source).await?;
        let file_size = file_info.size;

        if options.dry_run {
            let mut r = result.lock().await;
            r.downloaded_files = 1;
            r.downloaded_bytes = file_size;
        } else {
            let bytes = download_single_file(&*transport, source, dest).await?;
            let mut r = result.lock().await;
            r.downloaded_files = 1;
            r.downloaded_bytes = bytes;
        }
    }

    let final_result = Arc::try_unwrap(result)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap result"))?
        .into_inner();

    Ok(final_result)
}

/// Download a single file
pub async fn download_single_file<T: Transport + ?Sized>(
    transport: &T,
    source: &Path,
    dest: &Path,
) -> Result<u64> {
    // Create parent directories if needed
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Read from remote
    let data = transport.read_file(source).await?;
    let file_size = data.len() as u64;

    // Get mtime from remote
    let mtime = transport
        .get_mtime(source)
        .await
        .unwrap_or_else(|_| std::time::SystemTime::now());

    // Write to local
    tokio::fs::write(dest, &data).await?;

    // Set modification time
    filetime::set_file_mtime(dest, filetime::FileTime::from_system_time(mtime))?;

    Ok(file_size)
}

/// Download using server protocol (spawns sy --server on remote for pipelined transfers)
#[cfg(feature = "ssh")]
pub async fn download_via_server_protocol(
    config: &SshConfig,
    source: &Path,
    dest: &Path,
    options: &DownloadOptions,
    filter: &FilterEngine,
) -> Result<DownloadResult> {
    use std::collections::HashMap;

    let mut result = DownloadResult::new(
        &source.display().to_string(),
        &dest.display().to_string(),
        options.dry_run,
    );

    // Determine if destination is a file path or directory
    let dest_is_file = if dest.exists() {
        dest.is_file()
    } else {
        let has_extension = dest.extension().is_some();
        let parent_exists = dest.parent().map(|p| p.exists()).unwrap_or(false);
        let no_trailing_slash = !dest.to_string_lossy().ends_with('/');
        parent_exists
            && (has_extension || no_trailing_slash)
            && !source.to_string_lossy().ends_with('/')
    };

    // For file destinations, ensure parent directory exists
    // For directory destinations, ensure the directory exists
    if dest_is_file {
        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
    } else if !dest.exists() {
        std::fs::create_dir_all(dest)?;
    }

    // Scan local destination for comparison (to skip unchanged files)
    let local_entries: HashMap<String, (u64, i64)> = if dest.exists() {
        let mut map = HashMap::new();
        if std::fs::read_dir(dest).is_ok() {
            fn scan_dir(dir: &Path, base: &Path, map: &mut HashMap<String, (u64, i64)>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        if let Ok(metadata) = entry.metadata() {
                            if let Ok(rel_path) = entry.path().strip_prefix(base) {
                                let path_str = rel_path.to_string_lossy().to_string();
                                if metadata.is_dir() {
                                    scan_dir(&entry.path(), base, map);
                                } else {
                                    let mtime = metadata
                                        .modified()
                                        .unwrap_or(std::time::UNIX_EPOCH)
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                        as i64;
                                    map.insert(path_str, (metadata.len(), mtime));
                                }
                            }
                        }
                    }
                }
            }
            scan_dir(dest, dest, &mut map);
        }
        map
    } else {
        HashMap::new()
    };

    // Connect to server in PULL mode
    let mut session = ServerSession::connect_ssh_pull(config, source)
        .await
        .context(
            "Failed to connect to sy --server on remote. Is 'sy' installed on the remote host?",
        )?;

    // Step 1: Receive and create directories
    let mkdir_batch = session.read_mkdir_batch().await?;

    let mut failed_dirs: Vec<(String, String)> = Vec::new();

    for dir_path in &mkdir_batch.paths {
        let dir_path_obj = std::path::Path::new(dir_path);
        if !filter.should_include(dir_path_obj, true) {
            continue;
        }

        if let Some(max_depth) = options.max_depth {
            if dir_path_obj.components().count() > max_depth {
                continue;
            }
        }

        let full_path = dest.join(dir_path);
        if options.dry_run {
            result.created_dirs += 1;
        } else {
            match std::fs::create_dir_all(&full_path) {
                Ok(_) => result.created_dirs += 1,
                Err(e) => failed_dirs.push((dir_path.clone(), e.to_string())),
            }
        }
    }
    session
        .send_mkdir_batch_ack(result.created_dirs as u32, failed_dirs)
        .await?;

    // Step 2: Receive file list and send decisions
    let file_list = session.read_file_list().await?;

    let mut decisions = Vec::with_capacity(file_list.entries.len());
    let mut files_to_receive: Vec<(u32, String, u64)> = Vec::new();

    for (idx, entry) in file_list.entries.iter().enumerate() {
        let entry_path = std::path::Path::new(&entry.path);

        if !filter.should_include(entry_path, false) {
            decisions.push(crate::server::protocol::Decision {
                index: idx as u32,
                action: Action::Skip,
            });
            continue;
        }

        if let Some(max_depth) = options.max_depth {
            if entry_path.components().count() > max_depth {
                decisions.push(crate::server::protocol::Decision {
                    index: idx as u32,
                    action: Action::Skip,
                });
                continue;
            }
        }

        let action = if let Some((local_size, local_mtime)) = local_entries.get(&entry.path) {
            if *local_size == entry.size && *local_mtime >= entry.mtime {
                result.skipped_files += 1;
                Action::Skip
            } else {
                Action::Update
            }
        } else {
            Action::Create
        };

        if action != Action::Skip {
            files_to_receive.push((idx as u32, entry.path.clone(), entry.size));
        }

        decisions.push(crate::server::protocol::Decision {
            index: idx as u32,
            action,
        });
    }

    if options.dry_run {
        result.downloaded_files = files_to_receive.len();
        result.downloaded_bytes = files_to_receive.iter().map(|(_, _, s)| s).sum();

        let skip_decisions: Vec<crate::server::protocol::Decision> = (0..file_list.entries.len())
            .map(|idx| crate::server::protocol::Decision {
                index: idx as u32,
                action: Action::Skip,
            })
            .collect();
        session.send_file_list_ack(skip_decisions).await?;
        session.close().await?;
        return Ok(result);
    }

    session.send_file_list_ack(decisions).await?;

    // Step 3: Receive files
    let mut files_received: Vec<(u32, String, u8)> = Vec::new();
    let single_file_dest = dest_is_file && files_to_receive.len() == 1;

    for (idx, rel_path, _expected_size) in &files_to_receive {
        let file_data = match session.read_file_data().await? {
            Some(data) => data,
            None => break,
        };

        // For single file downloads to a file path, write directly to dest
        // Otherwise, join the relative path to the destination directory
        let full_path = if single_file_dest {
            dest.to_path_buf()
        } else {
            dest.join(rel_path)
        };

        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file_size = file_data.data.len() as u64;
        match std::fs::write(&full_path, &file_data.data) {
            Ok(_) => {
                result.downloaded_bytes += file_size;
                result.downloaded_files += 1;
                files_received.push((*idx, rel_path.clone(), 0));
            }
            Err(e) => {
                result.failed.push(FailedDownload {
                    path: rel_path.clone(),
                    error: e.to_string(),
                });
                files_received.push((*idx, rel_path.clone(), 2));
            }
        }
    }

    for (idx, _path, status) in &files_received {
        session.send_file_done(*idx, *status).await?;
    }

    // Step 4: Handle symlinks
    match session.read_symlink_batch_body().await {
        Ok(symlink_batch) => {
            let mut created = 0u32;
            let mut failed: Vec<(String, String)> = Vec::new();

            for entry in &symlink_batch.entries {
                let link_path = dest.join(&entry.path);

                if link_path.exists() || link_path.symlink_metadata().is_ok() {
                    let _ = std::fs::remove_file(&link_path);
                }

                if let Some(parent) = link_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                #[cfg(unix)]
                {
                    use std::os::unix::fs::symlink;
                    match symlink(&entry.target, &link_path) {
                        Ok(_) => created += 1,
                        Err(e) => failed.push((entry.path.clone(), e.to_string())),
                    }
                }
                #[cfg(not(unix))]
                {
                    failed.push((entry.path.clone(), "Symlinks not supported".to_string()));
                }
            }
            session.send_symlink_batch_ack(created, failed).await?;
        }
        Err(_) => {}
    }

    session.close().await?;

    Ok(result)
}

/// Download using daemon protocol (connects to daemon via Unix socket)
#[cfg(unix)]
pub async fn download_via_daemon(
    socket_path: &str,
    source: &Path,
    dest: &Path,
    options: &DownloadOptions,
    filter: &FilterEngine,
) -> Result<DownloadResult> {
    use crate::server::protocol::Action;
    use std::collections::HashMap;

    let mut result = DownloadResult::new(
        &source.display().to_string(),
        &dest.display().to_string(),
        options.dry_run,
    );

    // Determine if destination is a file path or directory
    // A destination is treated as a file if:
    // 1. It doesn't exist AND its parent exists AND it has a file extension or no trailing slash
    // 2. It already exists as a file
    let dest_is_file = if dest.exists() {
        dest.is_file()
    } else {
        let has_extension = dest.extension().is_some();
        let parent_exists = dest.parent().map(|p| p.exists()).unwrap_or(false);
        let no_trailing_slash = !dest.to_string_lossy().ends_with('/');
        parent_exists
            && (has_extension || no_trailing_slash)
            && !source.to_string_lossy().ends_with('/')
    };

    // For file destinations, ensure parent directory exists
    // For directory destinations, ensure the directory exists
    if dest_is_file {
        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
    } else if !dest.exists() {
        std::fs::create_dir_all(dest)?;
    }

    // Scan local destination for comparison (to skip unchanged files)
    let local_entries: HashMap<String, (u64, i64)> = if dest.exists() {
        let mut map = HashMap::new();
        if std::fs::read_dir(dest).is_ok() {
            fn scan_dir(dir: &Path, base: &Path, map: &mut HashMap<String, (u64, i64)>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        if let Ok(metadata) = entry.metadata() {
                            if let Ok(rel_path) = entry.path().strip_prefix(base) {
                                let path_str = rel_path.to_string_lossy().to_string();
                                if metadata.is_dir() {
                                    scan_dir(&entry.path(), base, map);
                                } else {
                                    let mtime = metadata
                                        .modified()
                                        .unwrap_or(std::time::UNIX_EPOCH)
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                        as i64;
                                    map.insert(path_str, (metadata.len(), mtime));
                                }
                            }
                        }
                    }
                }
            }
            scan_dir(dest, dest, &mut map);
        }
        map
    } else {
        HashMap::new()
    };

    // Connect to daemon in PULL mode
    let mut session = DaemonSession::connect_pull(socket_path, source)
        .await
        .context("Failed to connect to daemon")?;

    // Step 1: Receive and create directories
    let mkdir_batch = session.read_mkdir_batch().await?;

    let mut failed_dirs: Vec<(String, String)> = Vec::new();

    for dir_path in &mkdir_batch.paths {
        let dir_path_obj = std::path::Path::new(dir_path);
        if !filter.should_include(dir_path_obj, true) {
            continue;
        }

        if let Some(max_depth) = options.max_depth {
            if dir_path_obj.components().count() > max_depth {
                continue;
            }
        }

        let full_path = dest.join(dir_path);
        if options.dry_run {
            result.created_dirs += 1;
        } else {
            match std::fs::create_dir_all(&full_path) {
                Ok(_) => result.created_dirs += 1,
                Err(e) => failed_dirs.push((dir_path.clone(), e.to_string())),
            }
        }
    }
    session
        .send_mkdir_batch_ack(result.created_dirs as u32, failed_dirs)
        .await?;

    // Step 2: Receive file list and send decisions
    let file_list = session.read_file_list().await?;

    let mut decisions = Vec::with_capacity(file_list.entries.len());
    let mut files_to_receive: Vec<(u32, String, u64)> = Vec::new();

    for (idx, entry) in file_list.entries.iter().enumerate() {
        let entry_path = std::path::Path::new(&entry.path);

        if !filter.should_include(entry_path, false) {
            decisions.push(crate::server::protocol::Decision {
                index: idx as u32,
                action: Action::Skip,
            });
            continue;
        }

        if let Some(max_depth) = options.max_depth {
            if entry_path.components().count() > max_depth {
                decisions.push(crate::server::protocol::Decision {
                    index: idx as u32,
                    action: Action::Skip,
                });
                continue;
            }
        }

        let action = if let Some((local_size, local_mtime)) = local_entries.get(&entry.path) {
            if *local_size == entry.size && *local_mtime >= entry.mtime {
                result.skipped_files += 1;
                Action::Skip
            } else {
                Action::Update
            }
        } else {
            Action::Create
        };

        if action != Action::Skip {
            files_to_receive.push((idx as u32, entry.path.clone(), entry.size));
        }

        decisions.push(crate::server::protocol::Decision {
            index: idx as u32,
            action,
        });
    }

    if options.dry_run {
        result.downloaded_files = files_to_receive.len();
        result.downloaded_bytes = files_to_receive.iter().map(|(_, _, s)| s).sum();

        let skip_decisions: Vec<crate::server::protocol::Decision> = (0..file_list.entries.len())
            .map(|idx| crate::server::protocol::Decision {
                index: idx as u32,
                action: Action::Skip,
            })
            .collect();
        session.send_file_list_ack(skip_decisions).await?;
        session.close().await?;
        return Ok(result);
    }

    session.send_file_list_ack(decisions).await?;

    // Step 3: Receive files
    let mut files_received: Vec<(u32, String, u8)> = Vec::new();
    let single_file_dest = dest_is_file && files_to_receive.len() == 1;

    for (idx, rel_path, _expected_size) in &files_to_receive {
        let file_data = match session.read_file_data().await? {
            Some(data) => data,
            None => break,
        };

        // For single file downloads to a file path, write directly to dest
        // Otherwise, join the relative path to the destination directory
        let full_path = if single_file_dest {
            dest.to_path_buf()
        } else {
            dest.join(rel_path)
        };

        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file_size = file_data.data.len() as u64;
        match std::fs::write(&full_path, &file_data.data) {
            Ok(_) => {
                result.downloaded_bytes += file_size;
                result.downloaded_files += 1;
                files_received.push((*idx, rel_path.clone(), 0));
            }
            Err(e) => {
                result.failed.push(FailedDownload {
                    path: rel_path.clone(),
                    error: e.to_string(),
                });
                files_received.push((*idx, rel_path.clone(), 2));
            }
        }
    }

    for (idx, _path, status) in &files_received {
        session.send_file_done(*idx, *status).await?;
    }

    // Step 4: Handle symlinks
    match session.read_symlink_batch_body().await {
        Ok(symlink_batch) => {
            let mut created = 0u32;
            let mut failed: Vec<(String, String)> = Vec::new();

            for entry in &symlink_batch.entries {
                let link_path = dest.join(&entry.path);

                if link_path.exists() || link_path.symlink_metadata().is_ok() {
                    let _ = std::fs::remove_file(&link_path);
                }

                if let Some(parent) = link_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                #[cfg(unix)]
                {
                    use std::os::unix::fs::symlink;
                    match symlink(&entry.target, &link_path) {
                        Ok(_) => created += 1,
                        Err(e) => failed.push((entry.path.clone(), e.to_string())),
                    }
                }
                #[cfg(not(unix))]
                {
                    failed.push((entry.path.clone(), "Symlinks not supported".to_string()));
                }
            }
            session.send_symlink_batch_ack(created, failed).await?;
        }
        Err(_) => {}
    }

    session.close().await?;

    Ok(result)
}

/// Download files using pure SFTP (no sy-remote required)
///
/// This function uses SFTP's native directory listing and file transfer,
/// making it suitable for remote hosts that don't have sy-remote installed.
#[cfg(feature = "ssh")]
pub async fn download_sftp(
    transport: Arc<crate::transport::ssh::SshTransport>,
    source: &Path,
    dest: &Path,
    options: &DownloadOptions,
    filter: &FilterEngine,
) -> Result<DownloadResult> {
    let result = Arc::new(Mutex::new(DownloadResult::new(
        &source.display().to_string(),
        &dest.display().to_string(),
        options.dry_run,
    )));

    // Check if source exists
    if !transport.exists(source).await? {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    if options.recursive {
        // Use SFTP-based recursive scanning (doesn't require sy-remote)
        let entries = transport
            .scan_sftp_recursive(source)
            .await
            .context("Failed to list remote directory via SFTP")?;

        // Filter entries
        let filtered_entries: Vec<_> = entries
            .iter()
            .filter(|e| {
                let entry_path = &*e.relative_path;

                // Apply max_depth filter
                if let Some(max_depth) = options.max_depth {
                    if entry_path.components().count() > max_depth {
                        return false;
                    }
                }

                filter.should_include(entry_path, e.is_dir)
            })
            .collect();

        // Collect directories to create
        let mut dirs: Vec<_> = filtered_entries.iter().filter(|e| e.is_dir).collect();
        dirs.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        // Create local directories first
        for entry in &dirs {
            let dest_path = dest.join(&*entry.relative_path);
            if options.dry_run {
                let mut r = result.lock().await;
                r.created_dirs += 1;
            } else {
                match tokio::fs::create_dir_all(&dest_path).await {
                    Ok(()) => {
                        let mut r = result.lock().await;
                        r.created_dirs += 1;
                    }
                    Err(e) => {
                        let mut r = result.lock().await;
                        r.failed.push(FailedDownload {
                            path: dest_path.display().to_string(),
                            error: e.to_string(),
                        });
                    }
                }
            }
        }

        // Collect files to download
        let files: Vec<_> = filtered_entries
            .iter()
            .filter(|e| !e.is_dir)
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
            r.downloaded_files = files.len();
            r.downloaded_bytes = files.iter().map(|(_, _, s)| s).sum();
        } else {
            // Parallel downloads
            let concurrency = options.parallel.max(1);

            let download_tasks: Vec<_> = files
                .into_iter()
                .map(|(source_path, dest_path, _)| {
                    let transport = Arc::clone(&transport);
                    let result = Arc::clone(&result);

                    async move {
                        match download_single_file(&*transport, &source_path, &dest_path).await {
                            Ok(bytes) => {
                                let mut r = result.lock().await;
                                r.downloaded_files += 1;
                                r.downloaded_bytes += bytes;
                            }
                            Err(e) => {
                                let mut r = result.lock().await;
                                r.failed.push(FailedDownload {
                                    path: source_path.display().to_string(),
                                    error: e.to_string(),
                                });
                            }
                        }
                    }
                })
                .collect();

            stream::iter(download_tasks)
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await;
        }
    } else {
        // Single file download
        let file_info = transport.file_info(source).await?;
        let file_size = file_info.size;

        if options.dry_run {
            let mut r = result.lock().await;
            r.downloaded_files = 1;
            r.downloaded_bytes = file_size;
        } else {
            let bytes = download_single_file(&*transport, source, dest).await?;
            let mut r = result.lock().await;
            r.downloaded_files = 1;
            r.downloaded_bytes = bytes;
        }
    }

    let final_result = Arc::try_unwrap(result)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap result"))?
        .into_inner();

    Ok(final_result)
}

/// Main download function that dispatches to appropriate transport
pub async fn download(
    source: &SyncPath,
    dest: &Path,
    options: &DownloadOptions,
) -> Result<DownloadResult> {
    if source.is_local() {
        anyhow::bail!(
            "Source must be a remote path (SSH, S3, or GCS). Use sy or cp for local copies."
        );
    }

    let filter = options.build_filter()?;

    match source {
        SyncPath::Local { .. } => unreachable!(),

        #[cfg(feature = "ssh")]
        SyncPath::Remote { host, user, .. } => {
            use crate::retry::RetryConfig;
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

                // Use SFTP-specific download that doesn't require sy-remote
                download_sftp(transport, source.path(), dest, options, &filter).await
            } else {
                download_via_server_protocol(&config, source.path(), dest, options, &filter).await
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

            download_from_transport(transport, source.path(), dest, options, &filter).await
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

            download_from_transport(transport, source.path(), dest, options, &filter).await
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

            download_via_daemon(socket_path, path, dest, options, &filter).await
        }

        #[cfg(not(unix))]
        SyncPath::Daemon { .. } => {
            anyhow::bail!("Daemon mode is only supported on Unix systems");
        }
    }
}
