//! OpenAI SSE 流解析
//!
//! 将 OpenAI 的 SSE 流解析为标准化的 StreamEvent

use futures::{Stream, StreamExt};
use keycompute_types::{KeyComputeError, Result};
use llm_protocol_provider::ByteStream;
use llm_protocol_provider::StreamEvent;
use llm_protocol_provider::stream::sse;
use std::pin::Pin;
use tokio::sync::mpsc;

use crate::protocol::OpenAIStreamResponse;

/// 解析 OpenAI SSE 流
///
/// 将 HTTP 传输层的字节流转换为标准化的 StreamEvent 流
pub fn parse_openai_stream(
    stream: ByteStream,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

    tokio::spawn(async move {
        // Keep the raw bytes until a complete SSE line is available. Network
        // chunks may split a multi-byte UTF-8 character; decoding each chunk
        // with `from_utf8_lossy` would permanently replace such characters
        // with U+FFFD before the next chunk arrives.
        let mut buffer = Vec::new();
        let mut stream = stream;

        loop {
            // Dropping the receiver (for example after a client disconnect or
            // executor timeout) must also stop the producer and release the
            // underlying HTTP body. Without this branch, a detached parser
            // task can remain blocked in `stream.next()` until the transport's
            // much longer stream timeout expires.
            let Some(chunk_result) = (tokio::select! {
                _ = tx.closed() => return,
                chunk = stream.next() => chunk,
            }) else {
                break;
            };
            match chunk_result {
                Ok(chunk) => {
                    buffer.extend_from_slice(&chunk);

                    // 处理缓冲区中的完整行
                    while let Some(pos) = buffer.iter().position(|byte| *byte == b'\n') {
                        let mut line = buffer.drain(..=pos).collect::<Vec<_>>();

                        // 处理可能的 \r\n
                        line.pop(); // `\n`
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        let line = match std::str::from_utf8(&line) {
                            Ok(line) => line,
                            Err(error) => {
                                let _ = tx
                                    .send(Err(KeyComputeError::ProviderError(format!(
                                        "OpenAI stream contained invalid UTF-8: {error}"
                                    ))))
                                    .await;
                                return;
                            }
                        };

                        if !handle_sse_line(&tx, line).await {
                            return;
                        }
                    }
                }
                Err(e) => {
                    // Preserve structured transport/body-read failures. The
                    // executor separately normalizes parser-generated legacy
                    // ProviderError values as non-retryable protocol failures.
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }

        // 处理 EOF 前未以换行结尾的剩余字节。上游可以在 `data: [DONE]`
        // 之后直接关闭 TCP 而不补 LF；把剩余内容按行解析后再判断是否截断，
        // 否则携带完成标记的响应会被误判为截断。
        if !buffer.is_empty() {
            let remaining = std::mem::take(&mut buffer);
            let text = match std::str::from_utf8(&remaining) {
                Ok(text) => text,
                Err(error) => {
                    let _ = tx
                        .send(Err(KeyComputeError::ProviderError(format!(
                            "OpenAI stream contained invalid UTF-8: {error}"
                        ))))
                        .await;
                    return;
                }
            };
            for raw_line in text.split('\n') {
                if !handle_sse_line(&tx, raw_line.trim_end_matches('\r')).await {
                    return;
                }
            }
        }

        // EOF alone is not a successful OpenAI stream completion. A proxy or
        // upstream can close the connection after a partial response; only
        // the protocol's explicit `[DONE]` marker proves that the response
        // completed.
        let _ = tx
            .send(Err(KeyComputeError::ProviderError(
                "OpenAI stream ended without a terminal [DONE] marker".to_string(),
            )))
            .await;
    });

    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// 处理单条 SSE 行；返回 `false` 表示应停止解析（完成 / 错误 / 接收端关闭）。
async fn handle_sse_line(tx: &mpsc::Sender<Result<StreamEvent>>, line: &str) -> bool {
    let Some(data) = sse::parse_sse_line(line) else {
        return true;
    };

    if sse::is_done_marker(&data) {
        let _ = tx.send(Ok(StreamEvent::done())).await;
        return false;
    }

    // 解析 JSON 数据（一条上游事件可能产生多个 StreamEvent）
    match parse_openai_event(&data) {
        Ok(events) => {
            for event in events {
                if tx.send(Ok(event)).await.is_err() {
                    // 接收端已关闭（客户端断开），停止解析
                    return false;
                }
            }
            true
        }
        Err(e) => {
            let _ = tx.send(Err(e)).await;
            false
        }
    }
}

/// 解析 OpenAI 流事件 JSON
///
/// 一条上游事件可能产生 0~2 个 StreamEvent：部分上游（如 DeepSeek）
/// 会在最后一个 chunk 中同时携带 finish_reason 与 usage，
/// 先发 Delta（含 finish_reason）再发 Usage，避免两者互相吞掉
fn parse_openai_event(data: &str) -> Result<Vec<StreamEvent>> {
    // 先解析为通用 JSON：需要识别上游在流中发送的错误 payload
    //（`{"error": {...}}`，限流/内容过滤时常见），
    // 否则会因结构不匹配报 serde 错误，掩盖真实的上游错误信息
    let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
        KeyComputeError::ProviderError(format!("Failed to parse OpenAI stream event: {}", e))
    })?;

    if let Some(message) = extract_upstream_error(&value) {
        return Ok(vec![StreamEvent::error(message)]);
    }

    let response: OpenAIStreamResponse = serde_json::from_value(value).map_err(|e| {
        KeyComputeError::ProviderError(format!("Failed to parse OpenAI stream event: {}", e))
    })?;

    let mut events = Vec::new();

    // 先处理内容增量 / 结束原因
    if let Some(choice) = response.choices.first() {
        let delta = &choice.delta;

        if let Some(content) = &delta.content {
            events.push(StreamEvent::Delta {
                content: content.clone(),
                finish_reason: choice.finish_reason.clone(),
            });
        } else if choice.finish_reason.is_some() {
            events.push(StreamEvent::Delta {
                content: String::new(),
                finish_reason: choice.finish_reason.clone(),
            });
        }
        // 纯角色消息（首条 role-only delta）不产生事件
    }

    // 再补发用量信息（通常在流结束时）
    if let Some(usage) = response.usage {
        events.push(StreamEvent::usage(
            usage.prompt_tokens as u32,
            usage.completion_tokens as u32,
        ));
    }

    Ok(events)
}

/// 提取上游错误 payload 中的错误信息
///
/// OpenAI 协议的错误结构为 `{"error": {"message": ..., "type"/"code": ...}}`，
/// 部分兼容上游会把 error 直接放字符串（`{"error": "..."}`）
fn extract_upstream_error(value: &serde_json::Value) -> Option<String> {
    let error = value.get("error")?;
    // 部分上游在正常 chunk 中携带 `"error": null`，不应误判为错误
    if error.is_null() {
        return None;
    }
    if let Some(message) = error.as_str() {
        return Some(format!("Upstream stream error: {}", message));
    }
    let message = error
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown error");
    let kind = error
        .get("type")
        .or_else(|| error.get("code"))
        .and_then(|t| t.as_str());
    Some(match kind {
        Some(kind) => format!("Upstream stream error ({}): {}", kind, message),
        None => format!("Upstream stream error: {}", message),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::task::{Context, Poll};

    struct PendingDropStream {
        dropped: Arc<AtomicBool>,
    }

    impl futures::Stream for PendingDropStream {
        type Item = keycompute_types::Result<bytes::Bytes>;

        fn poll_next(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Pending
        }
    }

    impl Drop for PendingDropStream {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_parse_openai_event_with_content() {
        let data = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1694268190,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {"content": "Hello"},
                "finish_reason": null
            }]
        }"#;

        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Delta { content, .. } if content == "Hello"));
    }

    #[test]
    fn test_parse_openai_event_with_usage() {
        let data = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1694268190,
            "model": "gpt-4o",
            "choices": [],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        }"#;

        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 20
            }
        ));
    }

    #[test]
    fn test_parse_openai_event_finish() {
        let data = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1694268190,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        }"#;

        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], StreamEvent::Delta { content, finish_reason: Some(reason) }
            if content.is_empty() && reason == "stop")
        );
    }

    #[test]
    fn test_parse_openai_event_role_only_ignored() {
        // 首条 role-only delta 不产生事件
        let data = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1694268190,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant"},
                "finish_reason": null
            }]
        }"#;

        let events = parse_openai_event(data).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_openai_event_upstream_error_object() {
        // 上游在流中发送的错误 payload 应转为 Error 事件透传真实错误信息，
        // 而非因结构不匹配报 serde 错误中止整条流
        let data = r#"{"error": {"message": "Rate limit reached", "type": "rate_limit_error"}}"#;
        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::Error { message }
                if message.contains("rate_limit_error") && message.contains("Rate limit reached")
        ));
    }

    #[test]
    fn test_parse_openai_event_upstream_error_string() {
        // 部分兼容上游把 error 直接放字符串
        let data = r#"{"error": "upstream overloaded"}"#;
        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::Error { message } if message.contains("upstream overloaded")
        ));
    }

    #[test]
    fn test_parse_openai_event_error_null_not_misjudged() {
        // 部分上游在正常 chunk 中携带 "error": null，不应误判为错误
        let data = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1694268190,
            "model": "gpt-4o",
            "error": null,
            "choices": [{
                "index": 0,
                "delta": {"content": "ok"},
                "finish_reason": null
            }]
        }"#;
        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Delta { content, .. } if content == "ok"));
    }

    #[test]
    fn test_parse_openai_event_tolerates_missing_metadata_fields() {
        // 部分兼容上游（旧版 vLLM/中转代理）的 usage-only 末块
        // 会省略 choices/id/object 等字段，不应因此中断整条流
        let data = r#"{"usage": {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12}}"#;
        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::Usage {
                input_tokens: 5,
                output_tokens: 7
            }
        ));

        // 最小 chunk（无任何内容）安静跳过，不产生事件也不报错
        let events = parse_openai_event("{}").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_openai_event_finish_and_usage_in_same_chunk() {
        // DeepSeek 风格末块：同一 chunk 同时携带 finish_reason 与 usage，
        // 两者都必须上报，usage 不得吞掉 finish_reason
        let data = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1694268190,
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        }"#;

        let events = parse_openai_event(data).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamEvent::Delta { finish_reason: Some(r), .. } if r == "stop"
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 20
            }
        ));
    }

    #[tokio::test]
    async fn preserves_utf8_characters_split_across_network_chunks() {
        // Split the three-byte UTF-8 encoding of "你" between network chunks.
        // The parser must retain the bytes until the complete SSE line arrives.
        let source = futures::stream::iter(vec![
            Ok(bytes::Bytes::from_static(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"\xe4",
            )),
            Ok(bytes::Bytes::from_static(
                b"\xbd\xa0\"}}]}\n\ndata: [DONE]\n\n",
            )),
        ]);
        let mut parsed = parse_openai_stream(Box::pin(source));
        let event = parsed.next().await.unwrap().unwrap();
        assert!(matches!(
            event,
            StreamEvent::Delta { content, .. } if content == "你"
        ));
        assert!(matches!(parsed.next().await, Some(Ok(StreamEvent::Done))));
    }

    #[tokio::test]
    async fn eof_without_done_marker_is_reported_as_truncated() {
        let source = futures::stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        ))]);
        let mut parsed = parse_openai_stream(Box::pin(source));

        assert!(matches!(
            parsed.next().await,
            Some(Ok(StreamEvent::Delta { content, .. })) if content == "partial"
        ));
        assert!(matches!(
            parsed.next().await,
            Some(Err(KeyComputeError::ProviderError(message)))
                if message.contains("[DONE]")
        ));
    }

    #[tokio::test]
    async fn done_marker_without_final_newline_is_accepted() {
        // 上游可以在 `data: [DONE]` 之后直接关闭 TCP 而不补 LF；EOF 时残留
        // 的这一行必须解析为最终 SSE 行，响应才算正常完成而非截断。
        let source = futures::stream::iter(vec![
            Ok(bytes::Bytes::from_static(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
            )),
            Ok(bytes::Bytes::from_static(b"data: [DONE]")),
        ]);
        let mut parsed = parse_openai_stream(Box::pin(source));
        assert!(matches!(
            parsed.next().await,
            Some(Ok(StreamEvent::Delta { content, .. })) if content == "ok"
        ));
        assert!(matches!(parsed.next().await, Some(Ok(StreamEvent::Done))));
        assert!(parsed.next().await.is_none());
    }

    #[tokio::test]
    async fn eof_flushes_buffered_line_before_reporting_truncation() {
        // EOF 时残留的完整 data 行（非 [DONE]）必须先作为事件上报，
        // 再以缺少 [DONE] 标记判定截断，不能静默丢弃缓冲内容。
        let source = futures::stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"last\"}}]}",
        ))]);
        let mut parsed = parse_openai_stream(Box::pin(source));
        assert!(matches!(
            parsed.next().await,
            Some(Ok(StreamEvent::Delta { content, .. })) if content == "tail"
        ));
        assert!(matches!(
            parsed.next().await,
            Some(Ok(StreamEvent::Delta { content, .. })) if content == "last"
        ));
        assert!(matches!(
            parsed.next().await,
            Some(Err(KeyComputeError::ProviderError(message))) if message.contains("[DONE]")
        ));
    }

    #[tokio::test]
    async fn drops_upstream_when_receiver_is_dropped() {
        let dropped = Arc::new(AtomicBool::new(false));
        let source = PendingDropStream {
            dropped: Arc::clone(&dropped),
        };
        let parsed = parse_openai_stream(Box::pin(source));
        tokio::task::yield_now().await;
        drop(parsed);
        for _ in 0..10 {
            tokio::task::yield_now().await;
            if dropped.load(Ordering::SeqCst) {
                break;
            }
        }
        assert!(dropped.load(Ordering::SeqCst));
    }
}
