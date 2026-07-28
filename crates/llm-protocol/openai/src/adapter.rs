//! OpenAI 协议适配器实现
//!
//! 实现 ProviderAdapter trait，提供 OpenAI Chat Completions 协议的调用能力，
//! 适用于所有 OpenAI 兼容上游（OpenAI/DeepSeek/Ollama/vLLM/Gemini 兼容层等）。
//! 支持 Chat Completions（含 Vision 多模态）、图片生成、图片编辑
//!
//! 使用统一 HTTP 传输层：
//! - 通过 HttpTransport 发送请求
//! - 支持连接池复用和代理出口
//!
//! # 重要说明
//! - `endpoint` 为 Base URL（如 `https://api.openai.com/v1`），协议层负责拼接路径
//! - `endpoint` 和 `upstream_api_key` 由调用方通过 `UpstreamRequest` 传入
//! - 这些值通常从数据库 Account 表获取，而非配置文件
//! - 管理员可通过前端界面动态配置，无需重启系统

use async_trait::async_trait;
use keycompute_types::{KeyComputeError, Result};
use llm_protocol_provider::{
    ByteStream, HttpTransport, ProviderAdapter, StreamBox, StreamEvent, UpstreamRequest,
};
use serde_json;

use crate::protocol::{
    ImageEditRequest, ImageGenerationRequest, ImageGenerationResponse, ImageVariationRequest,
    OpenAIMessage, OpenAIRequest, OpenAIResponse, ResponsesRequest, ResponsesResponse,
    StreamOptions, convert_message_content,
};
use crate::stream::parse_openai_stream;

/// OpenAI Chat Completions 默认端点
pub const OPENAI_CHAT_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

/// OpenAI 图片生成默认端点
pub const OPENAI_IMAGE_GEN_ENDPOINT: &str = "https://api.openai.com/v1/images/generations";

/// OpenAI 图片编辑默认端点
pub const OPENAI_IMAGE_EDIT_ENDPOINT: &str = "https://api.openai.com/v1/images/edits";

/// OpenAI 图片变体默认端点
pub const OPENAI_IMAGE_VARIATION_ENDPOINT: &str = "https://api.openai.com/v1/images/variations";

/// OpenAI Responses API 默认端点（统一多模态接口）
pub const OPENAI_RESPONSES_ENDPOINT: &str = "https://api.openai.com/v1/responses";

/// OpenAI Provider 适配器
#[derive(Debug, Clone)]
pub struct OpenAIProvider;

impl Default for OpenAIProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAIProvider {
    /// 创建新的 OpenAI Provider
    pub fn new() -> Self {
        Self
    }

    /// 拼接 Chat Completions URL
    ///
    /// `endpoint` 只存 Base URL（如 `https://api.openai.com/v1`），
    /// 路径由协议层统一拼接，不做任何“已含路径”兼容检测。
    fn chat_url(endpoint: &str) -> String {
        format!("{}/chat/completions", endpoint.trim_end_matches('/'))
    }

    /// 判断是否为“可能由 stream_options 字段引发”的客户端错误
    ///
    /// 用于 stream_options 降级重试：部分 OpenAI 兼容上游（旧版 vLLM、
    /// 某些中转代理）不识别 `stream_options` 字段会返回 400/422。
    /// 有意排除 401/403/404/429 等与请求体无关的错误，
    /// 避免对无效 Key / 限流场景重发注定失败的请求。
    fn is_client_error(err: &KeyComputeError) -> bool {
        matches!(
            err,
            KeyComputeError::ProviderError(msg)
                if msg.contains("HTTP error (400") || msg.contains("HTTP error (422")
        )
    }

