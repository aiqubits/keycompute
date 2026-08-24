//! Anthropic Messages API 入站处理器。
//!
//! 此模块保留已验证的原始请求体，并仅将其发送给 Anthropic 协议上游。通用
//! `Message` 副本只用于路由、观测和用量估算，绝不作为原生请求的重建来源。

use crate::{
    error::{ApiError, Result},
    extractors::{AuthExtractor, ClientRequestId, RequestId, RequestReceivedAt},
    state::AppState,
};
use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
};
use futures::{Stream, StreamExt};
use keycompute_auth::Permission;
use keycompute_types::{
    ClientResponseOutcome, ErrorOrigin, ExecutionTarget, Message, MessageContent, MessageRole,
    NoopRequestLifecycleRecorder, RequestContext, RequestLifecycleRecorder, RequestStatus,
    RequestTraceStart, RouteType, TraceErrorCategory,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

/// Anthropic Messages 请求。
///
/// 已知的路由和采样字段有强类型校验；其它官方字段及未来扩展通过 flatten
/// 原样保留，例如 tools、tool_choice、thinking、context_management 与 metadata。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<AnthropicInputMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicInputMessage {
    pub role: String,
    pub content: Value,
}

impl AnthropicMessagesRequest {
    fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            return Err(ApiError::BadRequest("model must not be empty".to_string()));
        }
        if self.max_tokens == 0 {
            return Err(ApiError::BadRequest(
                "max_tokens must be greater than 0".to_string(),
            ));
        }
        if self.messages.is_empty() {
            return Err(ApiError::BadRequest(
                "messages must contain at least one message".to_string(),
            ));
        }
        if let Some(temperature) = self.temperature
            && !(0.0..=1.0).contains(&temperature)
        {
            return Err(ApiError::BadRequest(
                "temperature must be between 0.0 and 1.0".to_string(),
            ));
        }
        if let Some(top_p) = self.top_p
            && !(0.0..=1.0).contains(&top_p)
        {
            return Err(ApiError::BadRequest(
                "top_p must be between 0.0 and 1.0".to_string(),
            ));
        }

        for (index, message) in self.messages.iter().enumerate() {
            if !matches!(message.role.as_str(), "user" | "assistant") {
                return Err(ApiError::BadRequest(format!(
                    "messages[{index}].role must be user or assistant"
                )));
            }
            match &message.content {
                Value::String(_) => {}
                Value::Array(blocks) if !blocks.is_empty() => {}
                _ => {
                    return Err(ApiError::BadRequest(format!(
                        "messages[{index}].content must be a string or non-empty array"
                    )));
                }
            }
        }
        Ok(())
    }

    /// 产生仅供通用运行时使用的上下文消息。原始 body 另行保存且不会由这里
    /// 的结果重建，因此工具、图片、thinking 等块不会在上游请求中丢失。
    ///
    /// 注意：投影用占位符替换敏感块，因此基于它的 tiktoken 输入估算会低于
    /// 实际请求大小（工具定义、缓存内容未计入）；这是刻意的泄露防护权衡。
    /// 正常上游以 message_start 的精确 usage 覆盖估算，异常上游（无 usage）
    /// 时计费会偏低，属于可接受的已知局限。
    fn context_messages(&self) -> Vec<Message> {
        let mut messages = Vec::new();
        if let Some(system) = &self.system {
            messages.push(Message {
                role: MessageRole::System,
                content: MessageContent::text(value_text(system)),
            });
        }
        messages.extend(self.messages.iter().map(|message| Message {
            role: match message.role.as_str() {
                "assistant" => MessageRole::Assistant,
                _ => MessageRole::User,
            },
            content: MessageContent::text(value_text(&message.content)),
        }));
        messages
    }
}

async fn finish_anthropic_unexecuted_trace(
    guard: &mut super::PreExecutionTraceGuard,
    origin: ErrorOrigin,
    category: TraceErrorCategory,
    code: &str,
) {
    guard.finish_failed(origin, category, code).await;
}

