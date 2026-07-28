//! HTTP 客户端
//!
//! 统一的 HTTP 客户端，支持代理、超时、追踪
//!
//! 实现 HttpTransport trait，供 Provider Adapter 使用

use crate::proxy::ProxyConfig;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use llm_protocol_provider::{ByteStream, GetBinaryResponse, HttpTransport};
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
        request.send().await.map_err(|e| {
            keycompute_types::KeyComputeError::ProviderError(format!("HTTP request failed: {}", e))
        })
    }

    /// 执行流式请求
    ///
    /// 返回字节流，用于 SSE 解析
    pub async fn execute_stream(
        &self,
        request: RequestBuilder,
    ) -> keycompute_types::Result<impl Stream<Item = Result<Bytes, reqwest::Error>>> {
        let response = request.send().await.map_err(|e| {
            keycompute_types::KeyComputeError::ProviderError(format!(
                "HTTP stream request failed: {}",
                e
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(keycompute_types::KeyComputeError::ProviderError(format!(
                "HTTP error ({}): {}",
                status, error_text
            )));
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
}

#[async_trait]
impl HttpTransport for HttpClient {
    async fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
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
            .map_err(|e| {
                keycompute_types::KeyComputeError::ProviderError(format!(
                    "HTTP request failed: {}",
                    e
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(keycompute_types::KeyComputeError::ProviderError(format!(
                "HTTP error ({}): {}",
                status, error_text
            )));
        }

        response.text().await.map_err(|e| {
            keycompute_types::KeyComputeError::ProviderError(format!(
                "Failed to read response: {}",
                e
            ))
        })
    }

    async fn post_stream(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> keycompute_types::Result<ByteStream> {
        let mut request = self.client.post(url);

        for (key, value) in headers {
            request = request.header(key, value);
        }

        let response = request
            .body(body)
            .timeout(self.config.stream_timeout)
            .send()
            .await
            .map_err(|e| {
                keycompute_types::KeyComputeError::ProviderError(format!(
                    "HTTP stream request failed: {}",
                    e
                ))
            })?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(keycompute_types::KeyComputeError::ProviderError(format!(
                "HTTP error ({}): {}",
                status, error_text
            )));
        }

        // 转换字节流
        let stream = response.bytes_stream().map(|result| {
            result.map_err(|e| {
                keycompute_types::KeyComputeError::ProviderError(format!("Stream error: {}", e))
            })
        });

        Ok(Box::pin(stream))
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
            .map_err(|e| {
                keycompute_types::KeyComputeError::ProviderError(format!(
                    "HTTP request failed: {}",
                    e
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(keycompute_types::KeyComputeError::ProviderError(format!(
                "HTTP error ({}): {}",
                status, error_text
            )));
        }

        response.text().await.map_err(|e| {
            keycompute_types::KeyComputeError::ProviderError(format!(
                "Failed to read response: {}",
                e
            ))
        })
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
        let mut builder = self.client.get(url);
        for (key, value) in headers {
            builder = builder.header(key, value);
        }

        let response = builder
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|e| {
                keycompute_types::KeyComputeError::ProviderError(format!(
                    "HTTP GET request failed: {}",
                    e
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(keycompute_types::KeyComputeError::ProviderError(format!(
                "HTTP error ({}): {}",
                status, error_text
            )));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        response
            .bytes()
            .await
            .map(|b| GetBinaryResponse {
                body: b.to_vec(),
                content_type,
            })
            .map_err(|e| {
                keycompute_types::KeyComputeError::ProviderError(format!(
                    "Failed to read response: {}",
                    e
                ))
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
}
