//! Python wrappers for dry-run details

use pyo3::prelude::*;
use std::path::PathBuf;
use sy::sync::{ChangeAction, DirectoryChange, DryRunDetails, FileChange, SymlinkChange};

/// Action type for file/directory/symlink changes
#[pyclass(name = "ChangeAction")]
#[derive(Clone, Debug)]
pub enum PyChangeAction {
    Create,
    Update,
    Delete,
    Skip,
}

impl From<ChangeAction> for PyChangeAction {
    fn from(action: ChangeAction) -> Self {
        match action {
            ChangeAction::Create => PyChangeAction::Create,
            ChangeAction::Update => PyChangeAction::Update,
            ChangeAction::Delete => PyChangeAction::Delete,
            ChangeAction::Skip => PyChangeAction::Skip,
        }
    }
}

#[pymethods]
impl PyChangeAction {
    fn __str__(&self) -> String {
        match self {
            PyChangeAction::Create => "create".to_string(),
            PyChangeAction::Update => "update".to_string(),
            PyChangeAction::Delete => "delete".to_string(),
            PyChangeAction::Skip => "skip".to_string(),
        }
    }

    fn __repr__(&self) -> String {
        format!("ChangeAction.{}", self.__str__())
    }
}

/// Detailed information about a file change
#[pyclass(name = "FileChange")]
#[derive(Clone, Debug)]
pub struct PyFileChange {
    /// Relative path to the file
    #[pyo3(get)]
    pub path: String,

    /// Action type (create/update/delete/skip)
    #[pyo3(get)]
    pub action: String,

    /// File size in bytes
    #[pyo3(get)]
    pub size: u64,

    /// Actual bytes that would be transferred (may be less due to delta/compression)
    #[pyo3(get)]
    pub transfer_bytes: u64,

    /// Whether delta sync would be used
    #[pyo3(get)]
    pub would_use_delta: bool,

    /// Whether compression would be applied
    #[pyo3(get)]
    pub would_compress: bool,

    /// Reason for skipping (if action is Skip)
    #[pyo3(get)]
    pub skip_reason: Option<String>,
}

#[pymethods]
impl PyFileChange {
    fn __str__(&self) -> String {
        format!("{}: {} ({} bytes)", self.action, self.path, self.size)
    }

    fn __repr__(&self) -> String {
        format!(
            "FileChange(path='{}', action='{}', size={}, transfer_bytes={})",
            self.path, self.action, self.size, self.transfer_bytes
        )
    }

    /// Get as a dictionary for easy access
    fn to_dict(&self, py: Python<'_>) -> PyResult<PyObject> {
        use pyo3::types::PyDict;
        let dict = PyDict::new_bound(py);
        dict.set_item("path", &self.path)?;
        dict.set_item("action", &self.action)?;
        dict.set_item("size", self.size)?;
        dict.set_item("transfer_bytes", self.transfer_bytes)?;
        dict.set_item("would_use_delta", self.would_use_delta)?;
        dict.set_item("would_compress", self.would_compress)?;
        dict.set_item("skip_reason", &self.skip_reason)?;
        Ok(dict.into())
    }
}

impl From<&FileChange> for PyFileChange {
    fn from(fc: &FileChange) -> Self {
        PyFileChange {
            path: fc.path.to_string_lossy().to_string(),
            action: match fc.action {
                ChangeAction::Create => "create".to_string(),
                ChangeAction::Update => "update".to_string(),
                ChangeAction::Delete => "delete".to_string(),
                ChangeAction::Skip => "skip".to_string(),
            },
            size: fc.size,
            transfer_bytes: fc.transfer_bytes,
            would_use_delta: fc.would_use_delta,
            would_compress: fc.would_compress,
            skip_reason: fc.skip_reason.clone(),
        }
    }
}

/// Detailed information about a directory change
#[pyclass(name = "DirectoryChange")]
#[derive(Clone, Debug)]
pub struct PyDirectoryChange {
    /// Relative path to the directory
    #[pyo3(get)]
    pub path: String,

    /// Action type (create/delete)
    #[pyo3(get)]
    pub action: String,
}

#[pymethods]
impl PyDirectoryChange {
    fn __str__(&self) -> String {
        format!("{}: {}/", self.action, self.path)
    }

    fn __repr__(&self) -> String {
        format!(
            "DirectoryChange(path='{}', action='{}')",
            self.path, self.action
        )
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<PyObject> {
        use pyo3::types::PyDict;
        let dict = PyDict::new_bound(py);
        dict.set_item("path", &self.path)?;
        dict.set_item("action", &self.action)?;
        Ok(dict.into())
    }
}

impl From<&DirectoryChange> for PyDirectoryChange {
    fn from(dc: &DirectoryChange) -> Self {
        PyDirectoryChange {
            path: dc.path.to_string_lossy().to_string(),
            action: match dc.action {
                ChangeAction::Create => "create".to_string(),
                ChangeAction::Delete => "delete".to_string(),
                _ => "unknown".to_string(),
            },
        }
    }
}

/// Detailed information about a symlink change
#[pyclass(name = "SymlinkChange")]
#[derive(Clone, Debug)]
pub struct PySymlinkChange {
    /// Relative path to the symlink
    #[pyo3(get)]
    pub path: String,

