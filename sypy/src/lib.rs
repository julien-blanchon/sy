//! Python bindings for sy - Modern file synchronization tool
//!
//! This module provides PyO3-based Python bindings for the sy Rust crate,
//! exposing all major sync functionality with a Pythonic API.

// False positive: clippy flags return type annotations as "useless conversions"
#![allow(clippy::useless_conversion)]

mod cli;
mod config;
mod daemon;
mod dryrun;
mod error;
mod ls;
mod ops;
mod options;
mod path;
pub mod progress;
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
    m.add_class::<ls::PyListEntry>()?;

    // Register config classes
    m.add_class::<config::PyCloudClientOptions>()?;
    m.add_class::<config::PyS3Config>()?;
    m.add_class::<config::PyGcsConfig>()?;
    m.add_class::<config::PySshConfig>()?;

    // Register dry-run classes
    m.add_class::<dryrun::PyChangeAction>()?;
    m.add_class::<dryrun::PyFileChange>()?;
    m.add_class::<dryrun::PyDirectoryChange>()?;
    m.add_class::<dryrun::PySymlinkChange>()?;
    m.add_class::<dryrun::PyDryRunDetails>()?;

    // Register progress classes
    m.add_class::<progress::PyProgressSnapshot>()?;

    // Register daemon classes
    m.add_class::<daemon::PyDaemonConfig>()?;
    m.add_class::<daemon::PyDaemonInfo>()?;
    m.add_class::<daemon::PyDaemonContext>()?;

    // Register ops classes
    m.add_class::<ops::PyGetOptions>()?;
    m.add_class::<ops::PyPutOptions>()?;
    m.add_class::<ops::PyRemoveOptions>()?;
    m.add_class::<ops::PyGetResult>()?;
    m.add_class::<ops::PyPutResult>()?;
    m.add_class::<ops::PyRemoveResult>()?;
    m.add_class::<ops::PyFailedTransfer>()?;

    // Register functions
    m.add_function(wrap_pyfunction!(sync::sync, m)?)?;
    m.add_function(wrap_pyfunction!(sync::sync_with_options, m)?)?;
    m.add_function(wrap_pyfunction!(path::parse_path, m)?)?;
    m.add_function(wrap_pyfunction!(ls::ls, m)?)?;

    // Register ops functions (get, put, rm)
    m.add_function(wrap_pyfunction!(ops::get, m)?)?;
    m.add_function(wrap_pyfunction!(ops::get_with_options, m)?)?;
    m.add_function(wrap_pyfunction!(ops::put, m)?)?;
    m.add_function(wrap_pyfunction!(ops::put_with_options, m)?)?;
    m.add_function(wrap_pyfunction!(ops::rm, m)?)?;
    m.add_function(wrap_pyfunction!(ops::rm_with_options, m)?)?;

    // Register daemon functions
    m.add_function(wrap_pyfunction!(daemon::py_daemon_start, m)?)?;
    m.add_function(wrap_pyfunction!(daemon::py_daemon_check, m)?)?;
    m.add_function(wrap_pyfunction!(daemon::py_daemon_stop, m)?)?;

    // Register CLI functions
    m.add_function(wrap_pyfunction!(cli::main, m)?)?;
    m.add_function(wrap_pyfunction!(cli::run_server, m)?)?;
    m.add_function(wrap_pyfunction!(cli::run_daemon, m)?)?;
    m.add_function(wrap_pyfunction!(cli::remote_main, m)?)?;

    // Add version info
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}
