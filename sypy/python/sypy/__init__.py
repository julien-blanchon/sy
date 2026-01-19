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

        # Dry-run with typed summary
    >>> stats = sypy.sync("/source", "/dest", dry_run=True)
    >>> summary = stats.dry_run_summary
    >>> print(f"Would create {summary['would_create']['count']} files")

"""

from typing import TypedDict


class DryRunChange(TypedDict):
    """Details about changes that would be made"""

    count: int
    bytes: int


class DryRunSummary(TypedDict):
    """Summary of what would happen in a dry-run"""

    would_create: DryRunChange
    would_update: DryRunChange
    would_delete: DryRunChange
    total_files: int
    total_bytes: int


# Import TypedDict for ls functionality
from sypy._sypy import (
    # Dry-run classes
    ChangeAction,
    # Client options
    CloudClientOptions,
    DirectoryChange,
    DryRunDetails,
    FileChange,
    # Config classes
    GcsConfig,
    # List classes
    ListEntry,
    S3Config,
    SshConfig,
    SymlinkChange,
    # Classes
    SyncError,
    SyncOptions,
    SyncPath,
    SyncStats,
    # Version
    __version__,
    # Functions
    ls,
    # CLI functions
    main,
    parse_path,
    run_daemon,
    run_server,
    sync,
    sync_with_options,
)
from sypy.ls_types import ListEntryDict

__all__ = [
    # Dry-run classes
    "ChangeAction",
    # Client options
    "CloudClientOptions",
    "DirectoryChange",
    # TypedDicts
    "DryRunChange",
    "DryRunDetails",
    "DryRunSummary",
    "FileChange",
    # Config classes
    "GcsConfig",
    # List classes
    "ListEntry",
    "ListEntryDict",
    "S3Config",
    "SshConfig",
    "SymlinkChange",
    # Classes
    "SyncError",
    "SyncOptions",
    "SyncPath",
    "SyncStats",
    # Version
    "__version__",
    # Functions
    "ls",
    # CLI functions
    "main",
    "parse_path",
    "run_daemon",
    "run_server",
    "sync",
    "sync_with_options",
]
