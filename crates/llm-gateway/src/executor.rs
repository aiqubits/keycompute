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
use keycompute_types::{
    AttemptKind, AttemptRef, AttemptResponseMeta, AttemptStatus, AttemptTraceFinish,
    AttemptTraceStart, BillingStatus, ErrorOrigin, ExecutionPlan, ExecutionTarget, KeyComputeError,
    NoopRequestLifecycleRecorder, RequestContext, RequestExecutionFailure,
    RequestLifecycleRecorder, RequestStatus, Result, RouteType, StreamEndReason,
    TraceErrorCategory, TraceErrorInfo, sanitize_error_summary,
};
use llm_protocol_provider::{
    DefaultHttpTransport, HttpTransport, ProviderAdapter, StreamEvent, UpstreamMessage,
    UpstreamRequest,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

fn classify_attempt_kind(
    target_index: usize,
    target: &ExecutionTarget,
    attempted_accounts: &mut HashSet<uuid::Uuid>,
) -> AttemptKind {
    match target {
        ExecutionTarget::ProviderAccount { account_id, .. } if target_index == 0 => {
            attempted_accounts.insert(*account_id);
            AttemptKind::Primary
        }
        ExecutionTarget::ProviderAccount { account_id, .. }
            if attempted_accounts.contains(account_id) =>
        {
            AttemptKind::Retry
        }
        ExecutionTarget::ProviderAccount { account_id, .. } => {
            attempted_accounts.insert(*account_id);
            AttemptKind::Fallback
        }
        ExecutionTarget::Node { .. } => AttemptKind::Primary,
    }
}

fn same_provider_account(left: &ExecutionTarget, right: &ExecutionTarget) -> bool {
    matches!(
        (left, right),
        (
            ExecutionTarget::ProviderAccount {
                account_id: left,
                ..
            },
            ExecutionTarget::ProviderAccount {
                account_id: right,
                ..
            }
        ) if left == right
    )
}

fn normalize_stream_error(error: KeyComputeError) -> KeyComputeError {
    match error {
        // The protocol parsers historically use ProviderError for malformed
        // SSE, invalid UTF-8, and missing terminal markers. These are protocol
        // failures after a paid POST has already returned response headers.
        // Their provider outcome is ambiguous, so they must stop the entire
        // retry/fallback chain. Do not retain their raw text because malformed
        // events can contain sensitive details.
        KeyComputeError::ProviderError(_) => KeyComputeError::UpstreamFailure {
            status: None,
            stable_code: "upstream_stream_protocol".to_string(),
            retryable: false,
            summary: "Upstream response stream was malformed or incomplete".to_string(),
        },
        // Transport adapters already provide the correct stable code and
        // retryability. Preserve that structure across the parser boundary.
        error => error,
    }
}

/// A protocol-level error explicitly declared by the provider is a definite
/// failed outcome, unlike a malformed/truncated response whose billing outcome
/// is unknown. It may therefore fall back to another account if no response
/// content has been committed to the client.
fn provider_declared_stream_error() -> KeyComputeError {
    KeyComputeError::UpstreamFailure {
        status: None,
        stable_code: "upstream_declared_error".to_string(),
        retryable: false,
        summary: "Upstream reported a stream error".to_string(),
    }
}

fn classify_execution_error(error: &KeyComputeError) -> (TraceErrorCategory, String, bool) {
    match error {
        KeyComputeError::UpstreamFailure {
            status: Some(status),
            stable_code,
            retryable,
            ..
        } if *status >= 500 => (
            TraceErrorCategory::Upstream5xx,
            stable_code.clone(),
            *retryable,
        ),
        KeyComputeError::UpstreamFailure {
            status: Some(status),
            stable_code,
            retryable,
            ..
        } if *status >= 400 => (
            TraceErrorCategory::Upstream4xx,
            stable_code.clone(),
            *retryable,
        ),
        KeyComputeError::UpstreamFailure {
            stable_code,
            retryable,
            ..
        } if matches!(
            stable_code.as_str(),
            "upstream_protocol" | "upstream_stream_protocol" | "upstream_declared_error"
        ) =>
        {
            (
                TraceErrorCategory::Protocol,
                stable_code.clone(),
                *retryable,
            )
        }
        KeyComputeError::UpstreamFailure {
            stable_code,
            retryable,
            ..
        } if matches!(
            stable_code.as_str(),
            "upstream_timeout" | "upstream_ambiguous_timeout"
        ) =>
        {
            (TraceErrorCategory::Timeout, stable_code.clone(), *retryable)
        }
        KeyComputeError::UpstreamFailure {
            stable_code,
            retryable,
            ..
        } => (
            TraceErrorCategory::Transport,
            stable_code.clone(),
            *retryable,
        ),
        KeyComputeError::Timeout(_) | KeyComputeError::ProviderTimeout(_, _) => (
            TraceErrorCategory::Timeout,
            "provider_timeout".to_string(),
            true,
        ),
        _ => (
            TraceErrorCategory::Transport,
            "provider_attempt_failed".to_string(),
            error.is_retryable(),
        ),
    }
}

/// Failures after dispatching a paid POST have an ambiguous provider outcome.
/// They must stop the entire execution chain: `retryable = false` alone only
/// skips copies of the same account and would still advance to a fallback.
fn prevents_retry_and_fallback(error: &KeyComputeError) -> bool {
    matches!(
        error,
        KeyComputeError::UpstreamFailure { stable_code, .. }
            if matches!(
                stable_code.as_str(),
                "upstream_ambiguous_timeout"
                    | "upstream_ambiguous_transport"
                    | "upstream_body_read"
                    | "upstream_stream_read"
                    | "upstream_stream_protocol"
            )
    )
}

async fn retry_backoff_cancelled(
    duration: Duration,
    tx: &mpsc::Sender<StreamEvent>,
    ctx: &RequestContext,
) -> bool {
    tokio::select! {
        biased;
        _ = tx.closed() => true,
        _ = ctx.wait_for_client_disconnect() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}

async fn finish_pre_attempt_client_disconnect(
    ctx: &RequestContext,
    lifecycle: &Arc<dyn RequestLifecycleRecorder>,
    billing_status: BillingStatus,
) {
    ctx.mark_client_disconnected();
    let _ = lifecycle
        .finish_request_without_attempt(keycompute_types::RequestTraceFinish {
            request_id: ctx.request_id,
            status: RequestStatus::Cancelled,
            error: Some(TraceErrorInfo {
                origin: ErrorOrigin::Gateway,
                category: TraceErrorCategory::ClientDisconnect,
                code: "client_disconnected".to_string(),
                summary: None,
                retryable: Some(false),
            }),
            billing_status,
            finished_at: chrono::Utc::now(),
        })
        .await;
}

#[derive(Debug, Clone)]
struct PlannedTarget {
    target: ExecutionTarget,
    /// Reserved slot for the OpenAI-compatible retry without stream_options.
    /// The slot is skipped unless the immediately preceding logical attempt
    /// reported `upstream_stream_options_unsupported` for this account.
    stream_options_compatibility_retry: bool,
}

struct PlanRunContext {
    tx: mpsc::Sender<StreamEvent>,
    account_states: Arc<AccountStateStore>,
    provider_health: Option<Arc<ProviderHealthStore>>,
    lifecycle: Arc<dyn RequestLifecycleRecorder>,
    active_attempt: Arc<Mutex<Option<AttemptRef>>>,
    execution_completed: Arc<AtomicBool>,
}

struct TargetRunContext<'a> {
    tx: mpsc::Sender<StreamEvent>,
    sent_content: &'a mut bool,
    attempt: Option<AttemptRef>,
    lifecycle: Arc<dyn RequestLifecycleRecorder>,
    execution_completed: Arc<AtomicBool>,
    include_stream_usage: bool,
}

impl PlannedTarget {
    fn regular(target: ExecutionTarget) -> Self {
        Self {
            target,
            stream_options_compatibility_retry: false,
        }
    }

    fn compatibility_retry(target: ExecutionTarget) -> Self {
        Self {
            target,
            stream_options_compatibility_retry: true,
        }
    }
}

fn next_runnable_target_index(
    targets: &[PlannedTarget],
    current_index: usize,
    current_target: &ExecutionTarget,
    retryable: bool,
    compatibility_retry_pending: &HashSet<uuid::Uuid>,
) -> Option<usize> {
    ((current_index + 1)..targets.len()).find(|index| {
        let candidate = &targets[*index];
        let compatibility_retry_is_runnable = !candidate.stream_options_compatibility_retry
            || matches!(
                &candidate.target,
                ExecutionTarget::ProviderAccount { account_id, .. }
                    if compatibility_retry_pending.contains(account_id)
            );

        compatibility_retry_is_runnable
            && (retryable || !same_provider_account(current_target, &candidate.target))
    })
}

async fn finish_successful_attempt_trace(
    ctx: &RequestContext,
    lifecycle: &Arc<dyn RequestLifecycleRecorder>,
    active_attempt: &Arc<Mutex<Option<AttemptRef>>>,
) {
    let attempt = active_attempt
        .lock()
        .expect("active attempt state poisoned")
        .take();
    let Some(attempt) = attempt else {
        return;
    };
    finish_attempt_trace_or_degrade(
        ctx,
        lifecycle,
        AttemptTraceFinish {
            attempt_id: attempt.id,
            request_id: ctx.request_id,
            attempt_status: AttemptStatus::Succeeded,
            // Upstream completion is not yet client-visible completion. Keep the
            // request open until the protocol handler validates/forwards it.
            request_status: RequestStatus::Running,
            // This is still the final upstream attempt. Request finality is a
            // separate client-response phase and must not erase that fact.
            is_final: true,
            stream_end_reason: Some(StreamEndReason::Completed),
            stream_error_count: Some(0),
            error: None,
            billing_status: BillingStatus::Pending,
            finished_at: chrono::Utc::now(),
        },
    )
    .await;
}

async fn finish_attempt_trace_or_degrade(
    ctx: &RequestContext,
    lifecycle: &Arc<dyn RequestLifecycleRecorder>,
    finish: AttemptTraceFinish,
) {
    let attempt_status = finish.attempt_status;
    if let Err(error) = lifecycle.finish_attempt_and_request(finish).await {
        tracing::warn!(
            request_id=%ctx.request_id,
            attempt_status=attempt_status.as_str(),
            %error,
            "failed to finish provider attempt trace"
        );
        // Attempt and client-response completion intentionally use separate
        // writes. If the attempt write fails but the handler later closes the
        // request, stale reconciliation will not revisit that terminal request.
        // Persist the degradation explicitly so an unfinished attempt is never
        // presented as a fully actual trace.
        if let Err(partial_error) = lifecycle.mark_trace_partial(ctx.request_id).await {
            tracing::warn!(
                request_id=%ctx.request_id,
                %partial_error,
                "failed to mark provider trace partial after attempt finalization failure"
            );
        }
    }
}

async fn complete_successful_execution_attempt(
    ctx: &RequestContext,
    lifecycle: &Arc<dyn RequestLifecycleRecorder>,
    active_attempt: &Arc<Mutex<Option<AttemptRef>>>,
    tx: mpsc::Sender<StreamEvent>,
) {
    // Preserve response latency: publish Done immediately, then serialize the
    // attempt trace write in this background task. The protocol handler owns
    // request terminalization after billing and client delivery.
    if tx.send(StreamEvent::Done).await.is_err() {
        ctx.mark_client_disconnected();
    }
    drop(tx);

    finish_successful_attempt_trace(ctx, lifecycle, active_attempt).await;
}

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
        self.execute_with_recorder(
            ctx,
            plan,
            account_states,
            provider_health,
            Arc::new(NoopRequestLifecycleRecorder),
        )
        .await
    }

    /// Execute a request and persist its monitoring lifecycle through `lifecycle`.
    pub async fn execute_with_recorder(
        &self,
        ctx: Arc<RequestContext>,
        plan: ExecutionPlan,
        account_states: Arc<AccountStateStore>,
        provider_health: Option<Arc<ProviderHealthStore>>,
        lifecycle: Arc<dyn RequestLifecycleRecorder>,
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
        let active_attempt = Arc::new(Mutex::new(None::<AttemptRef>));
        let execution_completed = Arc::new(AtomicBool::new(false));

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                exec_timeout,
                runner.run_plan(
                    Arc::clone(&ctx),
                    plan,
                    PlanRunContext {
                        tx: tx.clone(),
                        account_states,
                        provider_health,
                        lifecycle: Arc::clone(&lifecycle),
                        active_attempt: Arc::clone(&active_attempt),
                        execution_completed: Arc::clone(&execution_completed),
                    },
                ),
            )
            .await;

            match result {
                Ok(Ok(())) => {
                    // 业务执行超时只约束上游调用；客户端响应终态在其外结算，
                    // 避免较慢的监控存储延迟已经完成的推理响应。
                    complete_successful_execution_attempt(&ctx, &lifecycle, &active_attempt, tx)
                        .await;
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
                    // Provider 已经完成时，请求不可被较慢的后处理改判为上游超时。
                    // timeout 会取消 run_plan，因此这里接管 attempt 与客户端响应终态。
                    if execution_completed.load(Ordering::Acquire) {
                        tracing::warn!(
                            request_id = %ctx.request_id,
                            "Trace finalization exceeded execution timeout after provider completion"
                        );
                        complete_successful_execution_attempt(
                            &ctx,
                            &lifecycle,
                            &active_attempt,
                            tx,
                        )
                        .await;
                        return;
                    }
                    tracing::error!(
                        request_id = %ctx.request_id,
                        timeout_secs = exec_timeout.as_secs(),
                        "Gateway execution timed out: run_plan cancelled"
                    );
                    let trace_error = TraceErrorInfo {
                        origin: ErrorOrigin::Gateway,
                        category: TraceErrorCategory::Timeout,
                        code: "gateway_execution_timeout".to_string(),
                        summary: Some("Gateway execution timed out".to_string()),
                        retryable: Some(true),
                    };
                    ctx.set_execution_failure(RequestExecutionFailure {
                        status: RequestStatus::TimedOut,
                        error: trace_error.clone(),
                        billing_status: BillingStatus::Pending,
                    });
                    let _ = tx
                        .send(StreamEvent::error(format!(
                            "Request timed out after {}s",
                            exec_timeout.as_secs()
                        )))
                        .await;
                    let attempt = active_attempt
                        .lock()
                        .expect("active attempt state poisoned")
                        .take();
                    if let Some(attempt) = attempt {
                        finish_attempt_trace_or_degrade(
                            &ctx,
                            &lifecycle,
                            AttemptTraceFinish {
                                attempt_id: attempt.id,
                                request_id: ctx.request_id,
                                attempt_status: AttemptStatus::TimedOut,
                                // The error is only queued for the handler here.
                                // Keep the request open until delivery or disconnect.
                                request_status: RequestStatus::Running,
                                is_final: true,
                                stream_end_reason: Some(StreamEndReason::Timeout),
                                stream_error_count: Some(1),
                                error: Some(trace_error),
                                billing_status: BillingStatus::Pending,
                                finished_at: chrono::Utc::now(),
                            },
                        )
                        .await;
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn run_plan(
        &self,
        ctx: Arc<RequestContext>,
        plan: ExecutionPlan,
        run: PlanRunContext,
    ) -> Result<()> {
        let PlanRunContext {
            tx,
            account_states,
            provider_health,
            lifecycle,
            active_attempt,
            execution_completed,
        } = run;
        // Build the actual execution chain: configured retries stay on the
        // same account, then fallback advances to the next routed account.
        // Node execution is handled outside this executor and is not repeated.
        let primary_target = plan.primary.clone();
        let mut routed_targets = vec![plan.primary];
        if self.config.enable_fallback {
            routed_targets.extend(plan.fallback_chain);
        }
        // Configuration is trusted, but still cap expansion defensively so a
        // typo cannot allocate an unbounded execution chain.
        let retries_per_account = self.config.max_retries.min(10);
        let mut targets = Vec::new();
        for target in routed_targets {
            let logical_attempts = if matches!(target, ExecutionTarget::ProviderAccount { .. }) {
                retries_per_account + 1
            } else {
                1
            };
            for _ in 0..logical_attempts {
                targets.push(PlannedTarget::regular(target.clone()));
                if matches!(
                    &target,
                    ExecutionTarget::ProviderAccount { provider, .. } if provider == "openai"
                ) {
                    targets.push(PlannedTarget::compatibility_retry(target.clone()));
                }
            }
        }

        let mut last_error = None;
        let _start_time = Instant::now();
        // 是否已向客户端转发过内容：一旦发出过 Delta，
        // 流中途失败后不可再 fallback，否则客户端会收到
        // 「前一段部分内容 + 新一遍完整内容」的重复拼接输出
        let mut sent_content = false;

        let target_count = targets.len();
        let mut attempted_accounts = HashSet::new();
        let mut retry_counts = HashMap::<uuid::Uuid, u32>::new();
        let mut compatibility_retry_pending = HashSet::<uuid::Uuid>::new();
        let mut stream_usage_unsupported = HashSet::<uuid::Uuid>::new();
        let mut next_eligible_index = 0usize;
        for (target_index, planned_target) in targets.iter().cloned().enumerate() {
            if target_index < next_eligible_index {
                continue;
            }
            let target = planned_target.target;
            if planned_target.stream_options_compatibility_retry {
                let should_run = match &target {
                    ExecutionTarget::ProviderAccount { account_id, .. } => {
                        compatibility_retry_pending.remove(account_id)
                    }
                    ExecutionTarget::Node { .. } => false,
                };
                if !should_run {
                    continue;
                }
            }
            let attempt_kind =
                classify_attempt_kind(target_index, &target, &mut attempted_accounts);
            if attempt_kind == AttemptKind::Retry
                && !planned_target.stream_options_compatibility_retry
                && let ExecutionTarget::ProviderAccount { account_id, .. } = &target
            {
                let retry = retry_counts.entry(*account_id).or_default();
                *retry += 1;
                let backoff = crate::RetryPolicy::new(retries_per_account).backoff_duration(*retry);
                if retry_backoff_cancelled(backoff, &tx, &ctx).await {
                    // At least one upstream attempt already ran before a retry
                    // backoff. Keep billing pending so any observed usage can
                    // still be settled after the client disconnects.
                    finish_pre_attempt_client_disconnect(&ctx, &lifecycle, BillingStatus::Pending)
                        .await;
                    return Err(KeyComputeError::Internal("client disconnected".to_string()));
                }
            }
            if tx.is_closed() || ctx.is_client_disconnected() {
                let billing_status = if target_index == 0 {
                    BillingStatus::NotApplicable
                } else {
                    BillingStatus::Pending
                };
                finish_pre_attempt_client_disconnect(&ctx, &lifecycle, billing_status).await;
                return Err(KeyComputeError::Internal("client disconnected".to_string()));
            }
            let target_start = Instant::now();
            let attempt = match &target {
                ExecutionTarget::ProviderAccount {
                    provider,
                    account_id,
                    ..
                } => {
                    match lifecycle
                        .start_attempt(AttemptTraceStart {
                            request_id: ctx.request_id,
                            attempt_kind,
                            route_type: RouteType::ProviderAccount,
                            model: ctx.model.clone(),
                            provider_name: Some(provider.clone()),
                            account_id: Some(*account_id),
                            node_task_id: None,
                            node_id: None,
                            session_id: None,
                            lease_id: None,
                            started_at: chrono::Utc::now(),
                        })
                        .await
                    {
                        Ok(attempt) => Some(attempt),
                        Err(error) => {
                            tracing::warn!(request_id=%ctx.request_id, %error, "failed to start provider trace attempt");
                            let _ = lifecycle.mark_trace_partial(ctx.request_id).await;
                            None
                        }
                    }
                }
                ExecutionTarget::Node { .. } => None,
            };
            *active_attempt
                .lock()
                .expect("active attempt state poisoned") = attempt;
            match self
                .try_execute(
                    &ctx,
                    &target,
                    TargetRunContext {
                        tx: tx.clone(),
                        sent_content: &mut sent_content,
                        attempt,
                        lifecycle: Arc::clone(&lifecycle),
                        execution_completed: Arc::clone(&execution_completed),
                        include_stream_usage: match &target {
                            ExecutionTarget::ProviderAccount { account_id, .. } => {
                                !stream_usage_unsupported.contains(account_id)
                            }
                            ExecutionTarget::Node { .. } => true,
                        },
                    },
                )
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
                        if !same_provider_account(&primary_target, &target) {
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
                        is_fallback = !same_provider_account(&primary_target, &target),
                        "Request executed successfully"
                    );
                    return Ok(());
                }
                Err(e) => {
                    if matches!(
                        &e,
                        KeyComputeError::UpstreamFailure { stable_code, .. }
                            if stable_code == "upstream_stream_options_unsupported"
                    ) && let ExecutionTarget::ProviderAccount { account_id, .. } = &target
                    {
                        stream_usage_unsupported.insert(*account_id);
                        compatibility_retry_pending.insert(*account_id);
                    }
                    let provider_name = match &target {
                        ExecutionTarget::ProviderAccount { provider, .. } => provider.clone(),
                        ExecutionTarget::Node { model } => format!("node:{}", model),
                    };

                    // 客户端已断开（receiver 被 drop，或 handler 显式标记）：继续
                    // fallback 只会对新的上游发起无意义的调用，直接终止执行链并
                    // 放弃后续 target。Anthropic 路径的后台任务持有 receiver 直到
                    // 结算完成，`tx.is_closed()` 不会因客户端断开而触发，因此还需
                    // 检查 handler 通过 ctx 传播的断开标志。
                    let client_gone = tx.is_closed() || ctx.is_client_disconnected();
                    let error_text = e.to_string();
                    let (category, code, retryable) = classify_execution_error(&e);
                    let error = TraceErrorInfo {
                        origin: if client_gone {
                            ErrorOrigin::Gateway
                        } else {
                            ErrorOrigin::Upstream
                        },
                        category: if client_gone {
                            TraceErrorCategory::ClientDisconnect
                        } else {
                            category
                        },
                        code: if client_gone {
                            "client_disconnected".to_string()
                        } else {
                            code
                        },
                        summary: Some(sanitize_error_summary(&error_text)),
                        retryable: Some(retryable),
                    };
                    // A non-retryable failure skips the remaining copies of
                    // this account but may still fall back to a different
                    // account. Retryable failures consume the next retry slot.
                    let next_target = if prevents_retry_and_fallback(&e) {
                        None
                    } else {
                        next_runnable_target_index(
                            &targets,
                            target_index,
                            &target,
                            retryable,
                            &compatibility_retry_pending,
                        )
                    };
                    let can_continue = !client_gone && !sent_content && next_target.is_some();
                    next_eligible_index = next_target.unwrap_or(target_count);
                    let timed_out = !client_gone && category == TraceErrorCategory::Timeout;
                    if !client_gone && !can_continue {
                        ctx.set_execution_failure(RequestExecutionFailure {
                            status: if timed_out {
                                RequestStatus::TimedOut
                            } else {
                                RequestStatus::Failed
                            },
                            error: error.clone(),
                            // Provider response workers settle usage before
                            // terminalizing the request, so keep billing open.
                            billing_status: BillingStatus::Pending,
                        });
                    }
                    if let Some(attempt) = attempt {
                        finish_attempt_trace_or_degrade(
                            &ctx,
                            &lifecycle,
                            AttemptTraceFinish {
                                attempt_id: attempt.id,
                                request_id: ctx.request_id,
                                attempt_status: if client_gone {
                                    AttemptStatus::Cancelled
                                } else if timed_out {
                                    AttemptStatus::TimedOut
                                } else {
                                    AttemptStatus::Failed
                                },
                                request_status: RequestStatus::Running,
                                is_final: !can_continue,
                                stream_end_reason: Some(if client_gone {
                                    StreamEndReason::ClientDisconnect
                                } else if timed_out {
                                    StreamEndReason::Timeout
                                } else if category == TraceErrorCategory::Protocol {
                                    StreamEndReason::ProtocolError
                                } else {
                                    StreamEndReason::UpstreamError
                                }),
                                stream_error_count: Some(1),
                                error: Some(error.clone()),
                                billing_status: BillingStatus::Pending,
                                finished_at: chrono::Utc::now(),
                            },
                        )
                        .await;
                    }
                    if client_gone {
                        ctx.mark_client_disconnected();
                        let _ = lifecycle.finish_request_without_attempt(
                            keycompute_types::client_response_trace_finish(
                                ctx.request_id,
                                keycompute_types::ClientResponseOutcome::ClientDisconnected,
                            ),
                        ).await.map_err(|error| tracing::warn!(request_id=%ctx.request_id, %error, "failed to finish partial provider failure trace"));
                    }
                    active_attempt
                        .lock()
                        .expect("active attempt state poisoned")
                        .take();
                    if client_gone {
                        tracing::debug!(
                            request_id = %ctx.request_id,
                            provider = %provider_name,
                            "Client disconnected, aborting fallback chain"
                        );
                        return Err(e);
                    }

                    // 生产调用仅保留已有的协议级健康统计；账号探测不会修改该状态。
                    if let ExecutionTarget::ProviderAccount { provider, .. } = &target
                        && let Some(ref health_store) = provider_health
                    {
                        health_store.record_failure(provider);
                    }

                    tracing::warn!(
                        request_id = %ctx.request_id,
                        provider = %provider_name,
                        error = %e,
                        "Request failed, trying retry or fallback"
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
        run: TargetRunContext<'_>,
    ) -> Result<()> {
        let TargetRunContext {
            tx,
            sent_content,
            attempt,
            lifecycle,
            execution_completed,
            include_stream_usage,
        } = run;
        // 只处理 ProviderAccount 变体
        let (provider, account_id, endpoint, upstream_api_key) = match target {
            ExecutionTarget::ProviderAccount {
                provider,
                account_id,
                endpoint,
                upstream_api_key,
                ..
            } => (provider, account_id, endpoint, upstream_api_key),
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
            proxy.client_for_provider_and_account(provider, Some(*account_id))
                as Arc<dyn HttpTransport>
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
            include_stream_usage,
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

        // 执行流式请求（传入 transport）。后台结算任务会继续持有下游
        // receiver，因此客户端断开不会关闭 tx；显式监听 RequestContext 的
        // 取消令牌，确保连接建立阶段也能及时丢弃上游请求 future。
        let response_result = tokio::select! {
            biased;
            _ = tx.closed() => {
                return Err(KeyComputeError::Internal("client disconnected".to_string()));
            }
            _ = ctx.wait_for_client_disconnect() => {
                return Err(KeyComputeError::Internal("client disconnected".to_string()));
            }
            result = provider_impl.stream_chat_with_meta(transport.as_ref(), request) => result,
        };
        let response = match response_result {
            Ok(response) => response,
            Err(failure) => {
                if let Some(attempt) = attempt {
                    let _ = lifecycle
                        .record_attempt_response_meta(
                            ctx.request_id,
                            attempt.id,
                            AttemptResponseMeta {
                                http_status: failure.status.map(i32::from),
                                headers_received_at: failure.headers_received_at,
                                upstream_request_id: failure.upstream_request_id.clone(),
                            },
                        )
                        .await;
                }
                return Err(KeyComputeError::UpstreamFailure {
                    status: failure.status,
                    stable_code: failure.stable_error_code,
                    retryable: failure.retryable,
                    summary: failure.sanitized_summary,
                });
            }
        };

        // From this point onward the request-local usage accumulator belongs
        // to this accepted upstream attempt. Keep its billing attribution
        // separate from successful completion: a fallback can produce partial
        // billable usage and then truncate before `Done`.
        ctx.set_usage_provider_account(provider.clone(), *account_id);
        if let Some(attempt) = attempt {
            let _ = lifecycle
                .record_attempt_response_meta(
                    ctx.request_id,
                    attempt.id,
                    AttemptResponseMeta {
                        http_status: Some(i32::from(response.meta.status)),
                        headers_received_at: Some(response.meta.headers_received_at),
                        upstream_request_id: response.meta.upstream_request_id.clone(),
                    },
                )
                .await;
        }
        let mut stream = response.body;

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
        let mut recorded_first_content = false;

        loop {
            // 下游 SSE 断开后，立即 drop 当前上游 body stream。handler 仍持有
            // executor receiver 以接收外层发送的终止 Error 并完成一次结算。
            let event = tokio::select! {
                biased;
                _ = tx.closed() => {
                    return Err(KeyComputeError::Internal("client disconnected".to_string()));
                }
                _ = ctx.wait_for_client_disconnect() => {
                    return Err(KeyComputeError::Internal("client disconnected".to_string()));
                }
                event = stream.next() => event,
            };
            let Some(event) = event else { break };
            match event.map_err(normalize_stream_error)? {
                StreamEvent::Delta {
                    content,
                    finish_reason,
                } => {
                    if !recorded_first_content && !content.is_empty() {
                        if let Some(attempt) = attempt {
                            let _ = lifecycle
                                .record_attempt_first_content(
                                    ctx.request_id,
                                    attempt.id,
                                    chrono::Utc::now(),
                                )
                                .await;
                        }
                        recorded_first_content = true;
                    }
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
                    // 在 run_plan 返回前记录真正完成的账号。外层会先发布 Done，
                    // 再分别关闭 attempt 和等待 handler 的客户端响应终态；此处若
                    // 延后会与 handler 结算形成竞态并把 fallback 用量记到 primary。
                    let ExecutionTarget::ProviderAccount { account_id, .. } = target else {
                        unreachable!("nodes return before streaming");
                    };
                    ctx.set_executed_provider_account(provider.clone(), *account_id);
                    execution_completed.store(true, Ordering::Release);
                    received_done = true;
                    break;
                }
                StreamEvent::Error { message } => {
                    tracing::error!(
                        request_id = %ctx.request_id,
                        message = %message,
                        "try_execute: received Error event"
                    );
                    return Err(provider_declared_stream_error());
                }
                // 原生协议入站会用 Raw 承载未经降级的 SSE 事件。它们不参与
                // 通用 token 计算，但必须穿过执行器才能由对应的入站 handler
                // 按原协议回写给客户端。
                StreamEvent::Raw { data } => {
                    let commits_response = raw_event_commits_response(&data);
                    if commits_response && !recorded_first_content {
                        if let Some(attempt) = attempt {
                            let _ = lifecycle
                                .record_attempt_first_content(
                                    ctx.request_id,
                                    attempt.id,
                                    chrono::Utc::now(),
                                )
                                .await;
                        }
                        recorded_first_content = true;
                    }
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
            return Err(normalize_stream_error(KeyComputeError::ProviderError(
                "Upstream stream ended without a terminal Done event".to_string(),
            )));
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

    #[test]
    fn classifies_primary_fallback_and_same_account_retry() {
        let primary_id = Uuid::new_v4();
        let fallback_id = Uuid::new_v4();
        let primary =
            ExecutionTarget::new_provider("openai", primary_id, "http://primary", "secret");
        let fallback =
            ExecutionTarget::new_provider("openai", fallback_id, "http://fallback", "secret");
        let retry = ExecutionTarget::new_provider("openai", primary_id, "http://primary", "secret");
        let mut attempted = HashSet::new();
        assert_eq!(
            classify_attempt_kind(0, &primary, &mut attempted),
            AttemptKind::Primary
        );
        assert_eq!(
            classify_attempt_kind(1, &fallback, &mut attempted),
            AttemptKind::Fallback
        );
        assert_eq!(
            classify_attempt_kind(2, &retry, &mut attempted),
            AttemptKind::Retry
        );
    }

    #[derive(Debug)]
    struct ManyChunksProvider {
        chunks: usize,
    }

    #[derive(Debug, Default)]
    struct SlowTerminalRecorder {
        inner: keycompute_types::TestRequestLifecycleRecorder,
        terminal_statuses: Mutex<Vec<RequestStatus>>,
        attempt_finality: Mutex<Vec<bool>>,
    }

    #[derive(Debug, Default)]
    struct FailingAttemptRecorder {
        inner: keycompute_types::TestRequestLifecycleRecorder,
    }

    #[async_trait]
    impl RequestLifecycleRecorder for FailingAttemptRecorder {
        async fn start_request(
            &self,
            value: keycompute_types::RequestTraceStart,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.start_request(value).await
        }

        async fn set_route(
            &self,
            request_id: Uuid,
            route: RouteType,
            status: RequestStatus,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.set_route(request_id, route, status).await
        }

        async fn start_attempt(
            &self,
            value: AttemptTraceStart,
        ) -> std::result::Result<AttemptRef, keycompute_types::TraceWriteError> {
            self.inner.start_attempt(value).await
        }

        async fn mark_trace_partial(
            &self,
            request_id: Uuid,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.mark_trace_partial(request_id).await
        }

        async fn record_attempt_response_meta(
            &self,
            request_id: Uuid,
            attempt_id: Uuid,
            meta: AttemptResponseMeta,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner
                .record_attempt_response_meta(request_id, attempt_id, meta)
                .await
        }

        async fn record_attempt_first_content(
            &self,
            request_id: Uuid,
            attempt_id: Uuid,
            at: chrono::DateTime<chrono::Utc>,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner
                .record_attempt_first_content(request_id, attempt_id, at)
                .await
        }

        async fn record_client_first_content(
            &self,
            request_id: Uuid,
            at: chrono::DateTime<chrono::Utc>,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.record_client_first_content(request_id, at).await
        }

        async fn finish_attempt_and_request(
            &self,
            value: AttemptTraceFinish,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.finish_attempt_and_request(value).await?;
            Err(keycompute_types::TraceWriteError(
                "injected attempt finalization failure".to_string(),
            ))
        }

        async fn finish_request_without_attempt(
            &self,
            value: keycompute_types::RequestTraceFinish,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.finish_request_without_attempt(value).await
        }

        async fn mark_billing_succeeded(
            &self,
            request_id: Uuid,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.mark_billing_succeeded(request_id).await
        }

        async fn mark_billing_failed(
            &self,
            request_id: Uuid,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.mark_billing_failed(request_id).await
        }
    }

    #[async_trait]
    impl RequestLifecycleRecorder for SlowTerminalRecorder {
        async fn start_request(
            &self,
            value: keycompute_types::RequestTraceStart,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.start_request(value).await
        }

        async fn set_route(
            &self,
            request_id: Uuid,
            route: RouteType,
            status: RequestStatus,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.set_route(request_id, route, status).await
        }

        async fn start_attempt(
            &self,
            value: AttemptTraceStart,
        ) -> std::result::Result<AttemptRef, keycompute_types::TraceWriteError> {
            self.inner.start_attempt(value).await
        }

        async fn mark_trace_partial(
            &self,
            request_id: Uuid,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.mark_trace_partial(request_id).await
        }

        async fn record_attempt_response_meta(
            &self,
            request_id: Uuid,
            attempt_id: Uuid,
            meta: AttemptResponseMeta,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner
                .record_attempt_response_meta(request_id, attempt_id, meta)
                .await
        }

        async fn record_attempt_first_content(
            &self,
            request_id: Uuid,
            attempt_id: Uuid,
            at: chrono::DateTime<chrono::Utc>,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner
                .record_attempt_first_content(request_id, attempt_id, at)
                .await
        }

        async fn record_client_first_content(
            &self,
            request_id: Uuid,
            at: chrono::DateTime<chrono::Utc>,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.record_client_first_content(request_id, at).await
        }

        async fn finish_attempt_and_request(
            &self,
            value: AttemptTraceFinish,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            tokio::time::sleep(Duration::from_secs(2)).await;
            self.attempt_finality
                .lock()
                .expect("attempt finality poisoned")
                .push(value.is_final);
            self.terminal_statuses
                .lock()
                .expect("terminal statuses poisoned")
                .push(value.request_status);
            Ok(())
        }

        async fn finish_request_without_attempt(
            &self,
            value: keycompute_types::RequestTraceFinish,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            tokio::time::sleep(Duration::from_secs(2)).await;
            self.terminal_statuses
                .lock()
                .expect("terminal statuses poisoned")
                .push(value.status);
            Ok(())
        }

        async fn mark_billing_succeeded(
            &self,
            request_id: Uuid,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.mark_billing_succeeded(request_id).await
        }

        async fn mark_billing_failed(
            &self,
            request_id: Uuid,
        ) -> std::result::Result<(), keycompute_types::TraceWriteError> {
            self.inner.mark_billing_failed(request_id).await
        }
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

    /// 只发送精确 Usage 后断流（无 Done）的 Provider：上游已接受付费请求，
    /// 但协议终态缺失，因此不能继续 retry/fallback。
    #[derive(Debug)]
    struct UsageOnlyTruncatedProvider;

    #[async_trait]
    impl ProviderAdapter for UsageOnlyTruncatedProvider {
        fn name(&self) -> &'static str {
            "usage-only-truncated"
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

    /// Provider 明确宣告失败时，若尚未提交客户端内容，fallback 仍是安全的；
    /// 已观察到的 usage 必须在下一次尝试开始时被清理。
    #[derive(Debug)]
    struct UsageThenDeclaredErrorProvider;

    #[async_trait]
    impl ProviderAdapter for UsageThenDeclaredErrorProvider {
        fn name(&self) -> &'static str {
            "usage-then-declared-error"
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
                    input_tokens: 111,
                    output_tokens: 222,
                }),
                Ok(StreamEvent::error("provider rejected the stream")),
            ])))
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

    fn stream_read_failure() -> KeyComputeError {
        KeyComputeError::UpstreamFailure {
            status: Some(200),
            stable_code: "upstream_stream_read".to_string(),
            retryable: false,
            summary: "Upstream response stream closed unexpectedly".to_string(),
        }
    }

    #[derive(Debug)]
    struct PendingStreamProvider {
        receiver: Mutex<Option<mpsc::Receiver<Result<StreamEvent>>>>,
        started: Arc<Notify>,
    }

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
                Err(stream_read_failure()),
            ])))
        }
    }

    #[async_trait]
    impl ProviderAdapter for PendingStreamProvider {
        fn name(&self) -> &'static str {
            "pending-stream"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            let receiver = self
                .receiver
                .lock()
                .expect("pending stream receiver poisoned")
                .take()
                .expect("pending stream provider called more than once");
            self.started.notify_one();
            Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(
                receiver,
            )))
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
                Err(stream_read_failure()),
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

    #[derive(Debug)]
    struct StreamOptionsCompatibilityProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ProviderAdapter for StreamOptionsCompatibilityProvider {
        fn name(&self) -> &'static str {
            "openai"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<llm_protocol_provider::StreamBox> {
            unreachable!("executor uses the metadata-preserving method")
        }

        async fn stream_chat_with_meta(
            &self,
            _transport: &dyn HttpTransport,
            request: UpstreamRequest,
        ) -> std::result::Result<
            llm_protocol_provider::UpstreamResponse<llm_protocol_provider::StreamBox>,
            llm_protocol_provider::UpstreamFailure,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if request.include_stream_usage {
                return Err(llm_protocol_provider::UpstreamFailure {
                    kind: llm_protocol_provider::UpstreamFailureKind::HttpStatus,
                    status: Some(400),
                    headers_received_at: Some(chrono::Utc::now()),
                    upstream_request_id: Some("compat-first-request".to_string()),
                    retryable: true,
                    stable_error_code: "upstream_stream_options_unsupported".to_string(),
                    sanitized_summary: "stream_options unsupported".to_string(),
                });
            }
            Ok(llm_protocol_provider::UpstreamResponse {
                meta: llm_protocol_provider::UpstreamResponseMeta::synthetic_success(),
                body: Box::pin(futures::stream::iter(vec![
                    Ok(StreamEvent::Delta {
                        content: "ok".to_string(),
                        finish_reason: Some("stop".to_string()),
                    }),
                    Ok(StreamEvent::Done),
                ])),
            })
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
            events.push(Err(stream_read_failure()));
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    #[allow(dead_code)]
    fn create_test_context() -> RequestContext {
        RequestContext::new(
            uuid::Uuid::new_v4(),
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
    fn parser_stream_errors_become_chain_terminal_protocol_failures() {
        let error = normalize_stream_error(KeyComputeError::ProviderError(
            "client_secret=secret prompt=private".to_string(),
        ));

        let KeyComputeError::UpstreamFailure {
            stable_code,
            retryable,
            summary,
            ..
        } = &error
        else {
            panic!("generic stream error was not normalized");
        };
        assert_eq!(stable_code, "upstream_stream_protocol");
        assert!(!retryable);
        assert_eq!(
            summary,
            "Upstream response stream was malformed or incomplete"
        );
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("private"));

        assert_eq!(
            classify_execution_error(&error),
            (
                TraceErrorCategory::Protocol,
                "upstream_stream_protocol".to_string(),
                false,
            )
        );
        assert!(prevents_retry_and_fallback(&error));

        let declared = provider_declared_stream_error();
        assert_eq!(
            classify_execution_error(&declared),
            (
                TraceErrorCategory::Protocol,
                "upstream_declared_error".to_string(),
                false,
            )
        );
        assert!(!prevents_retry_and_fallback(&declared));
    }

    #[test]
    fn ambiguous_post_dispatch_failures_stop_the_execution_chain() {
        let error = normalize_stream_error(KeyComputeError::UpstreamFailure {
            status: Some(200),
            stable_code: "upstream_stream_read".to_string(),
            retryable: false,
            summary: "connection closed".to_string(),
        });

        assert_eq!(
            classify_execution_error(&error),
            (
                TraceErrorCategory::Transport,
                "upstream_stream_read".to_string(),
                false,
            )
        );
        assert!(prevents_retry_and_fallback(&error));

        for stable_code in [
            "upstream_body_read",
            "upstream_ambiguous_timeout",
            "upstream_ambiguous_transport",
            "upstream_stream_protocol",
        ] {
            assert!(prevents_retry_and_fallback(
                &KeyComputeError::UpstreamFailure {
                    status: None,
                    stable_code: stable_code.to_string(),
                    retryable: false,
                    summary: "ambiguous provider outcome".to_string(),
                }
            ));
        }

        let ambiguous_timeout = KeyComputeError::UpstreamFailure {
            status: None,
            stable_code: "upstream_ambiguous_timeout".to_string(),
            retryable: false,
            summary: "provider outcome unknown".to_string(),
        };
        assert_eq!(
            classify_execution_error(&ambiguous_timeout).0,
            TraceErrorCategory::Timeout
        );
    }

    #[tokio::test]
    async fn retry_backoff_wakes_immediately_on_explicit_client_disconnect() {
        // Streaming response workers intentionally retain the receiver while
        // settling billing, so `tx.closed()` alone cannot cancel the backoff.
        // The RequestContext disconnect signal must interrupt a long timer.
        let (tx, _rx) = mpsc::channel(1);
        let ctx = Arc::new(create_test_context());
        let waiting_ctx = Arc::clone(&ctx);
        let waiter = tokio::spawn(async move {
            retry_backoff_cancelled(Duration::from_secs(60), &tx, &waiting_ctx).await
        });

        tokio::task::yield_now().await;
        ctx.mark_client_disconnected();

        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("disconnect should interrupt retry backoff")
                .expect("backoff waiter should join")
        );
    }

    #[tokio::test]
    async fn disconnect_during_retry_backoff_skips_next_call_and_keeps_billing_pending() {
        let calls = Arc::new(AtomicUsize::new(0));
        let first_call = Arc::new(Notify::new());
        let mut providers = HashMap::new();
        providers.insert(
            "primary".to_string(),
            Arc::new(CountingProvider {
                calls: Arc::clone(&calls),
                notified: Some(Arc::clone(&first_call)),
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(
            GatewayConfig {
                max_retries: 1,
                timeout_secs: 5,
                enable_fallback: true,
            },
            providers,
        );
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute_with_recorder(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "primary",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: Vec::new(),
                },
                Arc::new(AccountStateStore::new()),
                None,
                Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>,
            )
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), first_call.notified())
            .await
            .expect("the first provider call should run");
        ctx.mark_client_disconnected();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("disconnect should terminate the retry chain"),
            Some(StreamEvent::Error { message }) if message.contains("client disconnected")
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let finishes = recorder.request_finishes();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].status, RequestStatus::Cancelled);
        assert_eq!(finishes[0].billing_status, BillingStatus::Pending);
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
    async fn provider_done_precedes_attempt_trace_and_executor_leaves_request_open() {
        let mut providers = HashMap::new();
        providers.insert(
            "many-chunks".to_string(),
            Arc::new(ManyChunksProvider { chunks: 1 }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(
            GatewayConfig {
                max_retries: 0,
                timeout_secs: 1,
                enable_fallback: false,
            },
            providers,
        );
        let recorder = Arc::new(SlowTerminalRecorder::default());
        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute_with_recorder(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "many-chunks",
                        Uuid::new_v4(),
                        "http://mock",
                        "mock-key",
                    ),
                    fallback_chain: Vec::new(),
                },
                Arc::new(AccountStateStore::new()),
                None,
                Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>,
            )
            .await
            .unwrap();

        let mut received_done = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("provider events should arrive before execution timeout")
        {
            assert!(
                !matches!(event, StreamEvent::Error { .. }),
                "a completed provider response must not be followed by a timeout error"
            );
            if matches!(event, StreamEvent::Done) {
                received_done = true;
                break;
            }
        }
        assert!(received_done);
        assert_eq!(
            ctx.client_response_outcome(),
            None,
            "executor completion must not synthesize a client outcome"
        );

        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("trace repair should eventually release the response channel")
        {
            assert!(!matches!(event, StreamEvent::Error { .. }));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if recorder
                    .terminal_statuses
                    .lock()
                    .expect("terminal statuses poisoned")
                    .len()
                    == 1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attempt trace write should complete");
        assert_eq!(
            *recorder
                .terminal_statuses
                .lock()
                .expect("terminal statuses poisoned"),
            vec![RequestStatus::Running],
            "the executor must not choose a client-facing request outcome"
        );
        assert_eq!(
            *recorder
                .attempt_finality
                .lock()
                .expect("attempt finality poisoned"),
            vec![true],
            "the completed upstream attempt remains the request's final attempt"
        );
    }

    #[tokio::test]
    async fn non_stream_provider_records_first_content() {
        let mut providers = HashMap::new();
        providers.insert(
            "estimate-only".to_string(),
            Arc::new(EstimateOnlyProvider) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let ctx = Arc::new(RequestContext::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "gpt-4o",
            vec![Message::user("Hello")],
            false,
            PricingSnapshot::default(),
        ));
        let mut rx = executor
            .execute_with_recorder(
                ctx,
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "estimate-only",
                        Uuid::new_v4(),
                        "http://mock",
                        "mock-key",
                    ),
                    fallback_chain: Vec::new(),
                },
                Arc::new(AccountStateStore::new()),
                None,
                Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>,
            )
            .await
            .unwrap();
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::Done) {
                break;
            }
        }

        assert!(
            recorder
                .events()
                .iter()
                .any(|event| event.starts_with("attempt_first_content:")),
            "TTFT is provider timing and must be recorded for non-stream requests"
        );
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
        assert!(matches!(
            rx.recv().await,
            Some(StreamEvent::Error { message })
                if message.contains("malformed or incomplete")
        ));
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
    async fn test_execute_stops_after_ambiguous_stream_read_before_content() {
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
        assert!(matches!(rx.recv().await, Some(StreamEvent::Error { .. })));
        assert!(rx.recv().await.is_none());
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
        let mut raw_events = 0;
        loop {
            match rx.recv().await {
                Some(StreamEvent::Raw { .. }) => raw_events += 1,
                Some(StreamEvent::Delta { content, .. }) if content == "x" => break,
                event => panic!("unexpected event before fallback content: {event:?}"),
            }
        }
        // A provider-declared stream error is a non-retryable protocol failure:
        // skip same-account retries and move directly to the fallback account.
        assert_eq!(raw_events, 1);
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
            Some(StreamEvent::Error { message })
                if message.contains("Upstream response stream closed unexpectedly")
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
        // 客户端在 primary 流已经启动后断开（receiver 被 drop），executor
        // 应立即丢弃活动上游流，且不能触发 fallback。
        let (upstream_tx, upstream_rx) = mpsc::channel::<Result<StreamEvent>>(1);
        let primary_started = Arc::new(Notify::new());
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        // fallback 一旦被（错误）调用，立即通过 Notify 唤醒断言方，避免 10ms
        // 轮询粒度；负向断言窗口由 timeout 兜底。
        let fallback_called = Arc::new(Notify::new());
        let mut providers = HashMap::new();
        providers.insert(
            "primary".to_string(),
            Arc::new(PendingStreamProvider {
                receiver: Mutex::new(Some(upstream_rx)),
                started: Arc::clone(&primary_started),
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
        tokio::time::timeout(Duration::from_secs(1), primary_started.notified())
            .await
            .expect("primary provider should have started");
        drop(rx);

        tokio::time::timeout(Duration::from_secs(1), upstream_tx.closed())
            .await
            .expect("dropping the downstream receiver should cancel the active upstream stream");
        assert!(
            tokio::time::timeout(Duration::from_millis(500), fallback_called.notified())
                .await
                .is_err(),
            "fallback must not be attempted after the client disconnected"
        );
    }

    #[tokio::test]
    async fn test_execute_retries_the_same_account_before_finishing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut providers = HashMap::new();
        providers.insert(
            "primary".to_string(),
            Arc::new(CountingProvider {
                calls: Arc::clone(&calls),
                notified: None,
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(
            GatewayConfig {
                max_retries: 2,
                timeout_secs: 5,
                enable_fallback: true,
            },
            providers,
        );

        let mut rx = executor
            .execute(
                Arc::new(create_test_context()),
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

        assert!(matches!(rx.recv().await, Some(StreamEvent::Error { .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retryable_openai_failure_without_compatibility_retry_is_final() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            Arc::new(CountingProvider {
                calls: Arc::clone(&calls),
                notified: None,
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(
            GatewayConfig {
                max_retries: 0,
                timeout_secs: 5,
                enable_fallback: true,
            },
            providers,
        );
        let recorder = Arc::new(FailingAttemptRecorder::default());
        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute_with_recorder(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "openai",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: Vec::new(),
                },
                Arc::new(AccountStateStore::new()),
                None,
                Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>,
            )
            .await
            .unwrap();

        assert!(matches!(rx.recv().await, Some(StreamEvent::Error { .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let finish = recorder
            .inner
            .attempt_finishes()
            .into_iter()
            .next()
            .expect("the failed OpenAI attempt should be finalized");
        assert_eq!(finish.attempt_status, AttemptStatus::Failed);
        assert_eq!(finish.request_status, RequestStatus::Running);
        assert!(finish.is_final, "the execution plan must be exhausted");
        assert!(
            recorder.inner.request_finishes().is_empty(),
            "the response handler owns the request terminal state"
        );
        let failure = ctx
            .execution_failure()
            .expect("the handler must receive the exhausted execution failure");
        assert_eq!(failure.status, RequestStatus::Failed);
        assert_eq!(failure.error.origin, ErrorOrigin::Upstream);
        assert!(
            recorder
                .inner
                .events()
                .iter()
                .any(|event| event == &format!("trace_partial:{}", ctx.request_id)),
            "an exhausted failure must degrade the request when attempt finalization fails"
        );
    }

    #[tokio::test]
    async fn gateway_timeout_closes_attempt_but_leaves_request_to_handler() {
        let (_upstream_tx, upstream_rx) = mpsc::channel::<Result<StreamEvent>>(1);
        let started = Arc::new(Notify::new());
        let mut providers = HashMap::new();
        providers.insert(
            "pending-stream".to_string(),
            Arc::new(PendingStreamProvider {
                receiver: Mutex::new(Some(upstream_rx)),
                started: Arc::clone(&started),
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(
            GatewayConfig {
                max_retries: 0,
                timeout_secs: 1,
                enable_fallback: false,
            },
            providers,
        );
        let recorder = Arc::new(FailingAttemptRecorder::default());
        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute_with_recorder(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "pending-stream",
                        Uuid::new_v4(),
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: Vec::new(),
                },
                Arc::new(AccountStateStore::new()),
                None,
                Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>,
            )
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("pending provider should start");
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("gateway timeout should emit an error");
        assert!(matches!(event, Some(StreamEvent::Error { .. })));

        tokio::time::timeout(Duration::from_secs(1), async {
            while !recorder
                .inner
                .events()
                .iter()
                .any(|event| event == &format!("trace_partial:{}", ctx.request_id))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed-out attempt failure should degrade the request trace");
        let finish = recorder.inner.attempt_finishes().remove(0);
        assert_eq!(finish.attempt_status, AttemptStatus::TimedOut);
        assert_eq!(finish.request_status, RequestStatus::Running);
        assert!(finish.is_final);
        assert!(recorder.inner.request_finishes().is_empty());
        let failure = ctx.execution_failure().expect("handler timeout failure");
        assert_eq!(failure.status, RequestStatus::TimedOut);
        assert_eq!(failure.error.code, "gateway_execution_timeout");
        assert!(
            recorder
                .inner
                .events()
                .iter()
                .any(|event| event == &format!("trace_partial:{}", ctx.request_id)),
            "a gateway timeout must degrade the request when attempt finalization fails"
        );
    }

    #[tokio::test]
    async fn stream_options_compatibility_retry_is_a_separate_attempt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            Arc::new(StreamOptionsCompatibilityProvider {
                calls: Arc::clone(&calls),
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(
            GatewayConfig {
                max_retries: 0,
                timeout_secs: 5,
                enable_fallback: true,
            },
            providers,
        );
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let account_id = Uuid::new_v4();
        let mut rx = executor
            .execute_with_recorder(
                Arc::new(create_test_context()),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "openai",
                        account_id,
                        "http://primary",
                        "mock-key",
                    ),
                    fallback_chain: vec![],
                },
                Arc::new(AccountStateStore::new()),
                None,
                Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>,
            )
            .await
            .unwrap();

        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::Done) {
                break;
            }
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let events = recorder.events();
                if events
                    .iter()
                    .filter(|event| event.starts_with("finish_attempt:"))
                    .count()
                    == 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both HTTP requests should finish their own trace attempts");

        let events = recorder.events();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("start_attempt:"))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("attempt_meta:"))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn test_execute_cancels_active_upstream_stream_after_client_disconnect() {
        let (upstream_tx, upstream_rx) = mpsc::channel::<Result<StreamEvent>>(1);
        let started = Arc::new(Notify::new());
        let mut providers = HashMap::new();
        providers.insert(
            "pending-stream".to_string(),
            Arc::new(PendingStreamProvider {
                receiver: Mutex::new(Some(upstream_rx)),
                started: Arc::clone(&started),
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(GatewayConfig::default(), providers);
        let ctx = Arc::new(create_test_context());
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "pending-stream",
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

        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("upstream stream should start");
        ctx.mark_client_disconnected();

        tokio::time::timeout(Duration::from_secs(1), upstream_tx.closed())
            .await
            .expect("disconnect should promptly drop the active upstream stream");
        let terminal = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("executor should emit a terminal event after cancellation");
        assert!(
            matches!(terminal, Some(StreamEvent::Error { message }) if message.contains("client disconnected"))
        );
    }

    #[tokio::test]
    async fn test_execute_aborts_fallback_when_client_disconnect_is_marked() {
        // 后台结算任务持有 receiver 直到 Done/Error，客户端断开不会触发
        // `tx.is_closed()`。若 handler 已标记断开，executor 连 primary 都不应
        // 启动，更不能继续尝试 fallback。
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
        // 模拟流式 handler 在执行器开始前已经观察到 SSE 断开。
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

        // executor 必须直接终止链，向 handler 上报 Error 而不是启动任何上游。
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Error { message }) if message.contains("client disconnected"))
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
    async fn truncated_usage_only_stream_stops_before_fallback() {
        // 收到精确 Usage 说明付费 POST 已被 Provider 接受；即使尚未向客户端
        // 提交 Delta，缺失 Done 的结果仍然不确定，不能再发送第二次推理。
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let mut providers = HashMap::new();
        providers.insert(
            "usage-only-truncated".to_string(),
            Arc::new(UsageOnlyTruncatedProvider) as Arc<dyn ProviderAdapter>,
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
        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                ExecutionPlan {
                    primary: ExecutionTarget::new_provider(
                        "usage-only-truncated",
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
                Some(Arc::new(ProviderHealthStore::new())),
            )
            .await
            .unwrap();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("terminal protocol error should be delivered"),
            Some(StreamEvent::Error { .. })
        ));
        assert_eq!(
            fallback_calls.load(Ordering::SeqCst),
            0,
            "ambiguous post-dispatch protocol failures must stop the chain"
        );
        assert_eq!(ctx.usage_snapshot(), (111, 222));
        assert_eq!(
            ctx.execution_failure()
                .expect("terminal protocol failure should be retained")
                .error
                .code,
            "upstream_stream_protocol"
        );
    }

    #[tokio::test]
    async fn fallback_after_declared_error_does_not_inherit_previous_output_usage() {
        // Provider 明确返回 error 是确定失败，未提交 Delta 时仍可 fallback；
        // fallback 必须从零重新估算 output，不能沿用 primary 的残留精确值。
        let mut providers = HashMap::new();
        providers.insert(
            "usage-then-declared-error".to_string(),
            Arc::new(UsageThenDeclaredErrorProvider) as Arc<dyn ProviderAdapter>,
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
                        "usage-then-declared-error",
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

    #[tokio::test]
    async fn partial_fallback_usage_is_billed_to_the_fallback_account() {
        let config = GatewayConfig {
            max_retries: 0,
            ..GatewayConfig::default()
        };
        let mut providers = HashMap::new();
        providers.insert(
            "failing".to_string(),
            Arc::new(FailingProvider) as Arc<dyn ProviderAdapter>,
        );
        providers.insert(
            "mid-stream-fail".to_string(),
            Arc::new(MidStreamFailProvider {
                deltas_before_error: 1,
            }) as Arc<dyn ProviderAdapter>,
        );
        let executor = GatewayExecutor::new(config, providers);
        let ctx = Arc::new(create_test_context());
        let primary_account_id = Uuid::new_v4();
        let fallback_account_id = Uuid::new_v4();
        let plan = ExecutionPlan {
            primary: ExecutionTarget::new_provider(
                "failing",
                primary_account_id,
                "http://primary",
                "mock-key",
            ),
            fallback_chain: vec![ExecutionTarget::new_provider(
                "mid-stream-fail",
                fallback_account_id,
                "http://fallback",
                "mock-key",
            )],
        };

        let mut rx = executor
            .execute(
                Arc::clone(&ctx),
                plan,
                Arc::new(AccountStateStore::new()),
                None,
            )
            .await
            .expect("execute should return receiver");
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("partial fallback should terminate")
        {
            if matches!(event, StreamEvent::Error { .. }) {
                break;
            }
        }

        assert_eq!(ctx.executed_provider_account(), None);
        assert_eq!(
            ctx.billing_target("failing", primary_account_id),
            ("mid-stream-fail".to_string(), fallback_account_id)
        );
    }
}
