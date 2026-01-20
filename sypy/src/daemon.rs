//! Daemon management for Python API
//!
//! This module provides explicit control over daemon lifecycle,
//! allowing Python code to start, stop, and check daemon status.

use pyo3::prelude::*;
use std::path::PathBuf;
use sy::ssh::config::SshConfig;
use sy::sync::daemon_auto::{ensure_daemon_connection_with_config, DaemonAutoResult};

/// Configuration for daemon mode
///
/// Controls where daemon sockets are stored and how long connections persist.
///
/// Example:
///     >>> daemon = DaemonConfig(
///     ...     socket_dir="/tmp/my-sy-daemon",
///     ...     remote_socket="~/.sy/daemon.sock",
///     ...     control_persist="10m"
///     ... )
#[pyclass(name = "DaemonConfig")]
#[derive(Clone, Debug)]
pub struct PyDaemonConfig {
    /// Local directory for daemon sockets (default: /tmp/sy-daemon)
    #[pyo3(get, set)]
    pub socket_dir: Option<String>,

    /// Remote daemon socket path (default: ~/.sy/daemon.sock)
    #[pyo3(get, set)]
    pub remote_socket: Option<String>,

    /// SSH ControlPersist time (e.g., "10m", "1h")
    /// How long the SSH connection stays alive after last use
    /// Default: 10m
    #[pyo3(get, set)]
    pub control_persist: Option<String>,

    /// Whether to start daemon if not running (default: true)
    #[pyo3(get, set)]
    pub auto_start: bool,
}

impl Default for PyDaemonConfig {
    fn default() -> Self {
        Self {
            socket_dir: None,
            remote_socket: None,
            control_persist: None,
            auto_start: true,
        }
    }
}

#[pymethods]
impl PyDaemonConfig {
    #[new]
    #[pyo3(signature = (
        socket_dir = None,
        remote_socket = None,
        control_persist = None,
        auto_start = true
    ))]
    pub fn new(
        socket_dir: Option<String>,
        remote_socket: Option<String>,
        control_persist: Option<String>,
        auto_start: bool,
    ) -> Self {
        Self {
            socket_dir,
            remote_socket,
            control_persist,
            auto_start,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "DaemonConfig(socket_dir={:?}, remote_socket={:?}, control_persist={:?}, auto_start={})",
            self.socket_dir, self.remote_socket, self.control_persist, self.auto_start
        )
    }
}

/// Daemon connection information
///
/// Returned when starting a daemon, contains paths and status.
///
/// Example:
///     >>> info = daemon_start("user@host", "/remote/path")
///     >>> print(f"Daemon socket: {info.socket_path}")
///     >>> print(f"Daemon started: {info.daemon_started}")
#[pyclass(name = "DaemonInfo")]
#[derive(Clone, Debug)]
pub struct PyDaemonInfo {
    /// Local socket path to connect to daemon
    #[pyo3(get)]
    pub socket_path: String,

    /// SSH control socket path
    #[pyo3(get)]
    pub control_path: String,

    /// Whether daemon was started (vs reused)
    #[pyo3(get)]
    pub daemon_started: bool,

    /// Remote host
    #[pyo3(get)]
    pub host: String,

    /// Remote user
    #[pyo3(get)]
    pub user: String,
}

#[pymethods]
impl PyDaemonInfo {
    fn __repr__(&self) -> String {
        format!(
            "DaemonInfo(socket={}, started={}, host={}@{})",
            self.socket_path, self.daemon_started, self.user, self.host
        )
    }
}

