# sy

> Modern file synchronization tool - rsync, reimagined

[![CI](https://github.com/nijaru/sy/workflows/CI/badge.svg)](https://github.com/nijaru/sy/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## Quick Start

```bash
sy /source /destination
```

That's it. Use `sy --help` for options.

## When to Use sy

**sy excels at:**

- Repeated syncs (backups, deployments) — 3x faster after first run
- Large files on APFS/BTRFS/XFS — 40x+ faster via COW reflinks
- Mixed workloads — 2x faster
- Bulk SSH transfers — 2-4x faster

**rsync is better for:**

- First-time sync of many small files — ~1.3x faster
- SSH incremental updates — ~1.3x faster

**Bottom line:** If you sync the same paths repeatedly, sy saves time. If you're doing one-off copies of thousands of tiny files, rsync is faster.

## Installation

### Homebrew (macOS)

```bash
brew tap nijaru/tap
brew install sy
```

### From crates.io

```bash
cargo install sy

# Optional features
cargo install sy --features acl    # ACL preservation (Linux: requires libacl)
cargo install sy --features s3     # S3 support (experimental)
cargo install sy --features gcs    # GCS support (experimental)
```

### From Source

```bash
git clone https://github.com/nijaru/sy.git
cd sy
cargo install --path .
```

**For SSH sync:** Install sy on both local and remote machines.

## Examples

```bash
# Basic
sy ~/project ~/backup                    # Local backup
sy ~/src ~/dest --delete                 # Mirror (remove extra files)
sy /source /dest --dry-run               # Preview changes

# Remote
sy /local user@host:/remote              # SSH sync
sy /local user@host:/backup --bwlimit 1MB

# Verification
sy ~/src ~/dest --verify                 # Verify writes (xxHash3)
sy ~/backup ~/original --verify-only     # Audit existing files

# Filters
sy ~/src ~/dest --exclude "*.log"
sy ~/src ~/dest --gitignore --exclude-vcs

# Advanced
sy --bidirectional /laptop /backup       # Two-way sync
sy ~/dev /backup --watch                 # Continuous sync
sy ~/src ~/dest -j 1                     # Sequential (many tiny files)
```

## Daemon Mode (Fast Repeated Syncs)

For scenarios with many repeated syncs (development, watch mode), daemon mode eliminates the ~2.5s SSH+server startup overhead.

### Easy: Automatic Setup (Recommended)

```bash
# Just add --daemon-auto to any SSH sync
sy --daemon-auto /local user@host:/remote

# First run: Sets up daemon automatically (~6s)
# Subsequent runs: Reuses connection (~3.6s vs ~10s)
```

The connection persists for 10 minutes after last use.

### Manual Setup

For more control, set up daemon manually:

```bash
# 1. Start daemon on remote machine
ssh user@host sy --daemon --socket ~/.sy/daemon.sock

# 2. Forward socket via SSH (keep running)
ssh -L /tmp/sy.sock:~/.sy/daemon.sock user@host -N &

# 3. Sync using daemon (~3x faster)
sy --use-daemon /tmp/sy.sock /local/path /remote/path
```

**Performance comparison** (50 files, 500KB):
| Method | Time |
|--------|------|
| Without daemon | ~10s |
| With daemon | ~2.8s |

Daemon mode is most useful when:
- Syncing files repeatedly throughout the day
- Using watch mode for continuous sync
- Transferring many small batches of files

> **Trailing slash:** sy follows rsync semantics — `/source` copies the directory, `/source/` copies contents only.

## Features

- **Delta sync** — Only transfers changed bytes (rsync algorithm)
- **Parallel transfers** — Configurable worker count (`-j`)
- **Resume support** — Automatically resumes interrupted syncs
- **Integrity verification** — Optional xxHash3 checksums (`--verify`)
- **Bidirectional sync** — Two-way sync with conflict resolution
- **Watch mode** — Continuous file monitoring
- **SSH transport** — Binary protocol, faster than SFTP for bulk transfers
- **S3 support** — AWS S3, Cloudflare R2, Backblaze B2 (experimental)
- **GCS support** — Google Cloud Storage (experimental)
- **Metadata preservation** — Symlinks, permissions, xattrs, ACLs

## Platform Support

| Platform | Status                    |
| -------- | ------------------------- |
| macOS    | Fully tested              |
| Linux    | Fully tested              |
| Windows  | Untested (should compile) |

## Contributing

Contributions welcome! See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
