# 型安全性改善プラン (`serde_json::Value` / 文字列判定の排除)

- **日付**: 2026-11-08
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-acp`, `fxg-node`,
  `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `mise.toml` (`lint:errors` 拡張),
  `clippy.toml`, `.github/workflows/ci.yml`, `AGENTS.md`,
  `docs/06-type-safety.md`（新規）, `ui/tsconfig.json`, `ui/.oxlintrc.json`,
  `ui/src/lib/generated/bindings.ts`（生成物）

---

## 1. 目的と設計原則

外部境界（HTTP / WS / stdio / SQLite / ACP / CLI 引数）で必ずドメイン型へ
パースし、内部では意味を持たない `String` 比較と生 JSON アクセスを禁止する。

- **Parse, don't validate**: 境界で型に変換し、内側では型だけを扱う。
- **未知値を失わない**: 同期プロトコルのため、未知の値は `Unknown(String)`
  として保持し、既定値に丸めない。
- **不透明 JSON は明示する**: ツール引数など本質的に動的な値は `RawJson`
  newtype で「意図的に不透明」と示す。
- **語彙は1箇所**: serde の rename / `as_str()` / `FromStr` / DB の TEXT を
  同じ定義から生成し、ドリフトを防ぐ。
- **機械的に再発防止**: clippy と専用 lint で新しい `serde_json::Value` や
  `kind.starts_with(...)` の混入を CI で止める。

---

## 2. 現状の課題（調査サマリ）

### 2.1 ドメイン値が `String`

| 対象                                | 箇所                                          | 問題                                                     |
| :---------------------------------- | :-------------------------------------------- | :------------------------------------------------------- |
| `ToolCall.kind` / `status`          | `crates/fxg-protocol/src/events.rs:156-166`   | ACP の enum を文字列化して保持                           |
| `ToolCallProgress.status`           | `crates/fxg-protocol/src/common.rs:524-560`   | 同上                                                     |
| `PlanEntry.status`                  | `crates/fxg-protocol/src/common.rs:594-602`   | 同上                                                     |
| `PermissionOption.kind`             | `crates/fxg-protocol/src/common.rs:604-620`   | `allow_*` / `reject_*` の意味判定を文字列で実施          |
| `ElicitationRequest.mode`           | `crates/fxg-protocol/src/events.rs:209-219`   | 現状 `"form"` 固定だが型なし                             |
| `UserMessage.client_source`         | `crates/fxg-protocol/src/events.rs:118-124`   | `fork` / `resume` を内部マーカーとしても使用             |
| `PermissionResolved.resolved_by` 等 | `crates/fxg-protocol/src/events.rs:195-203`   | `cli` / `web` / `android_push` の列挙                    |
| `ForkHistoryItem.role`              | `crates/fxg-protocol/src/common.rs:760-770`   | `"user"` 以外をすべて Assistant 扱い                     |
| `AgentLaunchSpec.driver_kind`       | `crates/fxg-acp/src/driver.rs:20-34`          | `"acp"` を文字列比較                                     |
| `AuditLogEntry.action`              | `crates/fxg-protocol/src/common.rs:1408-1425` | `fxg-db/src/audit.rs` の定数群と二重管理                 |
| `SearchHit.event_type`              | `crates/fxg-protocol/src/common.rs:1390-1405` | `UnifiedEventPayload::event_type()` と DB 文字列が別管理 |
| `NodeSummary.os` / `arch`           | `crates/fxg-protocol/src/common.rs:1301-1310` | OS / arch の語彙が型で保証されない                       |

### 2.2 文字列による意味判定

- `crates/fxg-acp/src/acp.rs:2095-2113`: 未知の `ToolCallStatus` /
  `PlanEntryStatus` を `_ => "pending"` に丸めている。
- `crates/fxg-acp/src/acp.rs:2115-2123`: 未知の `PermissionOptionKind` を
  `_ => "unknown"` に落とし、元の値を失っている。
- `crates/fxg-db/src/events.rs:693-720`:
  `selected_option_id.contains("reject")` で承認/拒否を推測。
