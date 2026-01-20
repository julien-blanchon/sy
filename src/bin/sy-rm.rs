//! sy-rm - Remove files/directories across all supported transports
//!
//! This tool removes files and directories from local, SSH, S3, and GCS backends,
//! similar to `rclone delete` / `rclone purge`.

use anyhow::{Context, Result};
use clap::Parser;
use sy::filter::FilterEngine;
use sy::integrity::{ChecksumType, IntegrityVerifier};
use sy::ls::{list_directory, ListOptions};
use sy::path::SyncPath;
use sy::retry::RetryConfig;
use sy::transport::local::LocalTransport;
use sy::transport::Transport;
use tracing_subscriber::{fmt, EnvFilter};

#[cfg(feature = "ssh")]
use sy::ssh::config::{parse_ssh_config, SshConfig};
#[cfg(feature = "ssh")]
use sy::transport::ssh::SshTransport;

#[cfg(feature = "s3")]
use sy::transport::s3::S3Transport;

#[cfg(feature = "gcs")]
use sy::transport::gcs::GcsTransport;

fn parse_sync_path(s: &str) -> Result<SyncPath, String> {
    Ok(SyncPath::parse(s))
}

#[derive(Parser, Debug)]
#[command(name = "sy-rm")]
#[command(about = "Remove files/directories (works with local, SSH, S3, GCS)", long_about = None)]
#[command(version)]
#[command(after_help = "EXAMPLES:
    # Remove a single file
    sy-rm /path/to/file.txt

    # Remove a directory recursively
    sy-rm /path/to/directory -R

    # Remove files from S3
    sy-rm s3://bucket/path/to/file.txt
    sy-rm s3://bucket/prefix/ -R

    # Remove files from GCS
    sy-rm gs://bucket/path/to/file.txt

    # Remove files from SSH
    sy-rm user@host:/path/to/file.txt

    # Dry-run (preview what would be deleted)
    sy-rm /path/to/directory -R --dry-run

    # Remove with filters
    sy-rm /path -R --include \"*.log\" --exclude \"important.log\"

    # Remove with max depth
    sy-rm /path -R --max-depth 2

    # Remove empty directories after file deletion
    sy-rm /path -R --rmdirs

For more information: https://github.com/nijaru/sy")]
struct Cli {
    /// Path to remove (local, SSH, S3, GCS, etc.)
    /// Examples: /path, user@host:/path, s3://bucket/path, gs://bucket/path
    #[arg(value_parser = parse_sync_path)]
    pub path: SyncPath,

    /// Recursive removal (required for directories)
    #[arg(short = 'R', long)]
    pub recursive: bool,

    /// Maximum depth for recursive removal (default: unlimited)
    #[arg(long)]
    pub max_depth: Option<usize>,

    /// Force removal without confirmation
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Remove empty directories after removing files
    #[arg(long)]
    pub rmdirs: bool,

