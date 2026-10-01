//! `fxg bootstrap-workspace`: 一時VM内の Zero-Touch 初期化。
//!
//! 設計: `docs/01-architecture-and-sync.md` §6.3、`docs/05-cli-and-pwa-ui.md` §2.3。
//!
//! 中央サーバーが注入した短命トークン (`FXG_GIT_TOKEN`) を使ってリポジトリを
//! `git clone` し、リポジトリの構成ファイルからツールチェインを自動検出して
//! `mise` / `uv` で導入する。**この関数の出力はすべて `stderr` に限定する**
//! (`stdout` は後続の `fxg daemon --stdio` の JSON Lines 専用として保護する)。
//!
//! 終了時に `$FXG_HOME/bootstrap.env` へ環境変数 (PATH 追加 + `.fxg.toml` の
//! `[bootstrap.env]`) を書き出す。生成されるプロビジョナースクリプトが
//! `exec fxg daemon --stdio` の直前にこれを `source` することで、導入した
//! ツールがエージェントプロセスへ引き継がれる。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use fxg_protocol::config::{BOOTSTRAP_ENV_FILE_NAME, DEFAULT_GIT_USERNAME, ProjectConfig};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::error::NodeError;
use crate::git;

/// `bootstrap-workspace` の実行オプション。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapOptions {
    /// クローン元 Git URL (必須)
    pub repo: String,
    /// 対象ブランチ (省略時はリモートの既定ブランチ)
    pub branch: Option<String>,
    /// クローン先ディレクトリ
    pub dir: PathBuf,
    /// データディレクトリ (`~/.flexagent`)
    pub fxg_home: PathBuf,
    /// ツールチェイン (`mise` / `uv`) の導入とツールインストールを行うか
    /// (テストでは `false` にしてネットワークアクセスを避ける)
    pub install_toolchain: bool,
    /// 検出ツールに追加するツール (テスト・デバッグ用)
    pub extra_tools: Vec<String>,
}

/// 自動導入する標準必須 CLI (設計: docs/01 §6.3)。
pub const STANDARD_CLI_TOOLS: &[&str] = &["ripgrep", "fd", "jq", "gh"];

/// `bootstrap-workspace` の実行結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapOutcome {
    /// クローン済みワークスペース
    pub dir: PathBuf,
    /// 導入した (導入を試みた) ツール一覧
    pub tools: Vec<String>,
    /// 生成した環境変数ファイル
    pub env_file: PathBuf,
}

/// ブートストラップの進捗ログ (すべて `stderr` へ)。
fn log(message: impl std::fmt::Display) {
    eprintln!("[fxg-bootstrap] {message}");
}

/// 一時VM内のワークスペースを初期化する。
pub async fn bootstrap_workspace(options: BootstrapOptions) -> Result<BootstrapOutcome, NodeError> {
    std::fs::create_dir_all(&options.fxg_home).map_err(|source| NodeError::Io {
        path: options.fxg_home.clone(),
        source,
    })?;

    if !crate::bundle::git_available().await {
        return Err(NodeError::GitUnavailable(
            "git is required for bootstrap-workspace".to_owned(),
        ));
    }

    clone_repository(&options).await?;
    let config = ProjectConfig::load_from_dir(&options.dir)
        .map_err(|err| NodeError::Server(err.to_string()))?
        .unwrap_or_default();

    // ツール検出 (構成ファイル + `.fxg.toml` の明示指定)
    let mut tools = detect_tools(&options.dir, &config);
    for tool in &options.extra_tools {
        if !tools.contains(tool) {
            tools.push(tool.clone());
        }
    }
    log(format!("detected tools: {}", tools.join(", ")));

    let bin_dir = default_bin_dir();
    if options.install_toolchain {
        std::fs::create_dir_all(&bin_dir).map_err(|source| NodeError::Io {
            path: bin_dir.clone(),
            source,
        })?;
        install_mise(&bin_dir).await;
        if wants_python(&options.dir) {
            install_uv(&bin_dir).await;
        }
        install_tools(&bin_dir, &tools).await;
        if wants_python(&options.dir) {
            prepare_python_environment(&options.dir).await;
        }
    } else {
        log("toolchain installation skipped (install_toolchain = false)");
    }

    // `git` は最優先で使うため、見つからない場合のみ導入を試みる
    if options.install_toolchain && !crate::bundle::git_available().await {
        log("git is missing after setup; setup commands may fail");
    }

    // `.fxg.toml [bootstrap] setup` を実行する (出力は stderr へ)
    for command in &config.bootstrap.setup {
        log(format!("setup: {command}"));
        let status = run_streaming(&options.dir, command, &[]).await?;
        if !status {
            return Err(NodeError::Server(format!(
                "bootstrap setup command failed: {command}"
            )));
        }
    }

    let env_file = write_env_file(&options, &config, &bin_dir)?;
    log(format!("environment file written: {}", env_file.display()));
    log("bootstrap completed");

    Ok(BootstrapOutcome {
        dir: options.dir,
        tools,
        env_file,
    })
}