/// POST /v1/messages
pub async fn messages(
    State(state): State<AppState>,
    auth: AuthExtractor,
    request_id: RequestId,
    client_request_id: ClientRequestId,
    received_at: RequestReceivedAt,
    headers: HeaderMap,
    Json(request): Json<AnthropicMessagesRequest>,
) -> Result<axum::response::Response> {
    let mut lifecycle: Arc<dyn RequestLifecycleRecorder> = Arc::clone(&state.lifecycle);
    let mut pre_execution_guard =
        super::PreExecutionTraceGuard::new(Arc::clone(&lifecycle), request_id.0);
    if let Err(error) = lifecycle
        .start_request(RequestTraceStart {
            request_id: request_id.0,
            client_request_id: client_request_id.0,
            tenant_id: auth.tenant_id,
            user_id: auth.user_id,
            produce_ai_key_id: auth.produce_ai_key_id,
            protocol: "anthropic".to_string(),
            request_path: "/v1/messages".to_string(),
            requested_model: request.model.clone(),
            is_stream: request.stream,
            received_at: received_at.0,
        })
        .await
    {
        tracing::warn!(request_id=%request_id.0, %error, "request tracing disabled for this request");
        pre_execution_guard.disarm();
        lifecycle = Arc::new(NoopRequestLifecycleRecorder);
        pre_execution_guard =
            super::PreExecutionTraceGuard::new(Arc::clone(&lifecycle), request_id.0);
    }
    if let Err(error) = require_messages_api_permission(&auth) {
        finish_anthropic_unexecuted_trace(
            &mut pre_execution_guard,
            ErrorOrigin::Client,
            TraceErrorCategory::Authorization,
            "permission_denied",
        )
        .await;
        return Err(error);
    }
    if let Err(error) = request.validate() {
        finish_anthropic_unexecuted_trace(
            &mut pre_execution_guard,
            ErrorOrigin::Client,
            TraceErrorCategory::InvalidRequest,
            "invalid_request",
        )
        .await;
        return Err(error);
    }
    if let Err(error) = validate_anthropic_headers(&headers) {
        finish_anthropic_unexecuted_trace(
            &mut pre_execution_guard,
            ErrorOrigin::Client,
            TraceErrorCategory::InvalidRequest,
            "invalid_anthropic_headers",
        )
        .await;
        return Err(error);
    }

    if let Some(balance_service) = state.billing.balance_service()
        && let Err(error) = balance_service
            .check_balance_for_tenant(auth.user_id, auth.tenant_id)
            .await
    {
        finish_anthropic_unexecuted_trace(
            &mut pre_execution_guard,
            ErrorOrigin::Client,
            TraceErrorCategory::Balance,
            "insufficient_balance",
        )
        .await;
        return Err(ApiError::from(error));
    }

    // 在将反序列化请求转为原生 JSON 前提取轻量路由字段。若同时保留两种
    // 形式，可能会常驻两份 32 MiB 的多模态 payload。
    let model = request.model.clone();
    let max_tokens = request.max_tokens;
    let stream = request.stream;
    let temperature = request.temperature;
    let top_p = request.top_p;
    let context_messages = request.context_messages();

    let provider = keycompute_pricing::resolve_pricing_provider(&model);
    let pricing = match state
        .pricing
        .create_snapshot(&model, &auth.tenant_id, Some(provider))
        .await
    {
        Ok(pricing) => pricing,
        Err(error) => {
            finish_anthropic_unexecuted_trace(
                &mut pre_execution_guard,
                ErrorOrigin::Gateway,
                TraceErrorCategory::Internal,
                "pricing_failed",
            )
            .await;
            return Err(ApiError::Internal(format!(
                "Failed to create pricing snapshot: {error}"
            )));
        }
    };

    let native_anthropic_request = match serde_json::to_value(request) {
        Ok(request) => Arc::new(request),
        Err(error) => {
            finish_anthropic_unexecuted_trace(
                &mut pre_execution_guard,
                ErrorOrigin::Gateway,
                TraceErrorCategory::Internal,
                "request_serialization_failed",
            )
            .await;
            return Err(ApiError::Internal(format!(
                "Failed to serialize Messages request: {error}"
            )));
        }
    };
    // 注意内存特征：32 MiB 上限的 body 在反序列化（Json 提取器）与
    // to_value 之间各持有一份 Value，峰值约为 body 的 2~3 倍。上限是有意
    // 设定的（多模态内联块），但不要在下游再复制整个请求体；仅 Arc 共享。
    let mut request_ctx = RequestContext::new(
        request_id.0,
        auth.user_id,
        auth.tenant_id,
        auth.produce_ai_key_id,
        model.clone(),
        context_messages,
        stream,
        pricing,
    );
    request_ctx.max_tokens = Some(max_tokens);
    request_ctx.temperature = temperature;
    request_ctx.top_p = top_p;
    request_ctx.native_anthropic_request = Some(native_anthropic_request);
    request_ctx.native_anthropic_headers = forwarded_anthropic_headers(&headers);
    let mut ctx = Arc::new(request_ctx);

    let mut plan = match state.routing.route(&ctx).await {
        Ok(plan) => plan,
        Err(error) => {
            finish_anthropic_unexecuted_trace(
                &mut pre_execution_guard,
                ErrorOrigin::Gateway,
                TraceErrorCategory::Internal,
                "routing_failed",
            )
            .await;
            return Err(crate::error::map_routing_error(error, "anthropic"));
        }
    };

    let (primary_provider, primary_account_id) = match &plan.primary {
        ExecutionTarget::ProviderAccount {
            provider,
            account_id,
            ..
        } if provider.eq_ignore_ascii_case("anthropic") => (provider.clone(), *account_id),
        ExecutionTarget::ProviderAccount { .. } => {
            finish_anthropic_unexecuted_trace(
                &mut pre_execution_guard,
                ErrorOrigin::Gateway,
                TraceErrorCategory::InvalidRequest,
                "incompatible_provider_route",
            )
            .await;
            return Err(ApiError::BadRequest(format!(
                "Model {} is not available through an Anthropic-compatible provider",
                model
            )));
        }
        ExecutionTarget::Node { .. } => {
            finish_anthropic_unexecuted_trace(
                &mut pre_execution_guard,
                ErrorOrigin::Gateway,
                TraceErrorCategory::InvalidRequest,
                "unsupported_node_route",
            )
            .await;
            return Err(ApiError::BadRequest(
                "Anthropic Messages ingress cannot be routed to a node".to_string(),
            ));
        }
    };

    // 原生请求不得在失败后被降级到另一种协议，否则会悄悄遗失工具或 thinking。
    plan.fallback_chain.retain(|target| matches!(
        target,
        ExecutionTarget::ProviderAccount { provider, .. } if provider.eq_ignore_ascii_case("anthropic")
    ));
    if let Err(error) = lifecycle
        .set_route(
            request_id.0,
            RouteType::ProviderAccount,
            RequestStatus::Routing,
        )
        .await
    {
        tracing::warn!(request_id=%request_id.0, %error, "failed to record request route");
    }
    state
        .pricing
        .update_context_pricing(Arc::make_mut(&mut ctx), &primary_provider)
        .await;

    let timeout_duration = std::time::Duration::from_secs(state.gateway_config.timeout_secs);
    let mut client_response_guard =
        super::ClientResponseGuard::new(Arc::clone(&lifecycle), Arc::clone(&ctx));
    pre_execution_guard.disarm();
    // execute 会立即返回 receiver（后台任务持有上游连接），因此该 timeout 只
    // 防护“建立执行链”阶段的异常阻塞，不覆盖流式消费生命周期；流式超时由
    // executor 内部的 exec_timeout（同源 timeout_secs）负责。
    let rx = match tokio::time::timeout(
        timeout_duration,
        state.gateway.execute_with_recorder(
            Arc::clone(&ctx),
            plan,
            Arc::clone(&state.account_states),
            Some(Arc::clone(&state.provider_health)),
            Arc::clone(&lifecycle),
        ),
    )
    .await
    {
        Ok(Ok(rx)) => rx,
        Ok(Err(error)) => {
            client_response_guard
                .finish_with_outcome(ClientResponseOutcome::ResponseFailed)
                .await;
            return Err(crate::error::map_execution_error(error));
        }
        Err(_) => {
            client_response_guard
                .finish_with_outcome(ClientResponseOutcome::TimedOut)
                .await;
            return Err(ApiError::Internal(format!(
                "Gateway execute timeout after {}s",
                state.gateway_config.timeout_secs
            )));
        }
    };

    tracing::info!(
        request_id = %request_id.0,
        model = %model,
        stream,
        primary_provider = %primary_provider,
        "Anthropic Messages request"
    );

    let billing = Arc::clone(&state.billing);
    if stream {
        let stream = create_anthropic_stream_with_lifecycle(
            rx,
            ctx,
            primary_provider,
            primary_account_id,
            billing,
            Arc::clone(&lifecycle),
        );
        client_response_guard.disarm();
        Ok(Sse::new(stream).into_response())
    } else {
        // The nested response helper installs its own guard before its first
        // await, so ownership transfers without a cancellation gap.
        client_response_guard.disarm();
        let response = create_anthropic_response_with_lifecycle(
            rx,
            ctx,
            primary_provider,
            primary_account_id,
            billing,
            Arc::clone(&lifecycle),
        )
        .await?;
        Ok(Json(response).into_response())
    }
}

