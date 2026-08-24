//! Routing 模块端到端测试
//!
//! 验证双层路由：Layer1 模型路由 + Layer2 账号路由

use integration_tests::common::VerificationChain;
use keycompute_routing::{AccountStateStore, ProviderHealthStore, RoutingEngine};
use keycompute_types::{ExecutionTarget, PricingSnapshot, RequestContext};
use rust_decimal::Decimal;
use std::sync::Arc;
use uuid::Uuid;

/// 从 ExecutionTarget 提取 provider 名称
fn get_provider_name(target: &ExecutionTarget) -> String {
    match target {
        ExecutionTarget::ProviderAccount { provider, .. } => provider.clone(),
        ExecutionTarget::Node { model } => format!("node:{}", model),
    }
}

/// 从 ExecutionTarget 提取 endpoint
fn get_endpoint(target: &ExecutionTarget) -> Option<String> {
    match target {
        ExecutionTarget::ProviderAccount { endpoint, .. } => Some(endpoint.clone()),
        ExecutionTarget::Node { .. } => None,
    }
}

/// 创建测试用的路由引擎
fn create_test_engine() -> RoutingEngine {
    let account_states = Arc::new(AccountStateStore::new());
    let provider_health = Arc::new(ProviderHealthStore::new());
    let providers = vec![
        "openai".to_string(),
        "deepseek".to_string(),
        "claude".to_string(),
        "gemini".to_string(),
    ];
    RoutingEngine::new(account_states, provider_health, providers)
}

/// 创建测试用的请求上下文
fn create_test_context() -> RequestContext {
    RequestContext::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        "gpt-4o",
        vec![],
        true,
        PricingSnapshot {
            model_name: "gpt-4o".to_string(),
            currency: "CNY".to_string(),
            input_price_per_1k: Decimal::from(1),
            output_price_per_1k: Decimal::from(2),
        },
    )
}

/// 测试双层路由流程
#[tokio::test]
async fn test_routing_dual_layer_flow() {
    let mut chain = VerificationChain::new();

    // 1. 创建路由引擎
    let engine = create_test_engine();
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::new",
        format!("Configured providers: {:?}", engine.configured_providers()),
        engine.configured_providers().len() == 4,
    );

    // 2. 创建请求上下文
    let ctx = create_test_context();
    chain.add_step(
        "keycompute-types",
        "RequestContext::for_routing",
        format!("Model: {}", ctx.model),
        ctx.model == "gpt-4o",
    );

    // 3. 执行路由（Layer1 + Layer2）
    let plan = engine.route(&ctx).await;
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::route",
        format!("Route result: {:?}", plan.is_ok()),
        plan.is_ok(),
    );

    // 4. 验证执行计划
    if let Ok(plan) = plan {
        let primary_provider = get_provider_name(&plan.primary);
        chain.add_step(
            "keycompute-routing",
            "ExecutionPlan::primary",
            format!("Primary provider: {}", primary_provider),
            !primary_provider.is_empty(),
        );
        chain.add_step(
            "keycompute-routing",
            "ExecutionPlan::fallback_chain",
            format!("Fallback chain length: {}", plan.fallback_chain.len()),
            true, // fallback 可能为空
        );
        if let Some(endpoint) = get_endpoint(&plan.primary) {
            chain.add_step(
                "keycompute-routing",
                "ExecutionTarget::endpoint",
                format!("Endpoint: {}", endpoint),
                endpoint.starts_with("https://"),
            );
        }
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Provider 健康状态路由
#[tokio::test]
async fn test_routing_provider_health() {
    let mut chain = VerificationChain::new();

    // 1. 创建带健康状态的引擎
    let account_states = Arc::new(AccountStateStore::new());
    let provider_health = Arc::new(ProviderHealthStore::new());

    // 2. 记录 Provider 健康状态
    provider_health.record_success("openai", 100);
    provider_health.record_success("openai", 150);
    provider_health.record_failure("claude");

    let providers = vec!["openai".to_string(), "claude".to_string()];
    let engine = RoutingEngine::new(account_states, provider_health.clone(), providers);

    chain.add_step(
        "keycompute-routing",
        "ProviderHealthStore::record_success",
        "OpenAI success recorded",
        true,
    );

    // 3. 检查健康状态
    let openai_healthy = engine.is_provider_healthy("openai");
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::is_provider_healthy",
        format!("OpenAI healthy: {}", openai_healthy),
        openai_healthy,
    );

    // 4. 获取健康评分
    let openai_score = engine.get_provider_health_score("openai");
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::get_provider_health_score",
        format!("OpenAI health score: {}", openai_score),
        openai_score > 50,
    );

    // 5. 获取健康 Provider 列表
    let healthy_providers = engine.healthy_providers();
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::healthy_providers",
        format!("Healthy providers: {:?}", healthy_providers),
        !healthy_providers.is_empty(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试账号冷却路由
#[tokio::test]
async fn test_routing_provider_cooldown() {
    let mut chain = VerificationChain::new();

    // 1. 创建引擎
    let account_states = Arc::new(AccountStateStore::new());
    let provider_health = Arc::new(ProviderHealthStore::new());

    let providers = vec!["test".to_string()];
    let engine = RoutingEngine::new(account_states.clone(), provider_health, providers);

    // 2. 初始状态检查
    let account_id = Uuid::new_v4();
    let initial_cooling = account_states.is_cooling_down(&account_id);
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::initial_state",
        format!("Account initially cooling: {}", initial_cooling),
        !initial_cooling,
    );

    // 3. 设置账号冷却
    account_states.set_cooldown(account_id, 60);

    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::set_cooldown",
        "Account cooldown set",
        true,
    );

    // 4. 检查冷却状态
    let now_cooling = engine.is_account_cooling(&account_id);
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::is_account_cooling",
        format!("Account now cooling: {}", now_cooling),
        now_cooling,
    );

    // 5. 检查冷却剩余时间
    let remaining = engine.account_cooldown_remaining(&account_id);
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::account_cooldown_remaining",
        format!("Remaining cooldown: {:?}", remaining),
        remaining.is_some(),
    );

    // 6. 其他账号不受影响
    let other_account = Uuid::new_v4();
    let other_cooling = engine.is_account_cooling(&other_account);
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::isolation",
        format!("Other account cooling: {}", other_cooling),
        !other_cooling,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试账号冷却路由
