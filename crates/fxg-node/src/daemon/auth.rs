//! クライアント認証トークン (`~/.flexagent/auth_token`) の生成・読み書き。
//!
//! 設計: `docs/01-architecture-and-sync.md` §7.3。
//!
//! - 初回起動時に **64文字のランダム文字列** (32 バイトの乱数を hex 表現) を生成し、
//!   パーミッション `0600` で `~/.flexagent/auth_token` に保存する。
//! - 以降のリクエストは `Authorization: Bearer <token>` または
//!   `Cookie: fxg_session=<token>` で認証する。

use std::path::Path;

use crate::error::NodeError;

/// トークンの乱数バイト数 (hex 表現で 64 文字)。
pub const TOKEN_BYTES: usize = 32;

/// 新しいランダムトークンを生成する (64 文字の hex 文字列)。
pub fn generate_token() -> String {
    let bytes: [u8; TOKEN_BYTES] = rand::random();
    let mut token = String::with_capacity(TOKEN_BYTES * 2);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    token
}

/// トークンをファイルから読み込む (存在しなければ生成して保存する)。
pub fn load_or_create(path: &Path) -> Result<String, NodeError> {
    match std::fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => { /* 空ファイルは再生成する */ }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(NodeError::io(path, err)),
    }
    let token = generate_token();
    save(path, &token)?;
    Ok(token)
}

/// トークンをファイルへ保存する (可能な環境ではオーナーのみ読み書き可 `0600`)。
pub fn save(path: &Path, token: &str) -> Result<(), NodeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| NodeError::io(parent, err))?;
    }
    std::fs::write(path, token).map_err(|err| NodeError::io(path, err))?;
    set_owner_only_permissions(path)?;
    Ok(())
}

/// トークンを定数時間で比較する (タイミング攻撃の緩和)。
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
    // Windows ではユーザープロファイル配下 (`~/.flexagent`) の ACL に従う
    // (追加の ACL 操作は Phase 5 の `fxg service install` で扱う)。
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

        // 空ファイルは再生成される
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
        assert_eq!(mode & 0o777, 0o600, "auth_token は 0600 で保存する");
    }

    #[test]
    fn token_comparison_is_constant_time_like() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("abc", ""));
    }
}
