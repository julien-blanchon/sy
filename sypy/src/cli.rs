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
        return sy::server::run_server_pull_mode(&root_path, &mut stdin, &mut stdout)
            .await
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

/// Run sy-remote CLI with given arguments
///
/// This is the entry point for the `sy-remote` command, which is used by SSH transport
/// to execute operations on remote hosts.
#[pyfunction]
#[pyo3(signature = (args = None))]
pub fn remote_main(_py: Python<'_>, args: Option<Vec<String>>) -> PyResult<i32> {
    let args = args.unwrap_or_else(|| std::env::args().collect());

    // Parse arguments manually
    let mut iter = args.iter().skip(1); // Skip program name
    let mut show_help = false;
    let mut show_version = false;

    let command = iter.next().map(|s| s.as_str());

    if command == Some("-h") || command == Some("--help") {
        show_help = true;
    }
    if command == Some("-V") || command == Some("--version") {
        show_version = true;
    }

    if show_version {
        println!("sy-remote {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }

    if show_help || command.is_none() {
        print_remote_help();
        return Ok(0);
    }

    let remaining_args: Vec<String> = iter.cloned().collect();

    match command.unwrap() {
        "scan" => remote_scan(&remaining_args),
        "checksums" => remote_checksums(&remaining_args),
        "file-checksum" => remote_file_checksum(&remaining_args),
        "apply-delta" => remote_apply_delta(&remaining_args),
        "receive-file" => remote_receive_file(&remaining_args),
        "receive-sparse-file" => remote_receive_sparse_file(&remaining_args),
        _ => {
            eprintln!("Unknown command: {}", command.unwrap());
            print_remote_help();
            Ok(1)
        }
    }
}

fn print_remote_help() {
    println!(
        r#"sy-remote {} - Remote helper for sy (executes on remote hosts via SSH)

USAGE:
    sy-remote <COMMAND> [OPTIONS]

COMMANDS:
    scan <path>                     Scan directory and output file list as JSON
    checksums <path> --block-size N Compute block checksums for delta sync
    file-checksum <path>            Compute file checksum for verification
    apply-delta <base> <output>     Apply delta operations (reads from stdin)
    receive-file <output>           Receive file from stdin (may be compressed)
    receive-sparse-file <output>    Receive sparse file with data regions

OPTIONS:
    -h, --help      Show this help message
    -V, --version   Show version

EXAMPLES:
    sy-remote scan /path/to/dir
    sy-remote checksums /path/to/file --block-size 4096
    sy-remote file-checksum /path/to/file --checksum-type fast
"#,
        env!("CARGO_PKG_VERSION")
    );
}

fn remote_scan(args: &[String]) -> PyResult<i32> {
    use sy::sync::scanner::Scanner;

    let mut path: Option<&str> = None;
    let mut no_git_ignore = false;
    let mut include_git = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--no-git-ignore" => no_git_ignore = true,
            "--include-git" => include_git = true,
            _ if !arg.starts_with('-') => path = Some(arg.as_str()),
            _ => {}
        }
    }

    let path = path.ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err("Path argument required for scan command")
    })?;

    let scanner = Scanner::new(std::path::Path::new(path))
        .respect_gitignore(!no_git_ignore)
        .include_git_dir(include_git);

    let entries = scanner
        .scan()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    #[derive(serde::Serialize)]
    struct ScanOutput {
        entries: Vec<FileEntryJson>,
    }

    #[derive(serde::Serialize)]
    struct FileEntryJson {
        path: String,
        size: u64,
        mtime: i64,
        is_dir: bool,
        is_symlink: bool,
        symlink_target: Option<String>,
        is_sparse: bool,
        allocated_size: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        xattrs: Option<Vec<(String, String)>>,
        inode: Option<u64>,
        nlink: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        acls: Option<String>,
    }

    let json_entries: Vec<FileEntryJson> = entries
        .into_iter()
        .map(|e| {
            let mtime = e
                .modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            // Encode xattrs to base64 for transport
            let xattrs = e.xattrs.map(|xattrs_map| {
                use base64::{engine::general_purpose, Engine as _};
                xattrs_map
                    .into_iter()
                    .map(|(key, value)| {
                        let encoded = general_purpose::STANDARD.encode(&value);
                        (key, encoded)
                    })
                    .collect()
            });

            // Convert ACLs from bytes to string
            let acls = e
                .acls
                .and_then(|acl_bytes| String::from_utf8(acl_bytes).ok());

            FileEntryJson {
                path: e.path.to_string_lossy().to_string(),
                size: e.size,
                mtime,
                is_dir: e.is_dir,
                is_symlink: e.is_symlink,
                symlink_target: e.symlink_target.map(|p| p.to_string_lossy().to_string()),
                is_sparse: e.is_sparse,
                allocated_size: e.allocated_size,
                xattrs,
                inode: e.inode,
                nlink: e.nlink,
                acls,
            }
        })
        .collect();

    let output = ScanOutput {
        entries: json_entries,
    };

    println!(
        "{}",
        serde_json::to_string(&output)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
    );

    Ok(0)
}

