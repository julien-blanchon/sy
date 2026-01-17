//! Daemon mode sync - uses Unix socket connection to a running sy daemon
//!
//! This provides the same functionality as server_mode.rs but connects via
//! Unix socket instead of spawning an SSH process. This eliminates the ~2s
//! SSH connection overhead for repeated syncs.
//!
//! # Usage
//!
//! ```bash
//! # Start daemon on remote (via SSH)
//! ssh user@host sy --daemon --socket ~/.sy/daemon.sock
//!
//! # Forward socket locally
//! ssh -L /tmp/sy.sock:~/.sy/daemon.sock user@host -N &
//!
//! # Sync using daemon
//! sy --use-daemon /tmp/sy.sock /local/path /remote/path
//! ```

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::compress::{compress, is_compressed_extension, Compression};
use crate::delta::{generate_delta_streaming, BlockChecksum as DeltaBlockChecksum};
use crate::server::protocol::{
    delta_block_size, Action, Decision, DeltaOp, FileListEntry, SymlinkEntry, DATA_FLAG_COMPRESSED,
    DELTA_MIN_SIZE,
};
use crate::sync::scanner::{self, ScanOptions};
use crate::sync::SyncStats;
use crate::transport::server::DaemonSession;

/// Minimum size for compression (1MB)
const COMPRESS_MIN_SIZE: u64 = 1024 * 1024;

/// Number of delta checksum requests to pipeline before reading responses
const PIPELINE_DEPTH: usize = 8;

/// Source entry with all info needed for transfer
struct SourceEntry {
    rel_path: String,
    abs_path: Arc<PathBuf>,
    size: u64,
    mtime: i64,
    mode: u32,
    is_dir: bool,
    is_symlink: bool,
    symlink_target: Option<String>,
}

