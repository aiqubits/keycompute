//! OpenAI 兼容 API 处理器
//
//! 提供与 OpenAI API 完全兼容的接口
//! 参考: https://platform.openai.com/docs/api-reference

use crate::{
    error::{ApiError, Result},
    extractors::{AuthExtractor, RequestId},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
};
use futures::stream::Stream;
use keycompute_db::models::account::Account;
use keycompute_types::{
    ContentPart, ExecutionTarget, Message, MessageContent, MessageRole, RequestContext,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;

// ==================== Chat Completions ====================

/// Chat Completions 请求
/// 与 OpenAI API 完全对齐: https://platform.openai.com/docs/api-reference/chat/create
#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    /// 模型 ID (必需)
    pub model: String,
    /// 消息列表 (必需)
    pub messages: Vec<ChatCompletionMessage>,
    /// 是否流式输出 (默认 false)
    #[serde(default)]
    pub stream: bool,
    /// 最大生成 token 数
    #[serde(rename = "max_tokens")]
    pub max_tokens: Option<u32>,
    /// 最大生成 token 数（OpenAI 新版字段，与 max_tokens 等效；
    /// 两者同时提供时 max_tokens 优先）
    pub max_completion_tokens: Option<u32>,
    /// 温度参数 (0-2)
    pub temperature: Option<f32>,
    /// 核采样参数 (0-1)
    pub top_p: Option<f32>,
    /// 每个提示生成的结果数 (默认 1)
    #[serde(default = "default_n")]
    pub n: Option<u32>,
    /// 是否返回输入 token 的用量
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    /// 停止序列
    pub stop: Option<StopSequence>,
    /// 存在惩罚 (-2.0 到 2.0)
    pub presence_penalty: Option<f32>,
    /// 频率惩罚 (-2.0 到 2.0)
    pub frequency_penalty: Option<f32>,
    /// 日志概率 (0-5)
    pub logprobs: Option<bool>,
    /// 返回的日志概率选项数
    pub top_logprobs: Option<u32>,
    /// 用户标识 (用于监控滥用)
    pub user: Option<String>,
    /// 响应格式 (如 json_object)
    pub response_format: Option<ResponseFormat>,
    /// 种子值 (用于可重复的结果)
    pub seed: Option<i64>,
    /// 工具列表
    pub tools: Option<Vec<Tool>>,
    /// 工具选择策略
    pub tool_choice: Option<ToolChoice>,
}

fn default_n() -> Option<u32> {
    Some(1)
}

impl ChatCompletionRequest {
    /// 生效的最大生成 token 数（max_tokens 优先，回退 max_completion_tokens）
    fn effective_max_tokens(&self) -> Option<u32> {
        self.max_tokens.or(self.max_completion_tokens)
    }

    /// 校验采样参数范围
    ///
    /// 越界参数在 handler 层直接返回 400，避免确定性的上游 400
    /// 级联整条 fallback 链（浪费上游调用）并污染 Provider 健康评分。
    /// 注：NaN 不在任何区间内，同样会被拒绝
    fn validate_sampling_params(&self) -> Result<()> {
        if self.effective_max_tokens() == Some(0) {
            return Err(ApiError::BadRequest(
                "max_tokens must be greater than 0".to_string(),
            ));
        }
        if let Some(temperature) = self.temperature
            && !(0.0..=2.0).contains(&temperature)
        {
            return Err(ApiError::BadRequest(
                "temperature must be between 0.0 and 2.0".to_string(),
            ));
        }
        if let Some(top_p) = self.top_p
            && !(0.0..=1.0).contains(&top_p)
        {
            return Err(ApiError::BadRequest(
                "top_p must be between 0.0 and 1.0".to_string(),
            ));
        }
        Ok(())
    }
}

/// Chat Completion 消息
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatCompletionMessage {
    /// 角色: system, user, assistant, tool
    pub role: String,
    /// 内容：支持纯文本字符串或 Vision 多模态内容块数组
    pub content: Option<MessageContent>,
    /// 工具调用 (assistant 消息中)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 工具调用 ID (tool 消息中)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 名称 (function 消息中)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// 工具定义
#[derive(Debug, Deserialize)]
pub struct Tool {
    /// 工具类型 (目前只有 function)
    #[serde(rename = "type")]
    pub tool_type: String,
    /// 函数定义
    pub function: FunctionDefinition,
}

/// 函数定义
#[derive(Debug, Deserialize)]
pub struct FunctionDefinition {
    /// 函数名称
    pub name: String,
    /// 函数描述
    pub description: Option<String>,
    /// 参数定义 (JSON Schema)
    pub parameters: serde_json::Value,
}

/// 工具调用
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    /// 调用 ID
    pub id: String,
    /// 调用类型
    #[serde(rename = "type")]
    pub call_type: String,
    /// 函数调用
    pub function: FunctionCall,
}

/// 函数调用
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionCall {
    /// 函数名称
    pub name: String,
    /// 参数 (JSON 字符串)
    pub arguments: String,
}

/// 工具选择
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// 字符串选项: none, auto, required
    String(String),
    /// 指定调用特定函数
    Object {
        #[serde(rename = "type")]
        tool_type: String,
        function: FunctionChoice,
    },
}

/// 函数选择
#[derive(Debug, Deserialize)]
pub struct FunctionChoice {
    pub name: String,
}

/// 流式选项
#[derive(Debug, Deserialize)]
pub struct StreamOptions {
    /// 在流式消息的最后包含用量信息
    #[serde(default)]
    pub include_usage: bool,
}

/// 停止序列
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum StopSequence {
    /// 单个字符串
    String(String),
    /// 字符串数组 (最多 4 个)
    Array(Vec<String>),
}

/// 响应格式
#[derive(Debug, Deserialize)]
pub struct ResponseFormat {
    /// 格式类型: text 或 json_object
    #[serde(rename = "type")]
    pub format_type: String,
}

/// Chat Completion 响应 (非流式)
#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    /// 响应 ID
    pub id: String,
    /// 对象类型: chat.completion
    pub object: String,
    /// 创建时间戳 (Unix)
    pub created: i64,
    /// 模型名称
    pub model: String,
    /// 选择列表
    pub choices: Vec<ChatCompletionChoice>,
    /// 用量信息
    pub usage: CompletionUsage,
    /// 系统指纹
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
}

/// Chat Completion 选择项
#[derive(Debug, Serialize)]
pub struct ChatCompletionChoice {
    /// 索引
    pub index: u32,
    /// 消息
    pub message: ChatCompletionMessage,
    /// 结束原因: stop, length, content_filter, tool_calls
    pub finish_reason: Option<String>,
    /// 日志概率信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<serde_json::Value>,
}