fn remote_checksums(args: &[String]) -> PyResult<i32> {
    use sy::delta::compute_checksums;

    let mut path: Option<&str> = None;
    let mut block_size: Option<usize> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--block-size" => {
                block_size = iter.next().and_then(|s| s.parse().ok());
            }
            _ if !arg.starts_with('-') => path = Some(arg.as_str()),
            _ => {}
        }
    }

    let path = path.ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err("Path argument required for checksums command")
    })?;
    let block_size = block_size
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("--block-size argument required"))?;

    let checksums = compute_checksums(std::path::Path::new(path), block_size)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    println!(
        "{}",
        serde_json::to_string(&checksums)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
    );

    Ok(0)
}

fn remote_file_checksum(args: &[String]) -> PyResult<i32> {
    use sy::integrity::{ChecksumType, IntegrityVerifier};

    let mut path: Option<&str> = None;
    let mut checksum_type = "fast".to_string();

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--checksum-type" => {
                if let Some(t) = iter.next() {
                    checksum_type = t.clone();
                }
            }
            _ if !arg.starts_with('-') => path = Some(arg.as_str()),
            _ => {}
        }
    }

    let path = path.ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err("Path argument required for file-checksum command")
    })?;

    let csum_type = match checksum_type.as_str() {
        "fast" => ChecksumType::Fast,
        "cryptographic" => ChecksumType::Cryptographic,
        _ => {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "Invalid checksum type: {}. Use 'fast' or 'cryptographic'",
                checksum_type
            )))
        }
    };

    let verifier = IntegrityVerifier::new(csum_type, false);
    let checksum = verifier
        .compute_file_checksum(std::path::Path::new(path))
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    println!("{}", checksum.to_hex());

    Ok(0)
}

fn remote_apply_delta(args: &[String]) -> PyResult<i32> {
    use std::io::Read;
    use sy::compress::{decompress, Compression};
    use sy::delta::{apply_delta, Delta};

    if args.len() < 2 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "apply-delta requires <base_file> <output_file> arguments",
        ));
    }

    let base_file = std::path::Path::new(&args[0]);
    let output_file = std::path::Path::new(&args[1]);

    // Read delta data from stdin
    let mut stdin_data = Vec::new();
    std::io::stdin()
        .read_to_end(&mut stdin_data)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    // Check if data is compressed (Zstd magic: 0x28, 0xB5, 0x2F, 0xFD)
    let delta_json = if stdin_data.len() >= 4
        && stdin_data[0] == 0x28
        && stdin_data[1] == 0xB5
        && stdin_data[2] == 0x2F
        && stdin_data[3] == 0xFD
    {
        let decompressed = decompress(&stdin_data, Compression::Zstd)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        String::from_utf8(decompressed)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
    } else {
        String::from_utf8(stdin_data)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
    };

    let delta: Delta = serde_json::from_str(&delta_json)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    let stats = apply_delta(base_file, &delta, output_file)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    println!(
        "{{\"operations_count\": {}, \"literal_bytes\": {}}}",
        stats.operations_count, stats.literal_bytes
    );

    Ok(0)
}

