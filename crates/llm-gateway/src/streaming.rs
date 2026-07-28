//! Streaming Pipeline
//!
//! 流处理管道：chunk 转发、token 累积、SSE 编码。

use llm_protocol_provider::StreamEvent;
use uuid::Uuid;

/// 流处理上下文
#[derive(Debug)]
pub struct StreamingContext {
    /// 请求 ID
    pub request_id: Uuid,
    /// 已发送的 chunk 数
    pub chunks_sent: u64,
    /// 累积的 token 数
    pub tokens_accumulated: u64,
    /// 是否已完成
    pub completed: bool,
}

impl StreamingContext {
    /// 创建新的流上下文
    pub fn new(request_id: Uuid) -> Self {
        Self {
            request_id,
            chunks_sent: 0,
            tokens_accumulated: 0,
            completed: false,
        }
    }

    /// 记录 chunk 发送
    pub fn record_chunk(&mut self, tokens: u32) {
        self.chunks_sent += 1;
        self.tokens_accumulated += tokens as u64;
    }

    /// 标记完成
    pub fn mark_completed(&mut self) {
        self.completed = true;
    }
}

/// 流处理管道
#[derive(Debug)]
pub struct StreamPipeline {
    context: StreamingContext,
}

impl StreamPipeline {
    /// 创建新的流管道
    pub fn new(request_id: Uuid) -> Self {
        Self {
            context: StreamingContext::new(request_id),
        }
    }

    /// 处理流事件
    pub fn process_event(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::Delta { content, .. } => {
                let tokens = Self::estimate_tokens(content);
                self.context.record_chunk(tokens);
            }
            StreamEvent::Done => {
                self.context.mark_completed();
                tracing::debug!(
                    request_id = %self.context.request_id,
                    chunks = self.context.chunks_sent,
                    tokens = self.context.tokens_accumulated,
                    "Stream completed"
                );
            }
            _ => {}
        }
    }

    /// 获取上下文
    pub fn context(&self) -> &StreamingContext {
        &self.context
    }

    /// 精确计算 token 数（使用 tiktoken-rs）
    fn estimate_tokens(content: &str) -> u32 {
        if content.is_empty() {
            return 0;
        }
        let bpe = tiktoken_rs::o200k_base_singleton();
        bpe.encode_with_special_tokens(content).len() as u32
    }
}

/// SSE 编码器
pub struct SseEncoder;

impl SseEncoder {
    /// 将 StreamEvent 编码为 SSE 格式
    pub fn encode(event: &StreamEvent) -> String {
        match event {
            StreamEvent::Delta {
                content,
                finish_reason,
            } => {
                let data = serde_json::json!({
                    "content": content,
                    "finish_reason": finish_reason,
                });
                format!("data: {}\n\n", data)
            }
            StreamEvent::Done => "data: [DONE]\n\n".to_string(),
            StreamEvent::Error { message } => {
                format!("data: {{\"error\": \"{}\"}}\n\n", message)
            }
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_context() {
        let mut ctx = StreamingContext::new(Uuid::new_v4());
        assert_eq!(ctx.chunks_sent, 0);

        ctx.record_chunk(10);
        assert_eq!(ctx.chunks_sent, 1);
        assert_eq!(ctx.tokens_accumulated, 10);

        ctx.mark_completed();
        assert!(ctx.completed);
    }

    #[test]
    fn test_stream_pipeline() {
        let mut pipeline = StreamPipeline::new(Uuid::new_v4());

        let event = StreamEvent::Delta {
            content: "Hello".to_string(),
            finish_reason: None,
        };
        pipeline.process_event(&event);

        assert_eq!(pipeline.context().chunks_sent, 1);
    }

    #[test]
    fn test_sse_encoder() {
        let event = StreamEvent::Delta {
            content: "Hello".to_string(),
            finish_reason: None,
        };
        let encoded = SseEncoder::encode(&event);
        assert!(encoded.starts_with("data: "));

        let done = SseEncoder::encode(&StreamEvent::Done);
        assert!(done.contains("[DONE]"));
    }
}
