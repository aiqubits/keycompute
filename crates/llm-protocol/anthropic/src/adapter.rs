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
use crate::stream::parse_anthropic_stream;

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

    /// 执行非流式请求
    ///
    /// 返回 (content, usage, finish_reason) 元组
    async fn chat_internal(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<(String, Option<(u32, u32)>, Option<String>)> {
        let body = self.build_request_body(&request)?;
        let url = Self::messages_url(&request.endpoint);
        let body_json = serde_json::to_string(&body).map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to serialize request: {}", e))
        })?;

        let headers = self.build_headers(request.upstream_api_key.expose());

        let response_text = transport.post_json(&url, headers, body_json).await?;

        let anthropic_response: AnthropicResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                KeyComputeError::ProviderError(format!("Failed to parse Anthropic response: {}", e))
            })?;

        let content = anthropic_response.extract_text();
        let usage = Some((
            anthropic_response.usage.input_tokens,
            anthropic_response.usage.output_tokens,
        ));

        Ok((content, usage, anthropic_response.stop_reason))
    }

    /// 执行流式请求
    async fn stream_chat_internal(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<StreamBox> {
        let mut body = self.build_request_body(&request)?;
        let url = Self::messages_url(&request.endpoint);

        // 确保启用流式输出
        body.stream = Some(true);

        let body_json = serde_json::to_string(&body).map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to serialize request: {}", e))
        })?;

        let mut headers = self.build_headers(request.upstream_api_key.expose());
        headers.push(("Accept".to_string(), "text/event-stream".to_string()));

        let byte_stream: ByteStream = transport.post_stream(&url, headers, body_json).await?;

        // 转换为标准化的 StreamEvent 流
        Ok(parse_anthropic_stream(byte_stream))
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
            let (content, usage, finish_reason) = self.chat_internal(transport, request).await?;

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
        let (content, _usage, _finish_reason) = self.chat_internal(transport, request).await?;
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
        Ok(llm_protocol_provider::parse_models_response(&response.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_protocol_provider::UpstreamMessage;

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
