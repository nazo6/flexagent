//! FlexAgent CLI エントリポイント (`fxg` バイナリ)
//!
//! CLI パースには `usage-rs` (`usage` クレート) を使用し、
//! `#[derive(Cli)]` / `Args` / `Subcommands` + `Run` / `RunWith` でサブコマンドを
//! 定義する (Phase 2 以降で実装)。

fn main() {
    eprintln!(
        "fxg: コマンドは未実装です (Phase 1 ではワークスペースとプロトコル/DB層のみ実装)。\n\
         詳細は docs/changelog/2026-09-30-implementation-plan.md を参照してください。"
    );
    std::process::exit(1);
}
