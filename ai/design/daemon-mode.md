# Daemon Mode Design

## Overview

Daemon mode provides a persistent server that eliminates SSH connection and server startup overhead for repeated file syncs. Instead of spawning `sy --server` via SSH for each transfer (~2.5s overhead), the daemon listens on a Unix socket that can be forwarded over SSH.

## Problem Statement

Each SSH sync with `sy --server` mode incurs:
- SSH connection establishment: ~0.5s
- Remote `sy --server` process startup: ~2s
- **Total overhead per transfer: ~2.5s**

For users doing many small syncs (e.g., during development, watch mode), this overhead dominates transfer time.

## Solution Architecture

```
┌─────────────────┐     SSH Socket Forward     ┌─────────────────┐
│   Local sy      │ ◀══════════════════════▶   │  sy --daemon    │
│ --use-daemon    │   /tmp/sy.sock             │ (persistent)    │
└─────────────────┘                            └─────────────────┘
        │                                              │
        ▼                                              ▼
   Local files                                   Remote files
```

### Components

1. **Daemon Server** (`sy --daemon`)
   - Listens on Unix socket
   - Accepts multiple concurrent connections
   - Handles same protocol as `sy --server`
   - Adds `SET_ROOT` message for per-connection root path

2. **Daemon Client** (`--use-daemon`)
   - Connects to daemon via Unix socket
   - Sends `SET_ROOT` to specify remote path
   - Uses standard sync protocol

3. **SSH Socket Forwarding**
   - `ssh -L /tmp/sy.sock:~/.sy/daemon.sock user@host -N`
   - Tunnels Unix socket over SSH connection

## Protocol Extensions

### SET_ROOT Message (0x30)

After HELLO handshake, client sends SET_ROOT to specify the working directory:

```
┌─────────┬──────────┬──────────┬─────────────┐
│ Length  │  Type    │ PathLen  │    Path     │
│ 4 bytes │  1 byte  │ 2 bytes  │  variable   │
│         │  (0x30)  │          │             │
└─────────┴──────────┴──────────┴─────────────┘
```

### SET_ROOT_ACK Message (0x31)

```
┌─────────┬──────────┬──────────┐
│ Length  │  Type    │  Status  │
│ 4 bytes │  1 byte  │  1 byte  │
│         │  (0x31)  │  0=ok    │
└─────────┴──────────┴──────────┘
```

### PING/PONG (0x32/0x33)

Keepalive mechanism for long-running connections:
- PING (0x32): Client → Daemon
- PONG (0x33): Daemon → Client

## Performance Results

**Test**: 50 files (10KB each, 500KB total), 3 consecutive transfers

| Method | Transfer 1 | Transfer 2 | Transfer 3 | Average |
|--------|-----------|-----------|-----------|---------|
| **Without Daemon** | 8.9s | 8.5s | 11.8s | ~9.7s |
| **With Daemon** | 3.5s | 2.7s | 2.3s | ~2.8s |
| **SSH ControlMaster** | 9.0s | 10.8s | 8.9s | ~9.6s |

**Key findings:**
- Daemon mode is **~3.5x faster** for repeated transfers
- SSH ControlMaster doesn't help (bottleneck is server startup, not SSH connection)
- Subsequent transfers get faster as daemon warms up

## Usage

### Start Daemon on Remote

```bash
# Basic usage (current directory as root)
sy --daemon --socket ~/.sy/daemon.sock

# With explicit root path
sy --daemon --socket ~/.sy/daemon.sock /path/to/sync/root

# Background with nohup
nohup sy --daemon --socket ~/.sy/daemon.sock > ~/.sy/daemon.log 2>&1 &
```

### Forward Socket via SSH

```bash
# Create local socket that tunnels to remote daemon
ssh -o StreamLocalBindUnlink=yes \
    -L /tmp/sy.sock:~/.sy/daemon.sock \
    user@host -N &
```

### Sync Using Daemon

```bash
# Push local files to remote
sy --use-daemon /tmp/sy.sock /local/path /remote/path

# Pull remote files to local (not yet implemented)
sy --use-daemon /tmp/sy.sock /remote/path /local/path
```