/// 用量信息
#[derive(Debug, Serialize)]
pub struct CompletionUsage {
    /// 输入 token 数
    pub prompt_tokens: u32,
    /// 输出 token 数
    pub completion_tokens: u32,
    /// 总 token 数
    pub total_tokens: u32,
    /// 详细 token 信息 (可选)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<TokenDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<TokenDetails>,
}

/// Token 详情
#[derive(Debug, Serialize)]
pub struct TokenDetails {
    /// 缓存的 token 数
    pub cached_tokens: Option<u32>,
    /// 音频 token 数
    pub audio_tokens: Option<u32>,
}

/// Chat Completion 流式响应块
#[derive(Debug, Serialize)]
pub struct ChatCompletionChunk {
    /// 响应 ID
    pub id: String,
    /// 对象类型: chat.completion.chunk
    pub object: String,
    /// 创建时间戳
    pub created: i64,
    /// 模型名称
    pub model: String,
    /// 系统指纹
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    /// 选择列表
    pub choices: Vec<ChatCompletionChunkChoice>,
    /// 用量信息 (仅在最后一块，如果 stream_options.include_usage 为 true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<CompletionUsage>,
}

/// Chat Completion 流式选择项
#[derive(Debug, Serialize)]
pub struct ChatCompletionChunkChoice {
    /// 索引
    pub index: u32,
    /// Delta 内容
    pub delta: ChatCompletionChunkDelta,
    /// 结束原因
    pub finish_reason: Option<String>,
    /// 日志概率
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<serde_json::Value>,
}

