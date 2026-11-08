//! Generation and persistence of the client auth token (`~/.flexagent/auth_token`).
//!
//! Design: `docs/01-architecture-and-sync.md` §7.3.
//!
//! - On first start, a **64-character random string** (a 32-byte random value in
//!   hex form) is generated and stored in `~/.flexagent/auth_token` with
//!   permission `0600`.
//! - Subsequent requests authenticate with `Authorization: Bearer <token>` or
//!   `Cookie: fxg_session=<token>`.

use std::path::Path;

use crate::error::NodeError;

/// Number of random bytes in a token (64 characters in hex form).
pub const TOKEN_BYTES: usize = 32;

/// Generates a new random token (a 64-character hex string).
pub fn generate_token() -> String {
    let bytes: [u8; TOKEN_BYTES] = rand::random();
    let mut token = String::with_capacity(TOKEN_BYTES * 2);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    token
}

/// Loads the token from a file (generating and saving one if it does not exist).
pub fn load_or_create(path: &Path) -> Result<String, NodeError> {
    match std::fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => { /* regenerate an empty file */ }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(NodeError::io(path, err)),
    }
    let token = generate_token();
    save(path, &token)?;
    Ok(token)
}

/// Saves the token to a file (owner read/write only, `0600`, where supported).
pub fn save(path: &Path, token: &str) -> Result<(), NodeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| NodeError::io(parent, err))?;
    }
    std::fs::write(path, token).map_err(|err| NodeError::io(path, err))?;
    set_owner_only_permissions(path)?;
    Ok(())
}

/// Compares tokens in constant time (mitigates timing attacks).
pub fn token_matches(expected: &str, provided: &str) -> bool {
    let expected = expected.as_bytes();
    let provided = provided.as_bytes();
    if expected.len() != provided.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(provided.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<(), NodeError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|err| NodeError::io(path, err))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<(), NodeError> {
    // On Windows, rely on the ACL of the user profile directory (`~/.flexagent`)
    // (additional ACL handling is covered by `fxg service install` in Phase 5).
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_is_64_hex_chars_and_unique() {
        let token = generate_token();
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(token.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(token, generate_token());
    }

    #[test]
    fn load_or_create_persists_and_reuses_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth_token");

        let first = load_or_create(&path).expect("create");
        assert!(path.is_file());
        let second = load_or_create(&path).expect("load");
        assert_eq!(first, second);

        // An empty file is regenerated
        std::fs::write(&path, "").expect("write empty");
        let third = load_or_create(&path).expect("regenerate");
        assert_ne!(third, first);
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth_token");
        save(&path, "secret").expect("save");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "auth_token must be saved with 0600");
    }

    #[test]
    fn token_comparison_is_constant_time_like() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("abc", ""));
    }
}
