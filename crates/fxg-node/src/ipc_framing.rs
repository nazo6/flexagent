//! Local IPC framing (length-prefixed JSON).
//!
//! Used by both the server (`daemon::ipc`) and the client
//! ([`crate::ipc_client::IpcClient`]): **a 4-byte little-endian length header
//! followed by a JSON payload**.
//!
//! Design: `docs/03-protocol-and-api.md` §4.

#![allow(
    clippy::disallowed_types,
    reason = "tests decode raw JSON frames; see docs/06-type-safety.md"
)]

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::NodeError;

/// Maximum length of an incoming frame (prevents memory exhaustion from oversized frames).
pub(crate) const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Reads a single length-prefixed JSON frame.
///
/// Returns `Ok(None)` when the peer closed the connection cleanly (EOF).
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

/// Writes any serializable message as a length-prefixed JSON frame.
pub(crate) async fn write_message<S, M>(stream: &mut S, message: &M) -> Result<(), NodeError>
where
    S: AsyncWrite + Unpin,
    M: Serialize,
{
    let payload = serde_json::to_vec(message)?;
    let length = u32::try_from(payload.len())
        .map_err(|err| NodeError::Server(format!("ipc frame is too large ({err})")))?;
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
        drop(a); // signal EOF

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
