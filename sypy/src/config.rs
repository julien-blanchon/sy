//! Configuration classes for cloud and SSH credentials
//!
//! These provide a Pythonic way to pass credentials instead of using environment variables.

use pyo3::prelude::*;
use std::path::PathBuf;

/// HTTP client options for cloud storage (S3, GCS)
///
/// Controls connection pooling, timeouts, and retry behavior for HTTP requests.
/// These settings can significantly impact performance for parallel transfers.
///
/// Example:
///     >>> # Create high-throughput options for many parallel transfers
///     >>> options = CloudClientOptions(
///     ...     pool_max_idle_per_host=100,
///     ...     pool_idle_timeout_secs=60,
///     ...     request_timeout_secs=120,
///     ...     max_retries=3,
///     ... )
///     >>> s3 = S3Config(
///     ...     access_key_id="...",
///     ...     secret_access_key="...",
///     ...     client_options=options,
///     ... )
///     >>> stats = sync("/local/", "s3://bucket/", s3=s3, parallel=100)
#[pyclass(name = "CloudClientOptions")]
#[derive(Clone, Debug)]
pub struct PyCloudClientOptions {
    /// Maximum idle connections per host in the connection pool.
    /// Higher = more parallelism, but uses more memory.
    /// Default: 50
    #[pyo3(get, set)]
    pub pool_max_idle_per_host: usize,

    /// How long to keep idle connections alive (seconds).
    /// Longer = less connection overhead for repeated requests.
    /// Default: 30
    #[pyo3(get, set)]
    pub pool_idle_timeout_secs: u64,

    /// Timeout for establishing a connection (seconds).
    /// Should be short to fail fast and allow retries.
    /// Default: 5
    #[pyo3(get, set)]
    pub connect_timeout_secs: u64,

    /// Timeout for the entire request including transfer (seconds).
    /// Should be generous for large file uploads.
    /// Default: 60
    #[pyo3(get, set)]
    pub request_timeout_secs: u64,

    /// Maximum retry attempts for failed requests.
    /// Uses exponential backoff between retries.
    /// Default: 3
    #[pyo3(get, set)]
    pub max_retries: usize,

    /// Maximum time to spend retrying a request (seconds).
    /// After this, the request fails even if retries remain.
    /// Default: 15
    #[pyo3(get, set)]
    pub retry_timeout_secs: u64,

    /// Allow HTTP (non-TLS) connections.
    /// Only enable for local testing (MinIO, LocalStack).
    /// Default: False
    #[pyo3(get, set)]
    pub allow_http: bool,
}

impl Default for PyCloudClientOptions {
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

#[pymethods]
impl PyCloudClientOptions {
    #[new]
    #[pyo3(signature = (
        pool_max_idle_per_host = 50,
        pool_idle_timeout_secs = 30,
        connect_timeout_secs = 5,
        request_timeout_secs = 60,
        max_retries = 3,
        retry_timeout_secs = 15,
        allow_http = false
    ))]
    pub fn new(
        pool_max_idle_per_host: usize,
        pool_idle_timeout_secs: u64,
        connect_timeout_secs: u64,
        request_timeout_secs: u64,
        max_retries: usize,
        retry_timeout_secs: u64,
        allow_http: bool,
    ) -> Self {
        Self {
            pool_max_idle_per_host,
            pool_idle_timeout_secs,
            connect_timeout_secs,
            request_timeout_secs,
            max_retries,
            retry_timeout_secs,
            allow_http,
        }
    }

    /// Create options optimized for high-throughput (many parallel transfers)
    #[staticmethod]
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

    /// Create options optimized for low-latency (interactive use)
    #[staticmethod]
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

    fn __repr__(&self) -> String {
        format!(
            "CloudClientOptions(pool={}, timeout={}s, retries={})",
            self.pool_max_idle_per_host, self.request_timeout_secs, self.max_retries
        )
    }
}

impl PyCloudClientOptions {
    /// Convert to sy's CloudClientOptions
    pub fn to_sy_options(&self) -> sy::transport::CloudClientOptions {
        sy::transport::CloudClientOptions {
            pool_max_idle_per_host: self.pool_max_idle_per_host,
            pool_idle_timeout_secs: self.pool_idle_timeout_secs,
            connect_timeout_secs: self.connect_timeout_secs,
            request_timeout_secs: self.request_timeout_secs,
            max_retries: self.max_retries,
            retry_timeout_secs: self.retry_timeout_secs,
            allow_http: self.allow_http,
        }
    }
}

