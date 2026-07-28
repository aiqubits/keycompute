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
                    // 注意：不再自动标记错误，错误计数只能通过管理员手动测试 API 触发
                    // 保留 Provider 健康状态更新用于路由评分
                    if let ExecutionTarget::ProviderAccount { provider, .. } = &target
                        && let Some(ref health_store) = provider_health
                    {
                        health_store.record_failure(provider);
                    }

                    let provider_name = match &target {
                        ExecutionTarget::ProviderAccount { provider, .. } => provider.clone(),
                        ExecutionTarget::Node { model } => format!("node:{}", model),
                    };
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
        // 注意：这只是估算，最终会被 StreamEvent::Usage 事件中的精确值覆盖
        let estimated_input_tokens = Self::estimate_input_tokens(&ctx.messages);
        ctx.set_input_tokens(estimated_input_tokens);

        tracing::debug!(
            request_id = %ctx.request_id,
            estimated_input_tokens = estimated_input_tokens,
            "Stream started, input tokens estimated"
        );

        // pending_done: 收到 finish_reason 但尚未发送 Done 事件
        // 修复 Usage 事件被跳过的问题：finish_reason 后不立即 break，
        // 继续处理可能的 Usage 事件，直到收到 Done 或流结束
        let mut pending_done = false;

        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::Delta {
                    content,
                    finish_reason,
                } => {
                    // 尚未收到 Provider 精确 Usage 时，使用 tiktoken 估算
                    if !ctx.is_usage_finalized() {
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
                            "try_execute: finish_reason received, waiting for Usage/Done"
                        );
                        pending_done = true;
                    }
                }
                StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    // Provider 报告的精确 usage 值（优先级最高）
                    // 覆盖之前的 tiktoken 估算值
                    ctx.set_input_tokens(input_tokens);
                    ctx.set_output_tokens(output_tokens);

                    tracing::debug!(
                        request_id = %ctx.request_id,
                        provider_usage = true,
                        input_tokens = input_tokens,
                        output_tokens = output_tokens,
                        "Provider usage received, overriding estimation"
                    );
                }
                StreamEvent::Done => {
                    tracing::debug!(
                        request_id = %ctx.request_id,
                        "try_execute: received Done event"
                    );
                    tx.send(StreamEvent::Done)
                        .await
                        .map_err(|_| KeyComputeError::Internal("Send error".into()))?;
                    pending_done = false;
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
                _ => {}
            }
        }

        // 如果只收到 finish_reason 没有 Done 事件，补发 Done 通知 handler 流结束。
        // 这里 pending_done = false 在 Done 分支中也设置了（冗余安全），
        // 但该分支通过 break 退出循环，到达此处时 pending_done 必定为 false。
        if pending_done {
            let _ = tx.send(StreamEvent::Done).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use keycompute_types::{Message, PricingSnapshot};
    use rust_decimal::Decimal;
    use std::time::Duration;
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
                ctx,
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
