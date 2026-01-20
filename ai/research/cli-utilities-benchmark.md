# CLI Utilities: sy-put, sy-get, sy-rm

## Overview

New standalone CLI utilities for file operations on remote backends, similar to `rclone` commands:

| Command | Purpose | Similar to |
|---------|---------|------------|
| `sy-put` | Upload local → remote | `rclone copy {local} {remote}` |
| `sy-get` | Download remote → local | `rclone copy {remote} {local}` |
| `sy-rm` | Remove remote files | `rclone delete` / `rclone purge` |
| `sy-ls` | List remote files | `rclone ls` |

## Supported Backends

- **SSH**: `user@host:/path` - Uses server protocol (`sy --server`) for high performance
- **S3**: `s3://bucket/key` - AWS S3, DigitalOcean Spaces, MinIO, etc.
- **GCS**: `gs://bucket/key` - Google Cloud Storage

## Design Decisions

### SSH Transport: Server Protocol vs SFTP

For SSH destinations, we use **sy's server protocol** by default instead of SFTP:

```
┌─────────────┐      SSH      ┌─────────────┐
│   sy-put    │ ───────────▶  │ sy --server │
│   (client)  │  ◀───────────  │  (remote)   │
└─────────────┘    binary     └─────────────┘
                  protocol
```

**Why server protocol?**

| Aspect | SFTP | Server Protocol |
|--------|------|-----------------|
| Per-file latency | High (round-trip per op) | Low (pipelined) |
| Protocol | Text-based | Binary, compact |
| Compression | None | Zstd (optional) |
| Delta sync | No | Yes (for updates) |
| Requirement | SSH only | `sy` binary on remote |

**Fallback**: Use `--sftp` flag if remote doesn't have `sy` installed.

### Pipelined Transfers

The server protocol pipelines all operations:

```
PUSH MODE (sy-put):
  Client                          Server
    │                               │
    │─── MKDIR_BATCH ──────────────▶│
    │◀── MKDIR_BATCH_ACK ───────────│
    │                               │
    │─── FILE_LIST ────────────────▶│
    │◀── FILE_LIST_ACK (decisions)──│
    │                               │
    │─── FILE_DATA (file 1) ───────▶│
    │─── FILE_DATA (file 2) ───────▶│  ← Pipelined: send ALL
    │─── FILE_DATA (file N) ───────▶│    before waiting
    │◀── FILE_DONE (file 1) ────────│
    │◀── FILE_DONE (file N) ────────│  ← Collect ACKs at end
    │                               │

PULL MODE (sy-get):
  Client                          Server
    │                               │
    │◀── MKDIR_BATCH ───────────────│
    │─── MKDIR_BATCH_ACK ──────────▶│
    │                               │
    │◀── FILE_LIST ─────────────────│
    │─── FILE_LIST_ACK (decisions)─▶│
    │                               │
    │◀── FILE_DATA (file 1) ────────│
    │◀── FILE_DATA (file N) ────────│  ← Pipelined from server
    │─── FILE_DONE (all) ──────────▶│  ← Batch ACKs at end
```

**Before pipelining**: 100 small files took 34s (sequential round-trips)
**After pipelining**: 100 small files took 6.6s (same as rsync)

## Benchmark Results

### Test Environment

- Local: macOS (M1)
- Remote: Linux server (France), ~50ms RTT
- Test data: 10, 100, and 1000 files of various sizes

### SSH Upload Performance (100 × 1KB files)

| Mode | Time | vs rsync |
|------|------|----------|
| rsync -avz | 6.0s | baseline |
| sy-put (server protocol) | 6.4s | +7% |
| sy --use-daemon | **2.4s** | **2.5× faster** |

### SSH Download Performance

| Test | rsync | sy-get | Winner |
|------|-------|--------|--------|
| 100 × 1KB files | 6.7s | 6.6s | **same** |
| 10 × 1MB files | 11.6s | **8.9s** | **sy 23% faster** |

### S3 Upload Performance

| Test | rclone | sy-put | Winner |
|------|--------|--------|--------|
| 10 × 1KB files | 1.8s | 1.6s | **sy** |
| 10 × 1MB files | 6.5s | **3.6s** | **sy 45% faster** |

### Summary

| Backend | Result |
|---------|--------|
| **SSH** | Comparable to rsync; daemon mode 2.5× faster |
| **S3** | Faster than rclone for medium/large files |
| **GCS** | Rate-limited (needs `--tpslimit`, future work) |

## SSH Server Mode

### How It Works

When you run `sy-put` or `sy-get` to an SSH destination:

1. Client spawns `ssh user@host sy --server /remote/path`
2. Binary protocol handshake (HELLO)
3. File list exchange with decisions (CREATE/UPDATE/SKIP)
4. Pipelined file transfers
5. Clean shutdown

### Requirements

- `sy` binary must be installed on remote and in PATH
- SSH key authentication recommended