fn remote_receive_file(args: &[String]) -> PyResult<i32> {
    use std::io::{Read, Write};
    use sy::compress::{decompress, Compression};

    let mut output_path: Option<&str> = None;
    let mut mtime: Option<u64> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--mtime" => {
                mtime = iter.next().and_then(|s| s.parse().ok());
            }
            _ if !arg.starts_with('-') => output_path = Some(arg.as_str()),
            _ => {}
        }
    }

    let output_path = output_path.ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err("Output path required for receive-file command")
    })?;
    let output_path = std::path::Path::new(output_path);

    // Read file data from stdin
    let mut stdin_data = Vec::new();
    std::io::stdin()
        .read_to_end(&mut stdin_data)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    // Check if data is compressed (Zstd magic)
    let file_data = if stdin_data.len() >= 4
        && stdin_data[0] == 0x28
        && stdin_data[1] == 0xB5
        && stdin_data[2] == 0x2F
        && stdin_data[3] == 0xFD
    {
        decompress(&stdin_data, Compression::Zstd)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
    } else {
        stdin_data
    };

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    }

    // Write file
    let mut output_file = std::fs::File::create(output_path)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    output_file
        .write_all(&file_data)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    output_file
        .flush()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    // Set mtime if provided
    if let Some(mtime_secs) = mtime {
        use std::time::{Duration, UNIX_EPOCH};
        let mtime = UNIX_EPOCH + Duration::from_secs(mtime_secs);
        let _ = filetime::set_file_mtime(output_path, filetime::FileTime::from_system_time(mtime));
    }

    println!("{{\"bytes_written\": {}}}", file_data.len());

    Ok(0)
}

fn remote_receive_sparse_file(args: &[String]) -> PyResult<i32> {
    use std::io::{Read, Seek, SeekFrom, Write};
    use sy::sparse::DataRegion;

    let mut output_path: Option<&str> = None;
    let mut total_size: Option<u64> = None;
    let mut regions_json: Option<String> = None;
    let mut mtime: Option<u64> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--total-size" => {
                total_size = iter.next().and_then(|s| s.parse().ok());
            }
            "--regions" => {
                regions_json = iter.next().cloned();
            }
            "--mtime" => {
                mtime = iter.next().and_then(|s| s.parse().ok());
            }
            _ if !arg.starts_with('-') => output_path = Some(arg.as_str()),
            _ => {}
        }
    }

    let output_path = output_path.ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err(
            "Output path required for receive-sparse-file command",
        )
    })?;
    let total_size = total_size
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("--total-size argument required"))?;
    let regions_json = regions_json
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("--regions argument required"))?;

    let output_path = std::path::Path::new(output_path);
    let data_regions: Vec<DataRegion> = serde_json::from_str(&regions_json)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    }

    // Create file and set its size (creates sparse file with holes)
    let mut output_file = std::fs::File::create(output_path)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    output_file
        .set_len(total_size)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    // Read and write each data region from stdin
    let mut stdin = std::io::stdin();
    let mut total_bytes_written = 0u64;

    for region in &data_regions {
        output_file
            .seek(SeekFrom::Start(region.offset))
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let mut buffer = vec![0u8; region.length as usize];
        stdin
            .read_exact(&mut buffer)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        output_file
            .write_all(&buffer)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        total_bytes_written += region.length;
    }

    output_file
        .flush()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    output_file
        .sync_all()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    // Set mtime if provided
    if let Some(mtime_secs) = mtime {
        use std::time::{Duration, UNIX_EPOCH};
        let mtime = UNIX_EPOCH + Duration::from_secs(mtime_secs);
        let _ = filetime::set_file_mtime(output_path, filetime::FileTime::from_system_time(mtime));
    }

    println!(
        "{{\"bytes_written\": {}, \"file_size\": {}, \"regions\": {}}}",
        total_bytes_written,
        total_size,
        data_regions.len()
    );

    Ok(0)
}
