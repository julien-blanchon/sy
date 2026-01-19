//! CLI support for sypy
//!
//! This module exposes the sy CLI functionality to Python, allowing:
//! - `python -m sypy --server <path>` (server mode for SSH transport)
//! - `python -m sypy --daemon --socket <path>` (daemon mode)
//! - `python -m sypy /source /dest` (sync mode)

use pyo3::prelude::*;
use std::path::PathBuf;

/// Expand tilde (~) in paths
fn expand_tilde(path: &std::path::Path) -> PathBuf {
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

/// Run sy in server mode (used by SSH transport)
///
/// This is equivalent to `sy --server <path>` and is used internally
/// when sy connects via SSH.
#[pyfunction]
pub fn run_server(py: Python<'_>, path: &str) -> PyResult<()> {
    py.allow_threads(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        rt.block_on(async { run_server_with_path(path).await })
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    })
}

/// Internal server runner with explicit path
async fn run_server_with_path(path: &str) -> anyhow::Result<()> {
    use std::sync::Arc;
    use sy::server::handler::ServerHandler;
    use sy::server::protocol::{
        ChecksumReq, ChecksumResp, DeltaData, ErrorMessage, FileListEntry, Hello, MessageType,
        MkdirBatch, SymlinkBatch, PROTOCOL_VERSION,
    };
    use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc;

    let root_path = expand_tilde(std::path::Path::new(path));

    // Ensure root directory exists
    if !root_path.exists() {
        std::fs::create_dir_all(&root_path)?;
    }

    let mut stdin = io::stdin();
    let mut stdout = io::stdout();

    let mut handler = ServerHandler::new(root_path.clone());

    // Handshake
    let _len = stdin.read_u32().await?;
    let type_byte = stdin.read_u8().await?;

    if type_byte != MessageType::Hello as u8 {
        let err = ErrorMessage {
            code: 1,
            message: format!("Expected HELLO (0x01), got 0x{:02X}", type_byte),
        };
        err.write(&mut stdout).await?;
        return Ok(());
    }

    let hello = Hello::read(&mut stdin).await?;

    if hello.version != PROTOCOL_VERSION {
        let err = ErrorMessage {
            code: 1,
            message: format!(
                "Version mismatch: client {}, server {}",
                hello.version, PROTOCOL_VERSION
            ),
        };
        err.write(&mut stdout).await?;
        return Ok(());
    }

    // Send HELLO response
    let resp = Hello {
        version: PROTOCOL_VERSION,
        flags: 0,
        capabilities: vec![],
    };
    resp.write(&mut stdout).await?;
    stdout.flush().await?;

    // Check if client requested PULL mode (server sends files to client)
    use sy::server::protocol::HELLO_FLAG_PULL;
    if hello.flags & HELLO_FLAG_PULL != 0 {
        return sy::server::run_server_pull_mode(&root_path, &mut stdin, &mut stdout).await
            .map_err(|e| anyhow::anyhow!("Pull mode error: {}", e));
    }

    // Shared state for concurrent CHECKSUM_REQ handling
    let mut file_list: Option<Arc<Vec<FileListEntry>>> = None;
    let root_path_arc = Arc::new(root_path);

    // Channel for checksum results
    let (checksum_tx, mut checksum_rx) = mpsc::channel::<ChecksumResp>(32);
    let mut pending_checksum_count = 0usize;

    // Main message loop
    loop {
        tokio::select! {
            biased;

            Some(resp) = checksum_rx.recv(), if pending_checksum_count > 0 => {
                resp.write(&mut stdout).await?;
                pending_checksum_count -= 1;

                if pending_checksum_count == 0 || checksum_rx.is_empty() {
                    stdout.flush().await?;
                }
            }

            len_result = stdin.read_u32() => {
                let _len = match len_result {
                    Ok(len) => len,
                    Err(_) => break,
                };
                let type_byte = stdin.read_u8().await?;

                match MessageType::from_u8(type_byte) {
                    Some(MessageType::FileList) => {
                        drain_pending(&mut checksum_rx, &mut pending_checksum_count, &mut stdout).await?;
                        let list = sy::server::protocol::FileList::read(&mut stdin).await?;
                        file_list = Some(Arc::new(list.entries.clone()));
                        handler.handle_file_list(list, &mut stdout).await?;
                    }

                    Some(MessageType::MkdirBatch) => {
                        drain_pending(&mut checksum_rx, &mut pending_checksum_count, &mut stdout).await?;
                        let batch = MkdirBatch::read(&mut stdin).await?;
                        handler.handle_mkdir_batch(batch, &mut stdout).await?;
                    }

                    Some(MessageType::SymlinkBatch) => {
                        drain_pending(&mut checksum_rx, &mut pending_checksum_count, &mut stdout).await?;
                        let batch = SymlinkBatch::read(&mut stdin).await?;
                        handler.handle_symlink_batch(batch, &mut stdout).await?;
                    }

                    Some(MessageType::FileData) => {
                        drain_pending(&mut checksum_rx, &mut pending_checksum_count, &mut stdout).await?;
                        let data = sy::server::protocol::FileData::read(&mut stdin).await?;
                        handler.handle_file_data(data, &mut stdout).await?;
                    }

                    Some(MessageType::ChecksumReq) => {
                        let req = ChecksumReq::read(&mut stdin).await?;

                        if let Some(ref fl) = file_list {
                            use sy::server::handler::compute_checksum_response;
                            let fl = Arc::clone(fl);
                            let rp = Arc::clone(&root_path_arc);
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
                                        eprintln!("Checksum error: {}", e);
                                    }
                                }
                            });
                        } else {
                            handler.handle_checksum_req(req, &mut stdout).await?;
                        }
                    }

                    Some(MessageType::DeltaData) => {
                        drain_pending(&mut checksum_rx, &mut pending_checksum_count, &mut stdout).await?;
                        let delta = DeltaData::read(&mut stdin).await?;
                        handler.handle_delta_data(delta, &mut stdout).await?;
                    }

                    Some(MessageType::Error) => {
                        let err = sy::server::protocol::ErrorMessage::read(&mut stdin).await?;
                        return Err(anyhow::anyhow!("Remote error: {}", err.message));
                    }

                    _ => {
                        drain_pending(&mut checksum_rx, &mut pending_checksum_count, &mut stdout).await?;
                        let err = ErrorMessage {
                            code: 1,
                            message: format!("Unknown message type: 0x{:02X}", type_byte),
                        };
                        err.write(&mut stdout).await?;
                        stdout.flush().await?;
                        break;
                    }
                }
            }
        }
    }

    drain_pending(&mut checksum_rx, &mut pending_checksum_count, &mut stdout).await?;
    Ok(())
}

