//! `BedrockInvoke` — `Backend` implementation that talks the AWS
//! Bedrock-runtime `InvokeModelWithResponseStream` wire to Anthropic
//! Claude models.
//!
//! See [`super`](crate::bedrock_invoke) for the high-level shape and
//! the `BackendCapabilities` advertisement.

use super::body::{AnthropicEvent, BodyError, StreamAccumulator, request_body};
use super::eventstream::{ChunkPayload, EventStreamDecoder, FrameError};
use super::sigv4::{AwsCredentials, SignRequest, sign_request};
use crate::backend::{
    AcceleratorInfo, Backend, BackendCapabilities, GenerateError, TokenEventV2, TokenStream,
    TokenStreamV2,
};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::StreamExt;
use inferd_proto::Resolved;
use inferd_proto::v2::ResolvedV2;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

/// Auth shape for the Bedrock backend.
#[derive(Debug, Clone)]
pub enum BedrockAuth {
    /// Pre-issued bearer token (`AWS_BEARER_TOKEN_BEDROCK` shape, AWS
    /// rolled this out in 2025-06). Sent as `Authorization: Bearer <token>`,
    /// no SigV4 signing.
    BearerToken(String),
    /// SigV4 signing using the supplied access-key chain. Computed
    /// per-request against the configured region.
    Sigv4 {
        /// AWS access key id (`AWS_ACCESS_KEY_ID`).
        access_key_id: String,
        /// AWS secret access key (`AWS_SECRET_ACCESS_KEY`).
        secret_access_key: String,
        /// Optional STS session token (`AWS_SESSION_TOKEN`). Sent as
        /// the `x-amz-security-token` header when present.
        session_token: Option<String>,
    },
}

/// Configuration for `BedrockInvoke`.
#[derive(Debug, Clone)]
pub struct BedrockInvokeConfig {
    /// AWS region the Bedrock endpoint lives in, e.g. `"us-east-1"`,
    /// `"eu-central-1"`. Used for both the endpoint host and SigV4
    /// signing scope.
    pub region: String,
    /// Bedrock model id, e.g.
    /// `"anthropic.claude-3-5-sonnet-20241022-v2:0"`. Goes into the
    /// URL path; URL-encoded by the adapter.
    pub model_id: String,
    /// How to authenticate.
    pub auth: BedrockAuth,
    /// Total request timeout. Default 5 minutes.
    pub timeout: Duration,
    /// Override the endpoint host. Empty/absent → default
    /// `bedrock-runtime.<region>.amazonaws.com`. Useful for VPC
    /// endpoint integration testing.
    pub endpoint_override: Option<String>,
}

impl Default for BedrockInvokeConfig {
    fn default() -> Self {
        Self {
            region: "us-east-1".into(),
            model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0".into(),
            auth: BedrockAuth::BearerToken(String::new()),
            timeout: Duration::from_secs(300),
            endpoint_override: None,
        }
    }
}