/// Delta 内容
#[derive(Debug, Serialize, Default)]
pub struct ChatCompletionChunkDelta {
    /// 角色 (仅第一条)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 内容
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 工具调用
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Chat Completions 处理器
/// POST /v1/chat/completions
///
/// 注意：限流已在中间件层统一处理，此处直接开始业务逻辑
pub async fn chat_completions(
    State(state): State<AppState>,
    auth: AuthExtractor,
    request_id: RequestId,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<axum::response::Response> {
    // 0. 采样参数范围校验（越界直接 400，不进入路由/上游调用）
    request.validate_sampling_params()?;

    // 1. 余额预检查
    // 如果余额低于阈值（0.1元），直接拒绝请求
    if let Some(balance_service) = state.billing.balance_service() {
        balance_service
            .check_balance_for_tenant(auth.user_id, auth.tenant_id)
            .await
            .map_err(ApiError::from)?;
    }

    // 1. 构建 PricingSnapshot
    // 注意：此时 provider 尚未确定（路由在之后执行）
    // Node 模型（node:前缀）使用 empty provider，其他使用 openai
    let provider = keycompute_pricing::resolve_pricing_provider(&request.model);
    let pricing = state
        .pricing
        .create_snapshot(&request.model, &auth.tenant_id, Some(provider))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create pricing snapshot: {}", e)))?;

    // 3. 转换消息格式
    let messages: Vec<Message> = request
        .messages
        .iter()
        .map(|m| {
            let role = match m.role.as_str() {
                "system" => MessageRole::System,
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                "tool" => MessageRole::Tool,
                _ => MessageRole::User, // 默认角色
            };
            Message {
                role,
                content: m
                    .content
                    .clone()
                    .unwrap_or(MessageContent::Text(String::new())),
            }
        })
        .collect();

    // 4. 构建 RequestContext
    let mut request_ctx = RequestContext::new(
        auth.user_id,
        auth.tenant_id,
        auth.produce_ai_key_id,
        request.model.clone(),
        messages,
        request.stream,
        pricing,
    );
    // 透传客户端采样参数（协议层构建上游请求时使用，
    // Anthropic 协议的 max_tokens 为必填字段，不透传会被默认值硬截断）
    request_ctx.max_tokens = request.effective_max_tokens();
    request_ctx.temperature = request.temperature;
    request_ctx.top_p = request.top_p;
    let mut ctx = Arc::new(request_ctx);

    // 5. 智能路由
    let plan = state
        .routing
        .route(&ctx)
        .await
        .map_err(|e| ApiError::Internal(format!("Routing failed: {}", e)))?;

    // 5. 根据 ExecutionTarget 分流执行路径
    match &plan.primary {
        ExecutionTarget::Node { model } => {
            // 更新 ctx 的 model 字段（使用去掉前缀的实际模型名）
            let ctx_mut = Arc::make_mut(&mut ctx);
            ctx_mut.model = model.clone();

            // 更新定价快照（使用实际模型名和 NODE_PRICING_PROVIDER 进行定价查找）
            // 注意：必须先调用 update_context_pricing，再设置 provider
            // 因为 update_context_pricing 会检查 provider 是否变化
            state
                .pricing
                .update_context_pricing(ctx_mut, keycompute_pricing::NODE_PRICING_PROVIDER)
                .await;

            // 设置 provider 字段（用于日志追踪和后续逻辑）
            ctx_mut.set_provider(keycompute_pricing::NODE_PRICING_PROVIDER);

            // 调用 node-gateway 执行
            let node_gateway = state
                .node_gateway
                .as_ref()
                .ok_or_else(|| ApiError::Internal("node gateway not configured".to_string()))?;

            // 构建 NodeTaskPayload
            let payload = keycompute_types::node::NodeTaskPayload {
                request_id: ctx.request_id,
                chat: Some(keycompute_types::ChatCompletionRequest {
                    model: model.clone(), // 使用去掉 node: 前缀的实际模型名
                    messages: ctx.messages.clone(),
                    stream: Some(request.stream), // 传递 stream 标志
                    max_tokens: request.effective_max_tokens(),
                    temperature: request.temperature,
                    top_p: request.top_p,
                    n: request.n,
                    stop: None, // StopSequence 不支持 Clone，暂时使用 None
                }),
                image_generation: None,
                image_edit: None,
            };

            // 防御性校验 payload 互斥性
            if let Err(e) = payload.validate() {
                return Err(ApiError::Internal(format!(
                    "Invalid NodeTaskPayload: {}",
                    e
                )));
            }

            if request.stream {
                // 流式路径：获取完整响应后模拟流式输出
                let response = node_gateway
                    .enqueue_and_wait(auth.user_id, model.clone(), payload)
                    .await
                    .map_err(ApiError::from)?;

                // 更新 token 计数到 ctx（用于计费）
                ctx.set_input_tokens(response.usage.prompt_tokens);
                ctx.add_output_tokens(response.usage.completion_tokens);

                // 将完整响应转换为模拟流式输出
                let stream = simulate_node_stream(
                    response,
                    ctx,
                    model.clone(),
                    Arc::clone(&state.billing),
                    request.stream_options,
                );
                Ok(Sse::new(stream).into_response())
            } else {
                // 非流式路径：保持现有逻辑
                let response = node_gateway
                    .enqueue_and_wait(auth.user_id, model.clone(), payload)
                    .await
                    .map_err(ApiError::from)?;

                // 更新 token 计数到 ctx（用于计费）
                ctx.set_input_tokens(response.usage.prompt_tokens);
                ctx.add_output_tokens(response.usage.completion_tokens);

                // 将 ChatCompletionResponse 转换为 OpenAI 格式
                let openai_response = ChatCompletionResponse {
                    id: format!(
                        "chatcmpl-{}-kc",
                        uuid::Uuid::new_v4()
                            .to_string()
                            .replace("-", "")
                            .to_lowercase()
                    ),
                    object: "chat.completion".to_string(),
                    created: chrono::Utc::now().timestamp(),
                    model: model.clone(),
                    choices: vec![ChatCompletionChoice {
                        index: 0,
                        message: ChatCompletionMessage {
                            role: "assistant".to_string(),
                            content: response
                                .choices
                                .first()
                                .map(|c| MessageContent::text(c.message.content.clone())),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                        },
                        finish_reason: response
                            .choices
                            .first()
                            .and_then(|c| c.finish_reason.clone()),
                        logprobs: None,
                    }],
                    usage: CompletionUsage {
                        prompt_tokens: response.usage.prompt_tokens as u32,
                        completion_tokens: response.usage.completion_tokens as u32,
                        total_tokens: response.usage.total_tokens as u32,
                        prompt_tokens_details: None,
                        completion_tokens_details: None,
                    },
                    system_fingerprint: None,
                };

                // 触发计费（使用 NODE_PRICING_PROVIDER 常量，与路由层定价维度一致）
                let billing = Arc::clone(&state.billing);
                let _ = billing
                    .finalize_and_trigger_distribution(
                        &ctx,
                        keycompute_pricing::NODE_PRICING_PROVIDER,
                        uuid::Uuid::nil(),
                        "success",
                        auth.user_id,
                    )
                    .await;

                Ok(Json(openai_response).into_response())
            }
        }
        ExecutionTarget::ProviderAccount {
            provider,
            account_id,
            ..
        } => {
            // Provider 执行路径：继续后续逻辑
            let (primary_provider, primary_account_id) = (provider.clone(), *account_id);

            // 5.1 根据实际 provider 更新定价（如果需要）
            {
                let ctx_mut = Arc::make_mut(&mut ctx);
                state
                    .pricing
                    .update_context_pricing(ctx_mut, &primary_provider)
                    .await;
            }

            tracing::info!(
                request_id = %request_id.0,
                model = %request.model,
                stream = %request.stream,
                primary_provider = %primary_provider,
                "Chat completion request"
            );

            // 6. 执行（带超时保护）
            tracing::info!(
                request_id = %request_id.0,
                timeout_secs = state.gateway_config.timeout_secs,
                "Starting gateway execute"
            );

            let timeout_duration =
                std::time::Duration::from_secs(state.gateway_config.timeout_secs);
            let rx = match tokio::time::timeout(
                timeout_duration,
                state.gateway.execute(
                    Arc::clone(&ctx),
                    plan,
                    Arc::clone(&state.account_states),
                    Some(Arc::clone(&state.provider_health)),
                ),
            )
            .await
            {
                Ok(result) => {
                    result.map_err(|e| ApiError::Internal(format!("Execution failed: {}", e)))?
                }
                Err(_) => {
                    tracing::error!(
                        request_id = %request_id.0,
                        timeout_secs = state.gateway_config.timeout_secs,
                        "Gateway execute timeout"
                    );
                    return Err(ApiError::Internal(format!(
                        "Gateway execute timeout after {}s",
                        state.gateway_config.timeout_secs
                    )));
                }
            };

            tracing::info!(
                request_id = %request_id.0,
                "Gateway execute returned, creating response"
            );

            // 7. 根据 stream 参数返回不同类型的响应
            let billing = Arc::clone(&state.billing);
            let is_stream = request.stream;
            let model = request.model;
            let stream_options = request.stream_options;

            if is_stream {
                // 流式响应
                if has_image_content(&ctx.messages) {
                    // 流式 + 多模态：SSE keepalive 防止图片下载超时
                    let stream = create_openai_stream_with_keepalive(
                        rx,
                        ctx,
                        model,
                        primary_provider,
                        primary_account_id,
                        billing,
                        stream_options,
                    );
                    Ok(Sse::new(stream).into_response())
                } else {
                    // 流式 + 纯文本：原逻辑，无 keepalive
                    let stream = create_openai_stream(
                        rx,
                        ctx,
                        model,
                        primary_provider,
                        primary_account_id,
                        billing,
                        stream_options,
                    );
                    Ok(Sse::new(stream).into_response())
                }
            } else {
                // 非流式响应
                if has_image_content(&ctx.messages) {
                    // 多模态请求：使用 chunked keepalive 防止图片下载超时
                    Ok(create_non_streaming_json_with_keepalive(
                        rx,
                        ctx,
                        model,
                        primary_provider,
                        primary_account_id,
                        billing,
                    ))
                } else {
                    // 纯文本请求：直接返回 JSON（原快速路径）
                    let response = create_openai_response(
                        rx,
                        ctx,
                        model,
                        primary_provider,
                        primary_account_id,
                        billing,
                    )
                    .await?;
                    Ok(Json(response).into_response())
                }
            }
        }
    }
}

/// 创建 OpenAI 格式的非流式响应（纯文本快速路径）
async fn create_openai_response(
    mut rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    model: String,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
) -> Result<ChatCompletionResponse> {
    let completion_id = generate_completion_id();
    let created = chrono::Utc::now().timestamp();
    let mut collector = StreamCollector::new();

    // 收集所有事件
    while let Some(event) = rx.recv().await {
        match collector.process_event(event) {
            Ok(true) => {}
            Ok(false) => break,
            Err(message) => {
                tracing::error!(
                    request_id = %ctx.request_id,
                    error = %message,
                    "Stream error during non-streaming response"
                );
                let _ = billing
                    .finalize_and_trigger_distribution(
                        &ctx,
                        &provider_name,
                        account_id,
                        &collector.status,
                        ctx.user_id,
                    )
                    .await;
                return Err(ApiError::Internal(message));
            }
        }
    }

    // 检查流完成状态
    collector.check_completion(&ctx.request_id);

    // 流意外结束：先执行计费，再返回错误而非空 content 的 200 响应
    if collector.status == "incomplete" {
        let _ = billing
            .finalize_and_trigger_distribution(
                &ctx,
                &provider_name,
                account_id,
                &collector.status,
                ctx.user_id,
            )
            .await;
        return Err(ApiError::Internal(
            "Stream ended unexpectedly: channel closed without Done/Error event".to_string(),
        ));
    }

    // 执行 billing
    let _ = billing
        .finalize_and_trigger_distribution(
            &ctx,
            &provider_name,
            account_id,
            &collector.status,
            ctx.user_id,
        )
        .await;

    // 获取用量信息
    let (prompt_tokens, completion_tokens) = ctx.usage_snapshot();

    Ok(build_chat_completion_response(
        completion_id,
        created,
        model,
        collector.content,
        collector.finish_reason,
        prompt_tokens,
        completion_tokens,
        provider_name,
    ))
}

/// 检测消息列表中是否包含需要网络下载的图片 URL
///
/// 仅当存在 `ContentPart::ImageUrl` 且 URL 为 HTTP(S) 协议（非 data URI）时才返回 true。
/// data URI（如 `data:image/png;base64,...`）图片数据已内嵌在请求体中，
/// 上游 Provider 无需额外网络下载即可处理，不会触发超时问题。
/// 纯文本或仅有文本块的 Parts 不属于多模态。
fn has_image_content(messages: &[Message]) -> bool {
    messages.iter().any(|m| match &m.content {
        MessageContent::Parts(parts) => parts.iter().any(|p| match p {
            ContentPart::ImageUrl { image_url } => !image_url.url.starts_with("data:"),
            _ => false,
        }),
        MessageContent::Text(_) => false,
    })
}

/// 构建 OpenAI 格式的 ChatCompletion 响应
///
/// create_openai_response 与 create_non_streaming_json_with_keepalive 共享
#[allow(clippy::too_many_arguments)]
fn build_chat_completion_response(
    completion_id: String,
    created: i64,
    model: String,
    content: String,
    finish_reason: Option<String>,
    prompt_tokens: u32,
    completion_tokens: u32,
    provider_name: String,
) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: completion_id,
        object: "chat.completion".to_string(),
        created,
        model,
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatCompletionMessage {
                role: "assistant".to_string(),
                content: Some(MessageContent::text(content)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            finish_reason,
            logprobs: None,
        }],
        usage: CompletionUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        },
        system_fingerprint: Some(format!("fp_{}", provider_name)),
    }
}