/// Drain pending checksum responses
async fn drain_pending<W: tokio::io::AsyncWriteExt + Unpin>(
    rx: &mut tokio::sync::mpsc::Receiver<sy::server::protocol::ChecksumResp>,
    pending_count: &mut usize,
    writer: &mut W,
) -> anyhow::Result<()> {
    while *pending_count > 0 {
        if let Some(resp) = rx.recv().await {
            resp.write(writer).await?;
            *pending_count -= 1;
        } else {
            break;
        }
    }
    if *pending_count == 0 {
        writer.flush().await?;
    }
    Ok(())
}

/// Run sy in daemon mode (persistent server on Unix socket)
///
/// This is equivalent to `sy --daemon --socket <socket_path> [root_path]`
#[pyfunction]
#[pyo3(signature = (socket_path, root_path = None))]
#[cfg(unix)]
pub fn run_daemon(py: Python<'_>, socket_path: &str, root_path: Option<&str>) -> PyResult<()> {
    py.allow_threads(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let root = root_path
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        rt.block_on(async { sy::server::daemon::run_daemon(socket_path, &root).await })
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    })
}

/// Stub for non-Unix platforms
#[pyfunction]
#[pyo3(signature = (socket_path, root_path = None))]
#[cfg(not(unix))]
pub fn run_daemon(_py: Python<'_>, _socket_path: &str, _root_path: Option<&str>) -> PyResult<()> {
    Err(pyo3::exceptions::PyRuntimeError::new_err(
        "Daemon mode is only supported on Unix systems",
    ))
}

