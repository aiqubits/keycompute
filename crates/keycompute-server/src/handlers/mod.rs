//! 处理器模块
//
//! 处理各种 HTTP 请求

// 管理功能（拆分为多个模块）
pub mod admin_account;
pub mod admin_monitoring;
pub mod admin_node_gateway;
pub mod admin_pricing;
pub mod admin_settings;
pub mod admin_user;
pub mod anthropic;

pub mod auth;
pub mod billing;
pub mod distribution;
pub mod gateway;
pub mod health;
pub mod node;
pub mod node_gateway_token;
pub mod node_tips;
pub mod openai;
pub mod payment;
pub mod pricing;
pub mod requirement;
pub mod routing;
pub mod user;

// 认证相关
pub use auth::{
    complete_registration_handler, forgot_password_handler, login_handler, refresh_token_handler,
    register_handler, reset_password_handler, verify_reset_token_handler,
};

// 需求收集
pub use requirement::submit_requirement_handler;

// OpenAI 兼容 API (统一入口)
pub use openai::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ListModelsResponse, Model,
    chat_completions, list_models, retrieve_model,
};

// Anthropic Messages 兼容入口
pub use anthropic::messages;

// Distribution 分销管理
pub use distribution::{
    create_distribution_rule, delete_distribution_rule, generate_invite_link,
    get_distribution_stats, get_my_distribution_earnings, get_my_referral_code, get_my_referrals,
    list_distribution_records, list_distribution_rules, update_distribution_rule,
};

// 用户自服务
pub use user::{
    change_password, create_api_key, delete_api_key, get_current_user, get_my_usage,
    get_my_usage_stats, list_my_api_keys, update_profile,
};

// 用户管理（admin_user）
pub use admin_user::{
    AdminUserInfo, UpdateUserRequest, UserListQueryParams, UserListResponse, delete_user,
    freeze_user_balance, get_user_by_id, list_all_api_keys, list_all_users, list_tenants,
    unfreeze_user_balance, update_user, update_user_balance,
};

// 账号管理（admin_account）
pub use admin_account::{
    AccountInfo, CreateAccountRequest, UpdateAccountRequest, create_account, delete_account,
    get_default_endpoint, list_accounts, refresh_account, test_account, update_account,
};

// Node Gateway 管理
pub use admin_node_gateway::{
    delete_node, exclude_node, get_node_gateway_overview, recover_node, revoke_node_token,
};

// 监控追踪
pub use admin_account::{probe_account_for_monitoring, probe_enabled_account_for_monitoring};
pub use admin_monitoring::{
    get_monitoring_overview, get_monitoring_request, get_monitoring_summary,
    get_monitoring_target_health, list_monitoring_requests, probe_monitoring_targets,
};

// 定价管理（admin_pricing）
pub use admin_pricing::{
    CreatePricingAdminRequest, PricingInfo, UpdatePricingAdminRequest, create_pricing,
    delete_pricing, list_pricing, make_pricing_default, update_pricing,
};

// 系统设置（admin_settings）
pub use admin_settings::{
    AdminSystemSettings, get_public_settings, get_system_setting_by_key, get_system_settings,
    update_system_setting_by_key, update_system_settings,
};

// 定价和账单
pub use billing::{calculate_cost, get_billing_stats, list_billing_records};
pub use pricing::{calculate_cost as get_pricing_cost, get_pricing};

// 调试接口
pub use gateway::{check_provider_health, get_execution_stats, get_gateway_status};
pub use routing::{debug_routing, get_provider_health, reset_health, set_account_cooldown};

// 健康检查
pub use health::health_check;

// 节点网关
pub use node::{node_complete, node_heartbeat, node_poll, node_register};

// 用户节点网关 token 管理
pub use node_gateway_token::{
    admin_approve_token, admin_list_pending_tokens, create_my_node_gateway_token,
    delete_my_node_gateway_token, get_my_node_gateway_token, list_my_node_gateway_tokens,
};

// 节点租赁小费管理
pub use node_tips::{
    admin_approve_withdrawal, admin_complete_withdrawal, admin_get_tip_ratio,
    admin_list_pending_withdrawals, admin_update_tip_ratio, create_tip_withdrawal,
    get_my_tips_history, get_my_tips_summary, get_my_withdrawals,
};

// 支付相关
pub use payment::{
    admin_list_payment_orders, admin_payment_providers, admin_verify_payment_provider,
    alipay_notify, create_payment_order, get_my_balance, get_payment_order, list_my_payment_orders,
    list_payment_methods, sync_payment_order, wechatpay_notify,
};

