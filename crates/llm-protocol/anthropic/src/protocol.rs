//! Anthropic Messages API 协议类型
//!
//! Anthropic Messages API 的请求/响应结构定义
//! 文档: https://docs.anthropic.com/claude/reference/messages_post

use serde::{Deserialize, Serialize};

/// Anthropic Messages API 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicRequest {
    /// 模型名称，如 claude-3-5-sonnet-20241022
    pub model: String,
    /// 最大生成 token 数
    pub max_tokens: u32,
    /// 消息列表
    pub messages: Vec<AnthropicMessage>,
    /// 系统提示词（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// 是否流式输出
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 温度参数 (0-1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top P 参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 停止序列
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// 元数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<AnthropicMetadata>,
}

/// Anthropic 消息结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    /// 角色: user, assistant
    pub role: String,
    /// 消息内容（可以是字符串或内容块列表）
    pub content: AnthropicContent,
}

/// Anthropic 内容类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnthropicContent {
    /// 纯文本内容
    Text(String),
    /// 内容块列表（支持多模态）
    Blocks(Vec<ContentBlock>),
}

/// 内容块
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// 文本块
    #[serde(rename = "text")]
    Text { text: String },
    /// 图片块（base64）
    #[serde(rename = "image")]
    Image { source: ImageSource },
    /// 未知块类型（如 thinking、tool_use 等，解析时忽略）
    #[serde(other)]
    Unknown,
}

/// 图片来源
///
/// 支持 base64 内嵌数据与远程 URL 两种形式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ImageSource {
    /// base64 内嵌图片
    #[serde(rename = "base64")]
    Base64 {
        /// 媒体类型: image/jpeg, image/png, image/gif, image/webp
        media_type: String,
        /// base64 编码的数据
        data: String,
    },
    /// 远程 URL 图片
    #[serde(rename = "url")]
    Url {
        /// 图片 URL
        url: String,
    },
}

/// 请求元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMetadata {
    /// 用户标识（用于追踪）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// Anthropic Messages API 响应（非流式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicResponse {
    /// 响应 ID
    pub id: String,
    /// 对象类型: message
    pub r#type: String,
    /// 角色: assistant
    pub role: String,
    /// 模型名称
    pub model: String,
    /// 停止原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// 停止序列（如果是因为 stop_sequence 停止）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// 内容列表
    pub content: Vec<ContentBlock>,
    /// 用量信息
    pub usage: AnthropicUsage,
}

/// 用量信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnthropicUsage {
    /// 输入 token 数
    #[serde(default)]
    pub input_tokens: u32,
    /// 输出 token 数
    #[serde(default)]
    pub output_tokens: u32,
}

/// Anthropic 流式响应事件
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicStreamEvent {
    /// 消息开始
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicStreamMessage },
    /// 内容块开始
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    /// 内容块增量
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: ContentDelta },
    /// 内容块结束
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    /// 消息增量（用量信息）
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaInfo,
        usage: Option<AnthropicUsage>,
    },
    /// 消息结束
    #[serde(rename = "message_stop")]
    MessageStop,
    /// 错误
    #[serde(rename = "error")]
    Error { error: AnthropicError },
    /// Ping（保持连接）
    #[serde(rename = "ping")]
    Ping,
    /// 未知事件类型（如 thinking 系列、未来新增事件）
    ///
    /// Anthropic 官方要求客户端优雅忽略未知事件，
    /// 不能因解析失败而中断整条流
    #[serde(other)]
    Unknown,
}

/// 流式消息信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicStreamMessage {
    /// 消息 ID
    pub id: String,
    /// 对象类型
    pub r#type: String,
    /// 角色
    pub role: String,
    /// 模型
    pub model: String,
    /// 停止原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// 停止序列
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// 用量
    pub usage: AnthropicUsage,
}

/// 内容增量
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentDelta {
    /// 文本增量
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    /// 未知增量类型（如 thinking_delta、input_json_delta 等，忽略）
    #[serde(other)]
    Unknown,
}

/// 消息增量信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageDeltaInfo {
    /// 停止原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// 停止序列
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}

/// Anthropic API 错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicError {
    /// 错误类型
    pub r#type: String,
    /// 错误消息
    pub message: String,
}

impl AnthropicRequest {
    /// 创建新的请求
    pub fn new(model: impl Into<String>, max_tokens: u32) -> Self {
        Self {
            model: model.into(),
            max_tokens,
            messages: Vec::new(),
            system: None,
            stream: None,
            temperature: None,
            top_p: None,
            stop_sequences: None,
            metadata: None,
        }
    }

    /// 添加用户消息
    pub fn add_user_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(AnthropicMessage {
            role: "user".to_string(),
            content: AnthropicContent::Text(content.into()),
        });
        self
    }

    /// 添加助手消息
    pub fn add_assistant_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(AnthropicMessage {
            role: "assistant".to_string(),
            content: AnthropicContent::Text(content.into()),
        });
        self
    }

    /// 设置系统提示词
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// 设置流式输出
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }

    /// 设置温度参数
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// 设置 top_p 参数
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }
}

impl AnthropicMessage {
    /// 创建用户消息
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: AnthropicContent::Text(content.into()),
        }
    }

    /// 创建助手消息
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: AnthropicContent::Text(content.into()),
        }
    }
}

impl AnthropicResponse {
    /// 提取文本内容
    pub fn extract_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_request_serialization() {
        let request = AnthropicRequest::new("claude-3-5-sonnet-20241022", 1024)
            .with_system("You are helpful")
            .add_user_message("Hello")
            .with_stream(true)
            .with_temperature(0.7);

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("claude-3-5-sonnet-20241022"));
        assert!(json.contains("You are helpful"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_anthropic_response_parsing() {
        let json = r#"{
            "id": "msg_01XgYhR8f4h3n7sY3R4j4V3d",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-5-sonnet-20241022",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "Hello! How can I help you today?"}],
            "usage": {"input_tokens": 10, "output_tokens": 20}
        }"#;

        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.role, "assistant");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.extract_text(), "Hello! How can I help you today?");
    }

    #[test]
    fn test_claude_stream_event_parsing() {
        // 消息开始事件
        let json = r#"{"type": "message_start", "message": {"id": "msg_01XgYhR8f4h3n7sY3R4j4V3d", "type": "message", "role": "assistant", "model": "claude-3-5-sonnet-20241022", "usage": {"input_tokens": 10, "output_tokens": 0}}}"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::MessageStart { .. }));

        // 内容增量事件
        let json = r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(
            event,
            AnthropicStreamEvent::ContentBlockDelta { .. }
        ));

        // 消息结束事件
        let json = r#"{"type": "message_stop"}"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::MessageStop));
    }

    #[test]
    fn test_claude_usage() {
        let usage = AnthropicUsage {
            input_tokens: 100,
            output_tokens: 50,
        };
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
    }
}