/// S3 configuration for AWS S3 or S3-compatible services
///
/// Use this to provide explicit credentials instead of environment variables.
///
/// Example:
///     >>> # Basic usage
///     >>> s3 = S3Config(
///     ...     access_key_id="AKIAIOSFODNN7EXAMPLE",
///     ...     secret_access_key="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
///     ...     region="us-east-1",
///     ...     endpoint="https://sfo3.digitaloceanspaces.com",  # For DigitalOcean/R2/etc.
///     ... )
///     >>> stats = sync("/local/", "s3://bucket/path/", s3=s3)
///
///     >>> # With custom client options for high throughput
///     >>> s3 = S3Config(
///     ...     access_key_id="...",
///     ...     secret_access_key="...",
///     ...     client_options=CloudClientOptions.high_throughput(),
///     ... )
#[pyclass(name = "S3Config")]
#[derive(Clone, Debug, Default)]
pub struct PyS3Config {
    /// AWS access key ID
    #[pyo3(get, set)]
    pub access_key_id: Option<String>,

    /// AWS secret access key
    #[pyo3(get, set)]
    pub secret_access_key: Option<String>,

    /// AWS session token (for temporary credentials)
    #[pyo3(get, set)]
    pub session_token: Option<String>,

    /// AWS region (e.g., "us-east-1")
    #[pyo3(get, set)]
    pub region: Option<String>,

    /// Custom endpoint URL for S3-compatible services
    /// (DigitalOcean Spaces, Cloudflare R2, Backblaze B2, MinIO, etc.)
    #[pyo3(get, set)]
    pub endpoint: Option<String>,

    /// AWS profile name to use from ~/.aws/credentials
    #[pyo3(get, set)]
    pub profile: Option<String>,

    /// HTTP client options (timeouts, retries, connection pool)
    #[pyo3(get, set)]
    pub client_options: Option<PyCloudClientOptions>,
}

#[pymethods]
impl PyS3Config {
    #[new]
    #[pyo3(signature = (
        access_key_id = None,
        secret_access_key = None,
        session_token = None,
        region = None,
        endpoint = None,
        profile = None,
        client_options = None
    ))]
    pub fn new(
        access_key_id: Option<String>,
        secret_access_key: Option<String>,
        session_token: Option<String>,
        region: Option<String>,
        endpoint: Option<String>,
        profile: Option<String>,
        client_options: Option<PyCloudClientOptions>,
    ) -> Self {
        PyS3Config {
            access_key_id,
            secret_access_key,
            session_token,
            region,
            endpoint,
            profile,
            client_options,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "S3Config(region={:?}, endpoint={:?}, profile={:?})",
            self.region, self.endpoint, self.profile
        )
    }

    /// Apply this configuration to environment variables
    pub fn apply_to_env(&self) {
        if let Some(ref key) = self.access_key_id {
            std::env::set_var("AWS_ACCESS_KEY_ID", key);
        }
        if let Some(ref secret) = self.secret_access_key {
            std::env::set_var("AWS_SECRET_ACCESS_KEY", secret);
        }
        if let Some(ref token) = self.session_token {
            std::env::set_var("AWS_SESSION_TOKEN", token);
        }
        if let Some(ref region) = self.region {
            std::env::set_var("AWS_REGION", region);
        }
        if let Some(ref endpoint) = self.endpoint {
            std::env::set_var("AWS_ENDPOINT_URL", endpoint);
        }
        if let Some(ref profile) = self.profile {
            std::env::set_var("AWS_PROFILE", profile);
        }
    }
}

impl PyS3Config {
    /// Convert to sy's S3Config
    pub fn to_sy_config(&self) -> sy::transport::s3::S3Config {
        sy::transport::s3::S3Config {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            region: self.region.clone(),
            endpoint: self.endpoint.clone(),
            profile: self.profile.clone(),
            client_options: self.client_options.as_ref().map(|o| o.to_sy_options()),
        }
    }
}

/// GCS configuration for Google Cloud Storage
///
/// Use this to provide explicit credentials instead of environment variables.
///
/// Example:
///     >>> # Basic usage
///     >>> gcs = GcsConfig(
///     ...     credentials_file="/path/to/service-account.json",
///     ...     project_id="my-project",
///     ... )
///     >>> stats = sync("/local/", "gs://bucket/path/", gcs=gcs)
///
///     >>> # With custom client options
///     >>> gcs = GcsConfig(
///     ...     credentials_file="/path/to/key.json",
///     ...     client_options=CloudClientOptions.high_throughput(),
///     ... )
#[pyclass(name = "GcsConfig")]
#[derive(Clone, Debug, Default)]
pub struct PyGcsConfig {
    /// Path to service account JSON key file
    #[pyo3(get, set)]
    pub credentials_file: Option<String>,