/// Start or connect to a daemon for a remote host
///
/// This sets up SSH port forwarding and starts the daemon if needed.
/// The connection persists for the configured time (default 10 minutes).
///
/// Args:
///     remote (str): Remote path in format "user@host:/path" or "user@host"
///     ssh (SshConfig, optional): SSH configuration
///     daemon (DaemonConfig, optional): Daemon configuration
///
/// Returns:
///     DaemonInfo: Information about the daemon connection
///
/// Example:
///     >>> # Start daemon with defaults
///     >>> info = sypy.daemon_start("user@host:/remote/path")
///     >>> print(f"Daemon ready at {info.socket_path}")
///     >>>
///     >>> # Custom socket location
///     >>> daemon = sypy.DaemonConfig(socket_dir="/tmp/my-daemons")
///     >>> info = sypy.daemon_start("user@host:/path", daemon=daemon)
///     >>>
///     >>> # With SSH config
///     >>> ssh = sypy.SshConfig(port=2222, key_file="~/.ssh/id_rsa")
///     >>> info = sypy.daemon_start("user@host:/path", ssh=ssh)
#[pyfunction(name = "daemon_start")]
#[pyo3(signature = (remote, ssh = None, daemon = None))]
pub fn py_daemon_start(
    py: Python,
    remote: &str,
    ssh: Option<&crate::config::PySshConfig>,
    daemon: Option<PyDaemonConfig>,
) -> PyResult<PyDaemonInfo> {
    let daemon_config = daemon.unwrap_or_default();

    // Parse remote path
    let path = crate::path::parse_path(remote);

    let (host, user_opt, remote_path) = match &path.inner {
        sy::path::SyncPath::Remote {
            host, user, path, ..
        } => (host.clone(), user.clone(), path.clone()),
        _ => {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "daemon_start requires a remote path (user@host:/path)",
            ));
        }
    };

    // Get user (use whoami if not specified)
    let user = user_opt.unwrap_or_else(|| whoami::username());

    // Build SSH config
    let ssh_config = if let Some(ssh_cfg) = ssh {
        ssh_cfg.to_sy_ssh_config(&host, &user)
    } else {
        let mut cfg =
            sy::ssh::config::parse_ssh_config(&host).unwrap_or_else(|_| SshConfig::new(&host));
        cfg.user = user.clone();
        cfg
    };

    // Start daemon (with custom socket paths if configured)
    let custom_socket_dir = daemon_config.socket_dir.as_ref().map(PathBuf::from);
    let result: DaemonAutoResult = py
        .allow_threads(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                ensure_daemon_connection_with_config(
                    &host,
                    Some(user.as_str()),
                    &remote_path,
                    Some(&ssh_config),
                    custom_socket_dir.as_deref(),
                )
                .await
            })
        })
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

    Ok(PyDaemonInfo {
        socket_path: result.socket_path,
        control_path: result.control_path,
        daemon_started: result.daemon_started,
        host,
        user,
    })
}

/// Check if a daemon is running for a remote host
///
/// Args:
///     remote (str): Remote path in format "user@host:/path" or "user@host"
///     daemon (DaemonConfig, optional): Daemon configuration
///
/// Returns:
///     bool: True if daemon socket exists and is connectable
///
/// Example:
///     >>> if sypy.daemon_check("user@host"):
///     ...     print("Daemon is running")
///     ... else:
///     ...     print("Daemon is not running")
#[pyfunction(name = "daemon_check")]
#[pyo3(signature = (remote, daemon = None))]
pub fn py_daemon_check(py: Python, remote: &str, daemon: Option<PyDaemonConfig>) -> PyResult<bool> {
    let daemon_config = daemon.unwrap_or_default();

    // Parse remote path
    let path = crate::path::parse_path(remote);

    let (host, user_opt, _remote_path) = match &path.inner {
        sy::path::SyncPath::Remote {
            host, user, path, ..
        } => (host.clone(), user.clone(), path.clone()),
        _ => {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "daemon_check requires a remote path (user@host:/path)",
            ));
        }
    };

    // Get user for socket naming (matches daemon_start logic)
    let user = user_opt.unwrap_or_else(|| whoami::username());

    // Check if socket exists (use custom socket_dir if provided)
    let socket_dir = daemon_config
        .socket_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/sy-daemon"));
    let safe_host = host.replace(['/', ':', '@'], "_");
    let socket_name = format!("{}@{}", user, safe_host);
    let local_socket = socket_dir.join(format!("{}.sock", socket_name));

    if !local_socket.exists() {
        return Ok(false);
    }

    // Try to connect to socket
    let socket_path = local_socket.to_string_lossy().to_string();
    let can_connect = py.allow_threads(|| {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { tokio::net::UnixStream::connect(&socket_path).await.is_ok() })
    });

    Ok(can_connect)
}