/// Persist the first client-facing outcome selected by the handler.
///
/// Billing and protocol delivery happen before successful callers reach this
/// helper. `RequestContext` keeps the first outcome so a concurrent body drop
/// cannot overwrite a response that was already handed off, or vice versa.
pub(crate) async fn finish_client_response_trace(
    lifecycle: &std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
    ctx: &keycompute_types::RequestContext,
    outcome: keycompute_types::ClientResponseOutcome,
) {
    use keycompute_types::ClientResponseOutcome;

    match outcome {
        ClientResponseOutcome::Succeeded => ctx.mark_client_response_succeeded(),
        ClientResponseOutcome::ClientDisconnected => ctx.mark_client_disconnected(),
        ClientResponseOutcome::ResponseFailed => ctx.mark_client_response_failed(),
        ClientResponseOutcome::TimedOut => ctx.mark_client_response_timed_out(),
    }
    let effective_outcome = ctx.client_response_outcome().unwrap_or(outcome);
    let finish = keycompute_types::client_response_trace_finish_with_failure(
        ctx.request_id,
        effective_outcome,
        ctx.execution_failure(),
    );
    if let Err(error) = lifecycle.finish_request_without_attempt(finish).await {
        tracing::warn!(
            request_id = %ctx.request_id,
            ?effective_outcome,
            %error,
            "failed to finish request trace after client response"
        );
    }
}

/// Owns terminalization from the moment a request trace is created until a
/// `RequestContext`-aware response guard takes over.
///
/// Axum may drop a handler future when the client disconnects while the handler
/// is still awaiting balance, pricing, or routing. Those phases do not yet have
/// a `RequestContext`, but their process-local metrics and database trace still
/// need a terminal outcome.
pub(crate) struct PreExecutionTraceGuard {
    lifecycle: std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
    request_id: uuid::Uuid,
    armed: bool,
}

impl PreExecutionTraceGuard {
    pub(crate) fn new(
        lifecycle: std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
        request_id: uuid::Uuid,
    ) -> Self {
        Self {
            lifecycle,
            request_id,
            armed: true,
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    pub(crate) async fn finish_failed(
        &mut self,
        origin: keycompute_types::ErrorOrigin,
        category: keycompute_types::TraceErrorCategory,
        code: &str,
    ) {
        let finish = keycompute_types::RequestTraceFinish {
            request_id: self.request_id,
            status: keycompute_types::RequestStatus::Failed,
            error: Some(keycompute_types::TraceErrorInfo {
                origin,
                category,
                code: code.to_string(),
                summary: None,
                retryable: Some(false),
            }),
            billing_status: keycompute_types::BillingStatus::NotApplicable,
            finished_at: chrono::Utc::now(),
        };
        if let Err(error) = self.lifecycle.finish_request_without_attempt(finish).await {
            tracing::warn!(
                request_id = %self.request_id,
                %error,
                "failed to finish pre-execution trace"
            );
        }
        self.disarm();
    }
}

impl Drop for PreExecutionTraceGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        let lifecycle = std::sync::Arc::clone(&self.lifecycle);
        let request_id = self.request_id;
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(%request_id, "unable to finish cancelled pre-execution request outside a Tokio runtime");
            return;
        };
        runtime.spawn(async move {
            let finish = keycompute_types::RequestTraceFinish {
                request_id,
                status: keycompute_types::RequestStatus::Cancelled,
                error: Some(keycompute_types::TraceErrorInfo {
                    origin: keycompute_types::ErrorOrigin::Gateway,
                    category: keycompute_types::TraceErrorCategory::ClientDisconnect,
                    code: "client_disconnected".to_string(),
                    summary: None,
                    retryable: Some(false),
                }),
                billing_status: keycompute_types::BillingStatus::NotApplicable,
                finished_at: chrono::Utc::now(),
            };
            if let Err(error) = lifecycle.finish_request_without_attempt(finish).await {
                tracing::warn!(%request_id, %error, "failed to finish cancelled pre-execution request");
            }
        });
    }
}

/// Marks and persists a response as disconnected if the HTTP handler is
/// cancelled before it records a terminal client outcome. This closes the
/// narrow race where a background response worker successfully hands its
/// result to a oneshot channel just before Axum drops the handler future.
pub(crate) struct ClientResponseGuard {
    lifecycle: std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
    ctx: std::sync::Arc<keycompute_types::RequestContext>,
    armed: bool,
}

