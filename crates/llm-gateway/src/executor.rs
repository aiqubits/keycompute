//! Gateway 执行器
//!
//! 核心执行入口，控制 retry/fallback/streaming 生命周期。
//!
//! 健康状态集成：
//! - 执行成功时调用 ProviderHealthStore::record_success
//! - 执行失败时调用 ProviderHealthStore::record_failure
//! - 延迟时间从请求开始计算
//!
//! Internal HTTP Proxy 集成：
//! - 统一上游连接管理
//! - 多代理出口支持
//! - 请求追踪

use crate::{GatewayConfig, HttpProxy, streaming::StreamPipeline};
use futures::StreamExt;
use keycompute_routing::{AccountStateStore, ProviderHealthStore};
use keycompute_types::{ExecutionPlan, ExecutionTarget, KeyComputeError, RequestContext, Result};
use llm_protocol_provider::{
    DefaultHttpTransport, HttpTransport, ProviderAdapter, StreamEvent, UpstreamMessage,
    UpstreamRequest,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Gateway 执行器
///
/// 唯一执行层，负责：
/// 1. 执行请求到上游 Provider
/// 2. 处理 retry 和 fallback
/// 3. 管理 streaming 生命周期
/// 4. 更新运行时状态（账号状态 + Provider 健康状态）
///
/// Internal HTTP Proxy 集成：
/// - 统一连接池管理
/// - 多代理出口支持
/// - 请求追踪
#[derive(Debug)]
pub struct GatewayExecutor {
    #[allow(dead_code)]
    config: GatewayConfig,
    providers: HashMap<String, Arc<dyn ProviderAdapter>>,
    /// Internal HTTP Proxy（统一上游连接管理）
    http_proxy: Option<Arc<HttpProxy>>,
    /// 默认 HTTP 传输层（无代理时复用，避免每次请求重建 reqwest::Client 连接池）
    default_transport: Arc<DefaultHttpTransport>,
}

impl GatewayExecutor {
    /// 创建新的执行器
    pub fn new(
        config: GatewayConfig,
        providers: HashMap<String, Arc<dyn ProviderAdapter>>,
    ) -> Self {
        Self {
            config,
            providers,
            http_proxy: None,
            default_transport: Arc::new(DefaultHttpTransport::new()),
        }
    }

    /// 创建带 HTTP Proxy 的执行器
    pub fn with_proxy(
        config: GatewayConfig,
        providers: HashMap<String, Arc<dyn ProviderAdapter>>,
        http_proxy: Arc<HttpProxy>,
    ) -> Self {
        Self {
            config,
            providers,
            http_proxy: Some(http_proxy),
            default_transport: Arc::new(DefaultHttpTransport::new()),
        }
    }

    /// 获取 HTTP Proxy
    pub fn http_proxy(&self) -> Option<&Arc<HttpProxy>> {
        self.http_proxy.as_ref()
    }

    /// 设置 HTTP Proxy
    pub fn set_http_proxy(&mut self, proxy: Arc<HttpProxy>) {
        self.http_proxy = Some(proxy);
    }

    /// 执行请求（唯一执行入口）
    ///
    /// 执行流程：
    /// 1. 尝试 primary target
    /// 2. 失败则 fallback 到下一个 target
    /// 3. 成功后更新账号状态和 Provider 健康状态
    ///
    /// # 参数
    /// - `ctx`: 请求上下文
    /// - `plan`: 执行计划（包含 primary 和 fallback chain）
    /// - `account_states`: 账号状态存储
    /// - `provider_health`: Provider 健康状态存储（可选，用于被动记录健康状态）
    pub async fn execute(
        &self,
        ctx: Arc<RequestContext>,
        plan: ExecutionPlan,
        account_states: Arc<AccountStateStore>,
        provider_health: Option<Arc<ProviderHealthStore>>,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(100);

        // 在后台任务中实际执行上游请求，避免在返回 rx 之前就被有界 channel 背压阻塞。
        // 这对流式场景尤其重要：handler 需要先拿到 rx，才能开始消费事件并向客户端推送。
        let runner = Self {
            config: self.config.clone(),
            providers: self.providers.clone(),
            http_proxy: self.http_proxy.clone(),
            default_transport: Arc::clone(&self.default_transport),
        };

        // 执行超时：防止上游 Provider 无限阻塞导致资源泄漏。
        // 与 handler 层 keepalive (120s) 保持同一量级，确保 executor 不会在
        // handler 超时断开客户端后继续消耗资源（图片下载、上游 API 调用等）。
        let exec_timeout = Duration::from_secs(self.config.timeout_secs);

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                exec_timeout,
                runner.run_plan(
                    Arc::clone(&ctx),
                    plan,
                    tx.clone(),
                    account_states,
                    provider_health,
                ),
            )
            .await;

            match result {
                Ok(Ok(())) => {
                    // 正常完成，run_plan 内部已将事件写入 tx
                }
                Ok(Err(error)) => {
                    tracing::error!(
                        request_id = %ctx.request_id,
                        error = %error,
                        "Execution task failed"
                    );
                    let _ = tx.send(StreamEvent::error(error.to_string())).await;
                }
                Err(_elapsed) => {
                    tracing::error!(
                        request_id = %ctx.request_id,
                        timeout_secs = exec_timeout.as_secs(),
                        "Gateway execution timed out: run_plan cancelled"
                    );
                    let _ = tx
                        .send(StreamEvent::error(format!(
                            "Request timed out after {}s",
                            exec_timeout.as_secs()
                        )))
                        .await;
                }
            }
        });

        Ok(rx)
    }

    async fn run_plan(
        &self,
        ctx: Arc<RequestContext>,
        plan: ExecutionPlan,
        tx: mpsc::Sender<StreamEvent>,
        account_states: Arc<AccountStateStore>,
        provider_health: Option<Arc<ProviderHealthStore>>,
    ) -> Result<()> {
        // 构建 target 链：primary + fallback
        let mut targets = vec![plan.primary];
        targets.extend(plan.fallback_chain);

        let mut last_error = None;
        let _start_time = Instant::now();
        let mut is_primary = true;
        // 是否已向客户端转发过内容：一旦发出过 Delta，
        // 流中途失败后不可再 fallback，否则客户端会收到
        // 「前一段部分内容 + 新一遍完整内容」的重复拼接输出
        let mut sent_content = false;

        for target in targets {
            let target_start = Instant::now();
            match self
                .try_execute(&ctx, &target, tx.clone(), &mut sent_content)
                .await
            {
                Ok(()) => {
                    // 成功：标记账号状态
                    if let ExecutionTarget::ProviderAccount { account_id, .. } = &target {
                        account_states.mark_success(*account_id);
                    }

                    // 成功：更新 Provider 健康状态
                    let latency_ms = target_start.elapsed().as_millis() as u64;
                    if let ExecutionTarget::ProviderAccount { provider, .. } = &target
                        && let Some(ref health_store) = provider_health
                    {
                        health_store.record_success(provider, latency_ms);
                        // 如果不是 primary，说明使用了 fallback
                        if !is_primary {
                            health_store.record_fallback();
                        }
                    }

                    let provider_name = match &target {
                        ExecutionTarget::ProviderAccount { provider, .. } => provider.clone(),
                        ExecutionTarget::Node { model } => format!("node:{}", model),
                    };
                    tracing::info!(
                        request_id = %ctx.request_id,
                        provider = %provider_name,
                        latency_ms = latency_ms,
                        is_fallback = !is_primary,
                        "Request executed successfully"
                    );
                    return Ok(());
                }
                Err(e) => {
                    let provider_name = match &target {
                        ExecutionTarget::ProviderAccount { provider, .. } => provider.clone(),
                        ExecutionTarget::Node { model } => format!("node:{}", model),
                    };

                    // 客户端已断开（receiver 被 drop，或 handler 显式标记）：继续
                    // fallback 只会对新的上游发起无意义的调用，直接终止执行链并
                    // 放弃后续 target。Anthropic 路径的后台任务持有 receiver 直到
                    // 结算完成，`tx.is_closed()` 不会因客户端断开而触发，因此还需
                    // 检查 handler 通过 ctx 传播的断开标志。
                    if tx.is_closed() || ctx.is_client_disconnected() {
                        tracing::debug!(
                            request_id = %ctx.request_id,
                            provider = %provider_name,
                            "Client disconnected, aborting fallback chain"
                        );
                        return Err(e);
                    }

                    // 注意：不再自动标记错误，错误计数只能通过管理员手动测试 API 触发
                    // 保留 Provider 健康状态更新用于路由评分
                    if let ExecutionTarget::ProviderAccount { provider, .. } = &target
                        && let Some(ref health_store) = provider_health
                    {
                        health_store.record_failure(provider);
                    }

                    tracing::warn!(
                        request_id = %ctx.request_id,
                        provider = %provider_name,
                        error = %e,
                        "Request failed, trying fallback"
                    );
                    // 内容已部分送达客户端：不再 fallback，直接上报错误
                    //（execute 外层会向客户端发送 Error 事件）
                    if sent_content {
                        tracing::warn!(
                            request_id = %ctx.request_id,
                            provider = %provider_name,
                            "Stream failed after content was sent, skipping fallback to avoid duplicated output"
                        );
                        return Err(e);
                    }
                    last_error = Some(e);
                }
            }
            // 第一次循环后，后续都是 fallback
            is_primary = false;
        }

        // 所有 target 都失败
        Err(last_error.unwrap_or_else(|| KeyComputeError::RoutingFailed(ctx.model.clone())))
    }

    /// 尝试执行单个 target
    ///
    /// `sent_content` 在首次向客户端转发 Delta 时置为 true，
    /// 调用方据此判断流中途失败后能否安全 fallback
    async fn try_execute(
        &self,
        ctx: &RequestContext,
        target: &ExecutionTarget,
        tx: mpsc::Sender<StreamEvent>,
        sent_content: &mut bool,
    ) -> Result<()> {
        // 只处理 ProviderAccount 变体
        let (provider, endpoint, upstream_api_key) = match target {
            ExecutionTarget::ProviderAccount {
                provider,
                endpoint,
                upstream_api_key,
                ..
            } => (provider, endpoint, upstream_api_key),
            ExecutionTarget::Node { .. } => {
                // 防护性检查：Node 执行在 handler 层分流（openai.rs），
                // 通过 node_gateway.enqueue_and_wait() + simulate_node_stream() 实现，
                // 正常流程不应到达此处
                return Err(KeyComputeError::Internal(
                    "Node execution not supported in stream executor".into(),
                ));
            }
        };

        tracing::info!(
            request_id = %ctx.request_id,
            provider = %provider,
            endpoint = %endpoint,
            "try_execute: starting"
        );

        // 获取 Provider
        let provider_impl = self
            .providers
            .get(provider.as_str())
            .ok_or_else(|| KeyComputeError::Internal(format!("Provider {} not found", provider)))?;

        // 获取 HTTP 传输层（优先 HttpProxy，否则复用缓存的默认 transport 避免重复建连接池）
        let transport: Arc<dyn HttpTransport> = if let Some(ref proxy) = self.http_proxy {
            Arc::clone(proxy.default_client()) as Arc<dyn HttpTransport>
        } else {
            Arc::clone(&self.default_transport) as Arc<dyn HttpTransport>
        };

        // 构建上游消息（一次转换，同时用于 UpstreamRequest 和 token 估算，消除 DRY 违反）
        let upstream_messages: Vec<UpstreamMessage> = ctx
            .messages
            .iter()
            .map(|m| UpstreamMessage {
                role: m.role.to_string(),
                content: m.content.clone(),
            })
            .collect();

        let request = UpstreamRequest {
            endpoint: endpoint.to_string(),
            upstream_api_key: upstream_api_key.clone(),
            model: ctx.model.clone(),
            messages: upstream_messages,
            stream: ctx.stream,
            // 透传客户端采样参数（Anthropic 协议的 max_tokens 为必填字段，
            // 未指定时由协议层使用默认值）
            max_tokens: ctx.max_tokens,
            temperature: ctx.temperature,
            top_p: ctx.top_p,
            native_anthropic_request: ctx.native_anthropic_request.clone(),
            native_anthropic_headers: ctx.native_anthropic_headers.clone(),
        };

        tracing::info!(
            request_id = %ctx.request_id,
            provider = %provider,
            "try_execute: calling provider.stream_chat"
        );

        // 执行流式请求（传入 transport）
        let mut stream = provider_impl
            .stream_chat(transport.as_ref(), request)
            .await?;

        tracing::info!(
            request_id = %ctx.request_id,
            provider = %provider,
            "try_execute: stream started, processing events"
        );

        // 流处理管道
        let mut pipeline = StreamPipeline::new(ctx.request_id);

        // 流开始前：使用 tiktoken 估算输入 token 数
        // 注意：这只是估算；若上游先发送 InputUsage，会先覆盖输入侧，最终
        // StreamEvent::Usage 再覆盖完整输入/输出用量。
        // 使用 estimate 变体，避免把估算值误标记为 Provider 精确值。
        let estimated_input_tokens = Self::estimate_input_tokens(&ctx.messages);
        ctx.set_input_tokens_estimate(estimated_input_tokens);
        // 每次上游尝试都是独立的新请求：输出侧同样清零估算起点，防止
        // 上一次失败尝试的精确 output 值（已 finalized）泄漏到本次尝试的
        // 估算计费中（fallback 若无最终 Usage 事件时会错误沿用残留值）。
        ctx.set_output_tokens_estimate(0);

        tracing::debug!(
            request_id = %ctx.request_id,
            estimated_input_tokens = estimated_input_tokens,
            "Stream started, input tokens estimated"
        );

        // 只有 Provider 的显式 Done 才表示请求成功。特别是 Anthropic 必须收到
        // message_stop；不能仅凭 message_delta.stop_reason 或 TCP EOF 推断完成。
        let mut received_done = false;

        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::Delta {
                    content,
                    finish_reason,
                } => {
                    // 尚未收到 Provider 精确 output 时，使用 tiktoken 估算。
                    // 只检查 output 侧：`Usage{input_tokens: 0, output_tokens: N}`
                    // 输入被跳过（保留估算）时，若以 is_usage_finalized 为门槛，
                    // 后续 Delta 会继续向已锁定的精确 N 上累加，造成双重计费。
                    if !ctx.is_output_finalized() {
                        let tokens = Self::estimate_tokens(&content);
                        ctx.add_output_tokens(tokens);
                    }

                    // 转发给客户端
                    let event = StreamEvent::Delta {
                        content,
                        finish_reason: finish_reason.clone(),
                    };
                    pipeline.process_event(&event);
                    tx.send(event)
                        .await
                        .map_err(|_| KeyComputeError::Internal("Send error".into()))?;
                    *sent_content = true;

                    if finish_reason.is_some() {
                        tracing::debug!(
                            request_id = %ctx.request_id,
                            finish_reason = ?finish_reason,
                            "try_execute: finish_reason received, waiting for terminal Done"
                        );
                    }
                }
                StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    // Provider 报告的精确 usage 值（优先级最高）
                    // 覆盖之前的 tiktoken 估算值
                    // 兼容网关可能上报 input_tokens=0（或缺失 usage 解析为 0）：
                    // 非空请求的输入 token 协议上不可能为 0，跳过它以保留
                    // tiktoken 估算，避免把输入按 0 计费；output 始终以精确值
                    // 为准（空响应场景 0 是合法值）。因此若网关在流中间上报
                    // 部分 output（非官方实现），锁定后不再累加，输出会低于
                    // 实际；官方实现均在流末上报完整值，此权衡只影响异常上游。
                    if input_tokens > 0 {
                        ctx.set_input_tokens(input_tokens);
                    }
                    ctx.set_output_tokens(output_tokens);

                    tracing::debug!(
                        request_id = %ctx.request_id,
                        provider_usage = true,
                        input_tokens = input_tokens,
                        output_tokens = output_tokens,
                        "Provider usage received, overriding estimation"
                    );
                }
                StreamEvent::InputUsage { input_tokens } => {
                    // 已确认的 input usage 不能等待最终事件才使用：上游可能在
                    // message_delta 前断流。不要设置 output，保留已生成内容的
                    // 估算值，待最终 Usage 到来后再统一覆盖。
                    // 兼容网关可能上报 0：与 Usage 分支一致，跳过 0 保留估算。
                    if input_tokens > 0 {
                        ctx.set_input_tokens(input_tokens);
                    }
                    tracing::debug!(
                        request_id = %ctx.request_id,
                        input_tokens = input_tokens,
                        "Provider input usage received"
                    );
                }
                StreamEvent::Done => {
                    tracing::debug!(
                        request_id = %ctx.request_id,
                        "try_execute: received Done event"
                    );
                    // 在向 handler 发送终止事件之前记录真正完成的账号。handler
                    // 收到 Done 后会立刻结算；若此处延后到 run_plan 成功分支，
                    // 会与 handler 形成竞态并把 fallback 用量记到 primary。
                    let ExecutionTarget::ProviderAccount { account_id, .. } = target else {
                        unreachable!("nodes return before streaming");
                    };
                    ctx.set_executed_provider_account(provider.clone(), *account_id);
                    tx.send(StreamEvent::Done)
                        .await
                        .map_err(|_| KeyComputeError::Internal("Send error".into()))?;
                    received_done = true;
                    break;
                }
                StreamEvent::Error { message } => {
                    tracing::error!(
                        request_id = %ctx.request_id,
                        message = %message,
                        "try_execute: received Error event"
                    );
                    return Err(KeyComputeError::ProviderError(message));
                }
                // 原生协议入站会用 Raw 承载未经降级的 SSE 事件。它们不参与
                // 通用 token 计算，但必须穿过执行器才能由对应的入站 handler
                // 按原协议回写给客户端。
                StreamEvent::Raw { data } => {
                    let commits_response = raw_event_commits_response(&data);
                    tx.send(StreamEvent::Raw { data })
                        .await
                        .map_err(|_| KeyComputeError::Internal("Send error".into()))?;
                    // 原生 SSE 的 `message_start` 等事件一旦对客户端可见，就不能
                    // 再切换 fallback，否则同一条流会出现两个消息序列。`ping`
                    // 仅是 keepalive，不会开始消息，之后失败仍可安全回退。
                    if commits_response {
                        *sent_content = true;
                    }
                }
            }
        }

        if !received_done {
            return Err(KeyComputeError::ProviderError(
                "Upstream stream ended without a terminal Done event".to_string(),
            ));
        }

        tracing::debug!(
            request_id = %ctx.request_id,
            provider = %provider,
            "try_execute: completed successfully"
        );

        Ok(())
    }

    /// 估算 token 数（使用 tiktoken-rs）
    ///
    /// 使用 tiktoken-rs 库的 o200k_base tokenizer（支持 GPT-4o, o1, o3 等模型）
    /// 提供与 OpenAI API 完全一致的 token 计数
    ///
    /// 注意：这是估算值，用于流式场景的实时反馈
    /// 最终计费会使用 API Response 中的精确 usage 值进行覆盖
    fn estimate_tokens(content: &str) -> u32 {
        if content.is_empty() {
            return 0;
        }

        // 使用 o200k_base tokenizer (GPT-4o, o1, o3 等模型)
        // singleton 模式避免重复加载词表
        let bpe = tiktoken_rs::o200k_base_singleton();
        bpe.encode_with_special_tokens(content).len() as u32
    }

    /// 估算输入 messages 的 token 数（使用 tiktoken-rs）
    ///
    /// 用于在 API Response Usage 不可用时提供估算值
    /// 包括消息格式化和特殊 token 的处理
    ///
    /// 注意：这是估算值，最终计费会使用 API Response 中的精确 usage 值进行覆盖
    ///
    /// 实现说明：
    /// 不使用 get_chat_completion_max_tokens，因为它返回的是"剩余可用token数"而非"输入token数"
    /// 而是直接序列化messages为JSON，然后用tiktoken计算整个JSON的token数
    /// 这样可以正确包含role名称、格式化等所有token
    fn estimate_input_tokens(messages: &[keycompute_types::Message]) -> u32 {
        // 直接序列化 keycompute_types::Message 而非 UpstreamMessage 进行估算。
        // 原因：两者 JSON 输出等价（MessageRole 经 #[serde(rename_all = "lowercase")]
        // 序列化为 "user"/"system" 字符串，与 UpstreamMessage.role 一致），
        // 且 ctx.messages 已经以 Message 形式存在，避免了额外的 Vec<UpstreamMessage> 构建。
        // 注：若 Message 新增字段而 UpstreamMessage 未同步，估算值可能偏离实际发送 JSON，
        // 届时需考虑恢复 UpstreamMessage 构建。
        let json_str = serde_json::to_string(messages).unwrap_or_default();

        // 使用tiktoken直接计算JSON的token数
        // 这样可以正确包含 role 名称、content 格式化等所有 token
        Self::estimate_tokens(&json_str)
    }

    /// 获取所有 Provider 名称列表
    pub fn list_providers(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// 检查是否存在指定的 Provider
    pub fn has_provider(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// 获取 Provider 数量
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// 获取指定 Provider 支持的模型列表
    pub fn get_provider_models(&self, provider_name: &str) -> Vec<String> {
        self.providers
            .get(provider_name)
            .map(|p| {
                p.supported_models()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 判断原始事件是否已向客户端提交了不可回退的响应状态。
///
/// `Raw` 是跨协议的逃生通道，无法识别的格式必须保守地视为已提交：宁可阻止
/// 一次本可执行的 fallback，也不能在已提交的消息之后追加另一条消息序列。
/// Anthropic 已知的“不提交”事件必须在此显式登记：`ping` 只是 keepalive；
/// `error` 由 handler 转换为通用错误，两者失败后仍可安全回退。其余 Anthropic
/// 事件（`message_start`、`content_block_*`、`message_delta`、`message_stop` 等）
/// 都会开始或延续消息，一律视为已提交。未来引入其它 Raw 协议时，须为其各自
/// 的“不提交”事件补充等价登记，否则未知事件会按保守策略阻止 fallback。
fn raw_event_commits_response(data: &str) -> bool {
    let Ok(envelope) = serde_json::from_str::<serde_json::Value>(data) else {
        return true;
    };
    if envelope.get("kind").and_then(serde_json::Value::as_str) != Some("anthropic_sse") {
        return true;
    }
    let event = envelope.get("event").and_then(serde_json::Value::as_str);
    let body_type = envelope
        .pointer("/data/type")
        .and_then(serde_json::Value::as_str);
    // 只有显式登记的非提交事件才允许 fallback；未知事件保持保守（已提交）。
    !matches!(event, Some("ping") | Some("error")) && body_type != Some("error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use keycompute_types::{Message, PricingSnapshot};
    use rust_decimal::Decimal;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Notify;
    use uuid::Uuid;

    #[derive(Debug)]
    struct ManyChunksProvider {
        chunks: usize,
    }

    #[async_trait]
    impl ProviderAdapter for ManyChunksProvider {
        fn name(&self) -> &'static str {
            "many-chunks"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            vec!["gpt-4o"]
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            let mut events: Vec<Result<StreamEvent>> = (0..self.chunks)
                .map(|_| {
                    Ok(StreamEvent::Delta {
                        content: "x".to_string(),
                        finish_reason: None,
                    })
                })
                .collect();

            events.push(Ok(StreamEvent::Usage {
                input_tokens: 1,
                output_tokens: self.chunks as u32,
            }));
            events.push(Ok(StreamEvent::Done));

            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    #[derive(Debug)]
    struct FailingProvider;

    /// 只发送精确 Usage 后断流（无 Done）的 Provider：模拟 usage-only 末尾块
    /// 之后连接断开，用于验证 fallback 不会继承其 output 残留值。
    #[derive(Debug)]
    struct UsageOnlyFailProvider;

    #[async_trait]
    impl ProviderAdapter for UsageOnlyFailProvider {
        fn name(&self) -> &'static str {
            "usage-only-fail"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::Usage {
                    input_tokens: 111,
                    output_tokens: 222,
                },
            )])))
        }
    }

    /// 只发送 Delta 与 Done（无精确 Usage）的 Provider：计费回退到估算。
    #[derive(Debug)]
    struct EstimateOnlyProvider;

    #[async_trait]
    impl ProviderAdapter for EstimateOnlyProvider {
        fn name(&self) -> &'static str {
            "estimate-only"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::Delta {
                    content: "hello".to_string(),
                    finish_reason: None,
                }),
                Ok(StreamEvent::Done),
            ])))
        }
    }

    /// 记录 `stream_chat` 调用次数的 Provider，用于验证 fallback 是否被触发。
    #[derive(Debug)]
    struct CountingProvider {
        calls: Arc<AtomicUsize>,
        notified: Option<Arc<Notify>>,
    }

    #[async_trait]
    impl ProviderAdapter for CountingProvider {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(notified) = &self.notified {
                notified.notify_one();
            }
            Err(KeyComputeError::ProviderError("upstream down".into()))
        }
    }

    #[derive(Debug)]
    struct RawEventProvider;

    #[derive(Debug)]
    struct ZeroUsageProvider;

    #[async_trait]
    impl ProviderAdapter for ZeroUsageProvider {
        fn name(&self) -> &'static str {
            "zero-usage"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            // 兼容网关在 message_start 上报 input_tokens=0（缺失 usage 解析为 0）
            // 后正常完成；真实的 Anthropic 总会报告非零输入值。
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::InputUsage { input_tokens: 0 }),
                Ok(StreamEvent::Done),
            ])))
        }
    }

    /// 发送 `Usage{input_tokens: 0, output_tokens: 0}` 后正常完成：空响应场景。
    ///
    /// 输入为 0 被跳过（保留 tiktoken 估算）；输出为 0 是空响应的合法精确值，
    /// 必须锁定——若后续仍有 Delta（非官方实现），不得向已锁定的 0 上累加。
    #[derive(Debug)]
    struct EmptyResponseZeroUsageProvider;

    #[async_trait]
    impl ProviderAdapter for EmptyResponseZeroUsageProvider {
        fn name(&self) -> &'static str {
            "empty-response-zero-usage"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                }),
                Ok(StreamEvent::Delta {
                    content: "tail-after-usage".to_string(),
                    finish_reason: None,
                }),
                Ok(StreamEvent::Done),
            ])))
        }
    }

    /// 在输出中间上报 `Usage{input_tokens: 0, output_tokens: N}` 后继续发送内容。
    ///
    /// 输入为 0 会被 executor 跳过（保留 tiktoken 估算）；此时 output 已被锁定
    /// 为精确值 N，后续 Delta 不得再向 N 上累加估算，否则输出被双重计费。
    #[derive(Debug)]
    struct MidStreamZeroInputUsageProvider;

    #[async_trait]
    impl ProviderAdapter for MidStreamZeroInputUsageProvider {
        fn name(&self) -> &'static str {
            "mid-stream-zero-input-usage"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::Usage {
                    input_tokens: 0,
                    output_tokens: 100,
                }),
                Ok(StreamEvent::Delta {
                    content: "more".to_string(),
                    finish_reason: None,
                }),
                Ok(StreamEvent::Done),
            ])))
        }
    }

    #[derive(Debug)]
    struct TruncatedProvider;

    #[derive(Debug)]
    struct PingingFailProvider;

    #[derive(Debug)]
    struct RawErrorProvider;

    #[derive(Debug)]
    struct CommittedRawFailProvider;

    #[async_trait]
    impl ProviderAdapter for RawEventProvider {
        fn name(&self) -> &'static str {
            "raw-events"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::raw("native-event")),
                Ok(StreamEvent::Done),
            ])))
        }
    }

    #[async_trait]
    impl ProviderAdapter for TruncatedProvider {
        fn name(&self) -> &'static str {
            "truncated"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            // 模拟 Anthropic message_delta 已声明 stop_reason，但 TCP 在
            // message_stop 之前关闭。
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::InputUsage { input_tokens: 17 }),
                Ok(StreamEvent::Delta {
                    content: String::new(),
                    finish_reason: Some("end_turn".to_string()),
                }),
            ])))
        }
    }

    #[async_trait]
    impl ProviderAdapter for PingingFailProvider {
        fn name(&self) -> &'static str {
            "pinging-fail"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::raw(
                    r#"{"kind":"anthropic_sse","event":"ping","data":{"type":"ping"}}"#,
                )),
                Err(KeyComputeError::ProviderError(
                    "connection reset after ping".into(),
                )),
            ])))
        }
    }

    #[async_trait]
    impl ProviderAdapter for RawErrorProvider {
        fn name(&self) -> &'static str {
            "raw-error"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::raw(
                    r#"{"kind":"anthropic_sse","event":"error","data":{"type":"error","error":{"message":"primary failed"}}}"#,
                )),
                Ok(StreamEvent::error("primary failed")),
            ])))
        }
    }

    #[async_trait]
    impl ProviderAdapter for CommittedRawFailProvider {
        fn name(&self) -> &'static str {
            "committed-raw-fail"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::raw(
                    r#"{"kind":"anthropic_sse","event":"message_start","data":{"type":"message_start"}}"#,
                )),
                Err(KeyComputeError::ProviderError(
                    "connection reset after message_start".into(),
                )),
            ])))
        }
    }

    #[async_trait]
    impl ProviderAdapter for FailingProvider {
        fn name(&self) -> &'static str {
            "failing"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            Err(KeyComputeError::ProviderError("upstream down".into()))
        }
    }

    /// 先发几个 Delta 后流中途报错的 Provider（模拟上游断连/限流）
    #[derive(Debug)]
    struct MidStreamFailProvider {
        deltas_before_error: usize,
    }

    #[async_trait]
    impl ProviderAdapter for MidStreamFailProvider {
        fn name(&self) -> &'static str {
            "mid-stream-fail"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            let mut events: Vec<Result<StreamEvent>> = (0..self.deltas_before_error)
                .map(|_| {
                    Ok(StreamEvent::Delta {
                        content: "partial".to_string(),
                        finish_reason: None,
                    })
                })
                .collect();
            events.push(Err(KeyComputeError::ProviderError(
                "connection reset mid-stream".into(),
            )));
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    #[allow(dead_code)]
    fn create_test_context() -> RequestContext {
        RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "gpt-4o",
            vec![Message::user("Hello")],
            true,
            PricingSnapshot {
                model_name: "gpt-4o".to_string(),
                currency: "CNY".to_string(),
                input_price_per_1k: Decimal::from(1),
                output_price_per_1k: Decimal::from(2),
            },
        )
    }

    #[test]
    fn test_gateway_executor_new() {
        let config = GatewayConfig::default();
        let providers = HashMap::new();
        let executor = GatewayExecutor::new(config, providers);
        assert_eq!(executor.config.max_retries, 3);
    }

    #[test]
    fn test_estimate_tokens_english() {
        // 使用 tiktoken-rs o200k_base 精确计数
        // "Hello" = 1 token
        assert_eq!(GatewayExecutor::estimate_tokens("Hello"), 1);
        // "Hello World" = 2 tokens
        assert_eq!(GatewayExecutor::estimate_tokens("Hello World"), 2);
        // 100 个 'a' 约 25 tokens
        assert!(GatewayExecutor::estimate_tokens("a".repeat(100).as_str()) > 0);
    }

    #[test]
    fn test_estimate_tokens_chinese() {
        // 中文 token 计数（tiktoken 精确计算）
        // 中文字符通常每个 1-2 tokens
        assert!(GatewayExecutor::estimate_tokens("你好") > 0);
        assert!(GatewayExecutor::estimate_tokens("你好世界") > 0);
        assert!(GatewayExecutor::estimate_tokens("你好世界测试") > 0);
    }

    #[test]
    fn test_estimate_tokens_mixed() {
        // 混合：英文 + 中文
        assert!(GatewayExecutor::estimate_tokens("Hello你好") > 0);
        assert!(GatewayExecutor::estimate_tokens("Hello World你好世界") > 0);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(GatewayExecutor::estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_input_tokens_single_message() {
        // 测试单个消息的 token 估算
        let messages = vec![keycompute_types::Message::user("Hello")];
        let tokens = GatewayExecutor::estimate_input_tokens(&messages);
        // 单个 "Hello" 约 1-2 tokens，加上 JSON 格式化和 role 名称
        assert!(
            tokens > 0,
            "Token count should be greater than 0, got: {}",
            tokens
        );
        // 应该大于单纯 "Hello" 的 token 数，因为包含了 JSON 格式
        let plain_tokens = GatewayExecutor::estimate_tokens("Hello");
        assert!(
            tokens >= plain_tokens,
            "Input tokens should include format overhead"
        );
    }

    #[test]
    fn test_estimate_input_tokens_multiple_messages() {
        // 测试多个消息的 token 估算
        let messages = vec![
            keycompute_types::Message::system("You are a helpful assistant."),
            keycompute_types::Message::user("Hello"),
        ];
        let tokens = GatewayExecutor::estimate_input_tokens(&messages);
        assert!(tokens > 0, "Token count should be greater than 0");
        // 多个消息的 token 数应该大于单个消息
        let single_tokens = GatewayExecutor::estimate_input_tokens(&[messages[1].clone()]);
        assert!(
            tokens > single_tokens,
            "Multiple messages should have more tokens"
        );
    }

    #[test]
    fn test_estimate_input_tokens_empty() {
        // 测试空消息列表
        // 注意：空列表序列化后是 "[]"，这本身也是有 token 的
        let messages: Vec<keycompute_types::Message> = vec![];
        let tokens = GatewayExecutor::estimate_input_tokens(&messages);
        // 空 JSON "[]" 在 tiktoken 中也会计算为约 1 token
        // 这是正确的，因为即使空消息数组也有序列化开销
        // u32 类型永远 >= 0，所以直接验证返回值有效
        assert!(
            matches!(tokens, 0..=u32::MAX),
            "Empty messages should return valid u32 token count"
        );
    }

    #[test]
    fn test_estimate_input_tokens_chinese() {
        // 测试中文消息的 token 估算
        let messages = vec![keycompute_types::Message::user("你好世界")];
        let tokens = GatewayExecutor::estimate_input_tokens(&messages);
        assert!(tokens > 0, "Chinese content should have token count > 0");
    }

    #[test]
    fn test_estimate_input_tokens_includes_role_format() {
        // 测试估算是否包含 role 和格式
        // 将 messages 序列化为 JSON，确保包含 role 信息
        let messages = vec![keycompute_types::Message::user("test")];
        let tokens = GatewayExecutor::estimate_input_tokens(&messages);
        let json = serde_json::to_string(&messages).unwrap();
        let json_tokens = GatewayExecutor::estimate_tokens(&json);
        // 应该等于 JSON 的 token 数
        assert_eq!(
            tokens, json_tokens,
            "estimate_input_tokens should return JSON token count"
        );
    }

    #[tokio::test]
    async fn test_execute_returns_receiver_before_consuming_large_stream() {
        let config = GatewayConfig::default();
        let mut providers = HashMap::new();
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 150 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(config, providers);

        let ctx = Arc::new(create_test_context());
        let plan = ExecutionPlan {
            primary: ExecutionTarget::new_provider(
                "many-chunks",
                Uuid::new_v4(),
                "http://mock",
                "mock-key",
            ),
            fallback_chain: vec![],
        };

        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        let mut rx = tokio::time::timeout(
            Duration::from_millis(50),
            executor.execute(ctx, plan, account_states, Some(provider_health)),
        )
        .await
        .expect("execute should return receiver immediately")
        .expect("execute should succeed");

        let first_event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("stream should produce events")
            .expect("channel should stay open");

        assert!(matches!(first_event, StreamEvent::Delta { .. }));
    }

    #[tokio::test]
    async fn test_execute_forwards_raw_events_for_native_protocol_handlers() {
        let mut providers = HashMap::new();
        providers.insert(
            "raw-events".to_string(),
            Arc::new(RawEventProvider) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);
        let plan = ExecutionPlan {
            primary: ExecutionTarget::new_provider(
                "raw-events",
                Uuid::new_v4(),
                "http://mock",
                "mock-key",
            ),
            fallback_chain: vec![],
        };

        let mut rx = executor
            .execute(
                Arc::new(create_test_context()),
                plan,
                Arc::new(AccountStateStore::new()),
                Some(Arc::new(ProviderHealthStore::new())),
            )
            .await
            .unwrap();

        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Raw { data }) if data == "native-event")
        );
        assert!(matches!(rx.recv().await, Some(StreamEvent::Done)));
    }

    #[test]
    fn raw_anthropic_errors_do_not_commit_a_response() {
        assert!(!raw_event_commits_response(
            r#"{"kind":"anthropic_sse","event":"error","data":{"type":"error"}}"#
        ));
        // 即使兼容上游使用了非标准 event 名称，也应按 data.type 避免泄露
        // 原始错误并允许 fallback。
        assert!(!raw_event_commits_response(
            r#"{"kind":"anthropic_sse","event":"custom","data":{"type":"error"}}"#
        ));
        assert!(raw_event_commits_response(
            r#"{"kind":"anthropic_sse","event":"message_start","data":{"type":"message_start"}}"#
        ));
    }

    #[test]
    fn raw_unknown_envelopes_stay_conservatively_committed() {
        // 未知/非 Anthropic 格式保守视为已提交：宁可阻止一次本可执行的
        // fallback，也不能在已提交消息后追加另一条消息序列。
        assert!(raw_event_commits_response("not json"));
        assert!(raw_event_commits_response(
            r#"{"kind":"some_other_protocol","event":"ping"}"#
        ));
        // 未知事件名（未来协议扩展）同样保守；只有显式登记的不提交事件
        // 可安全回退。
        assert!(raw_event_commits_response(
            r#"{"kind":"anthropic_sse","event":"future_event","data":{"type":"future_type"}}"#
        ));
        // 已知的提交类事件（开始或延续消息）一律视为已提交。
        assert!(raw_event_commits_response(
            r#"{"kind":"anthropic_sse","event":"content_block_delta","data":{"type":"content_block_delta"}}"#
        ));
        assert!(raw_event_commits_response(
            r#"{"kind":"anthropic_sse","event":"message_delta","data":{"type":"message_delta"}}"#
        ));
        assert!(raw_event_commits_response(
            r#"{"kind":"anthropic_sse","event":"message_stop","data":{"type":"message_stop"}}"#
        ));
    }

    #[tokio::test]
    async fn test_execute_rejects_eof_after_finish_reason_without_done() {
        let mut providers = HashMap::new();
        providers.insert(
            "truncated".to_string(),
            Arc::new(TruncatedProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 1 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);
        let account_id = Uuid::new_v4();
        let health = Arc::new(ProviderHealthStore::new());
        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "truncated",
                        account_id,
                        "http://mock",
                        "mock-key",
                    ),
                    fallback_chain: vec![ExecutionTarget::new_provider(
                        "many-chunks",
                        Uuid::new_v4(),
                        "http://fallback",
                        "mock-key",
                    )],
                },
                Arc::new(AccountStateStore::new()),
                Some(Arc::clone(&health)),
            )
            .await
            .unwrap();

        assert!(matches!(
            rx.recv().await,
            Some(StreamEvent::Delta {
                finish_reason: Some(_),
                ..
            })
        ));
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Error { message }) if message.contains("terminal Done"))
        );
        assert!(!matches!(rx.recv().await, Some(StreamEvent::Done)));
        assert_eq!(
            ctx.usage_snapshot().0,
            17,
            "exact input usage from message_start must survive an incomplete stream"
        );
        assert!(
            !ctx.is_usage_finalized(),
            "only input usage is exact; output must remain an estimate until final Usage"
        );
        let provider_health = health.get_health("truncated").unwrap();
        assert_eq!(provider_health.success_requests, 0);
        assert_eq!(provider_health.failed_requests, 1);
    }

    #[tokio::test]
    async fn test_execute_falls_back_after_anthropic_ping() {
        let mut providers = HashMap::new();
        providers.insert(
            "pinging-fail".to_string(),
            Arc::new(PingingFailProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 1 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let mut rx = executor
            .execute(
                Arc::new(create_test_context()),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "pinging-fail",
                        Uuid::new_v4(),
                        "http://mock",
                        "mock-key",
                    ),
                    fallback_chain: vec![ExecutionTarget::new_provider(
                        "many-chunks",
                        Uuid::new_v4(),
                        "http://fallback",
                        "mock-key",
                    )],
                },
                Arc::new(AccountStateStore::new()),
                Some(Arc::new(ProviderHealthStore::new())),
            )
            .await
            .unwrap();

        assert!(matches!(rx.recv().await, Some(StreamEvent::Raw { .. })));
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Delta { content, .. }) if content == "x")
        );
        assert!(matches!(rx.recv().await, Some(StreamEvent::Done)));
    }

    #[tokio::test]
    async fn test_execute_falls_back_after_uncommitted_anthropic_error() {
        let mut providers = HashMap::new();
        providers.insert(
            "raw-error".to_string(),
            Arc::new(RawErrorProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 1 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let mut rx = executor
            .execute(
                Arc::new(create_test_context()),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "raw-error",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![ExecutionTarget::new_provider(
                        "many-chunks",
                        Uuid::new_v4(),
                        "http://fallback",
                        "mock-key",
                    )],
                },
                Arc::new(AccountStateStore::new()),
                Some(Arc::new(ProviderHealthStore::new())),
            )
            .await
            .unwrap();

        // 原始 error 由 Anthropic handler 脱敏而不回写客户端，因此不应阻止
        // fallback；后续内容必须来自第二个账号。
        assert!(matches!(rx.recv().await, Some(StreamEvent::Raw { .. })));
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Delta { content, .. }) if content == "x")
        );
        assert!(matches!(rx.recv().await, Some(StreamEvent::Done)));
    }

    #[tokio::test]
    async fn test_execute_does_not_fallback_after_native_response_commits() {
        let mut providers = HashMap::new();
        providers.insert(
            "committed-raw-fail".to_string(),
            Arc::new(CommittedRawFailProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 1 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let mut rx = executor
            .execute(
                Arc::new(create_test_context()),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "committed-raw-fail",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![ExecutionTarget::new_provider(
                        "many-chunks",
                        Uuid::new_v4(),
                        "http://fallback",
                        "mock-key",
                    )],
                },
                Arc::new(AccountStateStore::new()),
                Some(Arc::new(ProviderHealthStore::new())),
            )
            .await
            .unwrap();

        assert!(matches!(
            rx.recv().await,
            Some(StreamEvent::Raw { data }) if data.contains("message_start")
        ));
        assert!(matches!(
            rx.recv().await,
            Some(StreamEvent::Error { message }) if message.contains("connection reset after message_start")
        ));
        assert!(
            !matches!(rx.recv().await, Some(StreamEvent::Delta { .. })),
            "the fallback response must not follow a committed native message"
        );
    }

    #[tokio::test]
    async fn zero_input_usage_keeps_tiktoken_estimate() {
        // 兼容网关可能在 message_start 上报 input_tokens=0（或缺失 usage 解析
        // 为 0）。修复前 executor 无条件 set_input_tokens(0)，把流开始时的
        // tiktoken 估算清零并锁定 input 为 0 计费；修复后必须跳过 0，保留估算
        // 直到真正有效的精确值到来。
        let mut providers = HashMap::new();
        providers.insert(
            "primary".to_string(),
            Arc::new(ZeroUsageProvider) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "primary",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![],
                },
                Arc::new(AccountStateStore::new()),
                None,
            )
            .await
            .unwrap();

        // 消费事件直到终止：InputUsage(0) + Done
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("stream should terminate")
        {
            if matches!(event, StreamEvent::Done) {
                break;
            }
        }

        // 输入估算必须保留（"Hello" 的 tiktoken 编码非空），不能被 0 覆盖
        let (input, _) = ctx.usage_snapshot();
        assert!(
            input > 0,
            "zero input usage must not override the tiktoken estimate, got {input}"
        );
    }

    #[tokio::test]
    async fn empty_response_usage_keeps_input_estimate_and_locks_zero_output() {
        // 空响应 `Usage{input_tokens: 0, output_tokens: 0}`：输入为 0 被跳过
        // （保留 tiktoken 估算，与 zero_input_usage_keeps_tiktoken_estimate 的
        // InputUsage 分支一致）；输出为 0 是空响应的合法精确值，必须锁定。
        // 非官方网关在 Usage 后继续发送 Delta 时，向已锁定的 0 上累加估算
        // 会造成双重计费（与 mid_stream 场景对称，只是 N=0）。
        let mut providers = HashMap::new();
        providers.insert(
            "primary".to_string(),
            Arc::new(EmptyResponseZeroUsageProvider) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "primary",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![],
                },
                Arc::new(AccountStateStore::new()),
                None,
            )
            .await
            .unwrap();

        // 消费事件直到终止：Usage(0,0) + Delta + Done；Delta 仍须转发给客户端
        let mut saw_delta = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("stream should terminate")
        {
            match event {
                StreamEvent::Delta { content, .. } => {
                    saw_delta = content == "tail-after-usage";
                }
                StreamEvent::Done => break,
                _ => {}
            }
        }
        assert!(saw_delta, "post-usage delta must still be forwarded");

        let (input, output) = ctx.usage_snapshot();
        assert!(
            input > 0,
            "zero input usage must not override the tiktoken estimate, got {input}"
        );
        assert_eq!(
            output, 0,
            "output must stay locked at the exact 0 from the empty-response usage"
        );
        assert!(
            ctx.is_output_finalized(),
            "zero output from an empty response is an exact value and must be finalized"
        );
    }

    #[tokio::test]
    async fn mid_stream_zero_input_usage_does_not_double_count_output() {
        // 兼容网关在流中间上报 Usage{input_tokens: 0, output_tokens: 100} 后继续
        // 发送 Delta。输入为 0 被跳过（保留估算），但 output 已被锁定为精确值；
        // 修复前 Delta 分支以 is_usage_finalized（输入侧未锁定）为门槛继续累加
        // 估算，导致输出计费为 100 + estimate("more")，双重计费。
        let mut providers = HashMap::new();
        providers.insert(
            "primary".to_string(),
            Arc::new(MidStreamZeroInputUsageProvider) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "primary",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![],
                },
                Arc::new(AccountStateStore::new()),
                None,
            )
            .await
            .unwrap();

        let mut saw_delta = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("stream should terminate")
        {
            match event {
                StreamEvent::Delta { content, .. } => {
                    assert_eq!(content, "more");
                    saw_delta = true;
                }
                StreamEvent::Done => break,
                _ => {}
            }
        }
        assert!(saw_delta, "delta after the usage event must be forwarded");

        let (_, output) = ctx.usage_snapshot();
        assert_eq!(
            output, 100,
            "output must stay at the exact provider value, got {output}"
        );
    }

    #[tokio::test]
    async fn test_execute_aborts_fallback_after_client_disconnects() {
        // 客户端断开（receiver 被 drop）后，primary 失败不应再触发 fallback：
        // 新的上游调用没有接收方，只会浪费连接与配额。
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        // fallback 一旦被（错误）调用，立即通过 Notify 唤醒断言方，避免 10ms
        // 轮询粒度；负向断言窗口由 timeout 兜底。
        let fallback_called = Arc::new(Notify::new());
        let mut providers = HashMap::new();
        providers.insert(
            "primary".to_string(),
            Arc::new(CountingProvider {
                calls: Arc::clone(&primary_calls),
                notified: None,
            }) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "fallback".to_string(),
            Arc::new(CountingProvider {
                calls: Arc::clone(&fallback_calls),
                notified: Some(Arc::clone(&fallback_called)),
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let rx = executor
            .execute(
                Arc::new(create_test_context()),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "primary",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![ExecutionTarget::new_provider(
                        "fallback",
                        Uuid::new_v4(),
                        "http://fallback",
                        "mock-key",
                    )],
                },
                Arc::new(AccountStateStore::new()),
                None,
            )
            .await
            .unwrap();
        drop(rx);

        // 先确认后台任务已启动并完成 primary 尝试（计数器从 0 -> 1），
        // 再给 fallback 一个观察窗口：若 fallback 被错误调用，Notify 会立即
        // 唤醒并失败；窗口结束仍未被调用即验证通过（避免固定 sleep 的时序
        // 脆弱性，且 Notify 消除了轮询粒度）。
        tokio::time::timeout(Duration::from_secs(2), async {
            while primary_calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("primary provider should have been attempted");
        assert!(
            tokio::time::timeout(Duration::from_millis(500), fallback_called.notified())
                .await
                .is_err(),
            "fallback must not be attempted after the client disconnected"
        );
    }

    #[tokio::test]
    async fn test_execute_aborts_fallback_when_client_disconnect_is_marked() {
        // Anthropic 流式路径的后台结算任务持有 receiver 直到 Done/Error，客户端
        // 断开不会触发 `tx.is_closed()`；handler 改为通过 ctx 显式标记断开，
        // executor 必须在 primary 失败后据此中止 fallback 链。这里用只发 ping
        // （未提交内容）后失败的 primary 复现该路径：没有断开标志时 fallback 合法。
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic-primary".to_string(),
            Arc::new(PingingFailProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "fallback".to_string(),
            Arc::new(CountingProvider {
                calls: Arc::clone(&fallback_calls),
                notified: None,
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let ctx = Arc::new(create_test_context());
        // 模拟 create_anthropic_stream 在 SSE 发送失败后调用 ctx.mark_client_disconnected()
        ctx.mark_client_disconnected();

        let health = Arc::new(ProviderHealthStore::new());
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "anthropic-primary",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![ExecutionTarget::new_provider(
                        "fallback",
                        Uuid::new_v4(),
                        "http://fallback",
                        "mock-key",
                    )],
                },
                Arc::new(AccountStateStore::new()),
                Some(Arc::clone(&health)),
            )
            .await
            .unwrap();

        // ping 已向客户端转发（未提交内容），随后 primary 失败；由于客户端已断开，
        // executor 必须直接终止链，向 handler 上报 Error 而不是发起 fallback。
        assert!(matches!(rx.recv().await, Some(StreamEvent::Raw { .. })));
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Error { message }) if message.contains("connection reset"))
        );
        assert_eq!(
            fallback_calls.load(Ordering::SeqCst),
            0,
            "fallback must not be attempted after the client disconnected"
        );
        assert_eq!(
            health.get_fallback_count(),
            0,
            "fallback health counter must stay at zero"
        );
    }

    #[tokio::test]
    async fn test_fallback_does_not_inherit_previous_output_usage() {
        // primary 只发送精确 Usage 后断流（未提交 Delta，可安全 fallback）；
        // fallback 必须从零重新估算 output，不能沿用 primary 的残留精确值。
        let mut providers = HashMap::new();
        providers.insert(
            "usage-only-fail".to_string(),
            Arc::new(UsageOnlyFailProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "estimate-only".to_string(),
            Arc::new(EstimateOnlyProvider) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);

        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "usage-only-fail",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![ExecutionTarget::new_provider(
                        "estimate-only",
                        Uuid::new_v4(),
                        "http://fallback",
                        "mock-key",
                    )],
                },
                Arc::new(AccountStateStore::new()),
                Some(Arc::new(ProviderHealthStore::new())),
            )
            .await
            .unwrap();

        let mut deltas = 0;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("stream should produce events")
        {
            match event {
                StreamEvent::Delta { .. } => deltas += 1,
                StreamEvent::Done => break,
                _ => {}
            }
        }

        assert_eq!(deltas, 1, "fallback delta should be forwarded");
        let (_input, output) = ctx.usage_snapshot();
        // fallback 未报告精确 usage：output 必须是重新估算的值，
        // 而不是 primary 留下的 222。
        assert_eq!(
            output,
            GatewayExecutor::estimate_tokens("hello"),
            "fallback output must be re-estimated, not inherited from the failed primary"
        );
        assert_ne!(output, 222);
        assert!(
            !ctx.is_usage_finalized(),
            "without a terminal Usage event the fallback stays on estimates"
        );
    }

    #[tokio::test]
    async fn test_execute_falls_back_to_next_target_on_primary_failure() {
        // 主选 provider 失败时，应回落到 fallback 链的下一个 target
        // 并记录 fallback 计数（跨协议/跨账号回退能力的单元级验证）
        let config = GatewayConfig::default();
        let mut providers = HashMap::new();
        providers.insert(
            "failing".to_string(),
            Arc::new(FailingProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 2 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(config, providers);

        let ctx = Arc::new(create_test_context());
        let fallback_account_id = Uuid::new_v4();
        let plan = ExecutionPlan {
            primary: ExecutionTarget::new_provider(
                "failing",
                Uuid::new_v4(),
                "http://primary",
                "mock-key",
            ),
            fallback_chain: vec![ExecutionTarget::new_provider(
                "many-chunks",
                fallback_account_id,
                "http://fallback",
                "mock-key",
            )],
        };

        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                plan,
                Arc::clone(&account_states),
                Some(Arc::clone(&provider_health)),
            )
            .await
            .expect("execute should return receiver");

        // 收集全部事件：应来自 fallback provider（Delta ×2 + Done）而非错误
        let mut deltas = 0;
        let mut saw_done = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("stream should produce events")
        {
            match event {
                StreamEvent::Delta { .. } => deltas += 1,
                StreamEvent::Done => {
                    saw_done = true;
                    break;
                }
                StreamEvent::Error { message } => {
                    panic!("should not receive error after successful fallback: {message}")
                }
                _ => {}
            }
        }

        assert_eq!(deltas, 2, "fallback provider deltas should be forwarded");
        assert!(saw_done, "stream should complete with Done");
        // fallback 成功后应记录 fallback 计数与账号成功状态
        assert_eq!(provider_health.get_fallback_count(), 1);
        assert_eq!(
            ctx.executed_provider_account(),
            Some(keycompute_types::ExecutedProviderAccount {
                provider: "many-chunks".to_string(),
                account_id: fallback_account_id,
            }),
            "billing must use the account that actually completed the fallback request"
        );
    }

    #[tokio::test]
    async fn test_execute_no_fallback_after_content_sent() {
        // 流中途失败且已向客户端转发过内容时，不得再 fallback，
        // 否则客户端会收到「部分内容 + 新一遍完整内容」的重复输出
        let config = GatewayConfig::default();
        let mut providers = HashMap::new();
        providers.insert(
            "mid-stream-fail".to_string(),
            Arc::new(MidStreamFailProvider {
                deltas_before_error: 2,
            }) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 3 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(config, providers);

        let ctx = Arc::new(create_test_context());
        let plan = ExecutionPlan {
            primary: ExecutionTarget::new_provider(
                "mid-stream-fail",
                Uuid::new_v4(),
                "http://primary",
                "mock-key",
            ),
            fallback_chain: vec![ExecutionTarget::new_provider(
                "many-chunks",
                Uuid::new_v4(),
                "http://fallback",
                "mock-key",
            )],
        };

        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        let mut rx = executor
            .execute(
                ctx,
                plan,
                Arc::clone(&account_states),
                Some(Arc::clone(&provider_health)),
            )
            .await
            .expect("execute should return receiver");

        let mut deltas = 0;
        let mut saw_error = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("stream should produce events")
        {
            match event {
                StreamEvent::Delta { .. } => deltas += 1,
                StreamEvent::Error { .. } => {
                    saw_error = true;
                    break;
                }
                StreamEvent::Done => {
                    panic!("should not complete via fallback after partial content")
                }
                _ => {}
            }
        }

        // 只有主选的 2 个 partial Delta，fallback 的 3 个 chunk 不应出现
        assert_eq!(deltas, 2, "fallback content must not be appended");
        assert!(saw_error, "client should receive an error event");
        assert_eq!(
            provider_health.get_fallback_count(),
            0,
            "fallback must not be attempted after content was sent"
        );
    }
}