    /// Action type (create/delete)
    #[pyo3(get)]
    pub action: String,

    /// Target path the symlink points to
    #[pyo3(get)]
    pub target: String,
}

#[pymethods]
impl PySymlinkChange {
    fn __str__(&self) -> String {
        format!("{}: {} -> {}", self.action, self.path, self.target)
    }

    fn __repr__(&self) -> String {
        format!(
            "SymlinkChange(path='{}', action='{}', target='{}')",
            self.path, self.action, self.target
        )
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<PyObject> {
        use pyo3::types::PyDict;
        let dict = PyDict::new_bound(py);
        dict.set_item("path", &self.path)?;
        dict.set_item("action", &self.action)?;
        dict.set_item("target", &self.target)?;
        Ok(dict.into())
    }
}

impl From<&SymlinkChange> for PySymlinkChange {
    fn from(sc: &SymlinkChange) -> Self {
        PySymlinkChange {
            path: sc.path.to_string_lossy().to_string(),
            action: match sc.action {
                ChangeAction::Create => "create".to_string(),
                ChangeAction::Delete => "delete".to_string(),
                _ => "unknown".to_string(),
            },
            target: sc.target.clone(),
        }
    }
}

/// Comprehensive dry-run details with file-level information
#[pyclass(name = "DryRunDetails")]
#[derive(Clone, Debug)]
pub struct PyDryRunDetails {
    /// List of file changes
    file_changes: Vec<PyFileChange>,

    /// List of directory changes
    directory_changes: Vec<PyDirectoryChange>,

    /// List of symlink changes
    symlink_changes: Vec<PySymlinkChange>,

    /// List of filtered file paths
    filtered_files: Vec<String>,

    /// Estimated duration in seconds
    #[pyo3(get)]
    pub estimated_duration_secs: f64,

    /// Estimated network bytes to transfer
    #[pyo3(get)]
    pub estimated_network_bytes: u64,
}

#[pymethods]
impl PyDryRunDetails {
    /// Get list of file changes
    #[getter]
    pub fn file_changes(&self) -> Vec<PyFileChange> {
        self.file_changes.clone()
    }

    /// Get list of directory changes
    #[getter]
    pub fn directory_changes(&self) -> Vec<PyDirectoryChange> {
        self.directory_changes.clone()
    }

    /// Get list of symlink changes
    #[getter]
    pub fn symlink_changes(&self) -> Vec<PySymlinkChange> {
        self.symlink_changes.clone()
    }

    /// Get list of filtered files
    #[getter]
    pub fn filtered_files(&self) -> Vec<String> {
        self.filtered_files.clone()
    }

    /// Get all changes as a list of dicts (most pythonic)
    pub fn all_changes(&self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        use pyo3::types::PyDict;
        let mut changes = Vec::new();

        // Add file changes
        for fc in &self.file_changes {
            let dict = fc.to_dict(py)?;
            let dict_bound = dict.bind(py).downcast::<PyDict>()?;
            dict_bound.set_item("type", "file")?;
            changes.push(dict);
        }

        // Add directory changes
        for dc in &self.directory_changes {
            let dict = dc.to_dict(py)?;
            let dict_bound = dict.bind(py).downcast::<PyDict>()?;
            dict_bound.set_item("type", "directory")?;
            changes.push(dict);
        }

        // Add symlink changes
        for sc in &self.symlink_changes {
            let dict = sc.to_dict(py)?;
            let dict_bound = dict.bind(py).downcast::<PyDict>()?;
            dict_bound.set_item("type", "symlink")?;
            changes.push(dict);
        }

        Ok(changes)
    }

    /// Get files by action type
    pub fn files_by_action(&self, action: &str) -> Vec<PyFileChange> {
        self.file_changes
            .iter()
            .filter(|fc| fc.action == action)
            .cloned()
            .collect()
    }

    /// Get total number of changes
    #[getter]
    pub fn total_changes(&self) -> usize {
        self.file_changes.len() + self.directory_changes.len() + self.symlink_changes.len()
    }

    fn __str__(&self) -> String {
        format!(
            "DryRunDetails({} files, {} dirs, {} symlinks, ~{:.1}s, ~{} bytes)",
            self.file_changes.len(),
            self.directory_changes.len(),
            self.symlink_changes.len(),
            self.estimated_duration_secs,
            self.estimated_network_bytes
        )
    }

    fn __repr__(&self) -> String {
        self.__str__()
    }
}

impl From<&DryRunDetails> for PyDryRunDetails {
    fn from(details: &DryRunDetails) -> Self {
        PyDryRunDetails {
            file_changes: details
                .file_changes
                .iter()
                .map(PyFileChange::from)
                .collect(),
            directory_changes: details
                .directory_changes
                .iter()
                .map(PyDirectoryChange::from)
                .collect(),
            symlink_changes: details
                .symlink_changes
                .iter()
                .map(PySymlinkChange::from)
                .collect(),
            filtered_files: details
                .filtered_files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            estimated_duration_secs: details.estimated_duration_secs,
            estimated_network_bytes: details.estimated_network_bytes,
        }
    }
}
