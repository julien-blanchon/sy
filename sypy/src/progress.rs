//! Progress callback bridge for Python
//!
//! Note: This module is prepared for future progress callback support.
//! Currently unused but kept for planned features.

#![allow(dead_code)]

use pyo3::prelude::*;
use std::sync::Arc;

/// Progress callback wrapper that can be called from Rust
///
/// The callback signature is: (current: int, total: int, path: str, action: str)
#[derive(Clone)]
pub struct ProgressCallback {
    callback: Arc<PyObject>,
}

impl ProgressCallback {
    /// Create a new progress callback from a Python callable
    pub fn new(callback: PyObject) -> Self {
        ProgressCallback {
            callback: Arc::new(callback),
        }
    }

    /// Call the progress callback
    ///
    /// Returns Ok(()) if the callback was called successfully, or if there was no callback.
    /// Returns Err if the callback raised an exception.
    pub fn call(&self, current: u64, total: u64, path: &str, action: &str) -> PyResult<()> {
        Python::with_gil(|py| {
            self.callback
                .call1(py, (current, total, path, action))
                .map(|_| ())
        })
    }

    /// Call the progress callback, ignoring any errors
    pub fn call_ignore_errors(&self, current: u64, total: u64, path: &str, action: &str) {
        let _ = self.call(current, total, path, action);
    }
}

/// Progress action types
pub enum ProgressAction {
    Scanning,
    Creating,
    Updating,
    Deleting,
    Verifying,
    Skipping,
}

impl ProgressAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProgressAction::Scanning => "scanning",
            ProgressAction::Creating => "creating",
            ProgressAction::Updating => "updating",
            ProgressAction::Deleting => "deleting",
            ProgressAction::Verifying => "verifying",
            ProgressAction::Skipping => "skipping",
        }
    }
}
