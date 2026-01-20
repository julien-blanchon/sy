# System Design

## Overview

sy is a file synchronization tool with adaptive strategies for different environments (local, LAN, WAN, cloud).

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         CLI (main.rs)                        │
├─────────────────────────────────────────────────────────────┤
│                      Sync Engine (sync/)                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐ │
│  │ Scanner  │→│ Strategy │→│ Transfer │→│ Server Mode │ │
│  └──────────┘  └──────────┘  └──────────┘  └─────────────┘ │
├─────────────────────────────────────────────────────────────┤
│                    Transport Layer (transport/)              │
│  ┌───────┐  ┌──────┐  ┌────────┐  ┌────┐  ┌────────────┐  │
│  │ Local │  │ SSH  │  │ Server │  │ S3 │  │ GCS │  │ Dual │ │
│  └───────┘  └──────┘  └────────┘  └────┘  └────────────┘  │
├─────────────────────────────────────────────────────────────┤
│                     Support Modules                          │
│  ┌───────────┐  ┌──────────┐  ┌────────┐  ┌─────────────┐  │
│  │ Integrity │  │ Compress │  │ Filter │  │   Resume    │  │
│  │ (hashing) │  │ (zstd)   │  │(gitignore)│ │(checkpoints)│ │
│  └───────────┘  └──────────┘  └────────┘  └─────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

## Components

| Component | Purpose | Status |
|-----------|---------|--------|
| sync/scanner | Directory traversal, parallel scanning | Stable |
| sync/strategy | Planner: compare source/dest, decide actions | Stable |
| sync/transfer | File copy, delta sync, checksums | Stable |
| sync/server_mode | Binary protocol for SSH (push/pull) | Stable |
| transport/local | Local filesystem operations | Stable |
| transport/ssh | SFTP via ssh2 (C bindings) | Stable |
| transport/server | Server protocol client | Stable |
| transport/s3 | AWS S3 via object_store | Experimental |
| transport/gcs | Google Cloud Storage via object_store | Experimental |
| server/ | `sy --server` handler | Stable |
| integrity/ | xxHash3, BLAKE3, Adler-32 | Stable |
| compress/ | zstd, lz4 compression | Stable |
| filter/ | Gitignore, rsync patterns | Stable |

## Data Flow

**Local → Remote (Server Push):**
1. Scanner enumerates source files
2. Strategy compares with destination (via server)
3. Server mode streams files over binary protocol
4. Delta sync for large files (checksums → deltas)

**Remote → Local (Server Pull):**
1. Client connects, sends HELLO with PULL flag
2. Server scans source, sends MKDIR_BATCH → FILE_LIST
3. Client compares with local, sends decisions
4. Server streams FILE_DATA for requested files

## Key Design Decisions

→ See DECISIONS.md for rationale

| Decision | Choice | Why |
|----------|--------|-----|
| Hashing | xxHash3 + BLAKE3 | Speed + security |
| Compression | zstd adaptive | Best ratio/speed tradeoff |
| SSH | ssh2 (libssh2) | Mature, SSH agent works |
| Protocol | Custom binary | Pipelined, delta-aware |
| Database | fjall (LSM) | Pure Rust, embedded |

## CLI Utilities

In addition to the main `sy` command, several standalone utilities are available for direct file operations across all supported backends:

| Utility | Purpose | Similar to |
|---------|---------|------------|
| `sy-ls` | List files/directories on any backend | `rclone ls` |
| `sy-rm` | Remove files/directories | `rclone delete/purge` |
| `sy-put` | Upload local files to remote | `rclone copy {local} {remote}` |
| `sy-get` | Download remote files to local | `rclone copy {remote} {local}` |

### Common Features

All utilities share these capabilities:
- **Multi-backend**: Local, SSH, S3, GCS support
- **Filtering**: `--include`, `--exclude` patterns (gitignore-style)
- **Depth control**: `--max-depth` for directory operations
- **Dry-run**: Preview operations without executing
- **Recursive**: `-R` flag for directory operations

### Path Syntax

```
local:     /path/to/file
ssh:       user@host:/path/to/file
s3:        s3://bucket/prefix/file
gcs:       gs://bucket/prefix/file
```

### Usage Examples

```bash
# Upload to S3
sy-put /local/dir s3://bucket/prefix/ -R

# Download from GCS
sy-get gs://bucket/prefix/ /local/dir -R

# Remove files with filter
sy-rm s3://bucket/logs/ -R --include "*.log" --exclude "important.log"

# List SSH directory
sy-ls user@host:/remote/path -R --format human
```

## Component Details

→ See ai/design/ for detailed specs:
- `server-mode.md` — Binary protocol specification
- `daemon-mode.md` — Persistent daemon for fast repeated syncs

→ See ai/research/ for benchmarks and analysis:
- `cli-utilities-benchmark.md` — sy-put/get/rm design, SSH modes, performance results

## Performance Summary

| Backend | sy performance | Notes |
|---------|----------------|-------|
| **SSH (server mode)** | Same as rsync | Pipelined transfers |
| **SSH (daemon mode)** | **2.5× faster** | Eliminates startup overhead |
| **S3** | **45% faster** | Parallel uploads |
| **GCS** | Rate-limited | Needs throttling (future) |