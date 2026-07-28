//! 模拟 Provider Adapter
//!
//! 提供可配置的 Mock Provider，支持：
//! - 延迟响应
//! - 流错误注入
//! - 超时模拟
//! - 连续失败后恢复

use async_trait::async_trait;
use futures::stream;
use keycompute_types::KeyComputeError;
use llm_protocol_provider::{
    HttpTransport, ProviderAdapter, StreamBox, StreamEvent, UpstreamRequest,
};
use std::sync::Mutex;
use std::time::Duration;

/// 流错误配置
#[derive(Debug, Clone)]
pub struct StreamErrorConfig {
    /// 在第几个 chunk 后注入错误
    pub at_chunk: usize,
    /// 错误消息
    pub error_message: String,
}

/// 模拟 Provider
#[derive(Debug)]
pub struct MockProvider {
    name: &'static str,
    supported_models: Vec<&'static str>,
    response_chunks: Mutex<Vec<String>>,
    input_tokens: u32,
    output_tokens: u32,
    should_fail: bool,
    /// 响应延迟（整体）
    delay: Option<Duration>,
    /// 每个 chunk 的延迟
    per_chunk_delay: Option<Duration>,
    /// 流错误配置
    stream_error: Option<StreamErrorConfig>,
    /// 模拟超时
    simulate_timeout: bool,
    /// 连续失败计数器
    consecutive_failures: Mutex<usize>,
    /// 失败次数阈值（前 N 次失败，之后成功）
    failure_threshold: usize,
}

impl MockProvider {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            supported_models: vec!["gpt-4o", "gpt-3.5-turbo"],
            response_chunks: Mutex::new(vec![
                "Hello".to_string(),
                " from".to_string(),
                " mock".to_string(),
            ]),
            input_tokens: 10,
            output_tokens: 3,
            should_fail: false,
            delay: None,
            per_chunk_delay: None,
            stream_error: None,
            simulate_timeout: false,
            consecutive_failures: Mutex::new(0),
            failure_threshold: 0,
        }
    }

    pub fn with_chunks(mut self, chunks: Vec<String>) -> Self {
        let len = chunks.len();
        *self.response_chunks.lock().unwrap() = chunks;
        self.output_tokens = len as u32;
        self
    }

    pub fn with_tokens(mut self, input: u32, output: u32) -> Self {
        self.input_tokens = input;
        self.output_tokens = output;
        self
    }

    pub fn with_failure(mut self) -> Self {
        self.should_fail = true;
        self
    }

    pub fn with_models(mut self, models: Vec<&'static str>) -> Self {
        self.supported_models = models;
        self
    }

    /// 设置响应延迟
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// 设置每个 chunk 的延迟
    pub fn with_per_chunk_delay(mut self, delay: Duration) -> Self {
        self.per_chunk_delay = Some(delay);
        self
    }

    /// 设置流错误注入（在指定 chunk 后注入错误）
    pub fn with_stream_error(mut self, at_chunk: usize, message: impl Into<String>) -> Self {
        self.stream_error = Some(StreamErrorConfig {
            at_chunk,
            error_message: message.into(),
        });
        self
    }

    /// 设置模拟超时
    pub fn with_simulated_timeout(mut self) -> Self {
        self.simulate_timeout = true;
        self
    }

    /// 设置连续失败阈值（前 N 次失败，之后成功）
    pub fn with_failure_threshold(mut self, threshold: usize) -> Self {
        self.failure_threshold = threshold;
        self.should_fail = false; // 由阈值控制
        self
    }

    /// 重置失败计数
    pub fn reset_failure_count(&self) {
        *self.consecutive_failures.lock().unwrap() = 0;
    }

    /// 获取当前失败次数
    pub fn failure_count(&self) -> usize {
        *self.consecutive_failures.lock().unwrap()
    }
}