/// リポジトリをクローンする (既にクローン済みなら再利用)。
async fn clone_repository(options: &BootstrapOptions) -> Result<(), NodeError> {
    // 存在しないディレクトリでは git を実行できない
    if options.dir.is_dir() && git::toplevel(&options.dir).await?.is_some() {
        log(format!(
            "workspace already cloned: {}",
            options.dir.display()
        ));
        return Ok(());
    }

    if options.dir.exists() {
        let is_empty = std::fs::read_dir(&options.dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true);
        if !is_empty {
            return Err(NodeError::Server(format!(
                "target directory is not empty and not a git repository: {}",
                options.dir.display()
            )));
        }
    }
    if let Some(parent) = options.dir.parent() {
        std::fs::create_dir_all(parent).map_err(|source| NodeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // 短命トークン (FXG_GIT_TOKEN) を GIT_ASKPASS 経由で渡す。
    // URL へのトークン埋め込みは行わず、環境変数はオンメモリのみで扱う。
    let askpass = write_bootstrap_askpass(&options.fxg_home)?;
    let mut git_env: Vec<(String, String)> = vec![
        (
            "GIT_ASKPASS".to_owned(),
            askpass.to_string_lossy().into_owned(),
        ),
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
    ];
    if let Ok(username) = std::env::var("FXG_GIT_USERNAME") {
        git_env.push(("FXG_GIT_USERNAME".to_owned(), username));
    }
    if let Ok(token) = std::env::var("FXG_GIT_TOKEN") {
        git_env.push(("FXG_GIT_TOKEN".to_owned(), token));
    }

    log(format!(
        "cloning {} into {}",
        options.repo,
        options.dir.display()
    ));
    let mut args: Vec<&str> = vec!["clone", "--depth", "50"];
    if let Some(branch) = options.branch.as_deref().filter(|b| !b.trim().is_empty()) {
        args.push("--branch");
        args.push(branch);
    }
    args.push(&options.repo);
    let dir_arg = options.dir.to_string_lossy().into_owned();
    args.push(&dir_arg);

    let cwd = options
        .dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let env_refs: Vec<(&str, &str)> = git_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let output = git::capture(&cwd, &args, &env_refs).await?;
    if !output.success {
        return Err(NodeError::Git {
            cwd,
            args: args.join(" "),
            message: output.stderr,
        });
    }
    Ok(())
}

/// リポジトリの構成ファイルから導入するツールを検出する。
pub fn detect_tools(dir: &Path, config: &ProjectConfig) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    let mut push = |tool: &str| {
        if !tools.iter().any(|existing| existing == tool) {
            tools.push(tool.to_owned());
        }
    };

    if dir.join("Cargo.toml").exists() || dir.join("rust-toolchain.toml").exists() {
        push("rust");
    }
    if dir.join("package.json").exists() || dir.join(".node-version").exists() {
        push("node");
        if dir.join("pnpm-lock.yaml").exists() {
            push("pnpm");
        } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
            push("bun");
        } else if dir.join("yarn.lock").exists() {
            push("yarn");
        }
    }
    if dir.join("pyproject.toml").exists() || dir.join(".python-version").exists() {
        push("python");
    }
    // `mise.toml` / `.tool-versions` に記載された全ツール
    for tool in read_mise_tools(dir) {
        push(&tool);
    }
    // `.fxg.toml [bootstrap] tools` の明示指定
    for tool in &config.bootstrap.tools {
        push(tool);
    }
    tools
}

