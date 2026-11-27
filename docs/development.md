# 開発ガイド (`development.md`)

コントリビュータ・開発者向けのビルド・テスト・フォーマット・多言語化ワークフローをまとめます。

---

## 1. ツールチェインと `mise` タスク

ツールバージョンは [`mise.toml`](../mise.toml) で固定しています（`rust`, `node`, `pnpm`, `sqlx-cli`, `watchexec`, `tombi`）。

| タスク                                 | 用途                                                                                                    |
| :------------------------------------- | :------------------------------------------------------------------------------------------------------ |
| `mise run dev`                         | Vite dev server (`:5173`) と隔離された dev ノード (`:7861`、変更時自動リビルド) を並列起動              |
| `mise run dev:home`                    | 隔離された開発用 `FXG_HOME` と DB・addr・ログのパスを表示                                               |
| `mise run dev:cli` (`fxg-dev`)         | デバッグバイナリを隔離された dev home に対して実行（例: `fxg-dev session list`、§5.3）                  |
| `mise run install:prod`                | UI をビルドしてリリースバイナリを `cargo install --locked`（運用用、§5.6）                              |
| `mise run fmt` / `fmt:check`           | Rust (`rustfmt`), TOML (`tombi`), Markdown/JSON/YAML/TS/Svelte (`oxfmt`) の一括整形・検査               |
| `mise run lint`                        | `lint:rust` (`clippy`), `lint:errors` (型安全・エラー握りつぶし検査), `lint:toml`, `lint:ui` (`oxlint`) |
| `mise run types`                       | `fxg-protocol` から `specta` で `ui/src/lib/generated/bindings.ts` を再生成                             |
| `mise run sqlx:prepare` / `sqlx:check` | `crates/fxg-db/migrations/` を適用し `.sqlx` オフラインキャッシュを更新・検証                           |
| `mise run check:ui`                    | `svelte-check` による UI 型・テンプレート検査                                                           |
| `mise run test`                        | Rust ワークスペーステスト + UI (`Vitest`) を実行                                                        |
| `mise run check`                       | コミット前の全検証（`fmt:check` + `lint` + `check:ui` + `test`）                                        |
| `mise run build` / `build:docker`      | `fxg` リリースバイナリまたは Docker イメージをビルド                                                    |

デバッグビルドは `Cargo.toml` の `[profile.dev]` でワークスペースの line tables だけを残し、
依存クレートの debuginfo を落としています（`fxg` は約 200 MB。以前は依存の DWARF で 577 MB
ありました）。プロファイルを変えた直後は古い artifact が `target/debug` に残るため、
ディスクを回収したいときは `cargo clean` の後に作り直してください（テスト・開発用の
`target/debug` が 10 GB を超えることがあります）。sccache / mbx などのビルドキャッシュを
使っている環境では `target` の実体がキャッシュ側にあることがあり、回収量はキャッシュの
GC 設定に依存します。

---

## 2. 型生成・DB スキーマ変更時の手順

- **`crates/fxg-protocol` の型を変更した場合**: `mise run types` を実行し、`ui/src/lib/generated/bindings.ts` の差分をコミットする（生成物は直接編集しない）。
- **`crates/fxg-db/migrations/` を追加・変更した場合**: `mise run sqlx:prepare` を実行し、`.sqlx/` の更新をコミットする。
- **`sqlx::query!` 系マクロを追加・変更した場合**: 同じく `mise run sqlx:prepare` で `.sqlx/` を更新する。CI は欠落のみをコンパイル時に検出するため、古いメタデータのまま整合しているケースはローカルで防ぐ。
- **`ErrorCode` を追加・変更した場合**: `ui/messages/en.json` の対応キーと [`docs/troubleshooting.md`](./troubleshooting.md) の `## <ERROR_CODE>` セクションを追加する（`fxg-protocol` の契約テストで検証される）。

---

## 3. 言語ポリシーと i18n (`paraglide-js`)

- **コードは英語**: Rust / TypeScript / Svelte のコメント、doc コメント、ログ、CLI 出力、エラーメッセージ、UI 文言はすべて英語で記述する（多バイト境界テスト等の例外には `// allow-japanese: <理由>` を付記）。
- **ドキュメントは日本語**: `docs/` 配下（ステート docs・`proposals/`・`changelog/`）は日本語で維持する。
- **UI 多言語化**: `paraglide-js` を前提とし、Rust 側の API エラーは `ErrorCode` を i18n キーとして扱い、`message` は英語フォールバックとして返す（サーバー側でロケール交渉は行わない）。

---

## 4. CI (GitHub Actions)

CI は領域ごとに 3 つのワークフローへ分割している。目的は「変更していない領域のジョブを走らせない」こと。

