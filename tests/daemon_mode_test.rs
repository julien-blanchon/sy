//! Integration tests for daemon mode
//!
//! These tests verify that the daemon mode sync works correctly locally.
//! For real-world SSH socket forwarding tests, use a remote server.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

/// Helper to create a temporary directory with test files
fn create_test_source() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("Failed to create temp dir");
    let path = dir.path().to_path_buf();

    // Create some test files
    fs::create_dir_all(path.join("subdir")).unwrap();
    fs::write(path.join("file1.txt"), "hello world").unwrap();
    fs::write(path.join("file2.txt"), "another file").unwrap();
    fs::write(path.join("subdir/nested.txt"), "nested content").unwrap();

    (dir, path)
}

/// Test that we can start and stop a daemon
#[tokio::test]
async fn test_daemon_lifecycle() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let socket_path = temp.path().join("daemon.sock");
    let root_path = temp.path().join("root");
    fs::create_dir_all(&root_path).unwrap();

    // Start daemon in background
    let socket_str = socket_path.to_string_lossy().to_string();
    let root = root_path.clone();

    let daemon_handle = tokio::spawn(async move {
        sy::server::daemon::run_daemon(&socket_str, &root).await
    });

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check socket exists
    assert!(socket_path.exists(), "Daemon socket should exist");

    // Abort the daemon (simulating shutdown)
    daemon_handle.abort();
    let _ = daemon_handle.await;
}

/// Test daemon session connection and handshake
#[tokio::test]
async fn test_daemon_session_connect() {
    use sy::transport::server::DaemonSession;

    let temp = TempDir::new().expect("Failed to create temp dir");
    let socket_path = temp.path().join("daemon.sock");
    let root_path = temp.path().join("root");
    fs::create_dir_all(&root_path).unwrap();

    // Start daemon in background
    let socket_str = socket_path.to_string_lossy().to_string();
    let root = root_path.clone();

    let daemon_handle = tokio::spawn(async move {
        sy::server::daemon::run_daemon(&socket_str, &root).await
    });

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect to daemon
    let socket_str = socket_path.to_string_lossy().to_string();
    let remote_path = PathBuf::from("/tmp/test");

    let session_result = timeout(
        Duration::from_secs(5),
        DaemonSession::connect(&socket_str, &remote_path),
    )
    .await;

    assert!(
        session_result.is_ok(),
        "Should connect to daemon within timeout"
    );

    let session = session_result.unwrap();
    assert!(session.is_ok(), "Session handshake should succeed");

    // Clean up
    daemon_handle.abort();
    let _ = daemon_handle.await;
}

/// Test daemon session ping
#[tokio::test]
async fn test_daemon_session_ping() {
    use sy::transport::server::DaemonSession;

    let temp = TempDir::new().expect("Failed to create temp dir");
    let socket_path = temp.path().join("daemon.sock");
    let root_path = temp.path().join("root");
    fs::create_dir_all(&root_path).unwrap();

    // Start daemon in background
    let socket_str = socket_path.to_string_lossy().to_string();
    let root = root_path.clone();

    let daemon_handle = tokio::spawn(async move {
        sy::server::daemon::run_daemon(&socket_str, &root).await
    });

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect to daemon
    let socket_str = socket_path.to_string_lossy().to_string();
    let remote_path = PathBuf::from("/tmp/test");

    let mut session = DaemonSession::connect(&socket_str, &remote_path)
        .await
        .expect("Should connect to daemon");

    // Test ping
    let ping_result = session.ping().await;
    assert!(ping_result.is_ok(), "Ping should succeed");

    // Close session
    session.close().await.expect("Should close gracefully");

    // Clean up daemon
    daemon_handle.abort();
    let _ = daemon_handle.await;
}