/// The Messages endpoint is a billable LLM forwarding surface. Authentication
/// alone is insufficient: all access decisions must use the permission set
/// built by the authentication layer.
fn require_messages_api_permission(auth: &AuthExtractor) -> Result<()> {
    if auth.has_permission(&Permission::UseApi) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "API-use permission is required for /v1/messages".to_string(),
        ))
    }
}

#[cfg(test)]
async fn create_anthropic_response(
    rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
) -> Result<Value> {
    create_anthropic_response_with_lifecycle(
        rx,
        ctx,
        provider_name,
        account_id,
        billing,
        Arc::new(keycompute_types::NoopRequestLifecycleRecorder),
    )
    .await
}

async fn create_anthropic_response_with_lifecycle(
    mut rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
    lifecycle: Arc<dyn keycompute_types::RequestLifecycleRecorder>,
) -> Result<Value> {
    let mut client_response_guard =
        super::ClientResponseGuard::new(Arc::clone(&lifecycle), Arc::clone(&ctx));
    let (mut response_tx, response_rx) = tokio::sync::oneshot::channel();
    let worker_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut complete = false;
        let mut native_response = None;
        let mut status = "success";
        let mut handler_connected = true;
        let mut terminal_error = None;

        loop {
            tokio::select! {
                biased;
                _ = response_tx.closed(), if handler_connected => {
                    handler_connected = false;
                    worker_ctx.mark_client_disconnected();
                }
                event = rx.recv() => {
                    let Some(event) = event else { break };
                    match event {
                        llm_protocol_provider::StreamEvent::Raw { data } => {
                            if let Some(body) = raw_message_body(&data) {
                                native_response = Some(body);
                            }
                        }
                        llm_protocol_provider::StreamEvent::Done => {
                            complete = true;
                            break;
                        }
                        llm_protocol_provider::StreamEvent::Error { message } => {
                            status = "error";
                            tracing::warn!(request_id = %worker_ctx.request_id, error = %message, "Anthropic upstream request failed");
                            terminal_error = Some(ApiError::Provider(
                                "Upstream request failed".to_string(),
                            ));
                            break;
                        }
                        // 原生非流式路径由 Raw(anthropic_message) 承载完整响应体，不产生
                        // Delta；Usage/InputUsage 已由 executor 写入 ctx 用于计费。
                        llm_protocol_provider::StreamEvent::Delta { .. }
                        | llm_protocol_provider::StreamEvent::Usage { .. }
                        | llm_protocol_provider::StreamEvent::InputUsage { .. } => {}
                    }
                }
            }
        }

        if terminal_error.is_none() && !complete {
            status = "incomplete";
            terminal_error = Some(ApiError::Internal(
                "Stream ended unexpectedly: channel closed without Done event".to_string(),
            ));
        }
        if terminal_error.is_none() && native_response.is_none() {
            status = "incomplete";
            tracing::error!(
                request_id = %worker_ctx.request_id,
                "Anthropic stream completed without a native message body"
            );
            terminal_error = Some(ApiError::Internal(
                "Anthropic response body missing after stream completion".to_string(),
            ));
        }

        finalize_anthropic_billing_logged(
            &billing,
            &worker_ctx,
            &provider_name,
            account_id,
            status,
        )
        .await;
        let result = match (terminal_error, native_response) {
            (Some(error), _) => Err(error),
            (None, Some(response)) => Ok(response),
            (None, None) => Err(ApiError::Internal(
                "Anthropic response validation state was inconsistent".to_string(),
            )),
        };
        if handler_connected && response_tx.send(result).is_err() {
            worker_ctx.mark_client_disconnected();
        }
    });

    let response = match response_rx.await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            super::finish_client_response_trace(
                &lifecycle,
                &ctx,
                ClientResponseOutcome::ResponseFailed,
            )
            .await;
            client_response_guard.disarm();
            return Err(error);
        }
        Err(_) => {
            super::finish_client_response_trace(
                &lifecycle,
                &ctx,
                ClientResponseOutcome::ResponseFailed,
            )
            .await;
            client_response_guard.disarm();
            return Err(ApiError::Internal(
                "Non-streaming Anthropic response worker stopped unexpectedly".to_string(),
            ));
        }
    };
    if let Err(error) = super::record_final_client_first_content(&lifecycle, ctx.request_id).await {
        tracing::warn!(request_id = %ctx.request_id, %error, "failed to record client first content");
    }
    super::finish_client_response_trace(&lifecycle, &ctx, ClientResponseOutcome::Succeeded).await;
    client_response_guard.disarm();
    Ok(response)
}

/// SSE 转发超时：客户端保持连接但停止读取时（TCP backpressure 或代理不消费），
/// 无界的 await send 会让结算任务永久卡住。超时视为断开，保证 executor 能中止
/// fallback 且结算照常完成。
const SSE_SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// 向客户端转发一个 SSE 事件，失败或超时都按断开处理。
///
/// executor 的 receiver 由结算任务持有直到 Done/Error，`tx.is_closed()` 不会
/// 生效，因此必须显式标记断开，让 executor 在 primary 失败后中止 fallback。
async fn forward_sse_event(
    sse_tx: &mpsc::Sender<Event>,
    ctx: &RequestContext,
    client_connected: &mut bool,
    event: Event,
) -> bool {
    if !*client_connected {
        return false;
    }
    let sent = tokio::time::timeout(SSE_SEND_TIMEOUT, sse_tx.send(event))
        .await
        .map(|result| result.is_ok())
        .unwrap_or(false);
    if !sent {
        *client_connected = false;
        ctx.mark_client_disconnected();
    }
    sent
}