- `.github/workflows/ci-fmt.yml`: 整形チェック (rustfmt / tombi / oxfmt)。`docs/` の Markdown も整形対象なので常時実行する。
- `.github/workflows/ci-rust.yml`: `crates/**`、TOML マニフェスト、`.sqlx/**`、生成 TS 型などの変更時に clippy / 3 OS テスト / specta 型同期を実行する。
- `.github/workflows/ci-ui.yml`: `ui/**` の変更時に oxlint / svelte-check / Vitest / ビルドを実行する。

不変条件:

- **draft PR では全ジョブをスキップ**し、`ready_for_review` で実行する。`pull_request` の `types` に `ready_for_review` を含めること（既定の 3 種類だけでは ready にした時点で走らない）。
- **`.sqlx` の検査は専用ジョブを持たない**。オフラインキャッシュの欠落は `sqlx::query!` のコンパイル時に失敗するため、clippy / テストが同じ検査を兼ねる。クエリやマイグレーションを変更したらローカルで `mise run sqlx:prepare`、検証のみなら `sqlx:check` を実行して `.sqlx/` を更新する。
- CI では debuginfo を `line-tables-only` に落とし、`Swatinem/rust-cache` の保存は `test` ジョブだけに限定する（同一キーへの並列保存はレースになり、immutable なキャッシュが上書きできない）。
- 将来 branch protection で required check を設定する場合、パスフィルタでスキップされたワークフローは Pending のまま残る。常時実行される集約ジョブを required にすること。

---

## 5. 開発環境と運用環境の分離

fxg を日常使いのノード／ハブとして動かしながら、同じマシンで fxg 自体を開発するための
構成です。`mise` は開発側だけを隔離し、OS 標準ディレクトリは運用側に予約します。

### 5.1 なぜ分けるか

`<config>` / `<data>` / `<state>` / `<logs>` / `<cache>` 配下は 1 つのノード (または
ハブ) が所有する前提です（[`05-cli-and-config.md`](05-cli-and-config.md) §2.1）。
同じ場所で dev ノードを起動すると、次の競合が起きます。

- `node.db` の共有: dev のマイグレーションやテストセッションが運用中の履歴を書き換える。
  SQLite の WAL でも、2 プロセスが同じ DB を別々の真実として使う状態は壊れる。
- `node.addr` の共有: CLI はこのファイルで接続先を決めるため、起動のたびに取り合いになる。
- `auth_token` / `secrets/` / `worktrees/` / ログ / ACP Registry キャッシュの混在。
- 待受ポート (`7860` / `8080`) の取り合い。明示指定がないと ephemeral port に
  フォールバックし、どのインスタンスに繋がっているのか分からなくなる。

`FXG_HOME` はこの 5 つのルートをまとめて差し替える正規の切り替え点なので、
「ノードの同一性」は `FXG_HOME` だけで分かれます。

### 5.2 レイアウト

| 項目         | 運用 (prod)                                    | 開発 (dev)                                   |
| :----------- | :--------------------------------------------- | :------------------------------------------- |
| 起動         | `fxg service install node`（リリースバイナリ） | `mise run dev` / `mise run dev:node`（手動） |
| データ根     | OS 標準ディレクトリ                            | `$FXG_HOME` = `~/.local/share/flexagent-dev` |
| node 待受    | `127.0.0.1:7860`                               | `127.0.0.1:7861`                             |
| hub 待受     | `0.0.0.0:8080`                                 | `127.0.0.1:8081`（`mise run dev:hub`）       |
| ブラウザ     | `http://127.0.0.1:7860`                        | `http://localhost:5173`（Vite → `7861`）     |
| ハブ接続     | 本番ハブ                                       | dev ハブのみ（本番ハブに pair しない）       |
| サービス登録 | する                                           | しない                                       |

開発環境と運用環境はホスト名（`localhost` と `127.0.0.1`）が違うため、
Cookie (`fxg_session_<role>`)・`localStorage`・PWA の Service Worker は
運用側と混ざりません。Cookie はポートを区別しない点に注意
（同一ホストでは hub と node の Cookie 名がロール別に分かれている）。

### 5.3 使い方

開発用の `FXG_HOME`・待受ポート・Vite のプロキシ先 (`FXG_DEV_PROXY`) は
`mise.toml` の `dev:*` タスクの `env` にだけ設定します。リポジトリ全体の環境は
変えないため、インストール済みの `fxg` はリポジトリ内でもそのまま運用ノード
(`7860` / `8080`) に繋がります。

```sh
mise run dev        # Vite (5173) + dev ノード (7861)
mise run dev:node   # dev ノードだけ（Rust 変更で自動再起動）
mise run dev:hub    # dev ハブ (127.0.0.1:8081)
mise run dev:home   # dev home の実パスを確認

fxg session list      # インストール済みバイナリ → 運用ノード (7860)
fxg-dev session list  # デバッグビルド → dev ノード (7861)
```

