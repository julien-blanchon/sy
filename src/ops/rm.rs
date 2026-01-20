//! Remove operations - delete files from storage
//!
//! Supports removing files from local, SSH, S3, GCS, and daemon storage.

use crate::filter::FilterEngine;
use crate::ls::{list_directory, ListOptions};
use crate::path::SyncPath;
use crate::transport::Transport;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Options for remove operations
#[derive(Debug, Clone, Default)]
pub struct RemoveOptions {
    /// Recursive removal (for directories)
    pub recursive: bool,
    /// Maximum depth for recursive removal
    pub max_depth: Option<usize>,
    /// Preview changes without actually removing
    pub dry_run: bool,
    /// Remove empty directories after removing files
    pub rmdirs: bool,
    /// Use SFTP instead of server protocol for SSH
    pub sftp: bool,
    /// Include patterns for filtering
    pub include: Vec<String>,
    /// Exclude patterns for filtering
    pub exclude: Vec<String>,
    /// Daemon socket path (for daemon mode operations)
    pub daemon_socket: Option<String>,
}

impl RemoveOptions {
    pub fn new() -> Self {
        Self::default()
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

    pub fn remove_dirs(mut self) -> Self {
        self.rmdirs = true;
        self
    }

    pub fn use_sftp(mut self) -> Self {
        self.sftp = true;
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

/// Result of a remove operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveResult {
    pub path: String,
    pub removed_files: usize,
    pub removed_dirs: usize,
    pub failed: Vec<FailedRemove>,
    pub dry_run: bool,
}

impl RemoveResult {
    pub fn new(path: &str, dry_run: bool) -> Self {
        Self {
            path: path.to_string(),
            removed_files: 0,
            removed_dirs: 0,
            failed: Vec::new(),
            dry_run,
        }
    }

    pub fn success(&self) -> bool {
        self.failed.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedRemove {
    pub path: String,
    pub error: String,
}

/// Remove files using a transport
pub async fn remove_with_transport<T: Transport>(
    transport: &T,
    path: &Path,
    options: &RemoveOptions,
    filter: &FilterEngine,
) -> Result<RemoveResult> {
    let mut result = RemoveResult::new(&path.display().to_string(), options.dry_run);

    // Check if path exists
    if !transport.exists(path).await? {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    // For recursive removal, scan and collect all entries
    if options.recursive {
        let list_opts = ListOptions {
            recursive: true,
            max_depth: options.max_depth,
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
            if options.dry_run {
                result.removed_files += 1;
            } else {
                match transport.remove(&entry_path, false).await {
                    Ok(()) => {
                        result.removed_files += 1;
                    }
                    Err(e) => {
                        result.failed.push(FailedRemove {
                            path: entry_path.display().to_string(),
                            error: e.to_string(),
                        });
                    }
                }
            }
        }

        // Remove directories if --rmdirs is set
        if options.rmdirs {
            for entry in dirs {
                let entry_path = path.join(&entry.path);
                if options.dry_run {
                    result.removed_dirs += 1;
                } else {
                    match transport.remove(&entry_path, true).await {
                        Ok(()) => {
                            result.removed_dirs += 1;
                        }
                        Err(e) => {
                            result.failed.push(FailedRemove {
                                path: entry_path.display().to_string(),
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            // Finally, remove the root directory
            if options.dry_run {
                result.removed_dirs += 1;
            } else {
                match transport.remove(path, true).await {
                    Ok(()) => {
                        result.removed_dirs += 1;
                    }
                    Err(_) => {
                        // Don't fail if root dir removal fails (may not be empty)
                    }
                }
            }
        }
    } else {
        // Single file removal
        if options.dry_run {
            result.removed_files = 1;
        } else {
            transport
                .remove(path, false)
                .await
                .context(format!("Failed to remove {}", path.display()))?;
            result.removed_files = 1;
        }
    }

    Ok(result)
}

/// Remove files using pure SFTP (no sy-remote required)
///
/// This function uses SFTP's native directory listing and file removal,
/// making it suitable for remote hosts that don't have sy-remote installed.
#[cfg(feature = "ssh")]
pub async fn remove_sftp(
    transport: &crate::transport::ssh::SshTransport,
    path: &Path,
    options: &RemoveOptions,
    filter: &FilterEngine,
) -> Result<RemoveResult> {
    let mut result = RemoveResult::new(&path.display().to_string(), options.dry_run);

    // Check if path exists
    if !transport.exists(path).await? {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    // For recursive removal, use SFTP-based scanning
    if options.recursive {
        // Use SFTP-based recursive scanning (doesn't require sy-remote)
        let entries = transport
            .scan_sftp_recursive(path)
            .await
            .context("Failed to list directory contents via SFTP")?;

        // Collect entries to remove (filtered)
        let mut entries_to_remove: Vec<_> = entries
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

        // Sort by path length descending (remove deepest first for directories)
        entries_to_remove.sort_by(|a, b| {
            b.relative_path
                .to_string_lossy()
                .len()
                .cmp(&a.relative_path.to_string_lossy().len())
        });

        // Remove files first, then directories
        let (dirs, files): (Vec<_>, Vec<_>) = entries_to_remove.into_iter().partition(|e| e.is_dir);

        // Remove files
        for entry in files {
            let entry_path = path.join(&*entry.relative_path);
            if options.dry_run {
                result.removed_files += 1;
            } else {
                match transport.remove(&entry_path, false).await {
                    Ok(()) => {
                        result.removed_files += 1;
                    }
                    Err(e) => {
                        result.failed.push(FailedRemove {
                            path: entry_path.display().to_string(),
                            error: e.to_string(),
                        });
                    }
                }
            }
        }

        // Remove directories if --rmdirs is set
        if options.rmdirs {
            for entry in dirs {
                let entry_path = path.join(&*entry.relative_path);
                if options.dry_run {
                    result.removed_dirs += 1;
                } else {
                    match transport.remove(&entry_path, true).await {
                        Ok(()) => {
                            result.removed_dirs += 1;
                        }
                        Err(e) => {
                            result.failed.push(FailedRemove {
                                path: entry_path.display().to_string(),
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            // Finally, remove the root directory
            if options.dry_run {
                result.removed_dirs += 1;
            } else {
                match transport.remove(path, true).await {
                    Ok(()) => {
                        result.removed_dirs += 1;
                    }
                    Err(_) => {
                        // Don't fail if root dir removal fails (may not be empty)
                    }
                }
            }
        }
    } else {
        // Single file removal
        if options.dry_run {
            result.removed_files = 1;
        } else {
            transport
                .remove(path, false)
                .await
                .context(format!("Failed to remove {}", path.display()))?;
            result.removed_files = 1;
        }
    }

    Ok(result)
}

/// Main remove function that dispatches to appropriate transport
pub async fn remove(target: &SyncPath, options: &RemoveOptions) -> Result<RemoveResult> {
    use crate::integrity::{ChecksumType, IntegrityVerifier};
    use crate::retry::RetryConfig;
    use crate::transport::local::LocalTransport;

    let filter = options.build_filter()?;

    match target {
        SyncPath::Local { .. } => {
            let verifier = IntegrityVerifier::new(ChecksumType::None, false);
            let transport = LocalTransport::with_verifier(verifier);
            remove_with_transport(&transport, target.path(), options, &filter).await
        }

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

            let retry_config = RetryConfig::default();
            let transport = SshTransport::with_retry_config(&config, 1, retry_config)
                .await
                .context("Failed to create SSH transport")?;

            if options.sftp {
                // Use SFTP-specific remove that doesn't require sy-remote
                remove_sftp(&transport, target.path(), options, &filter).await
            } else {
                remove_with_transport(&transport, target.path(), options, &filter).await
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

            let transport = S3Transport::new(
                bucket.clone(),
                key.clone(),
                region.clone(),
                endpoint.clone(),
            )
            .await
            .context("Failed to create S3 transport")?;

            remove_with_transport(&transport, target.path(), options, &filter).await
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

            let transport = GcsTransport::new(
                bucket.clone(),
                key.clone(),
                project_id.clone(),
                service_account_path.clone(),
            )
            .await
            .context("Failed to create GCS transport")?;

            remove_with_transport(&transport, target.path(), options, &filter).await
        }

        #[cfg(not(feature = "gcs"))]
        SyncPath::Gcs { .. } => {
            anyhow::bail!(
                "GCS support not enabled. Reinstall with: cargo install sy --features gcs"
            );
        }

        #[cfg(unix)]
        SyncPath::Daemon { .. } => {
            // Note: Daemon protocol doesn't currently support deletion operations.
            // The DeleteBatch message type exists but isn't implemented in the server.
            // Users should use SSH paths with sftp=true for removal operations.
            anyhow::bail!(
                "Daemon mode doesn't support removal operations yet. Use SSH paths with sftp=true instead: user@host:/path"
            );
        }

        #[cfg(not(unix))]
        SyncPath::Daemon { .. } => {
            anyhow::bail!("Daemon mode is only supported on Unix systems");
        }
    }
}
