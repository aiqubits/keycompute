//! Anthropic SSE 流解析
//!
//! 将 Anthropic Messages API 的 SSE 流解析为标准化的 StreamEvent

use futures::{Stream, StreamExt};
use keycompute_types::{KeyComputeError, Result};
use llm_protocol_provider::ByteStream;
use llm_protocol_provider::StreamEvent;
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
    /// 是否已收到 message_stop。EOF 时据此判断截断：
    /// 只有 message_stop 才是 Anthropic 的正常终止信号。
    done_received: bool,
}

/// 解析 Anthropic SSE 流
///
/// 将 HTTP 传输层的字节流转换为标准化的 StreamEvent 流
pub fn parse_anthropic_stream(
    stream: ByteStream,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    parse_anthropic_stream_with_raw(stream, false)
}

/// 解析 Anthropic SSE 流，并可选择保留原始事件。
///
/// 原始事件仅供原生 `/v1/messages` 入站的同协议回写使用；标准化事件仍会
/// 同时产生，以便网关继续统计 usage、完成状态和 Provider 健康。
pub fn parse_anthropic_stream_with_raw(
    stream: ByteStream,
    preserve_raw: bool,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

    tokio::spawn(async move {
        // 必须保留未完成 UTF-8 序列的原始字节。网络 chunk 可以恰好截断一
        // 个多字节字符；若对每个 chunk 使用 from_utf8_lossy，会把合法内容
        // 永久替换为 U+FFFD。
        let mut buffer = Vec::new();
        let mut stream = stream;
        let mut state = StreamState::default();
        let mut event_name: Option<String> = None;
        let mut data_lines = Vec::new();

        loop {
            // If the downstream receiver is dropped (client disconnect or
            // executor timeout), cancel the detached parser task while it is
            // waiting on the upstream body so the HTTP connection is released
            // promptly instead of waiting for the transport stream timeout.
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
                                        "Anthropic stream contained invalid UTF-8: {error}"
                                    ))))
                                    .await;
                                return;
                            }
                        };

                        if !handle_complete_line(
                            &tx,
                            line,
                            &mut event_name,
                            &mut data_lines,
                            preserve_raw,
                            &mut state,
                        )
                        .await
                        {
                            return;
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

        // 处理 EOF 前未以换行结尾的剩余行。TCP 关闭不保证会补上 LF；剩余
        // 内容必须逐行解析（而不是把整个 buffer 当作一行），否则 `event:`
        // 与 `data:` 组成的最后帧会丢失 data 行，message_stop 永远发不出
        // Done，正常完成被误判为截断并导致计费状态错误。
        if !buffer.is_empty() {
            let remaining = std::mem::take(&mut buffer);
            let text = match std::str::from_utf8(&remaining) {
                Ok(text) => text,
                Err(error) => {
                    let _ = tx
                        .send(Err(KeyComputeError::ProviderError(format!(
                            "Anthropic stream contained invalid UTF-8: {error}"
                        ))))
                        .await;
                    return;
                }
            };
            for raw_line in text.split('\n') {
                if !handle_complete_line(
                    &tx,
                    raw_line.trim_end_matches('\r'),
                    &mut event_name,
                    &mut data_lines,
                    preserve_raw,
                    &mut state,
                )
                .await
                {
                    return;
                }
            }
        }

        // 上游在事件分隔空行之前断开时，仍应处理已经完整接收的 data；但绝不
        // 把 EOF 伪装成 Done。只有 message_stop 才是 Anthropic 的正常终止信号。
        if !data_lines.is_empty() {
            let data = data_lines.join("\n");
            let _ = dispatch_anthropic_event(
                &tx,
                &data,
                event_name.as_deref(),
                preserve_raw,
                &mut state,
            )
            .await;
        }

        // 与 OpenAI 解析器对称：EOF 本身不是成功的流完成。只有显式的
        // message_stop 才证明响应完整；否则向调用方显式上报截断，避免
        // 部分响应被当作成功计费。
        if !state.done_received {
            let _ = tx
                .send(Err(KeyComputeError::ProviderError(
                    "Anthropic stream ended without a terminal message_stop marker".to_string(),
                )))
                .await;
        }
    });

    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// 处理单条完整 SSE 行（不含换行符与尾随 `\r`）；返回 `false` 表示应停止解析。