/// Errors specific to the `BedrockInvoke` adapter.
#[derive(Debug, thiserror::Error)]
pub enum BedrockInvokeError {
    /// reqwest-level transport error.
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    /// Upstream returned a non-2xx HTTP status before the event-stream
    /// opened.
    #[error("upstream HTTP {status}: {body}")]
    HttpStatus {
        /// HTTP status code from the upstream.
        status: u16,
        /// Response body (truncated to the first 4 KB).
        body: String,
    },
    /// Body mapper rejected the request.
    #[error("request mapping: {0}")]
    Body(#[from] BodyError),
    /// Configuration issue surfaced at request time (empty model id,
    /// missing auth credentials, …).
    #[error("misconfiguration: {0}")]
    Config(String),
}

impl From<BedrockInvokeError> for GenerateError {
    fn from(e: BedrockInvokeError) -> Self {
        match e {
            BedrockInvokeError::Body(BodyError::AttachmentUnsupported(_))
            | BedrockInvokeError::Body(BodyError::UnknownContentBlock)
            | BedrockInvokeError::Body(BodyError::NonTextToolResult) => {
                GenerateError::InvalidRequest(e.to_string())
            }
            BedrockInvokeError::Config(_) => GenerateError::InvalidRequest(e.to_string()),
            _ => GenerateError::Unavailable(e.to_string()),
        }
    }
}

/// `Backend` adapter for AWS Bedrock-runtime InvokeModelWithResponseStream
/// against Anthropic Claude models.
#[derive(Debug)]
pub struct BedrockInvoke {
    name: &'static str,
    config: BedrockInvokeConfig,
    client: reqwest::Client,
}

impl BedrockInvoke {
    /// Construct a new adapter. Builds the `reqwest::Client` with
    /// rustls + the configured timeout.
    pub fn new(config: BedrockInvokeConfig) -> Result<Self, BedrockInvokeError> {
        if config.region.trim().is_empty() {
            return Err(BedrockInvokeError::Config(
                "region must not be empty".into(),
            ));
        }
        if config.model_id.trim().is_empty() {
            return Err(BedrockInvokeError::Config(
                "model_id must not be empty".into(),
            ));
        }
        let client = reqwest::Client::builder().timeout(config.timeout).build()?;
        Ok(Self {
            name: "bedrock-invoke",
            config,
            client,
        })
    }

    fn host(&self) -> String {
        if let Some(h) = self.config.endpoint_override.as_deref()
            && !h.is_empty()
        {
            return h.to_string();
        }
        format!("bedrock-runtime.{}.amazonaws.com", self.config.region)
    }

    fn path(&self) -> String {
        // AWS URL-encodes `:` in the model id as `%3A`. Other chars in
        // valid model ids (`a-zA-Z0-9.-_`) are safe as-is.
        let encoded = self.config.model_id.replace(':', "%3A");
        format!("/model/{encoded}/invoke-with-response-stream")
    }

    fn build_request(&self, body_bytes: Vec<u8>) -> Result<reqwest::Request, BedrockInvokeError> {
        let host = self.host();
        let path = self.path();
        let url = format!("https://{host}{path}");

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // AWS event-stream is the only response shape Bedrock emits
        // for InvokeModelWithResponseStream; declaring it tightens up
        // intermediary handling.
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("application/vnd.amazon.eventstream"),
        );

        match &self.config.auth {
            BedrockAuth::BearerToken(token) => {
                if token.is_empty() {
                    return Err(BedrockInvokeError::Config(
                        "BearerToken auth requires a non-empty token (set AWS_BEARER_TOKEN_BEDROCK)"
                            .into(),
                    ));
                }
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                        BedrockInvokeError::Config(
                            "bearer token contains invalid header bytes".into(),
                        )
                    })?,
                );
            }
            BedrockAuth::Sigv4 {
                access_key_id,
                secret_access_key,
                session_token,
            } => {
                if access_key_id.is_empty() || secret_access_key.is_empty() {
                    return Err(BedrockInvokeError::Config(
                        "SigV4 auth requires AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY".into(),
                    ));
                }
                let (amz_date, date_stamp) = now_amz_dates();
                let creds = AwsCredentials {
                    access_key_id: access_key_id.clone(),
                    secret_access_key: secret_access_key.clone(),
                    session_token: session_token.clone(),
                };
                let signed = sign_request(
                    &SignRequest {
                        method: "POST",
                        host: &host,
                        path: &path,
                        query: "",
                        region: &self.config.region,
                        service: "bedrock",
                        body: &body_bytes,
                        amz_date: &amz_date,
                        date_stamp: &date_stamp,
                    },
                    &creds,
                );
                headers.insert(
                    HeaderName::from_static("host"),
                    HeaderValue::from_str(&signed.host).map_err(|_| {
                        BedrockInvokeError::Config("host contains invalid header bytes".into())
                    })?,
                );
                headers.insert(
                    HeaderName::from_static("x-amz-date"),
                    HeaderValue::from_str(&signed.x_amz_date).map_err(|_| {
                        BedrockInvokeError::Config("amz-date contains invalid header bytes".into())
                    })?,
                );
                headers.insert(
                    HeaderName::from_static("x-amz-content-sha256"),
                    HeaderValue::from_str(&signed.x_amz_content_sha256)
                        .map_err(|_| BedrockInvokeError::Config("payload hash invalid".into()))?,
                );
                if let Some(token) = signed.x_amz_security_token {
                    headers.insert(
                        HeaderName::from_static("x-amz-security-token"),
                        HeaderValue::from_str(&token).map_err(|_| {
                            BedrockInvokeError::Config(
                                "session token contains invalid header bytes".into(),
                            )
                        })?,
                    );
                }
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&signed.authorization)
                        .map_err(|_| BedrockInvokeError::Config("authorization invalid".into()))?,
                );
            }
        }

        let mut req = self.client.post(&url).headers(headers).body(body_bytes);
        // reqwest's high-level Client uses the URL's host, but our
        // SigV4 signing canonicalises `host` from the configured
        // string — they should always match. Keep them in sync.
        let _ = &mut req;
        req.build().map_err(BedrockInvokeError::from)
    }
}

