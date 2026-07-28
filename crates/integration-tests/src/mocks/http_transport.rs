//! Mock HTTP 传输层实现
//!
//! 提供可配置的 Mock HTTP Transport，支持：
//! - 预配置响应
//! - 代理配置模拟
//! - 延迟模拟
//! - 错误模拟
//! - 流式响应模拟

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use keycompute_types::{KeyComputeError, Result};
use llm_protocol_provider::{ByteStream, GetBinaryResponse, HttpTransport};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

/// Mock 响应配置
#[derive(Debug, Clone)]
pub struct MockResponse {
    /// HTTP 状态码
    pub status: u16,
    /// 响应头
    pub headers: Vec<(String, String)>,
    /// 响应体
    pub body: String,
    /// 响应延迟
    pub delay: Option<Duration>,
}

impl MockResponse {
    /// 创建成功响应
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.into(),
            delay: None,
        }
    }

    /// 创建 JSON 成功响应
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: body.into(),
            delay: None,
        }
    }

    /// 创建错误响应
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            headers: vec![],
            body: message.into(),
            delay: None,
        }
    }

    /// 创建 500 错误
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::error(500, message)
    }

    /// 创建 429 限流错误
    pub fn rate_limited(retry_after: u64) -> Self {
        Self {
            status: 429,
            headers: vec![("retry-after".to_string(), retry_after.to_string())],
            body: r#"{"error": "Rate limit exceeded"}"#.to_string(),
            delay: None,
        }
    }

    /// 创建 401 认证错误
    pub fn unauthorized() -> Self {
        Self::error(401, r#"{"error": "Unauthorized"}"#)
    }

    /// 创建 403 禁止访问
    pub fn forbidden() -> Self {
        Self::error(403, r#"{"error": "Forbidden"}"#)
    }

    /// 创建 404 未找到
    pub fn not_found() -> Self {
        Self::error(404, r#"{"error": "Not found"}"#)
    }

    /// 创建超时响应（延迟后返回）
    pub fn timeout(delay: Duration) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: String::new(),
            delay: Some(delay),
        }
    }

    /// 添加延迟
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// 添加响应头
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }
}

/// 流式响应配置
#[derive(Debug, Clone)]
pub struct MockStreamConfig {
    /// SSE 数据块
    pub chunks: Vec<String>,
    /// 每个 chunk 的延迟
    pub chunk_delay: Option<Duration>,
    /// 是否在中间注入错误
    pub error_at: Option<usize>,
    /// 错误消息
    pub error_message: Option<String>,
}

impl MockStreamConfig {
    /// 创建简单的 SSE 流
    pub fn simple(chunks: Vec<String>) -> Self {
        Self {
            chunks,
            chunk_delay: None,
            error_at: None,
            error_message: None,
        }
    }

    /// 创建 OpenAI 风格的 SSE 流
    pub fn openai_style(content: &str) -> Self {
        let chunks: Vec<String> = content
            .split_whitespace()
            .enumerate()
            .map(|(i, word)| {
                format!(
                    r#"data: {{"id":"chatcmpl-{}","object":"chat.completion.chunk","choices":[{{"index":0,"delta":{{"content":"{}"}},"finish_reason":null}}]}}"#,
                    i, word
                )
            })
            .collect();

        let mut result = chunks;
        result.push("data: [DONE]".to_string());

        Self::simple(result)
    }

    /// 添加 chunk 延迟
    pub fn with_chunk_delay(mut self, delay: Duration) -> Self {
        self.chunk_delay = Some(delay);
        self
    }

    /// 在指定位置注入错误
    pub fn with_error_at(mut self, at: usize, message: impl Into<String>) -> Self {
        self.error_at = Some(at);
        self.error_message = Some(message.into());
        self
    }
}

/// 代理配置
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    /// HTTP 代理 URL
    pub http_proxy: Option<String>,
    /// HTTPS 代理 URL
    pub https_proxy: Option<String>,
    /// 代理用户名
    pub username: Option<String>,
    /// 代理密码
    pub password: Option<String>,
    /// 是否启用代理
    pub enabled: bool,
}

impl ProxyConfig {
    /// 创建新的代理配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 HTTP 代理
    pub fn http(mut self, url: impl Into<String>) -> Self {
        self.http_proxy = Some(url.into());
        self.enabled = true;
        self
    }

