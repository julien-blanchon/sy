//! Automatic daemon mode setup for SSH destinations
//!
//! This module handles the complexity of daemon mode automatically:
//! 1. Establishes SSH ControlMaster connection first (for connection reuse)
//! 2. Checks if daemon is already running/accessible (via ControlMaster)
//! 3. Starts daemon on remote if needed (via ControlMaster)
//! 4. Sets up SSH socket forwarding with ControlMaster
//! 5. Returns socket path for use with daemon_mode sync
//!
//! Performance optimization: By establishing ControlMaster first, all subsequent
//! SSH commands reuse the same TCP connection, reducing startup time from ~12-15s
//! to ~5-6s on high-latency networks.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::ssh::config::{parse_ssh_config, SshConfig};

/// Default socket directory
const SOCKET_DIR: &str = "/tmp/sy-daemon";

/// Default daemon socket name on remote
const REMOTE_SOCKET: &str = "~/.sy/daemon.sock";

/// ControlMaster persist time (10 minutes)
const CONTROL_PERSIST: &str = "10m";

/// Timeout for establishing ControlMaster connection
const CONTROL_MASTER_TIMEOUT_SECS: u64 = 30;

/// Result of daemon auto-setup
pub struct DaemonAutoResult {
    /// Local socket path to use
    pub socket_path: String,
    /// SSH control socket path (for cleanup)
    #[allow(dead_code)]
    pub control_path: String,
    /// Whether we started the daemon (vs reusing existing)
    pub daemon_started: bool,
}

/// Ensure daemon is running and accessible for the given SSH host
///
/// Returns the local socket path to use with daemon_mode sync
pub async fn ensure_daemon_connection(
    host: &str,
    user: Option<&str>,
    _remote_path: &Path,
) -> Result<DaemonAutoResult> {
    ensure_daemon_connection_with_config(host, user, _remote_path, None, None).await
}

pub async fn ensure_daemon_connection_with_config(
    host: &str,
    user: Option<&str>,
    _remote_path: &Path,
    ssh_config_override: Option<&SshConfig>,
    custom_socket_dir: Option<&Path>,
) -> Result<DaemonAutoResult> {
    // Parse SSH config to get full host details
    let config = if let Some(override_config) = ssh_config_override {
        // Use provided config but update host and user from parameters
        let mut c = override_config.clone();
        c.hostname = host.to_string();
        if let Some(u) = user {
            c.user = u.to_string();
        }
        c
    } else if let Some(u) = user {
        // Try to parse from SSH config file first, then use defaults
        parse_ssh_config(host).unwrap_or_else(|_| SshConfig {
            hostname: host.to_string(),
            user: u.to_string(),
            ..Default::default()
        })
    } else {
        parse_ssh_config(host)?
    };

    // Create socket directory (use custom if provided, else default)
    let socket_dir = custom_socket_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(SOCKET_DIR));
    tokio::fs::create_dir_all(&socket_dir).await.ok();

    // Generate unique socket paths based on user@host
    // Include user to avoid collisions when different users connect to the same host
    let safe_host = host.replace(['/', ':', '@'], "_");
    let socket_name = if config.user.is_empty() {
        safe_host
    } else {
        format!("{}@{}", config.user, safe_host)
    };
    let local_socket = socket_dir.join(format!("{}.sock", socket_name));
    let control_path = socket_dir.join(format!("{}.control", socket_name));

    // Check if local socket already exists and is usable
    if local_socket.exists() {
        if test_socket_connection(&local_socket).await.is_ok() {
            tracing::info!(
                "Reusing existing daemon connection at {}",
                local_socket.display()
            );
            return Ok(DaemonAutoResult {
                socket_path: local_socket.to_string_lossy().to_string(),
                control_path: control_path.to_string_lossy().to_string(),
                daemon_started: false,
            });
        } else {
            // Socket exists but not working, clean up both socket and control
            tracing::debug!("Local socket exists but not working, cleaning up");
            tokio::fs::remove_file(&local_socket).await.ok();
            cleanup_control_master(&control_path).await;
        }
    } else if control_path.exists() {
        // Local socket is missing but control path exists - this is a stale state
        // (e.g., /tmp cleanup removed socket but not control)
        // Clean up the stale ControlMaster to start fresh
        tracing::debug!(
            "Local socket missing but control path exists, cleaning up stale ControlMaster"
        );
        cleanup_control_master(&control_path).await;
    }

    // Start daemon on remote and set up forwarding
    tracing::info!("Setting up daemon connection to {}...", host);

    // Step 1: Establish ControlMaster connection FIRST
    // This is the key optimization - all subsequent SSH commands will reuse this connection
    establish_control_master(&config, &control_path).await?;

    // Step 2: Check if daemon is running on remote, start if not (uses ControlMaster)
    let daemon_started = ensure_remote_daemon(&config, &control_path).await?;

    // Step 3: Set up SSH socket forwarding (uses existing ControlMaster)
    setup_socket_forwarding(&config, &local_socket, &control_path).await?;

    // Step 4: Wait for socket to be ready
    wait_for_socket(&local_socket, Duration::from_secs(10)).await?;

    // Step 5: Test the connection
    test_socket_connection(&local_socket)
        .await
        .context("Failed to connect to daemon after setup")?;

    tracing::info!("Daemon connection ready at {}", local_socket.display());

    Ok(DaemonAutoResult {
        socket_path: local_socket.to_string_lossy().to_string(),
        control_path: control_path.to_string_lossy().to_string(),
        daemon_started,
    })
}

