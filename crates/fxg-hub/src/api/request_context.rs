//! Request correlation ID middleware.
//!
//! Every REST request is assigned a correlation ID that is:
//!
//! - echoed back in the `x-request-id` response header,
//! - attached to error bodies (`ErrorInfo::request_id`),
//! - included in the tracing span of the request so log lines can be grepped
//!   by the same value the user sees.
//!
//! A client-provided `x-request-id` is reused (bounded in length) so the UI or
//! a proxy can correlate its own retries.

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use fxg_protocol::util::uuid_v7;
use tracing::Instrument;

/// Header used to carry the request correlation ID.
pub(crate) const REQUEST_ID_HEADER: &str = "x-request-id";

/// Maximum accepted length of a client-provided request ID.
const MAX_REQUEST_ID_LEN: usize = 128;

tokio::task_local! {
    /// Correlation ID of the request currently being served.
    static REQUEST_ID: String;
}

/// Returns the correlation ID of the current request, if any.
// `Result::ok` is disallowed by the error-handling policy, so the explicit
// match below is intentional.
#[allow(clippy::manual_ok_err)]
pub(crate) fn current_request_id() -> Option<String> {
    match REQUEST_ID.try_with(Clone::clone) {
        Ok(request_id) => Some(request_id),
        Err(_) => None,
    }
}

/// Assigns a request ID, records the request span, and echoes the ID back.
///
/// This is the outermost layer so that security rejections (which never reach
/// a handler) are correlated as well.
pub(crate) async fn request_context(request: Request, next: Next) -> Response {
    let request_id = match request.headers().get(REQUEST_ID_HEADER) {
        Some(value) => match value.to_str() {
            Ok(value) => value.trim(),
            Err(_) => "",
        },
        None => "",
    };
    let request_id = if request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_LEN {
        uuid_v7()
    } else {
        request_id.to_owned()
    };

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %request.method(),
        path = %request.uri().path(),
    );

    let future = next.run(request).instrument(span);
    let mut response = REQUEST_ID.scope(request_id.clone(), future).await;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt;

    use crate::api::ApiError;
    use fxg_protocol::client_api::ApiErrorResponse;

    async fn fail() -> ApiError {
        ApiError::not_found("missing thing")
    }

    async fn ok() -> &'static str {
        "ok"
    }

    fn router() -> Router {
        Router::new()
            .route("/fail", get(fail))
            .route("/ok", get(ok))
            .layer(middleware::from_fn(request_context))
    }

    async fn send(request: HttpRequest<Body>) -> axum::response::Response {
        router().oneshot(request).await.expect("response")
    }

    fn header<'a>(response: &'a axum::response::Response, name: &str) -> &'a str {
        let value = response.headers().get(name).expect("header");
        match value.to_str() {
            Ok(value) => value,
            Err(_) => panic!("header {name} is not ASCII"),
        }
    }

    #[tokio::test]
    async fn assigns_request_id_to_successful_responses() {
        let response = send(HttpRequest::get("/ok").body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!header(&response, REQUEST_ID_HEADER).is_empty());
    }

    #[tokio::test]
    async fn attaches_request_id_to_error_bodies() {
        let response = send(HttpRequest::get("/fail").body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let header_id = header(&response, REQUEST_ID_HEADER).to_owned();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: ApiErrorResponse = serde_json::from_slice(&bytes).expect("error body");
        assert_eq!(body.error.request_id.as_deref(), Some(header_id.as_str()));
    }

    #[tokio::test]
    async fn reuses_client_provided_request_id() {
        let response = send(
            HttpRequest::get("/ok")
                .header(REQUEST_ID_HEADER, "client-id-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(header(&response, REQUEST_ID_HEADER), "client-id-123");
    }

    #[tokio::test]
    async fn ignores_overlong_client_request_id() {
        let too_long = "a".repeat(MAX_REQUEST_ID_LEN + 1);
        let response = send(
            HttpRequest::get("/ok")
                .header(REQUEST_ID_HEADER, too_long)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_ne!(
            header(&response, REQUEST_ID_HEADER).len(),
            MAX_REQUEST_ID_LEN + 1
        );
    }
}
