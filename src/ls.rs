//! Directory listing functionality (similar to rclone lsjson)
//!
//! This module provides efficient directory listing across all supported transports
//! (local, SSH, S3, GCS) with optional JSON output and recursive traversal.

use crate::error::Result;
use crate::sync::scanner::FileEntry;
use crate::transport::Transport;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::SystemTime;

/// A single entry in the listing output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEntry {
    /// Full path to the entry
    pub path: String,

    /// File size in bytes (0 for directories)
    #[serde(rename = "Size")]
    pub size: u64,

    /// File modification time (RFC3339 format)
    #[serde(rename = "ModTime")]
    pub mod_time: String,

    /// Whether this is a directory
    #[serde(rename = "IsDir")]
    pub is_dir: bool,

    /// Entry type: "file", "directory", or "symlink"
    #[serde(rename = "Type")]
    pub entry_type: String,

    /// MIME type hint (if available/inferred from extension)
    #[serde(rename = "MimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,

    /// Symlink target (if this is a symlink)
    #[serde(rename = "SymlinkTarget", skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,

    /// Whether this is a sparse file
    #[serde(rename = "IsSparse", skip_serializing_if = "Option::is_none")]
    pub is_sparse: Option<bool>,

    /// Actual allocated size on disk (may differ from Size for sparse files)
    #[serde(rename = "AllocatedSize", skip_serializing_if = "Option::is_none")]
    pub allocated_size: Option<u64>,

    /// Inode number (Unix only)
    #[serde(rename = "Inode", skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,

    /// Number of hard links to this file
    #[serde(rename = "NumLinks", skip_serializing_if = "Option::is_none")]
    pub num_links: Option<u64>,
}

impl ListEntry {
    /// Convert a FileEntry from the scanner to a ListEntry
    pub fn from_file_entry(entry: &FileEntry, _base_path: &Path) -> Self {
        let path_str = entry.relative_path.to_string_lossy().to_string();

        // Format modification time as RFC3339
        let mod_time = format_rfc3339(entry.modified);

        // Determine entry type
        let entry_type = if entry.is_symlink {
            "symlink".to_string()
        } else if entry.is_dir {
            "directory".to_string()
        } else {
            "file".to_string()
        };

        // Infer MIME type from extension (optional, simple heuristic)
        let mime_type = if !entry.is_dir {
            infer_mime_type(&path_str)
        } else {
            None
        };

        Self {
            path: path_str,
            size: entry.size,
            mod_time,
            is_dir: entry.is_dir,
            entry_type,
            mime_type,
            symlink_target: entry
                .symlink_target
                .as_ref()
                .map(|t| t.to_string_lossy().to_string()),
            is_sparse: if entry.is_sparse { Some(true) } else { None },
            allocated_size: if entry.is_sparse || entry.allocated_size != entry.size {
                Some(entry.allocated_size)
            } else {
                None
            },
            inode: entry.inode,
            num_links: if entry.nlink > 1 {
                Some(entry.nlink)
            } else {
                None
            },
        }
    }
}

/// Listing options
#[derive(Debug, Clone)]
pub struct ListOptions {
    /// Whether to recurse into subdirectories
    pub recursive: bool,

    /// Maximum depth to recurse (None = unlimited)
    pub max_depth: Option<usize>,

    /// Whether to include directories in output
    pub include_dirs: bool,

    /// Whether to include file/symlink entries
    pub include_files: bool,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            max_depth: None,
            include_dirs: true,
            include_files: true,
        }
    }
}

impl ListOptions {
    /// Create options for recursive listing
    pub fn recursive() -> Self {
        Self {
            recursive: true,
            ..Default::default()
        }
    }

    /// Create options for non-recursive listing (direct children only)
    pub fn flat() -> Self {
        Self {
            recursive: false,
            ..Default::default()
        }
    }

    /// Set maximum recursion depth
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = Some(depth);
        self
    }
}

