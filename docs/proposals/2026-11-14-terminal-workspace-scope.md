# 2026-11-14 ターミナルの workspace スコープ化と寿命分離

- **日付**: 2026-11-14
- **ステータス**: `Partially Implemented`
- **対象パッケージ**: `fxg-protocol`, `fxg-pty`, `fxg-hub`, `fxg-node`, `ui`
- **対象スクリプト / 設定**: `mise run check` / `mise run fmt` / `mise run types`
- **関連ドキュメント**:
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md) §7 (PTY と `allow_remote_pty`)
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md) §3.3 (`/api/v1/pty/ws`)
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md) §5 (ConPTY / Job Object)
  - [`docs/07-web-ui-and-pwa.md`](../07-web-ui-and-pwa.md) §4 (Web ターミナル)
  - [`docs/changelog/2026-11/2026-11-14-web-terminal-ime-and-duplicate-pty.md`](../changelog/2026-11/2026-11-14-web-terminal-ime-and-duplicate-pty.md)

---

## 0. 結論

- ターミナルのスコープを **`(node_id, canonical local_path)` = workspace** とする。
  プロジェクトはそのグルーピング（表示上の親）に留める。
- ターミナルの寿命を WebSocket から切り離す。**参照カウント + 無操作 TTL** で
  保持し、socket 切断では kill しない。
- `session_id` は所有権ではなく **origin（どのセッションが開いたか）** として残す。
- 出力は**リングバッファに保持**し、attach 時に replay する。
- ターミナルは**ノードローカル**のまま。DB へ永続化せず、ハブはルーティングと
  一覧の中継だけを行う。

## 1. 現状 (2026-11-14)

現行実装は「開いている間だけ存在する使い捨て PTY」である。

- UI の `TerminalView` がマウント時に `/api/v1/pty/ws` を開き `spawn` を送る。
- `spawn` は `session_id` を必須とし、ノードがそのセッションの `local_path` を
  cwd に使う。セッションが無ければ spawn できない。
- socket を閉じると、その socket が spawn した PTY は kill される。
- 出力は broadcast のみで、購読していない間のデータは捨てられる。
- PTY はノードのメモリ上の map にしか存在せず、列挙 API も無い。

この構造から、次の 4 点が実害になっている。

1. パネルを閉じる・セッションを切り替える・リロードするたびにシェルの状態
   (cwd / env / 実行中ジョブ) が失われる。
2. 同じ worktree で 2 セッション動かすとシェルが 2 本になり、dev server や
   `git` の状態を共有できない。
3. 再接続しても過去の出力が出ないため、事実上「開き直し」しかできない。
4. 生きているターミナルを一覧・復元する手段が無い (プロトコルに `attach` は
   あるが UI からは使われていない)。

## 2. なぜ project ではなく workspace か

`projects` は Git リモート由来の論理 ID で、シェルの実行単位ではない。

- 1 プロジェクトは複数ノードにまたがる。PTY はノード上の実プロセスなので、
  プロジェクト単位の 1 本では「どのノードで動かすか」が決まらない。
- 1 プロジェクトは 1 ノード上でも複数のローカルパスを持つ
  (メインリポジトリと worktree)。エージェントが worktree で作業しているのに
  ターミナルが main repo を向くのは事故になる。
- プロジェクトの同一性は共有されるが、シェルは共有できない。プロジェクトに
  状態を持たせると、エフェメラル VM やノード再起動で嘘の状態が残る。

一方、`(node_id, local_path)` は「そのシェルがどこで動いているか」と 1:1 で、
cwd が常に一意になる。プロジェクトは workspace をまとめて表示するためだけに
使う。

### 用語

- **workspace**: `(node_id, canonicalize 済みの local_path)`。ターミナルの
  スコープキー。
- **terminal**: workspace 上の PTY セッション。既定では workspace ごとに 1 本。
- **origin session**: ターミナルを開いたセッション。所有権ではなく来歴。