#[async_trait]
impl Backend for BedrockInvoke {
    fn name(&self) -> &str {
        self.name
    }

    fn ready(&self) -> bool {
        true
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            v2: true,
            tools: true,
            // Multimodal off in v0.2.0 (anthropic body supports image
            // inputs but mapping is deferred). Thinking off — see
            // module-level docs.
            vision: false,
            audio: false,
            video: false,
            thinking: false,
            embed: false,
            accelerator: AcceleratorInfo::default(),
        }
    }

    /// v1 path: not supported. Bedrock-invoke only ships v2 in v0.2.0.
    async fn generate(&self, _req: Resolved) -> Result<TokenStream, GenerateError> {
        Err(GenerateError::Internal(
            "bedrock-invoke backend supports v2 only; use the v2 socket".into(),
        ))
    }

    async fn generate_v2(&self, req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        let body = request_body(&req).map_err(BedrockInvokeError::from)?;
        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| BedrockInvokeError::Config(e.to_string()))?;

        let request = self.build_request(body_bytes)?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(BedrockInvokeError::from)?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".into());
            let truncated = if body.len() > 4096 {
                body[..4096].to_string()
            } else {
                body
            };
            return Err(BedrockInvokeError::HttpStatus {
                status,
                body: truncated,
            }
            .into());
        }

        let (tx, rx) = mpsc::channel(8);
        let byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            drive_event_stream(byte_stream, tx).await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn stop(&self, _timeout: Duration) -> Result<(), GenerateError> {
        Ok(())
    }
}

/// Drive the AWS event-stream end-to-end: decode binary frames,
/// extract chunk payloads, decode their base64 inner JSON, feed the
/// `StreamAccumulator`, push events out the channel.
async fn drive_event_stream<S>(mut byte_stream: S, tx: mpsc::Sender<TokenEventV2>)
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    let mut decoder = EventStreamDecoder::new();
    let mut acc = StreamAccumulator::new();

    while let Some(chunk) = byte_stream.next().await {
        let chunk = match chunk {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "bedrock-invoke transport error");
                return;
            }
        };
        decoder.feed(&chunk);

        loop {
            let frame = match decoder.next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(FrameError::TooLarge(n)) => {
                    warn!(bytes = n, "bedrock-invoke frame too large; aborting stream");
                    return;
                }
                Err(e) => {
                    warn!(error = %e, "bedrock-invoke malformed frame; aborting stream");
                    return;
                }
            };

            match frame.event_type.as_str() {
                "chunk" => {
                    let payload: ChunkPayload = match serde_json::from_slice(&frame.payload) {
                        Ok(p) => p,
                        Err(e) => {
                            warn!(error = %e, "bedrock-invoke malformed chunk payload");
                            continue;
                        }
                    };
                    let inner = match BASE64_STANDARD.decode(&payload.bytes) {
                        Ok(b) => b,
                        Err(e) => {
                            warn!(error = %e, "bedrock-invoke chunk base64 decode failed");
                            continue;
                        }
                    };
                    let event: AnthropicEvent = match serde_json::from_slice(&inner) {
                        Ok(e) => e,
                        Err(e) => {
                            warn!(error = %e, "bedrock-invoke inner event parse failed");
                            continue;
                        }
                    };
                    for ev in acc.ingest(event) {
                        if tx.send(ev).await.is_err() {
                            debug!("bedrock-invoke generation cancelled (receiver dropped)");
                            return;
                        }
                    }
                }
                "error" => {
                    warn!(
                        exception = ?frame.exception_type,
                        payload = String::from_utf8_lossy(&frame.payload).as_ref(),
                        "bedrock-invoke upstream emitted error frame"
                    );
                    // Bail without finalize() — leaves the stream
                    // terminated without a Done frame, which the
                    // daemon translates per ADR 0007.
                    return;
                }
                other => {
                    debug!(
                        event_type = other,
                        "bedrock-invoke skipping unknown event type"
                    );
                }
            }
        }
    }

    for ev in acc.finalize() {
        if tx.send(ev).await.is_err() {
            return;
        }
    }
}

