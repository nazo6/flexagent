# Web ターミナルの workspace スコープ化と寿命分離（Phase 1）

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-protocol`, `fxg-pty`, `fxg-hub`, `fxg-node`, `fxg-db`, `ui`, `docs`
- **対象スクリプト / 設定**: `cargo test -p fxg-protocol -p fxg-pty` /
  `cargo test -p fxg-node --lib` / `cargo clippy --workspace --all-targets` /
  `pnpm --dir ui run check` / `pnpm --dir ui run lint` / `mise run fmt`
- **関連ドキュメント**:
  - [`docs/proposals/2026-11-14-terminal-workspace-scope.md`](../proposals/2026-11-14-terminal-workspace-scope.md)
  - [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md) §3.3
  - [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §4
  - [`docs/01-architecture-and-sync.md`](../../01-architecture-and-sync.md) §7.4

## 背景

Web ターミナルは「パネルを開いている間だけ存在する使い捨て PTY」だった。

- socket を閉じると spawn した PTY を kill していた。
- 出力は broadcast のみで、購読していない間のデータは捨てていた。
- セッションを切り替えるとシェルが消え、同じ worktree の複数セッションでは
  シェルが増殖していた。
- 生きているターミナルを列挙・復元する手段がなかった。

## 変更

- **スコープ**: ターミナルは `(node_id, canonical local_path)` の workspace 単位に
  なった。同じ workspace の `open` は既存のシェルを再利用する（冪等）。
  セッションは所有権ではなく **origin** としてのみ保持する。
- **`fxg-pty`**: workspace レジストリ、`open` / `attach` / `detach`、参照カウント、
  無操作 TTL（既定 30 分）の回収スレッド、リングバッファ（256 KiB）と
  チャンクシーケンス番号を追加した。TTL は
  `PtySessionManager::with_idle_timeout` で差し替えられる。
- **`fxg-protocol`**: `PtyClientMessage::Open` / `PtyServerMessage::Opened` に
  置き換え、`output.seq`、`TerminalSummary` / `TerminalsResponse`、
  `NodeRpcRequest::ListTerminals`、`PtyOpened` / `PtyOpenFailed` /
  `PtyAttach` / `PtyDetach` を追加した。メッセージ形が変わるため
  `PROTOCOL_VERSION` を 3 に上げた（ノードとハブは同時更新が必要）。
- **node**: `open` で cwd を検証する。許可されるのは、そのノードの
  `project_node_bindings` / `project_path_links`、または既存セッションの
  `local_path` に**完全一致**するディレクトリだけ（サブディレクトリは不可）。
  `allow_remote_pty` はハブ経由の open に引き続き適用する。
- **hub**: workspace 単位で open を直列化し、ノードが返す PTY ID と保持出力を
  受け取って replay ログを初期化する。socket 切断では kill せず detach を送る。
  ノード接続が切れた場合はルートを破棄し、ノード側も hub 由来の attach を落とす。
- **UI**: `TerminalView` は `open` を送り、workspace キーの変化でのみ再接続する。
  dispose は detach のみ。attach 後にサイズを送り直し、再接続前には端末バッファを
  clear する（replay の二重描画を防ぐ）。
- **監査**: `pty_spawn` の details に `path` / `attached` を追加し、`pty_kill` を
  追加した。`pty_exit` はノイズになるため監査対象にせず、ログに留める。
- **API**: `GET /api/v1/terminals?node_id=...` を追加した。ノードのメモリ上の
  レジストリが正で、ハブは RPC で中継する。

## 不変条件

1. 1 workspace につき既定 1 ターミナル。同一 workspace への `open` は冪等。
2. 1 つの `TerminalView` は最大 1 つの PTY にしか接続しない。
3. socket 切断だけでは PTY を kill しない。kill は明示操作・TTL・プロセス exit・
   キルスイッチのみ。
4. ターミナルはノードローカル（DB に永続化しない）。一覧は再起動で空になる。
5. workspace として開けるのは登録済みパスに完全一致するディレクトリだけ。
6. `session_id` は origin であり、セッションの寿命はターミナルの寿命を決めない。

## テスト

- `fxg-pty`: workspace 再利用、`new` による置換、TTL 回収、replay の内容と
  シーケンス番号、既存の PTY テスト。
- `fxg-protocol`: 出力ログの seq / 切り詰め、`Open` / `Opened` / `output.seq`、
  PTY relay envelope の round-trip。
- `fxg-node`: パス検証（未登録・サブディレクトリの拒否）、workspace 再利用と
  attach カウント、PTY WS で切断 → 再接続時に同じ PTY ID・保持出力の replay。
- `fxg-node/tests/sync_e2e.rs`: ハブ経由の open → 出力中継 → 一覧 API →
  切断 → 再接続で同じ PTY ID と replay、までを実ハブ + 実ノードで検証。

## 既知の制約

- TTL（30 分）とリングバッファ上限（256 KiB）は定数。設定化は backlog。
- 複数クライアントの resize 通知・共有中バッジ・read-only は未実装（Phase 3）。
- ターミナル一覧 API は UI からまだ使っていない（リロード時の自動復元は Phase 2）。
- worktree 削除とターミナルの連動 kill は未実装（Phase 2）。
- ハブが再起動するとルーティングが消えるため、実行中のシェルはノード側に残り、
  再度 `open` されるまで一覧に出ない。TTL で回収される。
