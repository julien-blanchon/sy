"""
Entry point for `python -m sypy` command.

This allows running sy as a Python module:
    python -m sypy /source /dest
    python -m sypy --server /path
    python -m sypy --daemon --socket ~/.sy/daemon.sock
"""

import sys


def main() -> int:
    """Main entry point for the sy CLI."""
    from sypy._sypy import main as _main

    return _main(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