/// `mise.toml` の `[tools]` / `.tool-versions` からツール名を読み取る。
fn read_mise_tools(dir: &Path) -> Vec<String> {
    let mut tools = Vec::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("mise.toml"))
        && let Ok(value) = text.parse::<toml::Value>()
        && let Some(table) = value.get("tools").and_then(|tools| tools.as_table())
    {
        tools.extend(table.keys().cloned());
    }
    if let Ok(text) = std::fs::read_to_string(dir.join(".tool-versions")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((tool, _version)) = line.split_once(char::is_whitespace) {
                tools.push(tool.to_owned());
            }
        }
    }
    tools
}

/// Python プロジェクトか (uv による仮想環境構築を行うか)。
fn wants_python(dir: &Path) -> bool {
    dir.join("pyproject.toml").exists() || dir.join(".python-version").exists()
}

/// `~/.local/bin` (ツール配置先)。
fn default_bin_dir() -> PathBuf {
    if let Ok(fxg_home) = std::env::var("FXG_HOME") {
        // テストではデータディレクトリ配下へ隔離する
        return PathBuf::from(fxg_home).join("bin");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("bin")
}

/// mise の shim ディレクトリ。
fn mise_shims_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("mise")
        .join("shims")
}

/// `mise` を単一バイナリで導入する (root 権限・apt 不要)。
async fn install_mise(bin_dir: &Path) {
    let mise = bin_dir.join(if cfg!(windows) { "mise.exe" } else { "mise" });
    if mise.exists() {
        log("mise is already installed");
        return;
    }
    log("installing mise ...");
    let installer = format!(
        "curl -fsSL https://mise.run | MISE_INSTALL_PATH='{}' sh",
        mise.display()
    );
    match run_shell(bin_dir, &installer).await {
        Ok(true) => log("mise installed"),
        Ok(false) => log("failed to install mise (continuing without tool installs)"),
        Err(err) => log(format!("failed to install mise: {err}")),
    }
}

/// `uv` をプリビルドバイナリで導入する。
async fn install_uv(bin_dir: &Path) {
    let uv = bin_dir.join(if cfg!(windows) { "uv.exe" } else { "uv" });
    if uv.exists() {
        log("uv is already installed");
        return;
    }
    if cfg!(windows) {
        log("uv installation on windows is not automated; install uv manually");
        return;
    }
    let Some(target) = uv_target() else {
        log("unsupported platform for uv; skipping");
        return;
    };
    let url =
        format!("https://github.com/astral-sh/uv/releases/latest/download/uv-{target}.tar.gz");
    let archive = bin_dir.join("uv-download.tar.gz");
    log(format!("installing uv from {url} ..."));
    if !download_file(&url, &archive).await {
        log("failed to download uv");
        return;
    }
    let script = format!(
        "tar -xzf '{}' -C '{}' && find '{}' -name uv -type f -exec cp {{}} '{}' \\; && rm -f '{}'",
        archive.display(),
        bin_dir.display(),
        bin_dir.display(),
        uv.display(),
        archive.display()
    );
    match run_shell(bin_dir, &script).await {
        Ok(true) => log("uv installed"),
        Ok(false) => log("failed to extract uv archive"),
        Err(err) => log(format!("failed to extract uv: {err}")),
    }
}

/// uv の配布ターゲット名 (OS / アーキテクチャ)。
fn uv_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

/// `curl` (なければ `wget`) でファイルをダウンロードする。
async fn download_file(url: &str, dest: &Path) -> bool {
    let dest_arg = dest.to_string_lossy().into_owned();
    let curl = format!("curl -fsSL '{url}' -o '{dest_arg}'");
    if matches!(run_shell(Path::new("."), &curl).await, Ok(true)) {
        return true;
    }
    let wget = format!("wget -q '{url}' -O '{dest_arg}'");
    matches!(run_shell(Path::new("."), &wget).await, Ok(true))
}

/// `mise install --yes <tools>` でツールを導入する (失敗は警告のみ)。
async fn install_tools(bin_dir: &Path, tools: &[String]) {
    let mut all: Vec<String> = tools
        .iter()
        .filter(|tool| tool.as_str() != "python")
        .cloned()
        .collect();
    for tool in STANDARD_CLI_TOOLS {
        if !all.iter().any(|existing| existing == tool) {
            all.push((*tool).to_owned());
        }
    }
    if all.is_empty() {
        return;
    }
    let mise = bin_dir.join(if cfg!(windows) { "mise.exe" } else { "mise" });
    if !mise.exists() {
        log("mise is unavailable; skipping tool installs");
        return;
    }
    let shims = mise_shims_dir();
    let path_env = format!("PATH={}:{}:$PATH", bin_dir.display(), shims.display());
    log(format!("installing tools via mise: {}", all.join(" ")));
    let script = format!(
        "MISE_YES=1 {} '{}' install --yes {}",
        path_env,
        mise.display(),
        all.join(" ")
    );
    match run_shell(bin_dir, &script).await {
        Ok(true) => log("tools installed"),
        Ok(false) => log("some tools failed to install (continuing)"),
        Err(err) => log(format!("tool installation failed: {err}")),
    }
}