- `crates/fxg-server/src/state.rs:245-270` /
  `crates/fxg-node/src/daemon/api.rs:865-890`:
  `option.kind.contains("always") || option.option_id.contains("always")`。
- `crates/fxg-server/src/push.rs:240-255`: `starts_with("allow")` /
  `starts_with("reject")`。
- `crates/fxg-cli/src/commands.rs:1600-1660,1777`: `option_by_kind` が
  `starts_with` 判定。
- `crates/fxg-node/src/session_manager.rs:1941`: `item.role == "user"` 以外を
  すべて Assistant 扱い。
- `crates/fxg-node/src/session_manager.rs:2575,2725,2764,2902`:
  `client_source == "fork"` / `"resume"` で内部注入を判定。
- `crates/fxg-server/src/provisioner.rs:675,711,769-782`:
  `line.contains("\"op\":\"node_hello\"")` で stdout を判定し、その後 JSON を
  再パースしている。
- `crates/fxg-node/src/session_manager.rs:73`: `driver_kind.as_str()` で
  実行時エラーになりうる。

### 2.3 `serde_json::Value` の混在

**型にすべきもの**

- `ElicitationRequest.requested_schema` / `ElicitationResolved.content`
  （`crates/fxg-protocol/src/events.rs:209-236`）
  - 検証: `crates/fxg-acp/src/acp.rs:2144-2225`
  - CLI の再解釈: `crates/fxg-cli/src/commands.rs:1710-1726`
  - UI の独自 schema 定義: `ui/src/lib/components/ElicitationCard.svelte:18-46`
- `ConfigOptionInfo.current_value` / `options` /
  `SessionControlAction::SetConfig.value`
  （`crates/fxg-protocol/src/common.rs:649-676`）
  - 現状は `map_config_option` が Select / Boolean を
    `Value::String` / `Value::Bool` に落とし、`parse_config_value` が JSON の
    スカラー型で再分岐している（`crates/fxg-acp/src/acp.rs:1703-1755`）。
    `ConfigOptionValue` にすれば網羅 `match` になる。
- `AuditLogRecord.action` / `AuditLogEntry.action`
- CLI `provisioners --json` の `Value` 組み立て
  （`crates/fxg-cli/src/commands.rs:610-650`）

**不透明のまま残すが境界を明示すべきもの**

- `PermissionRequestEntry.details`（ツール引数）
- 未知の `ConfigOptionInfo` 値
- 将来の elicitation schema type
- `ErrorInfo.details`（現状 UI は JSON 文字列表示のみ）

### 2.4 DB 境界

- `crates/fxg-db/src/queries.rs:760-900` の row struct が `status: String` /
  `lifecycle_status: String` / `action: String` / `event_type: String`。
- `AuditLogEntry.action` / `SearchHit.event_type` は生文字列のまま通過。
- `SessionStatus` 等は serde / `as_str()` / `FromStr` の3箇所で語彙を定義。
- `crates/fxg-db/src/events.rs:446-460` などで SQL に `'pending'` / `'idle'`
  を直書き。

---

## 3. 目標設計

### 3.1 未知値を保持する open enum

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Unknown(String),
}
```

- `as_str()` / `FromStr` / `Serialize` / `Deserialize` はマクロまたは `strum`
  で1箇所から生成する。
- `Unknown` は JSON / DB をそのまま往復できることをテストする。
- 未知値を既定値に丸める `_ =>` を禁止する。

`PermissionOptionKind` には意味判定をメソッドとして持たせる。

```rust
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unknown(String),
}

impl PermissionOptionKind {
    pub fn is_allow(&self) -> bool;
    pub fn is_reject(&self) -> bool;
    pub fn is_always(&self) -> bool;
    pub fn is_once(&self) -> bool;
}
```

### 3.2 透過 JSON の newtype `RawJson`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
#[specta(transparent)]
pub struct RawJson(pub serde_json::Value);
```

- `Index` / `as_str()` は実装しない。
- `PermissionRequestEntry.details` などは `RawJson` にする。
- 生成 TS は従来どおり `JsonValue` を維持する。

### 3.3 Elicitation の型化

`fxg-protocol` に ACP 非依存の制限付きスキーマ型を定義する。

