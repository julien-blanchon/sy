"""
Entry point for `sy-remote` command.

This is the remote helper binary used by SSH transport to execute
operations on remote hosts. It provides commands like:
- scan: List directory contents as JSON
- checksums: Compute block checksums for delta sync
- file-checksum: Compute file checksum for verification
- apply-delta: Apply delta operations
- receive-file: Receive file data from stdin
- receive-sparse-file: Receive sparse file with data regions
"""

import sys


def main() -> int:
    """Main entry point for the sy-remote CLI."""
    from sypy._sypy import remote_main as _remote_main

    return _remote_main(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