/// Sync from local source to daemon destination (PUSH mode)
///
/// # Arguments
/// * `source` - Local source directory
/// * `socket_path` - Path to Unix socket (local or forwarded from remote)
/// * `remote_path` - Destination path on daemon side
pub async fn sync_daemon_mode(
    source: &Path,
    socket_path: &str,
    remote_path: &Path,
) -> Result<SyncStats> {
    let start = Instant::now();

    // Connect to daemon
    let mut session = DaemonSession::connect(socket_path, remote_path).await?;
    tracing::debug!("Connected to daemon at {}", socket_path);

    // Scan source
    tracing::debug!("Scanning source...");
    let source_entries = scan_source(source).await?;

    // Separate entries by type
    let mut directories: Vec<String> = Vec::new();
    let mut files: Vec<SourceEntry> = Vec::new();
    let mut symlinks: Vec<SourceEntry> = Vec::new();

    for entry in source_entries {
        if entry.is_dir {
            directories.push(entry.rel_path);
        } else if entry.is_symlink {
            symlinks.push(entry);
        } else {
            files.push(entry);
        }
    }

    // Build protocol entries (files only for FILE_LIST comparison)
    let proto_entries: Vec<FileListEntry> = files
        .iter()
        .map(|e| FileListEntry {
            path: e.rel_path.clone(),
            size: e.size,
            mtime: e.mtime,
            mode: e.mode,
            flags: 0,
            symlink_target: None,
        })
        .collect();

    let total_files = proto_entries.len();
    let total_dirs = directories.len();
    let total_symlinks = symlinks.len();

    tracing::info!(
        "Source: {} files, {} dirs, {} symlinks",
        total_files,
        total_dirs,
        total_symlinks
    );

    // Step 1: Create directories (if any)
    let mut dirs_created = 0u64;
    if !directories.is_empty() {
        tracing::debug!("Creating {} directories...", directories.len());
        session.send_mkdir_batch(directories).await?;
        let ack = session.read_mkdir_ack().await?;
        dirs_created = ack.created as u64;
        if !ack.failed.is_empty() {
            for (path, err) in &ack.failed {
                tracing::warn!("Failed to create dir {}: {}", path, err);
            }
        }
    }

    // Step 2: Send file list and get decisions
    tracing::debug!("Sending file list ({} files)...", total_files);
    session.send_file_list(proto_entries).await?;

    tracing::debug!("Waiting for daemon decisions...");
    let ack = session.read_ack().await?;

    // Count files needing transfer
    let files_to_transfer = ack
        .decisions
        .iter()
        .filter(|d| matches!(d.action, Action::Create | Action::Update))
        .count();
    tracing::info!("{} files need transfer", files_to_transfer);

    // Step 3: Transfer files - separate creates and updates
    let mut bytes_transferred = 0u64;
    let mut files_created = 0u64;
    let mut files_updated = 0u64;

    // Categorize by action type
    let creates: Vec<(u32, &SourceEntry)> = ack
        .decisions
        .iter()
        .filter_map(|d| {
            if d.action == Action::Create {
                Some((d.index, &files[d.index as usize]))
            } else {
                None
            }
        })
        .collect();

    let updates: Vec<(u32, &SourceEntry)> = ack
        .decisions
        .iter()
        .filter_map(|d| {
            if d.action == Action::Update {
                Some((d.index, &files[d.index as usize]))
            } else {
                None
            }
        })
        .collect();

    // Step 3a: Handle CREATES with full file transfer (+ compression)
    if !creates.is_empty() {
        tracing::debug!("Transferring {} new files...", creates.len());

        let paths: Vec<(u32, Arc<PathBuf>, String, u64)> = creates
            .iter()
            .map(|(idx, e)| (*idx, e.abs_path.clone(), e.rel_path.clone(), e.size))
            .collect();

        // Read and optionally compress files
        let files_data: Vec<(u32, Vec<u8>, u8)> = tokio::task::spawn_blocking(move || {
            paths
                .into_iter()
                .filter_map(|(idx, path, rel_path, size)| {
                    std::fs::read(&*path).ok().map(|data| {
                        // Compress if file is large enough and not already compressed
                        let (send_data, flags) =
                            if size >= COMPRESS_MIN_SIZE && !is_compressed_extension(&rel_path) {
                                match compress(&data, Compression::Zstd) {
                                    Ok(compressed) if compressed.len() < data.len() => {
                                        (compressed, DATA_FLAG_COMPRESSED)
                                    }
                                    _ => (data, 0),
                                }
                            } else {
                                (data, 0)
                            };
                        (idx, send_data, flags)
                    })
                })
                .collect()
        })
        .await?;

        // Send all creates
        for (idx, data, flags) in &files_data {
            bytes_transferred += data.len() as u64;
            session
                .send_file_data_with_flags(*idx, 0, *flags, data.clone())
                .await?;
        }
        session.flush().await?;

        // Read confirmations
        for _ in &files_data {
            let done = session.read_file_done().await?;
            if done.status != 0 {
                tracing::error!("Create failed: index {} status {}", done.index, done.status);
            } else {
                files_created += 1;
            }
        }
    }

    // Step 3b: Handle UPDATES - use delta sync for large files
    if !updates.is_empty() {
        let (delta_candidates, full_updates): (Vec<_>, Vec<_>) =
            updates.iter().partition(|(_, e)| e.size >= DELTA_MIN_SIZE);

        // Process delta candidates with pipelined checksum requests
        if !delta_candidates.is_empty() {
            tracing::debug!(
                "Delta syncing {} large files (>{}KB) with pipeline depth {}...",
                delta_candidates.len(),
                DELTA_MIN_SIZE / 1024,
                PIPELINE_DEPTH
            );

            // Collect pending requests for batching
            let mut pending: Vec<(u32, &SourceEntry, u32)> = Vec::with_capacity(PIPELINE_DEPTH);

            for (idx, entry) in &delta_candidates {
                let file_idx = *idx;
                let block_size = delta_block_size(entry.size);

                // Send checksum request without waiting (no flush)
                session
                    .send_checksum_req_no_flush(file_idx, block_size)
                    .await?;
                pending.push((file_idx, *entry, block_size));

                // Process batch when full
                if pending.len() >= PIPELINE_DEPTH {
                    session.flush().await?;
                    let (updated, transferred) =
                        process_delta_batch(&mut session, &pending).await?;
                    files_updated += updated;
                    bytes_transferred += transferred;
                    pending.clear();
                }
            }

            // Process remaining files
            if !pending.is_empty() {
                session.flush().await?;
                let (updated, transferred) = process_delta_batch(&mut session, &pending).await?;
                files_updated += updated;
                bytes_transferred += transferred;
            }
        }

        // Process small file updates with full transfer
        if !full_updates.is_empty() {
            tracing::debug!(
                "Updating {} small files with full transfer...",
                full_updates.len()
            );

            let paths: Vec<(u32, Arc<PathBuf>, String, u64)> = full_updates
                .iter()
                .map(|(idx, e)| (*idx, e.abs_path.clone(), e.rel_path.clone(), e.size))
                .collect();

            let files_data: Vec<(u32, Vec<u8>, u8)> = tokio::task::spawn_blocking(move || {
                paths
                    .into_iter()
                    .filter_map(|(idx, path, rel_path, size)| {
                        std::fs::read(&*path).ok().map(|data| {
                            let (send_data, flags) = if size >= COMPRESS_MIN_SIZE
                                && !is_compressed_extension(&rel_path)
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
                            (idx, send_data, flags)
                        })
                    })
                    .collect()
            })
            .await?;

            for (idx, data, flags) in &files_data {
                bytes_transferred += data.len() as u64;
                session
                    .send_file_data_with_flags(*idx, 0, *flags, data.clone())
                    .await?;
            }
            session.flush().await?;

            for _ in &files_data {
                let done = session.read_file_done().await?;
                if done.status != 0 {
                    tracing::error!("Update failed: index {} status {}", done.index, done.status);
                } else {
                    files_updated += 1;
                }
            }
        }
    }

    // Step 4: Create symlinks (if any)
    let mut symlinks_created = 0u64;
    if !symlinks.is_empty() {
        tracing::debug!("Creating {} symlinks...", symlinks.len());

        let entries: Vec<SymlinkEntry> = symlinks
            .iter()
            .filter_map(|e| {
                e.symlink_target.as_ref().map(|target| SymlinkEntry {
                    path: e.rel_path.clone(),
                    target: target.clone(),
                })
            })
            .collect();

        if !entries.is_empty() {
            session.send_symlink_batch(entries).await?;
            let ack = session.read_symlink_ack().await?;
            symlinks_created = ack.created as u64;
            if !ack.failed.is_empty() {
                for (path, err) in &ack.failed {
                    tracing::warn!("Failed to create symlink {}: {}", path, err);
                }
            }
        }
    }

    let duration = start.elapsed();
    let files_skipped = ack
        .decisions
        .iter()
        .filter(|d| d.action == Action::Skip)
        .count();

    tracing::info!(
        "Daemon sync complete: {} created, {} updated, {} skipped in {:?}",
        files_created,
        files_updated,
        files_skipped,
        duration
    );

    // Close session
    session.close().await?;

    Ok(SyncStats {
        files_scanned: total_files as u64,
        files_created,
        files_updated,
        files_deleted: 0,
        files_skipped,
        bytes_transferred,
        files_delta_synced: 0,
        delta_bytes_saved: 0,
        files_compressed: 0,
        compression_bytes_saved: 0,
        files_verified: 0,
        verification_failures: 0,
        duration,
        bytes_would_add: 0,
        bytes_would_change: 0,
        bytes_would_delete: 0,
        dirs_created,
        symlinks_created,
        errors: vec![],
    })
}