```rust
pub enum ElicitationProperty {
    String { r#enum: Vec<String>, one_of: Vec<OneOfOption>, title: Option<String>, /* ... */ },
    Integer { /* ... */ },
    Number { /* ... */ },
    Boolean { /* ... */ },
    StringArray { /* ... */ },
    Unknown(RawJson),
}

pub struct ElicitationSchema {
    pub properties: BTreeMap<String, ElicitationProperty>,
    pub required: Vec<String>,
}

pub enum ElicitationValue {
    String(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    StringArray(Vec<String>),
}
pub type ElicitationContent = BTreeMap<String, ElicitationValue>;
```

- 検証は `match property` の網羅列挙にする。
- CLI は `--set key=value` から `ElicitationValue` を構築する。
- UI は生成型を使い、独自 schema interface を削除する。

### 3.4 Config option の型化

```rust
pub enum ConfigOptionValue {
    Select(String),
    Boolean(bool),
    Unknown(RawJson),
}
```

既存 UI との互換が必要なら、まずは additive に `Unknown(RawJson)` を残す。

### 3.5 来歴・識別子の型化

- `ClientSource { Cli, Web, Android, Fork, Resume, Unknown(String) }`
- `Resolver { Cli, Web, AndroidPush, Unknown(String) }`
- `HistoryRole { User, Assistant, Unknown(String) }`
- `DriverKind { Acp, Unknown(String) }`
- `NodeOs` / `NodeArch`
- `AuditAction` / `EventType`
- `SessionId` / `NodeId` / `ProjectId` / `CommandId` / `RequestId` /
  `AgentId` の newtype 化（Phase 4、変更量大）

---

## 4. TODO

### Phase 0: 方針と検査基盤（挙動変更なし）

- [x] `docs/06-type-safety.md` を追加（open enum / `RawJson` / 変換責務）
- [x] `AGENTS.md` §3 / §4 に型安全ルールと参照先を追記
- [x] `clippy.toml` に `disallowed-types` を追加（`serde_json::Value`）
- [x] `mise run lint:errors` に `-D clippy::disallowed_types` と
      `-D clippy::wildcard_enum_match_arm` を追加
- [x] 既存の `serde_json::Value` 使用を理由付きの file-level
      `#![allow(clippy::disallowed_types)]` でベースライン化
- [x] `mise run lint` と `.github/workflows/ci.yml` に `lint:errors` を組み込む

#### Phase 0 の移行メモ

- 当初は独自の `xtask lint-typed` も検討したが、Phase 1 の enum 化で
  文字列判定自体がコンパイルエラーになるため、clippy 中心に絞った。
- `serde_json::Value` の既存使用は、ファイル（モジュール）単位の
  `#![allow(..., reason = "...")]` で許可している。新しいファイルでの使用は
  clippy が拒否する。Phase 2/3 で該当ファイルを型化したら allow を削除する。
- wildcard `_ =>` は既存違反が多いため、crate ルートの allow でベースライン化し、
  Phase 1 の enum 化に合わせて削除する。

### Phase 1: 閉じた enum 化（低リスク・高効果）

- [x] `PermissionOptionKind` を導入
  - [x] `crates/fxg-protocol/src/common.rs`
  - [x] `crates/fxg-db/src/events.rs`
  - [x] `crates/fxg-server/src/state.rs`
  - [x] `crates/fxg-node/src/daemon/api.rs`
  - [x] `crates/fxg-server/src/push.rs`
  - [x] `crates/fxg-cli/src/commands.rs`
  - [x] `selected_option_id` 推測を1関数に集約し、テストを追加
- [x] `DriverKind` を導入
  - [x] `crates/fxg-acp/src/driver.rs`
  - [x] `crates/fxg-acp/src/registry.rs`
  - [x] `crates/fxg-node/src/session_manager.rs`
- [x] `ClientSource` / `ResolvedBy` / `HistoryRole` を導入
  - [x] `crates/fxg-protocol/src/events.rs`
  - [x] `crates/fxg-protocol/src/common.rs`
  - [x] `crates/fxg-server/src/state.rs`
  - [x] `crates/fxg-node/src/session_manager.rs`
  - [x] `"fork"` / `"resume"` を enum variant に昇格
