# Bugs Found During Testing

**Test Date**: 2026-01-20
**sypy Version**: 0.2.3
**Test Environment**: macOS (local) + Linux remote (europe)

## Summary

| Bug | Severity | Component                                  | Status    |
| --- | -------- | ------------------------------------------ | --------- |
| #1  | Low      | `PutResult.success` is method not property | **Fixed** |
| #2  | Medium   | S3 put creates wrong paths                 | **Fixed** |
| #3  | Medium   | GCS download reports false errors          | **Fixed** |
| #4  | Low      | `ls()` doesn't resolve SSH aliases         | **Fixed** |
| #5  | Medium   | SSH put ignores subdirectory               | **Fixed** |
| #6  | High     | SSH rm fails completely                    | **Fixed** |
| #7  | High     | Daemon startup is very slow (~7-15s)       | **Fixed** |
| #8  | Medium   | DaemonConfig.socket_dir is ignored         | **Fixed** |
| #9  | Medium   | Daemon socket naming causes collisions     | **Fixed** |
| #10 | Medium   | Partial socket cleanup causes timeout      | **Fixed** |
| #11 | Medium   | S3/GCS get() fails with directory dest     | Open      |

## Benchmark Results

| Scenario                      | sypy   | rsync/rclone | Winner        |
| ----------------------------- | ------ | ------------ | ------------- |
| Local small files (100 x 1KB) | 0.015s | 0.029s       | **sypy 1.9x** |
| Local large files (5 x 1MB)   | 0.002s | 0.029s       | **sypy 13x**  |
| SSH small files (daemon)      | 3.69s  | 5.86s        | **sypy 1.6x** |
| S3 small files                | 5.17s  | 9.41s        | **sypy 1.8x** |
| Local re-sync (no changes)    | 0.007s | 0.016s       | **sypy 2.3x** |
| SSH re-sync (daemon)          | 1.99s  | 5.37s        | **sypy 2.7x** |

## What Works Well

- Local sync (all operations)
- S3 sync, ls, get (upload/download/list)
- GCS sync, ls (with rate limiting consideration)
- SSH sync with daemon mode (2.7x faster than rsync)
- Progress callbacks
- Unicode filenames
- Symlink preservation
- Files with spaces
- Exclude patterns
- Dry-run mode
- Delete mode

---

## Bug 1: `success` is a method instead of a property on result objects

**Affected classes**: `PutResult`, `GetResult`, `RemoveResult`

**Description**: The `success` attribute on result objects is exposed as a method that needs to be called with `()` instead of being a property that can be accessed directly.

**Current behavior**:

```python
result = sypy.put('/local/file.txt', 's3://bucket/path/')
print(result.success)  # Prints: <built-in method success of builtins.PutResult object at 0x...>
print(result.success())  # Prints: True (correct value)
```

**Expected behavior**:

```python
result = sypy.put('/local/file.txt', 's3://bucket/path/')
print(result.success)  # Should print: True
```

**Impact**: Low - workaround is to call `result.success()` but this is inconsistent with `SyncStats.success` which is a property.

**Location**: `sypy/src/ops.rs` - the PyO3 bindings need `#[getter]` attribute on the success method or it should be exposed as a property.

---

## Bug 2: S3 put creates strange nested paths

**Description**: When using `sypy.put()` to upload a file to S3, the destination path seems to be incorrectly constructed, creating nested paths like `put_test/sy-test` instead of just the file in `put_test/`.

**Steps to reproduce**:

```python
sypy.put('/tmp/sy-test/source/single_file.txt', 's3://bucket/sy-test/put_test/')
```

**Observed S3 contents**:

```
put_test: 11 bytes, dir=False  # File content at wrong location
put_test/sy-test: 0 bytes, dir=False  # Strange empty file
```

**Expected**: The file should be at `sy-test/put_test/single_file.txt`

**Impact**: Medium - files may be uploaded to incorrect paths

---

## Bug 3: GCS download reports errors for directory markers

**Description**: When downloading from GCS, the sync reports errors for directory marker objects (0-byte objects representing directories). The download still succeeds for actual files, but `success` is False.

**Steps to reproduce**:

