//! FlexAgent ノードデーモンクレート (`fxg-node`)
//!
//! 常駐ノード (`fxg daemon`) の中核実装。ローカルIPCサーバー (Named Pipe / UDS)、
//! ローカル HTTP/WS サーバー (`127.0.0.1:7860`)、Git Worktree 管理、Shadow Git Tree
//! スナップショット、Outbox 同期ワーカーを含む。
//!
//! **Phase 2 以降で実装予定** (現時点ではプレースホルダ)。
