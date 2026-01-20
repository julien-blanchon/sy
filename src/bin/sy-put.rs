//! sy-put - Upload files from local to remote backends
//!
//! This tool uploads files and directories from local filesystem to SSH, S3, and GCS backends,
//! similar to `rclone copy {local} {remote}`.
//!
//! For SSH destinations, this uses sy's server protocol (spawning `sy --server` on remote)
//! for high-performance pipelined transfers, similar to rsync.

use anyhow::{Context, Result};
use clap::Parser;
use futures::stream::{self, StreamExt};
use std::path::Path;
use std::sync::Arc;
use sy::filter::FilterEngine;
use sy::path::SyncPath;
use sy::retry::RetryConfig;
use sy::sync::scanner::Scanner;
use sy::transport::Transport;
use tokio::sync::Mutex;
use tracing_subscriber::{fmt, EnvFilter};

#[cfg(feature = "ssh")]
use sy::ssh::config::{parse_ssh_config, SshConfig};
#[cfg(feature = "ssh")]
use sy::transport::ssh::SshTransport;

#[cfg(feature = "s3")]
use sy::transport::s3::S3Transport;

#[cfg(feature = "gcs")]
use sy::transport::gcs::GcsTransport;

// Server protocol for high-performance SSH transfers
#[cfg(feature = "ssh")]
use sy::compress::{compress, is_compressed_extension, Compression};
#[cfg(feature = "ssh")]
use sy::server::protocol::{FileListEntry, DATA_FLAG_COMPRESSED};
#[cfg(feature = "ssh")]
use sy::transport::server::ServerSession;

fn parse_sync_path(s: &str) -> Result<SyncPath, String> {
    Ok(SyncPath::parse(s))
}

#[derive(Parser, Debug)]
#[command(name = "sy-put")]
#[command(about = "Upload files to remote storage (S3, GCS, SSH)", long_about = None)]
#[command(version)]
#[command(after_help = "EXAMPLES:
    # Upload a single file to S3
    sy-put /local/file.txt s3://bucket/path/file.txt

    # Upload a directory recursively to S3
    sy-put /local/dir s3://bucket/prefix/ -R

    # Upload to GCS
    sy-put /local/file.txt gs://bucket/path/file.txt

    # Upload to SSH
    sy-put /local/file.txt user@host:/remote/path/file.txt

    # Dry-run (preview what would be uploaded)
    sy-put /local/dir s3://bucket/prefix/ -R --dry-run

    # Upload with filters
    sy-put /local/dir s3://bucket/prefix/ -R --include \"*.txt\" --exclude \"*.tmp\"

    # Upload with max depth
    sy-put /local/dir s3://bucket/prefix/ -R --max-depth 2

For more information: https://github.com/nijaru/sy")]
struct Cli {
    /// Local source path
    #[arg()]
    pub source: String,

    /// Remote destination path (SSH, S3, GCS)
    /// Examples: user@host:/path, s3://bucket/path, gs://bucket/path
    #[arg(value_parser = parse_sync_path)]
    pub destination: SyncPath,

    /// Recursive upload (for directories)
    #[arg(short = 'R', long)]
    pub recursive: bool,

    /// Maximum depth for recursive upload (default: unlimited)
    #[arg(long)]
    pub max_depth: Option<usize>,

    /// Preview changes without actually uploading (dry-run)
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Exclude files matching pattern (can be repeated)
    /// Examples: "*.log", "node_modules", "target/"
    #[arg(long)]
    pub exclude: Vec<String>,

    /// Include files matching pattern (can be repeated, processed in order with --exclude)
    /// Examples: "*.rs", "important.log"
    #[arg(long)]
    pub include: Vec<String>,

    /// Number of parallel uploads (default: 8)
    #[arg(short = 'j', long, default_value = "8")]
    pub jobs: usize,

    /// Use SFTP instead of server protocol for SSH (slower but doesn't require sy on remote)
    #[arg(long)]
    pub sftp: bool,

    /// Verbosity level (can be repeated: -v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Quiet mode (only output errors)
    #[arg(short, long)]
    pub quiet: bool,

    /// Output JSON results
    #[arg(long)]
    pub json: bool,
}

impl Cli {
    fn log_level(&self) -> tracing::Level {
        if self.quiet {
            return tracing::Level::ERROR;
        }

        match self.verbose {
            0 => tracing::Level::INFO,
            1 => tracing::Level::DEBUG,
            _ => tracing::Level::TRACE,
        }
    }

