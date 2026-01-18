from collections.abc import Callable

__version__: str

class CloudClientOptions:
    pool_max_idle_per_host: int
    """Maximum idle connections per host. Default: 50."""

    pool_idle_timeout_secs: int
    """How long to keep idle connections (seconds). Default: 30."""

    connect_timeout_secs: int
    """Connection timeout (seconds). Default: 5."""

    request_timeout_secs: int
    """Request timeout including transfer (seconds). Default: 60."""

    max_retries: int
    """Maximum retry attempts. Default: 3."""

    retry_timeout_secs: int
    """Maximum time for retries (seconds). Default: 15."""

    allow_http: bool
    """Allow HTTP (non-TLS) connections. Default: False."""

    def __init__(
        self,
        pool_max_idle_per_host: int = 50,
        pool_idle_timeout_secs: int = 30,
        connect_timeout_secs: int = 5,
        request_timeout_secs: int = 60,
        max_retries: int = 3,
        retry_timeout_secs: int = 15,
        allow_http: bool = False,
    ) -> None: ...
    @staticmethod
    def high_throughput() -> CloudClientOptions: ...
    @staticmethod
    def low_latency() -> CloudClientOptions: ...

class S3Config:
    access_key_id: str | None
    """AWS access key ID."""

    secret_access_key: str | None
    """AWS secret access key."""

    session_token: str | None
    """AWS session token (for temporary credentials)."""

    region: str | None
    """AWS region (e.g., "us-east-1")."""

    endpoint: str | None
    """Custom endpoint URL for S3-compatible services."""

    profile: str | None
    """AWS profile name to use from ~/.aws/credentials."""

    client_options: CloudClientOptions | None
    """HTTP client options (timeouts, retries, connection pool)."""

    def __init__(
        self,
        access_key_id: str | None = None,
        secret_access_key: str | None = None,
        session_token: str | None = None,
        region: str | None = None,
        endpoint: str | None = None,
        profile: str | None = None,
        client_options: CloudClientOptions | None = None,
    ) -> None: ...

class GcsConfig:
    credentials_file: str | None
    """Path to service account JSON key file."""

    project_id: str | None
    """GCP project ID."""

    credentials_json: str | None
    """Service account JSON as a string (alternative to credentials_file)."""

    client_options: CloudClientOptions | None
    """HTTP client options (timeouts, retries, connection pool)."""

    def __init__(
        self,
        credentials_file: str | None = None,
        project_id: str | None = None,
        credentials_json: str | None = None,
        client_options: CloudClientOptions | None = None,
    ) -> None: ...

class SshConfig:
    key_file: str | None
    """Path to private key file."""

    port: int | None
    """SSH port (default: 22)."""

    password: str | None
    """SSH password (usually not needed with key authentication)."""

    compression: bool
    """Enable compression for SSH connection."""

    proxy_jump: str | None
    """Proxy jump host (e.g., "bastion@proxy.example.com")."""

    connect_timeout: int | None
    """Connection timeout in seconds."""

    pool_size: int | None
    """Number of parallel SSH connections."""

    def __init__(
        self,
        key_file: str | None = None,
        port: int | None = None,
        password: str | None = None,
        compression: bool = False,
        proxy_jump: str | None = None,
        connect_timeout: int | None = None,
        pool_size: int | None = None,
    ) -> None: ...

class SyncError:
    path: str
    """Path that caused the error."""

    error: str
    """Error message."""

    action: str
    """Action that was being performed."""

class SyncStats:
    files_scanned: int
    """Number of files scanned."""

    files_created: int
    """Number of files created."""

    files_updated: int
    """Number of files updated."""

    files_skipped: int
    """Number of files skipped (already up-to-date)."""

    files_deleted: int
    """Number of files deleted."""

    bytes_transferred: int
    """Total bytes transferred."""

    files_delta_synced: int
    """Number of files synced using delta algorithm."""

    delta_bytes_saved: int
    """Bytes saved by delta sync."""

    files_compressed: int
    """Number of files compressed during transfer."""

    compression_bytes_saved: int
    """Bytes saved by compression."""

    files_verified: int
    """Number of files verified."""

    verification_failures: int
    """Number of verification failures."""

    duration_secs: float
    """Duration of the sync operation in seconds."""

    bytes_would_add: int
    """Bytes that would be added (dry-run only)."""

    bytes_would_change: int
    """Bytes that would change (dry-run only)."""

    bytes_would_delete: int
    """Bytes that would be deleted (dry-run only)."""

    dirs_created: int
    """Number of directories created."""

    symlinks_created: int
    """Number of symlinks created."""

    @property
    def errors(self) -> list[SyncError]: ...
    @property
    def success(self) -> bool: ...
    @property
    def transfer_rate(self) -> float: ...

class SyncPath:
    def __init__(self, path: str) -> None: ...
    @property
    def path(self) -> str: ...
    @property
    def is_local(self) -> bool: ...
    @property
    def is_remote(self) -> bool: ...
    @property
    def is_s3(self) -> bool: ...
    @property
    def is_gcs(self) -> bool: ...
    @property
    def is_daemon(self) -> bool: ...
    @property
    def has_trailing_slash(self) -> bool: ...
    @property
    def host(self) -> str | None: ...
    @property
    def user(self) -> str | None: ...
    @property
    def bucket(self) -> str | None: ...