    /// 构建 OpenAI 请求体（支持 Vision 多模态）
    fn build_request_body(&self, request: &UpstreamRequest) -> OpenAIRequest {
        let messages: Vec<OpenAIMessage> = request
            .messages
            .iter()
            .map(|m| OpenAIMessage {
                role: m.role.clone(),
                content: Some(convert_message_content(m.content.clone())),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
            .collect();

        OpenAIRequest {
            model: request.model.clone(),
            messages,
            stream: Some(request.stream),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            top_p: request.top_p,
            stop: None,
            stream_options: if request.stream {
                Some(StreamOptions {
                    include_usage: Some(true),
                })
            } else {
                None
            },
        }
    }

    /// 执行非流式请求
    async fn chat_internal(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<(String, Option<(u32, u32)>, Option<String>)> {
        // 返回 (content, usage, finish_reason) 元组
        let body = self.build_request_body(&request);
        let body_json = serde_json::to_string(&body).map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to serialize request: {}", e))
        })?;

        let headers = vec![
            (
                "Authorization".to_string(),
                format!("Bearer {}", request.upstream_api_key.expose()),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        let response_text = transport
            .post_json(&Self::chat_url(&request.endpoint), headers, body_json)
            .await?;

        let openai_response: OpenAIResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                KeyComputeError::ProviderError(format!("Failed to parse response: {}", e))
            })?;

        // 使用 OpenAIResponse 的方法一次性提取所有字段
        let content = openai_response.extract_text();
        let finish_reason = openai_response.extract_finish_reason();

        // 提取 usage 信息（非流式响应通常包含完整的 usage 数据）
        let usage = openai_response
            .usage
            .map(|u| (u.prompt_tokens as u32, u.completion_tokens as u32));

        Ok((content, usage, finish_reason))
    }

    /// 执行流式请求
    async fn stream_chat_internal(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<StreamBox> {
        let body = self.build_request_body(&request);
        let body_json = serde_json::to_string(&body).map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to serialize request: {}", e))
        })?;

        let headers = vec![
            (
                "Authorization".to_string(),
                format!("Bearer {}", request.upstream_api_key.expose()),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "text/event-stream".to_string()),
        ];

        let url = Self::chat_url(&request.endpoint);
        let byte_stream: ByteStream = match transport
            .post_stream(&url, headers.clone(), body_json)
            .await
        {
            Ok(stream) => stream,
            // 部分 OpenAI 兼容上游不认识 stream_options 字段而返回 4xx，
            // 去掉该字段重试一次（代价：收不到精确 Usage，计费退化为估算）
            Err(e) if body.stream_options.is_some() && Self::is_client_error(&e) => {
                tracing::warn!(
                    error = %e,
                    "Upstream rejected stream_options, retrying without it"
                );
                let mut fallback_body = body;
                fallback_body.stream_options = None;
                let fallback_json = serde_json::to_string(&fallback_body).map_err(|e| {
                    KeyComputeError::ProviderError(format!("Failed to serialize request: {}", e))
                })?;
                transport.post_stream(&url, headers, fallback_json).await?
            }
            Err(e) => return Err(e),
        };

        // 转换字节流为 SSE 事件流
        Ok(parse_openai_stream(byte_stream))
    }

    // ========================================================================
    // 图片生成
    // ========================================================================

    /// 构建 JSON API 通用请求头（Authorization + Content-Type: application/json）
    ///
    /// 用于 generate_image、create_response 等使用 JSON body 的非流式端点。
    /// multipart 场景（edit_image / create_image_variation）使用 `build_auth_header`
    /// 单独构建 Authorization 头，因为 Content-Type 需要动态设置为 multipart 边界值。
    fn build_json_api_headers(&self, api_key: &str) -> Vec<(String, String)> {
        vec![
            ("Authorization".to_string(), format!("Bearer {}", api_key)),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]
    }

    /// 执行图片生成
    pub async fn generate_image(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &str,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse> {
        let body_json = serde_json::to_string(&request).map_err(|e| {
            KeyComputeError::ProviderError(format!(
                "Failed to serialize image generation request: {}",
                e
            ))
        })?;

        let headers = self.build_json_api_headers(api_key);

        let response_text = transport.post_json(endpoint, headers, body_json).await?;

        let response: ImageGenerationResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                KeyComputeError::ProviderError(format!(
                    "Failed to parse image generation response: {}",
                    e
                ))
            })?;