- [x] `ToolCallKind` / `ToolCallStatus` / `PlanEntryStatus` /
      `ElicitationMode` を導入
  - [x] `crates/fxg-protocol/src/common.rs`
  - [x] `crates/fxg-protocol/src/events.rs`
  - [x] `crates/fxg-acp/src/acp.rs` の `_ => "pending"` / `_ => "unknown"` を廃止
  - [x] open enum の TypeScript 出力は `string` のため、UI は追従不要
- [x] `AuditAction` / `EventType` を導入
  - [x] `crates/fxg-db/src/audit.rs`
  - [x] `crates/fxg-protocol/src/events.rs`
  - [x] `crates/fxg-db/src/queries.rs` の row struct

#### Phase 1 の移行メモ

- `open_string_enum!` マクロ (`fxg-protocol/src/string_enum.rs`) を追加した。
  未知値は `Unknown(String)` として保持し、JSON / DB の文字列表現は変えない。
- `driver_kind` は `fxg-acp::DriverKind` に、`resolved_by` は `ResolvedBy` に
  なった（計画の `Resolver` から名称変更）。
- `AuditAction` / `EventType` は `fxg-db` の定数・DB 文字列と接続した。
  `session_events.event_type` は引き続き TEXT。
- UI の生成型は open enum のため `string` のまま。Rust 側の網羅性を優先し、
  TypeScript の literal union 化は別途検討する。

### Phase 2: JSON 境界の整理

- [x] `RawJson` を `fxg-protocol` に追加し、specta 出力を確認
  - [x] `ui/src/lib/generated/bindings.ts` が `RawJson = JsonValue` のままであること
- [x] 不透明フィールドを `RawJson` に置換
  - [x] `PermissionRequestEntry.details`
  - [x] `PermissionRequest.details`
  - [x] `ErrorInfo.details`
  - [x] `AuditLogEntry.details`
- [x] `ElicitationSchema` / `ElicitationContent` を型化
  - [x] `ElicitationSchema` / `ElicitationProperty` / `ElicitationValue` を追加
  - [x] `crates/fxg-acp` の検証を網羅 `match` に置換
  - [x] `crates/fxg-cli` の `--json` / `--set` を型付き構築に変更
  - [x] `ui/src/lib/components/ElicitationCard.svelte` の独自 interface を削除
- [x] `ConfigOptionValue` を型化
  - [x] `ConfigOptionInfo.current_value` / `options` / `SetConfig.value`
  - [x] `AgentDriver::set_config`
  - [x] `ui/src/lib/config-options.ts` を生成型ベースに
- [x] provisioner stdout の typed parse
  - [x] `crates/fxg-server/src/provisioner.rs` を `NodeToServerMsg` の1回パースに変更
  - [x] `NodeToServerMsg` を `match` で分岐
  - [x] `extract_hello_node_id` を削除
- [x] CLI の `Value` 組み立て廃止
  - [x] `provisioners --json` を `ProvisionerListEntry` に
  - [x] `agents list --json` を `AgentListOutput` に

#### Phase 2 の移行メモ

- `RawJson` は `#[serde(transparent)]` + `#[specta(transparent)]` のため、
  wire format と TS の `JsonValue` は変わらない。`PermissionRequest.details` など
  ツール引数・監査 details・エラー details だけを不透明値として残した。
- Elicitation は schema を `ElicitationSchema`、回答を
  `Option<ElicitationContent>` にした。未知の property type は
  `ElicitationPropertyKind::Unknown(String)` として保持し、`extra` で
  未知キーも roundtrip する。`RespondElicitation.content` は `null` ではなく
  `None` で decline / cancel を表す。
- `ConfigOptionValue` は `#[serde(untagged)]` で文字列 / 真偽値 / 任意 JSON を
  そのまま往復する。ACP の未知 config value は `Unknown(RawJson)`。
- provisioner は `line.contains("\"op\":...")` を廃止し、stdout の各行を
  `NodeToServerMsg` として1回だけパースするようにした。
- Phase 0 の file-level `disallowed_types` allow は、Phase 2 で不要になった
  `provisioner.rs` / `cli` / `node` / `server` / `db` の該当ファイルから削除した。