/// Anthropic Errors schema 的 SSE 错误帧：对客户端只暴露通用文本。
fn anthropic_error_event(message: &str) -> Event {
    Event::default().event("error").data(
        json!({
            "type": "error",
            "error": {"type": "api_error", "message": message}
        })
        .to_string(),
    )
}

#[cfg(test)]
fn create_anthropic_stream(
    rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    create_anthropic_stream_with_lifecycle(
        rx,
        ctx,
        provider_name,
        account_id,
        billing,
        Arc::new(keycompute_types::NoopRequestLifecycleRecorder),
    )
}

fn create_anthropic_stream_with_lifecycle(
    mut rx: tokio::sync::mpsc::Receiver<llm_protocol_provider::StreamEvent>,
    ctx: Arc<RequestContext>,
    provider_name: String,
    account_id: uuid::Uuid,
    billing: Arc<keycompute_billing::BillingService>,
    lifecycle: Arc<dyn keycompute_types::RequestLifecycleRecorder>,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    let (sse_tx, sse_rx) = mpsc::channel(100);

    // 结算不能依赖 HTTP response body 的生命周期：客户端断开时 Axum 会 drop
    // SSE Stream，但上游调用和已产生的用量仍必须被完整消费和结算。
    tokio::spawn(async move {
        let mut completed = false;
        let mut status = "success";
        let mut client_connected = true;
        let mut client_first_content_recorded = false;

        loop {
            // 客户端断开时 SSE 的 receiver 被 drop，sse_tx 的 channel 随即关闭。
            // 后台任务在等待上游事件的空窗期（primary 连接中、首 token 延迟）不会
            // 产生失败的 send，必须主动监听 channel 关闭；否则 executor 的断开检查
            // （tx.is_closed 与 ctx 标志）都不会触发，fallback 会对一个已经离开的
            // 客户端继续调用新的上游。guard 防止 channel 关闭后 closed() 永久 ready
            // 导致 rx.recv() 饿死；标记断开后继续 drain，保证结算照常完成。
            tokio::select! {
                _ = sse_tx.closed(), if client_connected => {
                    client_connected = false;
                    ctx.mark_client_disconnected();
                }
                event = rx.recv() => {
                    let Some(event) = event else { break };
                    match event {
                        llm_protocol_provider::StreamEvent::Raw { data } => {
                            if let Some((event_name, body)) = raw_sse_event(&data) {
                                // 错误由随后的标准化 StreamEvent::Error 统一输出，避免
                                // 将上游响应体或传输层细节直接暴露给客户端。
                                if event_name == "error"
                                    || body.get("type").and_then(Value::as_str) == Some("error")
                                {
                                    continue;
                                }
                                let commits_response =
                                    raw_sse_event_commits_response(&event_name, &body);
                                let completes_response =
                                    raw_sse_event_completes_response(&event_name, &body);
                                let sent = forward_sse_event(
                                    &sse_tx,
                                    &ctx,
                                    &mut client_connected,
                                    Event::default().event(event_name).data(body.to_string()),
                                )
                                .await;
                                if sent && commits_response && !client_first_content_recorded
                                {
                                    if let Err(error) = lifecycle
                                        .record_client_first_content(ctx.request_id, chrono::Utc::now())
                                        .await
                                    {
                                        tracing::warn!(request_id = %ctx.request_id, %error, "failed to record client first content");
                                    }
                                    client_first_content_recorded = true;
                                }
                                // `message_stop` is the client-visible terminal
                                // frame. Some SDKs close the SSE body as soon as
                                // they consume it, before the executor's
                                // normalized Done reaches this worker. Record
                                // success in the shared context now so that an
                                // expected close is not misclassified as a
                                // disconnect. The request trace remains open
                                // until normalized Done completes billing.
                                if sent && completes_response {
                                    ctx.mark_client_response_succeeded();
                                }
                            }
                        }
                        llm_protocol_provider::StreamEvent::Done => {
                            completed = true;
                            finalize_anthropic_billing_logged(
                                &billing,
                                &ctx,
                                &provider_name,
                                account_id,
                                status,
                            )
                            .await;
                            super::finish_client_response_trace(
                                &lifecycle,
                                &ctx,
                                ClientResponseOutcome::Succeeded,
                            )
                            .await;
                            break;
                        }
                        llm_protocol_provider::StreamEvent::Error { message } => {
                            completed = true;
                            status = "error";
                            finalize_anthropic_billing_logged(
                                &billing,
                                &ctx,
                                &provider_name,
                                account_id,
                                status,
                            )
                            .await;
                            tracing::warn!(request_id = %ctx.request_id, error = %message, "Anthropic upstream stream failed");
                            let _ = forward_sse_event(
                                &sse_tx,
                                &ctx,
                                &mut client_connected,
                                anthropic_error_event("Upstream request failed"),
                            )
                            .await;
                            super::finish_client_response_trace(
                                &lifecycle,
                                &ctx,
                                ClientResponseOutcome::ResponseFailed,
                            )
                            .await;
                            break;
                        }
                        // 对原生 Anthropic 请求，适配器会同时产生 Raw 和标准化事件。
                        // 此处只输出 Raw，避免重复或把 tool/thinking 降级成 text。
                        llm_protocol_provider::StreamEvent::Delta { .. }
                        | llm_protocol_provider::StreamEvent::Usage { .. }
                        | llm_protocol_provider::StreamEvent::InputUsage { .. } => {}
                    }
                }
            }
        }

        if !completed {
            status = "incomplete";
            finalize_anthropic_billing_logged(&billing, &ctx, &provider_name, account_id, status)
                .await;
            let _ = forward_sse_event(
                &sse_tx,
                &ctx,
                &mut client_connected,
                anthropic_error_event("Upstream stream ended before message_stop"),
            )
            .await;
            super::finish_client_response_trace(
                &lifecycle,
                &ctx,
                ClientResponseOutcome::ResponseFailed,
            )
            .await;
        }
    });

    ReceiverStream::new(sse_rx).map(Ok)
}

