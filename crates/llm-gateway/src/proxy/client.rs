//! HTTP 客户端
//!
//! 统一的 HTTP 客户端，支持代理、超时、追踪
//!
//! 实现 HttpTransport trait，供 Provider Adapter 使用

use crate::proxy::ProxyConfig;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use llm_protocol_provider::{
    ByteStream, GetBinaryResponse, HttpTransport, UpstreamFailure, UpstreamFailureKind,
    UpstreamResponse, UpstreamResponseMeta, summarize_http_failure_response,
};
use reqwest::{Client, ClientBuilder, Proxy, RequestBuilder, Response};
use std::time::Duration;

/// HTTP 客户端
///
/// 封装 reqwest::Client，提供：
/// - 统一的超时配置
/// - 代理支持
/// - 请求追踪
/// - 连接池复用
#[derive(Debug, Clone)]
pub struct HttpClient {
    /// 内部 reqwest 客户端
    client: Client,
    /// 配置
    config: ProxyConfig,
    /// 是否使用代理
    has_proxy: bool,
}

impl HttpClient {
    /// 创建新的 HTTP 客户端
    pub fn new(config: &ProxyConfig, proxy_url: Option<&str>) -> Self {
        let mut builder = ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .pool_idle_timeout(config.pool_idle_timeout)
            .user_agent(&config.user_agent);

        // TCP keepalive
        if let Some(keepalive) = config.tcp_keepalive {
            builder = builder.tcp_keepalive(keepalive);
        }

        // 代理配置
        let has_proxy = proxy_url.is_some();
        if let Some(url) = proxy_url
            && let Ok(proxy) = Proxy::all(url)
        {
            builder = builder.proxy(proxy);
        }

        let client = builder.build().unwrap_or_else(|_| Client::new());

        Self {
            client,
            config: config.clone(),
            has_proxy,
        }
    }

    /// 创建 GET 请求
    pub fn get(&self, url: &str) -> RequestBuilder {
        self.client.get(url)
    }

    /// 创建 POST 请求
    pub fn post(&self, url: &str) -> RequestBuilder {
        self.client.post(url)
    }

    /// 创建带追踪的请求
    ///
    /// 自动添加 request_id 到请求头和 tracing span
    pub fn post_with_tracing(
        &self,
        url: &str,
        request_id: uuid::Uuid,
        provider: &str,
    ) -> RequestBuilder {
        self.client
            .post(url)
            .header("X-Request-ID", request_id.to_string())
            .header("X-Provider", provider)
    }

    /// 执行请求并返回响应
    pub async fn execute(&self, request: RequestBuilder) -> keycompute_types::Result<Response> {
        request
            .send()
            .await
            .map_err(|error| Self::transport_failure(&error).into_keycompute_error())
    }

    /// 执行流式请求
    ///
    /// 返回字节流，用于 SSE 解析
    pub async fn execute_stream(
        &self,
        request: RequestBuilder,
    ) -> keycompute_types::Result<impl Stream<Item = Result<Bytes, reqwest::Error>>> {
        let response = request
            .send()
            .await
            .map_err(|error| Self::transport_failure(&error).into_keycompute_error())?;

        if !response.status().is_success() {
            let meta = Self::response_meta(&response);
            return Err(Self::http_failure(response, meta)
                .await
                .into_keycompute_error());
        }

        Ok(response.bytes_stream())
    }

    /// 获取底层客户端
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// 是否使用代理
    pub fn has_proxy(&self) -> bool {
        self.has_proxy
    }

    /// 是否共享（用于测试）
    pub fn is_shared(&self) -> bool {
        true
    }

    /// 获取配置
    pub fn config(&self) -> &ProxyConfig {
        &self.config
    }

    fn response_meta(response: &Response) -> UpstreamResponseMeta {
        Self::response_meta_from_parts(response.status().as_u16(), response.headers())
    }

    fn response_meta_from_parts(
        status: u16,
        response_headers: &reqwest::header::HeaderMap,
    ) -> UpstreamResponseMeta {
        let headers = response_headers
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
            .find_map(|name| response_headers.get(*name))
            .and_then(|value| value.to_str().ok())
            .map(|value| value.chars().take(128).collect());
        UpstreamResponseMeta {
            status,
            headers_received_at: chrono::Utc::now(),
            upstream_request_id,
            headers,
        }
    }

