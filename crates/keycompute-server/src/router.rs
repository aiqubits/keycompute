//! 路由配置
//!
//! Axum Router 配置，挂载所有路由
//!
//! 路由设计原则：
//! - 统一使用 /api/v1/* 前缀（除 OpenAI 兼容 API 外）
//! - 权限控制通过中间件实现，而非路径前缀
//! - Admin 和普通用户共用前端，通过权限控制展示不同模块

use crate::{
    handlers::{
        admin_approve_token,
        admin_approve_withdrawal,
        admin_complete_withdrawal,
        admin_get_tip_ratio,
        // 支付相关
        admin_list_payment_orders,
        // 节点网关 token 审批（Admin）
        admin_list_pending_tokens,
        admin_list_pending_withdrawals,
        admin_payment_providers,
        admin_update_tip_ratio,
        admin_verify_payment_provider,
        alipay_notify,
        calculate_cost,
        change_password,
        // OpenAI 兼容 API
        chat_completions,
        check_provider_health,
        complete_registration_handler,
        create_account,
        create_api_key,
        create_distribution_rule,
        // 节点网关 token（用户自服务）
        create_my_node_gateway_token,
        create_payment_order,
        // 定价管理（Admin）
        create_pricing,
        create_tip_withdrawal,
        // 调试接口
        debug_routing,
        delete_account,
        delete_api_key,
        delete_distribution_rule,
        delete_my_node_gateway_token,
        delete_node,
        delete_pricing,
        delete_user,
        exclude_node,
        // 认证相关
        forgot_password_handler,
        freeze_user_balance,
        // Distribution 分销
        generate_invite_link,
        get_billing_stats,
        // 用户自服务
        get_current_user,
        // Distribution 分销
        get_distribution_stats,
        get_execution_stats,
        get_gateway_status,
        get_monitoring_overview,
        get_my_balance,
        get_my_distribution_earnings,
        get_my_node_gateway_token,
        get_my_referral_code,
        get_my_referrals,
        get_my_tips_history,
        // 节点租赁小费
        get_my_tips_summary,
        get_my_usage,
        get_my_usage_stats,
        get_my_withdrawals,
        get_node_gateway_overview,
        get_payment_order,
        get_provider_health,
        // 公开设置
        get_public_settings,
        get_system_setting_by_key,
        get_system_settings,
        get_user_by_id,
        // 健康检查
        health_check,
        list_accounts,
        list_all_api_keys,
        // 管理功能（Admin 权限）
        list_all_users,
        list_billing_records,
        list_distribution_records,
        list_distribution_rules,
        list_models,
        list_my_api_keys,
        list_my_node_gateway_tokens,
        list_my_payment_orders,
        list_payment_methods,
        // 定价管理
        list_pricing,
        list_tenants,
        login_handler,
        make_pricing_default,
        messages,
        // 节点网关
        node_complete,
        node_heartbeat,
        node_poll,
        node_register,
        recover_node,
        refresh_account,
        refresh_token_handler,
        register_handler,
        reset_health,
        reset_password_handler,
        retrieve_model,
        revoke_node_token,
        set_account_cooldown,
        submit_requirement_handler,
        sync_payment_order,
        test_account,
        unfreeze_user_balance,
        update_account,
        update_distribution_rule,
        update_pricing,
        update_profile,
        update_system_setting_by_key,
        update_system_settings,
        update_user,
        update_user_balance,
        verify_reset_token_handler,
        wechatpay_notify,
    },
    middleware::{
        admin_auth_middleware, anthropic_error_response_middleware, cors_layer,
        maintenance_mode_middleware, payment_notify_rate_limit_middleware,
        public_auth_rate_limit_middleware, rate_limit_middleware, request_logger,
        trace_id_middleware,
    },
    state::AppState,
};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware::from_fn_with_state,
    routing::{delete, get, post, put},
};
use tower_http::trace::TraceLayer;