/// `uv venv` / `uv sync` による Python 環境の構築 (ベストエフォート)。
async fn prepare_python_environment(dir: &Path) {
    let uv = default_bin_dir().join(if cfg!(windows) { "uv.exe" } else { "uv" });
    if !uv.exists() {
        log("uv is unavailable; skipping python environment setup");
        return;
    }
    log("preparing python environment with uv ...");
    let script = format!("'{}' venv", uv.display());
    match run_streaming(dir, &script, &[]).await {
        Ok(true) => log("python virtual environment ready (.venv)"),
        Ok(false) => log("uv venv failed (continuing)"),
        Err(err) => log(format!("uv venv failed: {err}")),
    }
}

/// `$FXG_HOME/bootstrap.env` を書き出す。
///
/// 生成形式は POSIX `sh` で `source` できる `export KEY='value'` 行。
/// `PATH` には導入ツール (`~/.local/bin` / mise shims / uv) を前置する。
fn write_env_file(
    options: &BootstrapOptions,
    config: &ProjectConfig,
    bin_dir: &Path,
) -> Result<PathBuf, NodeError> {
    let mut lines = vec![
        "# Generated by `fxg bootstrap-workspace`. Do not edit.".to_owned(),
        format!(
            "export PATH={}",
            sh_quote(&format!(
                "{}:{}:$PATH",
                bin_dir.display(),
                mise_shims_dir().display()
            ))
        ),
        format!(
            "export FXG_WORKSPACE={}",
            sh_quote(&options.dir.to_string_lossy())
        ),
    ];
    for (key, value) in &config.bootstrap.env {
        lines.push(format!("export {}={}", key, sh_quote(value)));
    }
    let path = options.fxg_home.join(BOOTSTRAP_ENV_FILE_NAME);
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).map_err(|source| NodeError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// POSIX シェル用にシングルクオートで安全に包む。
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// bootstrap 中の Git 認証用 `GIT_ASKPASS` ヘルパー (短命トークン) を生成する。
///
/// `bootstrap-workspace` はデーモン起動前のため、ローカルIPC を経由せず
/// 環境変数 `FXG_GIT_TOKEN` を直接読み出す最軽量なヘルパーを使う
/// (トークンは環境変数のみで、ディスクには保存しない)。
pub fn write_bootstrap_askpass(fxg_home: &Path) -> Result<PathBuf, NodeError> {
    std::fs::create_dir_all(fxg_home).map_err(|source| NodeError::Io {
        path: fxg_home.to_path_buf(),
        source,
    })?;
    let path = if cfg!(windows) {
        fxg_home.join("git-askpass-bootstrap.cmd")
    } else {
        fxg_home.join("git-askpass-bootstrap.sh")
    };
    let content = if cfg!(windows) {
        "@echo off\r\n\
         setlocal\r\n\
         set \"p=%~1\"\r\n\
         if not \"x%p:Username=%\"==\"x%p%\" (\r\n\
         \x20 echo %FXG_GIT_USERNAME%\r\n\
         ) else (\r\n\
         \x20 echo %FXG_GIT_TOKEN%\r\n\
         )\r\n"
            .to_owned()
    } else {
        format!(
            "#!/bin/sh\n\
             # fxg bootstrap GIT_ASKPASS helper (short-lived token). Auto-generated.\n\
             case \"$1\" in\n\
             \x20 *sername*) echo \"${{FXG_GIT_USERNAME:-{DEFAULT_GIT_USERNAME}}}\" ;;\n\
             \x20 *) echo \"$FXG_GIT_TOKEN\" ;;\n\
             esac\n"
        )
    };
    std::fs::write(&path, content).map_err(|source| NodeError::Io {
        path: path.clone(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o755);
            let _ = std::fs::set_permissions(&path, permissions);
        }
    }
    Ok(path)
}

/// コマンドをシェル経由で実行し、stdout / stderr を **stderr へ** ストリームする。
async fn run_streaming(cwd: &Path, command: &str, env: &[(&str, &str)]) -> Result<bool, NodeError> {
    let mut process = shell_command(command);
    process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        process.env(key, value);
    }
    let mut child = process.spawn().map_err(NodeError::Plain)?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("{line}");
            }
        }
    });
    let stderr_task = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("{line}");
            }
        }
    });
    let status = child.wait().await.map_err(NodeError::Plain)?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    Ok(status.success())
}