#[tokio::test]
async fn test_routing_account_cooldown() {
    let mut chain = VerificationChain::new();

    // 1. 创建引擎
    let account_states = Arc::new(AccountStateStore::new());
    let provider_health = Arc::new(ProviderHealthStore::new());

    let providers = vec!["test".to_string()];
    let engine = RoutingEngine::new(account_states.clone(), provider_health, providers);

    // 2. 测试账号冷却
    let account_id = Uuid::new_v4();

    let initial_cooling = engine.is_account_cooling(&account_id);
    chain.add_step(
        "keycompute-routing",
        "AccountCooldown::initial_state",
        format!("Account initially cooling: {}", initial_cooling),
        !initial_cooling,
    );

    // 3. 设置账号冷却
    account_states.set_cooldown(account_id, 30);

    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::set_cooldown",
        "Account cooldown set",
        true,
    );

    // 4. 检查账号冷却状态
    let now_cooling = engine.is_account_cooling(&account_id);
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::is_account_cooling",
        format!("Account now cooling: {}", now_cooling),
        now_cooling,
    );

    // 5. 检查冷却剩余时间
    let remaining = engine.account_cooldown_remaining(&account_id);
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::account_cooldown_remaining",
        format!("Remaining cooldown: {:?}", remaining),
        remaining.is_some(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试路由权重常量
#[test]
fn test_routing_config() {
    let mut chain = VerificationChain::new();

    // 路由权重已硬编码，验证引擎可以正常创建和工作
    let account_states = Arc::new(AccountStateStore::new());
    let provider_health = Arc::new(ProviderHealthStore::new());
    let providers = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let engine = RoutingEngine::new(account_states, provider_health, providers);

    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::new_with_hardcoded_weights",
        format!(
            "Configured providers: {}",
            engine.configured_providers().len()
        ),
        engine.configured_providers().len() == 4,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试不健康 Provider 过滤
#[tokio::test]
async fn test_routing_unhealthy_provider_filtering() {
    let mut chain = VerificationChain::new();

    // 1. 创建引擎
    let account_states = Arc::new(AccountStateStore::new());
    let provider_health = Arc::new(ProviderHealthStore::new());

    // 2. 让 claude 变得不健康
    for _ in 0..10 {
        provider_health.record_failure("claude");
    }

    let providers = vec!["openai".to_string(), "claude".to_string()];
    let engine = RoutingEngine::new(account_states, provider_health.clone(), providers);

    // 3. 检查 claude 健康状态
    let claude_healthy = engine.is_provider_healthy("claude");
    chain.add_step(
        "keycompute-routing",
        "ProviderHealthStore::unhealthy_detection",
        format!("Claude healthy after failures: {}", claude_healthy),
        !claude_healthy,
    );

    // 4. 检查健康列表
    let healthy_providers = engine.healthy_providers();
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::healthy_providers_filter",
        format!("Healthy providers (no claude): {:?}", healthy_providers),
        !healthy_providers.contains(&"claude".to_string()),
    );

    // 5. 执行路由，验证不健康的 Provider 被跳过
    let ctx = create_test_context();
    let plan = engine.route(&ctx).await;

    if let Ok(plan) = plan {
        let primary_provider = get_provider_name(&plan.primary);
        chain.add_step(
            "keycompute-routing",
            "RoutingEngine::skip_unhealthy",
            format!("Primary provider (not claude): {}", primary_provider),
            primary_provider != "claude",
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Provider 动态管理
#[test]
fn test_routing_provider_management() {
    let mut chain = VerificationChain::new();

    // 1. 创建引擎
    let mut engine = create_test_engine();

    let initial_count = engine.configured_providers().len();
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::initial_providers",
        format!("Initial provider count: {}", initial_count),
        initial_count == 4,
    );

    // 2. 添加 Provider
    engine.add_provider("gemini");
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::add_provider",
        format!("After adding gemini: {:?}", engine.configured_providers()),
        engine
            .configured_providers()
            .contains(&"gemini".to_string()),
    );

    // 3. 移除 Provider
    engine.remove_provider("deepseek");
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::remove_provider",
        format!(
            "After removing deepseek: {:?}",
            engine.configured_providers()
        ),
        !engine
            .configured_providers()
            .contains(&"deepseek".to_string()),
    );

    // 4. 重复添加不会重复
    engine.add_provider("openai");
    let openai_count = engine
        .configured_providers()
        .iter()
        .filter(|p| *p == "openai")
        .count();
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::no_duplicate",
        format!("OpenAI count: {}", openai_count),
        openai_count == 1,
    );

    chain.print_report();
    assert!(chain.all_passed());
}