/// 流事件收集器
///
/// 封装非流式响应路径中共享的事件处理状态与逻辑，
/// 消除 `create_openai_response` 与 `create_non_streaming_json_with_keepalive` 之间的重复。
struct StreamCollector {
    content: String,
    finish_reason: Option<String>,
    status: String,
    completed: bool,
}

impl StreamCollector {
    fn new() -> Self {
        Self {
            content: String::new(),
            finish_reason: None,
            status: "success".to_string(),
            completed: false,
        }
    }

    /// 处理单个流事件
    ///
    /// 返回值：
    /// - `Ok(true)` — 继续收集
    /// - `Ok(false)` — 流正常结束（收到 Done 事件）
    /// - `Err(message)` — 流异常（收到 Error 事件），调用者负责执行计费并决定错误输出方式
    fn process_event(
        &mut self,
        event: llm_protocol_provider::StreamEvent,
    ) -> std::result::Result<bool, String> {
        match event {
            llm_protocol_provider::StreamEvent::Delta {
                content: delta,
                finish_reason: reason,
            } => {
                self.content.push_str(&delta);
                if reason.is_some() {
                    self.finish_reason = reason;
                }
                Ok(true)
            }
            llm_protocol_provider::StreamEvent::Done => {
                self.completed = true;
                Ok(false)
            }
            llm_protocol_provider::StreamEvent::Error { message } => {
                self.status = "error".to_string();
                Err(message)
            }
            llm_protocol_provider::StreamEvent::Usage { .. }
            | llm_protocol_provider::StreamEvent::Raw { .. } => Ok(true),
        }
    }

    /// 检查流是否意外结束（channel 关闭但没有收到 Done/Error 事件）
    fn check_completion(&mut self, request_id: &uuid::Uuid) {
        if !self.completed {
            tracing::warn!(
                request_id = %request_id,
                "Non-streaming response: channel closed without Done/Error event"
            );
            self.status = "incomplete".to_string();
        }
    }
}

/// 生成 OpenAI 格式的 completion ID
fn generate_completion_id() -> String {
    format!(
        "chatcmpl-{}-kc",
        uuid::Uuid::new_v4()
            .to_string()
            .replace("-", "")
            .to_lowercase()
    )
}

