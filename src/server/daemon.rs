//! Daemon mode for sy - persistent server listening on Unix socket
//!
//! This eliminates SSH connection + server startup overhead (~2s) for repeated syncs.
//!
//! # Usage
//!
//! ## Start daemon on remote machine:
//! ```bash
//! sy --daemon --socket ~/.sy/daemon.sock
//! # Or with explicit root path (optional):
//! sy --daemon --socket ~/.sy/daemon.sock /path/to/sync/root
//! ```
//!
//! ## Connect via SSH socket forwarding:
//! ```bash
//! # Forward local socket to remote daemon
//! ssh -L /tmp/sy-local.sock:~/.sy/daemon.sock user@host -N &
//!
//! # Then sync using the daemon
//! sy --use-daemon /tmp/sy-local.sock /local/path daemon:/remote/path
//! ```
//!
//! # Protocol
//!
//! The daemon uses the same protocol as `sy --server`, with one addition:
//! - After HELLO, client sends SET_ROOT to specify the working directory
//! - Paths in subsequent messages are relative to this root

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::handler::{compute_checksum_response, ServerHandler};
use super::protocol::{
    ChecksumReq, ChecksumResp, DeltaData, ErrorMessage, Hello, MessageType, MkdirBatch,
    SymlinkBatch, PROTOCOL_VERSION,
};
use crate::sync::scanner::{self, ScanOptions};

/// Message types specific to daemon mode
pub const MSG_SET_ROOT: u8 = 0x30;
pub const MSG_SET_ROOT_ACK: u8 = 0x31;
pub const MSG_PING: u8 = 0x32;
pub const MSG_PONG: u8 = 0x33;

/// Expand tilde (~) in paths to the user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();

    if path_str == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    } else if let Some(rest) = path_str.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(rest)
        } else {
            path.to_path_buf()
        }
    } else {
        path.to_path_buf()
    }
}

/// Expand tilde in socket path string
fn expand_socket_path(socket_path: &str) -> PathBuf {
    expand_tilde(Path::new(socket_path))
}

/// Run the daemon server
///
/// # Arguments
/// * `socket_path` - Path to the Unix socket (supports ~ expansion)
/// * `default_root` - Default root path for file operations
pub async fn run_daemon(socket_path: &str, default_root: &Path) -> Result<()> {
    let socket_path = expand_socket_path(socket_path);
    let default_root = expand_tilde(default_root);

    // Ensure socket directory exists
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create socket directory: {}", parent.display()))?;
    }

    // Remove existing socket file if present
    if socket_path.exists() {
        tokio::fs::remove_file(&socket_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to remove existing socket: {}",
                    socket_path.display()
                )
            })?;
    }

    // Bind Unix socket
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind socket: {}", socket_path.display()))?;

    info!("Daemon listening on {}", socket_path.display());
    info!("Default root: {}", default_root.display());

    // Set up signal handler for graceful shutdown
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

    // Handle SIGTERM and SIGINT
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to register SIGTERM handler");
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("Failed to register SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down...");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT, shutting down...");
            }
        }
        let _ = shutdown_tx_clone.send(()).await;
    });

    // Track active connections
    let active_connections = Arc::new(tokio::sync::Semaphore::new(100)); // Max 100 concurrent connections

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _addr)) => {
                        let permit = match active_connections.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                warn!("Max connections reached, rejecting client");
                                continue;
                            }
                        };

                        let default_root = default_root.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, default_root).await {
                                // Don't log EOF as error - it's normal when client disconnects
                                if !e.to_string().contains("unexpected eof") {
                                    error!("Client error: {}", e);
                                }
                            }
                            drop(permit); // Release connection slot
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {}", e);
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Shutting down daemon...");
                break;
            }
        }
    }

    // Clean up socket file
    let _ = tokio::fs::remove_file(&socket_path).await;
    info!("Daemon stopped");

    Ok(())
}