    /// 设置 HTTPS 代理
    pub fn https(mut self, url: impl Into<String>) -> Self {
        self.https_proxy = Some(url.into());
        self.enabled = true;
        self
    }

    /// 设置认证信息
    pub fn auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }
}

/// 请求记录
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// 请求 URL
    pub url: String,
    /// 请求头
    pub headers: Vec<(String, String)>,
    /// 请求体（文本；对于二进制请求使用 lossy UTF-8 转换）
    pub body: String,
    /// 原始请求体字节（仅 post_raw 场景填充，用于检查二进制 body）
    pub raw_body: Option<Vec<u8>>,
    /// 请求时间
    pub timestamp: std::time::Instant,
}

/// Mock HTTP Transport
#[derive(Debug)]
pub struct MockHttpTransport {
    /// 预配置的响应队列
    responses: Mutex<VecDeque<MockResponse>>,
    /// 流式响应配置
    stream_config: Mutex<Option<MockStreamConfig>>,
    /// 代理配置
    proxy_config: ProxyConfig,
    /// 请求超时
    request_timeout: Duration,
    /// 流超时
    stream_timeout: Duration,
    /// 请求记录
    request_history: Mutex<Vec<RequestRecord>>,
    /// 是否自动失败
    should_fail: bool,
    /// 失败消息
    failure_message: String,
}

impl Default for MockHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockHttpTransport {
    /// 创建新的 Mock Transport
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            stream_config: Mutex::new(None),
            proxy_config: ProxyConfig::default(),
            request_timeout: Duration::from_secs(120),
            stream_timeout: Duration::from_secs(600),
            request_history: Mutex::new(Vec::new()),
            should_fail: false,
            failure_message: String::new(),
        }
    }

    /// 添加响应
    pub fn add_response(&self, response: MockResponse) {
        self.responses.lock().unwrap().push_back(response);
    }

    /// 设置响应序列
    pub fn with_responses(mut self, responses: Vec<MockResponse>) -> Self {
        self.responses = Mutex::new(responses.into_iter().collect());
        self
    }

    /// 设置单个响应
    pub fn with_response(mut self, response: MockResponse) -> Self {
        self.responses = Mutex::new(vec![response].into_iter().collect());
        self
    }

    /// 设置成功 JSON 响应
    pub fn with_success_json(mut self, body: impl Into<String>) -> Self {
        self.responses = Mutex::new(vec![MockResponse::json(body)].into_iter().collect());
        self
    }

    /// 设置错误响应
    pub fn with_error(mut self, status: u16, message: impl Into<String>) -> Self {
        self.responses = Mutex::new(
            vec![MockResponse::error(status, message)]
                .into_iter()
                .collect(),
        );
        self
    }

    /// 设置流式响应配置
    pub fn with_stream_config(mut self, config: MockStreamConfig) -> Self {
        self.stream_config = Mutex::new(Some(config));
        self
    }

    /// 设置代理配置
    pub fn with_proxy(mut self, config: ProxyConfig) -> Self {
        self.proxy_config = config;
        self
    }

    /// 设置请求超时
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// 设置流超时
    pub fn with_stream_timeout(mut self, timeout: Duration) -> Self {
        self.stream_timeout = timeout;
        self
    }

    /// 设置自动失败
    pub fn with_failure(mut self, message: impl Into<String>) -> Self {
        self.should_fail = true;
        self.failure_message = message.into();
        self
    }

    /// 获取请求历史
    pub fn request_history(&self) -> Vec<RequestRecord> {
        self.request_history.lock().unwrap().clone()
    }

    /// 获取请求次数
    pub fn request_count(&self) -> usize {
        self.request_history.lock().unwrap().len()
    }

    /// 获取最后一次请求
    pub fn last_request(&self) -> Option<RequestRecord> {
        self.request_history.lock().unwrap().last().cloned()
    }

    /// 清空请求历史
    pub fn clear_history(&self) {
        self.request_history.lock().unwrap().clear();
    }

    /// 获取代理配置
    pub fn proxy_config(&self) -> &ProxyConfig {
        &self.proxy_config
    }

    /// 记录请求（文本 body）
    fn record_request(&self, url: &str, headers: Vec<(String, String)>, body: String) {
        self.request_history.lock().unwrap().push(RequestRecord {
            url: url.to_string(),
            headers,
            body,
            raw_body: None,
            timestamp: std::time::Instant::now(),
        });
    }

    /// 记录请求（原始二进制 body，用于 multipart/form-data 等场景）
    fn record_request_raw(&self, url: &str, headers: Vec<(String, String)>, body: Vec<u8>) {
        // 使用 lossy UTF-8 转换存储文本表示，同时保留原始字节
        let body_str = String::from_utf8_lossy(&body).to_string();
        self.request_history.lock().unwrap().push(RequestRecord {
            url: url.to_string(),
            headers,
            body: body_str,
            raw_body: Some(body),
            timestamp: std::time::Instant::now(),
        });
    }

    /// 获取下一个响应
    fn get_next_response(&self) -> Option<MockResponse> {
        self.responses.lock().unwrap().pop_front()
    }
}

