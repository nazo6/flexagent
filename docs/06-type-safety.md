# 型安全性ガイド (`serde_json::Value` / 文字列判定の禁止)

- **日付**: 2026-11-08
- **対象**: `fxg-protocol`, `fxg-db`, `fxg-acp`, `fxg-node`, `fxg-hub`,
  `fxg-cli`, `ui`
- **関連**: [`docs/proposals/2026-11-08-type-safety-plan.md`](./proposals/2026-11-08-type-safety-plan.md)

---

## 1. 目的

FlexAgent は Node ⇔ Hub ⇔ Client の3経路で同じイベントと REST 型を運ぶ。
ここで `String` の意味判定や生 JSON のフィールドアクセスが混ざると、次の問題が起きる。

- エージェントやピアの新しい値が「既定値」に丸められ、データが失われる。
- `kind.contains("always")` のような判定が、別の option を誤って選ぶ。
- JSON キーの typo が実行時まで分からない。
- DB の TEXT 語彙と Rust の enum、serde の rename が三重管理になる。

本ガイドは、**外部境界で型に変換し、内側では型だけを扱う**ためのルールと、
それを機械的に守るための仕組みを定義する。

---

## 2. 基本ルール

### 2.1 ドメイン値を `String` で持たない

`kind` / `status` / `role` / `mode` / `client_source` / `resolved_by` /
`event_type` / `action` / `os` / `arch` / `driver_kind` など、
取りうる値が列挙できるものは enum にする。

許される `String` は次のいずれかである。

| 種類          | 例                                     | 扱い                                |
| :------------ | :------------------------------------- | :---------------------------------- |
| 自由入力の ID | `session_id`, `agent_id`, `project_id` | newtype 化を検討（Phase 4）         |
| 表示名・説明  | `title`, `name`, `message`             | `String` のまま                     |
| 外部識別子    | `mode_id`, `providerID`                | エージェント定義の値なので `String` |
| 列挙値        | `status`, `kind`, `role`               | enum（未知値は `Unknown`）          |

### 2.2 未知値を既定値に丸めない

同期プロトコルでは、未知の値を勝手に既知の variant へ変換してはいけない。

```rust
// Bad: 将来の status が「保留中」として表示される
match status {
    ToolCallStatus::Pending => "pending",
    ToolCallStatus::InProgress => "in_progress",
    ToolCallStatus::Completed => "completed",
    ToolCallStatus::Failed => "failed",
    _ => "pending",
}

// Good: 未知値もそのまま保持する
match status {
    ToolCallStatus::Pending => "pending",
    ToolCallStatus::InProgress => "in_progress",
    ToolCallStatus::Completed => "completed",
    ToolCallStatus::Failed => "failed",
    ToolCallStatus::Unknown(raw) => raw,
}
```

`Unknown(String)` は JSON / DB をそのまま往復できることテストする。

### 2.3 文字列で意味を判定しない

```rust
// Bad
option.kind.contains("always")
option.kind.starts_with("reject")
selected_option_id.contains("reject")
item.role == "user"

// Good (enum のメソッド)
option.kind.is_always()
option.kind.is_reject()
history.role == HistoryRole::User
```

### 2.4 生 JSON は境界だけに置く

