//! FlexAgent エージェントドライバクレート (`fxg-acp`)
//!
//! 設計: `docs/04-agent-drivers-and-windows.md`
//!
//! - [`driver`]: [`AgentDriver`] / [`ActiveSessionHandle`] トレイト ——
//!   独自プロトコルのエージェントを `fxg-node` 本体の変更なしに追加するための
//!   抽象化
//! - [`registry`]: ACP Registry (`registry.json`) の取得・キャッシュ・起動解決と
//!   導入管理 (`fxg agents ...`)
//! - [`acp`][]: `AcpDriver` (`agent-client-protocol` による標準ACPエージェント制御)
//! - [`opencode2`][]: `OpenCode2Driver` (`opencode serve` ブリッジ + 純正TUI Attach)

pub mod acp;
pub mod driver;
pub mod opencode2;
pub mod registry;
mod warm;

pub use acp::AcpDriver;
pub use driver::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent, NativeAttachInfo,
    NativeResumeUnavailable, ResumeRequest, StartSessionRequest, StartedSession,
};
pub use opencode2::{OpenCode2Driver, ensure_opencode_v2};