/// 创建带 chunked keepalive 的非流式 JSON 响应
///
/// 利用 HTTP chunked transfer encoding，在等待上游 Provider 响应期间，
/// 每 10 秒发送一个空格字符 chunk，保持 TCP 连接活跃。
/// 空格是 JSON 规范（RFC 8259）允许的前导空白字符，JSON 解析器会自动忽略，
/// 因此客户端收到的是完全合法的 JSON 响应，协议无变更。
///
/// 适用场景：非流式请求中包含图片 URL 需要下载时，
/// 图片下载可能耗时 30-40 秒，期间无任何数据返回，
/// 云平台 ~60s 超时会导致 504 Gateway Timeout。
///
/// ## 错误处理说明
///
/// 由于 HTTP chunked 响应的特性，一旦第一个数据帧发出，HTTP 状态码 (200)
/// 即已提交，无法后续修改。因此当流内发生上游 Provider 错误时，错误以
/// JSON error body 形式嵌入响应体（而非 HTTP 5xx），客户端需同时检查
/// HTTP 状态码和响应体中的 `error` 字段来判定请求是否成功。
///
/// 首个 keepalive 在 ~10s 时发送（而非立即发送），为上游连接阶段的错误
/// （如 DNS 解析失败、TLS 握手超时等）保留一个窗口期。上游 Provider
/// 的连接错误通常在数秒内暴露，10s 间隔足以覆盖绝大多数场景。
fn create_non_streaming_json_with_keepalive(
    mut rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    model: String,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
) -> axum::response::Response {
    use std::time::Duration;

    let stream = async_stream::stream! {
        let completion_id = generate_completion_id();
        let created = chrono::Utc::now().timestamp();
        let mut collector = StreamCollector::new();

        // 首个 keepalive 不在此时发送，而是在 loop 内通过 tokio::select! 的
        // sleep 分支延迟 ~10s 触发。这样为上游连接阶段的错误（DNS/TLS 等）
        // 保留一个窗口期，避免过早提交 HTTP 200 状态码。
        //
        // 最大 keepalive 持续时间 120s，防止上游 Provider 后台任务停滞
        // 导致循环无限期发送空格字节。120s 足够覆盖图片下载（~30-40s）+
        // LLM 推理（~20-40s），同时避免服务端资源长期占用。
        let max_keepalive = Duration::from_secs(120);
        let deadline = tokio::time::sleep(max_keepalive);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => {
                    collector.status = "timeout".to_string();
                    tracing::error!(
                        request_id = %ctx.request_id,
                        "Non-streaming keepalive: max duration (120s) exceeded, terminating"
                    );
                    let _ = billing
                        .finalize_and_trigger_distribution(
                            &ctx, &provider_name, account_id, &collector.status, ctx.user_id,
                        )
                        .await;
                    let error_json = serde_json::json!({
                        "error": {
                            "message": "Request timed out",
                            "type": "server_error",
                            "param": null,
                            "code": "timeout"
                        }
                    });
                    yield Ok(bytes::Bytes::from(error_json.to_string()));
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    // 空格是合法 JSON 前导空白 (RFC 8259 §2)，
                    // 作为 chunked encoding 的数据帧，重置 Nginx proxy_read_timeout
                    yield Ok::<bytes::Bytes, Infallible>(
                        bytes::Bytes::from_static(b" ")
                    );
                }
                event = rx.recv() => {
                    match event {
                        Some(event) => match collector.process_event(event) {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(message) => {
                                tracing::error!(
                                    request_id = %ctx.request_id,
                                    error = %message,
                                    "Stream error during non-streaming keepalive response"
                                );
                                let _ = billing
                                    .finalize_and_trigger_distribution(
                                        &ctx, &provider_name, account_id, &collector.status, ctx.user_id,
                                    )
                                    .await;
                                // 错误格式与 OpenAI API 对齐，包含 param 字段
                                let error_json = serde_json::json!({
                                    "error": {
                                        "message": message,
                                        "type": "api_error",
                                        "param": null,
                                        "code": "internal_error"
                                    }
                                });
                                yield Ok(bytes::Bytes::from(error_json.to_string()));
                                return;
                            }
                        },
                        None => {
                            // Channel 关闭但没有收到 Done 事件，将在循环后标记为 incomplete
                            break;
                        }
                    }
                }
            }
        }

        // 检查流完成状态
        collector.check_completion(&ctx.request_id);

        // 流意外结束：先执行计费，再返回 error JSON 而非空 content 的 200 响应
        if collector.status == "incomplete" {
            let _ = billing
                .finalize_and_trigger_distribution(
                    &ctx, &provider_name, account_id, &collector.status, ctx.user_id,
                )
                .await;
            let error_json = serde_json::json!({
                "error": {
                    "message": "Stream ended unexpectedly",
                    "type": "server_error",
                    "param": null,
                    "code": "incomplete"
                }
            });
            yield Ok(bytes::Bytes::from(error_json.to_string()));
            return;
        }

        // 执行计费
        let _ = billing
            .finalize_and_trigger_distribution(
                &ctx, &provider_name, account_id, &collector.status, ctx.user_id,
            )
            .await;

        // 获取用量信息
        let (prompt_tokens, completion_tokens) = ctx.usage_snapshot();

        // 构建最终 JSON 响应
        let response = build_chat_completion_response(
            completion_id,
            created,
            model,
            collector.content,
            collector.finish_reason,
            prompt_tokens,
            completion_tokens,
            provider_name,
        );

        let json = serde_json::to_string(&response)
            .unwrap_or_else(|e| {
                tracing::error!(
                    request_id = %ctx.request_id,
                    error = %e,
                    "Failed to serialize chat completion response"
                );
                serde_json::json!({
                    "error": {
                        "message": "Internal error: failed to serialize response",
                        "type": "server_error",
                        "param": null,
                        "code": null
                    }
                })
                .to_string()
            });
        yield Ok(bytes::Bytes::from(json));
    };

    axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|e| {
            tracing::error!(
                error = %e,
                "Failed to build keepalive response headers, returning 500"
            );
            axum::response::Response::builder()
                .status(500)
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"error":{"message":"Internal server error","type":"server_error","param":null,"code":null}}"#,
                ))
                .expect("500 response with static body should always succeed")
        })
}

/// 构建流式 Delta chunk 的 SSE 数据字符串
///
/// 供 `create_openai_stream` 与 `create_openai_stream_with_keepalive` 共享，
/// 消除 chunk 构建逻辑的重复。仅首个 chunk 携带 `role: "assistant"`，
/// 遵循 OpenAI SSE 协议规范。
fn make_delta_chunk_data(
    content: String,
    finish_reason: &Option<String>,
    first_chunk: &mut bool,
    completion_id: &str,
    created: i64,
    model: &str,
    provider_name: &str,
) -> String {
    let delta = if *first_chunk {
        *first_chunk = false;
        ChatCompletionChunkDelta {
            role: Some("assistant".to_string()),
            content: Some(content),
            tool_calls: None,
        }
    } else {
        ChatCompletionChunkDelta {
            role: None,
            content: Some(content),
            tool_calls: None,
        }
    };

    let chunk = ChatCompletionChunk {
        id: completion_id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
        system_fingerprint: Some(format!("fp_{}", provider_name)),
        choices: vec![ChatCompletionChunkChoice {
            index: 0,
            delta,
            finish_reason: finish_reason.clone(),
            logprobs: None,
        }],
        usage: None,
    };

    serde_json::to_string(&chunk).unwrap_or_else(|e| {
        tracing::error!(
            completion_id = %completion_id,
            error = %e,
            "Failed to serialize delta chunk"
        );
        serde_json::json!({
            "error": {
                "message": "Internal error: failed to serialize delta chunk",
                "type": "server_error",
                "param": null,
                "code": null
            }
        })
        .to_string()
    })
}

/// 构建流式 Usage chunk 的 SSE 数据字符串
///
/// 供 `create_openai_stream` 与 `create_openai_stream_with_keepalive` 共享。
fn make_usage_chunk_data(
    input_tokens: u32,
    output_tokens: u32,
    completion_id: &str,
    created: i64,
    model: &str,
    provider_name: &str,
) -> String {
    let usage_chunk = ChatCompletionChunk {
        id: completion_id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
        system_fingerprint: Some(format!("fp_{}", provider_name)),
        choices: vec![],
        usage: Some(CompletionUsage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }),
    };

    serde_json::to_string(&usage_chunk).unwrap_or_else(|e| {
        tracing::error!(
            completion_id = %completion_id,
            error = %e,
            "Failed to serialize usage chunk"
        );
        serde_json::json!({
            "error": {
                "message": "Internal error: failed to serialize usage chunk",
                "type": "server_error",
                "param": null,
                "code": null
            }
        })
        .to_string()
    })
}

