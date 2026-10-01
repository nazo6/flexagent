# antigravity-acp の起動遅延調査と終了時リーク修正 / プロセス再利用設計メモ

- **日付**: 2026-10-01
- **対象パッケージ**: `fxg-pty`, `fxg-acp` (調査: `fxg-node`
  のセッション起動経路)
- **対象スクリプト**:
  `target/tmp/measure-antigravity-startup.mjs`,
  `target/tmp/test-antigravity-childkill.mjs`,
  `target/tmp/test-antigravity-tempredirect.mjs`,
  `target/tmp/bench-smallfiles.mjs`,
  `target/tmp/e2e-antigravity-leak.ps1` (全て git 管理外)

---

## 1. 背景: 起動が 22 秒かかる実測結果

`antigravity-acp` (Google Antigravity, ACP Registry の binary 配布) は
セッション開始 (spawn → `initialize` → `session/new`) に **約 22 秒**かかる。
実測 (2026-10-01, Windows / NVMe) の内訳:

| 区間                | 時間         | 内容                                                                                               |
| ------------------- | ------------ | -------------------------------------------------------------------------------------------------- |
| spawn → Python 起動 | **約 16 秒** | **PyInstaller onefile の自己展開** (312MB / 8,283 ファイルを `%TEMP%\_MEIxxxx` へ) + Python ブート |
| 認証                | 約 2.5 秒    | OAuth トークン更新 + `loadCodeAssist` (Google API 往復、毎回)                                      |
| `session/new`       | 約 5 秒      | MCP 設定ロード + ローカル資格情報プロキシ起動 + 会話作成                                           |

- ウォーム起動でも同値 (OS キャッシュや初回スキャンの問題ではない)。
- 導入済み 1.2.1 が最新。支配的な 16 秒は Google 側のパッケージ形式
  (PyInstaller onefile) に固有で、fxg からは直接短縮できない。
- 参考: `%TEMP%` への生ファイル作成は約 1,124 files/s (0.9 ms/file) であり、
  展開 16 秒のうち約 8 秒は素の I/O、残りは展開 CPU とウイルス対策のスキャン。

### 副次的な発見

- **`_MEI` 残留 (7.14GB / 26 個)**: セッション停止のたびに 312MB が
  `%TEMP%` に残っていた (後述の修正で解消)。
- **1 プロセスで複数 `session/new` が可能**: 2 セッション目は約 3 秒で作成
  できる (プロセス再利用による高速化の根拠)。
- **参考**: `~/.gemini/config/mcp_config.json` は JS コメント入りの不正 JSON
  (警告ログのみ。中身は全コメントアウトのため実害なし)。

## 2. 修正: 終了時に `_MEI` が自動削除されない問題 (実装済み)

### 原因

PyInstaller onefile は「子 (実体の Python プロセス) が終了すると、親
(ブートストラップ) が展開ディレクトリを削除して自然終了する」仕様。
しかし `AcpDriver` の終了処理 (`crates/fxg-acp/src/acp.rs`) が
`child.kill()` で**ルートを即時強制終了**していたため、後始末が走らなかった。

### 修正内容

終了ポリシーはプロセス管理クレート (fxg-pty) に一般 API として集約し、
ACP ドライバ側は抽象だけを呼ぶ形にした (PyInstaller 固有の知識は
ACP クライアントには置かない)。

- `fxg-pty`: `ProcessTreeGuard::shutdown_tree(root_pid, grace)` を**全プラット
  フォーム共通の API**として追加 (プラットフォーム差は cfg 実装ブロックに隠蔽)。
  「内側のプロセスから終了 → ルートの自然終了を待機 → 猶予超過時のみ強制終了」
  をカプセル化する。
  - **Windows 実装**: `QueryInformationJobObject(JobObjectBasicProcessIdList)`
    で Job メンバーを列挙してルート以外を `TerminateProcess` し、ルートの自然
    終了を `WaitForSingleObject` で待つ (プロセスハンドルのシグナル待ち、
    ポーリングなし。`tokio::task::spawn_blocking` で非同期文脈から呼ぶ)。
    子孫が居ないプロセスは待機せず即時終了 (従来と同じ速度)。
  - **Unix 実装**: Job Object に相当する仕組みが無いためルート (直接の子) を
    `SIGKILL` するのみで、待機は呼び出し側の `Child::status()` に任せる
    (waitpid を fxg-pty 側で行うと `status()` と競合するため)。
  - 結果は
    `TreeShutdownOutcome { terminated_descendants, root_exited_naturally }`。
  - `windows` クレートを追加したが `win32job` 経由で既に依存ツリーに存在し、
    追加コストは実質ゼロ (Cargo.lock の `windows 0.61.3` を参照)。
- `fxg-acp`: 終了処理は `shutdown_tree(child.id(), AGENT_EXIT_GRACE)` の呼び出し
  (15 秒猶予) のみとなり、**`#[cfg]` 分岐もプロセスツリーの知識も持たない**。
  プロセスツリーへの登録も `ProcessTreeGuard::attach_pid(pid)` (PID ベース、全
  プラットフォーム共通) に統一し、従来の
  `#[cfg(windows)] + attach_raw_handle(child.as_raw_handle())` を除去した
  (Job Object へはハンドルを開き直して割り当てる。Unix では no-op)。

### なぜクライアント側でやるのか (設計判断)

- 「内側のプロセスを終了してラッパーに後始末させる」は PyInstaller 固有では
  なく、ラッパー型ランチャー (`npx` → node / `uvx` → python / PyInstaller
  onefile) 共通の一般則。**公式 ACP SDK も Unix では同じ理由
  (`wrapper launchers … killing only the immediate child orphans the real
  agent`) でプロセスグループ単位の終了を行っており、Windows 側にその実装が
  無い**ことの補完に相当する。