### Phase 3: DB 境界の型化

- [x] `crates/fxg-db/src/queries.rs` の row struct を enum に
  - [x] `SessionRow.status` → `SessionStatus`
  - [x] `NodeRow.lifecycle_status` → `NodeLifecycleStatus`
  - [x] `PermissionRow.status` / `resolved_by` → `PermissionRequestStatus` /
        `Option<ResolvedBy>`
  - [x] `ElicitationRow.mode` / `status` / `resolved_by` → `ElicitationMode` /
        `ElicitationRequestStatus` / `Option<ResolvedBy>`
  - [x] `SearchRow.event_type` → `EventType`
  - [x] `AuditRow.action` → `AuditAction`
- [x] `AuditLogEntry.action` / `SearchHit.event_type` を enum に
      （row の decode 時点で型付きになった）
- [x] `crates/fxg-db/src/events.rs` の SQL 直書き status を bind に
  - [x] `'idle'` / `'pending'` を `SessionStatus::Idle.as_str()` などに
  - [x] `ON CONFLICT` は `excluded.status` を使い二重 bind を回避
  - [x] `queries.rs` の `WHERE status = 'pending'` も bind に
- [x] `sqlx::Type` derive（feature gate）または decode/encode ヘルパーを整備
  - [x] `fxg-protocol` に `sqlx` feature を追加
  - [x] `impl_sqlx_text!` で `Type` / `Encode` / `Decode` を実装
  - [x] `fxg-db` が `fxg-protocol/sqlx` を有効化
- [x] 全 enum variant の INSERT → SELECT 往復テストを追加
  - [x] `crates/fxg-db/tests/enum_roundtrip.rs`
- [x] 既存 DB の文字列は変更しない
  - [x] migration / `as_str()` / JSON の語彙はそのまま
  - [x] open enum（`AuditAction` / `EventType` / `ElicitationMode` /
        `ResolvedBy`）は `Unknown(String)` で往復
  - [x] 閉じた status enum は未知値を decode エラーにする（従来の
        `FromStr` 挙動と同じ。新しい status はプロトコル追加として扱う）

#### Phase 3 の移行メモ

- DB の TEXT 表現は変えていない。`fxg-protocol` の `sqlx` feature で
  `Type<Sqlite>` / `Encode` / `Decode` を実装し、`query_as!` の row フィールドを
  直接 enum にした。SQL 側は `AS "status: SessionStatus"` の型注釈で
  `From<String>` を要求させない。
- `SessionStatus` などの閉じた enum は、未知の DB 値を既定値に丸めず
  decode エラーにする（`unknown_session_status_is_not_silently_coerced`）。
  未知値を許容する必要がある列は Phase 1 の open enum を使う。
- SQL の `'pending'` / `'idle'` は `as_str()` の bind に置き換えた。
  既存 DB のインデックス（`WHERE status = 'pending'`）は migration 側に残る。
- `.sqlx` キャッシュはクエリ文字列の変更に合わせて再生成した
  （`mise run sqlx:prepare`）。

### Phase 4: ID newtype（任意・戦略的）

- [ ] `SessionId` / `NodeId` / `ProjectId` / `CommandId` / `EventId` /
      `RequestId` / `AgentId`
- [ ] protocol → server/node → db の順で段階導入
- [ ] `#[serde(transparent)]` + specta で TS は `string` を維持

### Phase 5: 定着

- [ ] `docs/06-type-safety.md` の allow 状況を更新
- [ ] `AGENTS.md` に「新しい String フィールド追加時のチェックリスト」を追記
- [ ] `docs/changelog/2026-11-08-type-safety-plan.md` の進捗メモを更新
- [ ] レビュー観点の明文化
  - [ ] `String` は「自由入力 ID / 表示名 / 外部識別子 / enum」のどれか
  - [ ] `_ =>` で既定値に丸めていないか
  - [ ] 未知値を往復できるか
  - [ ] DB / JSON / serde の語彙が1箇所か

---

## 5. 再発防止の仕組み

