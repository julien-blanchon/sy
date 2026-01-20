//! sy-get - Download files from remote backends to local
//!
//! This tool downloads files and directories from SSH, S3, and GCS backends to local filesystem,
//! similar to `rclone copy {remote} {local}`.
//!
//! For SSH sources, this uses sy's server protocol (spawning `sy --server` on remote)
//! for high-performance pipelined transfers, similar to rsync.

use anyhow::{Context, Result};
use clap::Parser;
use futures::stream::{self, StreamExt};
use std::path::Path;
use std::sync::Arc;
use sy::filter::FilterEngine;
use sy::ls::{list_directory, ListOptions};
use sy::path::SyncPath;
use sy::retry::RetryConfig;
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
use sy::server::protocol::Action;
#[cfg(feature = "ssh")]
use sy::transport::server::ServerSession;

fn parse_sync_path(s: &str) -> Result<SyncPath, String> {
    Ok(SyncPath::parse(s))
}

#[derive(Parser, Debug)]
#[command(name = "sy-get")]
#[command(about = "Download files from remote storage (S3, GCS, SSH)", long_about = None)]
#[command(version)]
#[command(after_help = "EXAMPLES:
    # Download a single file from S3
    sy-get s3://bucket/path/file.txt /local/file.txt

    # Download a directory recursively from S3
    sy-get s3://bucket/prefix/ /local/dir -R

    # Download from GCS
    sy-get gs://bucket/path/file.txt /local/file.txt

    # Download from SSH
    sy-get user@host:/remote/path/file.txt /local/file.txt

    # Dry-run (preview what would be downloaded)
    sy-get s3://bucket/prefix/ /local/dir -R --dry-run

    # Download with filters
    sy-get s3://bucket/prefix/ /local/dir -R --include \"*.txt\" --exclude \"*.tmp\"

    # Download with max depth
    sy-get s3://bucket/prefix/ /local/dir -R --max-depth 2

For more information: https://github.com/nijaru/sy")]
struct Cli {
    /// Remote source path (SSH, S3, GCS)
    /// Examples: user@host:/path, s3://bucket/path, gs://bucket/path
    #[arg(value_parser = parse_sync_path)]
    pub source: SyncPath,

    /// Local destination path
    #[arg()]
    pub destination: String,

    /// Recursive download (for directories)
    #[arg(short = 'R', long)]
    pub recursive: bool,

    /// Maximum depth for recursive download (default: unlimited)
    #[arg(long)]
    pub max_depth: Option<usize>,

    /// Preview changes without actually downloading (dry-run)
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