    /// GCP project ID
    #[pyo3(get, set)]
    pub project_id: Option<String>,

    /// Service account JSON as a string (alternative to credentials_file)
    #[pyo3(get, set)]
    pub credentials_json: Option<String>,

    /// HTTP client options (timeouts, retries, connection pool)
    #[pyo3(get, set)]
    pub client_options: Option<PyCloudClientOptions>,
}

#[pymethods]
impl PyGcsConfig {
    #[new]
    #[pyo3(signature = (
        credentials_file = None,
        project_id = None,
        credentials_json = None,
        client_options = None
    ))]
    pub fn new(
        credentials_file: Option<String>,
        project_id: Option<String>,
        credentials_json: Option<String>,
        client_options: Option<PyCloudClientOptions>,
    ) -> Self {
        PyGcsConfig {
            credentials_file,
            project_id,
            credentials_json,
            client_options,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "GcsConfig(credentials_file={:?}, project_id={:?})",
            self.credentials_file, self.project_id
        )
    }

    /// Apply this configuration to environment variables
    pub fn apply_to_env(&self) {
        if let Some(ref creds) = self.credentials_file {
            std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", creds);
        }
        if let Some(ref project) = self.project_id {
            std::env::set_var("GCP_PROJECT_ID", project);
        }
        // For credentials_json, we'd need to write to a temp file
        // or modify the GCS transport to accept it directly
    }
}

impl PyGcsConfig {
    /// Convert to sy's GcsConfig
    pub fn to_sy_config(&self) -> sy::transport::gcs::GcsConfig {
        sy::transport::gcs::GcsConfig {
            credentials_file: self.credentials_file.clone(),
            project_id: self.project_id.clone(),
            client_options: self.client_options.as_ref().map(|o| o.to_sy_options()),
        }
    }
}

/// SSH configuration for remote connections
///
/// Use this to provide explicit SSH options instead of relying on ~/.ssh/config.
///
/// Example:
///     >>> ssh = SshConfig(
///     ...     key_file="/path/to/private/key",
///     ...     port=22,
///     ...     password="optional_password",  # Usually not needed with key auth
///     ... )
///     >>> stats = sync("/local/", "user@host:/path/", ssh=ssh)
#[pyclass(name = "SshConfig")]
#[derive(Clone, Debug, Default)]
pub struct PySshConfig {
    /// Path to private key file
    #[pyo3(get, set)]
    pub key_file: Option<String>,

    /// SSH port (default: 22)
    #[pyo3(get, set)]
    pub port: Option<u16>,

    /// SSH password (usually not needed with key authentication)
    #[pyo3(get, set)]
    pub password: Option<String>,

    /// Enable compression for SSH connection
    #[pyo3(get, set)]
    pub compression: bool,

    /// Proxy jump host (e.g., "bastion@proxy.example.com")
    #[pyo3(get, set)]
    pub proxy_jump: Option<String>,

    /// Connection timeout in seconds
    #[pyo3(get, set)]
    pub connect_timeout: Option<u64>,

    /// Number of parallel SSH connections
    #[pyo3(get, set)]
    pub pool_size: Option<usize>,
}

#[pymethods]
impl PySshConfig {
    #[new]
    #[pyo3(signature = (
        key_file = None,
        port = None,
        password = None,
        compression = false,
        proxy_jump = None,
        connect_timeout = None,
        pool_size = None
    ))]
    pub fn new(
        key_file: Option<String>,
        port: Option<u16>,
        password: Option<String>,
        compression: bool,
        proxy_jump: Option<String>,
        connect_timeout: Option<u64>,
        pool_size: Option<usize>,
    ) -> Self {
        PySshConfig {
            key_file,
            port,
            password,
            compression,
            proxy_jump,
            connect_timeout,
            pool_size,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SshConfig(port={:?}, key_file={:?}, compression={})",
            self.port, self.key_file, self.compression
        )
    }
}

impl PySshConfig {
    /// Convert to sy's SshConfig (internal helper, not exposed to Python)
    #[cfg(feature = "ssh")]
    pub fn to_sy_ssh_config(&self, hostname: &str, user: &str) -> sy::ssh::config::SshConfig {
        let mut config = sy::ssh::config::SshConfig::new(hostname);
        config.user = user.to_string();

        if let Some(port) = self.port {
            config.port = port;
        }
        if let Some(ref key) = self.key_file {
            config.identity_file = vec![PathBuf::from(key)];
        }
        if let Some(ref proxy) = self.proxy_jump {
            config.proxy_jump = Some(proxy.clone());
        }
        config.compression = self.compression;

        config
    }
}