`serde_json::Value` を直接フィールドに持たない。`RawJson` newtype を使い、
「意図的に不透明である」ことを型で示す。

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawJson(pub serde_json::Value);
```

`RawJson` は `Index` や `as_str()` を実装しない。読みたい箇所だけ、
明示的に `as_object()` などを呼ぶ。

`serde_json::Value` を使ってよいのは次の場所だけである。

- `fxg-protocol` の `RawJson` 定義
- ツール引数など、エージェント依存で本質的に開いた payload
- JSON Schema / elicitation の未知拡張（`Unknown(RawJson)`）
- テスト（`crates/*/tests`, `#[cfg(test)]`, `testutil.rs`）

---

## 3. 変換の責務

| 境界                            | 変換する場所                     | ルール                                                                          |
| :------------------------------ | :------------------------------- | :------------------------------------------------------------------------------ |
| ACP SDK → `UnifiedEventPayload` | `fxg-acp` の変換関数             | SDK の enum を必ず enum へ写す。`_ =>` の既定値丸め禁止                         |
| DB TEXT → Rust                  | `fxg-protocol` の `sqlx` feature | row struct のフィールドを直接 enum にする。閉じた enum の未知値は decode エラー |
| JSON → Rust                     | `serde`                          | `Unknown` を持つ open enum。同期 payload に `deny_unknown_fields` を付けない    |
| DB TEXT → Rust                  | `fxg-db` の row 変換             | `FromStr` / `TryFrom` を境界で1回だけ呼ぶ                                       |
| HTTP / WS / stdio → Rust        | 各ハンドラ                       | parse 後は型だけを渡す。文字列 `op` 判定をしない                                |
| Rust → TypeScript               | `specta` + `mise run types`      | 生成物を直接編集しない。UI は生成型を import する                               |
| エンティティ ID → newtype       | `fxg-protocol/src/ids.rs`        | protocol の公開 DTO とクレート間 API のみ型化。内部は `&str` + `as_str()`       |

### 3.1 open enum の形

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unknown(String),
}

impl PermissionOptionKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::AllowOnce => "allow_once",
            Self::AllowAlways => "allow_always",
            Self::RejectOnce => "reject_once",
            Self::RejectAlways => "reject_always",
            Self::Unknown(raw) => raw,
        }
    }

    pub fn is_allow(&self) -> bool { ... }
    pub fn is_reject(&self) -> bool { ... }
    pub fn is_always(&self) -> bool { ... }
    pub fn is_once(&self) -> bool { ... }
}
```

`as_str()` / `FromStr` / `Serialize` / `Deserialize` は同じ語彙から生成する。
手書きで4箇所に文字列を書かない。

### 3.2 ID newtype の方針

- 全面置換はしない。`SessionId` などは protocol の公開フィールドと
  クレート間の関数引数だけに使い、内部の `HashMap<String, _>` や SQL は
  文字列のまま残す。
- `#[serde(transparent)]` + `#[specta(transparent)]` で JSON / TS は `string`。
  DB も TEXT のまま（`impl_sqlx_text!` を再利用）。
- `Deref<Target = str>` は実装しない。`&SessionId` が `&str` に自動変換されると、
  型付けした関数の取り違え防止が効かなくなる。
- `Default` は実装しない。空 ID を有効値として扱わない。
- `FromStr` は infallible。既存 DB や `project_id` の `local:` 形式を
  検証で壊さない（UUID 形式の検証は作成境界で行う）。
- 導入順は Tier A（`CommandId` / `NodeId` / `SessionId` / `EventId`）→
  Tier B（`ProjectId`）→ Tier C（必要時）。
- `request_id` のように用途が複数ある語彙は、単一の `RequestId` にまとめず
  用途別の型にするか文字列のまま残す。

### 3.3 設定・秘密・プロジェクト同一性の型

設定 API / WebSocket に汎用 Key-Value (`SettingKey(String)` + `SettingValue`) を
作らない。所有者ごとの型付き構造体をそのまま送受信する。

- `NodeSettingsSnapshot` / `HubSettingsSnapshot` / `NodeSettings` /
  `AgentSettings` / `HubSettings` / `SecretSummary` / `ProjectProfile`
- `ConfigSource` / `ProjectIdentityKind` / `SecretKind` は `open_string_enum!`
- `SecretId` は ID newtype（`is_valid()` でファイル名として安全か検証）
- `SourcedValue<T>` は起動設定の値と出所 (`ConfigSource` + `source_detail`) を
  組にする。UI は出所バッジとして描画するだけで、文字列キーで分岐しない。

`project_id.starts_with("local:")` のような判定は禁止し、
`ProjectIdentityKind` で分岐する。`project_key` は `user:<key>` として
`project_id` に埋め込むが、判定は `ProjectIdentity.kind` を使う。

---

## 4. 機械的な強制

### 4.1 clippy

`clippy.toml` で `serde_json::Value` を `disallowed-types` に指定し、
`mise run lint:errors` で次を有効化している。

- `clippy::disallowed_types`
- `clippy::wildcard_enum_match_arm`
- 既存の `let_underscore_must_use` / `map_err_ignore` / `disallowed_methods`

`serde_json::Value` の既存使用は、移行ベースラインとして
**ファイル（モジュール）単位** の `#![allow(..., reason = "...")]` で許可している。

```rust
#![allow(
    clippy::disallowed_types,
    reason = "elicitation payloads are Phase 2 typed; see docs/06-type-safety.md"
)]
```

新設ファイルや既存ファイルでこれらの型を扱う際は、境界で型化する。
ファイル単位の `#![allow(..., reason = "...")]` は型化が完了したモジュールから
順次削除する。

### 4.2 specta の型同期

`fxg-protocol` の型を変えたら必ず:

```bash
mise run types
```

CI は生成物 (`ui/src/lib/generated/bindings.ts`) の差分を検査する。
UI 側で `status: string` のような独自型を作らず、生成型を使う。

---

## 5. 新しい型を追加するときのチェックリスト

- [ ] その `String` は「ID / 表示名 / 外部識別子 / enum」のどれか説明できる
- [ ] enum なら `Unknown(String)` を持つか（未知値を失わないか）
- [ ] `as_str` / `FromStr` / serde の語彙が1箇所か
- [ ] 未知値の JSON / DB 往復テストがあるか
- [ ] `_ =>` で既定値に丸めていないか
- [ ] DB に保存する場合、row 変換で enum になっているか
- [ ] UI が生成型を使っているか（`JsonValue` / `unknown` を再定義していないか）
- [ ] `mise run lint:errors` が通るか

---

## 6. レビュー観点

- `serde_json::Value` が増えていないか。増えた場合、`RawJson` か理由付きの
  file-level allow か。
- `kind.starts_with` / `status == "..."` のような判定が増えていないか。
- 新しい enum variant を追加したとき、`Unknown` へのフォールバックが壊れていないか。
- DB の TEXT 語彙と Rust の enum が一致しているか。
- `#[serde(deny_unknown_fields)]` を同期 payload に付けていないか。

---

## 7. 適用状況と残課題

`PermissionOptionKind` / `ToolCallKind` / `ToolCallStatus` / `AuditAction` / `EventType` 等の主要ドメイン値は `open_string_enum!` で型化済み（未知値は `Unknown(String)` 保持）、非構造化ペイロードは `RawJson`、主要 ID（`CommandId` / `NodeId` / `SessionId` / `EventId` / `ProjectId`）は newtype 化済みです。残る ID newtype 化やベースライン allow の削除は [`docs/backlog.md`](./backlog.md) で管理します。
