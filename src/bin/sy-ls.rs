//! sy-ls - List directory contents across all supported transports
//!
//! This tool provides efficient directory listing similar to `ls` or `rclone lsjson`,
//! working with local paths, SSH, S3, GCS, and other supported transports.

use anyhow::{Context, Result};
use clap::Parser;
use sy::integrity::{ChecksumType, IntegrityVerifier};
use sy::ls::{list_directory, ListEntry, ListOptions};
use sy::path::SyncPath;
use sy::retry::RetryConfig;
use sy::transport::local::LocalTransport;
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
#[command(name = "sy-ls")]
#[command(about = "List directory contents (works with local, SSH, S3, GCS)", long_about = None)]
#[command(version)]
#[command(after_help = "EXAMPLES:
    # List local directory
    sy-ls /path/to/directory

    # List with full details
    sy-ls /path/to/directory -v

    # Recursive listing
    sy-ls /path/to/directory -R

    # List remote directory via SSH
    sy-ls user@host:/remote/path

    # List S3 bucket
    sy-ls s3://bucket/path

    # List GCS bucket
    sy-ls gs://bucket/path

    # List only files (no directories)
    sy-ls /path --files-only

    # List with max depth
    sy-ls /path -R --max-depth 2

For more information: https://github.com/nijaru/sy")]
struct Cli {
    /// Path to list (local, SSH, S3, GCS, etc.)
    /// Examples: /path, user@host:/path, s3://bucket/path, gs://bucket/path
    #[arg(value_parser = parse_sync_path)]
    pub path: SyncPath,

    /// Recursive listing (traverse subdirectories)
    #[arg(short = 'R', long)]
    pub recursive: bool,

    /// Maximum depth for recursive listing (default: unlimited)
    #[arg(long)]
    pub max_depth: Option<usize>,

    /// Only list files (exclude directories)
    #[arg(long)]
    pub files_only: bool,

    /// Only list directories (exclude files)
    #[arg(long)]
    pub dirs_only: bool,

    /// Verbosity level (can be repeated: -v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Quiet mode (only output JSON, no progress/info messages)
    #[arg(short, long)]
    pub quiet: bool,

    /// Output format: json (default), human
    #[arg(long, default_value = "json")]
    pub format: OutputFormat,

    /// Pretty-print JSON output (default: compact)
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutputFormat {
    Json,
    Human,
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

    // Validate mutually exclusive options
    if cli.files_only && cli.dirs_only {
        anyhow::bail!("--files-only and --dirs-only are mutually exclusive");
    }

    // Create list options
    let mut list_opts = if cli.recursive {
        ListOptions::recursive()
    } else {
        ListOptions::flat()
    };

    if let Some(depth) = cli.max_depth {
        list_opts = list_opts.with_max_depth(depth);
    }

    list_opts.include_files = !cli.dirs_only;
    list_opts.include_dirs = !cli.files_only;

    // Create transport for the path
    tracing::info!("Listing: {}", cli.path);

    let entries: Vec<ListEntry> = match &cli.path {
        SyncPath::Local { .. } => {
            let verifier = IntegrityVerifier::new(ChecksumType::None, false);
            let transport = LocalTransport::with_verifier(verifier);
            list_directory(&transport, cli.path.path(), &list_opts)
                .await
                .context("Failed to list local directory")?
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

            list_directory(&transport, cli.path.path(), &list_opts)
                .await
                .context("Failed to list remote directory via SSH")?
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
            use sy::transport::{CloudClientOptions, S3Config};

            // Use optimized settings for listing (longer timeouts, more connections)
            let client_options = CloudClientOptions {
                pool_max_idle_per_host: 50,
                pool_idle_timeout_secs: 60,
                connect_timeout_secs: 5,
                request_timeout_secs: 120, // Generous timeout for large listings
                max_retries: 1,
                retry_timeout_secs: 30,
                allow_http: false,
            };

            let config = S3Config {
                access_key_id: None, // From env
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
                50, // More connections for parallel listing
            )
            .await
            .context("Failed to create S3 transport")?;

            list_directory(&transport, cli.path.path(), &list_opts)
                .await
                .context("Failed to list S3 bucket")?
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
            use sy::transport::{CloudClientOptions, GcsConfig};

            // Use optimized settings for listing (longer timeouts, more connections)
            let client_options = CloudClientOptions {
                pool_max_idle_per_host: 50,
                pool_idle_timeout_secs: 60,
                connect_timeout_secs: 5,
                request_timeout_secs: 120, // Generous timeout for large listings
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
                50, // More connections for parallel listing
            )
            .await
            .context("Failed to create GCS transport")?;

            list_directory(&transport, cli.path.path(), &list_opts)
                .await
                .context("Failed to list GCS bucket")?
        }
        #[cfg(not(feature = "gcs"))]
        SyncPath::Gcs { .. } => {
            anyhow::bail!(
                "GCS support not enabled. Reinstall with: cargo install sy --features gcs"
            );
        }
        SyncPath::Daemon { .. } => {
            anyhow::bail!("Daemon paths are not supported for listing. Use SSH paths directly: user@host:/path");
        }
    };

    // Output results
    match cli.format {
        OutputFormat::Json => {
            let json = if cli.pretty {
                serde_json::to_string_pretty(&entries)?
            } else {
                serde_json::to_string(&entries)?
            };
            println!("{}", json);
        }
        OutputFormat::Human => {
            print_human_format(&entries);
        }
    }

    tracing::info!("Listed {} entries", entries.len());
    Ok(())
}

/// Print entries in human-readable format (similar to ls -l)
fn print_human_format(entries: &[ListEntry]) {
    use colored::Colorize;

    for entry in entries {
        let type_char = match entry.entry_type.as_str() {
            "directory" => "d".blue(),
            "symlink" => "l".cyan(),
            "file" => "-".normal(),
            _ => "?".normal(),
        };

        let size_str = if entry.is_dir {
            format!("{:>10}", "-")
        } else {
            format!("{:>10}", format_bytes(entry.size))
        };

        let name = if entry.is_dir {
            entry.path.blue().to_string()
        } else {
            entry.path.to_string()
        };

        let symlink_indicator = if let Some(ref target) = entry.symlink_target {
            format!(" -> {}", target).cyan().to_string()
        } else {
            String::new()
        };

        println!(
            "{} {} {} {}{}",
            type_char, size_str, entry.mod_time, name, symlink_indicator
        );
    }

    println!("\nTotal: {} entries", entries.len());
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