impl ClientResponseGuard {
    pub(crate) fn new(
        lifecycle: std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
        ctx: std::sync::Arc<keycompute_types::RequestContext>,
    ) -> Self {
        Self {
            lifecycle,
            ctx,
            armed: true,
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    pub(crate) async fn finish_with_outcome(
        &mut self,
        outcome: keycompute_types::ClientResponseOutcome,
    ) {
        finish_client_response_trace(&self.lifecycle, &self.ctx, outcome).await;
        self.disarm();
    }
}

impl Drop for ClientResponseGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        self.ctx.mark_client_disconnected();
        let lifecycle = std::sync::Arc::clone(&self.lifecycle);
        let ctx = std::sync::Arc::clone(&self.ctx);
        let request_id = ctx.request_id;
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(%request_id, "unable to finish cancelled request outside a Tokio runtime");
            return;
        };
        runtime.spawn(async move {
            finish_client_response_trace(
                &lifecycle,
                &ctx,
                keycompute_types::ClientResponseOutcome::ClientDisconnected,
            )
            .await;
        });
    }
}

/// Record the first client-visible content for a response whose execution may
/// already be terminal, then place a barrier behind the queued write. The
/// barrier must run even when enqueueing fails so request-scoped failure state
/// is cleared and the PostgreSQL recorder can downgrade the trace to partial.
pub(crate) async fn record_final_client_first_content(
    lifecycle: &std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
    request_id: uuid::Uuid,
) -> Result<(), keycompute_types::TraceWriteError> {
    let record_result = lifecycle
        .record_client_first_content(request_id, chrono::Utc::now())
        .await;
    let flush_result = lifecycle.flush_intermediate_updates(request_id).await;

    match (record_result, flush_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(record_error), Err(flush_error)) => Err(keycompute_types::TraceWriteError(format!(
            "{record_error}; final intermediate flush failed: {flush_error}"
        ))),
    }
}

