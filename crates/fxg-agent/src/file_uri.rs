//! Conversion between node-local paths and `file:` URIs.
//!
//! Agents exchange attachments and resource links as `file:` URIs. The `url`
//! crate handles Windows drive letters, UNC shares and percent-encoding, so
//! paths with spaces or non-ASCII characters round-trip correctly.

use std::path::Path;

use url::Url;

/// Builds a `file:` URI for a node-local path.
pub(crate) fn to_uri(path: &str) -> String {
    match Url::from_file_path(path) {
        Ok(url) => url.to_string(),
        Err(()) => {
            // A relative path has no valid `file:` URI. Keep a readable
            // representation instead of silently inventing a working directory.
            tracing::debug!(path, "cannot build a file URI for a relative path");
            format!("file://{path}")
        }
    }
}

/// Returns the node-local path from a `file:` URI (percent-decoded).
///
/// Bare paths (the legacy OpenCode attachment shape) are returned unchanged;
/// other URI schemes are not node-local paths.
pub(crate) fn to_path(uri: &str) -> Option<String> {
    match Url::parse(uri) {
        Ok(url) if url.scheme() == "file" => match url.to_file_path() {
            Ok(path) => Some(path.to_string_lossy().into_owned()),
            // A drive-less `file:///repo/notes.md` has no absolute Windows
            // path. Keep the legacy shape instead of dropping the attachment.
            Err(()) => legacy_file_path(uri),
        },
        Ok(url) if url.has_host() || url.cannot_be_a_base() => None,
        Ok(_) => Some(uri.to_owned()),
        Err(_) => Some(uri.to_owned()),
    }
}

/// Best-effort path for a `file:` URI that `Url::to_file_path` cannot express.
fn legacy_file_path(uri: &str) -> Option<String> {
    let url = match Url::parse(uri) {
        Ok(url) => url,
        Err(_) => return None,
    };
    #[cfg(not(windows))]
    if url.host().is_some() {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(url.path())
        .decode_utf8_lossy()
        .into_owned();
    #[cfg(windows)]
    let decoded = decoded.strip_prefix('/').unwrap_or(&decoded);
    if decoded.is_empty() {
        None
    } else {
        Some(decoded.to_owned())
    }
}

/// Returns the file name from a `file:` URI or any other URI.
pub(crate) fn file_name(uri: &str) -> Option<String> {
    if let Some(path) = to_path(uri)
        && let Some(name) = Path::new(&path).file_name()
        && !name.is_empty()
    {
        return Some(name.to_string_lossy().into_owned());
    }
    let url = match Url::parse(uri) {
        Ok(url) => url,
        Err(_) => return None,
    };
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    const PATH: &str = "/tmp/日本語 ファイル.txt";
    #[cfg(windows)]
    const PATH: &str = r"C:\tmp\日本語 ファイル.txt";

    #[test]
    fn round_trips_spaces_and_non_ascii() {
        let uri = to_uri(PATH);
        assert!(uri.starts_with("file:"));
        assert!(!uri.contains(' '), "path must be percent-encoded: {uri}");
        assert_eq!(to_path(&uri).as_deref(), Some(PATH));
        assert_eq!(file_name(&uri).as_deref(), Some("日本語 ファイル.txt"));
    }

    #[test]
    fn non_file_uris_are_not_local_paths() {
        assert_eq!(to_path("https://example.com/a.png"), None);
        assert_eq!(to_path("data:image/png;base64,aGk="), None);
    }

    #[test]
    fn bare_paths_are_preserved() {
        assert_eq!(to_path("/tmp/a b").as_deref(), Some("/tmp/a b"));
        assert_eq!(
            file_name("https://example.com/a%20b.png").as_deref(),
            Some("a%20b.png")
        );
    }
}
