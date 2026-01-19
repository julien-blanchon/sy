//! Automatic daemon mode setup for SSH destinations
//!
//! This module handles the complexity of daemon mode automatically:
//! 1. Checks if daemon is already running/accessible
//! 2. Starts daemon on remote if needed
//! 3. Sets up SSH socket forwarding with ControlMaster
//! 4. Returns socket path for use with daemon_mode sync

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
    // Parse SSH config to get full host details
    let config = if let Some(u) = user {
        SshConfig {
            hostname: host.to_string(),
            user: u.to_string(),
            ..Default::default()
        }
    } else {
        parse_ssh_config(host)?
    };

    // Create socket directory
    let socket_dir = PathBuf::from(SOCKET_DIR);
    tokio::fs::create_dir_all(&socket_dir).await.ok();

    // Generate unique socket paths based on host
    let safe_host = host.replace(['/', ':', '@'], "_");
    let local_socket = socket_dir.join(format!("{}.sock", safe_host));
    let control_path = socket_dir.join(format!("{}.control", safe_host));

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
            // Socket exists but not working, clean up
            tokio::fs::remove_file(&local_socket).await.ok();
        }
    }

    // Start daemon on remote and set up forwarding
    tracing::info!("Setting up daemon connection to {}...", host);

    // Step 1: Check if daemon is running on remote, start if not
    let daemon_started = ensure_remote_daemon(&config).await?;

    // Step 2: Set up SSH socket forwarding with ControlMaster
    setup_socket_forwarding(&config, &local_socket, &control_path).await?;

    // Step 3: Wait for socket to be ready
    wait_for_socket(&local_socket, Duration::from_secs(10)).await?;

    // Step 4: Test the connection
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

/// Ensure daemon is running on remote host
async fn ensure_remote_daemon(config: &SshConfig) -> Result<bool> {
    // Check if daemon is already running
    let check_cmd = format!(
        "test -S {} && echo 'running' || echo 'not running'",
        REMOTE_SOCKET
    );

    let output = run_ssh_command(config, &check_cmd).await?;

    if output.trim() == "running" {
        tracing::debug!("Daemon already running on remote");
        return Ok(false);
    }

    // Start daemon on remote
    tracing::info!("Starting daemon on remote...");

    let start_cmd = format!(
        "mkdir -p ~/.sy && nohup sy --daemon --socket {} > ~/.sy/daemon.log 2>&1 &",
        REMOTE_SOCKET
    );

    run_ssh_command(config, &start_cmd).await?;

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify daemon started
    let verify_output = run_ssh_command(config, &check_cmd).await?;
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

    // Socket forwarding
    cmd.arg("-L").arg(format!(
        "{}:{}",
        local_socket.display(),
        REMOTE_SOCKET.replace('~', &format!("/home/{}", config.user))
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
    tokio::time::sleep(Duration::from_millis(500)).await;

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

/// Run an SSH command and return output
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

    cmd.arg(command);

    let output = cmd.output().await.context("Failed to run SSH command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("SSH command failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
}