```python
stats = sypy.sync('gs://bucket/path/', '/local/path/', gcs=gcs)
print(stats.success)  # False
print(stats.errors)   # Errors about "Is a directory (os error 21)"
```

**Observed errors**:

```
[create] /local/path/level1: I/O error: Is a directory (os error 21)
[create] /local/path/level1/level2: I/O error: Is a directory (os error 21)
```

**Expected behavior**: Directory marker objects (0-byte objects ending in `/` or representing directory paths) should be recognized and either:

1. Skipped during download, or
2. Used to create local directories without error

**Impact**: Medium - downloads succeed but report false failures, making it hard to detect real errors

---

## Bug 4: SSH config aliases not resolved by `ls()` function

**Description**: The `ls()` function doesn't resolve SSH config aliases (Host entries in `~/.ssh/config`). It requires the full hostname.

**Steps to reproduce**:

```python
# This works in sync():
sypy.sync('/local/', 'user@europe:/remote/')  # 'europe' is SSH config alias

# But ls() fails:
sypy.ls('user@europe:/remote/')  # Error: failed to lookup address
```

**Error**:

```
RuntimeError: Failed to list directory: I/O error: Failed to connect to europe:22:
failed to lookup address information: nodename nor servname provided, or not known
```

**Workaround**: Use full hostname instead of SSH config alias:

```python
sypy.ls('user@82.66.87.218:/remote/')  # Works
```

**Impact**: Low - workaround available, but inconsistent with sync() behavior

---

## Bug 5: SSH put ignores destination subdirectory

**Description**: When using `put()` with SSH destination that includes a subdirectory path, the file is uploaded to the parent directory instead of the specified subdirectory.

**Steps to reproduce**:

```python
sypy.put('/local/file.txt', 'user@host:~/dir/subdir/')
# Expected: file at ~/dir/subdir/file.txt
# Actual: file at ~/dir/file.txt
```

**Impact**: Medium - files uploaded to wrong location

---

## Bug 6: SSH rm fails with SFTP error

**Description**: The `rm()` function fails for SSH paths with "Failed to list directory contents via SFTP" error.

**Steps to reproduce**:

```python
sypy.rm('user@host:/path/to/dir/', recursive=True)
# RuntimeError: Failed to list directory contents via SFTP
```

**Notes**:

- Tried with both SSH config alias and full hostname - same error
- Tried with explicit SshConfig - same error
- The `sftp=True` parameter is default

**Impact**: High - cannot remove files on SSH remotes via Python API

---

## Bug 7: Daemon startup is very slow (~7-15s instead of ~1-2s)

**Description**: The `daemon_start()` function and `daemon_auto=True` take 7-15 seconds on first run, even though the actual network latency is only ~300ms. This is because the code runs multiple SSH commands sequentially without using SSH ControlMaster for the initial commands.

**Root cause analysis**:

The daemon startup flow in `src/sync/daemon_auto.rs` does:

1. SSH command to check if daemon socket exists (~4s)
2. SSH command to start daemon if needed (~4s)
3. SSH command to verify daemon started (~4s)
4. SSH command to set up socket forwarding (uses ControlMaster)

Each of the first 3 SSH commands creates a new TCP connection, which takes ~4s over high-latency networks.

**Evidence** (network to europe server, ~300ms ping):

```bash
# Individual SSH commands without ControlMaster:
$ time ssh europe "echo test"   # ~4.0s each

# With ControlMaster (first establishes, rest reuse):
$ time ssh -o ControlMaster=auto -o ControlPath=/tmp/test europe "echo 1"  # ~4.0s
$ time ssh -o ControlMaster=auto -o ControlPath=/tmp/test europe "echo 2"  # ~0.6s
$ time ssh -o ControlMaster=auto -o ControlPath=/tmp/test europe "echo 3"  # ~0.6s
```

**Steps to reproduce**:

```python
import sypy
import time

# Clean state
import os
os.system('rm -rf /tmp/sy-daemon/')
os.system('ssh europe "pkill -f sy.*daemon; rm -f ~/.sy/daemon.sock"')

# Time daemon_start
start = time.time()
info = sypy.daemon_start('user@europe:/path')
print(f'Took {time.time() - start:.1f}s')  # Prints ~7-15s instead of ~2s
```

**Expected behavior**: Daemon startup should take ~2-3s by:

