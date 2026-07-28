//! Anthropic SSE 流解析
//!
//! 将 Anthropic Messages API 的 SSE 流解析为标准化的 StreamEvent

use futures::{Stream, StreamExt};
use keycompute_types::{KeyComputeError, Result};
use llm_protocol_provider::ByteStream;
use llm_protocol_provider::StreamEvent;
use llm_protocol_provider::stream::sse;
use std::pin::Pin;
use tokio::sync::mpsc;

use crate::protocol::AnthropicStreamEvent;

/// 流解析状态
///
/// Anthropic 的 usage 分两次下发：`message_start` 携带 input_tokens，
/// `message_delta` 携带 output_tokens。需要跨事件累积后合并上报，
/// 否则后发的 Usage 事件会把 input_tokens 覆盖为 0，导致计费错误。
#[derive(Debug, Default)]
struct StreamState {
    /// message_start 上报的输入 token 数
    input_tokens: u32,
}

/// 解析 Anthropic SSE 流
///
/// 将 HTTP 传输层的字节流转换为标准化的 StreamEvent 流
pub fn parse_anthropic_stream(
    stream: ByteStream,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

    tokio::spawn(async move {
        let mut buffer = String::new();
        let mut stream = stream;
        let mut state = StreamState::default();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    let text = String::from_utf8_lossy(&chunk);
                    buffer.push_str(&text);

                    // 处理缓冲区中的完整行
                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer[..pos].to_string();
                        buffer.drain(..=pos);

                        // 处理可能的 \r\n
                        let line = line.trim_end_matches('\r');

                        if let Some(data) = sse::parse_sse_line(line) {
                            if sse::is_done_marker(&data) {
                                // Anthropic 不使用 [DONE] 标记，而是使用 message_stop 事件
                                continue;
                            }

                            // 解析 JSON 数据（一条上游事件可能产生多个 StreamEvent）
                            match parse_anthropic_event(&data, &mut state) {
                                Ok(events) => {
                                    for event in events {
                                        if tx.send(Ok(event)).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(Err(e)).await;
                                    return;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(KeyComputeError::ProviderError(e.to_string())))
                        .await;
                    return;
                }
            }
        }

        // 流结束
        let _ = tx.send(Ok(StreamEvent::done())).await;
    });

    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// 解析 Anthropic 流事件 JSON
///
/// 一条上游事件可能产生 0~2 个 StreamEvent（如 message_delta 同时携带
/// stop_reason 与 usage 时，先发 finish_reason Delta 再发 Usage）
fn parse_anthropic_event(data: &str, state: &mut StreamState) -> Result<Vec<StreamEvent>> {
    let event: AnthropicStreamEvent = serde_json::from_str(data).map_err(|e| {
        KeyComputeError::ProviderError(format!("Failed to parse Anthropic stream event: {}", e))
    })?;

    match event {
        AnthropicStreamEvent::MessageStart { message } => {
            // 记录输入 token 数，等 message_delta 拿到 output_tokens 后合并上报
            state.input_tokens = message.usage.input_tokens;
            Ok(Vec::new())
        }
        AnthropicStreamEvent::ContentBlockStart { content_block, .. } => {
            // 内容块开始
            if let crate::protocol::ContentBlock::Text { text } = content_block
                && !text.is_empty()
            {
                return Ok(vec![StreamEvent::delta(text)]);
            }
            Ok(Vec::new())
        }
        AnthropicStreamEvent::ContentBlockDelta { delta, .. } => {
            // 内容增量
            match delta {
                crate::protocol::ContentDelta::TextDelta { text } => {
                    Ok(vec![StreamEvent::delta(text)])
                }
                // 未知增量类型（thinking_delta、input_json_delta 等）忽略
                crate::protocol::ContentDelta::Unknown => Ok(Vec::new()),
            }
        }
        AnthropicStreamEvent::ContentBlockStop { .. } => {
            // 内容块结束，无需特殊处理
            Ok(Vec::new())
        }
        AnthropicStreamEvent::MessageDelta { delta, usage } => {
            let mut events = Vec::new();

            // 先发送停止原因，保证客户端能收到 finish_reason
            if delta.stop_reason.is_some() {
                events.push(StreamEvent::Delta {
                    content: String::new(),
                    finish_reason: delta.stop_reason,
                });
            }

            // 合并 message_start 的 input_tokens 与本事件的 output_tokens 上报
            if let Some(usage) = usage {
                // 部分实现会在 message_delta 中重复下发 input_tokens，优先使用非零值
                let input_tokens = if usage.input_tokens > 0 {
                    usage.input_tokens
                } else {
                    state.input_tokens
                };
                events.push(StreamEvent::usage(input_tokens, usage.output_tokens));
            }

            Ok(events)
        }
        AnthropicStreamEvent::MessageStop => {
            // 消息结束
            Ok(vec![StreamEvent::done()])
        }
        AnthropicStreamEvent::Error { error } => {
            // 错误事件
            Ok(vec![StreamEvent::error(format!(
                "Anthropic API error ({}): {}",
                error.r#type, error.message
            ))])
        }
        AnthropicStreamEvent::Ping => {
            // Ping 事件，忽略
            Ok(Vec::new())
        }
        AnthropicStreamEvent::Unknown => {
            // 未知事件类型（如 thinking 系列），按官方要求优雅忽略
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    #[test]
    fn test_parse_anthropic_event_message_start() {
        let data = r#"{"type": "message_start", "message": {"id": "msg_01XgYhR8f4h3n7sY3R4j4V3d", "type": "message", "role": "assistant", "model": "claude-3-5-sonnet-20241022", "usage": {"input_tokens": 10, "output_tokens": 0}}}"#;
        let mut state = StreamState::default();
        let events = parse_anthropic_event(data, &mut state).unwrap();

        // message_start 不再直接发 Usage，而是记录 input_tokens 待后续合并
        assert!(events.is_empty());
        assert_eq!(state.input_tokens, 10);
    }

    #[test]
    fn test_parse_anthropic_event_text_delta() {
        let data = r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}"#;
        let mut state = StreamState::default();
        let events = parse_anthropic_event(data, &mut state).unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Delta { content, .. } if content == "Hello"));
    }

    #[test]
    fn test_parse_anthropic_event_message_stop() {
        let data = r#"{"type": "message_stop"}"#;
        let mut state = StreamState::default();
        let events = parse_anthropic_event(data, &mut state).unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Done));
    }

    #[test]
    fn test_parse_anthropic_event_error() {
        let data = r#"{"type": "error", "error": {"type": "rate_limit_error", "message": "Rate limit exceeded"}}"#;
        let mut state = StreamState::default();
        let events = parse_anthropic_event(data, &mut state).unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Error { message }
            if message.contains("rate_limit_error")));
    }

    #[test]
    fn test_parse_anthropic_event_message_delta_merges_input_tokens() {
        let data = r#"{"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 50}}"#;
        let mut state = StreamState { input_tokens: 10 };
        let events = parse_anthropic_event(data, &mut state).unwrap();

        // 先 finish_reason Delta，再合并了 input_tokens 的 Usage
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamEvent::Delta { finish_reason: Some(r), .. } if r == "end_turn"
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 50
            }
        ));
    }

    #[test]
    fn test_parse_anthropic_event_message_delta_without_usage() {
        // 上游未下发 usage 时只发 finish_reason Delta，
        // 不产生虚假的零 Usage 事件（计费回落 tiktoken 估算，
        // message_start 记录的 input_tokens 也不会被误报）
        let data = r#"{"type": "message_delta", "delta": {"stop_reason": "end_turn"}}"#;
        let mut state = StreamState { input_tokens: 10 };
        let events = parse_anthropic_event(data, &mut state).unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::Delta { finish_reason: Some(r), .. } if r == "end_turn"
        ));
    }

    #[test]
    fn test_parse_anthropic_event_unknown_ignored() {
        // 未知事件类型（如 thinking 系列）应被忽略而非中断流
        let mut state = StreamState::default();
        let events = parse_anthropic_event(
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "..."}}"#,
            &mut state,
        )
        .unwrap();
        assert!(events.is_empty());

        let events = parse_anthropic_event(
            r#"{"type": "some_future_event", "payload": {"x": 1}}"#,
            &mut state,
        )
        .unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_parse_anthropic_stream() {
        // 模拟 SSE 数据
        let sse_data = vec![
            Ok(bytes::Bytes::from(
                "data: {\"type\": \"message_start\", \"message\": {\"id\": \"msg_01\", \"type\": \"message\", \"role\": \"assistant\", \"model\": \"claude-3-5-sonnet\", \"usage\": {\"input_tokens\": 10, \"output_tokens\": 0}}}\n\n",
            )),
            Ok(bytes::Bytes::from(
                "data: {\"type\": \"content_block_delta\", \"index\": 0, \"delta\": {\"type\": \"text_delta\", \"text\": \"Hello\"}}\n\n",
            )),
            Ok(bytes::Bytes::from(
                "data: {\"type\": \"content_block_delta\", \"index\": 0, \"delta\": {\"type\": \"text_delta\", \"text\": \" World\"}}\n\n",
            )),
            Ok(bytes::Bytes::from(
                "data: {\"type\": \"message_delta\", \"delta\": {\"stop_reason\": \"end_turn\"}, \"usage\": {\"output_tokens\": 2}}\n\n",
            )),
            Ok(bytes::Bytes::from("data: {\"type\": \"message_stop\"}\n\n")),
        ];

        let byte_stream: ByteStream = Box::pin(stream::iter(sse_data));
        let mut stream = parse_anthropic_stream(byte_stream);

        // 收集所有事件
        let mut events = Vec::new();
        while let Some(result) = stream.next().await {
            if let Ok(event) = result {
                events.push(event);
            }
        }

        // 验证事件序列：delta + delta + finish_reason delta + 合并后的 usage + done
        assert!(events.len() >= 4);
        assert!(matches!(&events[0], StreamEvent::Delta { content, .. } if content == "Hello"));
        assert!(matches!(&events[1], StreamEvent::Delta { content, .. } if content == " World"));
        // Usage 事件必须同时携带 message_start 的 input_tokens 与 message_delta 的 output_tokens
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 2
            }
        )));
        assert!(matches!(events.last().unwrap(), StreamEvent::Done));
    }
}