/// Run the CLI with given arguments
///
/// This is the main entry point for `python -m sypy` or the `sy` command.
#[pyfunction]
#[pyo3(signature = (args = None))]
pub fn main(py: Python<'_>, args: Option<Vec<String>>) -> PyResult<i32> {
    let args = args.unwrap_or_else(|| std::env::args().collect());

    // Parse arguments manually for the subset we support
    let mut iter = args.iter().skip(1); // Skip program name
    let mut server_mode = false;
    let mut daemon_mode = false;
    let mut socket_path: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut show_help = false;
    let mut show_version = false;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--server" => server_mode = true,
            "--daemon" => daemon_mode = true,
            "--socket" => {
                socket_path = iter.next().cloned();
            }
            "-h" | "--help" => show_help = true,
            "-V" | "--version" => show_version = true,
            _ if arg.starts_with('-') => {
                // Skip unknown flags for now
            }
            _ => positional.push(arg.clone()),
        }
    }

    if show_version {
        println!("sy-python {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }

    if show_help {
        print_help();
        return Ok(0);
    }

    // Handle server mode
    if server_mode {
        let path = positional.first().map(|s| s.as_str()).unwrap_or(".");
        return run_server(py, path).map(|_| 0);
    }

    // Handle daemon mode
    if daemon_mode {
        let socket = socket_path.as_deref().unwrap_or("~/.sy/daemon.sock");
        let root = positional.first().map(|s| s.as_str());
        return run_daemon(py, socket, root).map(|_| 0);
    }

    // Handle sync mode (default)
    if positional.len() >= 2 {
        let source = &positional[0];
        let dest = &positional[1];

        // Parse dry_run flag
        let dry_run = args.iter().any(|arg| arg == "--dry-run" || arg == "-n");

        // Use the sync function with all default values
        let result = crate::sync::sync(
            py,
            source,
            dest,
            dry_run,
            false,  // delete
            50,     // delete_threshold
            10,     // parallel
            false,  // verify
            false,  // compress
            false,  // checksum
            vec![], // exclude
            vec![], // include
            None,   // min_size
            None,   // max_size
            None,   // bwlimit
            None,   // progress_callback
            1000,   // progress_frequency_ms
            false,  // daemon_auto
            true,   // resume
            false,  // ignore_times
            false,  // size_only
            false,  // update
            false,  // ignore_existing
            false,  // gitignore
            false,  // exclude_vcs
            false,  // preserve_xattrs
            false,  // preserve_hardlinks
            false,  // preserve_permissions
            false,  // preserve_times
            3,      // retry
            1,      // retry_delay
            None,   // s3
            None,   // gcs
            None,   // ssh
        )?;

        // Format output for dry-run
        if dry_run && result.success() {
            println!("\n✓ Dry-run complete (no changes made)\n");
            println!("  Would create:  {} files", result.files_created);
            println!("  Would update:  {} files", result.files_updated);
            if result.bytes_would_add > 0 || result.bytes_would_change > 0 {
                println!();
                if result.bytes_would_add > 0 {
                    println!("  Data to add:     {} bytes", result.bytes_would_add);
                }
                if result.bytes_would_change > 0 {
                    println!("  Data to change:  {} bytes", result.bytes_would_change);
                }
            }
        }

        if result.success() {
            Ok(0)
        } else {
            Ok(1)
        }
    } else if positional.is_empty() {
        print_help();
        Ok(0)
    } else {
        eprintln!("Error: Both source and destination are required");
        eprintln!("Usage: sy <source> <destination>");
        Ok(1)
    }
}

fn print_help() {
    println!(
        r#"sy-python {} - Modern file synchronization tool

USAGE:
    sy [OPTIONS] <source> <destination>
    sy --server <path>
    sy --daemon [--socket <path>] [root_path]

MODES:
    (default)       Sync files from source to destination
    --server        Run as server (used by SSH transport)
    --daemon        Run as persistent daemon on Unix socket

OPTIONS:
    -h, --help      Show this help message
    -V, --version   Show version

EXAMPLES:
    # Basic sync
    sy /source /destination

    # Server mode (internal, used by SSH transport)
    sy --server /path/to/serve

    # Daemon mode
    sy --daemon --socket ~/.sy/daemon.sock

For full options, use the Python API:
    import sypy
    stats = sypy.sync("/source", "/dest", dry_run=True, parallel=20)
"#,
        env!("CARGO_PKG_VERSION")
    );
}