/// Handle a single client connection
async fn handle_client(stream: UnixStream, default_root: PathBuf) -> Result<()> {
    let (mut reader, mut writer) = stream.into_split();

    // Handshake
    let _len = reader.read_u32().await?;
    let type_byte = reader.read_u8().await?;

    if type_byte != MessageType::Hello as u8 {
        let err = ErrorMessage {
            code: 1,
            message: format!("Expected HELLO (0x01), got 0x{:02X}", type_byte),
        };
        err.write(&mut writer).await?;
        return Ok(());
    }

    let hello = Hello::read(&mut reader).await?;

    if hello.version != PROTOCOL_VERSION {
        let err = ErrorMessage {
            code: 1,
            message: format!(
                "Version mismatch: client {}, server {}",
                hello.version, PROTOCOL_VERSION
            ),
        };
        err.write(&mut writer).await?;
        return Ok(());
    }

    // Send HELLO response
    let resp = Hello {
        version: PROTOCOL_VERSION,
        flags: 0,
        capabilities: vec![],
    };
    resp.write(&mut writer).await?;
    writer.flush().await?;

    // Wait for SET_ROOT or use default
    let root_path = match read_set_root(&mut reader, &mut writer, &default_root).await? {
        Some(path) => path,
        None => default_root,
    };

    // Ensure root directory exists
    if !root_path.exists() {
        tokio::fs::create_dir_all(&root_path).await?;
    }

    info!("Client connected with root: {}", root_path.display());

    // Check if client requested PULL mode
    if hello.flags & super::protocol::HELLO_FLAG_PULL != 0 {
        return run_daemon_pull_mode(&root_path, &mut reader, &mut writer).await;
    }

    // Handle messages using the standard handler
    let mut handler = ServerHandler::new(root_path.clone());

    // Shared state for concurrent CHECKSUM_REQ handling
    let mut file_list: Option<Arc<Vec<super::protocol::FileListEntry>>> = None;
    let root_path = Arc::new(root_path);

    // Channel for checksum results
    let (checksum_tx, mut checksum_rx) = mpsc::channel::<ChecksumResp>(32);
    let mut pending_checksum_count = 0usize;

    // Main message loop
    loop {
        tokio::select! {
            biased;

            // Prioritize writing completed checksum responses
            Some(resp) = checksum_rx.recv(), if pending_checksum_count > 0 => {
                resp.write(&mut writer).await?;
                pending_checksum_count -= 1;

                if pending_checksum_count == 0 || checksum_rx.is_empty() {
                    writer.flush().await?;
                }
            }

            // Read and handle incoming messages
            len_result = reader.read_u32() => {
                let _len = match len_result {
                    Ok(len) => len,
                    Err(_) => break, // EOF or error, exit loop
                };
                let type_byte = reader.read_u8().await?;

                match type_byte {
                    // PING - keepalive
                    b if b == MSG_PING => {
                        writer.write_u32(0).await?;
                        writer.write_u8(MSG_PONG).await?;
                        writer.flush().await?;
                    }

                    // Standard protocol messages
                    b if b == MessageType::FileList as u8 => {
                        drain_pending_checksums(&mut checksum_rx, &mut pending_checksum_count, &mut writer).await?;
                        let list = super::protocol::FileList::read(&mut reader).await?;
                        file_list = Some(Arc::new(list.entries.clone()));
                        handler.handle_file_list(list, &mut writer).await?;
                    }

                    b if b == MessageType::MkdirBatch as u8 => {
                        drain_pending_checksums(&mut checksum_rx, &mut pending_checksum_count, &mut writer).await?;
                        let batch = MkdirBatch::read(&mut reader).await?;
                        handler.handle_mkdir_batch(batch, &mut writer).await?;
                    }

                    b if b == MessageType::SymlinkBatch as u8 => {
                        drain_pending_checksums(&mut checksum_rx, &mut pending_checksum_count, &mut writer).await?;
                        let batch = SymlinkBatch::read(&mut reader).await?;
                        handler.handle_symlink_batch(batch, &mut writer).await?;
                    }

                    b if b == MessageType::FileData as u8 => {
                        drain_pending_checksums(&mut checksum_rx, &mut pending_checksum_count, &mut writer).await?;
                        let data = super::protocol::FileData::read(&mut reader).await?;
                        handler.handle_file_data(data, &mut writer).await?;
                    }

                    b if b == MessageType::ChecksumReq as u8 => {
                        let req = ChecksumReq::read(&mut reader).await?;

                        if let Some(ref fl) = file_list {
                            let fl = Arc::clone(fl);
                            let rp = Arc::clone(&root_path);
                            let index = req.index;
                            let block_size = req.block_size as usize;
                            let tx = checksum_tx.clone();

                            pending_checksum_count += 1;
                            tokio::spawn(async move {
                                match compute_checksum_response(index, block_size, &fl, &rp).await {
                                    Ok(resp) => {
                                        let _ = tx.send(resp).await;
                                    }
                                    Err(e) => {
                                        tracing::error!("Checksum computation failed: {}", e);
                                    }
                                }
                            });
                        } else {
                            handler.handle_checksum_req(req, &mut writer).await?;
                        }
                    }

                    b if b == MessageType::DeltaData as u8 => {
                        drain_pending_checksums(&mut checksum_rx, &mut pending_checksum_count, &mut writer).await?;
                        let delta = DeltaData::read(&mut reader).await?;
                        handler.handle_delta_data(delta, &mut writer).await?;
                    }

                    b if b == MessageType::Error as u8 => {
                        let err = super::protocol::ErrorMessage::read(&mut reader).await?;
                        error!("Received error from client: {}", err.message);
                        break;
                    }

                    _ => {
                        drain_pending_checksums(&mut checksum_rx, &mut pending_checksum_count, &mut writer).await?;
                        warn!("Unknown message type: 0x{:02X}", type_byte);
                        let err = ErrorMessage {
                            code: 1,
                            message: format!("Unknown message type: 0x{:02X}", type_byte),
                        };
                        err.write(&mut writer).await?;
                        writer.flush().await?;
                        break;
                    }
                }
            }
        }
    }

    // Drain any remaining pending checksums
    drain_pending_checksums(&mut checksum_rx, &mut pending_checksum_count, &mut writer).await?;

    info!("Client disconnected");
    Ok(())
}

