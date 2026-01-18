//! Python exception types for sy errors

use pyo3::exceptions::{PyIOError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;

/// Convert anyhow::Error to PyErr
pub fn anyhow_to_pyerr(err: anyhow::Error) -> PyErr {
    // Check for specific error types and convert accordingly
    let msg = err.to_string();

    if msg.contains("not found") || msg.contains("No such file") {
        PyIOError::new_err(msg)
    } else if msg.contains("Permission denied") {
        PyIOError::new_err(msg)
    } else if msg.contains("Invalid") {
        PyValueError::new_err(msg)
    } else {
        PyRuntimeError::new_err(msg)
    }
}

/// Convert sy::error::SyncError to PyErr
pub fn sync_error_to_pyerr(err: sy::error::SyncError) -> PyErr {
    use sy::error::SyncError;

    match &err {
        SyncError::SourceNotFound { path } => {
            PyIOError::new_err(format!("Source not found: {}", path.display()))
        }
        SyncError::DestinationNotFound { path } => {
            PyIOError::new_err(format!("Destination not found: {}", path.display()))
        }
        SyncError::PermissionDenied { path } => {
            PyIOError::new_err(format!("Permission denied: {}", path.display()))
        }
        SyncError::Io(e) => PyIOError::new_err(e.to_string()),
        SyncError::ReadDirError { path, source } => {
            PyIOError::new_err(format!("Failed to read {}: {}", path.display(), source))
        }
        SyncError::CopyError { path, source } => {
            PyIOError::new_err(format!("Failed to copy {}: {}", path.display(), source))
        }
        SyncError::InvalidPath { path } => {
            PyValueError::new_err(format!("Invalid path: {}", path.display()))
        }
        SyncError::InsufficientDiskSpace {
            path,
            required,
            available,
        } => PyIOError::new_err(format!(
            "Insufficient disk space at {}: required {} bytes, available {} bytes",
            path.display(),
            required,
            available
        )),
        SyncError::NetworkTimeout { duration } => {
            PyRuntimeError::new_err(format!("Network timeout after {:?}", duration))
        }
        SyncError::NetworkDisconnected { reason } => {
            PyRuntimeError::new_err(format!("Network disconnected: {}", reason))
        }
        SyncError::NetworkRetryable { message, .. } => {
            PyRuntimeError::new_err(format!("Network error (retryable): {}", message))
        }
        SyncError::NetworkFatal { message } => {
            PyRuntimeError::new_err(format!("Network error: {}", message))
        }
        SyncError::Config(msg) => PyValueError::new_err(format!("Configuration error: {}", msg)),
        _ => PyRuntimeError::new_err(err.to_string()),
    }
}