/// 创建 OpenAI 格式的 SSE 流
fn create_openai_stream(
    mut rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    model: String,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
    stream_options: Option<StreamOptions>,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    async_stream::stream! {
        let mut status = "success".to_string();
        let mut completed = false; // 跟踪流是否正常完成
        let mut first_chunk = true;
        let completion_id = generate_completion_id();
        let created = chrono::Utc::now().timestamp();

        while let Some(event) = rx.recv().await {
            match event {
                llm_protocol_provider::StreamEvent::Delta { content, finish_reason } => {
                    let data = make_delta_chunk_data(
                        content, &finish_reason, &mut first_chunk,
                        &completion_id, created, &model, &provider_name,
                    );
                    yield Ok(Event::default().data(data));

                    // 如果有 finish_reason，这是最后一块，发送 [DONE] 并结束
                    if finish_reason.is_some() {
                        completed = true;
                        // 执行 billing
                        let _ = billing.finalize_and_trigger_distribution(
                            &ctx, &provider_name, account_id, &status, ctx.user_id
                        ).await;

                        // 如果需要包含用量信息
                        if stream_options.as_ref().map(|o| o.include_usage).unwrap_or(false) {
                            let (input_tokens, output_tokens) = ctx.usage_snapshot();
                            let data = make_usage_chunk_data(
                                input_tokens, output_tokens,
                                &completion_id, created, &model, &provider_name,
                            );
                            yield Ok(Event::default().data(data));
                        }

                        // 发送 [DONE] 标记
                        yield Ok(Event::default().data("[DONE]"));
                        break;
                    }
                }
                llm_protocol_provider::StreamEvent::Done => {
                    // 流正常结束
                    completed = true;
                    let _ = billing.finalize_and_trigger_distribution(
                        &ctx, &provider_name, account_id, &status, ctx.user_id
                    ).await;

                    // 如果需要包含用量信息
                    if stream_options.as_ref().map(|o| o.include_usage).unwrap_or(false) {
                        let (input_tokens, output_tokens) = ctx.usage_snapshot();
                        let data = make_usage_chunk_data(
                            input_tokens, output_tokens,
                            &completion_id, created, &model, &provider_name,
                        );
                        yield Ok(Event::default().data(data));
                    }

                    yield Ok(Event::default().data("[DONE]"));
                    break;
                }
                llm_protocol_provider::StreamEvent::Error { message } => {
                    completed = true;
                    status = "error".to_string();
                    let _ = billing.finalize_and_trigger_distribution(
                        &ctx, &provider_name, account_id, &status, ctx.user_id
                    ).await;

                    let error_chunk = serde_json::json!({
                        "error": {
                            "message": message,
                            "type": "api_error",
                            "param": null,
                            "code": "internal_error"
                        }
                    });
                    yield Ok(Event::default().data(error_chunk.to_string()));
                    break;
                }
                llm_protocol_provider::StreamEvent::Usage { .. }
                | llm_protocol_provider::StreamEvent::Raw { .. } => {
                    // Usage 由 executor 层通过 ctx.set_*_tokens() 消费，
                    // Raw 为 provider 原始事件不需要透传
                }
            }
        }

        // 流意外结束（channel 关闭但没有收到完成事件）
        if !completed {
            tracing::warn!(
                request_id = %ctx.request_id,
                "Stream ended without Done/Error/finish_reason event"
            );
            status = "incomplete".to_string();
            let _ = billing.finalize_and_trigger_distribution(
                &ctx, &provider_name, account_id, &status, ctx.user_id
            ).await;
        }
    }
}

/// 创建带 keepalive 的 SSE 流式响应（多模态专用）
///
/// 与非流式 `create_non_streaming_json_with_keepalive` 用途一致：
/// 图片下载期间每 10s 发送 SSE 空事件，防止 Nginx / 云平台
/// `proxy_read_timeout` 超时触发 504。
///
/// SSE 空事件（`data:\\n\\n`）对 OpenAI 兼容客户端透明，
/// 客户端 parser 会忽略空 data 字段。
fn create_openai_stream_with_keepalive(
    mut rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    model: String,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
    stream_options: Option<StreamOptions>,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    async_stream::stream! {
        let mut status = "success".to_string();
        let mut first_chunk = true;
        let completion_id = generate_completion_id();
        let created = chrono::Utc::now().timestamp();

        // 最大 keepalive 持续时间 120s，防止上游 Provider 后台任务停滞
        // 导致循环无限期发送空事件。120s 足够覆盖图片下载（~30-40s）+
        // LLM 推理（~20-40s），同时避免服务端资源长期占用。
        //
        // 注：此值与 GatewayConfig.timeout_secs 解耦，原因如下：
        // - executor 超时由 tokio::time::timeout 在后台任务中实现
        // - keepalive 是在 handler 层维护 TCP 连接存活，职责不同
        // - 两者设为相同默认值（120s）是巧合；运维调大 timeout_secs 时
        //   也应同步调整此值，反之亦然
        let max_keepalive = std::time::Duration::from_secs(120);
        let deadline = tokio::time::sleep(max_keepalive);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                _ = &mut deadline => {
                    status = "timeout".to_string();
                    tracing::error!(
                        request_id = %ctx.request_id,
                        "SSE stream keepalive: max duration (120s) exceeded, terminating"
                    );
                    let _ = billing.finalize_and_trigger_distribution(
                        &ctx, &provider_name, account_id, &status, ctx.user_id
                    ).await;
                    let error_json = serde_json::json!({
                        "error": {
                            "message": "Request timed out",
                            "type": "server_error",
                            "param": null,
                            "code": "timeout"
                        }
                    });
                    yield Ok(Event::default().data(error_json.to_string()));
                    yield Ok(Event::default().data("[DONE]"));
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                    // SSE 空事件作 keepalive：客户端 parser 忽略空 data，
                    // 但 Nginx / 云平台网关将其视为有效数据流，重置 proxy_read_timeout
                    yield Ok(Event::default().data(""));
                }
                event = rx.recv() => {
                    match event {
                        Some(event) => match event {
                            llm_protocol_provider::StreamEvent::Delta { content, finish_reason } => {
                                let data = make_delta_chunk_data(
                                    content, &finish_reason, &mut first_chunk,
                                    &completion_id, created, &model, &provider_name,
                                );
                                yield Ok(Event::default().data(data));

                                if finish_reason.is_some() {
                                    let _ = billing.finalize_and_trigger_distribution(
                                        &ctx, &provider_name, account_id, &status, ctx.user_id
                                    ).await;

                                    if stream_options.as_ref().map(|o| o.include_usage).unwrap_or(false) {
                                        let (input_tokens, output_tokens) = ctx.usage_snapshot();
                                        let data = make_usage_chunk_data(
                                            input_tokens, output_tokens,
                                            &completion_id, created, &model, &provider_name,
                                        );
                                        yield Ok(Event::default().data(data));
                                    }

                                    yield Ok(Event::default().data("[DONE]"));
                                    return;
                                }
                            }
                            llm_protocol_provider::StreamEvent::Done => {
                                let _ = billing.finalize_and_trigger_distribution(
                                    &ctx, &provider_name, account_id, &status, ctx.user_id
                                ).await;

                                if stream_options.as_ref().map(|o| o.include_usage).unwrap_or(false) {
                                    let (input_tokens, output_tokens) = ctx.usage_snapshot();
                                    let data = make_usage_chunk_data(
                                        input_tokens, output_tokens,
                                        &completion_id, created, &model, &provider_name,
                                    );
                                    yield Ok(Event::default().data(data));
                                }

                                yield Ok(Event::default().data("[DONE]"));
                                return;
                            }
                            llm_protocol_provider::StreamEvent::Error { message } => {
                                status = "error".to_string();
                                let _ = billing.finalize_and_trigger_distribution(
                                    &ctx, &provider_name, account_id, &status, ctx.user_id
                                ).await;

                                let error_chunk = serde_json::json!({
                                    "error": {
                                        "message": message,
                                        "type": "api_error",
                                        "param": null,
                                        "code": "internal_error"
                                    }
                                });
                                yield Ok(Event::default().data(error_chunk.to_string()));
                                yield Ok(Event::default().data("[DONE]"));
                                return;
                            }
                            llm_protocol_provider::StreamEvent::Usage { .. }
                            | llm_protocol_provider::StreamEvent::Raw { .. } => {
                                // Usage 由 executor 层通过 ctx.set_*_tokens() 消费，
                                // Raw 为 provider 原始事件不需要透传
                            }
                        },
                        None => break,
                    }
                }
            }
        }

        // 流意外结束（channel 关闭但没有收到完成事件）
        // 所有正常完成路径（finish_reason / Done / Error / deadline）均使用 return 退出，
        // 只有 channel 关闭（None）通过 break 到达此处
        tracing::warn!(
            request_id = %ctx.request_id,
            "SSE stream keepalive: ended without Done/Error/finish_reason event"
        );
        status = "incomplete".to_string();
        let _ = billing.finalize_and_trigger_distribution(
            &ctx, &provider_name, account_id, &status, ctx.user_id
        ).await;
    }
}