- ACP v1 には「接続終了 / プロセス終了」メソッドが存在せず、stdin EOF でも
  antigravity は終了しない (実測)。ACP Registry にも kill ポリシーの記載は
  無い。SDK 自身が「spawn したプロセスの終了は呼び出し側の責務」と定義して
  いるため、現時点ではクライアント側に置く以外の選択肢が無い。
- `session/close` は仕様に存在するが (v1 の `ActiveSession` には未露出)、
  セッションの論理終了でありプロセスの後始末は変わらない。将来 ACP に接続終了
  が追加された場合は `shutdown_tree` の前に「プロトコルで閉じる」を足すだけで
  済む構造とした。
- **既知の制約 (Unix)**: SDK は `process_group(0)` を設定済みだが、fxg の Unix
  実装はルートしか kill しないため、ラッパー型ランチャーの内側のプロセスは
  孤児になり得る (Windows の Job Object に相当する列挙手段が無い)。公式 ACP SDK
  はプロセスグループ単位の kill で対処するが、ルートごと終了するためラッパーの
  後始末は走らない。「ルートを残したプロセスグループ終了」は将来の課題
  (Windows ファーストのため今回は `SIGKILL` のみで従来動作を維持)。

### 検証

- 単体テスト (`crates/fxg-pty/src/proc.rs`):
  - `terminate_all_except_spares_root_but_kills_the_rest` (実 Job Object と
    実プロセスで「孫は死ぬ・ルートは生存」を検証)。
  - `shutdown_tree_waits_for_wrapper_cleanup` (ラッパーが孫の終了後に後始末を
    実行して自然終了することを `root_exited_naturally`
    とマーカーファイルで検証)。
- E2E (`target/tmp/e2e-antigravity-leak.ps1`, 一時 `FXG_HOME` + 別ポート):
  起動時に `_MEI*` が展開 → `fxg session kill` → デーモンログ
  「`TreeShutdownOutcome { terminated_descendants: 3, root_exited_naturally: true }`」
  → 約 1 秒でブートストラップが自然終了 → `_MEI` 自動削除・プロセス残存なしを
  確認 (リファクタ後にも再実行して PASS)。
- クロスプラットフォーム: `cargo check -p fxg-pty --target
  x86_64-unknown-linux-gnu` で Unix 実装の型チェックを実施 (`fxg-acp` は
  C ツールチェーン未導入のためクロスチェック不可だが、変更はプラットフォーム
  中立)。

### 却下・保留した施策

- **TEMP 集約 + 起動時 janitor**: 異常終了 (デーモン強制 kill / OS 再起動)
  時のみ有効な保険であり、通常経路は本修正で解決するため見送り
  (最小変更方針)。異常終了時の残留は手動掃除で対応する。
- **Defender 除外**: 未実施。設定する場合は `%TEMP%\_MEI*` ではなく
  TEMP 集約とセットで行うのが安全 (ランダム名のため対象を絞れない)。

## 3. 設計メモ: プロセス再利用 (warm agent) — 未実装

**目的**: 2 セッション目以降の作成を 22 秒 → 3〜5 秒へ短縮する。
1 プロセスが複数セッションをホストできることを実測済み。

### 設計案

- **プール**: `agent_id` (+ 起動 spec ハッシュ) ごとに、`initialize` 済みの
  接続とプロセスを最大 1 個保持する。アイドル TTL (既定 10 分程度) 超過で
  終了する。
- **セッション開始**: 互換なアイドルがあれば `session/new` のみ発行 (約 3〜5
  秒)。
  なければ従来どおり spawn (約 22 秒)。
- **セッション終了**: プロセスを kill せず接続を維持してアイドルへ返す。
  現在の `AcpCommand::Shutdown` を「セッション終了 (返却)」と
  「プロセス破棄」に分離する (`ActiveSessionHandle` の `shutdown` = 返却、
  新たに `dispose` = プロセス終了)。
- **破棄条件**: TTL 超過 / プロセス死亡 (接続断の検知) / `fxg kill-all` /
  デーモン終了。共有プロセスの死亡時は、そのプロセスに載る全セッションを
  `Failed` として通知する。
- **fxg-acp の構造変更**: `run_acp_session` を (a) プロセス + 接続 +
  アイドル返却を所有する常駐タスクと、(b) セッションごとの
  プロンプト/更新ループに分離する。`session/load`・`session/resume` は
  温まった接続上でも従来どおり動作する。
- **事前起動 (任意)**: UI で新規セッション画面を開いた時点で
  `PrepareAgent` を発行し、ユーザー操作中に初期化を進める (プール実装が前提)。

### 未決事項

- アイドル TTL の既定値と設定項目の有無 (`config.toml` の `[agents]` 配下)。
- プールで保持するプロセス数の上限 (agent ごと 1 個で足りる見込み)。
- `fxg session kill` の意味の再定義 (セッションのみ終了し、プロセスは
  温存される) と CLI 出力の文言。

## 4. 計測用スクリプト (git 管理外)

- `measure-antigravity-startup.mjs`: ACP ハンドシェイクの所要時間計測。
- `test-antigravity-childkill.mjs`: 子のみ kill した場合の `_MEI` 自動削除検証。
- `test-antigravity-tempredirect.mjs`: `TEMP`/`TMP` 差し替えの有効性検証。
- `bench-smallfiles.mjs`: 小ファイル作成スループットのベースライン計測。
- `e2e-antigravity-leak.ps1`: 一時 `FXG_HOME` でのセッション起動→停止 E2E。
