//! 标准化流事件类型
//!
//! 定义从 Provider 返回的流事件标准化格式

use serde::{Deserialize, Serialize};

/// 流事件枚举
///
/// 标准化的流事件类型，各 Provider Adapter 负责将各自格式转换为此格式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all = "snake_case")]
pub enum StreamEvent {
    /// 内容增量
    Delta {
        /// 增量内容
        content: String,
        /// 结束原因（可选）
        finish_reason: Option<String>,
    },
    /// Provider 报告的用量（流结束时）
    Usage {
        /// 输入 token 数
        input_tokens: u32,
        /// 输出 token 数
        output_tokens: u32,
    },
    /// 流结束
    Done,
    /// 错误
    Error {
        /// 错误消息
        message: String,
    },
    /// Provider 特定的事件（透传）
    Raw {
        /// 原始事件数据
        data: String,
    },
}

impl StreamEvent {
    /// 创建 Delta 事件
    pub fn delta(content: impl Into<String>) -> Self {
        Self::Delta {
            content: content.into(),
            finish_reason: None,
        }
    }

    /// 创建带结束原因的 Delta 事件
    pub fn delta_with_finish(content: impl Into<String>, finish_reason: impl Into<String>) -> Self {
        Self::Delta {
            content: content.into(),
            finish_reason: Some(finish_reason.into()),
        }
    }

    /// 创建 Usage 事件
    pub fn usage(input_tokens: u32, output_tokens: u32) -> Self {
        Self::Usage {
            input_tokens,
            output_tokens,
        }
    }

    /// 创建 Done 事件
    pub fn done() -> Self {
        Self::Done
    }

    /// 创建 Error 事件
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    /// 创建 Raw 事件
    pub fn raw(data: impl Into<String>) -> Self {
        Self::Raw { data: data.into() }
    }

    /// 检查是否是 Done 事件
    pub fn is_done(&self) -> bool {
        matches!(self, Self::Done)
    }

    /// 检查是否是 Error 事件
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// 获取错误消息（如果是 Error 事件）
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Error { message } => Some(message),
            _ => None,
        }
    }
}

/// SSE (Server-Sent Events) 解析工具
pub mod sse {
    /// 解析 SSE 数据行
    ///
    /// SSE 格式: `data: {...}\n\n`
    pub fn parse_sse_line(line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        if let Some(data) = line.strip_prefix("data: ") {
            let data = data.trim();
            if data == "[DONE]" {
                return Some(String::from("[DONE]"));
            }
            return Some(data.to_string());
        }

        // 忽略其他字段（id, event, retry 等）
        None
    }

    /// 检查是否是流结束标记
    pub fn is_done_marker(data: &str) -> bool {
        data.trim() == "[DONE]"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_helpers() {
        let delta = StreamEvent::delta("Hello");
        assert!(matches!(delta, StreamEvent::Delta { content, .. } if content == "Hello"));

        let usage = StreamEvent::usage(10, 20);
        assert!(matches!(
            usage,
            StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 20
            }
        ));

        let done = StreamEvent::done();
        assert!(done.is_done());

        let error = StreamEvent::error("Something went wrong");
        assert!(error.is_error());
        assert_eq!(error.error_message(), Some("Something went wrong"));
    }

    #[test]
    fn test_sse_parse() {
        use sse::parse_sse_line;

        assert_eq!(
            parse_sse_line("data: {\"content\": \"Hello\"}"),
            Some(String::from("{\"content\": \"Hello\"}"))
        );

        assert_eq!(parse_sse_line("data: [DONE]"), Some(String::from("[DONE]")));

        assert_eq!(parse_sse_line("id: 123"), None);
        assert_eq!(parse_sse_line(""), None);
        assert_eq!(parse_sse_line("event: message"), None);
    }

    #[test]
    fn test_sse_is_done_marker() {
        assert!(sse::is_done_marker("[DONE]"));
        assert!(sse::is_done_marker("  [DONE]  "));
        assert!(!sse::is_done_marker("{\"content\": \"Hello\"}"));
    }
}