/// List directory contents using the specified transport
///
/// Returns a vector of ListEntry objects with file metadata.
/// This function works across all supported transports (local, SSH, S3, GCS).
pub async fn list_directory<T: Transport>(
    transport: &T,
    path: &Path,
    options: &ListOptions,
) -> Result<Vec<ListEntry>> {
    // Use the transport's scan method which handles all platform specifics
    let entries = if options.recursive {
        // Recursive: scan entire tree
        transport.scan(path).await?
    } else {
        // Non-recursive: use efficient scan_flat (uses delimiter for S3/GCS)
        transport.scan_flat(path).await?
    };

    // Convert to ListEntry format
    let mut list_entries: Vec<ListEntry> = entries
        .iter()
        .filter(|e| {
            // Apply filters based on options
            if !options.include_dirs && e.is_dir {
                return false;
            }
            if !options.include_files && !e.is_dir {
                return false;
            }

            // Apply max_depth filter if set
            if let Some(max_depth) = options.max_depth {
                let depth = e.relative_path.components().count();
                if depth > max_depth {
                    return false;
                }
            }

            true
        })
        .map(|e| ListEntry::from_file_entry(e, path))
        .collect();

    // Sort by path for consistent output
    list_entries.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(list_entries)
}

/// Filter entries to only include direct children (non-recursive listing)
#[allow(dead_code)]
fn filter_direct_children(entries: Vec<FileEntry>, _base_path: &Path) -> Vec<FileEntry> {
    entries
        .into_iter()
        .filter(|entry| {
            // Count path components after stripping base
            let components: Vec<_> = entry.relative_path.components().collect();

            // Direct child has exactly 1 component (file or dir name)
            components.len() == 1
        })
        .collect()
}

/// Format SystemTime as RFC3339 string
fn format_rfc3339(time: SystemTime) -> String {
    use std::time::UNIX_EPOCH;

    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();

    // Convert to datetime components using chrono-like logic
    // Days since Unix epoch (1970-01-01)
    let days_since_epoch = (secs / 86400) as i64;
    let secs_today = secs % 86400;

    let hours = secs_today / 3600;
    let minutes = (secs_today % 3600) / 60;
    let seconds = secs_today % 60;
    let millis = nanos / 1_000_000;

    // Calculate year, month, day from days since epoch
    // This is a simplified calculation (not handling leap years perfectly)
    let mut year = 1970;
    let mut remaining_days = days_since_epoch;

    // Advance by full years
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    // Calculate month and day
    let days_in_months = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    let mut day = remaining_days + 1;

    for (m, &days) in days_in_months.iter().enumerate() {
        if day <= days {
            month = m + 1;
            break;
        }
        day -= days;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hours, minutes, seconds, millis
    )
}

/// Check if a year is a leap year
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Infer MIME type from file extension (simple heuristic)
fn infer_mime_type(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_lowercase();

    let mime = match ext.as_str() {
        // Text
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        "c" | "h" => "text/x-c",
        "cpp" | "cc" | "cxx" => "text/x-c++",
        "java" => "text/x-java",
        "go" => "text/x-go",

        // Images
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",

        // Video
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",

        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",

        // Archives
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "bz2" => "application/x-bzip2",
        "xz" => "application/x-xz",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/x-rar-compressed",

        // Documents
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",

        _ => return None,
    };

    Some(mime.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_mime_type() {
        assert_eq!(infer_mime_type("file.txt"), Some("text/plain".to_string()));
        assert_eq!(
            infer_mime_type("file.json"),
            Some("application/json".to_string())
        );
        assert_eq!(infer_mime_type("file.jpg"), Some("image/jpeg".to_string()));
        assert_eq!(infer_mime_type("file.mp4"), Some("video/mp4".to_string()));
        assert_eq!(infer_mime_type("file.unknown"), None);
    }

    #[test]
    fn test_default_options() {
        let opts = ListOptions::default();
        assert!(!opts.recursive);
        assert!(opts.include_dirs);
        assert!(opts.include_files);
        assert_eq!(opts.max_depth, None);
    }

    #[test]
    fn test_recursive_options() {
        let opts = ListOptions::recursive();
        assert!(opts.recursive);
    }

    #[test]
    fn test_flat_options() {
        let opts = ListOptions::flat();
        assert!(!opts.recursive);
    }
}