## 3. 設計

### 3.1 スコープキーと同一性

- パスは起動時に `dunce::canonicalize` で正規化した絶対パスをキーにする
  (シンボリックリンクや `..` で同一 workspace が分裂しないようにする)。
- パスはノードの `project_node_bindings` / `project_path_links` に登録済みで
  なければならない (§3.6)。worktree は独立した workspace として扱う。
- `pty_id` は内部 ID として残す。外部 API の同一性は workspace キーで表し、
  `pty_id` は attach / 操作のハンドルとしてのみ使う。
- 既定では **1 workspace = 1 terminal**。同一 workspace への `open` は
  新しいシェルを作らず既存 PTY への attach を返す（冪等）。
- 複数ターミナル (タブ) は Phase 3。その場合も workspace キー +
  terminal 連番で表し、workspace の既定ターミナルは 1 本のままにする。

### 3.2 ライフサイクル

状態は「起動 → 接続中 → 切断 (idle) → 終了」の 4 段階とする。

- **spawn / open**: workspace に PTY が無ければ起動し、あれば attach する。
- **attach**: socket の参照カウントを +1。idle タイマーを止める。
- **detach**: socket を閉じたときに -1。0 になった時点で idle タイマー開始。
  ここでは kill しない。
- **expire**: idle タイマー満了 (既定 30 分、ノード設定で変更可) で kill。
- **kill**: Web UI / CLI / kill switch / ノード停止 / worktree 削除による明示終了。
- **exit**: シェル自身の終了。終了コードを購読者へ配信し、しばらく
  「終了済み」として残してから破棄する（Phase 2 で扱う）。

UI の「Reconnect」は attach に変える。新しいシェルが欲しい場合だけ、
明示的な「New shell」操作 (kill + spawn) を提供する。

### 3.3 出力保持と replay

- PTY ごとに固定長のリングバッファ (既定 256 KiB、ノード設定で上限変更可) を
  持つ。broadcast の有無に関わらず出力を追記する。
- 各出力チャンクに単調増加のシーケンス番号を付ける。
- attach 時はリングバッファの内容を 1 つの snapshot として先に送り、以降の
  差分をシーケンス番号で続ける。UI は重複を捨て、欠落を検知できる。
- broadcast の `Lagged` は従来どおり最新優先で許容するが、snapshot 境界の
  シーケンス番号で整合を取る。

### 3.4 複数クライアント

- 1 つの PTY に複数 socket が attach できる (複数タブ・複数端末)。
- 入力は接続中の全クライアントから受け付ける。
- ウィンドウサイズは「最後に resize を送ったクライアント」を採用し、
  全購読者へ resize 通知を配る。UI 側は通知に従って再 fit する。
- 2 人目以降の接続は UI に「共有中」を表示する（入力が混ざることを隠さない）。
- read-only モードは Phase 3。

Phase 1 では「複数クライアントが同時に attach できる」ところまで実装し、
resize 通知・共有中バッジ・read-only は Phase 3 に回す。

### 3.5 列挙 API

- `GET /api/v1/terminals?node_id=...` を追加し、workspace ごとの
  ターミナル一覧を返す: `pty_id` / `path` / `project_id` /
  `origin_session_id` / 状態 / attach 数 / 作成時刻。
- ノードはメモリ上のレジストリから返す（DB は読まない）。ハブ経由の場合は
  ハブがノードへプロキシし、ノードがオフラインなら空 + 理由を返す。
- 再起動後に一覧が空になるのは正しい挙動とする。UI は「消えた」ではなく
  「今は無い」と表示し、必要なら開き直す導線を出す。

### 3.6 パス検証 (セキュリティ)

- `open` の `path` は、そのノードの `project_node_bindings` または
  `project_path_links` に**完全一致**するものだけ許可する。
  prefix 一致でサブディレクトリを開かせない（任意 cwd でのシェル起動を防ぐ）。