/// Compute `(YYYYMMDDTHHMMSSZ, YYYYMMDD)` from the system clock in
/// UTC. Hand-rolled because we don't otherwise pull in `chrono` /
/// `time` and the format is tiny.
fn now_amz_dates() -> (String, String) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_amz_dates(secs)
}

fn format_amz_dates(unix_secs: u64) -> (String, String) {
    // Convert to UTC YMDHMS without a date crate.
    // Algorithm: compute days since 1970-01-01, then convert.
    let days = (unix_secs / 86_400) as i64;
    let secs_today = unix_secs % 86_400;
    let h = secs_today / 3_600;
    let m = (secs_today % 3_600) / 60;
    let s = secs_today % 60;

    // Days → (year, month, day) using Howard Hinnant's algorithm
    // (civil_from_days), public-domain.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_civil = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m_civil <= 2 { y + 1 } else { y };

    let amz_date = format!(
        "{year:04}{m:02}{d:02}T{h:02}{min:02}{s:02}Z",
        year = year,
        m = m_civil,
        d = d,
        h = h,
        min = m,
        s = s,
    );
    let date_stamp = format!("{year:04}{m:02}{d:02}", year = year, m = m_civil, d = d);
    (amz_date, date_stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_formatter_known_unix_timestamps() {
        // 2026-05-21 12:00:00 UTC = 1779364800
        let (amz, date) = format_amz_dates(1_779_364_800);
        assert_eq!(amz, "20260521T120000Z");
        assert_eq!(date, "20260521");
    }

    #[test]
    fn date_formatter_handles_unix_epoch() {
        let (amz, date) = format_amz_dates(0);
        assert_eq!(amz, "19700101T000000Z");
        assert_eq!(date, "19700101");
    }

    #[test]
    fn date_formatter_handles_leap_year_feb_29() {
        // 2024-02-29 00:00:00 UTC = 1709164800
        let (amz, date) = format_amz_dates(1_709_164_800);
        assert_eq!(amz, "20240229T000000Z");
        assert_eq!(date, "20240229");
    }

    #[test]
    fn host_uses_default_endpoint_when_override_empty() {
        let cfg = BedrockInvokeConfig {
            region: "us-west-2".into(),
            model_id: "anthropic.claude-3-haiku-20240307-v1:0".into(),
            auth: BedrockAuth::BearerToken("t".into()),
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let backend = BedrockInvoke::new(cfg).unwrap();
        assert_eq!(backend.host(), "bedrock-runtime.us-west-2.amazonaws.com");
    }

    #[test]
    fn host_respects_endpoint_override() {
        let cfg = BedrockInvokeConfig {
            region: "us-east-1".into(),
            model_id: "x".into(),
            auth: BedrockAuth::BearerToken("t".into()),
            timeout: Duration::from_secs(30),
            endpoint_override: Some("vpce-1234.bedrock.us-east-1.vpce.amazonaws.com".into()),
        };
        let backend = BedrockInvoke::new(cfg).unwrap();
        assert_eq!(
            backend.host(),
            "vpce-1234.bedrock.us-east-1.vpce.amazonaws.com"
        );
    }

    #[test]
    fn path_url_encodes_colon_in_model_id() {
        let cfg = BedrockInvokeConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0".into(),
            auth: BedrockAuth::BearerToken("t".into()),
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let backend = BedrockInvoke::new(cfg).unwrap();
        assert_eq!(
            backend.path(),
            "/model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/invoke-with-response-stream"
        );
    }

    #[test]
    fn rejects_empty_region() {
        let cfg = BedrockInvokeConfig {
            region: "".into(),
            model_id: "x".into(),
            auth: BedrockAuth::BearerToken("t".into()),
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let err = BedrockInvoke::new(cfg).unwrap_err();
        assert!(matches!(err, BedrockInvokeError::Config(_)));
    }

    #[test]
    fn rejects_empty_model_id() {
        let cfg = BedrockInvokeConfig {
            region: "us-east-1".into(),
            model_id: "".into(),
            auth: BedrockAuth::BearerToken("t".into()),
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let err = BedrockInvoke::new(cfg).unwrap_err();
        assert!(matches!(err, BedrockInvokeError::Config(_)));
    }

    #[test]
    fn build_request_fails_for_empty_bearer_token() {
        let cfg = BedrockInvokeConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude".into(),
            auth: BedrockAuth::BearerToken(String::new()),
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let backend = BedrockInvoke::new(cfg).unwrap();
        let err = backend.build_request(b"{}".to_vec()).unwrap_err();
        assert!(matches!(err, BedrockInvokeError::Config(_)));
    }

    #[test]
    fn build_request_fails_for_empty_sigv4_keys() {
        let cfg = BedrockInvokeConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude".into(),
            auth: BedrockAuth::Sigv4 {
                access_key_id: String::new(),
                secret_access_key: "x".into(),
                session_token: None,
            },
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let backend = BedrockInvoke::new(cfg).unwrap();
        let err = backend.build_request(b"{}".to_vec()).unwrap_err();
        assert!(matches!(err, BedrockInvokeError::Config(_)));
    }

    #[test]
    fn build_request_with_bearer_attaches_authorization_header() {
        let cfg = BedrockInvokeConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude".into(),
            auth: BedrockAuth::BearerToken("ABCD".into()),
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let backend = BedrockInvoke::new(cfg).unwrap();
        let req = backend.build_request(b"{}".to_vec()).unwrap();
        let auth = req.headers().get(AUTHORIZATION).unwrap();
        assert_eq!(auth.to_str().unwrap(), "Bearer ABCD");
    }

    #[test]
    fn build_request_with_sigv4_attaches_aws_headers() {
        let cfg = BedrockInvokeConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude".into(),
            auth: BedrockAuth::Sigv4 {
                access_key_id: "AKID".into(),
                secret_access_key: "SECRET".into(),
                session_token: Some("TOKEN".into()),
            },
            timeout: Duration::from_secs(30),
            endpoint_override: None,
        };
        let backend = BedrockInvoke::new(cfg).unwrap();
        let req = backend.build_request(b"{}".to_vec()).unwrap();
        assert!(req.headers().contains_key("x-amz-date"));
        assert!(req.headers().contains_key("x-amz-content-sha256"));
        assert_eq!(
            req.headers()
                .get("x-amz-security-token")
                .and_then(|v| v.to_str().ok()),
            Some("TOKEN")
        );
        let auth = req.headers().get(AUTHORIZATION).unwrap().to_str().unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 "));
        assert!(auth.contains("Credential=AKID/"));
        assert!(auth.contains("/us-east-1/bedrock/aws4_request"));
    }
}
