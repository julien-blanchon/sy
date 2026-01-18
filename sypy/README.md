# sypy - Python Bindings for sy

Fast, modern file synchronization in Python - powered by [sy](https://github.com/nijaru/sy), a Rust reimagining of rsync.

## Features

- **Fast**: 2-11x faster than rsync for local syncs
- **Cross-platform**: Works on macOS, Linux, and Windows
- **Multiple backends**: Local, SSH, S3, and GCS support
- **Rich options**: Delta sync, compression, verification, and more
- **Pythonic API**: Type hints, callbacks, and intuitive interface

## Installation

```bash
pip install sy-python
```

Or with [uv](https://docs.astral.sh/uv/):

```bash
uv pip install sy-python
```

Or build from source:

```bash
cd sypy
pip install maturin
maturin develop
```

## Command Line Interface

The `sy-python` package includes the `sy` CLI, making it easy to use from the command line or with tools like `uvx`:

```bash
# After installation, use sy directly
sy /source /destination

# Or run as a Python module
python -m sypy /source /destination

# Run with uvx (no installation needed)
uvx --from sy-python sy /source /destination

# Show help
sy --help
```

### Server & Daemon Modes

For SSH remote syncs, `sy-python` can act as the remote server:

```bash
# Server mode (used internally by SSH transport)
sy --server /path/to/serve

# Daemon mode (persistent server for fast repeated syncs)
sy --daemon --socket ~/.sy/daemon.sock
```

This means you only need `pip install sy-python` on both local and remote machines - no separate Rust binary installation required!

## Quick Start

```python
import sypy

# Basic local sync
stats = sypy.sync("/source/dir/", "/dest/dir/")
print(f"Synced {stats.files_created} files in {stats.duration_secs:.2f}s")

# SSH remote sync
stats = sypy.sync("/local/path/", "user@host:/remote/path/")

# S3 sync
stats = sypy.sync("/local/path/", "s3://bucket/prefix/")

# GCS sync
stats = sypy.sync("/local/path/", "gs://bucket/prefix/")
```

## Usage Examples

### Basic Sync

```python
import sypy

# Sync with trailing slash = sync contents
stats = sypy.sync("/source/", "/dest/")

# Without trailing slash = sync the directory itself
stats = sypy.sync("/source", "/dest/")  # Creates /dest/source/
```

### Dry Run

Preview changes without applying them:

```python
stats = sypy.sync("/source/", "/dest/", dry_run=True)
print(f"Would create {stats.files_created} files")
print(f"Would transfer {stats.bytes_transferred:,} bytes")
```

### Delete Mode

Remove files in destination that don't exist in source:

```python
from sypy import SyncOptions, sync_with_options

options = SyncOptions(
    delete=True,
    delete_threshold=100,  # Allow deleting up to 100% of files
)
stats = sync_with_options("/source/", "/dest/", options)
print(f"Deleted {stats.files_deleted} files")
```

### Exclude Patterns

```python
stats = sypy.sync(
    "/source/", "/dest/",
    exclude=["*.log", "*.tmp", "node_modules", "__pycache__"],
)
```

### SSH Remote Sync

```python
import sypy

# Basic SSH sync (uses ~/.ssh/config)
stats = sypy.sync("/local/path/", "user@host:/remote/path/")

# Download from remote
stats = sypy.sync("user@host:/remote/path/", "/local/path/")

# With explicit SSH configuration
ssh = sypy.SshConfig(
    key_file="/path/to/private/key",
    port=22,
    compression=True,
)
stats = sypy.sync("/local/", "user@host:/remote/", ssh=ssh)

# With daemon mode for faster repeated syncs
stats = sypy.sync("/local/", "user@host:/remote/", daemon_auto=True)
```

### S3 Sync (DigitalOcean Spaces, AWS, etc.)

```python
import sypy

# Use S3Config for explicit credentials (recommended)
s3 = sypy.S3Config(
    access_key_id="your_key",
    secret_access_key="your_secret",
    region="us-east-1",
    endpoint="https://sfo3.digitaloceanspaces.com",  # For DigitalOcean/R2/etc.
)

# Upload to S3
stats = sypy.sync("/local/", "s3://my-bucket/prefix/", s3=s3)

# Or use environment variables (also works)
import os
os.environ["AWS_ACCESS_KEY_ID"] = "your_key"
os.environ["AWS_SECRET_ACCESS_KEY"] = "your_secret"
stats = sypy.sync("/local/", "s3://my-bucket/prefix/")
```

### GCS Sync

```python
import sypy

# Use GcsConfig for explicit credentials (recommended)
gcs = sypy.GcsConfig(
    credentials_file="/path/to/service-account.json",
    project_id="my-project",  # Optional
)

stats = sypy.sync("/local/", "gs://my-bucket/prefix/", gcs=gcs)

# Or use environment variables
import os
os.environ["GOOGLE_APPLICATION_CREDENTIALS"] = "/path/to/credentials.json"
stats = sypy.sync("/local/", "gs://my-bucket/prefix/")
```

### HTTP Client Options (Performance Tuning)

For S3 and GCS, you can fine-tune HTTP client behavior to optimize performance:

```python
import sypy

# Use preset configurations
options = sypy.CloudClientOptions.high_throughput()  # For many parallel transfers
options = sypy.CloudClientOptions.low_latency()      # For interactive use

# Or customize individual settings
options = sypy.CloudClientOptions(
    pool_max_idle_per_host=100,    # Max idle connections per host (default: 50)
    pool_idle_timeout_secs=60,     # Keep idle connections for 60s (default: 30)
    connect_timeout_secs=5,        # Connection timeout (default: 5)
    request_timeout_secs=120,      # Request timeout (default: 60)
    max_retries=3,                 # Retry attempts (default: 3)
    retry_timeout_secs=30,         # Max retry time (default: 15)
    allow_http=False,              # Allow non-TLS (default: False)
)

# Apply to S3
s3 = sypy.S3Config(
    access_key_id="...",
    secret_access_key="...",
    region="us-east-1",
    client_options=options,
)
stats = sypy.sync("/local/", "s3://bucket/", s3=s3, parallel=100)

# Apply to GCS
gcs = sypy.GcsConfig(
    credentials_file="/path/to/key.json",
    client_options=sypy.CloudClientOptions.high_throughput(),
)
stats = sypy.sync("/local/", "gs://bucket/", gcs=gcs, parallel=100)
```

**Performance Tips:**
- For many small files: increase `pool_max_idle_per_host` and `parallel`
- For large files: increase `request_timeout_secs`
- For unreliable networks: increase `max_retries` and `retry_timeout_secs`
- For local testing (MinIO, LocalStack): set `allow_http=True`

### Advanced Options

```python
from sypy import SyncOptions, sync_with_options

options = SyncOptions(
    # Performance
    parallel=20,              # Parallel transfers
    
    # Safety
    delete=False,             # Don't delete extra files
    delete_threshold=50,      # Max 50% deletions allowed
    dry_run=False,            # Actually apply changes
    
    # Comparison
    checksum=False,           # Use size+mtime (faster)
    ignore_times=False,       # Compare mtimes
    size_only=False,          # Compare sizes only
    
    # Preservation
    preserve_permissions=True,
    preserve_times=True,
    preserve_xattrs=False,
    preserve_hardlinks=False,
    
    # Filtering
    exclude=["*.tmp"],
    include=[],
    min_size=None,            # e.g., "1MB"
    max_size=None,            # e.g., "1GB"
    gitignore=False,
    exclude_vcs=False,
    
    # Resume/Retry
    resume=True,
    retry=3,
    retry_delay=1,
    
    # Verification
    verify=False,             # Hash verification after write
    
    # SSH daemon mode
    daemon_auto=False,
)

stats = sync_with_options("/source/", "/dest/", options)
```

### Using SyncStats

```python
stats = sypy.sync("/source/", "/dest/")

# Basic stats
print(f"Files scanned: {stats.files_scanned}")
print(f"Files created: {stats.files_created}")
print(f"Files updated: {stats.files_updated}")
print(f"Files skipped: {stats.files_skipped}")
print(f"Files deleted: {stats.files_deleted}")

# Transfer stats
print(f"Bytes transferred: {stats.bytes_transferred:,}")
print(f"Duration: {stats.duration_secs:.2f}s")

# Delta sync stats
print(f"Delta synced: {stats.files_delta_synced}")
print(f"Delta bytes saved: {stats.delta_bytes_saved:,}")

# Verification
print(f"Files verified: {stats.files_verified}")
print(f"Verification failures: {stats.verification_failures}")

# Success/errors
print(f"Success: {stats.success}")
if stats.errors:
    for error in stats.errors:
        print(f"Error: {error}")
```

### Path Parsing

```python
from sypy import parse_path

# Local path
p = parse_path("/local/path")
assert p.is_local

# SSH remote
p = parse_path("user@host:/path")
assert p.is_remote
assert p.user == "user"
assert p.host == "host"

# S3
p = parse_path("s3://bucket/key")
assert p.is_s3
assert p.bucket == "bucket"

# GCS
p = parse_path("gs://bucket/key")
assert p.is_gcs
```

### Progress with tqdm

While native progress callbacks are planned, you can implement polling-based progress:

```python
import sypy
from tqdm import tqdm
import threading
import time

def sync_with_progress(source: str, dest: str):
    """Sync with a simple progress indicator."""
    result = [None]
    
    def do_sync():
        result[0] = sypy.sync(source, dest)
    
    thread = threading.Thread(target=do_sync)
    thread.start()
    
    with tqdm(desc="Syncing", unit="files", leave=True) as pbar:
        while thread.is_alive():
            time.sleep(0.1)
            pbar.update(0)  # Keep the bar alive
            pbar.refresh()
    
    thread.join()
    stats = result[0]
    
    print(f"\n✓ Synced {stats.files_created} files")
    return stats

# Usage
stats = sync_with_progress("/source/", "/dest/")
```

## API Reference

### Functions

#### `sync(source, dest, **options) -> SyncStats`

Synchronize files from source to destination.

#### `sync_with_options(source, dest, options) -> SyncStats`

Synchronize using a SyncOptions object.

#### `parse_path(path) -> SyncPath`

Parse a path string into a SyncPath object.

### Classes

#### `SyncOptions`

Configuration for sync operations. All options have sensible defaults.

#### `SyncStats`

Statistics from a sync operation.

#### `SyncPath`

Represents a parsed sync path (local, SSH, S3, or GCS).

### Configuration Classes

#### `S3Config`

Configuration for AWS S3 and S3-compatible services.

```python
s3 = S3Config(
    access_key_id="...",        # AWS access key
    secret_access_key="...",    # AWS secret key
    session_token="...",        # Optional: for temporary credentials
    region="us-east-1",         # AWS region
    endpoint="...",             # Custom endpoint for S3-compatible services
    profile="default",          # AWS profile from ~/.aws/credentials
)
```

Supported services: AWS S3, DigitalOcean Spaces, Cloudflare R2, Backblaze B2, MinIO, Wasabi.

#### `GcsConfig`

Configuration for Google Cloud Storage.

```python
gcs = GcsConfig(
    credentials_file="/path/to/key.json",  # Service account JSON
    project_id="my-project",               # GCP project ID
    credentials_json="...",                # Alternative: JSON as string
)
```

#### `SshConfig`

Configuration for SSH remote connections.

```python
ssh = SshConfig(
    key_file="/path/to/key",    # Private key file
    port=22,                    # SSH port
    password="...",             # Optional: password auth
    compression=True,           # Enable compression
    proxy_jump="bastion@host",  # Jump host
    connect_timeout=30,         # Connection timeout
    pool_size=10,               # Parallel connections
)
```

## Platform Support

| Platform | Local | SSH | S3 | GCS |
|----------|-------|-----|-----|-----|
| macOS    | ✓     | ✓   | ✓   | ✓   |
| Linux    | ✓     | ✓   | ✓   | ✓   |
| Windows  | ✓     | ✓   | ✓   | ✓   |

## Performance

| Scenario | vs rsync |
|----------|----------|
| Local→Local | 2-11x faster |
| Delta sync | 2x faster |
| COW filesystems | 5-9x faster |

## Requirements

- Python 3.8+
- For SSH: `sy` binary on remote host (for server/daemon mode)
- For S3: AWS credentials in environment
- For GCS: Google Cloud credentials

## License

MIT - see [LICENSE](../LICENSE)