/// Read SET_ROOT message if present, or return None to use default
async fn read_set_root<R, W>(
    reader: &mut R,
    writer: &mut W,
    default_root: &Path,
) -> Result<Option<PathBuf>>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // Peek at next message - if it's SET_ROOT, process it
    let _len = reader.read_u32().await?;
    let type_byte = reader.read_u8().await?;

    if type_byte == MSG_SET_ROOT {
        // Read path length and path
        let path_len = reader.read_u16().await? as usize;
        let mut path_buf = vec![0u8; path_len];
        reader.read_exact(&mut path_buf).await?;
        let path_str =
            String::from_utf8(path_buf).with_context(|| "Invalid UTF-8 in SET_ROOT path")?;

        let root_path = expand_tilde(Path::new(&path_str));

        // Validate path exists or can be created
        let success = root_path.exists() || tokio::fs::create_dir_all(&root_path).await.is_ok();

        // Send SET_ROOT_ACK
        writer.write_u32(1).await?;
        writer.write_u8(MSG_SET_ROOT_ACK).await?;
        writer.write_u8(if success { 0 } else { 1 }).await?;
        writer.flush().await?;

        if success {
            Ok(Some(root_path))
        } else {
            Err(anyhow::anyhow!("Failed to access root path: {}", path_str))
        }
    } else {
        // Not SET_ROOT - this is unexpected in daemon mode
        // Client should send SET_ROOT first, but we'll continue with default
        // Push back the message type for normal processing
        // Actually, we've already consumed the length and type, so we need to handle this differently
        // For now, send an error asking for SET_ROOT
        let err = ErrorMessage {
            code: 2,
            message: format!(
                "Expected SET_ROOT (0x30) first, got 0x{:02X}. Using default root: {}",
                type_byte,
                default_root.display()
            ),
        };
        err.write(writer).await?;
        writer.flush().await?;

        // Return None to indicate we couldn't read SET_ROOT properly
        // The caller will need to handle the already-read message
        Err(anyhow::anyhow!(
            "Client didn't send SET_ROOT, got 0x{:02X}. Protocol requires SET_ROOT after HELLO in daemon mode.",
            type_byte
        ))
    }
}

/// Drain all pending checksum responses from the channel
async fn drain_pending_checksums<W: AsyncWriteExt + Unpin>(
    rx: &mut mpsc::Receiver<ChecksumResp>,
    pending_count: &mut usize,
    writer: &mut W,
) -> Result<()> {
    if *pending_count == 0 {
        return Ok(());
    }

    while *pending_count > 0 {
        if let Some(resp) = rx.recv().await {
            resp.write(writer).await?;
            *pending_count -= 1;
        } else {
            break;
        }
    }

    writer.flush().await?;
    Ok(())
}

