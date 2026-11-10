//! WebSocket upgrade endpoints.

use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;

use crate::api::{ClientApiBackend, ClientApiState, pty, ws};

// ----------------------------------------------------------------------
// WebSocket upgrades
// ----------------------------------------------------------------------

pub(crate) async fn client_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.on_upgrade(move |socket| ws::handle_client_ws(backend, socket))
}

pub(crate) async fn pty_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.on_upgrade(move |socket| pty::handle_pty_ws(backend, socket))
}
