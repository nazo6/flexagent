//! テスト用ヘルパー (`cfg(test)` のみ)。
//!
//! 一時ディレクトリに Git リポジトリを構築するなど、複数モジュールの
//! テストで共有する処理をまとめる。

#![cfg(test)]

use std::path::Path;

use crate::git;

/// 一時ディレクトリに Git リポジトリを作成し、1コミット入れておく。
pub(crate) async fn init_test_repo(dir: &Path) {
    git::run_git(dir, &["init", "-q", "-b", "main"])
        .await
        .expect("git init");
    git::run_git(dir, &["config", "user.email", "test@example.com"])
        .await
        .expect("git config email");
    git::run_git(dir, &["config", "user.name", "fxg test"])
        .await
        .expect("git config name");
    std::fs::write(dir.join("README.md"), "# test\n").expect("write readme");
    git::run_git(dir, &["add", "-A"]).await.expect("git add");
    git::run_git(dir, &["commit", "-q", "-m", "initial"])
        .await
        .expect("git commit");
}

/// Git リポジトリを作成し、`origin` remote を設定する。
pub(crate) async fn init_test_repo_with_remote(dir: &Path, remote_url: &str) {
    init_test_repo(dir).await;
    git::run_git(dir, &["remote", "add", "origin", remote_url])
        .await
        .expect("git remote add");
}

/// テスト用の Shadow Git Index パスを作る (一時ディレクトリ配下)。
pub(crate) fn snapshot_index_path(dir: &Path, session_id: &str) -> std::path::PathBuf {
    dir.join(format!("{session_id}.index"))
}

/// テスト用のローカルIPCエンドポイントを返す。
///
/// - **Windows**: Named Pipe (`\\.\pipe\fxg-test-<pid>-<n>`)。テストは同一
///   プロセス内で並列実行されるため、連番で一意化する。
/// - **Unix**: `dir` 配下の一時ソケット (テストごとに個別の tempdir を使う)。
pub(crate) fn test_ipc_endpoint(dir: &Path) -> String {
    if cfg!(windows) {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(r"\\.\pipe\fxg-test-{}-{n}", std::process::id())
    } else {
        dir.join("daemon.sock").to_string_lossy().into_owned()
    }
}
