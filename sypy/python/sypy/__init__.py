"""
sypy - Python bindings for sy file synchronization tool.

Fast, modern file synchronization with support for local, SSH, S3, and GCS.

Example:
    >>> import sypy
    >>> stats = sypy.sync("/source", "/dest")
    >>> print(f"Synced {stats.files_created} files")

    # With options
    >>> stats = sypy.sync(
    ...     "/source", "/dest",
    ...     dry_run=True,
    ...     exclude=["*.log", "node_modules"],
    ... )

    # With S3 credentials
    >>> s3 = sypy.S3Config(
    ...     access_key_id="...",
    ...     secret_access_key="...",
    ...     region="us-east-1",
    ... )
    >>> stats = sypy.sync("/local/", "s3://bucket/path/", s3=s3)

    # With GCS credentials
    >>> gcs = sypy.GcsConfig(credentials_file="/path/to/key.json")
    >>> stats = sypy.sync("/local/", "gs://bucket/path/", gcs=gcs)

    # With custom client options for high throughput
    >>> options = sypy.CloudClientOptions.high_throughput()
    >>> s3 = sypy.S3Config(..., client_options=options)
    >>> stats = sypy.sync("/local/", "s3://bucket/", s3=s3, parallel=100)

"""

from sypy._sypy import (
    # Client options
    CloudClientOptions,
    # Config classes
    GcsConfig,
    S3Config,
    SshConfig,
    # Classes
    SyncError,
    SyncOptions,
    SyncPath,
    SyncStats,
    # Version
    __version__,
    # Functions
    parse_path,
    sync,
    sync_with_options,
)

__all__ = [
    # Client options
    "CloudClientOptions",
    # Config classes
    "GcsConfig",
    "S3Config",
    "SshConfig",
    # Classes
    "SyncError",
    "SyncOptions",
    "SyncPath",
    "SyncStats",
    # Version
    "__version__",
    # Functions
    "parse_path",
    "sync",
    "sync_with_options",
]
