//! Shared types and utilities for cloud storage transports (S3, GCS)
//!
//! This module contains configuration that is common to all cloud storage
//! backends, avoiding duplication and ensuring consistent behavior.

use object_store::{ClientOptions, RetryConfig};
use std::time::Duration;

/// HTTP client options for cloud storage transports (S3, GCS)
///
/// These options control connection pooling, timeouts, and retry behavior
/// for HTTP requests to cloud storage services. They apply to both S3
/// and GCS transports.
///
/// # Performance Tuning
///
/// For high-throughput scenarios (many parallel transfers):
/// - Increase `pool_max_idle_per_host` to match your parallelism setting
/// - Use longer `pool_idle_timeout_secs` to reduce connection establishment overhead
/// - Increase `request_timeout_secs` for large file uploads
///
/// For low-latency scenarios (interactive use):
/// - Use smaller `pool_max_idle_per_host` to reduce memory usage
/// - Use shorter timeouts for faster failure detection
/// - Use fewer retries for faster feedback
///
/// # Example
///
/// ```rust
/// use sy::transport::CloudClientOptions;
///
/// // Default options (balanced for most use cases)
/// let options = CloudClientOptions::default();
///
/// // High throughput preset
/// let options = CloudClientOptions::high_throughput();
///
/// // Custom configuration
/// let options = CloudClientOptions {
///     pool_max_idle_per_host: 100,
///     request_timeout_secs: 120,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CloudClientOptions {
    /// Maximum number of idle connections per host in the connection pool.
    ///
    /// Higher values allow more parallel requests but use more memory.
    /// Should typically match your `--parallel` setting.
    ///
    /// Default: 50
    pub pool_max_idle_per_host: usize,

    /// How long to keep idle connections alive in the pool (seconds).
    ///
    /// Longer times reduce connection establishment overhead for repeated
    /// requests but use more memory for idle connections.
    ///
    /// Default: 30 seconds
    pub pool_idle_timeout_secs: u64,

    /// Timeout for establishing a new connection (seconds).
    ///
    /// Should be short to fail fast and allow the retry logic to try
    /// alternative connections quickly.
    ///
    /// Default: 5 seconds
    pub connect_timeout_secs: u64,

    /// Timeout for the entire request including data transfer (seconds).
    ///
    /// Should be generous for large file uploads. Small files typically
    /// complete well under this limit; large files use multipart upload
    /// which has per-part timeouts.
    ///
    /// Default: 60 seconds
    pub request_timeout_secs: u64,

    /// Maximum number of retry attempts for failed requests.
    ///
    /// Retries use exponential backoff. Set to 0 to disable retries.
    ///
    /// Default: 3
    pub max_retries: usize,

    /// Maximum total time to spend retrying a request (seconds).
    ///
    /// After this timeout, the request fails even if retry attempts remain.
    /// This prevents a single slow request from blocking the entire sync.
    ///
    /// Default: 15 seconds
    pub retry_timeout_secs: u64,

    /// Whether to allow HTTP (non-TLS) connections.
    ///
    /// Should only be enabled for local testing with services like
    /// MinIO or LocalStack that don't require TLS.
    ///
    /// Default: false (HTTPS only)
    pub allow_http: bool,
}

impl Default for CloudClientOptions {
    fn default() -> Self {
        Self {
            pool_max_idle_per_host: 50,
            pool_idle_timeout_secs: 30,
            connect_timeout_secs: 5,
            request_timeout_secs: 60,
            max_retries: 3,
            retry_timeout_secs: 15,
            allow_http: false,
        }
    }
}

impl CloudClientOptions {
    /// Create new client options with sensible defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure for high-throughput scenarios (many parallel transfers)
    ///
    /// Optimized for:
    /// - Large connection pool (100 connections)
    /// - Longer idle timeout (60s)
    /// - Longer request timeout (120s)
    /// - More generous retry timeout (30s)
    pub fn high_throughput() -> Self {
        Self {
            pool_max_idle_per_host: 100,
            pool_idle_timeout_secs: 60,
            connect_timeout_secs: 5,
            request_timeout_secs: 120,
            max_retries: 3,
            retry_timeout_secs: 30,
            allow_http: false,
        }
    }