/// PULL mode for daemon: Server scans source and sends files to client
async fn run_daemon_pull_mode<R, W>(root_path: &Path, reader: &mut R, writer: &mut W) -> Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    use super::protocol::{
        FileData, FileList, FileListEntry, MkdirBatchAck, SymlinkBatch, SymlinkBatchAck,
        SymlinkEntry,
    };

    // Scan source directory
    let scan_opts = ScanOptions::default();
    let root = root_path.to_path_buf();
    let entries = tokio::task::spawn_blocking(move || {
        scanner::Scanner::new(&root).with_options(scan_opts).scan()
    })
    .await??;

    // Separate entries by type
    let mut directories: Vec<String> = Vec::new();
    let mut files: Vec<(String, PathBuf, u64, i64, u32)> = Vec::new();
    let mut symlinks: Vec<SymlinkEntry> = Vec::new();

    // Check if root_path is a single file (not a directory)
    let is_single_file = root_path.is_file();

    for entry in entries {
        // For single file sources, don't strip prefix - use just the filename
        let rel_path_str = if is_single_file {
            // Single file: use just the filename as the relative path
            root_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        } else {
            // Directory: strip the root prefix to get relative path
            entry
                .path
                .strip_prefix(root_path)
                .ok()
                .filter(|p| !p.as_os_str().is_empty()) // Skip root directory entry
                .and_then(|p| p.to_str())
                .map(|s| s.to_string())
        };

        if let Some(path_str) = rel_path_str {
            let mtime = entry
                .modified
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            if entry.is_dir {
                directories.push(path_str.to_string());
            } else if entry.is_symlink {
                if let Some(target) = entry.symlink_target {
                    if let Some(target_str) = target.to_str() {
                        symlinks.push(SymlinkEntry {
                            path: path_str.to_string(),
                            target: target_str.to_string(),
                        });
                    }
                }
            } else {
                files.push((
                    path_str.to_string(),
                    entry.path.to_path_buf(),
                    entry.size,
                    mtime,
                    0o644,
                ));
            }
        }
    }

    // Step 1: Send directories (MKDIR_BATCH)
    let batch = MkdirBatch {
        paths: directories.clone(),
    };
    batch.write(writer).await?;
    writer.flush().await?;

    // Wait for MKDIR_BATCH_ACK
    let _len = reader.read_u32().await?;
    let type_byte = reader.read_u8().await?;
    if type_byte != MessageType::MkdirBatchAck as u8 {
        return Err(anyhow::anyhow!(
            "Expected MKDIR_BATCH_ACK, got 0x{:02X}",
            type_byte
        ));
    }
    let _ack = MkdirBatchAck::read(reader).await?;

    // Step 2: Send file list
    let file_entries: Vec<FileListEntry> = files
        .iter()
        .map(|(rel_path, _, size, mtime, mode)| FileListEntry {
            path: rel_path.clone(),
            size: *size,
            mtime: *mtime,
            mode: *mode,
            flags: 0,
            symlink_target: None,
        })
        .collect();

    let file_list = FileList {
        entries: file_entries,
    };
    file_list.write(writer).await?;
    writer.flush().await?;

    // Wait for FILE_LIST_ACK
    let _len = reader.read_u32().await?;
    let type_byte = reader.read_u8().await?;
    if type_byte != MessageType::FileListAck as u8 {
        return Err(anyhow::anyhow!(
            "Expected FILE_LIST_ACK, got 0x{:02X}",
            type_byte
        ));
    }
    let ack = super::protocol::FileListAck::read(reader).await?;

    // Step 3: Send files that client requested (pipelined - send all, then collect ACKs)
    let mut files_sent: Vec<u32> = Vec::new();

    for decision in &ack.decisions {
        if decision.action == super::protocol::Action::Skip {
            continue;
        }

        let idx = decision.index as usize;
        if idx >= files.len() {
            continue;
        }

        let (_, abs_path, _, _, _) = &files[idx];

        // Read file data (use spawn_blocking for async compatibility)
        let abs_path_clone = abs_path.clone();
        let data = match tokio::task::spawn_blocking(move || std::fs::read(&abs_path_clone)).await {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => {
                warn!("Failed to read {}: {}", abs_path.display(), e);
                continue;
            }
            Err(e) => {
                warn!("Task join error reading {}: {}", abs_path.display(), e);
                continue;
            }
        };

        let file_data = FileData {
            index: decision.index,
            offset: 0,
            flags: 0,
            data,
        };
        file_data.write(writer).await?;
        files_sent.push(decision.index);
    }

    // Flush once after sending all files
    writer.flush().await?;

    // Collect all FILE_DONE responses
    for _ in &files_sent {
        let _len = reader.read_u32().await?;
        let type_byte = reader.read_u8().await?;
        if type_byte != MessageType::FileDone as u8 {
            return Err(anyhow::anyhow!(
                "Expected FILE_DONE, got 0x{:02X}",
                type_byte
            ));
        }
        let _done = super::protocol::FileDone::read(reader).await?;
    }

    // Step 4: Send symlinks
    if !symlinks.is_empty() {
        let batch = SymlinkBatch {
            entries: symlinks.clone(),
        };
        batch.write(writer).await?;
        writer.flush().await?;

        // Wait for SYMLINK_BATCH_ACK
        let _len = reader.read_u32().await?;
        let type_byte = reader.read_u8().await?;
        if type_byte != MessageType::SymlinkBatchAck as u8 {
            return Err(anyhow::anyhow!(
                "Expected SYMLINK_BATCH_ACK, got 0x{:02X}",
                type_byte
            ));
        }
        let _ack = SymlinkBatchAck::read(reader).await?;
    }

    Ok(())
}