/// Sync from daemon source to local destination (PULL mode)
///
/// # Arguments
/// * `socket_path` - Path to Unix socket
/// * `remote_path` - Source path on daemon side
/// * `dest` - Local destination directory
pub async fn sync_pull_daemon_mode(
    socket_path: &str,
    remote_path: &Path,
    dest: &Path,
) -> Result<SyncStats> {
    let start = Instant::now();

    // Connect to daemon in PULL mode
    let mut session = DaemonSession::connect_pull(socket_path, remote_path).await?;
    tracing::debug!("Connected to daemon (PULL mode)");

    // Ensure local destination exists
    if !dest.exists() {
        std::fs::create_dir_all(dest)?;
    }

    // Scan local destination for comparison
    let local_entries = scan_local_dest(dest).await?;
    let local_map: HashMap<String, (u64, i64)> = local_entries
        .into_iter()
        .map(|e| (e.rel_path, (e.size, e.mtime)))
        .collect();

    let mut files_created = 0u64;
    let mut files_updated = 0u64;
    let mut files_skipped = 0usize;
    let mut bytes_transferred = 0u64;
    let mut symlinks_created = 0u64;

    // Step 1: Receive and create directories
    let mkdir_batch = session.read_mkdir_batch().await?;
    tracing::debug!("Received {} directories", mkdir_batch.paths.len());
    let mut dirs_created = 0u64;
    let mut failed: Vec<(String, String)> = Vec::new();

    for dir_path in &mkdir_batch.paths {
        let full_path = dest.join(dir_path);
        match std::fs::create_dir_all(&full_path) {
            Ok(_) => dirs_created += 1,
            Err(e) => failed.push((dir_path.clone(), e.to_string())),
        }
    }
    session
        .send_mkdir_batch_ack(dirs_created as u32, failed)
        .await?;

    // Step 2: Receive file list and send decisions
    let file_list = session.read_file_list().await?;
    tracing::debug!("Received {} files from daemon", file_list.entries.len());

    let mut decisions: Vec<Decision> = Vec::with_capacity(file_list.entries.len());
    let mut files_to_receive: Vec<(u32, String)> = Vec::new();

    for (idx, entry) in file_list.entries.iter().enumerate() {
        let action = if let Some((local_size, local_mtime)) = local_map.get(&entry.path) {
            if *local_size == entry.size && *local_mtime >= entry.mtime {
                Action::Skip
            } else {
                Action::Update
            }
        } else {
            Action::Create
        };

        if action != Action::Skip {
            files_to_receive.push((idx as u32, entry.path.clone()));
        } else {
            files_skipped += 1;
        }

        decisions.push(Decision {
            index: idx as u32,
            action,
        });
    }

    session.send_file_list_ack(decisions).await?;
    tracing::info!("{} files to receive", files_to_receive.len());

    // Step 3: Receive files
    for (idx, rel_path) in &files_to_receive {
        let file_data = match session.read_file_data().await? {
            Some(data) => data,
            None => break, // Got SYMLINK_BATCH instead
        };

        let full_path = dest.join(rel_path);

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file
        std::fs::write(&full_path, &file_data.data)?;
        bytes_transferred += file_data.data.len() as u64;

        // Update stats
        if local_map.contains_key(rel_path) {
            files_updated += 1;
        } else {
            files_created += 1;
        }

        // Send FILE_DONE
        session.send_file_done(*idx, 0).await?;
    }

    // Step 4: Handle symlinks
    if let Ok(symlink_batch) = session.read_symlink_batch_body().await {
        let mut created = 0u32;
        let mut failed: Vec<(String, String)> = Vec::new();

        for entry in &symlink_batch.entries {
            let link_path = dest.join(&entry.path);
            let target = PathBuf::from(&entry.target);

            // Remove existing symlink if present
            let _ = std::fs::remove_file(&link_path);

            match std::os::unix::fs::symlink(&target, &link_path) {
                Ok(_) => {
                    created += 1;
                    symlinks_created += 1;
                }
                Err(e) => {
                    failed.push((entry.path.clone(), e.to_string()));
                }
            }
        }

        session.send_symlink_batch_ack(created, failed).await?;
    }

    let duration = start.elapsed();

    tracing::info!(
        "Daemon pull complete: {} created, {} updated, {} skipped in {:?}",
        files_created,
        files_updated,
        files_skipped,
        duration
    );

    session.close().await?;

    Ok(SyncStats {
        files_scanned: file_list.entries.len() as u64,
        files_created,
        files_updated,
        files_deleted: 0,
        files_skipped,
        bytes_transferred,
        files_delta_synced: 0,
        delta_bytes_saved: 0,
        files_compressed: 0,
        compression_bytes_saved: 0,
        files_verified: 0,
        verification_failures: 0,
        duration,
        bytes_would_add: 0,
        bytes_would_change: 0,
        bytes_would_delete: 0,
        dirs_created,
        symlinks_created,
        errors: vec![],
    })
}