    /// Configure for low-latency scenarios (interactive use)
    ///
    /// Optimized for:
    /// - Smaller connection pool (20 connections)
    /// - Shorter idle timeout (15s)
    /// - Shorter request timeout (30s)
    /// - Faster failure detection (fewer retries, shorter retry timeout)
    pub fn low_latency() -> Self {
        Self {
            pool_max_idle_per_host: 20,
            pool_idle_timeout_secs: 15,
            connect_timeout_secs: 3,
            request_timeout_secs: 30,
            max_retries: 2,
            retry_timeout_secs: 10,
            allow_http: false,
        }
    }

    /// Configure for fast listing operations (metadata-only, non-recursive)
    ///
    /// Optimized for:
    /// - Very short timeouts for fast fail (2s connect, 10s request)
    /// - No retries (listings should be fast or fail)
    /// - Smaller pool (listing is typically sequential)
    pub fn fast_list() -> Self {
        Self {
            pool_max_idle_per_host: 10,
            pool_idle_timeout_secs: 30,
            connect_timeout_secs: 2,
            request_timeout_secs: 10,
            max_retries: 0,
            retry_timeout_secs: 5,
            allow_http: false,
        }
    }

    /// Build object_store `ClientOptions` from this configuration
    ///
    /// The `max_connections` parameter allows scaling the pool size based
    /// on the parallelism setting, taking the larger of `max_connections`
    /// and `pool_max_idle_per_host`.
    pub(crate) fn to_client_options(&self, max_connections: usize) -> ClientOptions {
        let pool_size = max_connections.max(self.pool_max_idle_per_host);

        let mut options = ClientOptions::new()
            .with_pool_max_idle_per_host(pool_size)
            .with_pool_idle_timeout(Duration::from_secs(self.pool_idle_timeout_secs))
            .with_connect_timeout(Duration::from_secs(self.connect_timeout_secs))
            .with_timeout(Duration::from_secs(self.request_timeout_secs));

        if self.allow_http {
            options = options.with_allow_http(true);
        }

        options
    }

    /// Build object_store `RetryConfig` from this configuration
    pub(crate) fn to_retry_config(&self) -> RetryConfig {
        RetryConfig {
            max_retries: self.max_retries,
            retry_timeout: Duration::from_secs(self.retry_timeout_secs),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let opts = CloudClientOptions::default();
        assert_eq!(opts.pool_max_idle_per_host, 50);
        assert_eq!(opts.pool_idle_timeout_secs, 30);
        assert_eq!(opts.connect_timeout_secs, 5);
        assert_eq!(opts.request_timeout_secs, 60);
        assert_eq!(opts.max_retries, 3);
        assert_eq!(opts.retry_timeout_secs, 15);
        assert!(!opts.allow_http);
    }

    #[test]
    fn test_high_throughput_preset() {
        let opts = CloudClientOptions::high_throughput();
        assert_eq!(opts.pool_max_idle_per_host, 100);
        assert_eq!(opts.request_timeout_secs, 120);
    }

    #[test]
    fn test_low_latency_preset() {
        let opts = CloudClientOptions::low_latency();
        assert_eq!(opts.pool_max_idle_per_host, 20);
        assert_eq!(opts.max_retries, 2);
    }

    #[test]
    fn test_to_retry_config() {
        let opts = CloudClientOptions {
            max_retries: 5,
            retry_timeout_secs: 20,
            ..Default::default()
        };
        let retry = opts.to_retry_config();
        assert_eq!(retry.max_retries, 5);
        assert_eq!(retry.retry_timeout, Duration::from_secs(20));
    }
}