        Ok(response)
    }

    /// 执行图片生成（使用默认端点）
    pub async fn generate_image_default(
        &self,
        transport: &dyn HttpTransport,
        api_key: &str,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse> {
        self.generate_image(transport, OPENAI_IMAGE_GEN_ENDPOINT, api_key, request)
            .await
    }

    // ========================================================================
    // 图片编辑
    // ========================================================================

    /// 构建 multipart/form-data 请求体
    ///
    /// 手动构建 multipart body，避免在 provider adapter 层引入 reqwest 依赖。
    ///
    /// # 安全
    /// - 所有字段值、文件名、content_type 中的控制字符（如 \r\n）会被过滤，防止 CRLF 注入攻击
    /// - 引号和反斜杠也会被过滤，防止引用逃逸
    fn build_multipart_body(
        text_fields: &[(&str, &str)],
        file_fields: &[(&str, &str, &str, &[u8])], // (name, filename, content_type, data)
    ) -> (Vec<u8>, String) {
        let boundary = format!("----KeyComputeBoundary{}", uuid::Uuid::new_v4().as_simple());
        let mut body = Vec::new();

        // 清理 header 值中的危险字符：ASCII 控制字符（\r\n）和引号
        // 仅用于 multipart header（name/filename/content_type），防止 CRLF 注入和引用逃逸。
        // 不过滤反斜杠 `\`，因为：
        // 1. CRLF 注入防护核心是过滤 \r / \n（已由 is_ascii_control 覆盖）
        // 2. filename 可能包含合法反斜杠（如 Unix 路径），不应静默修改
        // 3. Content-Disposition quoted-string 中仅需过滤 `"` 即可防止逃逸
        // 注意：文本字段的 body 值（如 prompt）不应调用此函数，因为：
        // 1. boundary 是随机 UUID，绝无碰撞可能
        // 2. prompt 中的引号和反斜杠是合法的用户输入
        fn sanitize_header_value(s: &str) -> String {
            s.chars()
                .filter(|c| !c.is_ascii_control() && *c != '"')
                .collect()
        }

        // 文本字段：name 做 header 安全过滤，但 value（如 prompt）不过滤
        for (name, value) in text_fields {
            let sanitized_name = sanitize_header_value(name);
            body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
            body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                    sanitized_name
                )
                .as_bytes(),
            );
            body.extend_from_slice(value.as_bytes());
            body.extend_from_slice(b"\r\n");
        }

        // 文件字段：name/filename/content_type 都需要 header 安全过滤
        for (name, filename, content_type, data) in file_fields {
            let sanitized_name = sanitize_header_value(name);
            let sanitized_filename = sanitize_header_value(filename);
            let sanitized_content_type = sanitize_header_value(content_type);

            body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
            body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                    sanitized_name, sanitized_filename
                )
                .as_bytes(),
            );
            body.extend_from_slice(
                format!("Content-Type: {}\r\n\r\n", sanitized_content_type).as_bytes(),
            );
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }

        // 结束边界
        body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

        let content_type = format!("multipart/form-data; boundary={}", boundary);
        (body, content_type)
    }

    /// 构建图片 API 通用文本字段列表
    ///
    /// 图片编辑和图片变体 API 共享相同的可选参数（model, n, size,
    /// response_format, user），此方法提取它们的通用构建逻辑。
    /// extra_fields 用于添加非共享字段（如 edit 的 prompt）。
    fn build_image_text_fields(
        model: &Option<String>,
        n: Option<u32>,
        size: &Option<String>,
        response_format: &Option<String>,
        user: &Option<String>,
        extra_fields: &[(&str, String)],
    ) -> Vec<(String, String)> {
        let mut text_fields: Vec<(String, String)> = extra_fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        if let Some(model) = model {
            text_fields.push(("model".to_string(), model.clone()));
        }
        if let Some(n) = n {
            text_fields.push(("n".to_string(), n.to_string()));
        }
        if let Some(size) = size {
            text_fields.push(("size".to_string(), size.clone()));
        }
        if let Some(fmt) = response_format {
            text_fields.push(("response_format".to_string(), fmt.clone()));
        }
        if let Some(user) = user {
            text_fields.push(("user".to_string(), user.clone()));
        }
        text_fields
    }

    /// 构建图片 API 请求头（multipart 场景需要动态 Content-Type，不在此设置）
    fn build_auth_header(&self, api_key: &str) -> (String, String) {
        ("Authorization".to_string(), format!("Bearer {}", api_key))
    }

    /// 执行 multipart/form-data 图片请求的通用逻辑
    ///
    /// `edit_image` 和 `create_image_variation` 共享的核心流程：
    /// 构建文本字段 → 构建文件字段 → 构建 multipart body → 发送请求 → 解析 JSON
    #[allow(clippy::too_many_arguments)]
    async fn execute_image_multipart_request(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &str,
        model: &Option<String>,
        n: Option<u32>,
        size: &Option<String>,
        response_format: &Option<String>,
        user: &Option<String>,
        extra_text_fields: &[(&str, String)],
        file_fields: &[(&str, &str, &str, &[u8])],
        error_label: &str,
    ) -> Result<ImageGenerationResponse> {
        let text_fields =
            Self::build_image_text_fields(model, n, size, response_format, user, extra_text_fields);
        let text_refs: Vec<(&str, &str)> = text_fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let (body, content_type) = Self::build_multipart_body(&text_refs, file_fields);

        let auth_header = self.build_auth_header(api_key);
        let headers = vec![auth_header, ("Content-Type".to_string(), content_type)];

        let response_text = transport.post_raw(endpoint, headers, body).await?;

        let response: ImageGenerationResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                KeyComputeError::ProviderError(format!(
                    "Failed to parse {} response: {}",
                    error_label, e
                ))
            })?;

        Ok(response)
    }

    /// 执行图片编辑（使用 multipart/form-data）
    ///
    /// OpenAI /v1/images/edits 端点的正确调用方式：
    /// - prompt: 文本字段
    /// - image: 文件字段（原始图片二进制）
    /// - mask: 文件字段（可选，遮罩图片二进制）
    /// - model/n/size/response_format/user: 文本字段
    pub async fn edit_image(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &str,
        request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse> {
        let stored_prompt = request.prompt.clone();
        let extra_fields = [("prompt", stored_prompt)];

        let mut file_fields: Vec<(&str, &str, &str, &[u8])> = vec![(
            "image",
            request.image_filename.as_str(),
            request.image_content_type.as_str(),
            &request.image,
        )];

        // mask 字段需要存活到 file_fields 使用完毕
        let mask_fn;
        let mask_ct;
        if let Some(ref mask) = request.mask {
            mask_fn = request
                .mask_filename
                .as_deref()
                .unwrap_or("mask.png")
                .to_string();
            mask_ct = request
                .mask_content_type
                .as_deref()
                .unwrap_or("image/png")
                .to_string();
            file_fields.push(("mask", mask_fn.as_str(), mask_ct.as_str(), mask));
        }

        self.execute_image_multipart_request(
            transport,
            endpoint,
            api_key,
            &request.model,
            request.n,
            &request.size,
            &request.response_format,
            &request.user,
            &extra_fields,
            &file_fields,
            "image edit",
        )
        .await
    }

    /// 执行图片编辑（使用默认端点）
    pub async fn edit_image_default(
        &self,
        transport: &dyn HttpTransport,
        api_key: &str,
        request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse> {
        self.edit_image(transport, OPENAI_IMAGE_EDIT_ENDPOINT, api_key, request)
            .await
    }

    // ========================================================================
    // 图片变体
    // ========================================================================

    /// 执行图片变体请求（使用 multipart/form-data）
    pub async fn create_image_variation(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &str,
        request: ImageVariationRequest,
    ) -> Result<ImageGenerationResponse> {
        let file_fields: Vec<(&str, &str, &str, &[u8])> = vec![(
            "image",
            request.image_filename.as_str(),
            request.image_content_type.as_str(),
            &request.image,
        )];

        self.execute_image_multipart_request(
            transport,
            endpoint,
            api_key,
            &request.model,
            request.n,
            &request.size,
            &request.response_format,
            &request.user,
            &[],
            &file_fields,
            "image variation",
        )
        .await
    }

    /// 执行图片变体（使用默认端点）
    pub async fn create_image_variation_default(
        &self,
        transport: &dyn HttpTransport,
        api_key: &str,
        request: ImageVariationRequest,
    ) -> Result<ImageGenerationResponse> {
        self.create_image_variation(transport, OPENAI_IMAGE_VARIATION_ENDPOINT, api_key, request)
            .await
    }

    // ========================================================================
    // Responses API（统一多模态接口）
    // ========================================================================

    /// 执行 Responses API 非流式请求
    ///
    /// Responses API 是 OpenAI 最新的统一接口，支持：
    /// - 文本 + 图片多模态输入
    /// - 工具调用（image_generation, web_search, file_search）
    /// - 状态保持
    pub async fn create_response(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &str,
        request: ResponsesRequest,
    ) -> Result<ResponsesResponse> {
        let body_json = serde_json::to_string(&request).map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to serialize responses request: {}", e))
        })?;

        let headers = self.build_json_api_headers(api_key);

        let response_text = transport.post_json(endpoint, headers, body_json).await?;

        let response: ResponsesResponse = serde_json::from_str(&response_text).map_err(|e| {
            KeyComputeError::ProviderError(format!("Failed to parse responses response: {}", e))
        })?;

        Ok(response)
    }

    /// 执行 Responses API 请求（使用默认端点）
    pub async fn create_response_default(
        &self,
        transport: &dyn HttpTransport,
        api_key: &str,
        request: ResponsesRequest,
    ) -> Result<ResponsesResponse> {
        self.create_response(transport, OPENAI_RESPONSES_ENDPOINT, api_key, request)
            .await
    }

    /// 执行 Responses API 流式请求
    pub async fn stream_response(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &str,
        request: ResponsesRequest,
    ) -> Result<StreamBox> {
        let mut stream_req = request;
        stream_req.stream = Some(true);

        let body_json = serde_json::to_string(&stream_req).map_err(|e| {
            KeyComputeError::ProviderError(format!(
                "Failed to serialize responses stream request: {}",
                e
            ))
        })?;

        let headers = vec![
            ("Authorization".to_string(), format!("Bearer {}", api_key)),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "text/event-stream".to_string()),
        ];

        let byte_stream: ByteStream = transport.post_stream(endpoint, headers, body_json).await?;

        Ok(parse_openai_stream(byte_stream))
    }

    /// 执行 Responses API 流式请求（使用默认端点）
    pub async fn stream_response_default(
        &self,
        transport: &dyn HttpTransport,
        api_key: &str,
        request: ResponsesRequest,
    ) -> Result<StreamBox> {
        self.stream_response(transport, OPENAI_RESPONSES_ENDPOINT, api_key, request)
            .await
    }
}