/// Anthropic permits inline multimodal blocks, so its compatibility endpoint
/// needs a higher limit than Axum's 2 MiB default. Keep the limit explicit and
/// bounded to avoid accepting arbitrarily large JSON payloads.
const ANTHROPIC_MESSAGES_BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// 创建路由器
pub fn create_router(state: AppState) -> Router {
    // ==================== 1. 公共注册路由（按 IP/设备限流） ====================
    let registration_routes = Router::new()
        .route("/api/v1/auth/register", post(register_handler))
        .route(
            "/api/v1/auth/register/complete",
            post(complete_registration_handler),
        )
        // 首页需求收集表单（公开，按 IP/设备限流防滥用）
        .route("/api/v1/requirements", post(submit_requirement_handler))
        .layer(from_fn_with_state(
            state.clone(),
            public_auth_rate_limit_middleware,
        ));

    // ==================== 2. 其他认证路由（不需要限流） ====================
    let auth_routes = registration_routes.merge(
        Router::new()
            .route("/api/v1/auth/login", post(login_handler))
            .route(
                "/api/v1/auth/forgot-password",
                post(forgot_password_handler),
            )
            .route("/api/v1/auth/reset-password", post(reset_password_handler))
            .route(
                "/api/v1/auth/verify-reset-token/{token}",
                get(verify_reset_token_handler),
            )
            .route("/api/v1/auth/refresh-token", post(refresh_token_handler)),
    );

    // ==================== 3. OpenAI 兼容 API（需要限流） ====================
    // 这些端点使用 API Key 认证，路径保持与 OpenAI 一致
    // 参考: https://platform.openai.com/docs/api-reference
    let openai_routes = Router::new()
        // Chat Completions
        .route("/v1/chat/completions", post(chat_completions))
        // Models
        .route("/v1/models", get(list_models))
        .route("/v1/models/{model}", get(retrieve_model))
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware));

    let anthropic_routes = Router::new()
        .route("/v1/messages", post(messages))
        .layer(DefaultBodyLimit::max(ANTHROPIC_MESSAGES_BODY_LIMIT_BYTES))
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware));

    // ==================== 4. 用户自服务 API（需要认证 + 限流） ====================
    // 用户管理自己的资源，Admin 也可以访问（根据业务逻辑返回不同范围的数据）
    let user_routes = Router::new()
        // 当前用户信息
        .route("/api/v1/me", get(get_current_user))
        .route("/api/v1/me/profile", put(update_profile))
        .route("/api/v1/me/password", put(change_password))
        // API Keys 管理
        .route("/api/v1/keys", get(list_my_api_keys).post(create_api_key))
        .route("/api/v1/keys/{id}", delete(delete_api_key))
        // 用量统计
        .route("/api/v1/usage", get(get_my_usage))
        .route("/api/v1/usage/stats", get(get_my_usage_stats))
        // 用户分销收益
        .route(
            "/api/v1/me/distribution/earnings",
            get(get_my_distribution_earnings),
        )
        .route("/api/v1/me/distribution/referrals", get(get_my_referrals))
        // 推荐码和邀请链接
        .route("/api/v1/me/referral/code", get(get_my_referral_code))
        .route(
            "/api/v1/me/referral/invite-link",
            post(generate_invite_link),
        )
        // 节点网关 token 管理
        .route(
            "/api/v1/me/node-gateway/token/{id}",
            delete(delete_my_node_gateway_token),
        )
        .route(
            "/api/v1/me/node-gateway/token",
            get(get_my_node_gateway_token).post(create_my_node_gateway_token),
        )
        .route(
            "/api/v1/me/node-gateway/tokens",
            get(list_my_node_gateway_tokens),
        )
        // 节点租赁小费
        .route("/api/v1/me/tips", get(get_my_tips_summary))
        .route("/api/v1/me/tips/history", get(get_my_tips_history))
        .route("/api/v1/me/tips/withdraw", post(create_tip_withdrawal))
        .route("/api/v1/me/tips/withdrawals", get(get_my_withdrawals))
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware));

    // ==================== 5. 管理功能 API（需要 Admin 权限） ====================
    // 用户管理（Admin 可以管理所有用户，普通用户只能看自己）
    let admin_user_routes = Router::new()
        .route("/api/v1/users", get(list_all_users))
        .route(
            "/api/v1/users/{id}",
            get(get_user_by_id).put(update_user).delete(delete_user),
        )
        .route("/api/v1/users/{id}/balance", post(update_user_balance))
        .route(
            "/api/v1/users/{id}/balance/freeze",
            post(freeze_user_balance),
        )
        .route(
            "/api/v1/users/{id}/balance/unfreeze",
            post(unfreeze_user_balance),
        )
        .route("/api/v1/users/{id}/api-keys", get(list_all_api_keys));

    // 账号/渠道管理（仅 Admin）
    let admin_account_routes = Router::new()
        .route("/api/v1/accounts", get(list_accounts).post(create_account))
        .route(
            "/api/v1/accounts/{id}",
            put(update_account).delete(delete_account),
        )
        .route("/api/v1/accounts/{id}/test", post(test_account))
        .route("/api/v1/accounts/{id}/refresh", post(refresh_account));

    // 租户管理（仅 Admin）
    let admin_tenant_routes = Router::new().route("/api/v1/tenants", get(list_tenants));

    // 系统设置（仅 Admin）
    let admin_settings_routes = Router::new()
        .route(
            "/api/v1/settings",
            get(get_system_settings).put(update_system_settings),
        )
        .route(
            "/api/v1/settings/{key}",
            get(get_system_setting_by_key).put(update_system_setting_by_key),
        );

    // 公开设置（无需认证）
    let public_settings_routes =
        Router::new().route("/api/v1/settings/public", get(get_public_settings));

    // Distribution 分销管理（仅 Admin）
    let admin_distribution_routes = Router::new()
        .route(
            "/api/v1/distribution/records",
            get(list_distribution_records),
        )
        .route("/api/v1/distribution/stats", get(get_distribution_stats))
        .route(
            "/api/v1/distribution/rules",
            get(list_distribution_rules).post(create_distribution_rule),
        )
        .route(
            "/api/v1/distribution/rules/{id}",
            put(update_distribution_rule).delete(delete_distribution_rule),
        );

    // 定价管理（仅 Admin）
    let admin_pricing_routes = Router::new()
        .route("/api/v1/pricing", get(list_pricing).post(create_pricing))
        .route(
            "/api/v1/pricing/{id}",
            put(update_pricing).delete(delete_pricing),
        )
        .route(
            "/api/v1/pricing/{id}/make-default",
            post(make_pricing_default),
        )
        .route("/api/v1/pricing/calculate", post(calculate_cost));

    // Node Gateway 管理（仅 Admin）
    let admin_node_gateway_routes = Router::new()
        .route(
            "/api/v1/admin/node-gateway/overview",
            get(get_node_gateway_overview),
        )
        .route("/api/v1/admin/nodes/{id}/recover", post(recover_node))
        .route("/api/v1/admin/nodes/{id}/exclude", post(exclude_node))
        .route(
            "/api/v1/admin/nodes/{id}/revoke-token",
            post(revoke_node_token),
        )
        .route("/api/v1/admin/nodes/{id}", delete(delete_node))
        // 节点注册 token 审批
        .route(
            "/api/v1/admin/node-gateway/tokens/pending",
            get(admin_list_pending_tokens),
        )
        .route(
            "/api/v1/admin/node-gateway/tokens/{id}/approve",
            post(admin_approve_token),
        );

    // 小费管理（仅 Admin）
    let admin_tips_routes = Router::new()
        .route(
            "/api/v1/admin/tips/withdrawals/pending",
            get(admin_list_pending_withdrawals),
        )
        .route(
            "/api/v1/admin/tips/withdrawals/{id}/approve",
            post(admin_approve_withdrawal),
        )
        .route(
            "/api/v1/admin/tips/withdrawals/{id}/complete",
            post(admin_complete_withdrawal),
        )
        .route(
            "/api/v1/admin/tips/settings/ratio",
            get(admin_get_tip_ratio).put(admin_update_tip_ratio),
        );

    // 监控追踪（仅 Admin）
    let admin_monitoring_routes = Router::new().route(
        "/api/v1/admin/monitoring/overview",
        get(get_monitoring_overview),
    );

    // 合并管理路由并添加认证和限流中间件
    // 注意：中间件执行顺序是反向的，所以先添加 rate_limit，再添加 admin_auth
    // 实际执行顺序：admin_auth_middleware -> rate_limit_middleware -> handler
    let admin_routes = admin_user_routes
        .merge(admin_account_routes)
        .merge(admin_tenant_routes)
        .merge(admin_settings_routes)
        .merge(admin_distribution_routes)
        .merge(admin_pricing_routes)
        .merge(admin_node_gateway_routes)
        .merge(admin_tips_routes)
        .merge(admin_monitoring_routes)
        // 先添加限流层（后执行）
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware))
        // 再添加 Admin 认证层（先执行），统一保护所有 Admin 路由
        .layer(from_fn_with_state(state.clone(), admin_auth_middleware));

    // ==================== 6. 定价和账单 API ====================
    let billing_routes = Router::new()
        // 账单记录（用户看自己的，Admin 看所有）
        .route("/api/v1/billing/records", get(list_billing_records))
        .route("/api/v1/billing/stats", get(get_billing_stats))
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware));

    // ==================== 7. 调试接口（仅 Admin 使用） ====================
    let debug_routes = Router::new()
        .route("/api/v1/debug/routing", get(debug_routing))
        .route("/api/v1/debug/providers", get(get_provider_health))
        .route("/api/v1/debug/providers/reset", post(reset_health))
        .route(
            "/api/v1/debug/accounts/{account_id}/cooldown",
            post(set_account_cooldown),
        )
        .route("/api/v1/debug/gateway/status", get(get_gateway_status))
        .route("/api/v1/debug/gateway/stats", get(get_execution_stats))
        .route("/api/v1/debug/gateway/health", post(check_provider_health))
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(from_fn_with_state(state.clone(), admin_auth_middleware));

    // ==================== 8. 支付 API ====================
    // 用户支付路由（需要认证 + 限流）
    let payment_routes = Router::new()
        // 创建支付订单（支持跳转支付和扫码支付）
        .route(
            "/api/v1/payments/orders",
            post(create_payment_order).get(list_my_payment_orders),
        )
        // 获取订单详情
        .route("/api/v1/payments/orders/{id}", get(get_payment_order))
        .route("/api/v1/payments/methods", get(list_payment_methods))
        // 同步订单状态
        .route(
            "/api/v1/payments/sync/{out_trade_no}",
            post(sync_payment_order),
        )
        .route(
            "/api/v1/payments/orders/{id}/sync",
            post(sync_payment_order),
        )
        // 获取我的余额
        .route("/api/v1/payments/balance", get(get_my_balance))
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware));

    // 支付宝异步通知（不需要认证）
    let payment_notify_routes = Router::new()
        .route("/api/v1/payments/notify/alipay", post(alipay_notify))
        .route("/api/v1/payments/notify/wechatpay", post(wechatpay_notify))
        .layer(from_fn_with_state(
            state.clone(),
            payment_notify_rate_limit_middleware,
        ));

    // 管理员支付路由
    let admin_payment_routes = Router::new()
        .route(
            "/api/v1/admin/payments/providers",
            get(admin_payment_providers),
        )
        .route(
            "/api/v1/admin/payments/providers/{method}/verify",
            post(admin_verify_payment_provider),
        )
        .route(
            "/api/v1/admin/payments/orders",
            get(admin_list_payment_orders),
        )
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(from_fn_with_state(state.clone(), admin_auth_middleware));

    // ==================== 9. 健康检查（公开） ====================
    let health_routes = Router::new().route("/health", get(health_check));

    // ==================== 10. 节点网关 API（使用 session token 认证） ====================
    let node_routes = Router::new()
        .route("/node/v1/register", post(node_register))
        .route("/node/v1/heartbeat", post(node_heartbeat))
        .route("/node/v1/tasks/poll", post(node_poll))
        .route("/node/v1/tasks/{task_id}/complete", post(node_complete));

    // ==================== 合并所有路由 ====================
    Router::new()
        .merge(auth_routes)
        .merge(openai_routes)
        .merge(anthropic_routes)
        .merge(user_routes)
        .merge(admin_routes)
        .merge(billing_routes)
        .merge(debug_routes)
        .merge(payment_routes)
        .merge(payment_notify_routes)
        .merge(admin_payment_routes)
        .merge(health_routes)
        .merge(node_routes)
        .merge(public_settings_routes) // 公开设置路由
        // 维护模式中间件（位于 anthropic 错误转换之内、业务层之前；
        // 其 503 拒绝会被更外层的 anthropic_error_response_middleware 转换）
        .layer(from_fn_with_state(
            state.clone(),
            maintenance_mode_middleware,
        ))
        // 仅 `/v1/messages` 的非 2xx 响应会被转换，其他 API 保持既有错误格式。
        // 放在维护模式之外，以覆盖全局维护拒绝。
        .layer(axum::middleware::from_fn(
            anthropic_error_response_middleware,
        ))
        .layer(axum::middleware::from_fn(request_logger))
        .layer(axum::middleware::from_fn(trace_id_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        middleware::from_fn,
        routing::post,
    };
    use serde_json::Value;
    use tower::ServiceExt;

    #[test]
    fn test_create_router() {
        let state = AppState::new();
        let router = create_router(state);
        // 确保可以创建路由器
        let _ = router;
    }

    #[tokio::test]
    async fn anthropic_body_limit_allows_payloads_larger_than_axum_default() {
        let app = Router::new()
            .route(
                "/v1/messages",
                post(|_: Json<Value>| async { StatusCode::NO_CONTENT }),
            )
            .layer(DefaultBodyLimit::max(ANTHROPIC_MESSAGES_BODY_LIMIT_BYTES));
        let payload = format!(r#"{{"content":"{}"}}"#, "x".repeat(2 * 1024 * 1024));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn anthropic_body_limit_returns_anthropic_request_too_large_error() {
        let app = Router::new()
            .route(
                "/v1/messages",
                post(|_: Json<Value>| async { StatusCode::NO_CONTENT }),
            )
            .layer(DefaultBodyLimit::max(ANTHROPIC_MESSAGES_BODY_LIMIT_BYTES))
            .layer(from_fn(anthropic_error_response_middleware));
        let payload = format!(
            r#"{{"content":"{}"}}"#,
            "x".repeat(ANTHROPIC_MESSAGES_BODY_LIMIT_BYTES)
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "request_too_large");
    }

    #[tokio::test]
    async fn messages_route_wraps_x_api_key_authentication_errors() {
        let app = create_router(AppState::new());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .header("anthropic-version", "2023-06-01")
                    .header("x-api-key", "sk-unconfigured-test-key")
                    .body(Body::from(
                        r#"{"model":"claude-test","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "authentication_error");
    }
}