1. Establishing ControlMaster connection FIRST (~4s)
2. Running all subsequent commands through ControlMaster (~0.6s each)
3. Total: ~5-6s instead of ~12-15s

**Suggested fix in** `src/sync/daemon_auto.rs`:

```rust
// In ensure_daemon_connection_with_config():
// 1. First establish ControlMaster with a simple command
// 2. Then run daemon check/start/verify through the ControlMaster
// 3. Then set up socket forwarding
```

**Impact**: High - daemon mode is supposed to be fast, but first connection is very slow

**Location**: `src/sync/daemon_auto.rs` - functions `ensure_remote_daemon()` and `run_ssh_command()`

---

## Bug 8: DaemonConfig.socket_dir is ignored

**Description**: When creating a `DaemonConfig` with a custom `socket_dir`, the setting is ignored and sockets are still created in the default `/tmp/sy-daemon/` directory.

**Steps to reproduce**:

```python
import sypy
import os

os.system('rm -rf /tmp/sy-daemon/ /tmp/my-custom-daemon/')

config = sypy.DaemonConfig(
    socket_dir='/tmp/my-custom-daemon',
    control_persist='5m',
)

print(f'Configured socket_dir: {config.socket_dir}')  # /tmp/my-custom-daemon

info = sypy.daemon_start('user@europe:/path', daemon=config)
print(f'Actual socket_path: {info.socket_path}')  # /tmp/sy-daemon/europe.sock (WRONG!)

# Verify:
os.system('ls -la /tmp/my-custom-daemon/')  # Directory doesn't exist!
os.system('ls -la /tmp/sy-daemon/')  # Socket is here instead
```

**Root cause**: In `sypy/src/daemon.rs`, the `py_daemon_start()` function creates `daemon_config` but then calls `ensure_daemon_connection_with_config()` without passing the socket_dir. The underlying Rust function uses hardcoded `SOCKET_DIR = "/tmp/sy-daemon"`.

**Location**:

- `sypy/src/daemon.rs:160` - `daemon_config` is created but `socket_dir` not passed
- `src/sync/daemon_auto.rs:20` - hardcoded `const SOCKET_DIR: &str = "/tmp/sy-daemon"`

**Suggested fix**: Pass `daemon_config.socket_dir` to `ensure_daemon_connection_with_config()` and use it instead of the hardcoded constant.

**Impact**: Medium - users cannot customize socket location, which may be needed for permissions or organization

---

## Bug 9: Daemon socket naming can cause collisions

**Description**: The daemon socket naming scheme only uses the hostname, not the user or a hash of the full connection details. This can cause collisions in several scenarios.

**Current naming logic** (from `src/sync/daemon_auto.rs`):

```rust
let safe_host = host.replace(['/', ':', '@'], "_");
let local_socket = socket_dir.join(format!("{}.sock", safe_host));
```

**Collision scenarios**:

1. **Different users, same host**:

    ```python
    # Both create socket named "myserver.sock"
    sypy.daemon_start('user1@myserver:/path')  # -> myserver.sock
    sypy.daemon_start('user2@myserver:/path')  # -> myserver.sock (COLLISION!)
    ```

2. **SSH alias vs resolved hostname**:

    ```python
    # If SSH config has: Host prod -> 192.168.1.10
    sypy.daemon_start('user@prod:/path')        # -> prod.sock
    sypy.daemon_start('user@192.168.1.10:/path') # -> 192.168.1.10.sock
    # These are the same server but different sockets - inconsistent
    ```

3. **Same alias name, different machines** (multi-user or config changes):
    ```python
    # User A has: Host myserver -> 10.0.0.1
    # User B has: Host myserver -> 10.0.0.2
    # Both generate: myserver.sock
    # If running on shared system, they conflict!
    ```

**Suggested fix**: Include user in socket name and optionally hash the full config:

```rust
// Option 1: Include user
let local_socket = socket_dir.join(format!("{}@{}.sock", user, safe_host));

// Option 2: Hash the full connection details
let config_hash = hash(format!("{}@{}:{}", user, hostname, port));
let local_socket = socket_dir.join(format!("{}.sock", config_hash));
```

**Impact**: Medium - can cause hard-to-debug issues when working with multiple servers or users