/// Process a batch of delta sync requests
async fn process_delta_batch(
    session: &mut DaemonSession,
    pending: &[(u32, &SourceEntry, u32)],
) -> Result<(u64, u64)> {
    let mut files_updated = 0u64;
    let mut bytes_transferred = 0u64;

    // Step 1: Read all checksum responses (may arrive out of order)
    let mut responses: HashMap<u32, crate::server::protocol::ChecksumResp> =
        HashMap::with_capacity(pending.len());
    for _ in 0..pending.len() {
        let resp = session.read_checksum_resp().await?;
        responses.insert(resp.index, resp);
    }

    // Step 2: Compute all deltas in parallel
    let delta_futures: Vec<_> = pending
        .iter()
        .map(|(file_idx, entry, block_size)| {
            let resp = responses.get(file_idx).cloned();
            let path = entry.abs_path.clone();
            let bs = *block_size as usize;
            let idx = *file_idx;

            async move {
                let resp = resp.ok_or_else(|| {
                    anyhow::anyhow!("Missing checksum response for index {}", idx)
                })?;

                // Convert protocol checksums to delta checksums
                let dest_checksums: Vec<DeltaBlockChecksum> = resp
                    .checksums
                    .iter()
                    .enumerate()
                    .map(|(i, c)| DeltaBlockChecksum {
                        index: i as u64,
                        offset: c.offset,
                        size: c.size as usize,
                        weak: c.weak,
                        strong: c.strong,
                    })
                    .collect();

                // Generate delta in blocking task
                let delta = tokio::task::spawn_blocking(move || {
                    generate_delta_streaming(&path, &dest_checksums, bs)
                })
                .await??;

                // Convert to protocol delta ops
                let mut ops: Vec<DeltaOp> = Vec::with_capacity(delta.ops.len());
                let mut delta_bytes = 0u64;

                for op in &delta.ops {
                    match op {
                        crate::delta::DeltaOp::Copy { offset, size } => {
                            ops.push(DeltaOp::Copy {
                                offset: *offset,
                                size: *size as u32,
                            });
                        }
                        crate::delta::DeltaOp::Data(data) => {
                            delta_bytes += data.len() as u64;
                            ops.push(DeltaOp::Data(data.clone()));
                        }
                    }
                }

                Ok::<_, anyhow::Error>((idx, ops, delta_bytes))
            }
        })
        .collect();

    let deltas: Vec<Result<(u32, Vec<DeltaOp>, u64)>> =
        futures::future::join_all(delta_futures).await;

    // Step 3: Send all DELTA_DATA without waiting for confirmations
    for result in deltas {
        let (idx, ops, delta_bytes) = result?;
        bytes_transferred += delta_bytes;

        session.send_delta_data(idx, 0, ops).await?;
    }

    session.flush().await?;

    // Step 4: Read confirmations
    for _ in pending {
        let done = session.read_file_done().await?;
        if done.status == 0 {
            files_updated += 1;
        }
    }

    Ok((files_updated, bytes_transferred))
}

