"""
TypedDict definitions for file operations (get, put, rm).

This module provides TypedDict types for type-safe usage of the
get(), put(), and rm() functions and their result objects.
"""

from typing import TypedDict


class FailedTransferDict(TypedDict):
    """
    TypedDict representing a failed file transfer.
    """

    path: str
    """Path of the file that failed."""

    error: str
    """Error message describing the failure."""


class GetResultDict(TypedDict):
    """
    TypedDict representing the result of a get (download) operation.

    Example:
        >>> result = sypy.get("s3://bucket/path/", "/local/", recursive=True)
        >>> result_dict: GetResultDict = result.to_dict()
    """

    source: str
    """Remote source path."""

    destination: str
    """Local destination path."""

    downloaded_files: int
    """Number of files downloaded."""

    downloaded_bytes: int
    """Total bytes downloaded."""

    created_dirs: int
    """Number of directories created."""

    skipped_files: int
    """Number of files skipped (already up-to-date)."""

    dry_run: bool
    """Whether this was a dry-run."""

    success: bool
    """Whether the operation completed without failures."""

    failed: list[FailedTransferDict]
    """List of failed transfers."""


class PutResultDict(TypedDict):
    """
    TypedDict representing the result of a put (upload) operation.

    Example:
        >>> result = sypy.put("/local/", "s3://bucket/path/", recursive=True)
        >>> result_dict: PutResultDict = result.to_dict()
    """

    source: str
    """Local source path."""

    destination: str
    """Remote destination path."""

    uploaded_files: int
    """Number of files uploaded."""

    uploaded_bytes: int
    """Total bytes uploaded."""

    created_dirs: int
    """Number of directories created."""

    skipped_files: int
    """Number of files skipped (already up-to-date)."""

    dry_run: bool
    """Whether this was a dry-run."""

    success: bool
    """Whether the operation completed without failures."""

    failed: list[FailedTransferDict]
    """List of failed transfers."""


class RemoveResultDict(TypedDict):
    """
    TypedDict representing the result of a rm (remove) operation.

    Example:
        >>> result = sypy.rm("s3://bucket/path/", recursive=True)
        >>> result_dict: RemoveResultDict = result.to_dict()
    """

    path: str
    """Path that was removed."""

    removed_files: int
    """Number of files removed."""

    removed_dirs: int
    """Number of directories removed."""

    dry_run: bool
    """Whether this was a dry-run."""

    success: bool
    """Whether the operation completed without failures."""

    failed: list[FailedTransferDict]
    """List of failed removals."""