/// Test if a Unix socket is connectable
async fn test_socket_connection(socket_path: &Path) -> Result<()> {
    let stream = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(socket_path))
        .await
        .context("Connection timeout")?
        .context("Failed to connect")?;

    // Just connecting is enough to verify it works
    drop(stream);
    Ok(())
}

/// Wait for socket file to appear
async fn wait_for_socket(socket_path: &Path, timeout: Duration) -> Result<()> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if socket_path.exists() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("Timeout waiting for socket at {}", socket_path.display())
}

/// Establish SSH ControlMaster connection
///
/// This creates a persistent SSH connection that subsequent commands can reuse,
/// dramatically reducing the time for multiple SSH operations.
async fn establish_control_master(config: &SshConfig, control_path: &Path) -> Result<()> {
    // Check if ControlMaster is already established
    if control_path.exists() {
        // Verify it's still working with a check command
        let check = Command::new("ssh")
            .arg("-O")
            .arg("check")
            .arg("-o")
            .arg(format!("ControlPath={}", control_path.display()))
            .arg(&config.hostname)
            .output()
            .await;

        if let Ok(output) = check {
            if output.status.success() {
                tracing::debug!("Reusing existing ControlMaster connection");
                return Ok(());
            }
        }
        // ControlMaster socket exists but not working, remove it
        tokio::fs::remove_file(control_path).await.ok();
    }

    tracing::debug!(
        "Establishing ControlMaster connection to {}...",
        config.hostname
    );

    // Build SSH command to establish ControlMaster
    let mut cmd = Command::new("ssh");

    cmd.arg(&config.hostname);

    if !config.user.is_empty() {
        cmd.arg("-l").arg(&config.user);
    }

    if config.port != 22 {
        cmd.arg("-p").arg(config.port.to_string());
    }

    for key in &config.identity_file {
        cmd.arg("-i").arg(key);
    }

    // ControlMaster options - establish as master
    cmd.arg("-o").arg("ControlMaster=yes");
    cmd.arg("-o")
        .arg(format!("ControlPath={}", control_path.display()));
    cmd.arg("-o")
        .arg(format!("ControlPersist={}", CONTROL_PERSIST));

    // Non-interactive options
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("StrictHostKeyChecking=no");
    cmd.arg("-o").arg("UserKnownHostsFile=/dev/null");

    // Run a simple command to establish the connection
    cmd.arg("echo").arg("connected");

    let output = tokio::time::timeout(
        Duration::from_secs(CONTROL_MASTER_TIMEOUT_SECS),
        cmd.output(),
    )
    .await
    .context("Timeout establishing ControlMaster connection")?
    .context("Failed to establish ControlMaster connection")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to establish ControlMaster: {}", stderr);
    }

    tracing::debug!("ControlMaster connection established");
    Ok(())
}

