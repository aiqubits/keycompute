use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

use crate::{PricingSnapshot, UsageAccumulator};

/// 请求上下文：贯穿全链路的唯一状态载体
///
/// # 设计说明
/// - `usage` 字段使用 `Arc<UsageAccumulator>` 实现共享状态，Clone 时会共享同一个用量累积器
/// - 通过 `add_output_tokens()` 和 `set_input_tokens()` 方法安全地更新用量
/// - 使用 `usage_snapshot()` 获取当前用量快照
/// - `provider` 字段在路由确定后被设置，用于精确的定价查询
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: Uuid,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub produce_ai_key_id: Uuid,
    pub model: String,
    /// Provider 名称（路由确定后设置）
    pub provider: Option<String>,
    pub messages: Vec<Message>,
    pub stream: bool,
    /// 客户端指定的最大生成 token 数（透传给上游协议层）
    pub max_tokens: Option<u32>,
    /// 客户端指定的温度参数（透传给上游协议层）
    pub temperature: Option<f32>,
    /// 客户端指定的 Top P 参数（透传给上游协议层）
    pub top_p: Option<f32>,
    pub pricing_snapshot: PricingSnapshot, // 请求开始时固化
    usage: Arc<UsageAccumulator>,          // streaming 中累积（共享状态）
    pub started_at: DateTime<Utc>,
}

impl RequestContext {
    pub fn new(
        user_id: Uuid,
        tenant_id: Uuid,
        produce_ai_key_id: Uuid,
        model: impl Into<String>,
        messages: Vec<Message>,
        stream: bool,
        pricing_snapshot: PricingSnapshot,
    ) -> Self {
        Self {
            request_id: Uuid::new_v4(),
            user_id,
            tenant_id,
            produce_ai_key_id,
            model: model.into(),
            provider: None,
            messages,
            stream,
            max_tokens: None,
            temperature: None,
            top_p: None,
            pricing_snapshot,
            usage: Arc::new(UsageAccumulator::new()),
            started_at: Utc::now(),
        }
    }

    /// 设置 Provider（路由确定后调用）
    pub fn set_provider(&mut self, provider: impl Into<String>) {
        self.provider = Some(provider.into());
    }

    /// 更新定价快照（路由后根据实际 provider 更新）
    pub fn update_pricing(&mut self, pricing: PricingSnapshot) {
        self.pricing_snapshot = pricing;
    }

    /// 获取请求持续时间
    pub fn duration(&self) -> chrono::Duration {
        Utc::now() - self.started_at
    }

    /// 获取当前用量快照
    pub fn usage_snapshot(&self) -> (u32, u32) {
        self.usage.snapshot()
    }

    /// 添加输出 token（原子更新）
    pub fn add_output_tokens(&self, tokens: u32) {
        self.usage.add_output(tokens);
    }

    /// 设置输出 token（用于覆盖估算值）
    ///
    /// 当 Provider 返回精确的 usage 信息时，使用此方法直接设置输出 token 数
    /// 而非累积，确保与 Provider 的计费完全一致
    pub fn set_output_tokens(&self, tokens: u32) {
        self.usage.set_output(tokens);
    }

    /// 设置输入 token（原子更新）
    pub fn set_input_tokens(&self, tokens: u32) {
        self.usage.set_input(tokens);
    }

    /// 检查 usage 是否已被 Provider 精确值覆盖
    ///
    /// 如果返回 true，说明收到过 StreamEvent::Usage 事件，使用的是 Provider 精确值
    /// 如果返回 false，说明未收到 Usage 事件，使用的是 tiktoken 估算值
    pub fn is_usage_finalized(&self) -> bool {
        self.usage.is_input_finalized() && self.usage.is_output_finalized()
    }
}

/// 消息角色枚举
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    /// 获取角色字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

impl fmt::Display for MessageRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 消息内容：支持纯文本和 Vision 多模态内容
///
/// 反序列化时拒绝空数组 `[]`，避免静默丢失数据。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// 纯文本内容
    Text(String),
    /// Vision 内容块列表（图片理解等）
    Parts(Vec<ContentPart>),
}

// 使用宏生成自定义 Deserialize 实现，拒绝空数组 []
crate::impl_untagged_content_deserialize!(
    MessageContent,
    ContentPart,
    "non-empty array of content parts"
);

impl MessageContent {
    /// 从纯文本创建
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text(content.into())
    }

    /// 提取纯文本内容（用于日志/计费等场景）
    pub fn extract_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// 是否为纯文本
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl std::fmt::Display for MessageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.extract_text())
    }
}

/// Vision 内容块
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    /// 文本块
    #[serde(rename = "text")]
    Text { text: String },
    /// 图片 URL 块
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

/// 图片 URL 描述
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    /// 图片 URL（支持 http/https URL 或 base64 data URI）
    pub url: String,
    /// 细节级别：low / high / auto（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 消息结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: MessageContent,
}

impl Message {
    pub fn new(role: MessageRole, content: impl Into<MessageContent>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<MessageContent>) -> Self {
        Self::new(MessageRole::System, content)
    }

    pub fn user(content: impl Into<MessageContent>) -> Self {
        Self::new(MessageRole::User, content)
    }