| 仕組み                                                            | 目的                                           |
| :---------------------------------------------------------------- | :--------------------------------------------- |
| `clippy.toml` の `disallowed-types` で `serde_json::Value` を禁止 | 新しい生 JSON の混入を防ぐ                     |
| `clippy::wildcard_enum_match_arm` を `-D`                         | `_ =>` による enum の握り潰しを防ぐ            |
| file-level `#![allow]` の削減（`lint:errors` で検出）             | 生 JSON の残存箇所を可視化・削減する           |
| `fxg-protocol` の specta surface audit テスト                     | 公開型に `kind: String` 等が増えたら失敗させる |
| `as_str` / serde / `FromStr` の一元マクロ + 往復テスト            | 3重定義のドリフトを防ぐ                        |
| DB enum 往復テスト                                                | TEXT 列と Rust enum の語彙ずれを検出           |
| 生成 TS 型の差分チェック（既存 CI）                               | UI 側の追従漏れを防ぐ                          |
| `docs/06-type-safety.md` + `AGENTS.md`                            | 判断基準を人間・エージェント双方に共有         |

---

## 6. 注意点

- ツール引数・未知のエージェント設定・将来の elicitation schema は
  本質的に開いている。型を強制せず `RawJson` / `Unknown` で境界を明示する。
- **未知値を既定値に丸めない**。同期プロトコルではデータ損失になる。
- `#[serde(deny_unknown_fields)]` を同期ペイロードに付けない。
  CLI 入力など自分たちが完全に制御する DTO に限定する。
- wire format は変えない。enum 化しても JSON 上の文字列は同一。
- 一度に全クレートを変えず、クレート単位の独立した PR にする。

---

## 7. 進捗メモ

- 2026-11-08: 調査とプラン作成。実装は未着手。
- 2026-11-08: `ad6f054`（ACP config の wire format 修正）と `bd2e079`
  （timeline の streaming 順序修正）に合わせて参照行を更新。`acp.rs` の
  config 変換が `map_config_option` / `parse_config_value` に分かれている点と、
  `permission_kind_str` が未知値を `"unknown"` に落としている点を追記。
  実装状況に変化はない。
- 2026-11-08: Phase 0 を実装。`docs/06-type-safety.md`、`AGENTS.md` の型安全
  ルール、`clippy.toml` の `disallowed-types`、`mise run lint:errors` の
  拡張、CI 組み込みを追加。既存の `serde_json::Value` は file-level
  `#![allow]`、wildcard `_ =>` は crate ルートの allow でベースライン化した。
  Phase 1 以降で allow を削減する。
- 2026-11-08: Phase 1 を実装。`open_string_enum!` マクロを追加し、
  `PermissionOptionKind` / `ToolCallKind` / `ToolCallStatus` /
  `PlanEntryStatus` / `ElicitationMode` / `ClientSource` / `ResolvedBy` /
  `HistoryRole` / `DriverKind` / `AuditAction` / `EventType` を導入した。
  ACP / DB / CLI / サーバーの文字列比較を enum のメソッドに置き換え、
  `selected_option_id` の推測は1関数に集約してテストを追加した。
  open enum の TypeScript 出力は `string` のため、UI の生成型は変わらない。
- 2026-11-08: Phase 2 を実装。`RawJson`、`ElicitationSchema` /
  `ElicitationProperty` / `ElicitationValue` / `ElicitationContent`、
  `ConfigOptionValue` を追加した。provisioner の stdout 判定を
  `NodeToServerMsg` の typed parse に変更し、CLI の provisioners / agents
  JSON 出力を Serialize struct に置き換えた。不要になった file-level
  `disallowed_types` allow も削除した。
- 2026-11-08: Phase 3 を実装。`fxg-db` の row struct を `SessionStatus` /
  `NodeLifecycleStatus` / `PermissionRequestStatus` /
  `ElicitationRequestStatus` / `ElicitationMode` / `ResolvedBy` /
  `AuditAction` / `EventType` に型化した。`fxg-protocol` に `sqlx` feature と
  `impl_sqlx_text!` を追加し、SQL の status リテラルを bind に置き換えた。
  全 variant の INSERT → SELECT 往復テストを追加し、`.sqlx` を再生成した。
