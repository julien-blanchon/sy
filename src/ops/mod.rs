//! File operations module - get, put, rm
//!
//! This module provides reusable functions for file operations across different transports
//! (local, SSH, S3, GCS). These are used by both CLI binaries and Python bindings.

pub mod get;
pub mod put;
pub mod rm;

pub use get::{download, DownloadOptions, DownloadResult, FailedDownload};
pub use put::{upload, FailedUpload, UploadOptions, UploadResult};
pub use rm::{remove, FailedRemove, RemoveOptions, RemoveResult};
