//! WebSocket upgrade endpoints.

use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;

use crate::{ClientApiBackend, ClientApiState, pty, ws};

/// Client WS messages may carry inline attachments (base64 images).
const CLIENT_WS_MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// PTY input is keystrokes / paste text; keep the frame small.
const PTY_WS_MAX_MESSAGE: usize = 1024 * 1024;

// ----------------------------------------------------------------------
// WebSocket upgrades
// ----------------------------------------------------------------------

pub(crate) async fn client_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.max_message_size(CLIENT_WS_MAX_MESSAGE)
        .max_frame_size(CLIENT_WS_MAX_MESSAGE)
        .on_upgrade(move |socket| ws::handle_client_ws(backend, socket))
}

pub(crate) async fn pty_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.max_message_size(PTY_WS_MAX_MESSAGE)
        .max_frame_size(PTY_WS_MAX_MESSAGE)
        .on_upgrade(move |socket| pty::handle_pty_ws(backend, socket))
}
