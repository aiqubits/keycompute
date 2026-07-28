//! 统一上游请求类型
//!
//! 定义发送到上游 Provider 的标准化请求格式
//!
//! # 重要说明
//! - `endpoint` 和 `upstream_api_key` 由调用方（如 Routing 引擎）在运行时动态传入
//! - 这些值通常从数据库中的 Account 表获取，而非配置文件
//! - 管理员可通过前端界面动态配置 Provider 端点和 Upstream API Key，无需重启系统

use keycompute_types::{MessageContent, SensitiveString};
use serde::{Deserialize, Serialize};

/// 上游请求结构
///
/// 标准化的请求格式，各 Provider Adapter 负责转换为各自协议
///
/// # 字段说明
/// - `endpoint`: Provider API 端点 URL，由调用方传入（如从 Account 表获取）
/// - `upstream_api_key`: 上游 Provider API Key，由调用方传入（如从 Account 表获取）
/// - 这些配置**不**从配置文件读取，支持运行时动态变更
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamRequest {
    /// 上游 API 端点（由调用方传入，如从 Account 表获取）
    pub endpoint: String,
    /// 上游 Provider API Key（由调用方传入，如从 Account 表获取）
    pub upstream_api_key: SensitiveString,
    /// 模型名称
    pub model: String,
    /// 消息列表
    pub messages: Vec<UpstreamMessage>,
    /// 是否流式输出
    pub stream: bool,
    /// 最大 token 数（可选）
    pub max_tokens: Option<u32>,
    /// 温度参数（可选）
    pub temperature: Option<f32>,
    /// Top P 参数（可选）
    pub top_p: Option<f32>,
}

impl UpstreamRequest {
    /// 创建新的上游请求
    pub fn new(
        endpoint: impl Into<String>,
        upstream_api_key: impl Into<SensitiveString>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            upstream_api_key: upstream_api_key.into(),
            model: model.into(),
            messages: Vec::new(),
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
        }
    }

    /// 添加消息
    pub fn with_message(
        mut self,
        role: impl Into<String>,
        content: impl Into<MessageContent>,
    ) -> Self {
        self.messages.push(UpstreamMessage {
            role: role.into(),
            content: content.into(),
        });
        self
    }

    /// 设置流式输出
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// 设置最大 token 数
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// 设置温度参数
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

/// 上游消息结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamMessage {
    /// 角色：system / user / assistant
    pub role: String,
    /// 消息内容（支持纯文本和 Vision 多模态）
    pub content: MessageContent,
}

impl UpstreamMessage {
    /// 创建系统消息
    pub fn system(content: impl Into<MessageContent>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    /// 创建用户消息
    pub fn user(content: impl Into<MessageContent>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    /// 创建助手消息
    pub fn assistant(content: impl Into<MessageContent>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }

    /// 从纯文本创建消息（便捷方法）
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::text(content),
        }
    }

    /// 创建带 Vision 内容的消息
    pub fn with_parts(role: impl Into<String>, parts: Vec<keycompute_types::ContentPart>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Parts(parts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upstream_request_builder() {
        let request = UpstreamRequest::new(
            "https://api.openai.com/v1/chat/completions",
            "sk-test",
            "gpt-4o",
        )
        .with_message("system", "You are a helpful assistant")
        .with_message("user", "Hello")
        .with_stream(true)
        .with_max_tokens(1000)
        .with_temperature(0.7);

        assert_eq!(request.model, "gpt-4o");
        assert_eq!(request.messages.len(), 2);
        assert!(request.stream);
        assert_eq!(request.max_tokens, Some(1000));
    }

    #[test]
    fn test_upstream_message_helpers() {
        let sys = UpstreamMessage::system("System prompt");
        let user = UpstreamMessage::user("User input");
        let assistant = UpstreamMessage::assistant("Assistant response");

        assert_eq!(sys.role, "system");
        assert_eq!(user.role, "user");
        assert_eq!(assistant.role, "assistant");
    }
}