async fn handle_complete_line(
    tx: &mpsc::Sender<Result<StreamEvent>>,
    line: &str,
    event_name: &mut Option<String>,
    data_lines: &mut Vec<String>,
    preserve_raw: bool,
    state: &mut StreamState,
) -> bool {
    if line.is_empty() {
        // SSE 的空行会结束当前事件；即使没有 data，也不能让
        // event 名称泄漏到下一帧。
        let raw_event_name = event_name.take();
        if data_lines.is_empty() {
            return true;
        }
        let data = data_lines.join("\n");
        data_lines.clear();
        return dispatch_anthropic_event(tx, &data, raw_event_name.as_deref(), preserve_raw, state)
            .await;
    }

    if let Some(name) = line.strip_prefix("event:") {
        // SSE 允许 event 名两侧空白；trim 而非 trim_start 保证 `event: error `
        // （尾随空格）等变体仍能被上层按名称精确识别（如错误帧判定）。
        *event_name = Some(name.trim().to_string());
    } else if let Some(data) = line.strip_prefix("data:") {
        // SSE 的 data 字段可分多行；仅移除可选的一个空格。
        let data = data.strip_prefix(' ').unwrap_or(data);
        if data.trim().is_empty() {
            // `data:` 与 `data: ` 空行不构成事件，忽略（与 provider 层
            // `sse::parse_sse_line` 的语义一致），避免空 JSON 中断整条流。
            return true;
        }
        if data.trim() == "[DONE]" {
            // 部分兼容网关会在 message_stop 之外追加 OpenAI 风格的 [DONE]
            // 终止标记。Anthropic 协议不使用它；忽略而非当作非法 JSON，
            // 否则会在 message_stop 尚未到达时错误中断流（与旧解析器的
            // is_done_marker 防御一致）。
            return true;
        }
        data_lines.push(data.to_string());
    }
    true
}

/// 将原始 Anthropic SSE 数据包装为通用 Raw 事件。
///
/// `event` 取自 data 中的 type 字段；官方要求两者一致，因此这能在不依赖
/// 上游是否发送 `event:` 行的情况下保留正确的事件名称。
async fn dispatch_anthropic_event(
    tx: &mpsc::Sender<Result<StreamEvent>>,
    data: &str,
    event_name: Option<&str>,
    preserve_raw: bool,
    state: &mut StreamState,
) -> bool {
    let mut events = if preserve_raw {
        match raw_anthropic_event(data, event_name) {
            Ok(event) => vec![event],
            Err(error) => {
                let _ = tx.send(Err(error)).await;
                return false;
            }
        }
    } else {
        Vec::new()
    };

    match parse_anthropic_event(data, state) {
        Ok(parsed_events) => events.extend(parsed_events),
        Err(error) => {
            let _ = tx.send(Err(error)).await;
            return false;
        }
    }
    for event in events {
        if tx.send(Ok(event)).await.is_err() {
            return false;
        }
    }
    true
}