async fn finalize_anthropic_billing(
    billing: &keycompute_billing::BillingService,
    ctx: &RequestContext,
    primary_provider: &str,
    primary_account_id: uuid::Uuid,
    status: &str,
) -> Result<keycompute_db::UsageLog> {
    let (provider, account_id) = ctx.billing_target(primary_provider, primary_account_id);
    billing
        .finalize_and_trigger_distribution(ctx, &provider, account_id, status, ctx.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Billing finalization failed: {e}")))
}

/// 结算并记录失败。billing 错误不影响请求结果（架构约束），仅记录日志。
async fn finalize_anthropic_billing_logged(
    billing: &keycompute_billing::BillingService,
    ctx: &RequestContext,
    primary_provider: &str,
    primary_account_id: uuid::Uuid,
    status: &str,
) {
    if let Err(error) =
        finalize_anthropic_billing(billing, ctx, primary_provider, primary_account_id, status).await
    {
        tracing::error!(
            request_id = %ctx.request_id,
            error = %error,
            "Failed to finalize Anthropic billing"
        );
    }
}

fn raw_message_body(data: &str) -> Option<Value> {
    let envelope: Value = serde_json::from_str(data).ok()?;
    (envelope.get("kind")?.as_str()? == "anthropic_message")
        .then(|| envelope.get("body").cloned())
        .flatten()
}

fn raw_sse_event(data: &str) -> Option<(String, Value)> {
    let envelope: Value = serde_json::from_str(data).ok()?;
    if envelope.get("kind")?.as_str()? != "anthropic_sse" {
        return None;
    }
    Some((
        envelope.get("event")?.as_str()?.to_string(),
        envelope.get("data")?.clone(),
    ))
}

/// Keep client TTFT semantics aligned with the executor: pings and error
/// envelopes do not begin a response, while all other raw Anthropic events do.
fn raw_sse_event_commits_response(event_name: &str, body: &Value) -> bool {
    event_name != "ping"
        && event_name != "error"
        && body.get("type").and_then(Value::as_str) != Some("error")
}

fn raw_sse_event_completes_response(event_name: &str, body: &Value) -> bool {
    event_name == "message_stop" || body.get("type").and_then(Value::as_str) == Some("message_stop")
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values
            .iter()
            .map(value_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
            match object.get("type").and_then(Value::as_str) {
                // 二进制媒体只参与原始 body 的上游转发，绝不能进入 tiktoken
                // 估算或 RequestContext 的 Debug/日志副本。
                Some("image") => "[image]".to_string(),
                Some("document") => "[document]".to_string(),
                // 工具调用参数与思考轨迹同样属于敏感内容，且与输入计数无关；
                // 只保留占位符，防止它们进入日志与 token 估算。
                Some("tool_use") => "[tool_use]".to_string(),
                Some("thinking") => "[thinking]".to_string(),
                // 工具结果文本仍是对话内容的一部分，必须保留以维持 token 估算；
                // 但缺 content（或 content 为空）时不得回退整个 JSON 对象（其中
                // 可能携带 tool_use_id 等上下文），保持占位符即可。
                Some("tool_result") => object
                    .get("content")
                    .map(value_text)
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "[tool_result]".to_string()),
                _ => object
                    .get("content")
                    .map(value_text)
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "[content]".to_string()),
            }
        }
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

/// 只透传影响 Anthropic 协议语义的头部。认证、客户端身份、宿主名及任意
/// 非白名单头都由网关自行管理，防止请求伪造或意外泄露。
fn forwarded_anthropic_headers(headers: &HeaderMap) -> std::collections::BTreeMap<String, String> {
    let mut forwarded = std::collections::BTreeMap::new();
    if let Some(version) = headers
        .get("anthropic-version")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        forwarded.insert("anthropic-version".to_string(), version.to_string());
    }
    let beta_values = headers
        .get_all("anthropic-beta")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if !beta_values.is_empty() {
        forwarded.insert("anthropic-beta".to_string(), beta_values.join(","));
    }
    forwarded
}

