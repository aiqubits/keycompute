//! 路由调试处理器
//!
//! 用于查看路由决策过程和状态

use crate::{
    error::{ApiError, Result},
    extractors::AuthExtractor,
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use keycompute_types::{ExecutionTarget, RequestContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// 从 ExecutionTarget 提取路由目标信息
fn extract_target_info(target: &ExecutionTarget) -> Option<RoutingTargetInfo> {
    match target {
        ExecutionTarget::ProviderAccount {
            provider,
            account_id,
            endpoint,
            ..
        } => Some(RoutingTargetInfo {
            provider: provider.clone(),
            account_id: *account_id,
            endpoint: endpoint.clone(),
        }),
        ExecutionTarget::Node { .. } => None, // Node 路径不支持此接口
    }
}

/// 路由调试请求
#[derive(Debug, Deserialize)]
pub struct RoutingDebugQuery {
    /// 模型名称
    pub model: String,
    /// 入口协议（openai / anthropic），默认 openai；
    /// 用于按入口协议隔离模拟路由（Anthropic 入口只从 anthropic 协议选路）
    #[serde(default)]
    pub entry: Option<String>,
}

/// 路由目标信息
#[derive(Debug, Serialize)]
pub struct RoutingTargetInfo {
    /// Provider 名称
    pub provider: String,
    /// 账号 ID
    pub account_id: Uuid,
    /// 端点
    pub endpoint: String,
}

/// Provider 状态信息
#[derive(Debug, Serialize)]
pub struct ProviderStatusInfo {
    /// Provider 名称
    pub provider: String,
    /// 是否健康
    pub is_healthy: bool,
    /// 账号数量
    pub account_count: usize,
    /// 状态描述
    pub status: String,
}

/// 路由调试响应
#[derive(Debug, Serialize)]
pub struct RoutingDebugResponse {
    /// 请求 ID
    pub request_id: Uuid,
    /// 是否成功路由
    pub routed: bool,
    /// 主目标（路由成功时有值）
    pub primary: Option<RoutingTargetInfo>,
    /// 备用链路（路由成功时有值）
    pub fallback_chain: Vec<RoutingTargetInfo>,
    /// 使用的定价
    pub pricing: PricingInfo,
    /// 所有 Provider 状态（用于诊断）
    pub provider_status: Vec<ProviderStatusInfo>,
    /// 提示信息
    pub message: Option<String>,
}

/// 定价信息
#[derive(Debug, Serialize)]
pub struct PricingInfo {
    /// 模型名称
    pub model_name: String,
    /// 货币
    pub currency: String,
    /// 输入价格（每 1K tokens）
    pub input_price_per_1k: String,
    /// 输出价格（每 1K tokens）
    pub output_price_per_1k: String,
}

/// 按 debug 查询的 entry 参数模拟入口协议（与真实 handler 行为一致）：
/// `entry=anthropic`（大小写不敏感）时设置原生 Anthropic 请求体，使
/// route() 按 anthropic 协议隔离选路；其余取值（含缺省）保持 openai。
///
/// 提取为纯函数便于单元测试：handler 级测试需构造完整 AppState，
/// 成本高且不必要。
///
/// 注意：此处注入的哨兵 body（空 messages）仅用于让 route() 判定入口
/// 协议；route() 当前只检查 native_anthropic_request 是否存在、不解析
/// 内容。若将来 route() 开始读取请求体，此哨兵必须替换为真实结构。
fn apply_debug_entry_protocol(ctx: &mut RequestContext, entry: Option<&str>) {
    if entry.is_some_and(|e| e.eq_ignore_ascii_case("anthropic")) {
        ctx.native_anthropic_request = Some(Arc::new(serde_json::json!({ "messages": [] })));
    }
}

/// 路由失败时的诊断提示文案（区分 node 模型与普通模型的排查路径）。
///
/// NoReadyNode 与 RoutingFailed 的排查路径不同：node 模型看节点是否
/// 在线，普通模型看账号/Provider 配置，提示需区分。提取为纯函数便于
/// 单元测试。
fn routing_debug_failure_message(model: &str) -> String {
    if model.starts_with("node:") {
        format!(
            "模型 '{}' 暂时没有在线节点。请检查：1) 节点是否已注册并在线；2) 节点会话是否有效。",
            model
        )
    } else {
        format!(
            "模型 '{}' 没有可用的路由目标。请检查：1) 是否已配置对应 Provider 的账号；2) Provider 是否健康；3) 模型名称是否正确。",
            model
        )
    }
}

/// 路由调试接口
///
/// 模拟一个请求，返回路由决策结果（不实际执行）
/// 即使路由失败也会返回 200，包含详细的诊断信息
pub async fn debug_routing(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthExtractor>,
    Query(query): Query<RoutingDebugQuery>,
) -> Result<Json<RoutingDebugResponse>> {
    use keycompute_db::models::Account;

    // 1. 构建 PricingSnapshot
    // Node 模型（node:前缀）使用 empty provider，其他使用 openai
    let provider = keycompute_pricing::resolve_pricing_provider(&query.model);
    let pricing = state
        .pricing
        .create_snapshot(&query.model, &auth.tenant_id, Some(provider))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create pricing snapshot: {}", e)))?;

    // 2. 构建模拟的 RequestContext
    let mut ctx = RequestContext::new(
        Uuid::new_v4(),
        auth.user_id,
        auth.tenant_id,
        auth.produce_ai_key_id,
        query.model.clone(),
        vec![],
        true,
        pricing.clone(),
    );

    // 入口协议隔离：按 entry 参数模拟对应入口的路由（与真实 handler 行为一致）
    apply_debug_entry_protocol(&mut ctx, query.entry.as_deref());

    // 3. 获取所有配置的 provider 列表
    let all_providers: Vec<String> = state.routing.configured_providers().to_vec();
    let healthy_providers = state.routing.healthy_providers();
    let healthy_set: std::collections::HashSet<String> =
        healthy_providers.iter().cloned().collect();

    // 4. 查询每个 provider 的账号数量
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    // 获取所有启用的账号
    let all_accounts = Account::find_enabled_all(pool).await.unwrap_or_default();

    // 按 provider 统计账号数量
    let mut provider_account_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for account in &all_accounts {
        *provider_account_counts
            .entry(account.provider.clone())
            .or_insert(0) += 1;
    }

    let mut provider_status = Vec::new();
    for provider in all_providers {
        let is_healthy = healthy_set.contains(&provider);
        let account_count = provider_account_counts.get(&provider).copied().unwrap_or(0);

        let status = if account_count == 0 {
            "未配置账号".to_string()
        } else if !is_healthy {
            "Provider 不健康".to_string()
        } else {
            format!("{} 个账号", account_count)
        };

        provider_status.push(ProviderStatusInfo {
            provider: provider.clone(),
            is_healthy,
            account_count,
            status,
        });
    }

    // 5. 执行路由（只读）
    match state.routing.route(&ctx).await {
        Ok(plan) => {
            // 提取 primary target 信息
            let primary_info = match extract_target_info(&plan.primary) {
                Some(info) => info,
                None => {
                    return Err(ApiError::Internal(
                        "Node execution not supported in routing debug".into(),
                    ));
                }
            };

            // 路由成功，根据实际 provider 更新定价
            state
                .pricing
                .update_context_pricing(&mut ctx, &primary_info.provider)
                .await;

            // 使用更新后的定价信息
            let pricing = &ctx.pricing_snapshot;

            // 提取 fallback chain 信息
            let fallback_chain: Vec<RoutingTargetInfo> = plan
                .fallback_chain
                .iter()
                .filter_map(extract_target_info)
                .collect();

            // 路由成功
            let response = RoutingDebugResponse {
                request_id: ctx.request_id,
                routed: true,
                primary: Some(primary_info),
                fallback_chain,
                pricing: PricingInfo {
                    model_name: pricing.model_name.clone(),
                    currency: pricing.currency.clone(),
                    input_price_per_1k: pricing.input_price_per_1k.to_string(),
                    output_price_per_1k: pricing.output_price_per_1k.to_string(),
                },
                provider_status,
                message: None,
            };

            let primary_provider = response
                .primary
                .as_ref()
                .map(|p| p.provider.clone())
                .unwrap_or_default();
            tracing::info!(
                request_id = %ctx.request_id,
                primary_provider = %primary_provider,
                fallback_count = response.fallback_chain.len(),
                "Routing debug completed"
            );

            Ok(Json(response))
        }
        Err(
            keycompute_types::KeyComputeError::RoutingFailed(_)
            | keycompute_types::KeyComputeError::NoReadyNode(_),
        ) => {
            // 路由失败，但仍返回诊断信息
            let message = routing_debug_failure_message(&query.model);
            let response = RoutingDebugResponse {
                request_id: ctx.request_id,
                routed: false,
                primary: None,
                fallback_chain: vec![],
                pricing: PricingInfo {
                    model_name: pricing.model_name,
                    currency: pricing.currency,
                    input_price_per_1k: pricing.input_price_per_1k.to_string(),
                    output_price_per_1k: pricing.output_price_per_1k.to_string(),
                },
                provider_status,
                message: Some(message),
            };

            tracing::warn!(
                request_id = %ctx.request_id,
                model = %query.model,
                "Routing debug failed: no available targets"
            );

            Ok(Json(response))
        }
        Err(e) => Err(ApiError::Internal(format!("Routing failed: {}", e))),
    }
}

/// Provider 健康状态响应
#[derive(Debug, Serialize)]
pub struct ProviderHealthResponse {
    /// 可用 Provider 列表
    pub healthy_providers: Vec<String>,
    /// 账号状态存储中的账号数量
    pub account_count: usize,
}

/// 获取 Provider 健康状态
pub async fn get_provider_health(
    State(state): State<AppState>,
) -> Result<Json<ProviderHealthResponse>> {
    let providers = state.routing.healthy_providers().to_vec();
    let account_count = state.account_states.all_states().len();

    Ok(Json(ProviderHealthResponse {
        healthy_providers: providers,
        account_count,
    }))
}

/// 重置健康状态响应
#[derive(Debug, Serialize)]
pub struct ResetHealthResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
}

/// 重置 Provider 健康状态和冷却状态
///
/// 用于调试，清除所有 Provider 和账号的健康状态和冷却状态
pub async fn reset_health(State(state): State<AppState>) -> Result<Json<ResetHealthResponse>> {
    // 重置所有 Provider 的健康状态
    let providers = state.routing.configured_providers();
    for provider in providers {
        state.provider_health.reset_stats(provider);
    }

    // 清除所有账号冷却
    for (account_id, _) in state.account_states.cooling_accounts() {
        state.account_states.clear_cooldown(account_id);
    }

    tracing::info!("Health and cooldown state reset for all providers");

    Ok(Json(ResetHealthResponse {
        success: true,
        message: "All provider health and cooldown states have been reset".to_string(),
    }))
}

/// 设置指定账号的冷却状态请求
#[derive(Debug, Deserialize)]
pub struct SetAccountCooldownRequest {
    /// 冷却持续时间（秒），默认 60 秒
    pub duration_secs: Option<u64>,
}

/// 设置指定账号的冷却状态响应
#[derive(Debug, Serialize)]
pub struct SetAccountCooldownResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 账号 ID
    pub account_id: String,
    /// 冷却持续时间（秒）
    pub duration_secs: u64,
}