fn raw_anthropic_event(data: &str, event_name: Option<&str>) -> Result<StreamEvent> {
    let body: serde_json::Value = serde_json::from_str(data).map_err(|e| {
        KeyComputeError::ProviderError(format!("Failed to parse Anthropic stream event: {}", e))
    })?;
    let body_type = body
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            KeyComputeError::ProviderError("Anthropic stream event missing type".into())
        })?;
    let event = event_name
        .filter(|name| !name.is_empty())
        .unwrap_or(body_type);
    Ok(StreamEvent::raw(
        serde_json::json!({
            "kind": "anthropic_sse", "event": event, "data": body
        })
        .to_string(),
    ))
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
            // 记录输入 token 数，供后续 message_delta 合并为最终 Usage；同时
            // 立即传递精确输入值，以便在 message_delta 前断流时仍能准确计费。
            state.input_tokens = message.usage.total_input_tokens()?;
            Ok(vec![StreamEvent::input_usage(state.input_tokens)])
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
                // 部分兼容实现会在 message_delta 中仅重复常规 input_tokens，
                // 而不重复 message_start 已报告的 prompt-cache 计数。保留两者
                // 中较大的已知总输入，避免流末事件将 cache token 覆盖掉。
                let reported_input_tokens = usage.total_input_tokens()?;
                let input_tokens = state.input_tokens.max(reported_input_tokens);
                events.push(StreamEvent::usage(input_tokens, usage.output_tokens));
            }

            Ok(events)
        }
        AnthropicStreamEvent::MessageStop => {
            // 消息结束
            state.done_received = true;
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
    fn test_parse_anthropic_event_message_start() {
        let data = r#"{"type": "message_start", "message": {"id": "msg_01XgYhR8f4h3n7sY3R4j4V3d", "type": "message", "role": "assistant", "model": "claude-3-5-sonnet-20241022", "usage": {"input_tokens": 10, "output_tokens": 0}}}"#;
        let mut state = StreamState::default();
        let events = parse_anthropic_event(data, &mut state).unwrap();

        // 输入 token 在 message_start 已经精确可用，但不能把尚未知晓的 output
        // 伪造成 0；最终 message_delta 会发出完整 Usage 覆盖它。
        assert!(matches!(
            events.as_slice(),
            [StreamEvent::InputUsage { input_tokens: 10 }]
        ));
        assert_eq!(state.input_tokens, 10);
    }

    #[test]
    fn message_start_counts_prompt_cache_tokens_as_input_usage() {
        let data = r#"{"type":"message_start","message":{"id":"msg_01","type":"message","role":"assistant","model":"claude-test","usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"output_tokens":0}}}"#;
        let mut state = StreamState::default();
        let events = parse_anthropic_event(data, &mut state).unwrap();

        assert!(matches!(
            events.as_slice(),
            [StreamEvent::InputUsage { input_tokens: 60 }]
        ));
        assert_eq!(state.input_tokens, 60);
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
        let mut state = StreamState {
            input_tokens: 10,
            ..StreamState::default()
        };
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
        let mut state = StreamState {
            input_tokens: 10,
            ..StreamState::default()
        };
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

        // 验证事件序列：精确 input usage + delta + delta + finish_reason delta +
        // 合并后的最终 usage + done。
        assert!(events.len() >= 5);
        assert!(matches!(
            &events[0],
            StreamEvent::InputUsage { input_tokens: 10 }
        ));
        assert!(matches!(&events[1], StreamEvent::Delta { content, .. } if content == "Hello"));
        assert!(matches!(&events[2], StreamEvent::Delta { content, .. } if content == " World"));
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

    #[tokio::test]
    async fn preserves_named_raw_events_for_native_messages_ingress() {
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"\"}}\n\n",
        ))]);
        let mut parsed = parse_anthropic_stream_with_raw(Box::pin(source), true);
        let raw = parsed.next().await.unwrap().unwrap();
        let StreamEvent::Raw { data } = raw else {
            panic!("first event must preserve the native SSE event");
        };
        let envelope: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(envelope["kind"], "anthropic_sse");
        assert_eq!(envelope["event"], "content_block_delta");
        assert_eq!(envelope["data"]["delta"]["type"], "input_json_delta");
    }

    #[tokio::test]
    async fn parses_multiline_data_and_preserves_sse_event_name() {
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"event: custom_name\ndata: {\"type\":\"message_stop\"\ndata: }\n\n",
        ))]);
        let events = parse_anthropic_stream_with_raw(Box::pin(source), true)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 2);
        let StreamEvent::Raw { data } = events[0].as_ref().unwrap() else {
            panic!("first event must be raw");
        };
        let envelope: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(envelope["event"], "custom_name");
        assert!(matches!(events[1].as_ref().unwrap(), StreamEvent::Done));
    }

    #[tokio::test]
    async fn eof_before_message_stop_does_not_emit_done() {
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        ))]);
        let events = parse_anthropic_stream(Box::pin(source))
            .collect::<Vec<_>>()
            .await;
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, Ok(StreamEvent::Done)))
        );
        assert!(
            matches!(events.first(), Some(Ok(StreamEvent::Delta { content, .. })) if content == "partial")
        );
        // 截断必须以错误显式上报，与 OpenAI 解析器保持一致的完成语义。
        assert!(matches!(
            events.last(),
            Some(Err(KeyComputeError::ProviderError(message)))
                if message.contains("message_stop")
        ));
    }

    #[tokio::test]
    async fn empty_event_does_not_leak_name_to_next_data_frame() {
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"event: ping\n\ndata: {\"type\":\"message_stop\"}\n\n",
        ))]);
        let events = parse_anthropic_stream_with_raw(Box::pin(source), true)
            .collect::<Vec<_>>()
            .await;
        let StreamEvent::Raw { data } = events[0].as_ref().unwrap() else {
            panic!("first event must preserve the data frame");
        };
        let envelope: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(envelope["event"], "message_stop");
        assert!(matches!(events[1].as_ref().unwrap(), StreamEvent::Done));
    }

    #[tokio::test]
    async fn trims_trailing_whitespace_from_event_names() {
        // `event: error `（尾随空格）必须被 trim 为 "error"；否则上层按名称精确
        // 匹配错误帧（event_name == "error"）会失效，只能依赖 data.type 兜底。
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"event: error \ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"boom\"}}\n\n",
        ))]);
        let events = parse_anthropic_stream_with_raw(Box::pin(source), true)
            .collect::<Vec<_>>()
            .await;
        let StreamEvent::Raw { data } = events[0].as_ref().unwrap() else {
            panic!("first event must preserve the native SSE event");
        };
        let envelope: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(envelope["event"], "error");
        assert_eq!(envelope["data"]["error"]["message"], "boom");
    }

    #[tokio::test]
    async fn preserves_utf8_characters_split_across_network_chunks() {
        let source = stream::iter(vec![
            Ok(bytes::Bytes::from_static(
                b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"\xe4\xbd",
            )),
            Ok(bytes::Bytes::from_static(b"\xa0\"}}\n\n")),
        ]);
        let events = parse_anthropic_stream(Box::pin(source))
            .collect::<Vec<_>>()
            .await;
        assert!(
            matches!(events.first(), Some(Ok(StreamEvent::Delta { content, .. })) if content == "你")
        );
    }

    #[tokio::test]
    async fn parses_message_stop_without_a_final_newline() {
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"data:{\"type\":\"message_stop\"}",
        ))]);
        let events = parse_anthropic_stream(Box::pin(source))
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(events.as_slice(), [Ok(StreamEvent::Done)]));
    }

    #[tokio::test]
    async fn parses_event_data_terminal_frame_without_final_newline() {
        // 最后一帧由 event: 与 data: 两行组成且没有最终换行符时，两行都
        // 必须被解析；message_stop 的 data 行不能丢失，Done 必须发出。
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}",
        ))]);
        let events = parse_anthropic_stream_with_raw(Box::pin(source), true)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 2);
        let StreamEvent::Raw { data } = events[0].as_ref().unwrap() else {
            panic!("first event must preserve the native SSE event");
        };
        let envelope: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(envelope["event"], "message_stop");
        assert!(matches!(events[1].as_ref().unwrap(), StreamEvent::Done));
    }

    #[tokio::test]
    async fn done_marker_frame_is_ignored_after_message_stop() {
        // 兼容网关会在 message_stop 之后追加 OpenAI 风格的 `data: [DONE]`。
        // 必须忽略它而不是当作非法 JSON 中断流：message_stop 已产生 Done，
        // 修复前该帧会让整条流以解析错误终止。
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"data: {\"type\":\"message_stop\"}\n\ndata: [DONE]\n\n",
        ))]);
        let events = parse_anthropic_stream(Box::pin(source))
            .collect::<Vec<_>>()
            .await;
        assert!(
            matches!(events.as_slice(), [Ok(StreamEvent::Done)]),
            "extra [DONE] frame must not tear down the stream"
        );
    }

    #[tokio::test]
    async fn done_marker_frame_before_message_stop_does_not_interrupt() {
        // 极端兼容实现可能在 message_stop 之前发送 [DONE]；忽略该帧后流必须
        // 继续等待真正的 message_stop，而不是立即以解析错误中断。
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"data: [DONE]\n\ndata: {\"type\":\"message_stop\"}\n\n",
        ))]);
        let events = parse_anthropic_stream(Box::pin(source))
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(events.as_slice(), [Ok(StreamEvent::Done)]));
    }

    #[tokio::test]
    async fn empty_data_frames_are_ignored() {
        // `data:` 与 `data: ` 空帧不构成事件（与 provider 层 parse_sse_line
        // 一致）；修复前它们会被当作空 JSON 解析并中断整条流。
        let source = stream::iter(vec![Ok(bytes::Bytes::from_static(
            b"data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\ndata:\n\ndata: \n\ndata: {\"type\":\"message_stop\"}\n\n",
        ))]);
        let events = parse_anthropic_stream(Box::pin(source))
            .collect::<Vec<_>>()
            .await;
        assert!(
            events.iter().all(Result::is_ok),
            "empty data frames must not produce parse errors"
        );
        assert!(matches!(
            events.as_slice(),
            [Ok(StreamEvent::InputUsage { .. }), Ok(StreamEvent::Done)]
        ));
    }

    #[tokio::test]
    async fn drops_upstream_when_receiver_is_dropped() {
        let dropped = Arc::new(AtomicBool::new(false));
        let source = PendingDropStream {
            dropped: Arc::clone(&dropped),
        };
        let parsed = parse_anthropic_stream(Box::pin(source));
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
