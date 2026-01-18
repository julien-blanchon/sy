# sypy Usage Guidelines

Comprehensive guide for using sypy in various scenarios.

## Table of Contents

- [Installation](#installation)
- [Quick Reference](#quick-reference)
- [Local Sync](#local-sync)
- [SSH Remote Sync](#ssh-remote-sync)
- [Daemon Mode (Fast Repeated Syncs)](#daemon-mode-fast-repeated-syncs)
- [S3 Sync](#s3-sync)
- [GCS Sync](#gcs-sync)
- [Performance Tuning](#performance-tuning)
- [Common Patterns](#common-patterns)
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

# Local sync
sypy.sync("/source/", "/dest/")

# SSH sync
sypy.sync("/local/", "user@host:/remote/")

# SSH with daemon mode (2x faster for repeated syncs)
sypy.sync("/local/", "user@host:/remote/", daemon_auto=True)

# S3 sync
s3 = sypy.S3Config(access_key_id="...", secret_access_key="...", region="us-east-1")
sypy.sync("/local/", "s3://bucket/path/", s3=s3)

# GCS sync
gcs = sypy.GcsConfig(credentials_file="/path/to/key.json")
sypy.sync("/local/", "gs://bucket/path/", gcs=gcs)
```

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

### When to Use Daemon Mode

| Scenario | Use `daemon_auto=True`? |
|----------|------------------------|
| Single one-time sync | ❌ No (overhead not worth it) |
| Repeated syncs (development) | ✅ Yes |
| CI/CD deployments | ✅ Yes |
| Watch mode / continuous sync | ✅ Yes |
| Backup scripts running frequently | ✅ Yes |

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

| Location | Path | Purpose |
|----------|------|---------|
| Remote | `~/.sy/daemon.sock` | Daemon listens here |
| Local | `/tmp/sy-daemon/{host}.sock` | Forwarded socket |
| Local | `/tmp/sy-daemon/{host}.control` | SSH ControlMaster |

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

```bash
# Check if daemon is running on remote
ssh user@host "ps aux | grep 'sy --daemon'"

# Check socket exists
ssh user@host "ls -la ~/.sy/daemon.sock"

# Manually start daemon
ssh user@host "sy --daemon --socket ~/.sy/daemon.sock &"

# Check local sockets
ls -la /tmp/sy-daemon/
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

## Summary

| Scenario | Recommended Settings |
|----------|---------------------|
| Local backup | `parallel=10`, `verify=True` |
| SSH development | `daemon_auto=True`, `parallel=10` |
| SSH production deploy | `daemon_auto=True`, `delete=True` |
| S3 many small files | `parallel=50`, `high_throughput()` |
| S3 large files | `parallel=4`, `request_timeout_secs=300` |
| GCS backup | `parallel=20`, `verify=True` |
| Mirror/clone | `delete=True`, `checksum=True` |

---

*For more details, see the [API Reference](README.md#api-reference) or the [sy documentation](https://github.com/nijaru/sy).*