/// `anthropic-version` 是 Messages API 的必需协议头。网关不能在客户端遗漏
/// 该头时静默选用自己的默认版本，否则同一 API key 的不同客户端会得到不可
/// 预测的字段与行为差异。
fn validate_anthropic_headers(headers: &HeaderMap) -> Result<()> {
    match headers
        .get("anthropic-version")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        Some(_) => Ok(()),
        None => Err(ApiError::BadRequest(
            "anthropic-version header is required for /v1/messages".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn messages_trace_preserves_ingress_received_at() {
        let received_at = chrono::DateTime::parse_from_rfc3339("2025-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let mut state = AppState::with_config(crate::state::AppStateConfig::default());
        state.lifecycle = Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>;
        let auth = AuthExtractor::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "user",
        );
        let request = serde_json::from_value(json!({
            "model": "claude-test",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .unwrap();

        let result = messages(
            State(state),
            auth,
            RequestId::new(),
            ClientRequestId(None),
            RequestReceivedAt(received_at),
            HeaderMap::new(),
            Json(request),
        )
        .await;

        assert!(matches!(result, Err(ApiError::Forbidden(_))));
        let starts = recorder.request_starts();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].protocol, "anthropic");
        assert_eq!(starts[0].received_at, received_at);
    }

    #[test]
    fn preserves_tools_and_cache_control_in_serialized_body() {
        let request: AnthropicMessagesRequest = serde_json::from_value(json!({
            "model": "claude-test",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi", "cache_control": {"type": "ephemeral"}}]}],
            "tools": [{"name": "read_file", "input_schema": {"type": "object"}}],
            "thinking": {"type": "enabled", "budget_tokens": 32}
        })).unwrap();

        request.validate().unwrap();
        let serialized = serde_json::to_value(request).unwrap();
        assert!(serialized["tools"].is_array());
        assert_eq!(serialized["thinking"]["type"], "enabled");
        assert_eq!(
            serialized["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn rejects_invalid_roles_before_routing() {
        let request: AnthropicMessagesRequest = serde_json::from_value(json!({
            "model": "claude-test", "max_tokens": 1,
            "messages": [{"role": "system", "content": "not allowed"}]
        }))
        .unwrap();
        assert!(request.validate().is_err());
    }

    #[test]
    fn decodes_raw_sse_envelope() {
        let data = json!({
            "kind": "anthropic_sse", "event": "message_start",
            "data": {"type": "message_start"}
        })
        .to_string();
        assert_eq!(raw_sse_event(&data).unwrap().0, "message_start");
    }

    #[test]
    fn pings_do_not_commit_client_first_content() {
        assert!(!raw_sse_event_commits_response(
            "ping",
            &json!({"type": "ping"})
        ));
        assert!(!raw_sse_event_commits_response(
            "error",
            &json!({"type": "error"})
        ));
        assert!(raw_sse_event_commits_response(
            "message_start",
            &json!({"type": "message_start"})
        ));
        assert!(raw_sse_event_completes_response(
            "message_stop",
            &json!({"type": "message_stop"})
        ));
        assert!(!raw_sse_event_completes_response(
            "message_delta",
            &json!({"type": "message_delta"})
        ));
    }

    #[test]
    fn context_projection_never_copies_base64_image_data() {
        let image = json!({
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": "very-secret-base64"}
        });
        assert_eq!(value_text(&image), "[image]");

        let tool_result = json!({
            "type": "tool_result",
            "content": [image, {"type": "text", "text": "tool output"}]
        });
        assert_eq!(value_text(&tool_result), "[image]\ntool output");
    }

    #[test]
    fn context_projection_redacts_tool_use_and_thinking() {
        let tool_use = json!({
            "type": "tool_use",
            "id": "toolu_01",
            "name": "read_file",
            "input": {"path": "/etc/passwd", "contents": "secret-file-content"}
        });
        assert_eq!(value_text(&tool_use), "[tool_use]");

        let thinking = json!({
            "type": "thinking",
            "thinking": "secret reasoning chain",
            "signature": "sig"
        });
        assert_eq!(value_text(&thinking), "[thinking]");

        // 工具结果文本仍是对话内容的一部分，必须保留以维持 token 估算。
        let tool_result = json!({
            "type": "tool_result",
            "tool_use_id": "toolu_01",
            "content": [{"type": "text", "text": "file content"}]
        });
        assert_eq!(value_text(&tool_result), "file content");
    }

    #[test]
    fn context_projection_placeholders_contentless_objects() {
        // 工具结果缺 content（或 content 为空）时，不得回退整个 JSON 对象
        // （其中可能携带 tool_use_id 等上下文），保持占位符即可。
        let tool_result_without_content = json!({
            "type": "tool_result",
            "tool_use_id": "toolu_01",
            "is_error": true
        });
        assert_eq!(value_text(&tool_result_without_content), "[tool_result]");

        let tool_result_with_empty_content = json!({ "type": "tool_result", "content": [] });
        assert_eq!(value_text(&tool_result_with_empty_content), "[tool_result]");

        // 未知块类型（无 text/type/content）同样保守占位，不泄露原始 JSON。
        let unknown_block = json!({"id": "blk_01", "signature": "sig"});
        assert_eq!(value_text(&unknown_block), "[content]");
    }

    #[test]
    fn only_forwards_anthropic_protocol_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
        headers.append("anthropic-beta", "tools-2025-01-01".parse().unwrap());
        headers.append(
            "anthropic-beta",
            "prompt-caching-2024-07-31".parse().unwrap(),
        );
        headers.insert("user-agent", "do-not-forward".parse().unwrap());
        let forwarded = forwarded_anthropic_headers(&headers);
        assert_eq!(forwarded["anthropic-version"], "2023-06-01");
        assert_eq!(
            forwarded["anthropic-beta"],
            "tools-2025-01-01,prompt-caching-2024-07-31"
        );
        assert!(!forwarded.contains_key("user-agent"));
    }

    #[test]
    fn requires_anthropic_version_header() {
        assert!(validate_anthropic_headers(&HeaderMap::new()).is_err());

        let mut headers = HeaderMap::new();
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
        assert!(validate_anthropic_headers(&headers).is_ok());
    }

    #[test]
    fn messages_endpoint_requires_api_use_permission() {
        let auth = AuthExtractor::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "user",
        );
        assert!(matches!(
            require_messages_api_permission(&auth),
            Err(ApiError::Forbidden(_))
        ));

        let auth = auth.with_permissions(vec![Permission::UseApi]);
        assert!(require_messages_api_permission(&auth).is_ok());
    }

    #[tokio::test]
    async fn committed_stream_error_does_not_expose_upstream_error_body() {
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let stream = create_anthropic_stream(
            rx,
            ctx,
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        );

        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "message_start",
                "data": {"type": "message_start", "message": {"id": "msg_test"}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "nonstandard_error_event",
                "data": {"type": "error", "error": {"message": "upstream-secret"}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        tx.send(llm_protocol_provider::StreamEvent::error("upstream-secret"))
            .await
            .unwrap();
        drop(tx);

        let response = Sse::new(stream).into_response();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: message_start"));
        assert!(body.contains("Upstream request failed"));
        assert!(!body.contains("upstream-secret"));
    }

    #[tokio::test]
    async fn stream_worker_finishes_after_client_disconnects() {
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let stream = create_anthropic_stream(
            rx,
            ctx,
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        );
        drop(stream);

        tx.send(llm_protocol_provider::StreamEvent::Done)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), tx.closed())
            .await
            .expect("worker should keep draining upstream events after client disconnects");
    }

    #[tokio::test]
    async fn client_disconnect_marks_context_and_billing_stays_on_primary() {
        // Anthropic 流式路径的后台结算任务持有 receiver 直到 Done/Error，客户端
        // 断开不会触发 executor 的 `tx.is_closed()`。SSE 发送失败时 handler 必须
        // 显式标记断开，让 executor 在 primary 失败后中止 fallback；由于没有任何
        // 账号完成请求，结算必须回退 primary 账号（失败尝试的确定性归属）。
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let primary_account_id = uuid::Uuid::new_v4();
        let stream = create_anthropic_stream(
            rx,
            Arc::clone(&ctx),
            "anthropic".to_string(),
            primary_account_id,
            Arc::new(keycompute_billing::BillingService::new()),
        );
        // 客户端断开：drop SSE stream，后续 sse_tx 发送必然失败
        drop(stream);

        // 上游在客户端断开后仍产生事件：后台任务为结算必须持续 drain。
        // message_start 发送失败应触发断开标记。
        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "message_start",
                "data": {"type": "message_start"}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !ctx.is_client_disconnected() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("handler should mark the context after SSE send failure");

        // 随后 primary 失败：executor 会因断开标志中止 fallback 并上报 Error，
        // 后台任务收到 Error 后以 error 状态结算并结束。
        tx.send(llm_protocol_provider::StreamEvent::error("upstream down"))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), tx.closed())
            .await
            .expect("worker should finalize billing after the error event");

        // 没有账号完成请求：billing_target 必须回退 primary，且以 error 状态落库。
        assert_eq!(
            ctx.billing_target("anthropic", primary_account_id),
            ("anthropic".to_string(), primary_account_id)
        );
        assert_eq!(ctx.executed_provider_account(), None);
        let log = finalize_anthropic_billing(
            &keycompute_billing::BillingService::new(),
            &ctx,
            "anthropic",
            primary_account_id,
            "error",
        )
        .await
        .unwrap();
        assert_eq!(log.status, "error");
        assert_eq!(log.account_id, primary_account_id);
    }

    #[tokio::test]
    async fn failed_sse_send_does_not_record_client_first_content() {
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let stream = create_anthropic_stream_with_lifecycle(
            rx,
            Arc::clone(&ctx),
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
            Arc::clone(&recorder) as Arc<dyn keycompute_types::RequestLifecycleRecorder>,
        );
        drop(stream);

        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "message_start",
                "data": {"type": "message_start"}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        tx.send(llm_protocol_provider::StreamEvent::Done)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), tx.closed())
            .await
            .expect("worker should drain and stop after the disconnected send");

        assert!(ctx.is_client_disconnected());
        assert_eq!(
            ctx.client_response_outcome(),
            Some(keycompute_types::ClientResponseOutcome::ClientDisconnected)
        );
        assert_eq!(recorder.request_finishes().len(), 1);
        assert_eq!(
            recorder.request_finishes()[0].status,
            keycompute_types::RequestStatus::Cancelled
        );
        assert!(
            recorder
                .events()
                .iter()
                .all(|event| !event.starts_with("client_first_content:"))
        );
    }

    #[tokio::test]
    async fn closing_after_message_stop_keeps_the_response_successful() {
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let mut stream = Box::pin(create_anthropic_stream_with_lifecycle(
            rx,
            Arc::clone(&ctx),
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
            Arc::clone(&recorder) as Arc<dyn keycompute_types::RequestLifecycleRecorder>,
        ));

        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "message_stop",
                "data": {"type": "message_stop"}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("message_stop should be forwarded")
            .expect("the SSE stream should contain message_stop")
            .expect("forwarded SSE events are infallible");
        tokio::time::timeout(Duration::from_secs(1), async {
            while ctx.client_response_outcome()
                != Some(keycompute_types::ClientResponseOutcome::Succeeded)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("message_stop should commit the client response outcome");

        // A compliant SDK may stop polling immediately after the terminal
        // protocol event, before the internal normalized Done is consumed.
        // That expected close must not cancel the upstream consumer: Done is
        // still required to finish billing and persist the successful trace.
        drop(stream);
        tokio::task::yield_now().await;
        assert!(!ctx.is_client_disconnected());
        assert_eq!(
            ctx.client_response_outcome(),
            Some(keycompute_types::ClientResponseOutcome::Succeeded),
            "a close after the terminal frame must not overwrite success"
        );

        tx.send(llm_protocol_provider::StreamEvent::Done)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), tx.closed())
            .await
            .expect("the worker should drain the normalized Done and exit");
        assert_eq!(recorder.request_finishes().len(), 1);
        assert_eq!(
            recorder.request_finishes()[0].status,
            keycompute_types::RequestStatus::Succeeded
        );
    }

    #[tokio::test]
    async fn client_disconnect_during_no_event_window_marks_context() {
        // P2 回归：客户端在后台任务等待上游事件（尚未发送任何 SSE 帧）时断开。
        // 空窗期内没有失败的 send、executor 的 tx 又因本任务持有 receiver 而不会
        // 关闭，唯一能感知断开的是 sse_tx 的 channel 关闭；若不标记 ctx，primary
        // 随后失败时 executor 无法中止 fallback，会对已经离开的客户端发起无意义
        // 的上游调用并按 success 落库。
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let stream = create_anthropic_stream(
            rx,
            Arc::clone(&ctx),
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        );
        // 客户端断开：drop SSE stream，不发送任何上游事件
        drop(stream);

        // 后台任务必须感知 channel 关闭并标记断开，即使没有任何事件流过
        tokio::time::timeout(Duration::from_secs(1), async {
            while !ctx.is_client_disconnected() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("disconnect must be detected without any upstream events");

        // 标记断开后仍须继续 drain 上游事件，结算才能完成（channel 最终关闭）
        tx.send(llm_protocol_provider::StreamEvent::Done)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), tx.closed())
            .await
            .expect("worker should keep draining upstream events after marking disconnect");
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_client_is_marked_disconnected_after_sse_send_timeout() {
        // 客户端保持连接但停止读取（TCP backpressure）时，sse channel 一旦填满，
        // 无界的 send 会让结算任务永久卡住：executor 的 receiver 由本任务持有，
        // 断开检查不会触发，fallback 与结算都无法推进。有界超时（SSE_SEND_TIMEOUT）
        // 必须把停滞客户端视为断开并标记 ctx。
        let (tx, rx) = mpsc::channel(1);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let stream = create_anthropic_stream(
            rx,
            Arc::clone(&ctx),
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        );
        // 客户端“在线”但不消费：持有 SSE body 且从不 poll，channel receiver
        // 保持存活（send 不会因 closed 失败），只能靠超时打破阻塞。
        let _stalled_body = Sse::new(stream).into_response().into_body();

        // 填满 sse channel（capacity 100），后续帧的转发必然阻塞在 send 上
        let ping = llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "ping",
                "data": {"type": "ping"}
            })
            .to_string(),
        );
        for _ in 0..100 {
            tx.send(ping.clone()).await.unwrap();
        }
        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "message_start",
                "data": {"type": "message_start"}
            })
            .to_string(),
        ))
        .await
        .unwrap();

        // 推进虚拟时间：SSE_SEND_TIMEOUT 后 worker 必须把停滞的客户端视为断开
        tokio::time::timeout(Duration::from_secs(60), async {
            while !ctx.is_client_disconnected() {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await
        .expect("worker should treat a stalled client as disconnected after SSE_SEND_TIMEOUT");

        // 标记断开后继续 drain 与结算，Done 到达后 channel 关闭
        tx.send(llm_protocol_provider::StreamEvent::Done)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(60), tx.closed())
            .await
            .expect("worker should finalize billing after marking the stalled client disconnected");
    }

    #[tokio::test]
    async fn sse_error_frame_with_trailing_whitespace_name_is_not_forwarded() {
        // P3 锁定：上游可能发送 `event: error `（event 名尾随空白）。解析器 trim
        // 后 event_name 为 "error"，handler 必须将其当作错误帧跳过转发，并只向
        // 客户端输出标准化的 "Upstream request failed"，不泄露上游 body。
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let stream = create_anthropic_stream(
            rx,
            ctx,
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        );

        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "error ",
                "data": {"type": "error", "error": {"message": "upstream-secret"}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        tx.send(llm_protocol_provider::StreamEvent::error("upstream-secret"))
            .await
            .unwrap();
        drop(tx);

        let response = Sse::new(stream).into_response();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Upstream request failed"));
        assert!(!body.contains("upstream-secret"));
    }

    #[tokio::test]
    async fn sse_error_frame_with_exact_event_name_is_not_forwarded() {
        // 覆盖 event_name == "error" 的精确匹配路径（其余错误帧测试走
        // data.type 兜底分支）：名称为 "error" 的 Raw 帧必须被跳过转发，
        // 客户端只能看到标准化的 "Upstream request failed"，不泄露上游 body。
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            true,
            keycompute_types::PricingSnapshot::default(),
        ));
        let stream = create_anthropic_stream(
            rx,
            ctx,
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        );

        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_sse",
                "event": "error",
                "data": {"type": "error", "error": {"message": "upstream-secret"}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        tx.send(llm_protocol_provider::StreamEvent::error("upstream-secret"))
            .await
            .unwrap();
        drop(tx);

        let response = Sse::new(stream).into_response();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Upstream request failed"));
        assert!(!body.contains("upstream-secret"));
    }

    #[tokio::test]
    async fn non_stream_error_does_not_expose_upstream_error_message() {
        let (tx, rx) = mpsc::channel(1);
        tx.send(llm_protocol_provider::StreamEvent::error("upstream-secret"))
            .await
            .unwrap();
        drop(tx);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            false,
            keycompute_types::PricingSnapshot::default(),
        ));
        let error = create_anthropic_response(
            rx,
            ctx,
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "Provider error: Upstream request failed");
    }

    #[tokio::test]
    async fn non_stream_worker_survives_handler_cancellation() {
        let (tx, rx) = mpsc::channel(4);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            false,
            keycompute_types::PricingSnapshot::default(),
        ));
        let handler = tokio::spawn(create_anthropic_response_with_lifecycle(
            rx,
            Arc::clone(&ctx),
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
            Arc::new(keycompute_types::NoopRequestLifecycleRecorder),
        ));
        tokio::task::yield_now().await;
        handler.abort();
        let _ = handler.await;

        tx.send(llm_protocol_provider::StreamEvent::error(
            "client disconnected",
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), tx.closed())
            .await
            .expect("detached worker must consume the terminal event after handler cancellation");

        assert!(ctx.is_client_disconnected());
        assert_eq!(
            ctx.client_response_outcome(),
            Some(keycompute_types::ClientResponseOutcome::ClientDisconnected)
        );
    }

    #[tokio::test]
    async fn non_stream_response_preserves_the_native_message_body() {
        let (tx, rx) = mpsc::channel(2);
        tx.send(llm_protocol_provider::StreamEvent::raw(
            json!({
                "kind": "anthropic_message",
                "body": {
                    "id": "msg_native",
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_native",
                        "name": "read_file",
                        "input": {"path": "README.md"}
                    }],
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 11, "output_tokens": 7}
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        tx.send(llm_protocol_provider::StreamEvent::Done)
            .await
            .unwrap();
        drop(tx);

        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            false,
            keycompute_types::PricingSnapshot::default(),
        ));
        let response = create_anthropic_response(
            rx,
            ctx,
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
        )
        .await
        .unwrap();

        assert_eq!(response["id"], "msg_native");
        assert_eq!(response["content"][0]["type"], "tool_use");
        assert_eq!(response["content"][0]["input"]["path"], "README.md");
        assert_eq!(response["stop_reason"], "tool_use");
    }

    #[tokio::test]
    async fn non_stream_completion_without_native_body_is_an_error() {
        // 防御路径：流以 Done 结束但 Raw(anthropic_message) 未到达时，不得返回
        // 空 content 的 200（静默空成功会误导客户端与计费）；必须以错误结束。
        let (tx, rx) = mpsc::channel(1);
        tx.send(llm_protocol_provider::StreamEvent::Done)
            .await
            .unwrap();
        drop(tx);
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            false,
            keycompute_types::PricingSnapshot::default(),
        ));
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());

        let error = create_anthropic_response_with_lifecycle(
            rx,
            Arc::clone(&ctx),
            "anthropic".to_string(),
            uuid::Uuid::new_v4(),
            Arc::new(keycompute_billing::BillingService::new()),
            recorder as Arc<dyn keycompute_types::RequestLifecycleRecorder>,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, ApiError::Internal(_)),
            "missing native body must surface as an internal error, got {error}"
        );
        assert_eq!(
            ctx.client_response_outcome(),
            Some(keycompute_types::ClientResponseOutcome::ResponseFailed)
        );
    }

    #[tokio::test]
    async fn non_stream_fallback_attributes_billing_to_completed_account() {
        // 非流式路径的结算必须归属到实际完成请求的账号：executor 在 fallback
        // 成功后写入 executed_provider_account，finalize 应使用该账号而非 primary。
        let ctx = Arc::new(RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            false,
            keycompute_types::PricingSnapshot::default(),
        ));
        ctx.set_input_tokens(10);
        ctx.set_output_tokens(20);

        let primary_account_id = uuid::Uuid::new_v4();
        let fallback_account_id = uuid::Uuid::new_v4();
        ctx.set_executed_provider_account("anthropic", fallback_account_id);

        let log = finalize_anthropic_billing(
            &keycompute_billing::BillingService::new(),
            &ctx,
            "anthropic",
            primary_account_id,
            "success",
        )
        .await
        .unwrap();

        assert_eq!(log.provider_name, "anthropic");
        assert_eq!(log.account_id, fallback_account_id);
        assert_ne!(log.account_id, primary_account_id);
        assert_eq!(log.status, "success");
    }
}
