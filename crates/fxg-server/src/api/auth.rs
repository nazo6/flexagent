//! Utilities for generating, saving and comparing auth tokens (client token / per-node token).
//!
//! Design: `docs/01-architecture-and-sync.md` §7.3.
//!
//! - **Client auth token** (`~/.flexagent/auth_token`): used by both the local node
//!   (`fxg daemon`) and the central server (`fxg server`) to authenticate Client REST/WS API
//!   requests (`Authorization: Bearer` / `fxg_session` cookie) (a 64-character random hex
//!   string is generated on first startup and saved with `0600`).
//! - **Per-node token** (`~/.flexagent/node_token`): Node ⇔ Server pairing authentication. The
//!   server stores only the [`hash_token`] (SHA-256 hex) in `server.db.nodes.token_hash`; the
//!   plaintext is shown only once at issue time.

use std::path::Path;

use sha2::{Digest, Sha256};

/// Number of random bytes in a token (64 characters in hex).
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

/// Computes the SHA-256 hash (hex) of a token (for storing in `nodes.token_hash`).
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hash.push_str(&format!("{byte:02x}"));
    }
    hash
}

/// Loads a token from a file (generating and saving one if absent).
pub fn load_or_create_token(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => { /* regenerate an empty file */ }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let token = generate_token();
    save_token(path, &token)?;
    Ok(token)
}

/// Saves a token to a file (owner-only read/write `0600` where supported).
pub fn save_token(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, token)?;
    set_owner_only_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> std::io::Result<()> {
    // On Windows, follow the ACLs of the user profile directory (`~/.flexagent`)
    // (additional ACL handling is covered by `fxg service install`).
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
    fn token_matches_is_exact() {
        assert!(token_matches("abcd", "abcd"));
        assert!(!token_matches("abcd", "abce"));
        assert!(!token_matches("abcd", "abcde"));
    }

    #[test]
    fn hash_token_is_stable_sha256_hex() {
        // SHA-256 of "abc" (known test vector)
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash_token("").len(), 64);
    }

    #[test]
    fn load_or_create_token_persists_and_reuses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth_token");
        let first = load_or_create_token(&path).expect("create");
        assert!(path.is_file());
        assert_eq!(first, load_or_create_token(&path).expect("load"));
        std::fs::write(&path, "").expect("write empty");
        assert_ne!(first, load_or_create_token(&path).expect("regenerate"));
    }
}
