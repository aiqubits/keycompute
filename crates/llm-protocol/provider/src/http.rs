//! HTTP 传输层抽象
//!
//! 定义统一的 HTTP 客户端接口，供 Provider Adapter 使用。
//! 具体实现由 llm-gateway 提供，避免循环依赖。

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use keycompute_types::Result;
use std::pin::Pin;
use std::time::Duration;

/// HTTP 传输层 trait
///
/// 抽象 HTTP 客户端操作，支持：
/// - 普通请求
/// - 流式请求
/// - multipart/form-data 请求
/// - 超时控制
#[async_trait]
pub trait HttpTransport: Send + Sync + std::fmt::Debug {
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
    async fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<String> {
        let response = self
            .build_request(reqwest::Method::POST, url, headers, body)
            .timeout(self.request_timeout)
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
    ) -> Result<ByteStream> {
        let response = self
            .build_request(reqwest::Method::POST, url, headers, body)
            .timeout(self.stream_timeout)
            .send()
            .await
            .map_err(|e| {
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
        let mut builder = self.client.get(url);
        for (key, value) in headers {
            builder = builder.header(key, value);
        }

        let response = builder
            .timeout(self.request_timeout)
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
}