    fn transport_failure(error: &reqwest::Error) -> UpstreamFailure {
        let timeout = error.is_timeout();
        let definitely_pre_dispatch = error.is_connect();
        let ambiguous_after_dispatch = !definitely_pre_dispatch;
        UpstreamFailure {
            kind: if timeout {
                UpstreamFailureKind::Timeout
            } else {
                UpstreamFailureKind::Transport
            },
            status: None,
            headers_received_at: None,
            upstream_request_id: None,
            retryable: definitely_pre_dispatch,
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

    async fn http_failure(response: Response, meta: UpstreamResponseMeta) -> UpstreamFailure {
        let status = meta.status;
        let summary = summarize_http_failure_response(response).await;
        UpstreamFailure {
            kind: UpstreamFailureKind::HttpStatus,
            status: Some(status),
            headers_received_at: Some(meta.headers_received_at),
            upstream_request_id: meta.upstream_request_id,
            retryable: status == 408 || status == 409 || status == 429 || status >= 500,
            stable_error_code: format!("upstream_http_{status}"),
            sanitized_summary: summary,
        }
    }
}

#[async_trait]
impl HttpTransport for HttpClient {
    async fn post_json_response(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> std::result::Result<UpstreamResponse<String>, UpstreamFailure> {
        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request
            .body(body)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|error| Self::transport_failure(&error))?;
        let meta = Self::response_meta(&response);
        if !response.status().is_success() {
            return Err(Self::http_failure(response, meta).await);
        }
        let body = response.text().await.map_err(|error| UpstreamFailure {
            kind: UpstreamFailureKind::BodyRead,
            status: Some(meta.status),
            headers_received_at: Some(meta.headers_received_at),
            upstream_request_id: meta.upstream_request_id.clone(),
            // A successful status proves that the paid POST reached the
            // provider. Reissuing it without provider idempotency could charge
            // the request twice even though no body reached the client.
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
        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request
            .body(body)
            .timeout(self.config.stream_timeout)
            .send()
            .await
            .map_err(|error| Self::transport_failure(&error))?;
        let meta = Self::response_meta(&response);
        if !response.status().is_success() {
            return Err(Self::http_failure(response, meta).await);
        }
        let status = meta.status;
        let stream = response.bytes_stream().map(move |result| {
            result.map_err(|error| keycompute_types::KeyComputeError::UpstreamFailure {
                status: Some(status),
                stable_code: "upstream_stream_read".to_string(),
                // The provider has already accepted the paid POST. A read
                // failure before the first chunk is still an ambiguous result,
                // not permission to issue the inference again.
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
    ) -> keycompute_types::Result<String> {
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
    ) -> keycompute_types::Result<ByteStream> {
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
    ) -> keycompute_types::Result<String> {
        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }

        let response = request
            .body(body)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|error| Self::transport_failure(&error).into_keycompute_error())?;

        let meta = Self::response_meta(&response);
        if !response.status().is_success() {
            return Err(Self::http_failure(response, meta)
                .await
                .into_keycompute_error());
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
        self.config.request_timeout
    }

    fn stream_timeout(&self) -> Duration {
        self.config.stream_timeout
    }

    async fn get_binary(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> keycompute_types::Result<GetBinaryResponse> {
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
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|error| {
                let timeout = error.is_timeout();
                UpstreamFailure {
                    kind: if timeout {
                        UpstreamFailureKind::Timeout
                    } else {
                        UpstreamFailureKind::Transport
                    },
                    status: None,
                    headers_received_at: None,
                    upstream_request_id: None,
                    retryable: timeout || error.is_connect(),
                    stable_error_code: if timeout {
                        "upstream_timeout"
                    } else {
                        "upstream_transport"
                    }
                    .to_string(),
                    sanitized_summary: keycompute_types::sanitize_error_summary(&error.to_string()),
                }
            })?;

        let response_headers = response
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
        let meta = UpstreamResponseMeta {
            status: response.status().as_u16(),
            headers_received_at: chrono::Utc::now(),
            upstream_request_id,
            headers: response_headers,
        };

        if !response.status().is_success() {
            return Err(Self::http_failure(response, meta).await);
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

/// 请求构建器扩展
pub trait RequestBuilderExt {
    /// 设置流式请求超时
    fn stream_timeout(self, duration: Duration) -> Self;

    /// 添加请求追踪头
    fn with_tracing(self, request_id: uuid::Uuid, provider: &str) -> Self;
}

impl RequestBuilderExt for RequestBuilder {
    fn stream_timeout(self, duration: Duration) -> Self {
        self.timeout(duration)
    }

    fn with_tracing(self, request_id: uuid::Uuid, provider: &str) -> Self {
        self.header("X-Request-ID", request_id.to_string())
            .header("X-Provider", provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_client_new() {
        let config = ProxyConfig::default();
        let client = HttpClient::new(&config, None);

        assert!(!client.has_proxy());
    }

    #[test]
    fn test_http_client_with_proxy() {
        let config = ProxyConfig::default();
        let client = HttpClient::new(&config, Some("http://localhost:8080"));

        assert!(client.has_proxy());
    }

    #[test]
    fn test_http_client_post() {
        let config = ProxyConfig::default();
        let client = HttpClient::new(&config, None);

        let _request = client.post("https://api.example.com/v1/chat");
    }

    #[test]
    fn test_http_client_post_with_tracing() {
        let config = ProxyConfig::default();
        let client = HttpClient::new(&config, None);

        let request_id = uuid::Uuid::new_v4();
        let _request =
            client.post_with_tracing("https://api.example.com/v1/chat", request_id, "openai");
    }

    #[test]
    fn structured_post_metadata_keeps_status_and_upstream_request_id() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-request-id", "provider-request-id".parse().unwrap());
        let metadata = HttpClient::response_meta_from_parts(401, &headers);

        assert_eq!(metadata.status, 401);
        assert_eq!(
            metadata.upstream_request_id.as_deref(),
            Some("provider-request-id")
        );
    }
}
