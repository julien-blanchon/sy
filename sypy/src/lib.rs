//! Python bindings for sy - Modern file synchronization tool
//!
//! This module provides PyO3-based Python bindings for the sy Rust crate,
//! exposing all major sync functionality with a Pythonic API.

mod cli;
mod config;
mod error;
mod options;
mod path;
mod progress;
mod stats;
mod sync;

use pyo3::prelude::*;

/// Python module for sy file synchronization
#[pymodule]
fn _sypy(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Register classes
    m.add_class::<stats::PySyncStats>()?;
    m.add_class::<stats::PySyncError>()?;
    m.add_class::<path::PySyncPath>()?;
    m.add_class::<options::PySyncOptions>()?;

    // Register config classes
    m.add_class::<config::PyCloudClientOptions>()?;
    m.add_class::<config::PyS3Config>()?;
    m.add_class::<config::PyGcsConfig>()?;
    m.add_class::<config::PySshConfig>()?;

    // Register functions
    m.add_function(wrap_pyfunction!(sync::sync, m)?)?;
    m.add_function(wrap_pyfunction!(sync::sync_with_options, m)?)?;
    m.add_function(wrap_pyfunction!(path::parse_path, m)?)?;

    // Register CLI functions
    m.add_function(wrap_pyfunction!(cli::main, m)?)?;
    m.add_function(wrap_pyfunction!(cli::run_server, m)?)?;
    m.add_function(wrap_pyfunction!(cli::run_daemon, m)?)?;

    // Add version info
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}