/// シェル (Windows は `cmd /C`、それ以外は `sh -c`) のコマンドを組み立てる。
fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    } else {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        process
    }
}

/// シェルコマンドを実行する (結果のみ、出力は stderr へ)。
async fn run_shell(cwd: &Path, command: &str) -> Result<bool, NodeError> {
    run_streaming(cwd, command, &[]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_toolchains_from_repository_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").expect("write");
        std::fs::write(dir.path().join("package.json"), "{}").expect("write");
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").expect("write");
        std::fs::write(
            dir.path().join("mise.toml"),
            "[tools]\nrust = \"stable\"\nnode = \"22\"\n",
        )
        .expect("write");

        let tools = detect_tools(dir.path(), &ProjectConfig::default());
        assert!(tools.contains(&"rust".to_owned()));
        assert!(tools.contains(&"node".to_owned()));
        assert!(tools.contains(&"pnpm".to_owned()));
        // 重複しない
        assert_eq!(tools.iter().filter(|tool| *tool == "rust").count(), 1);
    }

    #[test]
    fn detects_python_and_fxg_bootstrap_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "[project]\n").expect("write");
        let mut config = ProjectConfig::default();
        config.bootstrap.tools = vec!["rust@stable".to_owned()];

        let tools = detect_tools(dir.path(), &config);
        assert!(tools.contains(&"python".to_owned()));
        assert!(tools.contains(&"rust@stable".to_owned()));
        assert!(wants_python(dir.path()));
    }

    #[test]
    fn reads_tool_versions_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".tool-versions"),
            "node 22.1.0\n# comment\ngolang 1.22\n",
        )
        .expect("write");
        let tools = detect_tools(dir.path(), &ProjectConfig::default());
        assert!(tools.contains(&"node".to_owned()));
        assert!(tools.contains(&"golang".to_owned()));
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
    }

    #[tokio::test]
    async fn bootstrap_clones_local_repository_and_writes_env() {
        let repo_dir = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("tempdir");
        let workspace = home.path().join("workspace");

        // ローカル Git リポジトリ (file:// URL でクローン)
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "fxg test"],
        ] {
            git::run_git(repo_dir.path(), &args).await.expect("git");
        }
        std::fs::write(
            repo_dir.path().join("pyproject.toml"),
            "[project]\nname = \"demo\"\n",
        )
        .expect("write");
        git::run_git(repo_dir.path(), &["add", "-A"])
            .await
            .expect("add");
        git::run_git(repo_dir.path(), &["commit", "-q", "-m", "init"])
            .await
            .expect("commit");

        let repo_url = repo_dir.path().display().to_string();
        let outcome = bootstrap_workspace(BootstrapOptions {
            repo: repo_url,
            branch: Some("main".to_owned()),
            dir: workspace.clone(),
            fxg_home: home.path().join(".flexagent"),
            install_toolchain: false,
            extra_tools: Vec::new(),
        })
        .await
        .expect("bootstrap");

        assert!(workspace.join("pyproject.toml").exists());
        assert!(outcome.tools.contains(&"python".to_owned()));
        let env = std::fs::read_to_string(&outcome.env_file).expect("env file");
        assert!(env.contains("export FXG_WORKSPACE="));
        assert!(env.contains("export PATH="));

        // 再実行は冪等 (既存クローンを再利用)
        let again = bootstrap_workspace(BootstrapOptions {
            repo: repo_dir.path().display().to_string(),
            branch: Some("main".to_owned()),
            dir: workspace.clone(),
            fxg_home: home.path().join(".flexagent"),
            install_toolchain: false,
            extra_tools: Vec::new(),
        })
        .await
        .expect("bootstrap again");
        assert_eq!(again.dir, workspace);
    }

    #[test]
    fn bootstrap_askpass_script_is_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_bootstrap_askpass(dir.path()).expect("write");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("FXG_GIT_TOKEN"));
    }
}