- dev home はリポジトリの外に置く。`target/` の内側に置くと `cargo clean` や
  `git clean -xfd` で DB・セッションが消える。
- `fxg` は `~/.cargo/bin/fxg`（[§5.6](#56-運用バイナリのビルドとインストール) の
  リリースバイナリ）、`fxg-dev` は `mise run dev:cli -- ...` と同じデバッグビルドで、
  どちらもカレントディレクトリの `mise.toml` から解決されます。dev の CLI を
  `mise` の外から使う場合は `FXG_HOME=~/.local/share/flexagent-dev` を明示してください。

  ```sh
  FXG_HOME=~/.local/share/flexagent-dev cargo run -p fxg-cli -- node status
  ```

- dev home の削除はディレクトリごと消せばよい（`FXG_HOME` の外に状態はない）。

### 5.4 fxg で fxg を開発するときの注意

- **編集セッションを再起動するノードに載せない**: `mise run dev:node` は `crates/` の
  変更でノードを再起動する。そのノード上のセッションが fxg を編集すると、保存のたびに
  自分を載せているノードが落ちて自己再起動ループになる。編集は安定したインスタンス
  （運用ノードなど）に任せ、検証だけ dev インスタンスで行う。
- dev ノードでセルフホストする場合は、監視なしで起動し、ビルドで差し替わらない
  バイナリのコピーを使う（`cargo run` は再ビルド時に実行ファイルを置き換える）。
  ```sh
  cp target/debug/fxg ~/bin/fxg-pinned
  FXG_HOME=~/.local/share/flexagent-dev ~/bin/fxg-pinned node start --no-tray
  ```
- 同じチェックアウトで複数セッションが `cargo` を走らせるとビルドロックで待つ。
  `fxg-dev session new ... -w` の Worktree を使い、タスクごとに作業ツリーを分ける
  （dev 側の Worktree は dev home 配下に作られる）。
- **`fxg-dev service install` を実行しない**: unit / plist / タスクは
  実行パスだけを記録し、`FXG_HOME` を埋め込まない。dev バイナリで登録すると、運用側の
  自動起動が dev ビルドに差し替わる。サービス登録はリリースバイナリだけで行う。
- **secrets は `FXG_HOME` 配下**: エージェントの API キーや Git 認証は dev home では
  未設定になる。dev 用に最小限だけ登録し、運用側の値をそのままコピーしない。
- **エージェント自身の状態は `FXG_HOME` の外**: ACP の認証や OpenCode の
  `service.json`（`dirs::state_dir()`）は運用側と共有される。完全に分離したい場合だけ、
  dev 起動時だけ `XDG_STATE_HOME` / `HOME` を変える（ログインもやり直しになる）。
- 運用ハブに dev ノードを pair すると、テストセッションが運用側の `hub.db` に投影され
  UI に現れる。ハブ連携を試すときは `mise run dev:hub` の dev ハブを使う。

### 5.5 確認

```sh
mise run dev:home
fxg-dev node status
ss -ltn | grep -E ':(7860|7861)'
```

`node status` が `127.0.0.1:7861` を返し、運用ノードが `7860` のままなら分離できています。
dev のログは `$FXG_HOME/logs/node/node.log.YYYY-MM-DD` に出るため、運用側のログと混ざりません。

### 5.6 運用バイナリのビルドとインストール

運用側はリリースバイナリをパスの通った場所にコピーし、サービスはその絶対パスで登録します。
`target/debug` / `target/release` を `ExecStart` に直接書かないでください（`cargo clean` や
開発ビルドで消える・差し替わる）。

```sh
mise run install:prod                 # ui/build を作成 → cargo install --locked（~/.cargo/bin/fxg）
cd ~ && ~/.cargo/bin/fxg service install node
```

- `ui/build` は `rust-embed` でバイナリに埋め込まれる。UI をビルドせずにリリースビルドすると
  「UI not built」のプレースホルダが入るため、`install:prod` は `build:ui` を先に実行する。
- サービス登録はリポジトリの外で絶対パス指定で行う。リポジトリ内の `fxg` は
  インストール済みバイナリを指すが、デバッグビルドの `fxg-dev` で
  `fxg-dev service install` を実行すると dev バイナリ (`target/debug/fxg`) が登録されてしまう。
- 更新は `mise run install:prod` → `~/.cargo/bin/fxg node restart`（サービス登録済みなら
  systemd / launchd / Task Scheduler に委譲される）。`cargo install` はバイナリをコピーする
  ので、以降の dev ビルドで運用バイナリが置き換わることはない。
- リポジトリ内でも `fxg` はインストール済みバイナリを指す（`mise` は `FXG_HOME` などを
  リポジトリ全体には設定しない）。デバッグビルドを使うときだけ `fxg-dev` を使う。
