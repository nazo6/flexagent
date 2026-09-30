//! 認証トークンの生成・保存・照合ユーティリティ (クライアントトークン / ノード個別トークン)。
//!
//! 設計: `docs/01-architecture-and-sync.md` §7.3。
//!
//! - **クライアント認証トークン** (`~/.flexagent/auth_token`): ローカルノード
//!   (`fxg daemon`) と中央サーバー (`fxg server`) の双方で、Client REST/WS API の
//!   認証 (`Authorization: Bearer` / `fxg_session` Cookie) に使用する
//!   (初回起動時に 64 文字のランダム hex 文字列を生成し、`0600` で保存)。
//! - **ノード個別トークン** (`~/.flexagent/node_token`): Node ⇔ Server の
//!   ペアリング認証。サーバーは `server.db.nodes.token_hash` に
//!   [`hash_token`] (SHA-256 hex) のみを保存し、平文は発行時に一度だけ表示する。

use std::path::Path;

use sha2::{Digest, Sha256};

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

/// トークンの SHA-256 ハッシュ (hex) を計算する (`nodes.token_hash` 保存用)。
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hash.push_str(&format!("{byte:02x}"));
    }
    hash
}

/// トークンをファイルから読み込む (存在しなければ生成して保存する)。
pub fn load_or_create_token(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => { /* 空ファイルは再生成する */ }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let token = generate_token();
    save_token(path, &token)?;
    Ok(token)
}

/// トークンをファイルへ保存する (可能な環境ではオーナーのみ読み書き可 `0600`)。
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
    // Windows ではユーザープロファイル配下 (`~/.flexagent`) の ACL に従う
    // (追加の ACL 操作は `fxg service install` で扱う)。
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
        // "abc" の SHA-256 (既知のテストベクタ)
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
