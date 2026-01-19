#!/usr/bin/env python3
"""Test progress callback with SSH sync."""
import sypy
import tempfile
import os
import time

def progress_callback(snapshot):
    pct = f"{snapshot.percentage:.1f}%" if snapshot.percentage is not None else "N/A"
    speed_mb = snapshot.bytes_per_sec / (1024 * 1024) if snapshot.bytes_per_sec else 0
    transferring = snapshot.transferring[:2] if snapshot.transferring else []
    print(f"  [{pct}] {snapshot.bytes}/{snapshot.total_bytes} bytes, "
          f"{snapshot.transfers}/{snapshot.total_transfers} files, "
          f"{speed_mb:.2f} MB/s, active={snapshot.active_transfers}")
    if transferring:
        print(f"    Transferring: {transferring}")

# Pull the files we just pushed back to a new local directory
with tempfile.TemporaryDirectory() as tmpdir:
    dest = os.path.join(tmpdir, "pulled")

    # Use the most recent test directory we pushed
    # List remote directories to find our test
    print("Testing SSH sync (PULL) from europe server with progress...")

    # We'll pull from /tmp/sy-ssh-test-* 
    source = "europe:/tmp/sy-ssh-test-1768818054/"
    print(f"Syncing {source} to {dest}")

    try:
        result = sypy.sync(
            source,
            dest,
            progress_callback=progress_callback,
            progress_frequency_ms=200,
        )
        print(f"\nResult: {result}")

        # Verify files were pulled
        if os.path.exists(dest):
            files = os.listdir(dest)
            print(f"Pulled {len(files)} files: {files[:5]}...")
    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