    /// Preview changes without actually removing (dry-run)
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

/// Result of a remove operation
#[derive(Debug, serde::Serialize)]
struct RemoveResult {
    path: String,
    removed_files: usize,
    removed_dirs: usize,
    failed: Vec<FailedRemove>,
    dry_run: bool,
}

#[derive(Debug, serde::Serialize)]
struct FailedRemove {
    path: String,
    error: String,
}

async fn remove_with_transport<T: Transport>(
    transport: &T,
    path: &std::path::Path,
    cli: &Cli,
    filter: &FilterEngine,
) -> Result<RemoveResult> {
    let mut result = RemoveResult {
        path: path.display().to_string(),
        removed_files: 0,
        removed_dirs: 0,
        failed: Vec::new(),
        dry_run: cli.dry_run,
    };

    // Check if path exists
    if !transport.exists(path).await? {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    // For recursive removal, scan and collect all entries
    if cli.recursive {
        let list_opts = ListOptions {
            recursive: true,
            max_depth: cli.max_depth,
            include_dirs: true,
            include_files: true,
        };

        let entries = list_directory(transport, path, &list_opts)
            .await
            .context("Failed to list directory contents")?;

        // Collect entries to remove (filtered)
        let mut files_to_remove: Vec<_> = entries
            .iter()
            .filter(|e| {
                let entry_path = std::path::Path::new(&e.path);
                filter.should_include(entry_path, e.is_dir)
            })
            .collect();

        // Sort by path length descending (remove deepest first for directories)
        files_to_remove.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

        // Remove files first, then directories
        let (dirs, files): (Vec<_>, Vec<_>) = files_to_remove.into_iter().partition(|e| e.is_dir);

        // Remove files
        for entry in files {
            let entry_path = path.join(&entry.path);
            if cli.dry_run {
                if !cli.quiet {
                    println!("Would remove file: {}", entry_path.display());
                }
                result.removed_files += 1;
            } else {
                match transport.remove(&entry_path, false).await {
                    Ok(()) => {
                        if !cli.quiet && cli.verbose > 0 {
                            println!("Removed file: {}", entry_path.display());
                        }
                        result.removed_files += 1;
                    }
                    Err(e) => {
                        result.failed.push(FailedRemove {
                            path: entry_path.display().to_string(),
                            error: e.to_string(),
                        });
                        if !cli.quiet {
                            tracing::warn!("Failed to remove {}: {}", entry_path.display(), e);
                        }
                    }
                }
            }
        }

        // Remove directories if --rmdirs is set
        if cli.rmdirs {
            for entry in dirs {
                let entry_path = path.join(&entry.path);
                if cli.dry_run {
                    if !cli.quiet {
                        println!("Would remove directory: {}", entry_path.display());
                    }
                    result.removed_dirs += 1;
                } else {
                    match transport.remove(&entry_path, true).await {
                        Ok(()) => {
                            if !cli.quiet && cli.verbose > 0 {
                                println!("Removed directory: {}", entry_path.display());
                            }
                            result.removed_dirs += 1;
                        }
                        Err(e) => {
                            result.failed.push(FailedRemove {
                                path: entry_path.display().to_string(),
                                error: e.to_string(),
                            });
                            if !cli.quiet {
                                tracing::warn!(
                                    "Failed to remove directory {}: {}",
                                    entry_path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }

        // Finally, remove the root directory if --rmdirs
        if cli.rmdirs {
            if cli.dry_run {
                if !cli.quiet {
                    println!("Would remove directory: {}", path.display());
                }
                result.removed_dirs += 1;
            } else {
                match transport.remove(path, true).await {
                    Ok(()) => {
                        if !cli.quiet && cli.verbose > 0 {
                            println!("Removed directory: {}", path.display());
                        }
                        result.removed_dirs += 1;
                    }
                    Err(e) => {
                        // Don't fail if root dir removal fails (may not be empty)
                        tracing::debug!(
                            "Could not remove root directory {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }
    } else {
        // Single file removal
        if cli.dry_run {
            if !cli.quiet {
                println!("Would remove: {}", path.display());
            }
            result.removed_files = 1;
        } else {
            transport
                .remove(path, false)
                .await
                .context(format!("Failed to remove {}", path.display()))?;
            if !cli.quiet {
                println!("Removed: {}", path.display());
            }
            result.removed_files = 1;
        }
    }

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

    // Build filter engine
    let path_filter = cli.build_filter()?;

    // Confirmation prompt for non-forced removal
    if !cli.force && !cli.dry_run {
        eprintln!(
            "Warning: This will remove {} {}",
            cli.path,
            if cli.recursive {
                "and all its contents"
            } else {
                ""
            }
        );
        eprintln!("Use --dry-run to preview changes, or --force to skip this prompt.");
        eprintln!("Press Ctrl+C to cancel, or Enter to continue...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
    }

    tracing::info!(
        "{}Removing: {}",
        if cli.dry_run { "[DRY-RUN] " } else { "" },
        cli.path
    );

    let result: RemoveResult = match &cli.path {
        SyncPath::Local { .. } => {
            let verifier = IntegrityVerifier::new(ChecksumType::None, false);
            let transport = LocalTransport::with_verifier(verifier);
            remove_with_transport(&transport, cli.path.path(), &cli, &path_filter).await?
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

            let retry_config = RetryConfig::default();
            let transport = SshTransport::with_retry_config(&config, 1, retry_config)
                .await
                .context("Failed to create SSH transport")?;

            remove_with_transport(&transport, cli.path.path(), &cli, &path_filter).await?
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
            let transport = S3Transport::new(
                bucket.clone(),
                key.clone(),
                region.clone(),
                endpoint.clone(),
            )
            .await
            .context("Failed to create S3 transport")?;

            remove_with_transport(&transport, cli.path.path(), &cli, &path_filter).await?
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
            let transport = GcsTransport::new(
                bucket.clone(),
                key.clone(),
                project_id.clone(),
                service_account_path.clone(),
            )
            .await
            .context("Failed to create GCS transport")?;

            remove_with_transport(&transport, cli.path.path(), &cli, &path_filter).await?
        }
        #[cfg(not(feature = "gcs"))]
        SyncPath::Gcs { .. } => {
            anyhow::bail!(
                "GCS support not enabled. Reinstall with: cargo install sy --features gcs"
            );
        }
        SyncPath::Daemon { .. } => {
            anyhow::bail!(
                "Daemon paths are not supported for removal. Use SSH paths directly: user@host:/path"
            );
        }
    };

    // Output results
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if !cli.quiet {
        let action = if cli.dry_run {
            "Would remove"
        } else {
            "Removed"
        };
        println!(
            "\n{}: {} files, {} directories",
            action, result.removed_files, result.removed_dirs
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
