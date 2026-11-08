# 2026-11-08 ACP バイナリ直接展開機能のオミットと依存関係の軽量化

- **日付**: 2026-11-08
- **対象パッケージ**: `Cargo.toml`, `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト**: `mise run fmt` / `mise run types` / `cargo test --workspace` / `pnpm --dir ui run check`

## 背景

`fxg-acp` 内で ACP Registry 経由のエージェントバイナリ (`.tar.gz` / `.zip`) を直接ダウンロード・展開・配置するミニパッケージマネージャ機能 (`AcpRegistry::install`) を保持していたため、`zip` (およびその推移依存である `zopfli`, `zeroize`, `typed-path`, `bumpalo` 等)、`tar`、`flate2`、`sha2` のクレート依存が増加していた。

ACP エージェントはすでに `PATH` 上の既存バイナリ (`external`)、`npx` / `uvx` (`package`)、およびカスタム設定 (`custom`) で柔軟に起動可能であり、また有名クライアント（例: Antigravity / OpenCode 等）は今後プリセット設定や既知パス自動検出によってワンタッチで構成できる設計とするため、バイナリアーカイブの自動展開・管理機能をオミットして依存関係とコードベースを大幅にスリム化した。

## 変更内容

### 1. 依存クレートの削除

- ルート `Cargo.toml` および `crates/fxg-acp/Cargo.toml` から `zip`, `tar`, `flate2` を削除（※ `fxg-cli` のトレイアイコン用途で `png` が内部利用する推移的 `flate2`、および `fxg-server` のトークンハッシュ用の `sha2` は維持）。
- `fxg-acp` から `sha2` を削除。

### 2. `fxg-acp` のリファクタリング

- `extract_archive_bytes`, `sha256_hex`, `agents_dir`, `agent_version_dir`, `installed_versions`, `remove`, `install` を削除。
- `LocalSource::Managed` を削除し、起動経路を `External` (PATH), `Npx`, `Uvx` に整理。
- `launch_spec` の解決順を **カスタムエージェント → PATH 上の既存インストール → npx → uvx** に更新。

### 3. プロトコル (`fxg-protocol`) と API の整理

- `AgentSource`: `Managed` を削除 (`External`, `Package`, `Custom`)。
- `AgentSummary`: `installable`, `managed_versions` を削除。
- `AgentAction`: `Install`, `Update`, `Remove` を削除し、レジストリインデックス更新用の `RefreshCatalog` を追加。
- REST API: `POST /api/v1/nodes/:node_id/agents/:agent_id/install` および `DELETE /api/v1/nodes/:node_id/agents/:agent_id` を削除。`POST /api/v1/nodes/:node_id/agents/refresh` (`RefreshCatalog`) を追加。
- `specta` による TypeScript 型定義 (`ui/src/lib/generated/bindings.ts`) を同期更新。

### 4. CLI (`fxg-cli`)

- `fxg agents list`: 表示から `managed` を除外。
- `fxg agents install <id>`: バイナリ自動DLを廃止。package 配布がある場合は有効化し、binary 配布のみの場合は手動インストールまたはカスタム設定を促す案内を表示。
- `fxg agents update`: レジストリインデックス (`registry.json`) のキャッシュ強制再取得・更新に特化。
- `fxg agents remove <id>`: カスタムエージェントの削除または package の無効化を実行。

### 5. Web UI (`ui`)

- 設定 > エージェント管理 (`AgentManager.svelte`):
  - 「fxg でインストール」ボタンおよび導入バージョンバッジを削除。
  - カタログダイアログ: `package_available` の場合は「npx/uvx を有効化」、それ以外（バイナリ配布のみ）の場合は「手動導入が必要 (PATH / カスタム追加)」と表示。
  - カタログ更新ボタンを `refreshCatalog()` に対応。
- `ui/src/lib/agents.ts`: `managed` 関連のラベル・説明ヘルパーを整理。
