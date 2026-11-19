# CLI 出力のテーマ層と表組み

- **日付**: 2026-11-19
- **対象パッケージ**: `fxg-cli`
- **対象スクリプト**: `cargo fmt`, `cargo clippy -p fxg-cli --all-targets`,
  `cargo test -p fxg-cli`

## 背景

CLI の出力は `crates/fxg-cli/src/output.rs` の `KvList` / `Banner` と、
`client.rs` の手書き `print_table` で組まれていた。色は呼び出し側が
`Color::Cyan.bold()` のように直接指定し、`use_color: bool` を全呼び出しへ
引き回していた。その結果:

- `session show` / `doctor` / `web` は `render(false)` のベタ書きで色が出ていなかった。
- 同じ「警告」でも呼び出しごとに色が異なり、共通のスタイルがなかった。
- 表は `chars().count()` で桁を計算していたため CJK や絵文字で列がずれ、
  端末幅への折り返しもできなかった。

## 変更

- `output.rs` を `output/{mod,theme,table}.rs` に分割した。
- `Role`（`Heading` / `Key` / `Muted` / `Accent` / `AccentAlt` / `Success` /
  `Warning` / `Error`）と `Theme` を導入し、コマンドは色名ではなく役割を指定する。
  `Theme::plain` は全役割を空スタイルに解決するため、色なし出力にエスケープが
  残らない。
- `nu-ansi-term` を `anstyle` + `anstream` に置き換えた。TTY 判定、パイプ時の
  ANSI 除去、色深度の降格は `anstream` が行う。
- `--no-color` / `log.no_color` / `NO_COLOR` は `main` で `output::init` に
  1 回だけ渡し、出力関数から `use_color` 引数を削除した。
- 手書きの `print_table` を `comfy-table` ベースの `Table` に置き換えた
  （`session list`、`session show` の承認一覧）。
- 一覧のステータス、`doctor` の合否、見出しに役割を割り当てた。
- `--json` の真偽値判定を `OutputFormat` に統一した。

## 理由

- 色の決定を 1 箇所に集約しないと、コマンドを追加するたびにスタイルが増え、
  色の付け忘れ（`render(false)`）が再発する。
- ANSI を後から剥がす方式より、そもそも生成しない方式の方が、テスト・
  パイプ・リダイレクトのすべてで同じ経路を通る。
- 表の桁計算は文字幅の知識が必要で、自前実装を維持する理由がない。

## 影響

- パイプ・リダイレクト時の出力は従来どおり ANSI なし。
- CJK・絵文字を含むセルでも列がずれず、狭い端末では折り返す。
- 配色は 16 色の名前付き色に統一し、端末のパレットに追従する（UI の Desk
  トークンとは語彙のみ共有する）。
- 表はヘッダー下に罫線を 1 本引く。行末の余白はトリムする。
