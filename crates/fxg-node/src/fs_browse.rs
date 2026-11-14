//! Local filesystem browsing on the node (`/api/v1/nodes/:id/fs/browse`).

use std::fs;
use std::path::{Path, PathBuf};

use fxg_protocol::client_api::{FsBrowseResponse, FsEntry};

use crate::error::NodeError;

/// Resolves the path and browses the contents of the given directory.
///
/// - If `path` is `None`, browse the default (the home directory, or the root if
///   unavailable).
/// - On Windows, if `path` is the empty string `Some("")`, return the list of
///   available drives.
pub fn browse_fs(path: Option<&str>) -> Result<FsBrowseResponse, NodeError> {
    match path {
        Some("") => {
            #[cfg(windows)]
            {
                Ok(FsBrowseResponse {
                    current_path: String::new(),
                    parent_path: None,
                    entries: list_windows_drives(),
                })
            }
            #[cfg(not(windows))]
            {
                browse_directory(Path::new("/"))
            }
        }
        Some(raw_path) => {
            let target = PathBuf::from(raw_path);
            let canonical = dunce::canonicalize(&target).map_err(|err| NodeError::Io {
                path: target.clone(),
                source: err,
            })?;
            if !canonical.is_dir() {
                return Err(NodeError::Io {
                    path: canonical,
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        "specified path is not a directory",
                    ),
                });
            }
            browse_directory(&canonical)
        }
        None => {
            if let Some(home) = dirs::home_dir()
                && home.is_dir()
            {
                let canonical = dunce::canonicalize(&home).unwrap_or(home);
                return browse_directory(&canonical);
            }
            #[cfg(windows)]
            {
                Ok(FsBrowseResponse {
                    current_path: String::new(),
                    parent_path: None,
                    entries: list_windows_drives(),
                })
            }
            #[cfg(not(windows))]
            {
                browse_directory(Path::new("/"))
            }
        }
    }
}

/// Reads the contents of a single directory and builds an `FsBrowseResponse`.
fn browse_directory(dir: &Path) -> Result<FsBrowseResponse, NodeError> {
    let current_path = dir.to_string_lossy().to_string();

    let parent_path = if let Some(parent) = dir.parent() {
        let parent_str = parent.to_string_lossy().to_string();
        if parent_str.is_empty() {
            None
        } else {
            Some(parent_str)
        }
    } else {
        #[cfg(windows)]
        {
            // On Windows, at a drive root (e.g. "C:\\") make the parent the empty string
            // so the user can go back to the drive list
            Some(String::new())
        }
        #[cfg(not(windows))]
        {
            None
        }
    };

    let read_dir = fs::read_dir(dir).map_err(|err| NodeError::Io {
        path: dir.to_path_buf(),
        source: err,
    })?;

    let mut entries = Vec::new();
    for item in read_dir {
        let entry = match item {
            Ok(entry) => entry,
            Err(_) => continue,
        };

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        let name = entry.file_name().to_string_lossy().to_string();
        let path_buf = entry.path();
        let canonical_entry = dunce::canonicalize(&path_buf).unwrap_or(path_buf);
        let path_str = canonical_entry.to_string_lossy().to_string();
        let is_dir = file_type.is_dir();
        let is_hidden = is_entry_hidden(&entry);

        entries.push(FsEntry {
            name,
            path: path_str,
            is_dir,
            is_hidden,
        });
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(FsBrowseResponse {
        current_path,
        parent_path,
        entries,
    })
}

/// Determines whether the entry is hidden (dot-prefixed, or hidden on Windows).
#[cfg(not(windows))]
fn is_entry_hidden(entry: &fs::DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

/// Determines whether the entry is hidden (dot-prefixed, or hidden on Windows).
#[cfg(windows)]
fn is_entry_hidden(entry: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    let name = entry.file_name();
    name.to_string_lossy().starts_with('.')
        || entry
            .metadata()
            .is_ok_and(|meta| meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

#[cfg(windows)]
fn list_windows_drives() -> Vec<FsEntry> {
    let mut drives = Vec::new();
    for letter in b'A'..=b'Z' {
        let drive_str = format!("{}:\\", letter as char);
        let path = Path::new(&drive_str);
        if path.exists() {
            drives.push(FsEntry {
                name: format!("{}:", letter as char),
                path: drive_str,
                is_dir: true,
                is_hidden: false,
            });
        }
    }
    drives
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_current_dir() {
        let current = std::env::current_dir().unwrap();
        let current_str = current.to_string_lossy().to_string();
        let response = browse_fs(Some(&current_str)).expect("browse current directory");
        assert!(!response.entries.is_empty());
        assert!(response.entries.iter().any(|e| e.name == "Cargo.toml"));
    }

    #[test]
    fn browse_default_resolves_home_or_root() {
        let response = browse_fs(None).expect("browse default");
        assert!(!response.current_path.is_empty() || !response.entries.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn browse_empty_string_lists_drives_on_windows() {
        let response = browse_fs(Some("")).expect("browse drives");
        assert!(response.current_path.is_empty());
        assert!(response.parent_path.is_none());
        assert!(!response.entries.is_empty());
        assert!(response.entries.iter().any(|e| e.name.ends_with(':')));
    }
}
