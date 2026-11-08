# Auto-generated TypeScript type definitions (generated files are committed)

`bindings.ts` is generated from the `fxg-protocol` types by
[specta](https://github.com/specta-rs/specta). Do not edit it by hand.

- Generate: `cargo test -p fxg-protocol export_bindings` (or `mise run types`)
- Source: `crates/fxg-protocol/src/export_bindings.rs` (types annotated with
  `#[specta(collect)]` and the types they reference)
- Import: `import type { SessionSummary } from "#lib/generated/bindings";`
- Output is a single file (`Layout::FlatFile`), emitted stably in ascending type
  name order.

Whenever you change a Rust-side type, regenerate and commit the generated diff
(see the development rules in `docs/README.md` / `AGENTS.md`).