class SyncOptions:
    dry_run: bool
    """Dry run mode - show changes without applying."""

    delete: bool
    """Delete files in destination not present in source."""

    delete_threshold: int
    """Maximum percentage of files that can be deleted (0-100)."""

    trash: bool
    """Move deleted files to trash instead of permanent deletion."""

    force_delete: bool
    """Skip deletion safety checks."""

    parallel: int
    """Number of parallel file transfers."""

    max_errors: int
    """Maximum number of errors before aborting (0 = unlimited)."""

    min_size: str | None
    """Minimum file size to sync (e.g., "1MB")."""

    max_size: str | None
    """Maximum file size to sync (e.g., "1GB")."""

    exclude: list[str]
    """Exclude patterns."""

    include: list[str]
    """Include patterns."""

    bwlimit: str | None
    """Bandwidth limit (e.g., "10MB")."""

    resume: bool
    """Enable resume support for interrupted transfers."""

    verify: bool
    """Verify file integrity after write."""

    compress: bool
    """Enable compression for network transfers."""

    preserve_xattrs: bool
    """Preserve extended attributes."""

    preserve_hardlinks: bool
    """Preserve hard links."""

    preserve_acls: bool
    """Preserve access control lists."""

    preserve_permissions: bool
    """Preserve permissions."""

    preserve_times: bool
    """Preserve modification times."""

    ignore_times: bool
    """Ignore modification times, always compare checksums."""

    size_only: bool
    """Only compare file size, skip mtime checks."""

    checksum: bool
    """Always compare checksums instead of size+mtime."""

    update: bool
    """Skip files where destination is newer."""

    ignore_existing: bool
    """Skip files that already exist in destination."""

    gitignore: bool
    """Apply .gitignore rules."""

    exclude_vcs: bool
    """Exclude .git directories."""

    bidirectional: bool
    """Bidirectional sync mode."""

    conflict_resolve: str
    """Conflict resolution strategy ("newer", "larger", "smaller", "source", "dest", "rename")."""

    daemon_auto: bool
    """Use daemon mode for fast repeated syncs."""

    retry: int
    """Maximum retry attempts for network operations."""

    retry_delay: int
    """Initial delay between retries in seconds."""

    s3: S3Config | None
    """S3 configuration for S3/S3-compatible storage."""

    gcs: GcsConfig | None
    """GCS configuration for Google Cloud Storage."""

    ssh: SshConfig | None
    """SSH configuration for remote connections."""

    def __init__(
        self,
        dry_run: bool = False,
        delete: bool = False,
        delete_threshold: int = 50,
        trash: bool = False,
        force_delete: bool = False,
        parallel: int = 10,
        max_errors: int = 100,
        min_size: str | None = None,
        max_size: str | None = None,
        exclude: list[str] | None = None,
        include: list[str] | None = None,
        bwlimit: str | None = None,
        resume: bool = True,
        verify: bool = False,
        compress: bool = False,
        preserve_xattrs: bool = False,
        preserve_hardlinks: bool = False,
        preserve_acls: bool = False,
        preserve_permissions: bool = False,
        preserve_times: bool = False,
        ignore_times: bool = False,
        size_only: bool = False,
        checksum: bool = False,
        update: bool = False,
        ignore_existing: bool = False,
        gitignore: bool = False,
        exclude_vcs: bool = False,
        bidirectional: bool = False,
        conflict_resolve: str = "newer",
        daemon_auto: bool = False,
        retry: int = 3,
        retry_delay: int = 1,
        s3: S3Config | None = None,
        gcs: GcsConfig | None = None,
        ssh: SshConfig | None = None,
    ) -> None: ...

# Type alias for progress callback
type ProgressCallback = Callable[[int, int, str, str], None]
"""
Progress callback function signature.

Args:
    current: Current progress count
    total: Total count
    path: Current file path
    action: Current action ("scanning", "creating", "updating", etc.)
"""

def sync(
    source: str,
    dest: str,
    *,
    dry_run: bool = False,
    delete: bool = False,
    delete_threshold: int = 50,
    parallel: int = 10,
    verify: bool = False,
    compress: bool = False,
    checksum: bool = False,
    exclude: list[str] | None = None,
    include: list[str] | None = None,
    min_size: str | None = None,
    max_size: str | None = None,
    bwlimit: str | None = None,
    progress_callback: ProgressCallback | None = None,
    daemon_auto: bool = False,
    resume: bool = True,
    ignore_times: bool = False,
    size_only: bool = False,
    update: bool = False,
    ignore_existing: bool = False,
    gitignore: bool = False,
    exclude_vcs: bool = False,
    preserve_xattrs: bool = False,
    preserve_hardlinks: bool = False,
    preserve_permissions: bool = False,
    preserve_times: bool = False,
    retry: int = 3,
    retry_delay: int = 1,
    s3: S3Config | None = None,
    gcs: GcsConfig | None = None,
    ssh: SshConfig | None = None,
) -> SyncStats: ...
def sync_with_options(
    source: str,
    dest: str,
    options: SyncOptions,
    progress_callback: ProgressCallback | None = None,
) -> SyncStats: ...
def parse_path(path: str) -> SyncPath: ...

# CLI functions
def main(args: list[str] | None = None) -> int: ...
def run_server(path: str) -> None: ...
def run_daemon(socket_path: str, root_path: str | None = None) -> None: ...
