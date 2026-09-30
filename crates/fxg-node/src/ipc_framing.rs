//! ローカルIPC のフレーミング (Length-prefixed JSON)。
//!
//! サーバー (`daemon::ipc`) とクライアント ([`crate::ipc_client::IpcClient`]) の
//! 双方から使用する: **4バイトのリトルエンディアン長さヘッダ + JSON ペイロード**。
//!
//! 設計: `docs/03-protocol-and-api.md` §4。

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::NodeError;

/// 受信フレームの最大長 (過大なフレームによるメモリ枯渇を防ぐ)。
pub(crate) const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Length-prefixed JSON フレームを1つ読み込む。
///
/// 相手が正常に切断した場合 (EOF) は `Ok(None)` を返す。
pub(crate) async fn read_frame<S>(stream: &mut S) -> Result<Option<Vec<u8>>, NodeError>
where
    S: AsyncRead + Unpin,
{
    let mut length_buf = [0u8; 4];
    match stream.read_exact(&mut length_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => {
            return Err(NodeError::Server(format!("ipc length read failed: {err}")));
        }
    }
    let length = u32::from_le_bytes(length_buf);
    if length > MAX_FRAME_BYTES {
        return Err(NodeError::Server(format!("ipc frame too large: {length}")));
    }
    let mut payload = vec![0u8; length as usize];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|err| NodeError::Server(format!("ipc payload read failed: {err}")))?;
    Ok(Some(payload))
}

/// 任意のシリアライズ可能なメッセージを Length-prefixed JSON フレームとして書き込む。
pub(crate) async fn write_message<S, M>(stream: &mut S, message: &M) -> Result<(), NodeError>
where
    S: AsyncWrite + Unpin,
    M: Serialize,
{
    let payload = serde_json::to_vec(message)?;
    let length = u32::try_from(payload.len())
        .map_err(|_| NodeError::Server("ipc frame too large".to_owned()))?;
    stream
        .write_all(&length.to_le_bytes())
        .await
        .map_err(|err| NodeError::Server(format!("ipc write failed: {err}")))?;
    stream
        .write_all(&payload)
        .await
        .map_err(|err| NodeError::Server(format!("ipc write failed: {err}")))?;
    stream
        .flush()
        .await
        .map_err(|err| NodeError::Server(format!("ipc flush failed: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrips_over_duplex_stream() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let message = serde_json::json!({ "op": "ping", "value": 42 });
        write_message(&mut a, &message).await.expect("write");
        drop(a); // EOF を伝える

        let frame = read_frame(&mut b)
            .await
            .expect("read")
            .expect("frame present");
        let decoded: serde_json::Value = serde_json::from_slice(&frame).expect("decode");
        assert_eq!(decoded, message);

        assert!(read_frame(&mut b).await.expect("eof").is_none());
    }

    #[tokio::test]
    async fn rejects_oversized_frame_length() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&(MAX_FRAME_BYTES + 1).to_le_bytes())
            .await
            .expect("write length");
        let err = read_frame(&mut b).await.expect_err("must fail");
        assert!(matches!(err, NodeError::Server(_)));
    }
}