    /// Number of parallel downloads (default: 8)
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

/// Result of a download operation
#[derive(Debug, serde::Serialize)]
struct DownloadResult {
    source: String,
    destination: String,
    downloaded_files: usize,
    downloaded_bytes: u64,
    created_dirs: usize,
    failed: Vec<FailedDownload>,
    dry_run: bool,
}

#[derive(Debug, serde::Serialize)]
struct FailedDownload {
    path: String,
    error: String,
}

async fn download_from_transport<T: Transport + Send + Sync + 'static>(
    transport: Arc<T>,
    source: &Path,
    dest: &Path,
    cli: &Cli,
    filter: &FilterEngine,
) -> Result<DownloadResult> {
    let result = Arc::new(Mutex::new(DownloadResult {
        source: source.display().to_string(),
        destination: dest.display().to_string(),
        downloaded_files: 0,
        downloaded_bytes: 0,
        created_dirs: 0,
        failed: Vec::new(),
        dry_run: cli.dry_run,
    }));

    // Check if source exists
    if !transport.exists(source).await? {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }

    if cli.recursive {
        // Directory download
        let list_opts = ListOptions {
            recursive: true,
            max_depth: cli.max_depth,
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

        // Create local directories first (sequentially to ensure parent dirs exist)
        for entry in &dirs {
            let dest_path = dest.join(&entry.path);
            if cli.dry_run {
                if !cli.quiet && cli.verbose > 0 {
                    println!("Would create directory: {}", dest_path.display());
                }
                let mut r = result.lock().await;
                r.created_dirs += 1;
            } else {
                match tokio::fs::create_dir_all(&dest_path).await {
                    Ok(()) => {
                        if !cli.quiet && cli.verbose > 0 {
                            println!("Created directory: {}", dest_path.display());
                        }
                        let mut r = result.lock().await;
                        r.created_dirs += 1;
                    }
                    Err(e) => {
                        let mut r = result.lock().await;
                        r.failed.push(FailedDownload {
                            path: dest_path.display().to_string(),
                            error: e.to_string(),
                        });
                        if !cli.quiet {
                            tracing::warn!(
                                "Failed to create directory {}: {}",
                                dest_path.display(),
                                e
                            );
                        }
                    }
                }
            }
        }

        // Collect files to download (skip directories and 0-byte directory markers)
        let files: Vec<_> = filtered_entries
            .iter()
            .filter(|e| {
                // Skip directories
                if e.is_dir {
                    return false;
                }
                // Skip paths ending with / (directory markers)
                if e.path.ends_with('/') {
                    return false;
                }
                // Skip 0-byte files that look like directory markers (common in S3/GCS)
                if e.size == 0 && !e.path.contains('.') {
                    tracing::debug!("Skipping 0-byte directory marker: {}", e.path);
                    return false;
                }
                true
            })
            .map(|e| (source.join(&e.path), dest.join(&e.path), e.size))
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
                        "Would download: {} -> {} ({} bytes)",
                        source_path.display(),
                        dest_path.display(),
                        file_size
                    );
                }
            }
            let mut r = result.lock().await;
            r.downloaded_files = total_files;
            r.downloaded_bytes = files.iter().map(|(_, _, s)| s).sum();
        } else {
            // Parallel downloads
            let download_tasks: Vec<_> = files
                .into_iter()
                .map(|(source_path, dest_path, _file_size)| {
                    let transport = Arc::clone(&transport);
                    let result = Arc::clone(&result);

                    async move {
                        match download_single_file(&*transport, &source_path, &dest_path).await {
                            Ok(bytes) => {
                                if !quiet && verbose > 0 {
                                    println!(
                                        "Downloaded: {} -> {} ({} bytes)",
                                        source_path.display(),
                                        dest_path.display(),
                                        bytes
                                    );
                                }
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
                                if !quiet {
                                    tracing::warn!(
                                        "Failed to download {}: {}",
                                        source_path.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                })
                .collect();

            // Execute downloads with controlled concurrency
            stream::iter(download_tasks)
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await;
        }
    } else {
        // Single file download
        let file_info = transport.file_info(source).await?;
        let file_size = file_info.size;

        if cli.dry_run {
            if !cli.quiet {
                println!(
                    "Would download: {} -> {} ({} bytes)",
                    source.display(),
                    dest.display(),
                    file_size
                );
            }
            let mut r = result.lock().await;
            r.downloaded_files = 1;
            r.downloaded_bytes = file_size;
        } else {
            let bytes = download_single_file(&*transport, source, dest).await?;

            if !cli.quiet {
                println!(
                    "Downloaded: {} -> {} ({} bytes)",
                    source.display(),
                    dest.display(),
                    bytes
                );
            }
            let mut r = result.lock().await;
            r.downloaded_files = 1;
            r.downloaded_bytes = bytes;
        }
    }

    // Extract result from Arc<Mutex>
    let final_result = Arc::try_unwrap(result)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap result"))?
        .into_inner();

    Ok(final_result)
}

async fn download_single_file<T: Transport + ?Sized>(
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
async fn download_via_server_protocol(
    config: &SshConfig,
    source: &Path,
    dest: &Path,
    cli: &Cli,
    filter: &FilterEngine,
) -> Result<DownloadResult> {
    use std::collections::HashMap;

    let mut result = DownloadResult {
        source: source.display().to_string(),
        destination: dest.display().to_string(),
        downloaded_files: 0,
        downloaded_bytes: 0,
        created_dirs: 0,
        failed: Vec::new(),
        dry_run: cli.dry_run,
    };

    // Ensure destination directory exists
    if !dest.exists() {
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
    tracing::debug!("Received {} directories", mkdir_batch.paths.len());

    let mut failed_dirs: Vec<(String, String)> = Vec::new();

    for dir_path in &mkdir_batch.paths {
        // Apply filter
        let dir_path_obj = std::path::Path::new(dir_path);
        if !filter.should_include(dir_path_obj, true) {
            continue;
        }

        // Apply max depth
        if let Some(max_depth) = cli.max_depth {
            if dir_path_obj.components().count() > max_depth {
                continue;
            }
        }

        let full_path = dest.join(dir_path);
        if cli.dry_run {
            if !cli.quiet && cli.verbose > 0 {
                println!("Would create directory: {}", full_path.display());
            }
            result.created_dirs += 1;
        } else {
            match std::fs::create_dir_all(&full_path) {
                Ok(_) => {
                    result.created_dirs += 1;
                    if !cli.quiet && cli.verbose > 0 {
                        println!("Created directory: {}", dir_path);
                    }
                }
                Err(e) => failed_dirs.push((dir_path.clone(), e.to_string())),
            }
        }
    }
    session
        .send_mkdir_batch_ack(result.created_dirs as u32, failed_dirs)
        .await?;

    // Step 2: Receive file list and send decisions
    let file_list = session.read_file_list().await?;
    tracing::debug!("Received {} files from server", file_list.entries.len());

    let mut decisions = Vec::with_capacity(file_list.entries.len());
    let mut files_to_receive: Vec<(u32, String, u64)> = Vec::new();

    for (idx, entry) in file_list.entries.iter().enumerate() {
        let entry_path = std::path::Path::new(&entry.path);

        // Apply filter
        if !filter.should_include(entry_path, false) {
            decisions.push(sy::server::protocol::Decision {
                index: idx as u32,
                action: Action::Skip,
            });
            continue;
        }

        // Apply max depth
        if let Some(max_depth) = cli.max_depth {
            if entry_path.components().count() > max_depth {
                decisions.push(sy::server::protocol::Decision {
                    index: idx as u32,
                    action: Action::Skip,
                });
                continue;
            }
        }

        // Check if file needs download
        let action = if let Some((local_size, local_mtime)) = local_entries.get(&entry.path) {
            if *local_size == entry.size && *local_mtime >= entry.mtime {
                Action::Skip
            } else {
                Action::Update
            }
        } else {
            Action::Create
        };

        if action != Action::Skip {
            files_to_receive.push((idx as u32, entry.path.clone(), entry.size));
        } else if !cli.quiet && cli.verbose > 1 {
            println!("Skipped (up-to-date): {}", entry.path);
        }

        decisions.push(sy::server::protocol::Decision {
            index: idx as u32,
            action,
        });
    }

    let total_to_download = files_to_receive.len();
    tracing::info!("{} files to download", total_to_download);

    if cli.dry_run {
        // Dry run - just count
        for (_, path, size) in &files_to_receive {
            if !cli.quiet {
                let full_path = dest.join(path);
                println!(
                    "Would download: {} -> {} ({} bytes)",
                    path,
                    full_path.display(),
                    size
                );
            }
        }
        result.downloaded_files = total_to_download;
        result.downloaded_bytes = files_to_receive.iter().map(|(_, _, s)| s).sum();

        // Tell server to skip everything
        let skip_decisions: Vec<sy::server::protocol::Decision> = (0..file_list.entries.len())
            .map(|idx| sy::server::protocol::Decision {
                index: idx as u32,
                action: Action::Skip,
            })
            .collect();
        session.send_file_list_ack(skip_decisions).await?;
        session.close().await?;
        return Ok(result);
    }

    session.send_file_list_ack(decisions).await?;

    // Step 3: Receive files (pipelined - receive all, then send all ACKs)
    let mut files_received: Vec<(u32, String, u8)> = Vec::new(); // (idx, path, status)

    for (idx, rel_path, _expected_size) in &files_to_receive {
        let file_data = match session.read_file_data().await? {
            Some(data) => data,
            None => break, // Server sent symlinks instead
        };

        let full_path = dest.join(rel_path);

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file
        let file_size = file_data.data.len() as u64;
        match std::fs::write(&full_path, &file_data.data) {
            Ok(_) => {
                result.downloaded_bytes += file_size;
                result.downloaded_files += 1;
                if !cli.quiet && cli.verbose > 0 {
                    println!("Downloaded: {} ({} bytes)", rel_path, file_size);
                }
                files_received.push((*idx, rel_path.clone(), 0)); // Status OK
            }
            Err(e) => {
                result.failed.push(FailedDownload {
                    path: rel_path.clone(),
                    error: e.to_string(),
                });
                if !cli.quiet {
                    tracing::warn!("Failed to write {}: {}", full_path.display(), e);
                }
                files_received.push((*idx, rel_path.clone(), 2)); // STATUS_WRITE_ERROR
            }
        }
    }

    // Send all FILE_DONE responses at once (pipelined)
    for (idx, _path, status) in &files_received {
        session.send_file_done(*idx, *status).await?;
    }

    // Step 4: Handle symlinks (if any)
    // Note: Server might send symlink batch after files
    match session.read_symlink_batch_body().await {
        Ok(symlink_batch) => {
            tracing::debug!("Received {} symlinks", symlink_batch.entries.len());
            let mut created = 0u32;
            let mut failed: Vec<(String, String)> = Vec::new();

            for entry in &symlink_batch.entries {
                let link_path = dest.join(&entry.path);

                // Remove existing if present
                if link_path.exists() || link_path.symlink_metadata().is_ok() {
                    let _ = std::fs::remove_file(&link_path);
                }

                // Ensure parent exists
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
        Err(_) => {
            // No symlinks or EOF - this is fine
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

    // Validate source is remote
    if cli.source.is_local() {
        anyhow::bail!(
            "Source must be a remote path (SSH, S3, or GCS). Use sy or cp for local copies."
        );
    }

    // Parse destination as local path
    let dest = std::path::Path::new(&cli.destination);

    // Build filter engine
    let path_filter = cli.build_filter()?;

    tracing::info!(
        "{}Downloading: {} -> {}",
        if cli.dry_run { "[DRY-RUN] " } else { "" },
        cli.source,
        dest.display()
    );

    let result: DownloadResult = match &cli.source {
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

                transport
                    .prepare_for_transfer(1000)
                    .await
                    .context("Failed to expand SSH connection pool")?;

                download_from_transport(transport, cli.source.path(), dest, &cli, &path_filter)
                    .await?
            } else {
                // Server protocol mode (fast, pipelined - requires sy on remote)
                download_via_server_protocol(&config, cli.source.path(), dest, &cli, &path_filter)
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

            download_from_transport(transport, cli.source.path(), dest, &cli, &path_filter).await?
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

            download_from_transport(transport, cli.source.path(), dest, &cli, &path_filter).await?
        }
        #[cfg(not(feature = "gcs"))]
        SyncPath::Gcs { .. } => {
            anyhow::bail!(
                "GCS support not enabled. Reinstall with: cargo install sy --features gcs"
            );
        }
        SyncPath::Daemon { .. } => {
            anyhow::bail!(
                "Daemon paths are not supported for download. Use SSH paths directly: user@host:/path"
            );
        }
    };

    // Output results
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if !cli.quiet {
        let action = if cli.dry_run {
            "Would download"
        } else {
            "Downloaded"
        };
        println!(
            "\n{}: {} files ({} bytes), {} directories",
            action,
            result.downloaded_files,
            format_bytes(result.downloaded_bytes),
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