#[async_trait]
impl HttpTransport for MockHttpTransport {
    async fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<String> {
        // 记录请求
        self.record_request(url, headers.clone(), body.clone());

        // 模拟延迟
        if let Some(response) = self.get_next_response() {
            if let Some(delay) = response.delay {
                tokio::time::sleep(delay).await;
            }

            if response.status >= 200 && response.status < 300 {
                Ok(response.body)
            } else {
                Err(KeyComputeError::ProviderError(format!(
                    "HTTP error ({}): {}",
                    response.status, response.body
                )))
            }
        } else if self.should_fail {
            Err(KeyComputeError::ProviderError(self.failure_message.clone()))
        } else {
            // 默认返回空成功响应
            Ok(r#"{"status": "ok"}"#.to_string())
        }
    }

    async fn post_stream(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<ByteStream> {
        // 记录请求
        self.record_request(url, headers, body);

        // 检查是否应该失败
        if self.should_fail {
            return Err(KeyComputeError::ProviderError(self.failure_message.clone()));
        }

        // 获取流配置
        let config = self.stream_config.lock().unwrap().clone();

        if let Some(config) = config {
            let chunks = config.chunks;
            let chunk_delay = config.chunk_delay;
            let error_at = config.error_at;
            let error_message = config.error_message;

            let stream = stream::unfold(
                (chunks, 0usize, chunk_delay, error_at, error_message),
                move |(chunks, index, delay, error_at, error_message)| async move {
                    // 每个 chunk 延迟
                    if let Some(d) = delay {
                        tokio::time::sleep(d).await;
                    }

                    if index >= chunks.len() {
                        return None;
                    }

                    // 检查是否需要注入错误
                    if let Some(err_idx) = error_at
                        && index == err_idx
                        && let Some(msg) = error_message
                    {
                        return Some((
                            Err(KeyComputeError::ProviderError(msg)),
                            (chunks, index + 1, delay, None, None),
                        ));
                    }

                    let chunk = chunks[index].clone();
                    Some((
                        Ok(Bytes::from(chunk)),
                        (chunks, index + 1, delay, error_at, error_message),
                    ))
                },
            );

            Ok(Box::pin(stream))
        } else {
            // 默认返回空流
            Ok(Box::pin(stream::empty()))
        }
    }

    async fn post_raw(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<String> {
        // 记录请求（保留原始字节）
        self.record_request_raw(url, headers.clone(), body.clone());

        // 模拟延迟
        if let Some(response) = self.get_next_response() {
            if let Some(delay) = response.delay {
                tokio::time::sleep(delay).await;
            }

            if response.status >= 200 && response.status < 300 {
                Ok(response.body)
            } else {
                Err(KeyComputeError::ProviderError(format!(
                    "HTTP error ({}): {}",
                    response.status, response.body
                )))
            }
        } else if self.should_fail {
            Err(KeyComputeError::ProviderError(self.failure_message.clone()))
        } else {
            // 默认返回空成功响应
            Ok(r#"{"status": "ok"}"#.to_string())
        }
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
        // 记录请求（GET）
        self.record_request(url, headers.clone(), String::new());

        // 模拟延迟
        if let Some(response) = self.get_next_response() {
            if let Some(delay) = response.delay {
                tokio::time::sleep(delay).await;
            }

            if response.status >= 200 && response.status < 300 {
                let content_type = response
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.clone());
                Ok(GetBinaryResponse {
                    body: response.body.into_bytes(),
                    content_type,
                })
            } else {
                Err(KeyComputeError::ProviderError(format!(
                    "HTTP error ({}): {}",
                    response.status, response.body
                )))
            }
        } else if self.should_fail {
            Err(KeyComputeError::ProviderError(self.failure_message.clone()))
        } else {
            // 默认返回空字节
            Ok(GetBinaryResponse {
                body: Vec::new(),
                content_type: None,
            })
        }
    }
}

/// MockHttpTransport 工厂
pub struct MockHttpTransportFactory;

impl MockHttpTransportFactory {
    /// 创建成功的 Mock Transport
    pub fn success() -> MockHttpTransport {
        MockHttpTransport::new().with_success_json(r#"{"result": "success"}"#)
    }

    /// 创建返回错误的 Mock Transport
    pub fn error(status: u16, message: &str) -> MockHttpTransport {
        MockHttpTransport::new().with_error(status, message)
    }

    /// 创建超时的 Mock Transport
    pub fn timeout(delay: Duration) -> MockHttpTransport {
        MockHttpTransport::new().with_response(MockResponse::timeout(delay))
    }

    /// 创建带代理的 Mock Transport
    pub fn with_proxy(proxy_config: ProxyConfig) -> MockHttpTransport {
        MockHttpTransport::new().with_proxy(proxy_config)
    }

    /// 创建返回 SSE 流的 Mock Transport
    pub fn stream(chunks: Vec<String>) -> MockHttpTransport {
        MockHttpTransport::new().with_stream_config(MockStreamConfig::simple(chunks))
    }

    /// 创建 OpenAI 风格的流 Mock Transport
    pub fn openai_stream(content: &str) -> MockHttpTransport {
        MockHttpTransport::new().with_stream_config(MockStreamConfig::openai_style(content))
    }

    /// 创建限流的 Mock Transport
    pub fn rate_limited(retry_after: u64) -> MockHttpTransport {
        MockHttpTransport::new().with_response(MockResponse::rate_limited(retry_after))
    }

    /// 创建认证失败的 Mock Transport
    pub fn unauthorized() -> MockHttpTransport {
        MockHttpTransport::new().with_response(MockResponse::unauthorized())
    }

    /// 创建服务器错误的 Mock Transport
    pub fn server_error() -> MockHttpTransport {
        MockHttpTransport::new()
            .with_response(MockResponse::internal_error("Internal server error"))
    }

    /// 创建响应序列
    pub fn sequence(responses: Vec<MockResponse>) -> MockHttpTransport {
        MockHttpTransport::new().with_responses(responses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_response_builders() {
        let ok = MockResponse::ok("test");
        assert_eq!(ok.status, 200);
        assert_eq!(ok.body, "test");

        let err = MockResponse::error(404, "not found");
        assert_eq!(err.status, 404);

        let rate_limited = MockResponse::rate_limited(60);
        assert_eq!(rate_limited.status, 429);
    }

    #[test]
    fn test_proxy_config() {
        let config = ProxyConfig::new()
            .http("http://proxy:8080")
            .auth("user", "pass");

        assert_eq!(config.http_proxy, Some("http://proxy:8080".to_string()));
        assert!(config.enabled);
    }

    #[tokio::test]
    async fn test_mock_transport_success() {
        let transport = MockHttpTransportFactory::success();
        let result = transport
            .post_json("http://test", vec![], "{}".to_string())
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_transport_error() {
        let transport = MockHttpTransportFactory::error(500, "Server error");
        let result = transport
            .post_json("http://test", vec![], "{}".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_transport_request_history() {
        let transport = MockHttpTransport::new();
        transport.add_response(MockResponse::ok("response"));

        let _ = transport
            .post_json(
                "http://test-url",
                vec![("Authorization".to_string(), "Bearer token".to_string())],
                r#"{"test": true}"#.to_string(),
            )
            .await;

        assert_eq!(transport.request_count(), 1);
        let last = transport.last_request().unwrap();
        assert_eq!(last.url, "http://test-url");
        assert_eq!(last.headers.len(), 1);
    }
}