- 既存の `allow_remote_pty`（既定 false、ハブ経由の spawn を拒否）を踏襲する。
- 認証は既存のトークン / Host / Origin 検証をそのまま使う。
- attach も `pty_id` の存在確認に加え、要求元が同じノードに到達できることを
  前提にする（ハブのルーティング表を経由させる）。

### 3.7 監査

- `pty_spawn` は継続し、details に workspace のパスと origin session を入れる。
- `pty_kill` を追加する。`pty_exit` は頻度が高くノイズになるため、Phase 1 では
  監査ではなくログ (`tracing`) に留める。attach も記録しない。
- kill switch の集計 (`killed_ptys`) はそのまま。

### 3.8 UI

- パネル状態をセッション画面のローカル `$state` からアプリレベルの store へ移す。
- セッション画面のターミナルパネルは、そのセッションの workspace を
  open/attach する。同じ workspace のセッションを切り替えても再接続しない。
- リロード後は一覧 API で復元し、attach + replay で scrollback を戻す。
- プロジェクト画面に workspace ごとのターミナル導線を置く (Phase 2)。
- `64rem` を跨ぐリサイズでパネルが再マウントされても、dispose では detach
  だけを行い PTY を kill しない（既知の制約の解消）。

### 3.9 worktree / セッションとの連動

- worktree 削除時は、そのパス配下の workspace のターミナルを kill する
  （削除 API の中で明示的に行う）。
- セッション削除ではターミナルを残す。origin session が消えた場合は
  `origin_session_id` を空にする。
- パスが外部要因で消えた場合は kill しない（シェルは生きている）。UI に
  警告を出し、次の open では新しいディレクトリとして扱う。

### 3.10 非目標

- ターミナル状態の DB 永続化・ノード間移行・ハブ再起動を跨ぐ復元。
- エフェメラル VM の破棄を跨ぐ保持（VM と共に消える）。
- シェル統合 (command blocks / cwd 追跡 / プロファイル連携)。
- ACP の `terminal/*`（エージェント用）のスコープ変更。あちらは agent
  プロセスに従属する別系統として維持する。

## 4. プロトコル変更の概要

型の正は `crates/fxg-protocol`（と生成 bindings）とし、ここでは意味だけを定める。

- クライアント → サーバー:
  - `spawn` を `open` に置き換える。`scope`（node_id + path または
    project_id + path）、`cols` / `rows`、任意の `session_id`（origin）、
    `new`（既存を無視して作り直すか）を運ぶ。
  - `attach` は既存のまま `pty_id` で再接続する。
  - `input` / `resize` / `kill` は既存のまま。
- サーバー → クライアント:
  - `spawned` の代わりに、attach か新規かを区別できる応答を返す。
  - `output` にシーケンス番号を追加し、attach 直後の snapshot を表現する。
  - `exit` / `error` は既存のまま。

## 5. 不変条件

1. 1 workspace につき既定 1 ターミナル。同一 workspace への open は冪等。
2. 1 つの `TerminalView` は最大 1 つの PTY にしか接続しない（既存の不変条件）。
3. socket 切断だけでは PTY を kill しない。kill は明示操作・TTL・プロセス exit
   のいずれかでのみ起きる。
4. ターミナルはノードローカル。プロジェクト / ハブはグルーピングとルーティング
   のみを持ち、状態の正にはならない。
5. パスは登録済み binding に完全一致するものだけを workspace として開ける。
6. `session_id` は来歴であり、セッションの寿命はターミナルの寿命を決めない。

## 6. 段階計画

Phase 1 から workspace キーで実装する。session キーで寿命分離だけ先に行うと、
Phase 2 でキーを作り直すことになり、得るものが少ない。

### 6.1 実装状況 (2026-11-14)

Phase 1 は実装済み。Phase 2 / 3 は [`docs/backlog.md`](../backlog.md) に残した。

- `fxg-pty`: workspace レジストリ (`open` / `attach` / `detach`)、参照カウント +
  無操作 TTL (既定 30 分の定数)、リングバッファ (256 KiB) とチャンク
  シーケンス番号、`PtyReplay`。