    pub fn assistant(content: impl Into<MessageContent>) -> Self {
        Self::new(MessageRole::Assistant, content)
    }

    pub fn tool(content: impl Into<MessageContent>) -> Self {
        Self::new(MessageRole::Tool, content)
    }
}

/// OpenAI 兼容的请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

impl ChatCompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            n: None,
            stop: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_role_as_str() {
        assert_eq!(MessageRole::System.as_str(), "system");
        assert_eq!(MessageRole::User.as_str(), "user");
        assert_eq!(MessageRole::Assistant.as_str(), "assistant");
        assert_eq!(MessageRole::Tool.as_str(), "tool");
    }

    #[test]
    fn test_message_role_all_variants() {
        // 测试所有变体的字符串表示
        let roles = vec![
            (MessageRole::System, "system"),
            (MessageRole::User, "user"),
            (MessageRole::Assistant, "assistant"),
            (MessageRole::Tool, "tool"),
        ];
        for (role, expected) in roles {
            assert_eq!(role.as_str(), expected);
            assert_eq!(format!("{}", role), expected);
        }
    }

    #[test]
    fn test_message_role_display() {
        assert_eq!(format!("{}", MessageRole::System), "system");
        assert_eq!(format!("{}", MessageRole::User), "user");
    }

    #[test]
    fn test_message_role_default() {
        assert_eq!(MessageRole::default(), MessageRole::User);
    }

    #[test]
    fn test_message_role_serialize() {
        let role = MessageRole::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"assistant\"");
    }

    #[test]
    fn test_message_role_deserialize() {
        let json = "\"system\"";
        let role: MessageRole = serde_json::from_str(json).unwrap();
        assert_eq!(role, MessageRole::System);
    }

    #[test]
    fn test_message_role_deserialize_invalid() {
        let json = "\"invalid_role\"";
        let result: Result<MessageRole, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_message_creation() {
        let msg = Message::new(MessageRole::User, "Hello");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content.extract_text(), "Hello");
    }

    #[test]
    fn test_message_convenience_constructors() {
        let system_msg = Message::system("You are a helpful assistant");
        assert_eq!(system_msg.role, MessageRole::System);

        let user_msg = Message::user("Hello");
        assert_eq!(user_msg.role, MessageRole::User);

        let assistant_msg = Message::assistant("Hi there!");
        assert_eq!(assistant_msg.role, MessageRole::Assistant);

        let tool_msg = Message::tool("Tool result");
        assert_eq!(tool_msg.role, MessageRole::Tool);
    }

    #[test]
    fn test_message_serialize() {
        let msg = Message::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"content\":\"Hello\""));
    }

    #[test]
    fn test_message_vision_deserialize() {
        let json = r#"{"role":"user","content":[{"type":"text","text":"What's in this image?"},{"type":"image_url","image_url":{"url":"https://example.com/image.png","detail":"high"}}]}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, MessageRole::User);
        assert!(matches!(msg.content, MessageContent::Parts(_)));
        assert_eq!(msg.content.extract_text(), "What's in this image?");
    }

    #[test]
    fn test_message_content_text_serde_roundtrip() {
        let msg = Message::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.content.extract_text(), "Hello");
    }

    #[test]
    fn test_message_deserialize() {
        let json = r#"{"role":"assistant","content":"Hello!"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content.extract_text(), "Hello!");
    }

    #[test]
    fn test_request_context_new() {
        let ctx = RequestContext::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "gpt-4",
            vec![Message::user("Hello")],
            false,
            PricingSnapshot::default(),
        );
        assert_eq!(ctx.model, "gpt-4");
        assert!(!ctx.stream);
    }

    #[test]
    fn test_request_context_usage_shared() {
        let ctx = RequestContext::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "gpt-4",
            vec![Message::user("Hello")],
            false,
            PricingSnapshot::default(),
        );

        // 添加 token
        ctx.add_output_tokens(100);
        ctx.set_input_tokens(50);

        // 验证用量
        let (input, output) = ctx.usage_snapshot();
        assert_eq!(input, 50);
        assert_eq!(output, 100);

        // Clone 后共享同一个 usage
        let ctx2 = ctx.clone();
        ctx2.add_output_tokens(50);

        // ctx 也能看到更新
        let (_, output2) = ctx.usage_snapshot();
        assert_eq!(output2, 150);
    }

    #[test]
    fn test_chat_completion_request_new() {
        let req = ChatCompletionRequest::new("gpt-4", vec![Message::user("Hello")]);
        assert_eq!(req.model, "gpt-4");
        assert_eq!(req.messages.len(), 1);
        assert!(req.stream.is_none());
    }

    #[test]
    fn test_chat_completion_request_serialize() {
        let req = ChatCompletionRequest::new("gpt-4", vec![Message::user("Hello")]);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"gpt-4\""));
        assert!(json.contains("\"role\":\"user\""));
    }

    #[test]
    fn test_message_content_rejects_empty_array() {
        // 空数组 [] 应该被拒绝，不能反序列化为 MessageContent::Text("")
        let json = r#"{"role":"user","content":[]}"#;
        let result: Result<Message, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Empty array [] should be rejected");
    }
}