/// 设置指定账号的冷却状态
///
/// POST /api/v1/debug/accounts/{account_id}/cooldown
///
/// 手动触发指定账号进入冷却状态，用于临时禁用某个账号
pub async fn set_account_cooldown(
    State(state): State<AppState>,
    Path(account_id): Path<Uuid>,
    Json(req): Json<SetAccountCooldownRequest>,
) -> Result<Json<SetAccountCooldownResponse>> {
    let duration_secs = req.duration_secs.unwrap_or(60);

    // 设置账号冷却状态
    state.account_states.set_cooldown(account_id, duration_secs);

    tracing::info!(
        account_id = %account_id,
        duration_secs = duration_secs,
        "Account cooldown set via API"
    );

    Ok(Json(SetAccountCooldownResponse {
        success: true,
        message: format!(
            "Account {} entered cooldown state for {} seconds",
            account_id, duration_secs
        ),
        account_id: account_id.to_string(),
        duration_secs,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_debug_query_deserialize() {
        // 缺失 entry 时默认 None（按 openai 协议模拟）
        let json = r#"{"model": "gpt-4o"}"#;
        let query: RoutingDebugQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.model, "gpt-4o");
        assert!(query.entry.is_none());

        // entry 显式指定入口协议
        let json = r#"{"model": "gpt-4o", "entry": "anthropic"}"#;
        let query: RoutingDebugQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.entry.as_deref(), Some("anthropic"));
    }

    #[test]
    fn apply_debug_entry_protocol_sets_native_request_only_for_anthropic() {
        let make_ctx = || {
            RequestContext::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                "gpt-4o".to_string(),
                vec![],
                true,
                keycompute_types::PricingSnapshot::default(),
            )
        };

        // 缺省或 openai：保持 openai 入口（不设置原生请求体）
        let mut ctx = make_ctx();
        apply_debug_entry_protocol(&mut ctx, None);
        assert!(ctx.native_anthropic_request.is_none());
        let mut ctx = make_ctx();
        apply_debug_entry_protocol(&mut ctx, Some("openai"));
        assert!(ctx.native_anthropic_request.is_none());
        let mut ctx = make_ctx();
        apply_debug_entry_protocol(&mut ctx, Some("unknown"));
        assert!(ctx.native_anthropic_request.is_none());

        // anthropic（大小写不敏感）：模拟 anthropic 入口
        let mut ctx = make_ctx();
        apply_debug_entry_protocol(&mut ctx, Some("anthropic"));
        assert!(ctx.native_anthropic_request.is_some());
        let mut ctx = make_ctx();
        apply_debug_entry_protocol(&mut ctx, Some("ANTHROPIC"));
        assert!(ctx.native_anthropic_request.is_some());
    }

    #[test]
    fn routing_debug_failure_message_distinguishes_node_models() {
        // node 模型：提示检查节点在线状态
        let node_msg = routing_debug_failure_message("node:llama3");
        assert!(node_msg.contains("node:llama3"));
        assert!(node_msg.contains("在线节点"));
        assert!(!node_msg.contains("路由目标"));

        // 普通模型：提示检查账号/Provider 配置
        let model_msg = routing_debug_failure_message("gpt-4o");
        assert!(model_msg.contains("gpt-4o"));
        assert!(model_msg.contains("路由目标"));
        assert!(!model_msg.contains("在线节点"));
    }

    #[test]
    fn test_routing_target_info_serialize() {
        let info = RoutingTargetInfo {
            provider: "openai".to_string(),
            account_id: Uuid::new_v4(),
            endpoint: "https://api.openai.com/v1".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("openai"));
    }
}