#[async_trait]
impl ProviderAdapter for OpenAIProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        // 协议层不维护模型白名单，模型由渠道账号的 models_supported 声明
        Vec::new()
    }

    /// 协议层接受任意模型，路由层已按账号 models_supported 过滤
    fn supports_model(&self, _model: &str) -> bool {
        true
    }

    /// OpenAI 原生支持图片生成（DALL-E）
    fn supports_image_generation(&self) -> bool {
        true
    }

    /// OpenAI 原生支持图片编辑
    fn supports_image_editing(&self) -> bool {
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
                // 非流式响应有finish_reason，设为Some
                finish_reason: Some(finish_reason.unwrap_or_else(|| "stop".to_string())),
            };

            let mut events: Vec<Result<StreamEvent>> = vec![Ok(event)];

            // 如果有 usage 信息，添加 Usage 事件
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_protocol_provider::ByteStream;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Mock 传输层：首次 post_stream 返回指定 HTTP 错误，后续调用成功，
    /// 并记录每次请求的 body 用于断言 stream_options 降级行为
    #[derive(Debug)]
    struct DegradingTransport {
        /// 首次请求返回的错误消息
        first_error: String,
        /// 记录收到的请求 body
        bodies: Mutex<Vec<String>>,
    }

    impl DegradingTransport {
        fn new(first_error: impl Into<String>) -> Self {
            Self {
                first_error: first_error.into(),
                bodies: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl HttpTransport for DegradingTransport {
        async fn post_json(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
            _body: String,
        ) -> Result<String> {
            Err(KeyComputeError::ProviderError("not used".into()))
        }

        async fn post_stream(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
            body: String,
        ) -> Result<ByteStream> {
            let mut bodies = self.bodies.lock().unwrap();
            bodies.push(body);
            if bodies.len() == 1 {
                Err(KeyComputeError::ProviderError(self.first_error.clone()))
            } else {
                Ok(Box::pin(futures::stream::empty()))
            }
        }

        fn request_timeout(&self) -> Duration {
            Duration::from_secs(1)
        }

        fn stream_timeout(&self) -> Duration {
            Duration::from_secs(1)
        }
    }

    fn stream_request() -> UpstreamRequest {
        UpstreamRequest::new("https://api.openai.com/v1", "sk-test", "gpt-4o")
            .with_message("user", "Hello")
            .with_stream(true)
    }

    #[tokio::test]
    async fn test_stream_options_degradation_on_400() {
        // 上游不认识 stream_options 返回 400 时，应去掉该字段重试一次
        let provider = OpenAIProvider::new();
        let transport =
            DegradingTransport::new("HTTP error (400 Bad Request): unknown field stream_options");

        let result = provider.stream_chat(&transport, stream_request()).await;
        assert!(result.is_ok(), "degraded retry should succeed");

        let bodies = transport.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2, "should retry exactly once");
        assert!(
            bodies[0].contains("stream_options"),
            "first request should carry stream_options"
        );
        assert!(
            !bodies[1].contains("stream_options"),
            "retry request should drop stream_options"
        );
    }

    #[tokio::test]
    async fn test_stream_options_degradation_on_422() {
        let provider = OpenAIProvider::new();
        let transport = DegradingTransport::new(
            "HTTP error (422 Unprocessable Entity): extra field stream_options",
        );

        let result = provider.stream_chat(&transport, stream_request()).await;
        assert!(result.is_ok());
        assert_eq!(transport.bodies.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_stream_options_no_retry_on_auth_or_ratelimit() {
        // 401/429 与请求体无关，不应触发降级重试
        for error in [
            "HTTP error (401 Unauthorized): invalid api key",
            "HTTP error (429 Too Many Requests): rate limited",
        ] {
            let provider = OpenAIProvider::new();
            let transport = DegradingTransport::new(error);

            let result = provider.stream_chat(&transport, stream_request()).await;
            assert!(result.is_err(), "{error} should not be retried");
            assert_eq!(
                transport.bodies.lock().unwrap().len(),
                1,
                "{error} should only be sent once"
            );
        }
    }

    #[test]
    fn test_openai_provider_name() {
        let provider = OpenAIProvider::new();
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn test_openai_supported_models_empty() {
        let provider = OpenAIProvider::new();
        // 协议层不维护模型白名单
        assert!(provider.supported_models().is_empty());
    }

    #[test]
    fn test_openai_supports_any_model() {
        let provider = OpenAIProvider::new();
        assert!(provider.supports_model("gpt-4o"));
        assert!(provider.supports_model("deepseek-chat"));
        assert!(provider.supports_model("any-model"));
    }

    #[test]
    fn test_chat_url_join() {
        assert_eq!(
            OpenAIProvider::chat_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            OpenAIProvider::chat_url("https://api.deepseek.com/v1/"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // 空 endpoint 回落的协议默认 Base URL 必须能拼出完整请求 URL
        assert_eq!(
            OpenAIProvider::chat_url(
                llm_protocol_provider::ProtocolType::Openai.default_endpoint()
            ),
            OPENAI_CHAT_ENDPOINT
        );
    }

    #[test]
    fn test_build_request_body() {
        let provider = OpenAIProvider::new();
        let request = UpstreamRequest::new("https://api.openai.com/v1", "sk-test", "gpt-4o")
            .with_message("system", "You are helpful")
            .with_message("user", "Hello")
            .with_stream(true)
            .with_max_tokens(100)
            .with_temperature(0.7);

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "gpt-4o");
        assert_eq!(body.messages.len(), 2);
        assert_eq!(body.stream, Some(true));
        assert_eq!(body.max_tokens, Some(100));
        assert_eq!(body.temperature, Some(0.7));
    }
}
