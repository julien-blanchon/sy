# Python Bindings (sypy)

Python bindings for sy using PyO3 and Maturin.

## Architecture

```
sypy/
├── src/           # Rust PyO3 bindings
│   ├── lib.rs     # Module entry point
│   ├── sync.rs    # sync() and sync_with_options()
│   ├── options.rs # PySyncOptions wrapper
│   ├── stats.rs   # PySyncStats wrapper
│   ├── path.rs    # PySyncPath wrapper
│   ├── config.rs  # S3Config, GcsConfig, SshConfig, CloudClientOptions
│   ├── error.rs   # Error conversion
│   └── progress.rs# Progress callbacks (WIP)
└── python/sypy/   # Python package
    ├── __init__.py
    └── _sypy.pyi  # Type stubs
```

## Design Decisions

### 1. Explicit Credentials over Environment Variables

Credentials can be passed explicitly via config classes (`S3Config`, `GcsConfig`, `SshConfig`) rather than requiring environment variables. More Pythonic and practical for programmatic use.

```python
s3 = sypy.S3Config(access_key_id="...", secret_access_key="...")
stats = sypy.sync("/local/", "s3://bucket/", s3=s3)
```

### 2. Shared CloudClientOptions

HTTP client settings (connection pooling, timeouts, retries) are shared between S3 and GCS via `CloudClientOptions`. Located in `sy::transport::cloud` module to avoid coupling GCS to S3.

Presets provided: `high_throughput()`, `low_latency()`.

### 3. Wrapper Types

Each core sy type has a Py-prefixed wrapper:
- `sy::sync::SyncStats` → `PySyncStats`
- `sy::path::SyncPath` → `PySyncPath`
- `sy::cli::Cli` → `PySyncOptions`

Wrappers convert between Rust and Python representations, exposing only what's needed for the Python API.

### 4. Async Runtime

Sync functions use `tokio::runtime::Runtime::block_on()` internally. Python API is synchronous - users can use threading/asyncio externally if needed.

### 5. Error Handling

`sy::error::SyncError` variants map to Python exceptions via `SyncPyError`. Anyhow errors become generic `RuntimeError`.

## API Surface

- `sync(source, dest, **kwargs)` - Main sync function
- `sync_with_options(source, dest, options)` - With SyncOptions object
- `parse_path(path)` - Parse path strings
- Config classes: `S3Config`, `GcsConfig`, `SshConfig`, `CloudClientOptions`
- Result types: `SyncStats`, `SyncPath`, `SyncError`

## Build

```bash
cd sypy
pip install maturin
maturin develop --features "s3,gcs"
```

## Performance

Python overhead is negligible (<1%). The actual sync work happens in Rust. Benchmarks show sypy performs identically to native sy CLI.
