//! Anthropic 协议适配器实现
//!
//! 实现 ProviderAdapter trait，提供 Anthropic Messages API 协议的调用能力，
//! 适用于 Claude 系列及其它 Anthropic 协议兼容上游。
//!
//! Anthropic Messages API 与 OpenAI API 的主要差异：
//! - 路径: `{base_url}/messages`
//! - 认证: x-api-key 头部（而非 Authorization: Bearer）
//! - 请求结构: messages 数组不包含 system 角色，system 是独立字段
//! - 响应结构: content 是数组而非单一字符串
//! - max_tokens 为必填字段（未指定时使用默认值）
//!
//! 使用统一 HTTP 传输层：
//! - 通过 HttpTransport 发送请求
//! - 支持连接池复用和代理出口
//!
//! # 重要说明
//! - `endpoint` 为 Base URL（如 `https://api.anthropic.com/v1`），协议层负责拼接路径
//! - `endpoint` 和 `upstream_api_key` 由调用方通过 `UpstreamRequest` 传入
//! - 这些值通常从数据库 Account 表获取，而非配置文件
//! - 管理员可通过前端界面动态配置，无需重启系统

use async_trait::async_trait;
use keycompute_types::{ContentPart, KeyComputeError, MessageContent, Result, SensitiveString};
use llm_protocol_provider::{
    ByteStream, HttpTransport, ProviderAdapter, StreamBox, StreamEvent, UpstreamRequest,
};
use serde_json;

use crate::protocol::{
    AnthropicContent, AnthropicMessage, AnthropicRequest, AnthropicResponse, ContentBlock,
    ImageSource,
};

/// Anthropic API 版本
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// 默认 max_tokens（Anthropic 要求必填，客户端未指定时使用）
pub const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 4096;