#[async_trait]
impl ProviderAdapter for MockProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn supported_models(&self) -> Vec<&'static str> {
        self.supported_models.clone()
    }

    async fn stream_chat(
        &self,
        _transport: &dyn HttpTransport,
        _request: UpstreamRequest,
    ) -> keycompute_types::Result<StreamBox> {
        // 模拟超时
        if self.simulate_timeout {
            return Err(KeyComputeError::ProviderError(
                "Request timeout simulated".to_string(),
            ));
        }

        // 模拟延迟
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }

        // 检查是否应该失败（考虑阈值）
        if self.failure_threshold > 0 {
            let mut failures = self.consecutive_failures.lock().unwrap();
            if *failures < self.failure_threshold {
                *failures += 1;
                return Err(KeyComputeError::ProviderError(format!(
                    "Mock failure #{}",
                    *failures
                )));
            }
            // 超过阈值后成功
        } else if self.should_fail {
            return Err(KeyComputeError::ProviderError("Mock failure".to_string()));
        }

        let chunks = self.response_chunks.lock().unwrap().clone();
        let input_tokens = self.input_tokens;
        let output_tokens = self.output_tokens;
        let stream_error = self.stream_error.clone();
        let per_chunk_delay = self.per_chunk_delay;

        let stream = stream::unfold(
            (
                chunks,
                0usize,
                input_tokens,
                output_tokens,
                stream_error,
                per_chunk_delay,
            ),
            move |(chunks, index, input, output, stream_error, per_chunk_delay)| async move {
                // 每个 chunk 可选延迟
                if let Some(delay) = per_chunk_delay {
                    tokio::time::sleep(delay).await;
                }

                if index >= chunks.len() {
                    // 发送 Usage 事件后结束
                    if index == chunks.len() {
                        let event = StreamEvent::Usage {
                            input_tokens: input,
                            output_tokens: output,
                        };
                        return Some((
                            Ok(event),
                            (
                                chunks,
                                index + 1,
                                input,
                                output,
                                stream_error,
                                per_chunk_delay,
                            ),
                        ));
                    }
                    // 发送 Done 事件
                    if index == chunks.len() + 1 {
                        let event = StreamEvent::Done;
                        return Some((
                            Ok(event),
                            (
                                chunks,
                                index + 1,
                                input,
                                output,
                                stream_error,
                                per_chunk_delay,
                            ),
                        ));
                    }
                    return None;
                }

                // 检查是否需要注入流错误
                if let Some(ref err_config) = stream_error
                    && index == err_config.at_chunk
                {
                    let event = StreamEvent::Error {
                        message: err_config.error_message.clone(),
                    };
                    // 发送错误后继续（模拟部分数据后错误）
                    return Some((
                        Ok(event),
                        (chunks, index + 1, input, output, None, per_chunk_delay),
                    ));
                }

                let content = chunks[index].clone();
                let event = StreamEvent::Delta {
                    content,
                    finish_reason: None,
                };

                Some((
                    Ok(event),
                    (
                        chunks,
                        index + 1,
                        input,
                        output,
                        stream_error,
                        per_chunk_delay,
                    ),
                ))
            },
        );

        Ok(Box::pin(stream))
    }
}

/// 创建模拟 Provider 的工厂
pub struct MockProviderFactory;

impl MockProviderFactory {
    /// 创建一个成功的 OpenAI 模拟 Provider
    pub fn create_openai() -> MockProvider {
        MockProvider::new("openai")
            .with_models(vec!["gpt-4o", "gpt-4o-mini", "gpt-3.5-turbo"])
            .with_chunks(vec![
                "Hello".to_string(),
                " from".to_string(),
                " OpenAI".to_string(),
            ])
            .with_tokens(10, 3)
    }

    /// 创建一个成功的 Anthropic 模拟 Provider
    pub fn create_anthropic() -> MockProvider {
        MockProvider::new("anthropic")
            .with_models(vec!["claude-3-opus", "claude-3-sonnet"])
            .with_chunks(vec![
                "Hello".to_string(),
                " from".to_string(),
                " Claude".to_string(),
            ])
            .with_tokens(8, 3)
    }

    /// 创建一个会失败的 Provider（用于测试 fallback）
    pub fn create_failing() -> MockProvider {
        MockProvider::new("failing").with_failure()
    }

