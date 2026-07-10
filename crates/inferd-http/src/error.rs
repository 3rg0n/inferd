//! HTTP error mapping: inferd/daemon failures → OpenAI error envelope +
//! the right HTTP status, so OpenAI-SDK clients react correctly (e.g.
//! back off on 429).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use inferd_openai_wire::{ErrorBody, ErrorEnvelope};
use inferd_proto::v2::ErrorCodeV2;

/// A translated error ready to become an HTTP response.
pub struct HttpError {
    pub status: StatusCode,
    pub kind: &'static str,
    pub message: String,
    pub code: Option<String>,
}

impl HttpError {
    pub fn new(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
            code: None,
        }
    }

    /// 400 invalid_request_error.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", message)
    }

    /// 502 — the daemon couldn't be reached / dropped.
    pub fn daemon_unreachable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "api_error", message)
    }

    /// Map a terminal inferd `Error` frame's code to an HTTP status +
    /// OpenAI error type.
    pub fn from_inferd(code: ErrorCodeV2, message: String) -> Self {
        let (status, kind) = match code {
            ErrorCodeV2::QueueFull => (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error"),
            ErrorCodeV2::BackendUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "api_error"),
            ErrorCodeV2::InvalidRequest => (StatusCode::BAD_REQUEST, "invalid_request_error"),
            ErrorCodeV2::FrameTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "invalid_request_error"),
            ErrorCodeV2::AttachmentUnsupported => {
                (StatusCode::BAD_REQUEST, "invalid_request_error")
            }
            ErrorCodeV2::ToolCallMalformed => (StatusCode::BAD_GATEWAY, "api_error"),
            ErrorCodeV2::WireVersionUnsupported => (StatusCode::BAD_GATEWAY, "api_error"),
            ErrorCodeV2::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "api_error"),
        };
        let mut e = Self::new(status, kind, message);
        e.code = Some(format!("{code:?}"));
        e
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = ErrorEnvelope {
            error: ErrorBody {
                message: self.message,
                kind: self.kind.to_string(),
                code: self.code,
            },
        };
        (self.status, Json(body)).into_response()
    }
}