/// Stop daemon for a remote host
///
/// This closes the SSH ControlMaster connection. The daemon on the
/// remote host continues running, but the local socket is closed.
///
/// Args:
///     remote (str): Remote path in format "user@host:/path" or "user@host"
///     daemon (DaemonConfig, optional): Daemon configuration
///
/// Returns:
///     bool: True if daemon was stopped, False if not running
///
/// Example:
///     >>> # Stop daemon
///     >>> if sypy.daemon_stop("user@host"):
///     ...     print("Daemon stopped")
///     ... else:
///     ...     print("Daemon was not running")
#[pyfunction(name = "daemon_stop")]
#[pyo3(signature = (remote, daemon = None))]
pub fn py_daemon_stop(py: Python, remote: &str, daemon: Option<PyDaemonConfig>) -> PyResult<bool> {
    let daemon_config = daemon.unwrap_or_default();

    // Parse remote path
    let path = crate::path::parse_path(remote);

    let (host, user_opt, _remote_path) = match &path.inner {
        sy::path::SyncPath::Remote {
            host, user, path, ..
        } => (host.clone(), user.clone(), path.clone()),
        _ => {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "daemon_stop requires a remote path (user@host:/path)",
            ));
        }
    };

    // Get user for socket naming (matches daemon_start logic)
    let user = user_opt.unwrap_or_else(|| whoami::username());

    // Get control socket path (use custom socket_dir if provided)
    let socket_dir = daemon_config
        .socket_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/sy-daemon"));
    let safe_host = host.replace(['/', ':', '@'], "_");
    let socket_name = format!("{}@{}", user, safe_host);
    let control_path = socket_dir.join(format!("{}.control", socket_name));

    if !control_path.exists() {
        return Ok(false);
    }

    // Send SSH -O exit command
    let control_path_str = control_path.to_string_lossy().to_string();
    let stopped = py.allow_threads(|| {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            sy::sync::daemon_auto::cleanup_daemon_connection(&control_path_str)
                .await
                .is_ok()
        })
    });

    // Clean up local socket
    let local_socket = socket_dir.join(format!("{}.sock", socket_name));
    if local_socket.exists() {
        std::fs::remove_file(&local_socket).ok();
    }

    Ok(stopped)
}

/// Context manager for daemon connections
///
/// Automatically starts daemon on enter and optionally stops on exit.
///
/// Example:
///     >>> with sypy.daemon_context("user@host:/path") as info:
///     ...     # Daemon is ready
///     ...     stats = sypy.sync("/local/", "user@host:/remote/", daemon_auto=True)
///     ...     # Daemon stays alive for 10 minutes after exit
#[pyclass(name = "DaemonContext")]
pub struct PyDaemonContext {
    remote: String,
    ssh: Option<crate::config::PySshConfig>,
    daemon: Option<PyDaemonConfig>,
    info: Option<PyDaemonInfo>,
    auto_stop: bool,
}

#[pymethods]
impl PyDaemonContext {
    #[new]
    #[pyo3(signature = (remote, ssh = None, daemon = None, auto_stop = false))]
    fn new(
        remote: String,
        ssh: Option<crate::config::PySshConfig>,
        daemon: Option<PyDaemonConfig>,
        auto_stop: bool,
    ) -> Self {
        Self {
            remote,
            ssh,
            daemon,
            info: None,
            auto_stop,
        }
    }

    fn __enter__(&mut self, py: Python) -> PyResult<PyDaemonInfo> {
        let info = py_daemon_start(py, &self.remote, self.ssh.as_ref(), self.daemon.clone())?;
        self.info = Some(info.clone());
        Ok(info)
    }

    fn __exit__(
        &mut self,
        py: Python,
        _exc_type: PyObject,
        _exc_value: PyObject,
        _traceback: PyObject,
    ) -> PyResult<bool> {
        if self.auto_stop {
            py_daemon_stop(py, &self.remote, self.daemon.clone())?;
        }
        Ok(false) // Don't suppress exceptions
    }

    fn __repr__(&self) -> String {
        format!("DaemonContext(remote={})", self.remote)
    }
}