/// Anthropic 协议适配器
#[derive(Debug, Clone)]
pub struct AnthropicProvider;

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicProvider {
    /// 创建新的 Anthropic Provider
    pub fn new() -> Self {
        Self
    }

    /// 拼接 Messages API URL
    ///
    /// `endpoint` 只存 Base URL（如 `https://api.anthropic.com/v1`），
    /// 路径由协议层统一拼接，不做任何“已含路径”兼容检测。
    fn messages_url(endpoint: &str) -> String {
        format!("{}/messages", endpoint.trim_end_matches('/'))
    }

    /// 将 OpenAI 风格的图片 URL 转换为 Anthropic 图片来源
    ///
    /// - `data:<media_type>;base64,<data>` → base64 内嵌图片
    /// - http(s) URL → 远程 URL 图片（由 Anthropic 侧下载）
    fn convert_image_url(url: &str) -> Result<ImageSource> {
        if let Some(rest) = url.strip_prefix("data:") {
            let (media_type, data) = rest.split_once(";base64,").ok_or_else(|| {
                KeyComputeError::ProviderError(
                    "Invalid data URI for image: expected 'data:<media_type>;base64,<data>'"
                        .to_string(),
                )
            })?;
            Ok(ImageSource::Base64 {
                media_type: media_type.to_string(),
                data: data.to_string(),
            })
        } else {
            Ok(ImageSource::Url {
                url: url.to_string(),
            })
        }
    }

    /// 将标准化消息内容转换为 Anthropic 内容结构（支持 Vision 多模态）
    fn convert_content(content: &MessageContent) -> Result<AnthropicContent> {
        match content {
            MessageContent::Text(text) => Ok(AnthropicContent::Text(text.clone())),
            MessageContent::Parts(parts) => {
                let blocks = parts
                    .iter()
                    .map(|part| match part {
                        ContentPart::Text { text } => Ok(ContentBlock::Text { text: text.clone() }),
                        ContentPart::ImageUrl { image_url } => Ok(ContentBlock::Image {
                            source: Self::convert_image_url(&image_url.url)?,
                        }),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(AnthropicContent::Blocks(blocks))
            }
        }
    }

    /// 构建 Anthropic 请求体
    ///
    /// 将标准化的 UpstreamRequest 转换为 Anthropic Messages API 格式
    fn build_request_body(&self, request: &UpstreamRequest) -> Result<AnthropicRequest> {
        // 分离 system 消息和普通消息
        let mut system_content: Option<String> = None;
        let mut messages = Vec::new();

        for msg in request.messages.iter() {
            if msg.role == "system" {
                // Anthropic 的 system 是独立字段，不是消息角色；
                // 多条 system 消息拼接保留，避免后发覆盖先发导致指令丢失
                let text = msg.content.to_string();
                match system_content.as_mut() {
                    Some(existing) => {
                        existing.push_str("\n\n");
                        existing.push_str(&text);
                    }
                    None => system_content = Some(text),
                }
            } else {
                // 转换角色: "assistant" -> "assistant"，其余归为 "user"
                let role = if msg.role == "assistant" {
                    "assistant"
                } else {
                    "user"
                };

                messages.push(AnthropicMessage {
                    role: role.to_string(),
                    content: Self::convert_content(&msg.content)?,
                });
            }
        }

        // max_tokens 为 Anthropic 必填字段：优先透传客户端值，未指定时用默认值
        let max_tokens = request.max_tokens.unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS);

        // 入口按 OpenAI 语义校验 temperature ∈ [0, 2]，但 Anthropic 协议
        // 合法范围为 [0, 1]，超范围值直接透传会被上游确定性 400 拒绝，
        // 此处钉制到协议上限（行业通行做法，保留“尽量发散”的语义）
        let temperature = request.temperature.map(|t| {
            if t > 1.0 {
                tracing::debug!(
                    temperature = t,
                    "Clamping temperature to Anthropic protocol max 1.0"
                );
                1.0
            } else {
                t
            }
        });

        Ok(AnthropicRequest {
            model: request.model.clone(),
            max_tokens,
            messages,
            system: system_content,
            stream: Some(request.stream),
            temperature,
            top_p: request.top_p,
            stop_sequences: None,
            metadata: None,
        })
    }

    /// 构建经过验证的原生 Anthropic 请求体。
    ///
    /// 原生入站的扩展字段（例如 tool_use、thinking、cache_control）不能经过
    /// 通用 `MessageContent` 往返，否则会被静默丢弃。仅在调用方显式携带
    /// `native_anthropic_request` 时走此路径，并由入站处理器保证只会路由到
    /// Anthropic 协议账号。
    fn build_native_request_body(
        &self,
        request: &UpstreamRequest,
    ) -> Result<Option<serde_json::Value>> {
        let Some(native_body) = request.native_anthropic_request.as_ref() else {
            return Ok(None);
        };
        // 只有适配器需要修改保留的 payload（强制使用路由后的 model 与
        // stream 标志），因此只在这里进行一次必要的深拷贝，而非每次网关
        // 交接时复制。
        let mut body = native_body.as_ref().clone();

        let object = body.as_object_mut().ok_or_else(|| {
            KeyComputeError::ProviderError("Native Anthropic request must be a JSON object".into())
        })?;
        object.insert(
            "model".to_string(),
            serde_json::Value::String(request.model.clone()),
        );
        object.insert(
            "stream".to_string(),
            serde_json::Value::Bool(request.stream),
        );
        Ok(Some(body))
    }

    /// 构建 Anthropic API 请求头
    fn build_headers(&self, api_key: &str) -> Vec<(String, String)> {
        vec![
            ("x-api-key".to_string(), api_key.to_string()),
            (
                "anthropic-version".to_string(),
                ANTHROPIC_API_VERSION.to_string(),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]
    }

    /// 构建上游认证与协议头。仅原生入站允许覆盖 API version 或追加 beta，
    /// 以避免普通 OpenAI 兼容请求意外继承 Anthropic 的实验能力。
    fn build_request_headers(&self, request: &UpstreamRequest) -> Vec<(String, String)> {
        let mut headers = self.build_headers(request.upstream_api_key.expose());
        if request.native_anthropic_request.is_none() {
            return headers;
        }

        if let Some(version) = request.native_anthropic_headers.get("anthropic-version")
            && let Some((_, value)) = headers
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
        {
            *value = version.clone();
        }
        if let Some(beta) = request.native_anthropic_headers.get("anthropic-beta") {
            headers.push(("anthropic-beta".to_string(), beta.clone()));
        }
        headers
    }

    /// 解析原生响应中的精确 usage。不能将缺失或非法值静默降为零，否则
    /// Gateway 会把它们当作最终用量并覆盖本地估算，导致错误计费。
    fn parse_native_usage(response: &serde_json::Value) -> Result<Option<(u32, u32)>> {
        let Some(usage) = response.get("usage") else {
            return Ok(None);
        };
        let usage = usage.as_object().ok_or_else(|| {
            KeyComputeError::ProviderError(
                "Native Anthropic response usage must be an object".into(),
            )
        })?;
        let parse_tokens = |field: &str| -> Result<u32> {
            let value = usage
                .get(field)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    KeyComputeError::ProviderError(format!(
                        "Native Anthropic response usage.{field} must be an unsigned integer"
                    ))
                })?;
            u32::try_from(value).map_err(|_| {
                KeyComputeError::ProviderError(format!(
                    "Native Anthropic response usage.{field} exceeds u32"
                ))
            })
        };

        let parse_optional_tokens = |field: &str| -> Result<u32> {
            match usage.get(field) {
                None => Ok(0),
                Some(value) => value
                    .as_u64()
                    .ok_or_else(|| {
                        KeyComputeError::ProviderError(format!(
                            "Native Anthropic response usage.{field} must be an unsigned integer"
                        ))
                    })
                    .and_then(|value| {
                        u32::try_from(value).map_err(|_| {
                            KeyComputeError::ProviderError(format!(
                                "Native Anthropic response usage.{field} exceeds u32"
                            ))
                        })
                    }),
            }
        };

        let usage = crate::protocol::AnthropicUsage {
            input_tokens: parse_tokens("input_tokens")?,
            cache_creation_input_tokens: parse_optional_tokens("cache_creation_input_tokens")?,
            cache_read_input_tokens: parse_optional_tokens("cache_read_input_tokens")?,
            output_tokens: parse_tokens("output_tokens")?,
        };

        Ok(Some((usage.total_input_tokens()?, usage.output_tokens)))
    }

    /// 执行非流式请求
    ///
    /// 返回 (content, usage, finish_reason) 元组
    async fn chat_internal(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<(String, Option<(u32, u32)>, Option<String>, Option<String>)> {
        let native = self.build_native_request_body(&request)?;
        let body = match native.as_ref() {
            Some(body) => serde_json::to_string(body),
            None => serde_json::to_string(&self.build_request_body(&request)?),
        }
        .map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to serialize request: {}", e))
        })?;
        let url = Self::messages_url(&request.endpoint);

        let headers = self.build_request_headers(&request);

        let response_text = transport.post_json(&url, headers, body).await?;

        if native.is_some() {
            let response: serde_json::Value =
                serde_json::from_str(&response_text).map_err(|e| {
                    KeyComputeError::ProviderError(format!(
                        "Failed to parse Anthropic response: {}",
                        e
                    ))
                })?;
            let usage = Self::parse_native_usage(&response)?;
            // `ProviderAdapter::chat` is a text-returning public API. Native
            // ingress normally consumes the raw body through `stream_chat`,
            // but direct callers must still receive the response's text rather
            // than a silent empty string.
            let content: AnthropicResponse =
                serde_json::from_value(response.clone()).map_err(|e| {
                    KeyComputeError::ProviderError(format!(
                        "Failed to parse Anthropic response: {}",
                        e
                    ))
                })?;
            let stop_reason = response
                .get("stop_reason")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            return Ok((
                content.extract_text(),
                usage,
                stop_reason,
                Some(response_text),
            ));
        }

        let anthropic_response: AnthropicResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                KeyComputeError::ProviderError(format!("Failed to parse Anthropic response: {}", e))
            })?;

        let content = anthropic_response.extract_text();
        let usage = Some((
            anthropic_response.usage.total_input_tokens()?,
            anthropic_response.usage.output_tokens,
        ));

        Ok((content, usage, anthropic_response.stop_reason, None))
    }

    /// 执行流式请求
    async fn stream_chat_internal(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<StreamBox> {
        let native = self.build_native_request_body(&request)?;
        let url = Self::messages_url(&request.endpoint);

        let body_json = match native.as_ref() {
            Some(body) => serde_json::to_string(body),
            None => {
                let mut body = self.build_request_body(&request)?;
                // 确保启用流式输出
                body.stream = Some(true);
                serde_json::to_string(&body)
            }
        }
        .map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to serialize request: {}", e))
        })?;

        let mut headers = self.build_request_headers(&request);
        headers.push(("Accept".to_string(), "text/event-stream".to_string()));

        let byte_stream: ByteStream = transport.post_stream(&url, headers, body_json).await?;

        // 转换为标准化的 StreamEvent 流
        Ok(crate::stream::parse_anthropic_stream_with_raw(
            byte_stream,
            native.is_some(),
        ))
    }
}