fn normalize_public_base_url(base_url: &str) -> Option<String> {
    let normalized = base_url.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub(crate) fn configured_public_base_url(configured_base_url: Option<&str>) -> Option<String> {
    configured_base_url.and_then(normalize_public_base_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_guard_context() -> std::sync::Arc<keycompute_types::RequestContext> {
        std::sync::Arc::new(keycompute_types::RequestContext::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "test-model",
            Vec::new(),
            false,
            keycompute_types::PricingSnapshot::default(),
        ))
    }

    #[tokio::test]
    async fn client_response_guard_only_finishes_an_armed_handler_as_disconnected() {
        let cancelled = response_guard_context();
        let cancelled_recorder =
            std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        drop(ClientResponseGuard::new(
            std::sync::Arc::clone(&cancelled_recorder)
                as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
            std::sync::Arc::clone(&cancelled),
        ));
        assert!(cancelled.is_client_disconnected());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while cancelled_recorder.request_finishes().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("armed guard should persist its disconnect outcome");
        assert_eq!(
            cancelled_recorder.request_finishes()[0].status,
            keycompute_types::RequestStatus::Cancelled
        );

        let completed = response_guard_context();
        let completed_recorder =
            std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let mut guard = ClientResponseGuard::new(
            std::sync::Arc::clone(&completed_recorder)
                as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
            std::sync::Arc::clone(&completed),
        );
        guard.disarm();
        drop(guard);
        assert!(!completed.is_client_disconnected());
        tokio::task::yield_now().await;
        assert!(completed_recorder.request_finishes().is_empty());
    }

    #[tokio::test]
    async fn client_response_guard_explicit_outcome_disarms_disconnect_fallback() {
        for (outcome, expected_status) in [
            (
                keycompute_types::ClientResponseOutcome::ResponseFailed,
                keycompute_types::RequestStatus::Failed,
            ),
            (
                keycompute_types::ClientResponseOutcome::TimedOut,
                keycompute_types::RequestStatus::TimedOut,
            ),
        ] {
            let ctx = response_guard_context();
            let recorder =
                std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
            let mut guard = ClientResponseGuard::new(
                std::sync::Arc::clone(&recorder)
                    as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
                std::sync::Arc::clone(&ctx),
            );

            guard.finish_with_outcome(outcome).await;
            drop(guard);
            tokio::task::yield_now().await;

            assert_eq!(ctx.client_response_outcome(), Some(outcome));
            let finishes = recorder.request_finishes();
            assert_eq!(finishes.len(), 1);
            assert_eq!(finishes[0].status, expected_status);
        }
    }

    #[tokio::test]
    async fn pre_execution_guard_finishes_cancelled_requests_without_billing() {
        let request_id = uuid::Uuid::new_v4();
        let recorder =
            std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        drop(PreExecutionTraceGuard::new(
            std::sync::Arc::clone(&recorder)
                as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
            request_id,
        ));

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while recorder.request_finishes().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pre-execution cancellation should be persisted");
        let finish = &recorder.request_finishes()[0];
        assert_eq!(finish.request_id, request_id);
        assert_eq!(finish.status, keycompute_types::RequestStatus::Cancelled);
        assert_eq!(
            finish.billing_status,
            keycompute_types::BillingStatus::NotApplicable
        );
        assert_eq!(
            finish.error.as_ref().map(|error| error.category),
            Some(keycompute_types::TraceErrorCategory::ClientDisconnect)
        );
    }

    #[tokio::test]
    async fn pre_execution_guard_explicit_failure_disarms_disconnect_fallback() {
        let request_id = uuid::Uuid::new_v4();
        let recorder =
            std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let mut guard = PreExecutionTraceGuard::new(
            std::sync::Arc::clone(&recorder)
                as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>,
            request_id,
        );

        guard
            .finish_failed(
                keycompute_types::ErrorOrigin::Client,
                keycompute_types::TraceErrorCategory::InvalidRequest,
                "invalid_request",
            )
            .await;
        drop(guard);
        tokio::task::yield_now().await;

        let finishes = recorder.request_finishes();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].request_id, request_id);
        assert_eq!(finishes[0].status, keycompute_types::RequestStatus::Failed);
        assert_eq!(
            finishes[0].billing_status,
            keycompute_types::BillingStatus::NotApplicable
        );
        assert_eq!(
            finishes[0].error.as_ref().map(|error| error.category),
            Some(keycompute_types::TraceErrorCategory::InvalidRequest)
        );
    }

    #[tokio::test]
    async fn client_response_trace_preserves_the_first_terminal_outcome() {
        let ctx = response_guard_context();
        let recorder =
            std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let lifecycle = std::sync::Arc::clone(&recorder)
            as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>;

        ctx.set_execution_failure(keycompute_types::RequestExecutionFailure {
            status: keycompute_types::RequestStatus::TimedOut,
            error: keycompute_types::TraceErrorInfo {
                origin: keycompute_types::ErrorOrigin::Node,
                category: keycompute_types::TraceErrorCategory::NodeExpired,
                code: "node_expired".to_string(),
                summary: None,
                retryable: Some(false),
            },
            billing_status: keycompute_types::BillingStatus::NotApplicable,
        });
        ctx.mark_client_disconnected();
        finish_client_response_trace(
            &lifecycle,
            &ctx,
            keycompute_types::ClientResponseOutcome::Succeeded,
        )
        .await;

        assert_eq!(
            ctx.client_response_outcome(),
            Some(keycompute_types::ClientResponseOutcome::ClientDisconnected)
        );
        assert_eq!(
            recorder.request_finishes()[0].status,
            keycompute_types::RequestStatus::Cancelled
        );
        assert_eq!(
            recorder.request_finishes()[0]
                .error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("client_disconnected")
        );
    }

    #[tokio::test]
    async fn client_response_trace_uses_the_execution_failure() {
        let ctx = response_guard_context();
        let recorder =
            std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let lifecycle = std::sync::Arc::clone(&recorder)
            as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>;
        ctx.set_execution_failure(keycompute_types::RequestExecutionFailure {
            status: keycompute_types::RequestStatus::Failed,
            error: keycompute_types::TraceErrorInfo {
                origin: keycompute_types::ErrorOrigin::Upstream,
                category: keycompute_types::TraceErrorCategory::Upstream5xx,
                code: "upstream_failed".to_string(),
                summary: None,
                retryable: Some(true),
            },
            billing_status: keycompute_types::BillingStatus::Pending,
        });

        finish_client_response_trace(
            &lifecycle,
            &ctx,
            keycompute_types::ClientResponseOutcome::ResponseFailed,
        )
        .await;

        let finishes = recorder.request_finishes();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].status, keycompute_types::RequestStatus::Failed);
        assert_eq!(
            finishes[0].error.as_ref().map(|error| error.code.as_str()),
            Some("upstream_failed")
        );
    }

    #[tokio::test]
    async fn final_client_content_is_followed_by_an_intermediate_barrier() {
        let request_id = uuid::Uuid::new_v4();
        let recorder =
            std::sync::Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let lifecycle = std::sync::Arc::clone(&recorder)
            as std::sync::Arc<dyn keycompute_types::RequestLifecycleRecorder>;

        record_final_client_first_content(&lifecycle, request_id)
            .await
            .unwrap();

        assert_eq!(
            recorder.events(),
            [
                format!("client_first_content:{request_id}"),
                format!("flush_intermediate:{request_id}"),
            ]
        );
    }

    #[test]
    fn test_configured_public_base_url_prefers_configured_value() {
        let base_url = configured_public_base_url(Some("https://configured.example.com/"));

        assert_eq!(base_url.as_deref(), Some("https://configured.example.com"));
    }

    #[test]
    fn test_configured_public_base_url_returns_none_when_missing() {
        let base_url = configured_public_base_url(None);

        assert!(base_url.is_none());
    }

    #[test]
    fn test_configured_public_base_url_ignores_blank_values() {
        let base_url = configured_public_base_url(Some("   "));

        assert!(base_url.is_none());
    }
}
