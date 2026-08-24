//! HTTP 传输层抽象
//!
//! 定义统一的 HTTP 客户端接口，供 Provider Adapter 使用。
//! 具体实现由 llm-gateway 提供，避免循环依赖。

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use keycompute_types::Result;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamResponseMeta {
    pub status: u16,
    pub headers_received_at: DateTime<Utc>,
    pub upstream_request_id: Option<String>,
    /// Kept for protocol adapters to select provider-specific correlation headers.
    pub headers: Vec<(String, String)>,
}
impl UpstreamResponseMeta {
    pub fn synthetic_success() -> Self {
        Self {
            status: 200,
            headers_received_at: Utc::now(),
            upstream_request_id: None,
            headers: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct UpstreamResponse<T> {
    pub meta: UpstreamResponseMeta,
    pub body: T,
}
impl<T> UpstreamResponse<T> {
    pub fn map_body<U>(self, map: impl FnOnce(T) -> U) -> UpstreamResponse<U> {
        UpstreamResponse {
            meta: self.meta,
            body: map(self.body),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamFailureKind {
    Transport,
    Timeout,
    HttpStatus,
    BodyRead,
    Protocol,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{stable_error_code}: {sanitized_summary}")]
pub struct UpstreamFailure {
    pub kind: UpstreamFailureKind,
    pub status: Option<u16>,
    pub headers_received_at: Option<DateTime<Utc>>,
    pub upstream_request_id: Option<String>,
    pub retryable: bool,
    pub stable_error_code: String,
    pub sanitized_summary: String,
}

impl UpstreamFailure {
    fn transport(error: &reqwest::Error, request_is_idempotent: bool) -> Self {
        let timeout = error.is_timeout();
        let definitely_pre_dispatch = error.is_connect();
        let ambiguous_after_dispatch = !request_is_idempotent && !definitely_pre_dispatch;
        Self {
            kind: if timeout {
                UpstreamFailureKind::Timeout
            } else {
                UpstreamFailureKind::Transport
            },
            status: None,
            headers_received_at: None,
            upstream_request_id: None,
            retryable: definitely_pre_dispatch || (request_is_idempotent && timeout),
            stable_error_code: if ambiguous_after_dispatch && timeout {
                "upstream_ambiguous_timeout"
            } else if ambiguous_after_dispatch {
                "upstream_ambiguous_transport"
            } else if timeout {
                "upstream_timeout"
            } else {
                "upstream_transport"
            }
            .to_string(),
            sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
        }
    }

    pub fn into_keycompute_error(self) -> keycompute_types::KeyComputeError {
        keycompute_types::KeyComputeError::UpstreamFailure {
            status: self.status,
            stable_code: self.stable_error_code,
            retryable: self.retryable,
            summary: self.sanitized_summary,
        }
    }
}

fn response_meta(response: &reqwest::Response) -> UpstreamResponseMeta {
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect::<Vec<_>>();
    let upstream_request_id = ["x-request-id", "request-id", "x-amzn-requestid"]
        .iter()
        .find_map(|name| response.headers().get(*name))
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(128).collect());
    UpstreamResponseMeta {
        status: response.status().as_u16(),
        headers_received_at: Utc::now(),
        upstream_request_id,
        headers,
    }
}

const MAX_HTTP_FAILURE_INSPECTION_BYTES: usize = 8 * 1024;

fn summarize_http_failure_body(status: u16, body: &[u8]) -> String {
    // The upstream body is untrusted and may contain credentials, request
    // fragments, or provider-internal details. Only retain the one allowlisted
    // signal needed for the OpenAI stream_options compatibility retry.
    let mentions_stream_options = matches!(status, 400 | 422)
        && String::from_utf8_lossy(body)
            .to_ascii_lowercase()
            .contains("stream_options");
    if mentions_stream_options {
        "Upstream rejected stream_options".to_string()
    } else {
        format!("Upstream returned HTTP {status}")
    }
}

/// Consume a bounded prefix of an unsuccessful HTTP response and return an
/// allowlisted summary. Raw upstream bodies must never enter errors, traces,
/// logs, or admin monitoring records.
pub async fn summarize_http_failure_response(mut response: reqwest::Response) -> String {
    let status = response.status().as_u16();
    let mut inspected = Vec::new();
    while inspected.len() < MAX_HTTP_FAILURE_INSPECTION_BYTES {
        let Ok(Some(chunk)) = response.chunk().await else {
            break;
        };
        let remaining = MAX_HTTP_FAILURE_INSPECTION_BYTES - inspected.len();
        inspected.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() >= remaining {
            break;
        }
    }
    summarize_http_failure_body(status, &inspected)
}

async fn http_failure(response: reqwest::Response, meta: UpstreamResponseMeta) -> UpstreamFailure {
    let status = meta.status;
    let summary = summarize_http_failure_response(response).await;
    UpstreamFailure {
        kind: UpstreamFailureKind::HttpStatus,
        status: Some(status),
        headers_received_at: Some(meta.headers_received_at),
        upstream_request_id: meta.upstream_request_id,
        retryable: status == 408 || status == 409 || status == 429 || status >= 500,
        stable_error_code: format!("upstream_http_{status}"),
        sanitized_summary: keycompute_types::sanitize_error_summary(&summary),
    }
}

/// HTTP 传输层 trait
///
/// 抽象 HTTP 客户端操作，支持：
/// - 普通请求
/// - 流式请求
/// - multipart/form-data 请求
/// - 超时控制
#[async_trait]
pub trait HttpTransport: Send + Sync + std::fmt::Debug {
    /// Structured response variant. Implementations should override this to preserve metadata.
    async fn post_json_response(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> std::result::Result<UpstreamResponse<String>, UpstreamFailure> {
        self.post_json(url, headers, body)
            .await
            .map(|body| UpstreamResponse {
                meta: UpstreamResponseMeta {
                    status: 200,
                    headers_received_at: Utc::now(),
                    upstream_request_id: None,
                    headers: Vec::new(),
                },
                body,
            })
            .map_err(|error| UpstreamFailure {
                kind: UpstreamFailureKind::Transport,
                status: None,
                headers_received_at: None,
                upstream_request_id: None,
                retryable: error.is_retryable(),
                stable_error_code: "upstream_transport".to_string(),
                sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
            })
    }

    async fn post_stream_response(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> std::result::Result<UpstreamResponse<ByteStream>, UpstreamFailure> {
        self.post_stream(url, headers, body)
            .await
            .map(|body| UpstreamResponse {
                meta: UpstreamResponseMeta {
                    status: 200,
                    headers_received_at: Utc::now(),
                    upstream_request_id: None,
                    headers: Vec::new(),
                },
                body,
            })
            .map_err(|error| UpstreamFailure {
                kind: UpstreamFailureKind::Transport,
                status: None,
                headers_received_at: None,
                upstream_request_id: None,
                retryable: error.is_retryable(),
                stable_error_code: "upstream_transport".to_string(),
                sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
            })
    }
    /// 发送 POST 请求并返回响应体
    async fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<String>;

    /// 发送 POST 请求并返回字节流（用于 SSE）
    async fn post_stream(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<ByteStream>;

    /// 发送原始 POST 请求（自定义 Content-Type），用于 multipart/form-data 等场景
    ///
    /// 默认实现返回错误。需要处理二进制 body 的实现方（如 multipart/form-data）
    /// 必须显式覆盖此方法。不提供隐式回退到 `post_json`，因为：
    /// 1. multipart body 的二进制数据无法安全地通过 UTF-8 转换
    /// 2. Content-Type 语义不同（multipart/form-data vs application/json）
    /// 3. 静默回退会导致难以排查的运行时数据损坏
    async fn post_raw(
        &self,
        _url: &str,
        _headers: Vec<(String, String)>,
        _body: Vec<u8>,
    ) -> Result<String> {
        Err(keycompute_types::KeyComputeError::ProviderError(
            "post_raw is not supported by this transport implementation".into(),
        ))
    }

    /// 获取请求超时
    fn request_timeout(&self) -> Duration;

    /// 获取流式请求超时
    fn stream_timeout(&self) -> Duration;

    /// 发送 GET 请求并返回二进制响应体与 Content-Type
    ///
    /// 默认实现返回错误。需要处理二进制 GET 请求的实现方应覆盖此方法。
    /// 用于图片下载等场景，支持通过 Host header 实现 DNS 重绑定防护。
    /// 返回 `GetBinaryResponse` 包含 body 和 `content_type`，
    /// 便于调用方校验响应 MIME 类型（如图片下载后验证 `image/*`）。
    ///
    /// # 安全要求
    ///
    /// 实现方必须禁止 HTTP 重定向（`redirect::Policy::none()`），
    /// 防止 SSRF 攻击者通过 30x 重定向将请求引流至内网地址，
    /// 绕过调用方的 DNS 重绑定防护。
    async fn get_binary(
        &self,
        _url: &str,
        _headers: Vec<(String, String)>,
    ) -> Result<GetBinaryResponse> {
        Err(keycompute_types::KeyComputeError::ProviderError(
            "get_binary is not supported by this transport implementation".into(),
        ))
    }

    /// Metadata-preserving GET variant used by account probes. Legacy test or
    /// custom transports inherit a wrapper around `get_binary`; production
    /// transports override it to retain HTTP status and request correlation.
    async fn get_binary_response(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> std::result::Result<UpstreamResponse<GetBinaryResponse>, UpstreamFailure> {
        self.get_binary(url, headers)
            .await
            .map(|body| UpstreamResponse {
                meta: UpstreamResponseMeta::synthetic_success(),
                body,
            })
            .map_err(|error| UpstreamFailure {
                kind: UpstreamFailureKind::Transport,
                status: None,
                headers_received_at: None,
                upstream_request_id: None,
                retryable: error.is_retryable(),
                stable_error_code: "upstream_transport".to_string(),
                sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
            })
    }
}

/// GET 二进制响应
#[derive(Debug, Clone)]
pub struct GetBinaryResponse {
    /// 响应体字节
    pub body: Vec<u8>,
    /// Content-Type（从响应头提取）
    pub content_type: Option<String>,
}

/// 字节流类型
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

/// 默认 HTTP 传输实现（使用 reqwest）
#[derive(Debug, Clone)]
pub struct DefaultHttpTransport {
    client: reqwest::Client,
    request_timeout: Duration,
    stream_timeout: Duration,
}

impl Default for DefaultHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultHttpTransport {
    /// 创建新的默认传输
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("Failed to build HTTP client"),
            request_timeout: Duration::from_secs(120),
            stream_timeout: Duration::from_secs(600),
        }
    }

    /// 创建带自定义超时的传输
    pub fn with_timeouts(request_timeout: Duration, stream_timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("Failed to build HTTP client"),
            request_timeout,
            stream_timeout,
        }
    }

    /// 构建请求
    fn build_request(
        &self,
        method: reqwest::Method,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> reqwest::RequestBuilder {
        let mut builder = self.client.request(method, url);
        for (key, value) in headers {
            builder = builder.header(key, value);
        }
        builder.body(body)
    }
}

#[async_trait]
impl HttpTransport for DefaultHttpTransport {
    async fn post_json_response(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> std::result::Result<UpstreamResponse<String>, UpstreamFailure> {
        let response = self
            .build_request(reqwest::Method::POST, url, headers, body)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|error| UpstreamFailure::transport(&error, false))?;
        let meta = response_meta(&response);
        if !response.status().is_success() {
            return Err(http_failure(response, meta).await);
        }
        let body = response.text().await.map_err(|error| UpstreamFailure {
            kind: UpstreamFailureKind::BodyRead,
            status: Some(meta.status),
            headers_received_at: Some(meta.headers_received_at),
            upstream_request_id: meta.upstream_request_id.clone(),
            // Headers from a successful paid POST make the outcome ambiguous:
            // the provider may have completed and charged the inference.
            retryable: false,
            stable_error_code: "upstream_body_read".to_string(),
            sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
        })?;
        Ok(UpstreamResponse { meta, body })
    }

    async fn post_stream_response(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> std::result::Result<UpstreamResponse<ByteStream>, UpstreamFailure> {
        let response = self
            .build_request(reqwest::Method::POST, url, headers, body)
            .timeout(self.stream_timeout)
            .send()
            .await
            .map_err(|error| UpstreamFailure::transport(&error, false))?;
        let meta = response_meta(&response);
        if !response.status().is_success() {
            return Err(http_failure(response, meta).await);
        }
        let stream_status = meta.status;
        let stream = response.bytes_stream().map(move |result| {
            result.map_err(|error| keycompute_types::KeyComputeError::UpstreamFailure {
                status: Some(stream_status),
                stable_code: "upstream_stream_read".to_string(),
                // Do not repeat a paid POST after the provider accepted it.
                retryable: false,
                summary: keycompute_types::sanitize_error_summary(&error.to_string()),
            })
        });
        Ok(UpstreamResponse {
            meta,
            body: Box::pin(stream),
        })
    }

    async fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<String> {
        self.post_json_response(url, headers, body)
            .await
            .map(|response| response.body)
            .map_err(UpstreamFailure::into_keycompute_error)
    }

    async fn post_stream(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<ByteStream> {
        self.post_stream_response(url, headers, body)
            .await
            .map(|response| response.body)
            .map_err(UpstreamFailure::into_keycompute_error)
    }

    async fn post_raw(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<String> {
        let mut builder = self.client.post(url);
        for (key, value) in headers {
            builder = builder.header(key, value);
        }

        let response = builder
            .body(body)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|error| UpstreamFailure::transport(&error, false).into_keycompute_error())?;

        let meta = response_meta(&response);
        if !response.status().is_success() {
            return Err(http_failure(response, meta).await.into_keycompute_error());
        }

        response
            .text()
            .await
            .map_err(|error| UpstreamFailure {
                kind: UpstreamFailureKind::BodyRead,
                status: Some(meta.status),
                headers_received_at: Some(meta.headers_received_at),
                upstream_request_id: meta.upstream_request_id,
                retryable: false,
                stable_error_code: "upstream_body_read".to_string(),
                sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
            })
            .map_err(UpstreamFailure::into_keycompute_error)
    }

    fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    fn stream_timeout(&self) -> Duration {
        self.stream_timeout
    }

    async fn get_binary(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> Result<GetBinaryResponse> {
        self.get_binary_response(url, headers)
            .await
            .map(|response| response.body)
            .map_err(UpstreamFailure::into_keycompute_error)
    }

    async fn get_binary_response(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> std::result::Result<UpstreamResponse<GetBinaryResponse>, UpstreamFailure> {
        let mut builder = self.client.get(url);
        for (key, value) in headers {
            builder = builder.header(key, value);
        }

        let response = builder
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|error| UpstreamFailure::transport(&error, true))?;

        let meta = response_meta(&response);
        if !response.status().is_success() {
            return Err(http_failure(response, meta).await);
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body = response.bytes().await.map_err(|error| UpstreamFailure {
            kind: UpstreamFailureKind::BodyRead,
            status: Some(meta.status),
            headers_received_at: Some(meta.headers_received_at),
            upstream_request_id: meta.upstream_request_id.clone(),
            retryable: true,
            stable_error_code: "upstream_body_read".to_string(),
            sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
        })?;
        Ok(UpstreamResponse {
            meta,
            body: GetBinaryResponse {
                body: body.to_vec(),
                content_type,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_transport_new() {
        let transport = DefaultHttpTransport::new();
        assert_eq!(transport.request_timeout(), Duration::from_secs(120));
        assert_eq!(transport.stream_timeout(), Duration::from_secs(600));
    }

    #[test]
    fn test_default_transport_with_timeouts() {
        let transport =
            DefaultHttpTransport::with_timeouts(Duration::from_secs(60), Duration::from_secs(300));
        assert_eq!(transport.request_timeout(), Duration::from_secs(60));
        assert_eq!(transport.stream_timeout(), Duration::from_secs(300));
    }

    #[test]
    fn http_failure_summary_never_preserves_raw_body_details() {
        let body = br#"{"error":"client_secret=secret access_token=token prompt=private"}"#;
        let summary = summarize_http_failure_body(401, body);

        assert_eq!(summary, "Upstream returned HTTP 401");
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("token"));
        assert!(!summary.contains("private"));
    }

    #[test]
    fn http_failure_summary_only_preserves_stream_options_compatibility_signal() {
        let body = br#"{"error":"unknown field stream_options","client_secret":"secret"}"#;

        assert_eq!(
            summarize_http_failure_body(400, body),
            "Upstream rejected stream_options"
        );
        assert_eq!(
            summarize_http_failure_body(500, body),
            "Upstream returned HTTP 500"
        );
    }
}
