# 新規セッションのモデル記憶をプロジェクト + エージェント単位にする

- **日付**: 2026-11-14
- **状態**: 実装済み
- **対象パッケージ**: `ui`, `docs`
- **対象スクリプト**: `mise run fmt:oxc`, `pnpm --dir ui run test`,
  `pnpm --dir ui run check`, `pnpm --dir ui run lint`

---

## 1. 背景

新規セッション画面はモデル / thinking level をプロジェクト単位で
localStorage (`fxg:session_config:<project_id>`) に保存していた。このため
エージェントを切り替えると、別エージェントで選んだモデル値が能力プローブの
`initial_config` にそのまま使われた。

モデル値の形式はエージェント固有である。OpenCode は `provider/model` を要求する
ため、Gemini CLI などで保存された `gemini-3.8-flash` のような裸の ID を渡すと、
ACP は `-32602 Invalid params` (`model not found`) を返す。既定モデルへ
フォールバックするため実害は小さいが、プローブごとに警告が出る。

## 2. 変更内容

- `ui/src/lib/session-prefs.ts`:
  - 設定キーを `fxg:session_config:<project_id>:<encoded_agent_id>` に変更する。
  - `loadSessionConfig` / `saveSessionConfigOption` はエージェント ID を必須に
    する。
  - 旧キー `fxg:session_config:<project_id>` は読まない。どのエージェントの値か
    復元できないため、他エージェントへ漏らすよりは既定値から始める。
- `ui/src/lib/components/session/NewSessionChat.svelte`:
  - プローブの初期モデルと保存値の照合を、現在のエージェントの記憶に限定する。
  - `initialConfig()` は、プローブ済みで選択肢と照合済みの値だけを送る。
    プローブできない、または失敗した場合は空を返し、エージェント既定値を使わせる。
- `ui/src/lib/components/chat/Composer.svelte`:
  - セッション中の `set_config` もプロジェクト + エージェント単位で保存する。
- `ui/src/lib/config-options.ts` / `docs/04-agent-drivers-and-windows.md`:
  - 保存単位と `initial_config` の検証条件を説明する。

## 3. 挙動の変更

- 同じプロジェクトでも、エージェントごとに別のモデル / thinking level を記憶する。
- 既存のプロジェクト単位の記憶は一度失われ、各エージェントの既定値から始まる。
- プロビジョナー経由のセッションは能力プローブができないため、`initial_config`
  を送らなくなる。モデルはエージェント既定値になる。

## 4. テスト

- `ui/src/lib/session-prefs.test.ts`:
  - プロジェクト + エージェント単位の round-trip。
  - エージェント / プロジェクトをまたいだ分離。
  - コロンを含む ID 同士のキー衝突がないこと。
  - エージェント ID が空のときは保存しないこと。
  - 旧プロジェクト単位キーを読まないこと。
- `pnpm --dir ui run check` / `pnpm --dir ui run lint` / `pnpm --dir ui run test`。