#[async_trait]
impl ProviderAdapter for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        // 协议层不维护模型白名单，模型由渠道账号的 models_supported 声明
        Vec::new()
    }

    /// 协议层接受任意模型，路由层已按账号 models_supported 过滤
    fn supports_model(&self, _model: &str) -> bool {
        true
    }

    async fn stream_chat(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<StreamBox> {
        if request.stream {
            self.stream_chat_internal(transport, request).await
        } else {
            // 非流式请求，包装为单事件流
            let (content, usage, finish_reason, native_response) =
                self.chat_internal(transport, request).await?;

            if let Some(native_response) = native_response {
                let mut events: Vec<Result<StreamEvent>> = vec![Ok(StreamEvent::raw(
                    serde_json::json!({"kind": "anthropic_message", "body": serde_json::from_str::<serde_json::Value>(&native_response).unwrap_or(serde_json::Value::Null)}).to_string(),
                ))];
                if let Some((input_tokens, output_tokens)) = usage {
                    events.push(Ok(StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    }));
                }
                events.push(Ok(StreamEvent::Done));
                return Ok(Box::pin(futures::stream::iter(events)));
            }

            let event = StreamEvent::Delta {
                content,
                // 非流式响应有 finish_reason，未提供时回退为 "stop"
                finish_reason: Some(finish_reason.unwrap_or_else(|| "stop".to_string())),
            };

            let mut events: Vec<Result<StreamEvent>> = vec![Ok(event)];

            // 如果有 usage 信息，添加 Usage 事件（保证精确计费）
            if let Some((input_tokens, output_tokens)) = usage {
                events.push(Ok(StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                }));
            }

            events.push(Ok(StreamEvent::Done));

            let stream = futures::stream::iter(events);
            Ok(Box::pin(stream))
        }
    }

    async fn chat(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<String> {
        let (content, _usage, _finish_reason, _native_response) =
            self.chat_internal(transport, request).await?;
        Ok(content)
    }

    /// 获取上游模型列表（兼作连通性验证）
    ///
    /// Anthropic 使用 x-api-key + anthropic-version 认证（覆盖默认的 Bearer 实现）
    async fn list_models(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &SensitiveString,
    ) -> Result<Vec<String>> {
        let url = format!("{}/models", endpoint.trim_end_matches('/'));
        let headers = vec![
            ("x-api-key".to_string(), api_key.expose().to_string()),
            (
                "anthropic-version".to_string(),
                ANTHROPIC_API_VERSION.to_string(),
            ),
        ];
        let response = transport.get_binary(&url, headers).await?;
        llm_protocol_provider::parse_models_response(&response.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use llm_protocol_provider::{UpstreamMessage, test_support::RecordingGetTransport};
    use std::time::Duration;

    #[derive(Debug)]
    struct StaticPostTransport {
        response: String,
    }

    #[async_trait]
    impl HttpTransport for StaticPostTransport {
        async fn post_json(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
            _body: String,
        ) -> Result<String> {
            Ok(self.response.clone())
        }

        async fn post_stream(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
            _body: String,
        ) -> Result<ByteStream> {
            Err(KeyComputeError::ProviderError(
                "StaticPostTransport does not support streams".into(),
            ))
        }

        fn request_timeout(&self) -> Duration {
            Duration::from_secs(1)
        }

        fn stream_timeout(&self) -> Duration {
            Duration::from_secs(1)
        }
    }

    #[test]
    fn test_anthropic_provider_name() {
        let provider = AnthropicProvider::new();
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn test_anthropic_supported_models_empty() {
        let provider = AnthropicProvider::new();
        // 协议层不维护模型白名单
        assert!(provider.supported_models().is_empty());
    }

    #[tokio::test]
    async fn list_models_propagates_invalid_response_with_anthropic_auth() {
        let transport = RecordingGetTransport::new(br#"{"data": "invalid"}"#.to_vec());
        let provider = AnthropicProvider::new();
        let api_key = SensitiveString::new("test-key");

        let error = provider
            .list_models(&transport, "https://provider.example/v1/", &api_key)
            .await
            .unwrap_err();

        assert!(matches!(error, KeyComputeError::ProviderError(_)));
        assert_eq!(
            transport.requests(),
            vec![(
                "https://provider.example/v1/models".to_string(),
                vec![
                    ("x-api-key".to_string(), "test-key".to_string()),
                    (
                        "anthropic-version".to_string(),
                        ANTHROPIC_API_VERSION.to_string(),
                    ),
                ],
            )]
        );
    }

    #[test]
    fn test_anthropic_supports_any_model() {
        let provider = AnthropicProvider::new();
        assert!(provider.supports_model("claude-3-5-sonnet-20241022"));
        assert!(provider.supports_model("any-model"));
    }

    #[test]
    fn test_messages_url_join() {
        assert_eq!(
            AnthropicProvider::messages_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            AnthropicProvider::messages_url("https://custom.example.com/v1/"),
            "https://custom.example.com/v1/messages"
        );
        // 空 endpoint 回落的协议默认 Base URL 必须能拼出完整请求 URL
        assert_eq!(
            AnthropicProvider::messages_url(
                llm_protocol_provider::ProtocolType::Anthropic.default_endpoint()
            ),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_build_request_body() {
        let provider = AnthropicProvider::new();
        let request = UpstreamRequest::new(
            "https://api.anthropic.com/v1",
            "sk-test",
            "claude-3-5-sonnet-20241022",
        )
        .with_message("system", "You are helpful")
        .with_message("user", "Hello")
        .with_stream(true)
        .with_temperature(0.7);

        let body = provider.build_request_body(&request).unwrap();

        assert_eq!(body.model, "claude-3-5-sonnet-20241022");
        assert_eq!(body.system, Some("You are helpful".to_string()));
        assert_eq!(body.messages.len(), 1); // 只有 user 消息
        assert_eq!(body.stream, Some(true));
        assert_eq!(body.temperature, Some(0.7));
        assert_eq!(body.max_tokens, ANTHROPIC_DEFAULT_MAX_TOKENS); // 默认值
    }

    #[test]
    fn test_build_request_body_with_max_tokens() {
        let provider = AnthropicProvider::new();
        let request = UpstreamRequest::new(
            "https://api.anthropic.com/v1",
            "sk-test",
            "claude-3-5-sonnet-20241022",
        )
        .with_message("user", "Hello")
        .with_max_tokens(1024);

        let body = provider.build_request_body(&request).unwrap();
        assert_eq!(body.max_tokens, 1024);
    }

    #[test]
    fn test_build_request_body_concatenates_multiple_system_messages() {
        let provider = AnthropicProvider::new();
        let request = UpstreamRequest::new(
            "https://api.anthropic.com/v1",
            "sk-test",
            "claude-3-5-sonnet-20241022",
        )
        .with_message("system", "You are helpful")
        .with_message("system", "Answer in Chinese")
        .with_message("user", "Hello");

        let body = provider.build_request_body(&request).unwrap();

        // 多条 system 消息应拼接保留，而非后发覆盖先发
        assert_eq!(
            body.system,
            Some("You are helpful\n\nAnswer in Chinese".to_string())
        );
        assert_eq!(body.messages.len(), 1); // 只有 user 消息
    }

    #[test]
    fn test_build_request_body_clamps_temperature_to_protocol_range() {
        // OpenAI 入口允许 temperature ∈ [0, 2]，Anthropic 协议上限为 1.0，
        // 超范围值应钉制而非透传（否则上游确定性 400）
        let provider = AnthropicProvider::new();
        let request = UpstreamRequest::new(
            "https://api.anthropic.com/v1",
            "sk-test",
            "claude-3-5-sonnet-20241022",
        )
        .with_message("user", "Hello")
        .with_temperature(1.5);

        let body = provider.build_request_body(&request).unwrap();
        assert_eq!(body.temperature, Some(1.0));

        // 范围内的值原样透传
        let request = UpstreamRequest::new(
            "https://api.anthropic.com/v1",
            "sk-test",
            "claude-3-5-sonnet-20241022",
        )
        .with_message("user", "Hello")
        .with_temperature(0.3);
        let body = provider.build_request_body(&request).unwrap();
        assert_eq!(body.temperature, Some(0.3));
    }

    #[test]
    fn test_build_headers() {
        let provider = AnthropicProvider::new();
        let headers = provider.build_headers("sk-test-key");

        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "x-api-key" && v == "sk-test-key")
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "anthropic-version" && v == ANTHROPIC_API_VERSION)
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/json")
        );
    }

    #[test]
    fn native_request_headers_preserve_version_and_beta() {
        let provider = AnthropicProvider::new();
        let mut request =
            UpstreamRequest::new("https://api.anthropic.com/v1", "sk-test-key", "claude-test");
        request.native_anthropic_request =
            Some(std::sync::Arc::new(serde_json::json!({"messages": []})));
        request
            .native_anthropic_headers
            .insert("anthropic-version".to_string(), "2025-01-01".to_string());
        request.native_anthropic_headers.insert(
            "anthropic-beta".to_string(),
            "fine-grained-tool-streaming-2025-05-14".to_string(),
        );

        let headers = provider.build_request_headers(&request);
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "anthropic-version" && value == "2025-01-01")
        );
        assert!(headers.iter().any(|(name, value)| {
            name == "anthropic-beta" && value == "fine-grained-tool-streaming-2025-05-14"
        }));
    }

    #[test]
    fn native_usage_rejects_malformed_or_overflowing_token_counts() {
        assert!(
            AnthropicProvider::parse_native_usage(&serde_json::json!({
                "usage": {"input_tokens": 1}
            }))
            .is_err()
        );
        assert!(
            AnthropicProvider::parse_native_usage(&serde_json::json!({
                "usage": null
            }))
            .is_err()
        );
        assert!(
            AnthropicProvider::parse_native_usage(&serde_json::json!({
                "usage": {"input_tokens": 1, "output_tokens": u64::from(u32::MAX) + 1}
            }))
            .is_err()
        );
        assert_eq!(
            AnthropicProvider::parse_native_usage(&serde_json::json!({
                "usage": {"input_tokens": 3, "output_tokens": 5}
            }))
            .unwrap(),
            Some((3, 5))
        );
    }

    #[test]
    fn native_usage_includes_prompt_cache_read_and_write_tokens() {
        assert_eq!(
            AnthropicProvider::parse_native_usage(&serde_json::json!({
                "usage": {
                    "input_tokens": 3,
                    "cache_creation_input_tokens": 5,
                    "cache_read_input_tokens": 7,
                    "output_tokens": 11
                }
            }))
            .unwrap(),
            Some((15, 11))
        );
    }

    #[tokio::test]
    async fn malformed_native_usage_never_constructs_a_billable_stream() {
        let provider = AnthropicProvider::new();
        let transport = StaticPostTransport {
            response: r#"{"type":"message","usage":{"input_tokens":1}}"#.to_string(),
        };
        let mut request =
            UpstreamRequest::new("https://provider.example/v1", "sk-test", "claude-test");
        request.native_anthropic_request = Some(std::sync::Arc::new(serde_json::json!({
            "model": "claude-test", "max_tokens": 1, "messages": []
        })));

        match provider.stream_chat(&transport, request).await {
            Err(KeyComputeError::ProviderError(message)) => {
                assert!(message.contains("usage.output_tokens"));
            }
            Err(error) => panic!("expected a provider error, got {error}"),
            Ok(_) => panic!("malformed native usage must not create a stream"),
        }
    }

    #[tokio::test]
    async fn native_chat_returns_text_for_direct_provider_callers() {
        let provider = AnthropicProvider::new();
        let transport = StaticPostTransport {
            response: r#"{
                "id":"msg_test",
                "type":"message",
                "role":"assistant",
                "model":"claude-test",
                "content":[{"type":"text","text":"native reply"}],
                "stop_reason":"end_turn",
                "usage":{"input_tokens":3,"output_tokens":2}
            }"#
            .to_string(),
        };
        let mut request =
            UpstreamRequest::new("https://provider.example/v1", "sk-test", "claude-test");
        request.native_anthropic_request = Some(std::sync::Arc::new(serde_json::json!({
            "model": "claude-test", "max_tokens": 1, "messages": []
        })));

        assert_eq!(
            provider.chat(&transport, request).await.unwrap(),
            "native reply"
        );
    }

    #[tokio::test]
    async fn non_native_chat_counts_prompt_cache_tokens_in_usage() {
        let provider = AnthropicProvider::new();
        let transport = StaticPostTransport {
            response: r#"{
                "id":"msg_test",
                "type":"message",
                "role":"assistant",
                "model":"claude-test",
                "content":[{"type":"text","text":"reply"}],
                "stop_reason":"end_turn",
                "usage":{
                    "input_tokens":3,
                    "cache_creation_input_tokens":5,
                    "cache_read_input_tokens":7,
                    "output_tokens":2
                }
            }"#
            .to_string(),
        };
        let request = UpstreamRequest::new("https://provider.example/v1", "sk-test", "claude-test");

        let (content, usage, _, _) = provider.chat_internal(&transport, request).await.unwrap();
        assert_eq!(content, "reply");
        assert_eq!(usage, Some((15, 2)));
    }

    #[test]
    fn test_build_request_body_converts_roles() {
        let provider = AnthropicProvider::new();
        let request = UpstreamRequest {
            endpoint: "https://api.anthropic.com/v1".to_string(),
            upstream_api_key: "sk-test".into(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![
                UpstreamMessage {
                    role: "system".to_string(),
                    content: MessageContent::text("You are helpful"),
                },
                UpstreamMessage {
                    role: "user".to_string(),
                    content: MessageContent::text("Hello"),
                },
                UpstreamMessage {
                    role: "assistant".to_string(),
                    content: MessageContent::text("Hi there!"),
                },
                UpstreamMessage {
                    role: "user".to_string(),
                    content: MessageContent::text("How are you?"),
                },
            ],
            stream: true,
            max_tokens: Some(1024),
            temperature: None,
            top_p: None,
            native_anthropic_request: None,
            native_anthropic_headers: std::collections::BTreeMap::new(),
        };

        let body = provider.build_request_body(&request).unwrap();

        // system 应该被提取到独立字段
        assert_eq!(body.system, Some("You are helpful".to_string()));

        // 消息列表应该只有 3 条（不含 system）
        assert_eq!(body.messages.len(), 3);

        // 验证角色转换
        assert_eq!(body.messages[0].role, "user");
        assert_eq!(body.messages[1].role, "assistant");
        assert_eq!(body.messages[2].role, "user");
    }

    #[test]
    fn test_convert_image_url_data_uri() {
        let source =
            AnthropicProvider::convert_image_url("data:image/png;base64,aGVsbG8=").unwrap();
        match source {
            ImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "aGVsbG8=");
            }
            _ => panic!("expected base64 source"),
        }
    }

    #[test]
    fn test_convert_image_url_http() {
        let source = AnthropicProvider::convert_image_url("https://example.com/cat.png").unwrap();
        assert!(matches!(source, ImageSource::Url { url } if url == "https://example.com/cat.png"));
    }

    #[test]
    fn test_convert_image_url_invalid_data_uri() {
        assert!(AnthropicProvider::convert_image_url("data:image/png,notbase64").is_err());
    }

    #[test]
    fn test_build_request_body_with_vision_parts() {
        let provider = AnthropicProvider::new();
        let parts = vec![
            ContentPart::Text {
                text: "What is in this image?".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: keycompute_types::ImageUrl {
                    url: "data:image/jpeg;base64,QUJD".to_string(),
                    detail: None,
                },
            },
        ];
        let request = UpstreamRequest {
            endpoint: "https://api.anthropic.com/v1".to_string(),
            upstream_api_key: "sk-test".into(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![UpstreamMessage {
                role: "user".to_string(),
                content: MessageContent::Parts(parts),
            }],
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
            native_anthropic_request: None,
            native_anthropic_headers: std::collections::BTreeMap::new(),
        };

        let body = provider.build_request_body(&request).unwrap();
        assert_eq!(body.messages.len(), 1);
        match &body.messages[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                assert!(
                    matches!(&blocks[0], ContentBlock::Text { text } if text.contains("image"))
                );
                assert!(matches!(
                    &blocks[1],
                    ContentBlock::Image {
                        source: ImageSource::Base64 { media_type, .. }
                    } if media_type == "image/jpeg"
                ));
            }
            _ => panic!("expected content blocks"),
        }
    }
}