/// Test full sync through daemon (local mode)
#[tokio::test]
async fn test_daemon_sync_push() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let socket_path = temp.path().join("daemon.sock");
    let root_path = temp.path().join("dest");
    fs::create_dir_all(&root_path).unwrap();

    // Create source files
    let (source_temp, source_path) = create_test_source();

    // Start daemon in background
    let socket_str = socket_path.to_string_lossy().to_string();
    let root = root_path.clone();

    let daemon_handle = tokio::spawn(async move {
        sy::server::daemon::run_daemon(&socket_str, &root).await
    });

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Run sync - use absolute path for remote
    let socket_str = socket_path.to_string_lossy().to_string();
    let sync_result = sy::sync::daemon_mode::sync_daemon_mode(
        &source_path,
        &socket_str,
        &root_path, // Use absolute path
    )
    .await;

    assert!(sync_result.is_ok(), "Sync should succeed: {:?}", sync_result);

    let stats = sync_result.unwrap();
    assert_eq!(stats.files_created, 3, "Should create 3 files");
    assert!(stats.dirs_created >= 1, "Should create at least 1 directory");

    // Verify files were copied
    assert!(root_path.join("file1.txt").exists());
    assert!(root_path.join("file2.txt").exists());
    assert!(root_path.join("subdir/nested.txt").exists());

    // Verify content
    assert_eq!(
        fs::read_to_string(root_path.join("file1.txt")).unwrap(),
        "hello world"
    );

    // Clean up
    daemon_handle.abort();
    let _ = daemon_handle.await;
    drop(source_temp);
}

/// Test incremental sync (skip unchanged files)
#[tokio::test]
async fn test_daemon_sync_incremental() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let socket_path = temp.path().join("daemon.sock");
    let root_path = temp.path().join("dest");
    fs::create_dir_all(&root_path).unwrap();

    // Create source files
    let (source_temp, source_path) = create_test_source();

    // Start daemon in background
    let socket_str = socket_path.to_string_lossy().to_string();
    let root = root_path.clone();

    let daemon_handle = tokio::spawn(async move {
        sy::server::daemon::run_daemon(&socket_str, &root).await
    });

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    let socket_str = socket_path.to_string_lossy().to_string();

    // First sync - should create files
    let stats1 = sy::sync::daemon_mode::sync_daemon_mode(
        &source_path,
        &socket_str,
        &root_path, // Use absolute path
    )
    .await
    .expect("First sync should succeed");

    assert_eq!(stats1.files_created, 3, "First sync should create 3 files");

    // Second sync - should skip unchanged files
    let stats2 = sy::sync::daemon_mode::sync_daemon_mode(
        &source_path,
        &socket_str,
        &root_path, // Use absolute path
    )
    .await
    .expect("Second sync should succeed");

    assert_eq!(stats2.files_created, 0, "Second sync should create 0 files");
    assert_eq!(stats2.files_skipped, 3, "Second sync should skip 3 files");

    // Clean up
    daemon_handle.abort();
    let _ = daemon_handle.await;
    drop(source_temp);
}

/// Test pull mode (daemon sends files to local)
#[tokio::test]
async fn test_daemon_sync_pull() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let socket_path = temp.path().join("daemon.sock");
    let local_dest = temp.path().join("local_dest");
    fs::create_dir_all(&local_dest).unwrap();

    // Create source files in daemon root
    let daemon_root = temp.path().join("daemon_root");
    fs::create_dir_all(daemon_root.join("subdir")).unwrap();
    fs::write(daemon_root.join("remote1.txt"), "remote content 1").unwrap();
    fs::write(daemon_root.join("remote2.txt"), "remote content 2").unwrap();
    fs::write(daemon_root.join("subdir/remote_nested.txt"), "nested remote").unwrap();

    // Start daemon in background
    let socket_str = socket_path.to_string_lossy().to_string();
    let root = daemon_root.clone();

    let daemon_handle = tokio::spawn(async move {
        sy::server::daemon::run_daemon(&socket_str, &root).await
    });

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Run pull sync - use absolute path
    let socket_str = socket_path.to_string_lossy().to_string();
    let sync_result = sy::sync::daemon_mode::sync_pull_daemon_mode(
        &socket_str,
        &daemon_root, // Use absolute path
        &local_dest,
    )
    .await;

    assert!(sync_result.is_ok(), "Pull sync should succeed: {:?}", sync_result);

    let stats = sync_result.unwrap();
    assert_eq!(stats.files_created, 3, "Should pull 3 files");

    // Verify files were pulled
    assert!(local_dest.join("remote1.txt").exists());
    assert!(local_dest.join("remote2.txt").exists());
    assert!(local_dest.join("subdir/remote_nested.txt").exists());

    // Verify content
    assert_eq!(
        fs::read_to_string(local_dest.join("remote1.txt")).unwrap(),
        "remote content 1"
    );

    // Clean up
    daemon_handle.abort();
    let _ = daemon_handle.await;
}