// ==================== Models ====================

/// 模型信息
#[derive(Debug, Serialize, Deserialize)]
pub struct Model {
    /// 模型 ID
    pub id: String,
    /// 对象类型: model
    pub object: String,
    /// 创建时间戳
    pub created: i64,
    /// 拥有者
    pub owned_by: String,
}

/// 模型列表响应
#[derive(Debug, Serialize, Deserialize)]
pub struct ListModelsResponse {
    /// 对象类型: list
    pub object: String,
    /// 模型列表
    pub data: Vec<Model>,
}

/// 列出所有模型
/// GET /v1/models
/// 从数据库聚合所有启用的 Provider 账号支持的模型列表
pub async fn list_models(State(state): State<AppState>) -> Result<Json<ListModelsResponse>> {
    let mut model_set = std::collections::HashSet::new();
    let mut provider_map = std::collections::HashMap::new();

    // 尝试从数据库获取模型列表
    if let Some(pool) = state.pool.as_deref() {
        // 查询所有启用的账号（不限制 tenant_id，使用系统级查询）
        if let Ok(accounts) = Account::find_enabled_all(pool).await {
            for account in accounts {
                for model in account.models_supported {
                    model_set.insert(model.clone());
                    provider_map.insert(model, account.provider.clone());
                }
            }
        }
    }

    // 如果数据库中没有模型，使用默认模型列表（仅保留一个示例模型）
    if model_set.is_empty() {
        model_set.insert("model-empty".to_string());

        // 使用 provideraccount 计费维度
        let provider = keycompute_pricing::DEFAULT_PRICING_PROVIDER;
        provider_map.insert("model-empty".to_string(), provider.to_string());
    }

    let models: Vec<Model> = model_set
        .into_iter()
        .map(|id| Model {
            id: id.clone(),
            object: "model".to_string(),
            created: chrono::Utc::now().timestamp(),
            owned_by: provider_map
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
        })
        .collect();

    Ok(Json(ListModelsResponse {
        object: "list".to_string(),
        data: models,
    }))
}

