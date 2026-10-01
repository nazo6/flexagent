//! ノードのローカルファイルシステム走査 (`/api/v1/nodes/:id/fs/browse`)。

use std::fs;
use std::path::{Path, PathBuf};

use fxg_protocol::client_api::{FsBrowseResponse, FsEntry};

use crate::error::NodeError;

/// パスを解決し、指定されたディレクトリの内容をブラウズする。
///
/// - `path` が `None` の場合は既定 (ホームディレクトリ、取得できなければルート) を閲覧する。
/// - Windows で `path` が空文字 `Some("")` の場合、利用可能なドライブ一覧を返す。
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

/// 単一ディレクトリの内容を読み込み、`FsBrowseResponse` を組み立てる。
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
            // Windows でドライブ直下 (例: "C:\\") の場合、親は空文字 (ドライブ一覧) に戻れるようにする
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

/// 隠し属性かどうかを判定する。
fn is_entry_hidden(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name();
    let name_str = name.to_string_lossy();
    if name_str.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(meta) = entry.metadata() {
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            if meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
                return true;
            }
        }
    }
    false
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