## Implementation Details

### Files

- `src/server/daemon.rs` - Daemon server implementation
- `src/sync/daemon_mode.rs` - Sync logic for daemon connections
- `src/transport/server.rs` - `DaemonSession` client

### Signal Handling

Daemon handles:
- SIGTERM: Graceful shutdown
- SIGINT: Graceful shutdown

### Connection Limits

- Max concurrent connections: 100 (configurable via semaphore)
- Connections are tracked and cleaned up properly

### Tilde Expansion

Socket paths and root paths support `~` expansion:
- `~/.sy/daemon.sock` → `/home/user/.sy/daemon.sock`

## Alternatives Considered

### SSH ControlMaster

SSH ControlMaster reuses TCP connections but doesn't help because:
- Each sync still spawns a new `sy --server` process
- Server startup (~2s) is the real bottleneck
- Measured: No performance improvement

### rsync Daemon

Similar concept (persistent server) but:
- rsync-specific protocol
- Different configuration model
- sy daemon uses same protocol as `sy --server`

### Keep `sy --server` Running

Essentially what daemon mode does, but:
- Daemon adds proper lifecycle management
- Socket forwarding enables SSH tunneling
- SET_ROOT allows per-connection paths

## Daemon-Auto Mode

### Overview

`--daemon-auto` handles all the complexity automatically:

1. Checks if daemon is already running/accessible on remote
2. Starts daemon on remote if needed
3. Sets up SSH socket forwarding with ControlMaster
4. Reuses connection for subsequent syncs (persists 10 minutes)

### Usage

```bash
# Just add --daemon-auto to any SSH sync
sy --daemon-auto /local/path user@host:/remote/path

# First run: Sets up daemon + forwarding (~6-8s)
# Subsequent runs: Reuses connection (~3-4s vs ~10s without)
```

### How It Works

```
┌──────────────────────────────────────────────────────────────┐
│                        --daemon-auto                         │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│  1. Check local socket exists?  ────▶  Yes: Reuse & sync    │
│         │                                                    │
│         ▼ No                                                 │
│  2. SSH: Check remote daemon    ────▶  Running: Skip start  │
│         │                                                    │
│         ▼ Not running                                        │
│  3. SSH: Start daemon on remote                              │
│         │                                                    │
│         ▼                                                    │
│  4. Start SSH with:                                          │
│     - ControlMaster=auto                                     │
│     - ControlPersist=10m                                     │
│     - Socket forwarding (-L)                                 │
│         │                                                    │
│         ▼                                                    │
│  5. Connect to local socket, sync files                      │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### Performance Results (daemon-auto)

**Test**: 50 files (10KB each), 3 consecutive transfers to remote server

| Mode | Run 1 | Run 2 | Run 3 | Average |
|------|-------|-------|-------|---------|
| **Regular SSH** | 8.7s | 12.2s | 9.1s | ~10s |
| **Daemon-auto** | 3.8s | 3.6s | 3.6s | ~3.6s |

**Result**: ~2.7x faster for repeated syncs

### Implementation Files

- `src/sync/daemon_auto.rs` - Automatic setup logic
- `src/cli.rs` - `--daemon-auto` flag

### Socket Locations

- Local socket: `/tmp/sy-daemon/{host}.sock`
- Control socket: `/tmp/sy-daemon/{host}.control`
- Remote socket: `~/.sy/daemon.sock`

### Connection Persistence

SSH ControlMaster keeps the connection alive for 10 minutes after last use.
The daemon runs indefinitely until explicitly stopped.

## Future Enhancements

1. ~~**Automatic daemon start**: Detect if daemon is available, fall back to `sy --server`~~ ✅ Implemented as `--daemon-auto`
2. **Connection pooling**: Reuse connections for multiple syncs
3. **Daemon status command**: `sy --daemon-status /path/to/socket`
4. **Windows support**: Named pipes instead of Unix sockets
5. **Configurable ControlPersist**: Allow users to set connection timeout

## References

- `ai/design/server-mode.md` - Base protocol design
- `src/server/protocol.rs` - Protocol implementation
- `tests/daemon_mode_test.rs` - Integration tests
