# `fxg daemon --tray` タスクトレイ常駐の実装

- **日付**: 2026-10-01
- **対象パッケージ**: `fxg-protocol`, `fxg-node`, `fxg-cli`
- **対象スクリプト / 設定**: `crates/fxg-cli/Cargo.toml` (target 別依存),
  `docs/04-agent-drivers-and-windows.md` §5.5,
  `docs/05-cli-and-pwa-ui.md` §1.1 / §1.2 / §2

## 概要

`fxg daemon --tray` でデーモンをタスクトレイに常駐させる。メニューから
ステータス表示（中央サーバー接続・未同期 Outbox・稼働中セッション）、
自動起動の登録/解除（`fxg service` と同一実装）、Web UI の起動、graceful
shutdown を操作できる。既定は無効（opt-in）で、`[node] tray = true` により
常時有効化、`--no-tray` で一時的に無効化する。

## 実装

| ファイル                             | 内容                                                                          |
| :----------------------------------- | :---------------------------------------------------------------------------- |
| `crates/fxg-protocol/src/config.rs`  | `[node] tray: Option<bool>` を追加                                            |
| `crates/fxg-node/src/daemon/mod.rs`  | `ShutdownHandle`（`NodeDaemon::shutdown_handle()`）を追加                     |
| `crates/fxg-cli/src/tray/mod.rs`     | メニュー/ステータス整形/アイコン生成/アクション処理（全 OS 共通）             |
| `crates/fxg-cli/src/tray/windows.rs` | 専用スレッド + Win32 メッセージループ（`GetMessageW` / `PostThreadMessageW`） |
| `crates/fxg-cli/src/tray/linux.rs`   | 専用スレッド + チャネル駆動（`ksni` = 純 Rust D-Bus。イベントループ不要）     |
| `crates/fxg-cli/src/tray/macos.rs`   | メインスレッド tao ループ + デーモンをワーカースレッドへ退避                  |
| `crates/fxg-cli/src/commands.rs`     | `--tray` / `--no-tray`、`NodeDaemon::start` → トレイ → `wait()` へ再構成      |
| `crates/fxg-cli/src/service.rs`      | `run_sync`（同期コア）と `is_installed`（登録状態判定）を公開                 |
| `crates/fxg-cli/src/main.rs`         | macOS のみ `tray::run_daemon_if_requested` への早期分岐                       |

- 依存: `tray-icon` 0.26（Windows/Linux/macOS、Linux は `default-features =
  false, features = ["ksni"]`）、`windows-sys` 0.61、`tao` 0.37（macOS）、`png`
  0.18（同梱アイコンのデコード）。`fxg-cli` の **target 別依存** として追加。
- アイコンは `ui/static/icon-192.png` を `include_bytes!` で埋め込み、32×32 へ
  ブロック平均縮小 + 右下に接続状態ドット（緑/グレー）を合成する。
- トレイ生成に失敗する環境（SSH・D-Bus なし・デスクトップセッションなし）では
  警告のみでデーモンは稼働を継続する。`--stdio` / `--ephemeral` では常に無効。

## 設計判断

- **イベントループの所在は OS 別**: `tray-icon` はループを自前で持たず、Windows
  は「トレイ作成スレッド」、macOS は「メインスレッド」、Linux(ksni) は不要という
  制約がある。共通ロジック（メニュー・状態・アイコン・アクション処理）を
  `tray/mod.rs` に集約し、ループ制御のみを OS 別モジュールに閉じ込めた。
- **macOS のみ実行形態を変更**: `NSStatusItem` の制約によりメインスレッドを tao
  に明け渡す必要があるため、`main.rs` で `fxg daemon --tray`
  のときだけ早期分岐し、
  デーモン本体（tokio ランタイム）をワーカースレッドで起動する。Windows / Linux
  は
  既存の `#[tokio::main]` 構成を変更しない（回帰リスクを最小化）。
- **メニュー操作の単一スレッド化**: `muda` のメニュー項目は `!Send` のため、
  メニューの生成・更新はトレイを作成したスレッドのみで行い、デーモン側とは
  tokio チャネル（アクション）/ チャネル +
  起床メッセージ（状態更新）で連携する。
- **自動起動トグルは既存実装を再利用**: `service::run_sync(Install/Uninstall)`
  を
  `spawn_blocking` 経由で呼び、実行後に `service::is_installed`
  で実状態を再確認して
  チェック表示を同期する（タスクスケジューラ → スタートアップフォルダの
  フォールバック結果も正しく反映される）。
- **状態の取得元はプロセス内**: 3 秒間隔で `DaemonState`（`node.db` /
  `sync` ワーカーのフラグ）から直接収集する（ローカル IPC / HTTP
  は経由しない）。

## 検証

- **ユニットテスト** (`tray::tests`, 3 OS 共通): フラグ/設定の優先順位
  (`--no-tray` > `--tray` > `[node] tray`)、ステータス文言整形、ツールチップ、
  アイコン生成（192px → 32px 縮小 + ドット合成、接続状態で画像が変わる）。
- **Windows 実機スモーク** (2026-10-01): 一時 `FXG_HOME` で
  (1) `--tray` → `tray icon started` ログ + 5 秒間警告なし、
  (2) `[node] tray = true`（フラグなし）→ トレイ生成、
  (3) `--tray --no-tray` → トレイ生成なし、を確認。
- **クロスコンパイル確認**: `fxg-*` は sqlite (C) を抱えるため Linux / macOS
  への
  クロスチェックができない。同期間、`target/tmp/tray-check`（コミット対象外）に
  tray モジュールのコピー + 最小スタブを置き、`x86_64-unknown-linux-gnu` /
  `aarch64-apple-darwin` 向け `cargo check` を実施して API 整合を確認した
  （この検証で tao 0.37 の `EventLoopBuilder::build()` が `Result` ではなく
  `EventLoop` を返すことを検出・修正済み）。
- **未確認**: トレイメニューのクリック操作（自動起動トグル・Web UI
  起動・終了）と
  Linux / macOS 実機での表示は手動確認が必要（CI にデスクトップセッションが
  ないため自動テスト対象外）。
