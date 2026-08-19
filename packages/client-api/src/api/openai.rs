//! OpenAI 兼容 API 模块
//!
//! 提供与 OpenAI API 兼容的接口，使用 API Key 认证

use crate::client::OpenAiClient;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// OpenAI API 客户端
#[derive(Debug, Clone)]
pub struct OpenAiApi {
    client: OpenAiClient,
}

impl OpenAiApi {
    /// 创建新的 OpenAI API 客户端
    pub fn new(client: &OpenAiClient) -> Self {
        Self {
            client: client.clone(),
        }
    }

    /// Chat Completions
    pub async fn chat_completions(
        &self,
        req: &ChatCompletionRequest,
        api_key: &str,
    ) -> Result<ChatCompletionResponse> {
        self.client
            .post_json("/v1/chat/completions", req, api_key)
            .await
    }

    /// 获取模型列表
    ///
    /// 不指定协议，网关按 openai 入口过滤模型（见 `list_models_for_protocol`）。
    pub async fn list_models(&self, api_key: &str) -> Result<ModelListResponse> {
        self.list_models_for_protocol(api_key, None).await
    }

    /// 按入口协议获取模型列表（keycompute 网关扩展）
    ///
    /// `protocol` 为 `openai` / `anthropic`：网关只返回该协议账号声明的模型，
    /// 与入口协议隔离路由保持一致；`None` 时网关缺省按 openai 入口过滤。
    /// 协议名作为 query 值拼接前做 URL 编码（协议名受限于代码内固定取值，
    /// 编码为防御性措施，防特殊字符破坏 query 结构）。
    pub async fn list_models_for_protocol(
        &self,
        api_key: &str,
        protocol: Option<&str>,
    ) -> Result<ModelListResponse> {
        let path = match protocol {
            Some(p) => format!("/v1/models?protocol={}", urlencoding::encode(p)),
            None => "/v1/models".to_string(),
        };
        self.client.get_json(&path, api_key).await
    }

    /// 获取模型详情
    pub async fn retrieve_model(&self, model: &str, api_key: &str) -> Result<ModelInfo> {
        self.client
            .get_json(&format!("/v1/models/{}", model), api_key)
            .await
    }
}

/// Chat Completion 请求
#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

impl ChatCompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature: None,
            max_tokens: None,
            stream: None,
        }
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: i32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }
}

/// 消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

/// Chat Completion 响应
#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

/// 选择结果
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub index: i32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

/// Token 使用统计
#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

/// 模型列表响应
#[derive(Debug, Clone, Deserialize)]
pub struct ModelListResponse {
    pub object: String,
    pub data: Vec<ModelInfo>,
}

/// 模型信息
#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}