### Fallback to SFTP

```bash
# If remote doesn't have sy installed
sy-put /local/path user@host:/remote/path -R --sftp
```

## Daemon Mode (Fastest)

### Problem

Each SSH sync with server mode has ~4-5s overhead:
- SSH connection: ~0.5s
- `sy --server` startup: ~4s

### Solution

Persistent daemon eliminates startup overhead:

```
┌─────────────┐   SSH Socket    ┌─────────────┐
│   sy-put    │   Forwarding    │ sy --daemon │
│ --use-daemon│ ◀═════════════▶ │ (persistent)│
└─────────────┘  /tmp/sy.sock   └─────────────┘
```

### Setup

#### 1. Start Daemon on Remote

```bash
# SSH to remote server
ssh user@host

# Start daemon (runs in background)
mkdir -p ~/.sy
nohup sy --daemon --socket ~/.sy/daemon.sock > ~/.sy/daemon.log 2>&1 &

# Verify
ls -la ~/.sy/daemon.sock
```

#### 2. Forward Socket via SSH

```bash
# On local machine - create tunnel
ssh -fN -o StreamLocalBindUnlink=yes \
    -L /tmp/sy-daemon.sock:/home/user/.sy/daemon.sock \
    user@host

# Verify
ls -la /tmp/sy-daemon.sock
```

#### 3. Use Daemon for Syncs

```bash
# Push files (note the daemon: prefix for remote path)
sy --use-daemon /tmp/sy-daemon.sock /local/path daemon:/remote/path

# Pull files
sy --use-daemon /tmp/sy-daemon.sock daemon:/remote/path /local/path
```

### Daemon Mode Performance

| Run | Server Mode | Daemon Mode |
|-----|-------------|-------------|
| 1st | 6.3s | 2.9s |
| 2nd | 6.5s | 2.3s |
| 3rd | 6.3s | 2.1s |

Daemon gets faster as it warms up (filesystem caches, etc).

### Automatic Daemon Setup

For convenience, `--daemon-auto` handles everything:

```bash
sy --daemon-auto /local/path user@host:/remote/path

# First run: Starts daemon, sets up forwarding (~6s)
# Subsequent runs: Reuses connection (~3s)
```

Note: Requires correct SSH config (IdentityFile, etc).

## CLI Reference

### sy-put (Upload)

```bash
# Upload single file
sy-put /local/file.txt user@host:/remote/file.txt

# Upload directory recursively
sy-put /local/dir user@host:/remote/dir/ -R

# With filters
sy-put /local/dir s3://bucket/prefix/ -R \
  --include "*.rs" \
  --exclude "target/"

# Preview (dry-run)
sy-put /local/dir user@host:/remote/ -R --dry-run

# Use SFTP fallback
sy-put /local/dir user@host:/remote/ -R --sftp
```

### sy-get (Download)

```bash
# Download single file
sy-get user@host:/remote/file.txt /local/file.txt

# Download directory recursively
sy-get s3://bucket/prefix/ /local/dir -R

# With max depth
sy-get user@host:/remote/ /local/ -R --max-depth 2
```

### sy-rm (Remove)

```bash
# Remove single file
sy-rm user@host:/remote/file.txt

# Remove directory recursively
sy-rm s3://bucket/prefix/ -R

# Remove only files, keep directories
sy-rm user@host:/remote/dir/ -R

# Also remove empty directories
sy-rm user@host:/remote/dir/ -R --rmdirs

# Dry-run
sy-rm s3://bucket/prefix/ -R --dry-run
```

### Common Options

| Option | Description |
|--------|-------------|
| `-R, --recursive` | Process directories recursively |
| `-n, --dry-run` | Preview without executing |
| `-j, --jobs N` | Parallel operations (default: 8) |
| `--include PATTERN` | Include files matching pattern |
| `--exclude PATTERN` | Exclude files matching pattern |
| `--max-depth N` | Maximum directory depth |
| `-v, --verbose` | Verbose output |
| `-q, --quiet` | Suppress output |
| `--json` | Output results as JSON |

## Files Changed

```
src/bin/sy-put.rs     # Upload utility
src/bin/sy-get.rs     # Download utility  
src/bin/sy-rm.rs      # Remove utility
src/server/mod.rs     # Pipelined PULL mode
src/server/daemon.rs  # Pipelined daemon PULL
src/sync/server_mode.rs  # Client-side pipelining
```

## Future Work

1. **GCS rate limiting**: Implement `--tpslimit` like rclone
2. **Resume support**: Continue interrupted transfers
3. **Progress bars**: Per-file progress for large files
4. **Checksum verification**: Post-transfer integrity check

## References

- `ai/design/server-mode.md` - Server protocol design
- `ai/design/daemon-mode.md` - Daemon mode design
- `src/server/protocol.rs` - Binary protocol implementation