/// 获取模型信息
/// GET /v1/models/{model}
///
/// 从数据库查询指定模型，返回其所属 Provider 信息
pub async fn retrieve_model(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Result<Json<Model>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    // 查询所有启用的账号，找到支持该模型的 Provider
    let accounts = Account::find_enabled_all(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query accounts: {}", e)))?;

    for account in accounts {
        if account.models_supported.contains(&model_id) {
            return Ok(Json(Model {
                id: model_id,
                object: "model".to_string(),
                created: chrono::Utc::now().timestamp(),
                owned_by: account.provider,
            }));
        }
    }

    // 模型不存在
    Err(ApiError::NotFound(format!("Model not found: {}", model_id)))
}

/// 将节点的完整响应转换为模拟流式输出
///
/// 该函数接收节点返回的完整 ChatCompletionResponse，
/// 将其内容拆分为多个 SSE chunk，模拟 token 级流式输出。
fn simulate_node_stream(
    response: keycompute_types::ChatCompletionResponse,
    ctx: Arc<RequestContext>,
    model: String,
    billing: Arc<keycompute_billing::BillingService>,
    stream_options: Option<StreamOptions>,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    // 伪流式（simulated streaming）：
    // - Node 路径先通过 enqueue_and_wait() 获取完整响应
    // - 再将完整文本按字符拆分为 ~20 个块，每块间隔 10ms 发送
    // - 模拟真实 SSE 流式输出的用户体验
    //
    // 注：Node 响应目前仅包含单个 choice（n=1），多 choice 场景暂不支持。
    async_stream::stream! {
        let completion_id = generate_completion_id();
        let created = chrono::Utc::now().timestamp();

        // 获取第一个 choice 的文本内容
        let content = response.choices.first().map(|c| c.message.content.clone()).unwrap_or_default();

        // 将内容拆分为字符级别的 chunk（模拟 token 级输出）
        // 注：这里是简单实现，按字符拆分，实际可以按 token 拆分
        let chars: Vec<char> = content.chars().collect();
        let chunk_size = std::cmp::max(1, chars.len() / 20); // 至少 1 个字符，最多 20 个 chunk

        // 发送 content chunks，仅首个 chunk 携带 role（遵循 OpenAI SSE 协议）
        let mut first_chunk = true;
        for chunk in chars.chunks(chunk_size) {
            let chunk_content: String = chunk.iter().collect();
            let delta = if first_chunk {
                first_chunk = false;
                serde_json::json!({
                    "role": "assistant",
                    "content": chunk_content
                })
            } else {
                serde_json::json!({
                    "content": chunk_content
                })
            };
            let data = serde_json::json!({
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": delta,
                    "finish_reason": null
                }]
            });
            yield Ok(Event::default().data(data.to_string()));

            // 小延迟，模拟真实流式输出
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // 发送最后一个带有 finish_reason 的 chunk
        let finish_reason = response.choices.first().and_then(|c| c.finish_reason.clone()).unwrap_or("stop".to_string());
        let data = serde_json::json!({
            "id": completion_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": finish_reason
            }]
        });
        yield Ok(Event::default().data(data.to_string()));

        // 如果请求了 usage，发送 usage chunk
        if stream_options.as_ref().map(|o| o.include_usage).unwrap_or(false) {
            let data = serde_json::json!({
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [],
                "usage": {
                    "prompt_tokens": response.usage.prompt_tokens,
                    "completion_tokens": response.usage.completion_tokens,
                    "total_tokens": response.usage.total_tokens
                }
            });
            yield Ok(Event::default().data(data.to_string()));
        }

        // 发送 [DONE] 标记，声明流式传输结束（OpenAI SSE 协议要求）
        yield Ok(Event::default().data("[DONE]"));

        // 计费（使用 NODE_PRICING_PROVIDER 常量，与路由层定价维度一致）
        let _ = billing
            .finalize_and_trigger_distribution(
                &ctx,
                keycompute_pricing::NODE_PRICING_PROVIDER,
                uuid::Uuid::nil(),
                "success",
                ctx.user_id,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_completion_request_deserialize() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello"}],
            "temperature": 0.7,
            "max_tokens": 100
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert!(!req.stream);
        assert_eq!(req.temperature, Some(0.7));
    }

    #[test]
    fn test_chat_completion_stream_request() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": true,
            "stream_options": {"include_usage": true}
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert!(req.stream);
        assert!(req.stream_options.unwrap().include_usage);
    }

    fn minimal_request(extra: &str) -> ChatCompletionRequest {
        let json = format!(
            r#"{{
                "model": "gpt-4o",
                "messages": [{{"role": "user", "content": "Hello"}}]{}{}
            }}"#,
            if extra.is_empty() { "" } else { "," },
            extra
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn test_validate_sampling_params_in_range() {
        assert!(minimal_request("").validate_sampling_params().is_ok());
        assert!(
            minimal_request(r#""max_tokens": 100, "temperature": 2.0, "top_p": 1.0"#)
                .validate_sampling_params()
                .is_ok()
        );
        assert!(
            minimal_request(r#""temperature": 0.0, "top_p": 0.0"#)
                .validate_sampling_params()
                .is_ok()
        );
    }

    #[test]
    fn test_validate_sampling_params_out_of_range() {
        // 越界参数应在 handler 层拒绝，不进入路由/上游调用
        for extra in [
            r#""max_tokens": 0"#,
            r#""max_completion_tokens": 0"#,
            r#""temperature": -0.1"#,
            r#""temperature": 2.1"#,
            r#""top_p": -0.1"#,
            r#""top_p": 1.5"#,
        ] {
            assert!(
                minimal_request(extra).validate_sampling_params().is_err(),
                "{extra} should be rejected"
            );
        }
    }

    #[test]
    fn test_max_completion_tokens_alias() {
        // 新版字段 max_completion_tokens 作为 max_tokens 的回退别名
        let req = minimal_request(r#""max_completion_tokens": 256"#);
        assert_eq!(req.effective_max_tokens(), Some(256));

        // 两者同时提供时 max_tokens 优先
        let req = minimal_request(r#""max_tokens": 100, "max_completion_tokens": 256"#);
        assert_eq!(req.effective_max_tokens(), Some(100));
    }

    #[test]
    fn test_tool_call_serialization() {
        let tool_call = ToolCall {
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_weather".to_string(),
                arguments: r#"{"location": "Beijing"}"#.to_string(),
            },
        };
        let json = serde_json::to_string(&tool_call).unwrap();
        assert!(json.contains("call_123"));
        assert!(json.contains("get_weather"));
    }

    #[tokio::test]
    async fn test_list_models() {
        // 测试模型结构序列化
        let model = Model {
            id: "gpt-4o".to_string(),
            object: "model".to_string(),
            created: chrono::Utc::now().timestamp(),
            owned_by: "openai".to_string(),
        };
        let json = serde_json::to_string(&model).unwrap();
        assert!(json.contains("gpt-4o"));
        assert!(json.contains("model"));
    }

    // 注意：retrieve_model 需要 AppState 和数据库连接，
    // 适合在集成测试中测试，这里不再单独测试

    #[test]
    fn test_has_image_content_empty() {
        assert!(!has_image_content(&[]));
    }

    #[test]
    fn test_has_image_content_text_only() {
        let msg = Message::new(MessageRole::User, MessageContent::text("Hello"));
        assert!(!has_image_content(&[msg]));
    }

    #[test]
    fn test_has_image_content_text_parts() {
        let msg = Message {
            role: MessageRole::User,
            content: MessageContent::Parts(vec![ContentPart::Text {
                text: "Hello".to_string(),
            }]),
        };
        assert!(!has_image_content(&[msg]));
    }

    #[test]
    fn test_has_image_content_with_image_url() {
        use keycompute_types::ImageUrl;
        let msg = Message {
            role: MessageRole::User,
            content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/image.png".to_string(),
                    detail: None,
                },
            }]),
        };
        assert!(has_image_content(&[msg]));
    }

    #[test]
    fn test_has_image_content_mixed_parts() {
        use keycompute_types::ImageUrl;
        let msg = Message {
            role: MessageRole::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "Describe this".to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "https://example.com/photo.jpg".to_string(),
                        detail: None,
                    },
                },
            ]),
        };
        assert!(has_image_content(&[msg]));
    }

    #[test]
    fn test_has_image_content_data_uri() {
        use keycompute_types::ImageUrl;
        let msg = Message {
            role: MessageRole::User,
            content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,iVBORw0KGgo...".to_string(),
                    detail: None,
                },
            }]),
        };
        assert!(!has_image_content(&[msg]));
    }
}