- `fxg-protocol`: `PtyClientMessage::Open`、`PtyServerMessage::Opened`、
  `output.seq`、`TerminalSummary` / `TerminalsResponse`、
  `NodeRpcRequest::ListTerminals`、`PtyOpened` / `PtyOpenFailed` /
  `PtyAttach` / `PtyDetach`。
- node / hub: パス検証 (binding または session `local_path` との完全一致)、
  workspace 単位の open 直列化、ハブ側 replay ログ、socket 切断時の detach、
  `GET /api/v1/terminals`。ハブ接続断ではノードが hub 由来の attach を落とす。
- UI: `TerminalView` は `open` を使い、workspace キーの変化でのみ再接続する。
  dispose は detach のみ。attach 後の fit でサイズを合わせ直す。
- 監査: `pty_spawn` の details に path / attached を追加し、`pty_kill` を追加。

未対応: TTL / バッファ上限の設定化、複数クライアントの resize 調停、
終了済みターミナルの表示、worktree 削除時の連動 kill、UI の一覧復元。

- **Phase 1: 寿命分離と replay**
  - workspace キー、参照カウント + idle TTL、リングバッファ + replay、
    `GET /api/v1/terminals`、パス検証、監査の追加。
  - UI は従来どおりセッション画面から開くが、dispose では detach のみ。
  - セッション切替・リロードでシェルが残る。
- **Phase 2: workspace の可視化と連動**
  - プロジェクト画面からの導線、リロード後の復元、worktree 削除連動、
    「終了済み」表示。
- **Phase 3: 複数ターミナルと共有**
  - workspace あたりの複数タブ、共有中バッジ、read-only、resize 調停の改善。

## 7. リスクと代替

- **共有ターミナルの混乱**: 別タブの入力が見える。表示で明示し、Phase 3 で
  read-only とタブ分離を用意する。既定 1 本を崩さないのはこのため。
- **TTL による意図しない終了**: 既定を長め (30 分) にし、ノード設定で変更可能に
  する。UI に残り時間や「まもなく終了」を出すかは Phase 2 で判断する。
- **メモリ使用量**: リングバッファ × ターミナル数。上限をノード設定で制限し、
  idle 中のターミナル数を上限で刈る。
- **パス検証の穴**: binding 未登録のパスでセッションが作られる経路があれば
  spawn できない。実装時にセッション作成側の登録漏れを確認する。
- **代替案 (却下)**: プロジェクト単位の 1 本。cwd 規則とノード選択を発明する
  必要があり、worktree 並列という主要ユースケースと衝突する。
- **代替案 (却下)**: terminal 状態をハブ DB に持つ。ノード再起動や VM 破棄で
  嘘になり、ローカルファーストの原則に反する。

## 8. 未決事項

- 既定 TTL とリングバッファ上限を `node_settings` に持たせるか、`node.toml`
  に置くか（実行時変更したいので前者が自然）。Phase 1 では定数のまま。
- 一覧 API を REST にするか、Client WS のイベントにするか。
- 終了済みターミナルの scrollback を「直近 1 世代だけメモリ保持」するか。
- Phase 3 の複数ターミナルを最初からキーに含めるか（後方互換を増やさないため、
  Phase 1 では既定 1 本に固定し、拡張可能なキーだけ用意する方針）。

## 9. 受け入れ条件

- ターミナルパネルを閉じて開き直すと、同じシェル (cwd / env) に戻る。
- ブラウザをリロードしても scrollback が復元される。
- 同じ workspace の 2 セッションから開いてもシェルは 1 本。
- worktree を削除すると、その workspace のターミナルが終了する。
- ノードを再起動すると一覧が空になり、UI がそれを正しく表示する。
- 登録されていないパスを指定した open は拒否される。
- kill switch / ノード停止で全ターミナルが確実に終了する。