    /// 创建一个有延迟的 Provider
    pub fn create_delayed(delay_ms: u64) -> MockProvider {
        MockProvider::new("delayed").with_delay(Duration::from_millis(delay_ms))
    }

    /// 创建一个会注入流错误的 Provider
    pub fn create_with_stream_error(at_chunk: usize) -> MockProvider {
        MockProvider::new("stream-error")
            .with_chunks(vec![
                "Chunk1".to_string(),
                "Chunk2".to_string(),
                "Chunk3".to_string(),
                "Chunk4".to_string(),
                "Chunk5".to_string(),
            ])
            .with_stream_error(at_chunk, "Simulated stream error")
    }

    /// 创建一个前 N 次失败后成功的 Provider（用于测试冷却后恢复）
    pub fn create_flaky(failure_threshold: usize) -> MockProvider {
        MockProvider::new("flaky").with_failure_threshold(failure_threshold)
    }

    /// 创建一个模拟超时的 Provider
    pub fn create_timeout() -> MockProvider {
        MockProvider::new("timeout").with_simulated_timeout()
    }

    /// 创建一个慢速流 Provider（每个 chunk 有延迟）
    pub fn create_slow_stream(chunk_delay_ms: u64) -> MockProvider {
        MockProvider::new("slow-stream")
            .with_chunks(vec![
                "Slow1".to_string(),
                "Slow2".to_string(),
                "Slow3".to_string(),
            ])
            .with_per_chunk_delay(Duration::from_millis(chunk_delay_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_mock_provider_stream() {
        let provider = MockProviderFactory::create_openai();
        let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o");

        let transport = llm_protocol_provider::DefaultHttpTransport::new();
        let mut stream: llm_protocol_provider::StreamBox =
            provider.stream_chat(&transport, request).await.unwrap();
        let mut events = Vec::new();

        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }

        // 3 个 Delta + 1 个 Usage + 1 个 Done = 5 个事件
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn test_mock_provider_failure() {
        let provider = MockProviderFactory::create_failing();
        let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o");

        let transport = llm_protocol_provider::DefaultHttpTransport::new();
        let result: Result<llm_protocol_provider::StreamBox, _> =
            provider.stream_chat(&transport, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_provider_stream_error() {
        let provider = MockProviderFactory::create_with_stream_error(2);
        let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o");

        let transport = llm_protocol_provider::DefaultHttpTransport::new();
        let mut stream = provider.stream_chat(&transport, request).await.unwrap();
        let mut events = Vec::new();

        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }

        // 应该包含错误事件
        let has_error = events
            .iter()
            .any(|e| matches!(e, StreamEvent::Error { .. }));
        assert!(has_error, "Should have error event");
    }

    #[tokio::test]
    async fn test_mock_provider_flaky() {
        let provider = MockProviderFactory::create_flaky(3);
        let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o");

        let transport = llm_protocol_provider::DefaultHttpTransport::new();

        // 前 3 次失败
        for i in 1..=3 {
            let result = provider.stream_chat(&transport, request.clone()).await;
            assert!(result.is_err(), "Request {} should fail", i);
            assert_eq!(provider.failure_count(), i);
        }

        // 第 4 次成功
        let result = provider.stream_chat(&transport, request).await;
        assert!(result.is_ok(), "Request 4 should succeed");
    }

    #[tokio::test]
    async fn test_mock_provider_delayed() {
        let provider = MockProviderFactory::create_delayed(100);
        let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o");

        let transport = llm_protocol_provider::DefaultHttpTransport::new();

        let start = std::time::Instant::now();
        let _ = provider.stream_chat(&transport, request).await;
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(100), "Should have delay");
    }

    #[tokio::test]
    async fn test_mock_provider_timeout() {
        let provider = MockProviderFactory::create_timeout();
        let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o");

        let transport = llm_protocol_provider::DefaultHttpTransport::new();
        let result = provider.stream_chat(&transport, request).await;

        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("timeout"));
        }
    }
}
