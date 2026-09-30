//! CLI (`fxg`) → Daemon のローカルIPCクライアント。
//!
//! デーモン (`fxg daemon`) のIPCエンドポイントへ接続し、
//! Length-prefixed JSON で1リクエスト/1レスポンスをやり取りする。
//!
//! 設計: `docs/03-protocol-and-api.md` §4。

use fxg_protocol::ipc::{IpcClientMessage, IpcServerMessage};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::NodeError;
use crate::ipc_framing::{read_frame, write_message};

/// ローカルIPCの双方向ストリーム (Unix Domain Socket / Named Pipe)。
trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

/// ローカルIPCクライアント。
pub struct IpcClient {
    stream: Box<dyn AsyncStream>,
}

impl std::fmt::Debug for IpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcClient").finish_non_exhaustive()
    }
}

impl IpcClient {
    /// エンドポイントへ接続する。
    ///
    /// エンドポイントは [`crate::ipc_endpoint`] で解決する
    /// (Windows: Named Pipe / Unix: Unix Domain Socket)。
    #[cfg(unix)]
    pub async fn connect(endpoint: &str) -> Result<Self, NodeError> {
        let stream = tokio::net::UnixStream::connect(endpoint)
            .await
            .map_err(|err| NodeError::Server(format!("failed to connect to {endpoint}: {err}")))?;
        Ok(Self {
            stream: Box::new(stream),
        })
    }

    /// エンドポイントへ接続する (Windows: Named Pipe)。
    #[cfg(windows)]
    pub async fn connect(endpoint: &str) -> Result<Self, NodeError> {
        use tokio::net::windows::named_pipe::ClientOptions;

        let stream = ClientOptions::new()
            .open(endpoint)
            .map_err(|err| NodeError::Server(format!("failed to connect to {endpoint}: {err}")))?;
        Ok(Self {
            stream: Box::new(stream),
        })
    }

    /// 1メッセージを送信する (レスポンスを待たない)。
    ///
    /// `AttachSession` でイベントストリーミング中の接続へコマンド
    /// (`SendPrompt` 等) を送る場合に [`Self::send`] + [`Self::recv`] を使う。
    pub async fn send(&mut self, message: &IpcClientMessage) -> Result<(), NodeError> {
        write_message(&mut self.stream, message).await
    }

    /// 次のサーバーメッセージを受信する (接続が閉じた場合は `None`)。
    ///
    /// アタッチ中は `EventBatch` / `LiveStreamDelta` とコマンド応答
    /// (`Result` / `Error`) が混在して届くため、`command_id` で相関する。
    pub async fn recv(&mut self) -> Result<Option<IpcServerMessage>, NodeError> {
        let frame = read_frame(&mut self.stream).await?;
        match frame {
            Some(frame) => Ok(Some(serde_json::from_slice(&frame)?)),
            None => Ok(None),
        }
    }

    /// 1リクエストを送信し、次のレスポンスを受信する。
    pub async fn request(
        &mut self,
        message: &IpcClientMessage,
    ) -> Result<IpcServerMessage, NodeError> {
        self.send(message).await?;
        self.recv()
            .await?
            .ok_or_else(|| NodeError::Server("daemon closed the connection".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, NodeDaemon};
    use crate::ipc_framing::write_message;
    use std::time::Duration;

    #[tokio::test]
    async fn exchanges_messages_with_daemon() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "client-node", "Client Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start");

        // サーバーが listen を開始するまで待つ
        let mut client = None;
        for _ in 0..200 {
            match IpcClient::connect(daemon.ipc_endpoint()).await {
                Ok(connected) => {
                    client = Some(connected);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        let mut client = client.expect("client connects");

        let response = client
            .request(&IpcClientMessage::ListSessions {
                command_id: "c1".to_owned(),
                include_stopped: true,
            })
            .await
            .expect("request");
        match response {
            IpcServerMessage::Result {
                result: fxg_protocol::ipc::IpcResult::Sessions { sessions },
                command_id,
            } => {
                assert_eq!(command_id, "c1");
                assert!(sessions.is_empty());
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // デーモン側から接続が切れた場合はエラーになる
        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
        let err = client
            .request(&IpcClientMessage::Ping)
            .await
            .expect_err("connection closed");
        assert!(matches!(err, NodeError::Server(_)));
    }

    #[tokio::test]
    async fn reports_connect_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = dir
            .path()
            .join("missing.sock")
            .to_string_lossy()
            .into_owned();
        let err = IpcClient::connect(&endpoint).await.expect_err("must fail");
        assert!(matches!(err, NodeError::Server(_)));
    }

    #[tokio::test]
    async fn framing_helpers_are_shared_with_server() {
        // サーバー側のフレーミングと同じ関数をクライアントが使っていることを確認する
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_message(&mut a, &IpcClientMessage::Ping)
            .await
            .expect("write");
        let frame = read_frame(&mut b).await.expect("read").expect("frame");
        let decoded: IpcClientMessage = serde_json::from_slice(&frame).expect("decode");
        assert_eq!(decoded, IpcClientMessage::Ping);
    }
}
