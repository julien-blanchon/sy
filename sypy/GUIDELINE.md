# sypy Usage Guidelines

Comprehensive guide for using sypy in various scenarios.

## Table of Contents

- [Installation](#installation)
- [Quick Reference](#quick-reference)
- [Local Sync](#local-sync)
- [Copy Operations (rclone copy style)](#copy-operations-rclone-copy-style)
- [SSH Remote Sync](#ssh-remote-sync)
- [Daemon Mode (Fast Repeated Syncs)](#daemon-mode-fast-repeated-syncs)
- [S3 Sync](#s3-sync)
- [GCS Sync](#gcs-sync)
- [Performance Tuning](#performance-tuning)
- [Common Patterns](#common-patterns)
- [Directory Listing (sy.ls)](#directory-listing-syls)
- [Troubleshooting](#troubleshooting)

---

## Installation

### Basic Installation

```bash
# Using pip
pip install sy-python

# Using uv (recommended for speed)
uv pip install sy-python
```

### Remote Machine Setup (for SSH sync)

```bash
# On remote machine
uv venv ~/.sy-venv
source ~/.sy-venv/bin/activate
uv pip install sy-python

# Add to PATH (one-time setup)
ln -sf ~/.sy-venv/bin/sy ~/.local/bin/sy
```

### Verify Installation

```python
import sypy
print(f"sypy version: {sypy.__version__}")
```

---

## Quick Reference

```python
import sypy

# Local sync (bidirectional)
sypy.sync("/source/", "/dest/")

# PUT - Upload files to remote
sypy.put("/local/file.txt", "user@host:/remote/")              # Single file via SSH
sypy.put("/local/dir/", "user@host:/remote/", recursive=True)  # Directory via SSH
sypy.put("/local/", "daemon:/remote/", daemon_socket="/tmp/sy.sock")  # Via daemon

# GET - Download files from remote
sypy.get("user@host:/remote/file.txt", "/local/")              # Single file via SSH
sypy.get("user@host:/remote/dir/", "/local/", recursive=True)  # Directory via SSH
sypy.get("daemon:/remote/", "/local/", daemon_socket="/tmp/sy.sock")  # Via daemon

# RM - Remove files
sypy.rm("/local/file.txt")                                     # Single file
sypy.rm("/local/dir/", recursive=True)                         # Directory (files only)
sypy.rm("/local/dir/", recursive=True, rmdirs=True)            # Directory + subdirs
sypy.rm("user@host:/remote/file.txt", sftp=True)               # Remote via SFTP
```

---

## Progress Callbacks

You can receive **real-time progress updates** during `sypy.sync()` by providing a
`progress_callback`. The callback receives a `sypy.ProgressSnapshot` object.

- **Callback signature**: `progress_callback(snapshot: sypy.ProgressSnapshot) -> None`
- **Frequency**: control update rate with `progress_frequency_ms` (default: `1000`)
- **Errors**: if your callback raises, the error is printed and the sync continues (keep it fast and robust)

### Example: print throughput + active transfers

```python
import sypy


def progress_callback(snapshot: sypy.ProgressSnapshot) -> None:
    pct = f"{snapshot.percentage:.1f}%" if snapshot.percentage is not None else "N/A"
    speed_mb = snapshot.bytes_per_sec / (1024 * 1024) if snapshot.bytes_per_sec else 0.0
    transferring = list(snapshot.transferring)[:2] if snapshot.transferring else []

    print(
        f"  [{pct}] {snapshot.bytes}/{snapshot.total_bytes} bytes, "
        f"{snapshot.transfers}/{snapshot.total_transfers} files, "
        f"{speed_mb:.2f} MB/s, active={snapshot.active_transfers}"
    )
    if transferring:
        print(f"    Transferring: {transferring}")


source = "/source/"
dest = "/dest/"

stats = sypy.sync(
    source,
    dest,
    progress_callback=progress_callback,
    progress_frequency_ms=200,
)
print(f"Done: {stats.bytes_transferred:,} bytes transferred")
```

### `ProgressSnapshot` fields (most useful)

- **`bytes` / `total_bytes`**: bytes completed vs estimated total (total may be 0 early on)
- **`bytes_per_sec`**: instantaneous speed in bytes/sec
- **`percentage`**: float \(0.0..100.0\), or `None` if total is unknown
- **`transfers` / `total_transfers`**: completed vs planned file transfers
- **`active_transfers`**: number of in-flight transfers
- **`transferring`**: list of paths currently being transferred
- **Convenience**: `current_file`, `speed_human`, `eta_secs`, `bytes_human`, `total_bytes_human`

---

## Local Sync

### Basic Directory Sync

```python
import sypy

# Sync contents of source to destination
stats = sypy.sync("/source/", "/dest/")
print(f"Synced {stats.files_created} files in {stats.duration_secs:.2f}s")
```

### Trailing Slash Semantics

```python
# WITH trailing slash: sync contents only
sypy.sync("/project/", "/backup/")
# Result: /backup/file1.txt, /backup/file2.txt

# WITHOUT trailing slash: sync the directory itself
sypy.sync("/project", "/backup/")
# Result: /backup/project/file1.txt, /backup/project/file2.txt
```

### Mirror Mode (Delete Extra Files)

```python
stats = sypy.sync(
    "/source/", "/dest/",
    delete=True,           # Enable deletion
    delete_threshold=100,  # Allow up to 100% deletion
)
print(f"Deleted {stats.files_deleted} extra files")
```

### Dry Run (Preview Changes)

```python
stats = sypy.sync("/source/", "/dest/", dry_run=True)
print(f"Would create: {stats.files_created}")
print(f"Would update: {stats.files_updated}")
print(f"Would delete: {stats.files_deleted}")
```

### Exclude Patterns

```python
stats = sypy.sync(
    "/source/", "/dest/",
    exclude=[
        "*.log",           # All log files
        "*.tmp",           # Temp files
        "node_modules",    # Node.js dependencies
        "__pycache__",     # Python cache
        ".git",            # Git directory
        "*.pyc",           # Compiled Python
    ],
)
```

### Using .gitignore

```python
stats = sypy.sync(
    "/source/", "/dest/",
    gitignore=True,    # Apply .gitignore rules
    exclude_vcs=True,  # Also exclude .git, .svn, etc.
)
```

---

## Copy Operations (rclone copy style)

### Understanding Sync vs Copy

`sy` is fundamentally a **synchronization tool** (like `rsync`), but it can be used for **copy operations** (like `rclone copy`):

| Tool                   | Default Behavior                                          |
| ---------------------- | --------------------------------------------------------- |
| `rclone copy`          | Copy new files only, skip existing files                  |
| `sy` (default)         | Copy new files + update changed files (size/mtime differ) |
| `sy --ignore-existing` | Copy new files only (exact `rclone copy` behavior)        |

### Basic Copy (sy default)

```python
import sypy

# Upload: copies new + updates changed files
stats = sypy.sync("/local/", "s3://bucket/path/")

# Download: same behavior
stats = sypy.sync("s3://bucket/path/", "/local/")

print(f"Created: {stats.files_created}, Updated: {stats.files_updated}")
```

**Default behavior:**

- ✅ Copies new files
- ✅ Updates files where size or mtime differs
- ❌ Never deletes files (unless `delete=True`)

### Exact rclone copy Behavior

To replicate `rclone copy` exactly (skip all existing files):

```python
# Skip existing files, even if they're different
stats = sypy.sync("/local/", "s3://bucket/path/", ignore_existing=True)

# Only new files are copied
print(f"Copied {stats.files_created} new files")
print(f"Skipped {stats.files_skipped} existing files")
```

### Smart Copy (update only if newer)

```python
# Update files only if source is newer
stats = sypy.sync("/local/", "s3://bucket/path/", update=True)
```

**Behavior:**

- ✅ Copies new files
- ✅ Updates files where source is newer
- ⏭️ Skips files where destination is newer or same age

### Comparison Modes

```python
# Default: compare by size + mtime (fast)
sypy.sync("/source/", "/dest/")

# Force copy all files (ignore mtime)
sypy.sync("/source/", "/dest/", ignore_times=True)

# Compare by size only (fastest)
sypy.sync("/source/", "/dest/", size_only=True)

# Compare by checksum (slowest but most accurate)
sypy.sync("/source/", "/dest/", checksum=True)
```

### Copy Use Cases

#### Simple Upload (S3)

```python
import sypy

# Like: rclone copy /local s3://bucket
s3 = sypy.S3Config(
    access_key_id="...",
    secret_access_key="...",
    region="us-east-1",
)

stats = sypy.sync("/local/data/", "s3://my-bucket/data/", s3=s3)
print(f"Uploaded {stats.files_created} new files")
print(f"Updated {stats.files_updated} changed files")
```

#### Simple Download (S3)

```python
# Like: rclone copy s3://bucket /local
stats = sypy.sync("s3://my-bucket/data/", "/local/data/", s3=s3)
```

#### Skip Existing Files (exact rclone copy)

```python
# Like: rclone copy --ignore-existing
stats = sypy.sync(
    "/local/data/",
    "s3://my-bucket/data/",
    s3=s3,
    ignore_existing=True,  # Skip all existing files
)
```

#### Copy with Exclusions

```python
# Like: rclone copy --exclude "*.log"
stats = sypy.sync(
    "/local/data/",
    "s3://my-bucket/data/",
    s3=s3,
    exclude=["*.log", "*.tmp", "__pycache__"],
)
```

#### Copy Large Dataset (optimized)

```python
# High-throughput copy for many files
client_opts = sypy.CloudClientOptions.high_throughput()
s3 = sypy.S3Config(
    access_key_id="...",
    secret_access_key="...",
    region="us-east-1",
    client_options=client_opts,
)

stats = sypy.sync(
    "/local/data/",
    "s3://my-bucket/data/",
    s3=s3,
    parallel=100,  # Many parallel transfers
)

print(f"Copied {stats.bytes_transferred / 1024**3:.2f} GB")
```

#### Copy to Multiple Destinations

```python
import sypy

def copy_to_backups(source: str, s3_config):
    """Copy to multiple S3 locations."""
    destinations = [
        "s3://primary-backup/data/",
        "s3://secondary-backup/data/",
        "s3://archive-backup/data/",
    ]

    for dest in destinations:
        stats = sypy.sync(source, dest, s3=s3_config, ignore_existing=True)
        print(f"{dest}: copied {stats.files_created} files")

# Usage
s3 = sypy.S3Config(access_key_id="...", secret_access_key="...", region="us-east-1")
copy_to_backups("/local/important/", s3)
```

### Copy vs Sync Decision Guide

| Goal                                  | Use                    |
| ------------------------------------- | ---------------------- |
| One-way copy, skip existing           | `ignore_existing=True` |
| One-way copy, update changed          | Default (no flags)     |
| One-way copy, update if newer         | `update=True`          |
| Mirror (copy + delete extra)          | `delete=True`          |
| Two-way sync with conflict resolution | `bidirectional=True`   |

### Command Line Equivalents

For reference, here are the CLI equivalents:

```bash
# Default (copy + update changed)
sy /local/ s3://bucket/path/

# Skip existing (exact rclone copy)
sy /local/ s3://bucket/path/ --ignore-existing

# Update only if newer
sy /local/ s3://bucket/path/ --update

# Mirror (copy + delete extra)
sy /local/ s3://bucket/path/ --delete
```

---

## SSH Remote Sync

### Basic SSH Sync

```python
import sypy

# Upload to remote
stats = sypy.sync("/local/path/", "user@host:/remote/path/")

# Download from remote
stats = sypy.sync("user@host:/remote/path/", "/local/path/")
```

### With SSH Config

```python
ssh = sypy.SshConfig(
    key_file="~/.ssh/id_ed25519",  # Private key
    port=22,                        # SSH port
    compression=True,               # Enable compression
)

stats = sypy.sync("/local/", "user@host:/remote/", ssh=ssh)
```

### Through Jump Host (Bastion)

```python
ssh = sypy.SshConfig(
    proxy_jump="bastion@jumphost.example.com",
)

stats = sypy.sync("/local/", "user@internal-server:/remote/", ssh=ssh)
```

---

## Daemon Mode (Fast Repeated Syncs)

Daemon mode eliminates SSH connection overhead for repeated syncs, providing **~2x speedup**.

### Requirements

**SSH Key-Based Authentication Required**: Daemon mode uses SSH ControlMaster for connection multiplexing, which requires key-based authentication. Password authentication will not work because ControlMaster cannot prompt for passwords interactively.

```bash
# Set up SSH keys (if not already done)
ssh-keygen -t ed25519 -C "your_email@example.com"

# Copy public key to remote server
ssh-copy-id user@host

# Verify key-based auth works
ssh -o BatchMode=yes user@host "echo 'Key auth works!'"
```

### When to Use Daemon Mode

| Scenario                          | Use `daemon_auto=True`?       |
| --------------------------------- | ----------------------------- |
| Single one-time sync              | ❌ No (overhead not worth it) |
| Repeated syncs (development)      | ✅ Yes                        |
| CI/CD deployments                 | ✅ Yes                        |
| Watch mode / continuous sync      | ✅ Yes                        |
| Backup scripts running frequently | ✅ Yes                        |

### Basic Usage

```python
import sypy

# First call: starts daemon automatically (~5-6s)
# Subsequent calls: reuses daemon (~2-3s instead of ~5s)
stats = sypy.sync("/local/", "user@host:/remote/", daemon_auto=True)
```

### Performance Comparison

```python
import sypy
import time

# Without daemon (each call ~5s)
for i in range(5):
    start = time.time()
    sypy.sync("/local/", "user@host:/remote/", daemon_auto=False)
    print(f"Regular: {time.time() - start:.2f}s")

# With daemon (first ~6s, subsequent ~2-3s)
for i in range(5):
    start = time.time()
    sypy.sync("/local/", "user@host:/remote/", daemon_auto=True)
    print(f"Daemon: {time.time() - start:.2f}s")
```

**Typical results:**

- Regular SSH: ~5.3s average
- Daemon mode: ~2.4s average (after first run)
- **Speedup: 2.2x**

### How It Works

```
First call with daemon_auto=True:
┌─────────────────────────────────────────────────────────┐
│ 1. Check if daemon socket exists locally                │
│ 2. SSH to remote: check if daemon running               │
│ 3. If not running: start `sy --daemon` on remote        │
│ 4. Set up SSH socket forwarding (ControlMaster)         │
│ 5. Sync files through daemon                            │
└─────────────────────────────────────────────────────────┘

Subsequent calls:
┌─────────────────────────────────────────────────────────┐
│ 1. Detect existing local socket → reuse connection      │
│ 2. Sync files directly (skip all setup)                 │
└─────────────────────────────────────────────────────────┘
```

### Socket Locations

| Location | Path                            | Purpose             |
| -------- | ------------------------------- | ------------------- |
| Remote   | `~/.sy/daemon.sock`             | Daemon listens here |
| Local    | `/tmp/sy-daemon/{host}.sock`    | Forwarded socket    |
| Local    | `/tmp/sy-daemon/{host}.control` | SSH ControlMaster   |

### Connection Persistence

- SSH ControlMaster keeps connection alive for **10 minutes** after last use
- Daemon runs indefinitely until stopped
- No manual cleanup needed

### Development Workflow Example

```python
import sypy
from pathlib import Path
import time

def deploy_to_staging(project_dir: str, remote: str):
    """Deploy project to staging server."""
    stats = sypy.sync(
        f"{project_dir}/",
        remote,
        daemon_auto=True,      # Fast repeated deploys
        exclude=[
            "*.pyc", "__pycache__",
            ".git", ".env",
            "node_modules", ".venv",
        ],
        delete=True,           # Mirror mode
    )
    print(f"Deployed {stats.files_created + stats.files_updated} files")
    return stats

# Usage
deploy_to_staging("/code/myapp", "deploy@staging.example.com:/var/www/myapp")
```

---

## S3 Sync

### Basic S3 Upload

```python
import sypy

s3 = sypy.S3Config(
    access_key_id="AKIAIOSFODNN7EXAMPLE",
    secret_access_key="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    region="us-east-1",
)

# Upload directory to S3
stats = sypy.sync("/local/data/", "s3://my-bucket/data/", s3=s3)
print(f"Uploaded {stats.files_created} files, {stats.bytes_transferred:,} bytes")
```

### Download from S3

```python
stats = sypy.sync("s3://my-bucket/data/", "/local/data/", s3=s3)
```

### S3-Compatible Services

#### DigitalOcean Spaces

```python
s3 = sypy.S3Config(
    access_key_id="DO00...",
    secret_access_key="...",
    region="us-east-1",  # Required but ignored
    endpoint="https://sfo3.digitaloceanspaces.com",
)

sypy.sync("/local/", "s3://my-space/path/", s3=s3)
```

#### Cloudflare R2

```python
s3 = sypy.S3Config(
    access_key_id="...",
    secret_access_key="...",
    region="auto",
    endpoint="https://<account_id>.r2.cloudflarestorage.com",
)

sypy.sync("/local/", "s3://my-bucket/path/", s3=s3)
```

#### MinIO (Self-Hosted)

```python
s3 = sypy.S3Config(
    access_key_id="minioadmin",
    secret_access_key="minioadmin",
    region="us-east-1",
    endpoint="http://localhost:9000",
)

# For HTTP endpoints, use CloudClientOptions
client_opts = sypy.CloudClientOptions(allow_http=True)
s3 = sypy.S3Config(
    access_key_id="minioadmin",
    secret_access_key="minioadmin",
    endpoint="http://localhost:9000",
    client_options=client_opts,
)
```

#### Backblaze B2

```python
s3 = sypy.S3Config(
    access_key_id="<applicationKeyId>",
    secret_access_key="<applicationKey>",
    region="us-west-004",
    endpoint="https://s3.us-west-004.backblazeb2.com",
)
```

### Using Environment Variables

```python
import os

os.environ["AWS_ACCESS_KEY_ID"] = "..."
os.environ["AWS_SECRET_ACCESS_KEY"] = "..."
os.environ["AWS_REGION"] = "us-east-1"

# No S3Config needed
sypy.sync("/local/", "s3://my-bucket/path/")
```

### Using AWS Profile

```python
s3 = sypy.S3Config(profile="production")
sypy.sync("/local/", "s3://my-bucket/path/", s3=s3)
```

---

## GCS Sync

### Basic GCS Upload

```python
import sypy

gcs = sypy.GcsConfig(
    credentials_file="/path/to/service-account.json",
    project_id="my-gcp-project",  # Optional
)

# Upload to GCS
stats = sypy.sync("/local/data/", "gs://my-bucket/data/", gcs=gcs)
```

### Download from GCS

```python
stats = sypy.sync("gs://my-bucket/data/", "/local/data/", gcs=gcs)
```

### Using Environment Variables

```python
import os

os.environ["GOOGLE_APPLICATION_CREDENTIALS"] = "/path/to/key.json"

# No GcsConfig needed
sypy.sync("/local/", "gs://my-bucket/path/")
```

### Using Application Default Credentials

If running on GCP (Compute Engine, Cloud Run, etc.):

```python
gcs = sypy.GcsConfig(project_id="my-project")  # Uses instance credentials
sypy.sync("/local/", "gs://my-bucket/path/", gcs=gcs)
```

---

## Performance Tuning

### Parallel Transfers

```python
# For many small files: increase parallelism
stats = sypy.sync("/source/", "/dest/", parallel=50)

# For few large files: lower parallelism
stats = sypy.sync("/source/", "/dest/", parallel=4)
```

### Cloud Client Options

```python
# High throughput preset (many parallel transfers)
client_opts = sypy.CloudClientOptions.high_throughput()
# - pool_max_idle_per_host: 100
# - request_timeout_secs: 120
# - max_retries: 3

# Low latency preset (interactive use)
client_opts = sypy.CloudClientOptions.low_latency()
# - pool_max_idle_per_host: 20
# - request_timeout_secs: 30
# - max_retries: 2

# Custom configuration
client_opts = sypy.CloudClientOptions(
    pool_max_idle_per_host=100,   # Connection pool size
    pool_idle_timeout_secs=60,    # Keep connections for 60s
    connect_timeout_secs=5,       # Connection timeout
    request_timeout_secs=300,     # For large files
    max_retries=5,                # Retry attempts
    retry_timeout_secs=30,        # Max retry duration
)

s3 = sypy.S3Config(
    access_key_id="...",
    secret_access_key="...",
    client_options=client_opts,
)
```

### Bandwidth Limiting

```python
# Limit to 10 MB/s
stats = sypy.sync("/source/", "user@host:/dest/", bwlimit="10MB")

# Limit to 1 MB/s
stats = sypy.sync("/source/", "user@host:/dest/", bwlimit="1MB")
```

### Checksum vs Time-based Comparison

```python
# Default: compare by size + mtime (fast)
stats = sypy.sync("/source/", "/dest/")

# Compare by checksum (slower but more accurate)
stats = sypy.sync("/source/", "/dest/", checksum=True)

# Compare by size only (fastest)
stats = sypy.sync("/source/", "/dest/", size_only=True)
```

---

## Common Patterns

### Backup Script

```python
#!/usr/bin/env python3
"""Daily backup script with rotation."""
import sypy
from datetime import datetime

def backup_to_s3(source: str, bucket: str, prefix: str):
    s3 = sypy.S3Config(
        access_key_id="...",
        secret_access_key="...",
        region="us-east-1",
        client_options=sypy.CloudClientOptions.high_throughput(),
    )

    date = datetime.now().strftime("%Y-%m-%d")
    dest = f"s3://{bucket}/{prefix}/{date}/"

    stats = sypy.sync(
        source,
        dest,
        s3=s3,
        parallel=50,
        exclude=["*.log", "*.tmp", ".git"],
    )

    print(f"Backup complete: {stats.files_created} files, "
          f"{stats.bytes_transferred / 1024 / 1024:.1f} MB")
    return stats

# Usage
backup_to_s3("/data/important/", "my-backups", "daily")
```

### CI/CD Deployment

```python
#!/usr/bin/env python3
"""Deploy application to server."""
import sypy
import sys

def deploy(env: str):
    servers = {
        "staging": "deploy@staging.example.com:/var/www/app",
        "production": "deploy@prod.example.com:/var/www/app",
    }

    if env not in servers:
        print(f"Unknown environment: {env}")
        sys.exit(1)

    stats = sypy.sync(
        "./dist/",
        servers[env],
        daemon_auto=True,  # Fast repeated deploys
        delete=True,       # Remove old files
        exclude=[".env", "*.log"],
    )

    if stats.success:
        print(f"✓ Deployed to {env}: {stats.files_created + stats.files_updated} files")
    else:
        print(f"✗ Deployment failed")
        for error in stats.errors:
            print(f"  - {error}")
        sys.exit(1)

if __name__ == "__main__":
    deploy(sys.argv[1] if len(sys.argv) > 1 else "staging")
```

### Data Pipeline (S3 → Local → Process → GCS)

```python
import sypy
from pathlib import Path
import tempfile

def process_data():
    s3 = sypy.S3Config(access_key_id="...", secret_access_key="...", region="us-east-1")
    gcs = sypy.GcsConfig(credentials_file="/path/to/key.json")

    with tempfile.TemporaryDirectory() as tmpdir:
        # Download from S3
        print("Downloading from S3...")
        sypy.sync("s3://input-bucket/data/", f"{tmpdir}/input/", s3=s3)

        # Process data
        print("Processing...")
        process_files(Path(tmpdir) / "input", Path(tmpdir) / "output")

        # Upload to GCS
        print("Uploading to GCS...")
        stats = sypy.sync(f"{tmpdir}/output/", "gs://output-bucket/processed/", gcs=gcs)

        print(f"Done: {stats.files_created} files uploaded")

def process_files(input_dir: Path, output_dir: Path):
    output_dir.mkdir(parents=True, exist_ok=True)
    for f in input_dir.glob("*.csv"):
        # Your processing logic here
        (output_dir / f.name).write_text(f.read_text().upper())
```

### Multi-Target Sync

```python
import sypy
from concurrent.futures import ThreadPoolExecutor

def sync_to_multiple_targets(source: str, targets: list[str]):
    """Sync to multiple destinations in parallel."""

    def sync_one(target: str):
        try:
            stats = sypy.sync(source, target, daemon_auto=True)
            return (target, stats, None)
        except Exception as e:
            return (target, None, str(e))

    with ThreadPoolExecutor(max_workers=len(targets)) as executor:
        results = list(executor.map(sync_one, targets))

    for target, stats, error in results:
        if error:
            print(f"✗ {target}: {error}")
        else:
            print(f"✓ {target}: {stats.files_created} files")

# Usage
sync_to_multiple_targets(
    "/local/app/",
    [
        "deploy@server1.example.com:/var/www/app",
        "deploy@server2.example.com:/var/www/app",
        "deploy@server3.example.com:/var/www/app",
    ]
)
```

---

## Troubleshooting

### SSH Connection Issues

```python
# Enable verbose SSH output
import os
os.environ["RUST_LOG"] = "debug"

# Then run sync
sypy.sync("/local/", "user@host:/remote/")
```

### Daemon Mode Not Working

**Most common cause: Password authentication instead of key-based auth**

Daemon mode requires SSH key-based authentication. If you see errors like:

- `Permission denied (publickey,password)`
- `Failed to establish ControlMaster`
- Timeout during daemon startup

Fix by setting up SSH keys:

```bash
# 1. Generate SSH key (if you don't have one)
ssh-keygen -t ed25519

# 2. Copy key to remote server
ssh-copy-id user@host

# 3. Verify key auth works (should NOT prompt for password)
ssh -o BatchMode=yes user@host "echo 'success'"
```

**Other troubleshooting steps:**

```bash
# Check if daemon is running on remote
ssh user@host "ps aux | grep 'sy --daemon'"

# Check socket exists
ssh user@host "ls -la ~/.sy/daemon.sock"

# Manually start daemon
ssh user@host "sy --daemon --socket ~/.sy/daemon.sock &"

# Check local sockets
ls -la /tmp/sy-daemon/

# Clean up stale sockets and retry
rm -rf /tmp/sy-daemon/
ssh user@host "pkill -f 'sy.*daemon'; rm -f ~/.sy/daemon.sock"
```

### S3 Permission Errors

```python
# Test with minimal permissions first
s3 = sypy.S3Config(
    access_key_id="...",
    secret_access_key="...",
    region="us-east-1",
)

# Dry run to check permissions
stats = sypy.sync("/local/", "s3://bucket/path/", s3=s3, dry_run=True)
```

### GCS Authentication Issues

```bash
# Verify credentials file
cat /path/to/key.json | jq .client_email

# Test with gcloud
gcloud auth activate-service-account --key-file=/path/to/key.json
gsutil ls gs://my-bucket/
```

### Performance Issues

```python
# Profile sync operation
import time

start = time.time()
stats = sypy.sync("/source/", "/dest/")
elapsed = time.time() - start

print(f"Duration: {elapsed:.2f}s")
print(f"Files: {stats.files_scanned}")
print(f"Transferred: {stats.bytes_transferred:,} bytes")
print(f"Rate: {stats.bytes_transferred / elapsed / 1024 / 1024:.1f} MB/s")
```

---

## Directory Listing (sy.ls)

sy provides a fast, unified directory listing API that works across all storage backends.

### Basic Listing

```python
import sypy

# List local directory (non-recursive)
entries = sypy.ls("/path/to/directory")

for entry in entries:
    icon = "📁" if entry.is_dir else "📄"
    print(f"{icon} {entry.path}: {entry.size} bytes")
```

### Recursive Listing

```python
# List all files and subdirectories
entries = sypy.ls("/path/to/directory", recursive=True)

print(f"Found {len(entries)} total entries")

# Group by type
files = [e for e in entries if not e.is_dir]
dirs = [e for e in entries if e.is_dir]
print(f"Files: {len(files)}, Directories: {len(dirs)}")
```

### Filters

```python
# Only files (exclude directories)
files = sypy.ls("/path", recursive=True, files_only=True)

# Only directories (exclude files)
dirs = sypy.ls("/path", recursive=True, dirs_only=True)

# Limit recursion depth
entries = sypy.ls("/path", recursive=True, max_depth=2)
```

### Remote Storage

```python
import os

# SSH listing
entries = sypy.ls("user@host:/remote/path")

# S3 listing (uses environment credentials)
os.environ['AWS_ACCESS_KEY_ID'] = '...'
os.environ['AWS_SECRET_ACCESS_KEY'] = '...'
entries = sypy.ls("s3://bucket/path/")

# GCS listing
os.environ['GOOGLE_APPLICATION_CREDENTIALS'] = '/path/to/key.json'
entries = sypy.ls("gs://bucket/path/")
```

### Working with Metadata

```python
entries = sypy.ls("/path", recursive=True)

# MIME type filtering
rust_files = [e for e in entries if e.mime_type == "text/x-rust"]
images = [e for e in entries if e.mime_type and e.mime_type.startswith("image/")]

# Size calculations
total_size = sum(e.size for e in entries if not e.is_dir)
print(f"Total: {total_size / 1024 / 1024:.2f} MB")

# Find large files
large_files = [e for e in entries if e.size > 100 * 1024 * 1024]  # > 100MB
for f in sorted(large_files, key=lambda x: -x.size):
    print(f"{f.path}: {f.size / 1024 / 1024:.1f} MB")

# Sparse file detection (Unix only)
sparse_files = [e for e in entries if e.is_sparse]
for f in sparse_files:
    ratio = f.size / f.allocated_size if f.allocated_size else 1
    print(f"{f.path}: {ratio:.1f}x sparse ratio")

# Hard link detection (Unix only)
hardlinked = [e for e in entries if e.num_links and e.num_links > 1]
for f in hardlinked:
    print(f"{f.path}: {f.num_links} links (inode {f.inode})")
```

### TypedDict Integration

```python
from sypy import ListEntryDict
import json

# Convert to dicts for JSON APIs or databases
entries = sypy.ls("/path", recursive=True)
dicts: list[ListEntryDict] = [entry.to_dict() for entry in entries]

# Type-safe field access (type checkers understand all fields)
for d in dicts:
    path: str = d['path']
    size: int = d['size']
    mod_time: str = d['mod_time']
    is_dir: bool = d['is_dir']

    # Optional fields (check first)
    if 'mime_type' in d:
        mime: str = d['mime_type']
        print(f"{path}: {mime}")

# Serialize to JSON
json_str = json.dumps(dicts, indent=2)
print(json_str)
```

### Performance - S3/GCS

```python
import time

# Efficient flat listing (uses delimiter, fast)
start = time.time()
entries = sypy.ls("s3://my-bucket/path/")  # Non-recursive
print(f"Listed {len(entries)} entries in {time.time() - start:.2f}s")
# Typical: 0.5-1.0s for hundreds of objects

# Full recursive listing (scans entire tree)
start = time.time()
entries = sypy.ls("s3://my-bucket/path/", recursive=True)
print(f"Listed {len(entries)} entries in {time.time() - start:.2f}s")
# Typical: 18-20s for 66K objects
```

### Real-World Examples

#### Find Recently Modified Files

```python
from datetime import datetime, timedelta

entries = sypy.ls("s3://bucket/logs/", recursive=True, files_only=True)

# Parse modification times
recent = []
cutoff = datetime.now() - timedelta(days=7)

for e in entries:
    mod_time = datetime.fromisoformat(e.mod_time.replace('Z', '+00:00'))
    if mod_time > cutoff:
        recent.append((e.path, mod_time))

print(f"Files modified in last 7 days: {len(recent)}")
```

#### Calculate Storage Costs

```python
entries = sypy.ls("s3://bucket/", recursive=True, files_only=True)

total_bytes = sum(e.size for e in entries)
total_gb = total_bytes / (1024 ** 3)

# S3 Standard pricing: ~$0.023/GB/month
estimated_cost = total_gb * 0.023

print(f"Total storage: {total_gb:.2f} GB")
print(f"Estimated monthly cost: ${estimated_cost:.2f}")
```

#### Inventory Report

```python
import csv
from collections import defaultdict

entries = sypy.ls("gs://bucket/data/", recursive=True)

# Group by MIME type
by_type = defaultdict(lambda: {'count': 0, 'size': 0})

for e in entries:
    if not e.is_dir and e.mime_type:
        by_type[e.mime_type]['count'] += 1
        by_type[e.mime_type]['size'] += e.size

# Export to CSV
with open('storage_inventory.csv', 'w', newline='') as f:
    writer = csv.writer(f)
    writer.writerow(['MIME Type', 'File Count', 'Total Size (MB)'])

    for mime, stats in sorted(by_type.items(), key=lambda x: -x[1]['size']):
        size_mb = stats['size'] / (1024 * 1024)
        writer.writerow([mime, stats['count'], f"{size_mb:.2f}"])

print("Inventory report saved to storage_inventory.csv")
```

### Performance Tips

| Use Case          | Recommendation                          |
| ----------------- | --------------------------------------- |
| List S3/GCS root  | **Don't use `-R`** (flat is 27x faster) |
| Scan large bucket | Use `-R` only when needed               |
| Filter files      | Use `files_only=True` client-side       |
| Process metadata  | Convert to dict with `.to_dict()`       |

### ListEntry Fields

| Field            | Type  | Description                    | Available On      |
| ---------------- | ----- | ------------------------------ | ----------------- |
| `path`           | str   | Relative path                  | All               |
| `size`           | int   | Size in bytes                  | All               |
| `mod_time`       | str   | RFC3339 timestamp              | All               |
| `is_dir`         | bool  | Is directory                   | All               |
| `entry_type`     | str   | "file", "directory", "symlink" | All               |
| `mime_type`      | str?  | Inferred MIME type             | All               |
| `symlink_target` | str?  | Symlink target                 | Local, SSH        |
| `is_sparse`      | bool? | Sparse file                    | Local, SSH (Unix) |
| `allocated_size` | int?  | Actual disk usage              | Local, SSH (Unix) |
| `inode`          | int?  | Inode number                   | Local, SSH (Unix) |
| `num_links`      | int?  | Hard link count                | Local, SSH (Unix) |

---

## Summary

| Scenario              | Recommended Settings                     |
| --------------------- | ---------------------------------------- |
| Local backup          | `parallel=10`, `verify=True`             |
| SSH development       | `daemon_auto=True`, `parallel=10`        |
| SSH production deploy | `daemon_auto=True`, `delete=True`        |
| S3 many small files   | `parallel=50`, `high_throughput()`       |
| S3 large files        | `parallel=4`, `request_timeout_secs=300` |
| GCS backup            | `parallel=20`, `verify=True`             |
| Mirror/clone          | `delete=True`, `checksum=True`           |
| **List S3/GCS flat**  | **No `-R` flag** (27x faster)            |
| **List recursively**  | `recursive=True` (complete scan)         |

---

_For more details, see the [API Reference](README.md#api-reference) or the [sy documentation](https://github.com/nijaru/sy)._