    fn build_filter(&self) -> Result<FilterEngine> {
        let mut filter = FilterEngine::new();

        // Add include rules first (they take precedence when matched first)
        for pattern in &self.include {
            filter.add_include(pattern)?;
        }

        // Add exclude rules
        for pattern in &self.exclude {
            filter.add_exclude(pattern)?;
        }

        Ok(filter)
    }
}

/// Result of an upload operation
#[derive(Debug, serde::Serialize)]
struct UploadResult {
    source: String,
    destination: String,
    uploaded_files: usize,
    uploaded_bytes: u64,
    created_dirs: usize,
    failed: Vec<FailedUpload>,
    dry_run: bool,
}

#[derive(Debug, serde::Serialize)]
struct FailedUpload {
    path: String,
    error: String,
}

async fn upload_to_transport<T: Transport + Send + Sync + 'static>(
    transport: Arc<T>,
    source: &Path,
    dest: &Path,
    cli: &Cli,
    filter: &FilterEngine,
) -> Result<UploadResult> {
    let result = Arc::new(Mutex::new(UploadResult {
        source: source.display().to_string(),
        destination: dest.display().to_string(),
        uploaded_files: 0,
        uploaded_bytes: 0,
        created_dirs: 0,
        failed: Vec::new(),
        dry_run: cli.dry_run,
    }));

    // Check if source exists
    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    let source_is_file = source.is_file();

    if source_is_file {
        // Single file upload
        let file_size = std::fs::metadata(source)?.len();

        if cli.dry_run {
            if !cli.quiet {
                println!(
                    "Would upload: {} -> {} ({} bytes)",
                    source.display(),
                    dest.display(),
                    file_size
                );
            }
            let mut r = result.lock().await;
            r.uploaded_files = 1;
            r.uploaded_bytes = file_size;
        } else {
            // Read file and upload
            let data = tokio::fs::read(source).await?;
            let mtime = std::fs::metadata(source)?.modified()?;

            // Create parent directories if needed
            if let Some(parent) = dest.parent() {
                if !parent.as_os_str().is_empty() {
                    transport.create_dir_all(parent).await.ok(); // Ignore errors for cloud storage
                }
            }

            transport
                .write_file(dest, &data, mtime)
                .await
                .context(format!("Failed to upload {}", source.display()))?;

            if !cli.quiet {
                println!(
                    "Uploaded: {} -> {} ({} bytes)",
                    source.display(),
                    dest.display(),
                    file_size
                );
            }
            let mut r = result.lock().await;
            r.uploaded_files = 1;
            r.uploaded_bytes = file_size;
        }
    } else if cli.recursive {
        // Directory upload
        let scanner = Scanner::new(source.to_path_buf());
        let entries = scanner.scan().context("Failed to scan source directory")?;

        // Filter entries and apply max depth
        let filtered_entries: Vec<_> = entries
            .iter()
            .filter(|e| {
                // Apply filter
                if !filter.should_include(&e.relative_path, e.is_dir) {
                    return false;
                }

                // Apply max depth
                if let Some(max_depth) = cli.max_depth {
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

        // Create directories first (sequentially to ensure parent dirs exist)
        for entry in &dirs {
            let dest_path = dest.join(&*entry.relative_path);
            if cli.dry_run {
                if !cli.quiet && cli.verbose > 0 {
                    println!("Would create directory: {}", dest_path.display());
                }
                let mut r = result.lock().await;
                r.created_dirs += 1;
            } else {
                match transport.create_dir_all(&dest_path).await {
                    Ok(()) => {
                        if !cli.quiet && cli.verbose > 0 {
                            println!("Created directory: {}", dest_path.display());
                        }
                        let mut r = result.lock().await;
                        r.created_dirs += 1;
                    }
                    Err(e) => {
                        // Ignore directory creation errors for cloud storage (they don't have real dirs)
                        tracing::debug!("Directory creation note: {}", e);
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

        let total_files = files.len();
        let concurrency = cli.jobs.max(1);
        let dry_run = cli.dry_run;
        let quiet = cli.quiet;
        let verbose = cli.verbose;

        if dry_run {
            // Dry run - just count
            for (source_path, dest_path, file_size) in &files {
                if !quiet {
                    println!(
                        "Would upload: {} -> {} ({} bytes)",
                        source_path.display(),
                        dest_path.display(),
                        file_size
                    );
                }
            }
            let mut r = result.lock().await;
            r.uploaded_files = total_files;
            r.uploaded_bytes = files.iter().map(|(_, _, s)| s).sum();
        } else {
            // Parallel uploads
            let upload_tasks: Vec<_> = files
                .into_iter()
                .map(|(source_path, dest_path, _file_size)| {
                    let transport = Arc::clone(&transport);
                    let result = Arc::clone(&result);

                    async move {
                        match upload_single_file(&source_path, &dest_path, &*transport).await {
                            Ok(bytes) => {
                                if !quiet && verbose > 0 {
                                    println!(
                                        "Uploaded: {} -> {} ({} bytes)",
                                        source_path.display(),
                                        dest_path.display(),
                                        bytes
                                    );
                                }
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
                                if !quiet {
                                    tracing::warn!(
                                        "Failed to upload {}: {}",
                                        source_path.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                })
                .collect();

            // Execute uploads with controlled concurrency
            stream::iter(upload_tasks)
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await;
        }
    } else {
        anyhow::bail!("Source is a directory. Use -R/--recursive to upload directories.");
    }

    // Extract result from Arc<Mutex>
    let final_result = Arc::try_unwrap(result)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap result"))?
        .into_inner();

    Ok(final_result)
}

async fn upload_single_file<T: Transport + ?Sized>(
    source: &Path,
    dest: &Path,
    transport: &T,
) -> Result<u64> {
    let data = tokio::fs::read(source).await?;
    let file_size = data.len() as u64;
    let mtime = std::fs::metadata(source)?.modified()?;

    // Create parent directories if needed (for cloud storage, parent dirs are implicit)
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
async fn upload_via_server_protocol(
    source: &Path,
    config: &SshConfig,
    dest: &Path,
    cli: &Cli,
    filter: &FilterEngine,
) -> Result<UploadResult> {
    use std::sync::Arc as StdArc;

    let mut result = UploadResult {
        source: source.display().to_string(),
        destination: dest.display().to_string(),
        uploaded_files: 0,
        uploaded_bytes: 0,
        created_dirs: 0,
        failed: Vec::new(),
        dry_run: cli.dry_run,
    };

    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    let source_is_file = source.is_file();

    if source_is_file {
        // Single file upload - still use server protocol for consistency
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

        if cli.dry_run {
            if !cli.quiet {
                println!(
                    "Would upload: {} -> {} ({} bytes)",
                    source.display(),
                    dest.display(),
                    file_size
                );
            }
            result.uploaded_files = 1;
            result.uploaded_bytes = file_size;
            return Ok(result);
        }

        // Connect to server
        let mut session = ServerSession::connect_ssh(config, dest.parent().unwrap_or(dest))
            .await
            .context(
                "Failed to connect to sy --server on remote. Is 'sy' installed on the remote host?",
            )?;

        // Send file list with single file
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

        // Check if file needs transfer
        let needs_transfer = ack
            .decisions
            .first()
            .map(|d| d.action != sy::server::protocol::Action::Skip)
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
                result.uploaded_bytes = send_data.len() as u64;
                if !cli.quiet {
                    println!(
                        "Uploaded: {} -> {} ({} bytes)",
                        source.display(),
                        dest.display(),
                        file_size
                    );
                }
            } else {
                result.failed.push(FailedUpload {
                    path: source.display().to_string(),
                    error: format!("Server returned status {}", done.status),
                });
            }
        } else {
            if !cli.quiet && cli.verbose > 0 {
                println!("Skipped (up-to-date): {}", source.display());
            }
        }

        session.close().await?;
        return Ok(result);
    }

    if !cli.recursive {
        anyhow::bail!("Source is a directory. Use -R/--recursive to upload directories.");
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
            if let Some(max_depth) = cli.max_depth {
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

    // Sort directories by depth (parents first)
    directories.sort();

    let total_files = files.len();
    let total_dirs = directories.len();
    let total_bytes: u64 = files.iter().map(|(_, _, s, _)| s).sum();

    tracing::info!(
        "Source: {} files ({} bytes), {} directories",
        total_files,
        total_bytes,
        total_dirs
    );

    if cli.dry_run {
        for dir in &directories {
            if !cli.quiet && cli.verbose > 0 {
                println!("Would create directory: {}", dir);
            }
        }
        for (abs_path, rel_path, size, _) in &files {
            if !cli.quiet {
                println!(
                    "Would upload: {} -> {} ({} bytes)",
                    abs_path.display(),
                    rel_path,
                    size
                );
            }
        }
        result.created_dirs = total_dirs;
        result.uploaded_files = total_files;
        result.uploaded_bytes = total_bytes;
        return Ok(result);
    }

    // Connect to server
    let mut session = ServerSession::connect_ssh(config, dest).await.context(
        "Failed to connect to sy --server on remote. Is 'sy' installed on the remote host?",
    )?;

    // Step 1: Create directories (batched)
    if !directories.is_empty() {
        tracing::debug!("Creating {} directories...", directories.len());
        session.send_mkdir_batch(directories).await?;
        let ack = session.read_mkdir_ack().await?;
        result.created_dirs = ack.created as usize;
        if !ack.failed.is_empty() {
            for (path, err) in &ack.failed {
                tracing::warn!("Failed to create dir {}: {}", path, err);
            }
        }
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

    tracing::debug!("Sending file list ({} files)...", total_files);
    session.send_file_list(proto_entries).await?;

    tracing::debug!("Waiting for server decisions...");
    let ack = session.read_ack().await?;

    // Categorize files by action
    let creates: Vec<(u32, &(StdArc<std::path::PathBuf>, String, u64, i64))> = ack
        .decisions
        .iter()
        .filter_map(|d| {
            if d.action == sy::server::protocol::Action::Create {
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
            if d.action == sy::server::protocol::Action::Update {
                Some((d.index, &files[d.index as usize]))
            } else {
                None
            }
        })
        .collect();

    let files_to_transfer = creates.len() + updates.len();
    tracing::info!("{} files need transfer", files_to_transfer);

    // Step 3: Transfer files (pipelined - send all, then wait for confirmations)
    let mut files_sent = Vec::new();

    // Send creates
    for (idx, (abs_path, rel_path, size, _)) in &creates {
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

    // Send updates
    for (idx, (abs_path, rel_path, size, _)) in &updates {
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

    // Flush and wait for confirmations
    session.flush().await?;

    for (_idx, rel_path) in files_sent {
        let done = session.read_file_done().await?;
        if done.status == 0 {
            result.uploaded_files += 1;
            if !cli.quiet && cli.verbose > 0 {
                println!("Uploaded: {}", rel_path);
            }
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup logging
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(cli.log_level().as_str()));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .compact()
        .init();

    // Validate destination is remote
    if cli.destination.is_local() {
        anyhow::bail!(
            "Destination must be a remote path (SSH, S3, or GCS). Use sy or cp for local copies."
        );
    }

    // Parse source as local path
    let source = std::path::Path::new(&cli.source);
    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    // Build filter engine
    let path_filter = cli.build_filter()?;

    tracing::info!(
        "{}Uploading: {} -> {}",
        if cli.dry_run { "[DRY-RUN] " } else { "" },
        source.display(),
        cli.destination
    );

    let result: UploadResult = match &cli.destination {
        SyncPath::Local { .. } => {
            // Already validated above
            unreachable!()
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
                parse_ssh_config(host)?
            };

            if cli.sftp {
                // SFTP mode (slower, but doesn't require sy on remote)
                tracing::info!("Using SFTP transport (--sftp flag)");
                let retry_config = RetryConfig::default();
                let pool_size = cli.jobs.max(1);
                let transport = Arc::new(
                    SshTransport::with_retry_config(&config, pool_size, retry_config)
                        .await
                        .context("Failed to create SSH transport")?,
                );

                tracing::info!(
                    "Expanding SSH connection pool to {} connections...",
                    pool_size
                );
                transport
                    .prepare_for_transfer(1000)
                    .await
                    .context("Failed to expand SSH connection pool")?;

                upload_to_transport(
                    transport,
                    source,
                    cli.destination.path(),
                    &cli,
                    &path_filter,
                )
                .await?
            } else {
                // Server protocol mode (fast, pipelined - requires sy on remote)
                upload_via_server_protocol(
                    source,
                    &config,
                    cli.destination.path(),
                    &cli,
                    &path_filter,
                )
                .await?
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

            upload_to_transport(
                transport,
                source,
                cli.destination.path(),
                &cli,
                &path_filter,
            )
            .await?
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

            upload_to_transport(
                transport,
                source,
                cli.destination.path(),
                &cli,
                &path_filter,
            )
            .await?
        }
        #[cfg(not(feature = "gcs"))]
        SyncPath::Gcs { .. } => {
            anyhow::bail!(
                "GCS support not enabled. Reinstall with: cargo install sy --features gcs"
            );
        }
        SyncPath::Daemon { .. } => {
            anyhow::bail!(
                "Daemon paths are not supported for upload. Use SSH paths directly: user@host:/path"
            );
        }
    };

    // Output results
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if !cli.quiet {
        let action = if cli.dry_run {
            "Would upload"
        } else {
            "Uploaded"
        };
        println!(
            "\n{}: {} files ({} bytes), {} directories",
            action,
            result.uploaded_files,
            format_bytes(result.uploaded_bytes),
            result.created_dirs
        );
        if !result.failed.is_empty() {
            println!("Failed: {} items", result.failed.len());
            for fail in &result.failed {
                println!("  - {}: {}", fail.path, fail.error);
            }
        }
    }

    if !result.failed.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

/// Format bytes in human-readable format
fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}