/// Ensure daemon is running on remote host
///
/// Uses ControlMaster for fast command execution
async fn ensure_remote_daemon(config: &SshConfig, control_path: &Path) -> Result<bool> {
    // Check if daemon is already running
    let check_cmd = format!(
        "test -S {} && echo 'running' || echo 'not running'",
        REMOTE_SOCKET
    );

    let output = run_ssh_command_with_control(config, control_path, &check_cmd).await?;

    if output.trim() == "running" {
        tracing::debug!("Daemon already running on remote");
        return Ok(false);
    }

    // Start daemon on remote
    tracing::info!("Starting daemon on remote...");

    // Use subshell backgrounding `( command & )` which properly detaches the process
    // This is more reliable than `nohup` for SSH sessions as it doesn't wait for
    // the background process to close stdout/stderr
    let start_cmd = format!(
        "mkdir -p ~/.sy && ( sy --daemon --socket {} </dev/null >~/.sy/daemon.log 2>&1 & )",
        REMOTE_SOCKET
    );

    run_ssh_command_with_control(config, control_path, &start_cmd).await?;

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify daemon started
    let verify_output = run_ssh_command_with_control(config, control_path, &check_cmd).await?;
    if verify_output.trim() != "running" {
        anyhow::bail!("Failed to start daemon on remote");
    }

    Ok(true)
}

/// Set up SSH socket forwarding with ControlMaster
async fn setup_socket_forwarding(
    config: &SshConfig,
    local_socket: &Path,
    control_path: &Path,
) -> Result<()> {
    // Remove existing socket if present
    tokio::fs::remove_file(local_socket).await.ok();

    // Build SSH command with ControlMaster and socket forwarding
    let mut cmd = Command::new("ssh");

    // Host
    cmd.arg(&config.hostname);

    // User
    if !config.user.is_empty() {
        cmd.arg("-l").arg(&config.user);
    }

    // Port
    if config.port != 22 {
        cmd.arg("-p").arg(config.port.to_string());
    }

    // Identity files
    for key in &config.identity_file {
        cmd.arg("-i").arg(key);
    }

    // ControlMaster options
    cmd.arg("-o").arg("ControlMaster=auto");
    cmd.arg("-o")
        .arg(format!("ControlPath={}", control_path.display()));
    cmd.arg("-o")
        .arg(format!("ControlPersist={}", CONTROL_PERSIST));
    cmd.arg("-o").arg("StreamLocalBindUnlink=yes");

    // Disable host key checking for non-interactive use
    cmd.arg("-o").arg("StrictHostKeyChecking=no");
    cmd.arg("-o").arg("UserKnownHostsFile=/dev/null");

    // Socket forwarding
    // Note: root's home is /root, not /home/root
    let home_dir = if config.user == "root" {
        "/root".to_string()
    } else {
        format!("/home/{}", config.user)
    };
    cmd.arg("-L").arg(format!(
        "{}:{}",
        local_socket.display(),
        REMOTE_SOCKET.replace('~', &home_dir)
    ));

    // Don't execute command, just forward
    cmd.arg("-N");

    // Run in background
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    tracing::debug!("Starting SSH forwarding: {:?}", cmd);

    let mut child = cmd.spawn().context("Failed to spawn SSH process")?;

    // Give SSH time to establish connection
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check if SSH is still running
    match child.try_wait() {
        Ok(Some(status)) => {
            if !status.success() {
                anyhow::bail!("SSH socket forwarding failed with status: {}", status);
            }
        }
        Ok(None) => {
            // Still running, good
            tracing::debug!("SSH forwarding established");
        }
        Err(e) => {
            anyhow::bail!("Failed to check SSH status: {}", e);
        }
    }

    Ok(())
}

