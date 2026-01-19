"""
TypedDict definitions for sy-ls functionality.

This module provides TypedDict types for type-safe usage of the ls() function
and ListEntry objects.
"""

from typing import TypedDict


class ListEntryDict(TypedDict, total=False):
    """
    TypedDict representing a directory listing entry.

    All fields are required except those marked Optional.
    Use this for type hints when calling entry.to_dict().

    Example:
        >>> entries = sypy.ls("/path")
        >>> dicts: list[ListEntryDict] = [e.to_dict() for e in entries]

    """

    path: str
    """Relative path of the entry."""

    size: int
    """File size in bytes (0 for directories)."""

    mod_time: str
    """Modification time in RFC3339 format."""

    is_dir: bool
    """Whether this is a directory."""

    entry_type: str
    """Entry type: "file", "directory", or "symlink"."""

    mime_type: str | None
    """MIME type (inferred from extension)."""

    symlink_target: str | None
    """Symlink target path (if this is a symlink)."""

    is_sparse: bool | None
    """Whether this is a sparse file."""

    allocated_size: int | None
    """Actual allocated size on disk."""

    inode: int | None
    """Inode number (Unix only)."""

    num_links: int | None
    """Number of hard links to this file."""