**Location**: `src/sync/daemon_auto.rs:82-84`

---

## Bug 10: Partial socket cleanup causes daemon timeout

**Description**: If the local daemon socket file is deleted but the SSH ControlMaster socket remains, subsequent `daemon_auto` syncs fail with a timeout error instead of recovering gracefully.

**Steps to reproduce**:

```python
import sypy
import os

# Start daemon
info = sypy.daemon_start('user@europe:/path')
print(f'Daemon started: {info.socket_path}')

# Sync works
stats = sypy.sync('/tmp/test/', 'user@europe:/tmp/dest/', daemon_auto=True)
print('Initial sync OK')

# Simulate partial cleanup (e.g., /tmp cleanup, manual deletion)
os.system('rm -f /tmp/sy-daemon/europe.sock')  # Remove socket but not control
os.system('ls -la /tmp/sy-daemon/')  # Shows: europe.control still exists

# daemon_check correctly returns False
print(f'daemon_check: {sypy.daemon_check("user@europe:/path")}')  # False

# But sync with daemon_auto fails instead of recovering!
try:
    stats = sypy.sync('/tmp/test/', 'user@europe:/tmp/dest/', daemon_auto=True)
except RuntimeError as e:
    print(f'Error: {e}')  # "Timeout waiting for socket at /tmp/sy-daemon/europe.sock"
```

**Expected behavior**: When local socket is missing but control socket exists, the code should either:

1. Clean up the stale control socket and start fresh, OR
2. Re-establish the socket forwarding using the existing ControlMaster

**Root cause**: The code in `ensure_daemon_connection_with_config()` checks if the socket exists and is connectable, but when it's missing, it tries to set up forwarding again. However, the existing ControlMaster may interfere or the forwarding setup may not work correctly with a stale control socket.

**Workaround**: Full cleanup before retrying:

```python
os.system('rm -rf /tmp/sy-daemon/')  # Remove ALL sockets
stats = sypy.sync('/tmp/test/', 'user@europe:/tmp/dest/', daemon_auto=True)  # Works
```

**Impact**: Medium - partial failures require manual cleanup

**Location**: `src/sync/daemon_auto.rs` - `ensure_daemon_connection_with_config()` and `setup_socket_forwarding()`

---

## Bug 11: S3/GCS get() fails when destination is a directory

**Severity**: Medium

**Description**: The `get()` function fails with "Is a directory (os error 21)" when downloading a single file from S3 or GCS to a directory destination. SSH `get()` works correctly.

**Steps to reproduce**:

```python
import sypy
import os

# Upload a file first
sypy.put('/tmp/test.txt', 's3://bucket/test-path/')

# Try to download to a directory
os.makedirs('/tmp/dest', exist_ok=True)

# This fails:
sypy.get('s3://bucket/test-path/test.txt', '/tmp/dest/')
# RuntimeError: Is a directory (os error 21)

# This also fails (no trailing slash):
sypy.get('s3://bucket/test-path/test.txt', '/tmp/dest')
# RuntimeError: Is a directory (os error 21)

# This works (explicit filename):
sypy.get('s3://bucket/test-path/test.txt', '/tmp/dest/test.txt')
# Success!
```

**Affected backends**:

- ✗ S3 - fails
- ✗ GCS - fails
- ✓ SSH - works correctly

**Workarounds**:

1. Specify explicit destination filename:

    ```python
    sypy.get('s3://bucket/path/file.txt', '/tmp/dest/file.txt')
    ```

2. Use `sync()` instead:

    ```python
    sypy.sync('s3://bucket/path/', '/tmp/dest/')
    ```

3. Use `get()` with `recursive=True` for directories:
    ```python
    sypy.get('s3://bucket/path/', '/tmp/dest/', recursive=True)
    # This works!
    ```

**Expected behavior**: When destination is a directory (exists and is a directory, or ends with `/`), the source filename should be preserved and the file downloaded as `{dest}/{filename}`.

**Root cause**: The `get()` operation for cloud storage doesn't detect that the destination is a directory and tries to write directly to the path, which fails because it's a directory.

**Location**: `src/ops/get.rs` - needs to check if destination is a directory and append source filename if so.

**Impact**: Medium - workarounds available, but behavior is inconsistent with SSH and unintuitive

---