/// Scan source directory
async fn scan_source(source: &Path) -> Result<Vec<SourceEntry>> {
    let scan_opts = ScanOptions::default();
    let src = source.to_path_buf();

    let entries = tokio::task::spawn_blocking(move || {
        scanner::Scanner::new(&src).with_options(scan_opts).scan()
    })
    .await??;

    let result: Vec<SourceEntry> = entries
        .into_iter()
        .filter_map(|e| {
            e.path.strip_prefix(source).ok().and_then(|rel| {
                if rel.as_os_str().is_empty() {
                    return None;
                }
                rel.to_str().map(|s| {
                    let mtime = e
                        .modified
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    SourceEntry {
                        rel_path: s.to_string(),
                        abs_path: Arc::new(e.path.to_path_buf()),
                        size: e.size,
                        mtime,
                        mode: 0o644,
                        is_dir: e.is_dir,
                        is_symlink: e.is_symlink,
                        symlink_target: e.symlink_target.and_then(|t| t.to_str().map(String::from)),
                    }
                })
            })
        })
        .collect();

    Ok(result)
}

/// Scan local destination for comparison
async fn scan_local_dest(dest: &Path) -> Result<Vec<SourceEntry>> {
    let scan_opts = ScanOptions::default();
    let d = dest.to_path_buf();

    let entries = tokio::task::spawn_blocking(move || {
        scanner::Scanner::new(&d).with_options(scan_opts).scan()
    })
    .await??;

    let result: Vec<SourceEntry> = entries
        .into_iter()
        .filter_map(|e| {
            e.path.strip_prefix(dest).ok().and_then(|rel| {
                if rel.as_os_str().is_empty() || e.is_dir {
                    return None;
                }
                rel.to_str().map(|s| {
                    let mtime = e
                        .modified
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    SourceEntry {
                        rel_path: s.to_string(),
                        abs_path: Arc::new(e.path.to_path_buf()),
                        size: e.size,
                        mtime,
                        mode: 0o644,
                        is_dir: e.is_dir,
                        is_symlink: e.is_symlink,
                        symlink_target: None,
                    }
                })
            })
        })
        .collect();

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_entry_fields() {
        let entry = SourceEntry {
            rel_path: "test.txt".to_string(),
            abs_path: Arc::new(PathBuf::from("/tmp/test.txt")),
            size: 100,
            mtime: 1234567890,
            mode: 0o644,
            is_dir: false,
            is_symlink: false,
            symlink_target: None,
        };

        assert_eq!(entry.rel_path, "test.txt");
        assert_eq!(entry.size, 100);
        assert!(!entry.is_dir);
    }
}