/// Run an SSH command using ControlMaster for fast execution
///
/// This reuses the existing ControlMaster connection, making command execution
/// nearly instant instead of requiring a new TCP handshake + SSH handshake.
async fn run_ssh_command_with_control(
    config: &SshConfig,
    control_path: &Path,
    command: &str,
) -> Result<String> {
    let mut cmd = Command::new("ssh");

    cmd.arg(&config.hostname);

    if !config.user.is_empty() {
        cmd.arg("-l").arg(&config.user);
    }

    if config.port != 22 {
        cmd.arg("-p").arg(config.port.to_string());
    }

    for key in &config.identity_file {
        cmd.arg("-i").arg(key);
    }

    // Use existing ControlMaster connection
    cmd.arg("-o").arg("ControlMaster=auto");
    cmd.arg("-o")
        .arg(format!("ControlPath={}", control_path.display()));

    // Batch mode for non-interactive
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("StrictHostKeyChecking=no");
    cmd.arg("-o").arg("UserKnownHostsFile=/dev/null");

    cmd.arg(command);

    let output = cmd.output().await.context("Failed to run SSH command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("SSH command failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run an SSH command without ControlMaster (for cases where it's not available)
#[allow(dead_code)]
async fn run_ssh_command(config: &SshConfig, command: &str) -> Result<String> {
    let mut cmd = Command::new("ssh");

    cmd.arg(&config.hostname);

    if !config.user.is_empty() {
        cmd.arg("-l").arg(&config.user);
    }

    if config.port != 22 {
        cmd.arg("-p").arg(config.port.to_string());
    }

    for key in &config.identity_file {
        cmd.arg("-i").arg(key);
    }

    // Batch mode for non-interactive
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("StrictHostKeyChecking=no");
    cmd.arg("-o").arg("UserKnownHostsFile=/dev/null");

    cmd.arg(command);

    let output = cmd.output().await.context("Failed to run SSH command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("SSH command failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Clean up a stale ControlMaster connection
///
/// Sends the exit command to terminate the ControlMaster and removes the socket file.
/// This is used when we detect a stale state (e.g., local socket deleted but control remains).
async fn cleanup_control_master(control_path: &Path) {
    // Try to gracefully exit the ControlMaster
    let mut cmd = Command::new("ssh");
    cmd.arg("-O").arg("exit");
    cmd.arg("-o")
        .arg(format!("ControlPath={}", control_path.display()));
    cmd.arg("dummy"); // Host doesn't matter for -O exit

    // Don't care about result - might already be dead
    let _ = tokio::time::timeout(Duration::from_secs(5), cmd.output()).await;

    // Also remove the socket file in case exit didn't clean it up
    tokio::fs::remove_file(control_path).await.ok();

    tracing::debug!("Cleaned up ControlMaster at {}", control_path.display());
}

/// Clean up daemon auto resources
#[allow(dead_code)]
pub async fn cleanup_daemon_connection(control_path: &str) -> Result<()> {
    // Send exit command to ControlMaster
    let mut cmd = Command::new("ssh");
    cmd.arg("-O").arg("exit");
    cmd.arg("-o").arg(format!("ControlPath={}", control_path));
    cmd.arg("dummy"); // Host doesn't matter for -O exit

    let _ = cmd.output().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_safe_host_name() {
        let host = "user@host.com:22";
        let safe = host.replace(['/', ':', '@'], "_");
        assert_eq!(safe, "user_host.com_22");
    }

    #[test]
    fn test_socket_naming_with_user() {
        // Simulate the socket naming logic
        let host = "myserver";
        let user = "testuser";
        let safe_host = host.replace(['/', ':', '@'], "_");
        let socket_name = format!("{}@{}", user, safe_host);
        assert_eq!(socket_name, "testuser@myserver");
    }

    #[test]
    fn test_different_users_different_sockets() {
        let host = "myserver";
        let safe_host = host.replace(['/', ':', '@'], "_");

        let socket1 = format!("{}@{}.sock", "user1", safe_host);
        let socket2 = format!("{}@{}.sock", "user2", safe_host);

        assert_ne!(socket1, socket2);
        assert_eq!(socket1, "user1@myserver.sock");
        assert_eq!(socket2, "user2@myserver.sock");
    }
}
