# フォーマッタの dprint → oxfmt 統一

- **日付**: 2026-11-08
- **対象パッケージ**: ワークスペース全体 (実装コードは `ui` の devDependency のみ)
- **対象スクリプト / 設定**: `.oxfmtrc.json` (新規), `dprint.json` (削除),
  `ui/.oxfmtrc.json` (削除), `mise.toml`, `ui/package.json`,
  `.github/workflows/ci.yml`, `.vscode/settings.json`,
  `.vscode/extensions.json`, `AGENTS.md`, `tombi.toml`

## 概要

Markdown 用に使っていた dprint を廃止し、UI (TS / Svelte) で既に使っていた
**oxfmt (oxc)** に一本化した。あわせて整形対象を Markdown / JSON / JSONC /
YAML / HTML へ広げ、これまで**どのツールでも整形されていなかった
`.svelte` (132 ファイル)** を整形対象に加えた。

| 対象                                | ツール  | 設定                       |
| :---------------------------------- | :------ | :------------------------- |
| Rust                                | rustfmt | `cargo fmt --all`          |
| TOML                                | tombi   | `tombi.toml`               |
| Markdown / JSON / YAML / HTML / CSS | oxfmt   | `.oxfmtrc.json`            |
| JS / TS / Svelte                    | oxfmt   | `.oxfmtrc.json` (`svelte`) |

タスクは `mise run fmt` / `mise run fmt:check`
のまま (内訳は `fmt:rust` / `fmt:toml` / `fmt:oxc`)。

## 設計判断

- **TOML は tombi を継続**: oxfmt の TOML 整形は配列を 1 行に潰し `printWidth`
  も超える (`mise.toml` の `run = [...]` が 100 文字超の 1 行になる)。
  また tombi は JSON Schema による lint (`mise run lint:toml`)
  も担っているため、TOML は oxfmt の対象外 (`ignorePatterns: ["**/*.toml"]`)。
- **oxfmt は `ui` の devDependency に置く**: `.svelte`
  の整形は oxfmt が同梱する `prettier-plugin-svelte` が `svelte`
  パッケージを **oxfmt 自身のインストール先から解決**するため、`svelte`
  と同じ `node_modules` (=`ui`) に置く必要がある。そのためリポジトリルートから
  `node ui/node_modules/oxfmt/bin/oxfmt` として起動する (シェル非依存。
  `ui/node_modules/.bin` のシェルスクリプトは Windows で動かない)。
- **`embeddedLanguageFormatting` は Markdown のみ `off`**: oxfmt 0.77 は
  `"off"` のとき `prettier-plugin-svelte` が壊れて `.svelte` の整形が失敗する
  (upstream の既知の制約)。`overrides` で `**/*.md` だけ `"off"`
  にし、Markdown のコードフェンス (特に TOML 例) は tombi
  の整形結果と食い違わないよう現状維持する。
- **`printWidth: 100`**: 既存の TS は oxfmt 既定の 100 で整形済みで、rustfmt
  の既定とも揃う。Markdown は `proseWrap: "preserve"`
  (手動折り返しを保持) のため行幅の影響を受けない。
- **`ignorePatterns`**: `target/` / `.sqlx/` / `workspace/` / `node_modules` /
  `ui/build` / `ui/.svelte-kit` / `ui/src/lib/generated` (ts-rs 生成物) /
  lockfile / `*.toml`。`ui/src/lib/generated` は `mise run types`
  の生成物なので整形しない。

## 主な差分

- **`.svelte` 132 ファイル**: これまで未整形だったため初回のみ大きな差分
  (タブインデントの是正、クォートの TS 側 (`"`) への統一など)。
  以降は CI (`mise run fmt:check`) で固定される。
- Markdown はほぼ差分なし (既存 25 ファイル中 1 ファイル、リスト前の空行 1 行)。
- `.vscode/extensions.json` の推奨拡張を `dprint.dprint` → `oxc.oxc-vscode`
  に差し替え。oxc 拡張はワークスペース配下の `package.json`
  を走査して `ui/node_modules/.bin/oxfmt` を自動検出するため、追加設定は不要。
- CI の `fmt` ジョブに `mise run ui:install` を追加 (oxfmt が `ui`
  の依存のため)。`check:ui` は `lint:ui` + `test:ui`
  に変更し、フォーマット検査はリポジトリ全体を見る `fmt:check` に集約した。

## 検証

```sh
mise run fmt            # rustfmt / tombi / oxfmt
mise run fmt:check
pnpm --dir ui run check # svelte-check (整形後も型エラーが出ないこと)
pnpm --dir ui run lint
pnpm --dir ui run test
```