/// Write SET_ROOT message to daemon
pub async fn write_set_root<W: AsyncWriteExt + Unpin>(writer: &mut W, path: &str) -> Result<()> {
    let path_bytes = path.as_bytes();
    let len = 2 + path_bytes.len();

    writer.write_u32(len as u32).await?;
    writer.write_u8(MSG_SET_ROOT).await?;
    writer.write_u16(path_bytes.len() as u16).await?;
    writer.write_all(path_bytes).await?;
    writer.flush().await?;

    Ok(())
}

/// Read SET_ROOT_ACK from daemon
pub async fn read_set_root_ack<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<bool> {
    let _len = reader.read_u32().await?;
    let type_byte = reader.read_u8().await?;

    if type_byte == MessageType::Error as u8 {
        let err = super::protocol::ErrorMessage::read(reader).await?;
        return Err(anyhow::anyhow!("Daemon error: {}", err.message));
    }

    if type_byte != MSG_SET_ROOT_ACK {
        return Err(anyhow::anyhow!(
            "Expected SET_ROOT_ACK (0x31), got 0x{:02X}",
            type_byte
        ));
    }

    let status = reader.read_u8().await?;
    Ok(status == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn test_expand_socket_path() {
        let home = dirs::home_dir().unwrap();

        let path = expand_socket_path("~/.sy/daemon.sock");
        assert_eq!(path, home.join(".sy/daemon.sock"));

        let path = expand_socket_path("/tmp/sy.sock");
        assert_eq!(path, PathBuf::from("/tmp/sy.sock"));
    }

    #[tokio::test]
    async fn test_write_set_root() {
        let mut buf = Vec::new();
        write_set_root(&mut buf, "/home/user/sync").await.unwrap();

        // Verify structure: len(4) + type(1) + path_len(2) + path
        assert_eq!(buf.len(), 4 + 1 + 2 + "/home/user/sync".len());
        assert_eq!(buf[4], MSG_SET_ROOT);
    }

    #[tokio::test]
    async fn test_read_set_root_ack_success() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_be_bytes()); // len
        buf.push(MSG_SET_ROOT_ACK);
        buf.push(0); // success

        let mut cursor = Cursor::new(buf);
        let result = read_set_root_ack(&mut cursor).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_read_set_root_ack_failure() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_be_bytes()); // len
        buf.push(MSG_SET_ROOT_ACK);
        buf.push(1); // failure

        let mut cursor = Cursor::new(buf);
        let result = read_set_root_ack(&mut cursor).await.unwrap();
        assert!(!result);
    }
}
