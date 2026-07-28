//! Gateway 模块端到端测试
//!
//! 验证执行网关：重试、Fallback、流处理

use integration_tests::common::VerificationChain;
use integration_tests::mocks::provider::MockProviderFactory;
use llm_gateway::retry::RetryState;
use llm_gateway::{FailoverManager, GatewayBuilder, GatewayConfig, RetryPolicy};
use llm_protocol_provider::ProviderAdapter;
use std::sync::Arc;
use std::time::Duration;

/// 测试 Gateway 基础配置
#[test]
fn test_gateway_config() {
    let mut chain = VerificationChain::new();

    // 1. 默认配置
    let config = GatewayConfig::default();
    chain.add_step(
        "llm-gateway",
        "GatewayConfig::default_max_retries",
        format!("Default max_retries: {}", config.max_retries),
        config.max_retries == 3,
    );
    chain.add_step(
        "llm-gateway",
        "GatewayConfig::default_timeout",
        format!("Default timeout_secs: {}", config.timeout_secs),
        config.timeout_secs == 120,
    );

    // 2. 构建器模式
    let _builder = GatewayBuilder::new();
    chain.add_step(
        "llm-gateway",
        "GatewayBuilder::new",
        "Gateway builder created",
        true,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试重试策略
#[test]
fn test_gateway_retry_policy() {
    let mut chain = VerificationChain::new();

    // 1. 默认重试策略
    let policy = RetryPolicy::default();
    chain.add_step(
        "llm-gateway",
        "RetryPolicy::default_max_retries",
        format!("Max retries: {}", policy.max_retries),
        policy.max_retries == 3,
    );
    chain.add_step(
        "llm-gateway",
        "RetryPolicy::default_initial_backoff",
        format!("Initial backoff: {}ms", policy.initial_backoff_ms),
        policy.initial_backoff_ms == 100,
    );

    // 2. 重试状态
    let state = RetryState::new(policy);
    chain.add_step(
        "llm-gateway",
        "RetryState::new",
        format!("Initial attempt: {}", state.attempt),
        state.attempt == 0,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试退避策略
#[test]
fn test_gateway_backoff() {
    let mut chain = VerificationChain::new();

    let policy = RetryPolicy::default();

    // 1. 第一次退避
    let delay0 = policy.backoff_duration(0);
    chain.add_step(
        "llm-gateway",
        "RetryPolicy::backoff_0",
        format!("Backoff at attempt 0: {:?}", delay0),
        delay0 == Duration::from_millis(0),
    );

    // 2. 第一次退避
    let delay1 = policy.backoff_duration(1);
    chain.add_step(
        "llm-gateway",
        "RetryPolicy::backoff_1",
        format!("Backoff at attempt 1: {:?}", delay1),
        delay1 >= Duration::from_millis(100),
    );

    // 3. 第二次退避
    let delay2 = policy.backoff_duration(2);
    chain.add_step(
        "llm-gateway",
        "RetryPolicy::backoff_2",
        format!("Backoff at attempt 2: {:?}", delay2),
        delay2 >= delay1,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Failover 管理
#[test]
fn test_gateway_failover() {
    use keycompute_types::ExecutionTarget;
    use uuid::Uuid;

    let mut chain = VerificationChain::new();

    // 1. 创建 Failover 管理器
    let manager = FailoverManager::new();
    chain.add_step(
        "llm-gateway",
        "FailoverManager::new",
        "Failover manager created",
        true,
    );

    // 2. 创建测试 targets
    let targets = vec![
        ExecutionTarget::new_provider("openai", Uuid::new_v4(), "https://api.openai.com", "key1"),
        ExecutionTarget::new_provider(
            "claude",
            Uuid::new_v4(),
            "https://api.anthropic.com",
            "key2",
        ),
    ];

    // 3. 选择下一个 target
    let next = manager.select_next(&targets, 0);
    let next_provider = match &next {
        Some(ExecutionTarget::ProviderAccount { provider, .. }) => Some(provider.clone()),
        Some(ExecutionTarget::Node { model }) => Some(format!("node:{}", model)),
        None => None,
    };
    chain.add_step(
        "llm-gateway",
        "FailoverManager::select_next",
        format!("Next provider: {:?}", next_provider),
        next_provider == Some("claude".to_string()),
    );

    // 4. 从最后一个选择
    let none = manager.select_next(&targets, 1);
    chain.add_step(
        "llm-gateway",
        "FailoverManager::select_next_last",
        format!("Next after last: {:?}", none),
        none.is_none(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Provider 流执行
#[tokio::test]
async fn test_gateway_provider_stream() {
    use futures::StreamExt;

    let mut chain = VerificationChain::new();

    // 1. 创建模拟 Provider
    let provider = Arc::new(MockProviderFactory::create_openai());
    chain.add_step(
        "integration-tests::mocks",
        "MockProvider::create_openai",
        format!("Provider: {}", provider.name()),
        provider.name() == "openai",
    );

    // 2. 构建请求
    let request =
        llm_protocol_provider::UpstreamRequest::new("http://mock-openai", "mock-key", "gpt-4o")
            .with_stream(true)
            .with_message("user", "Hello");

    // 3. 执行流请求
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let stream = provider.stream_chat(&transport, request).await;
    chain.add_step(
        "llm-protocol-provider",
        "ProviderAdapter::stream_chat",
        "Stream request initiated",
        stream.is_ok(),
    );

    // 4. 消费流
    if let Ok(mut stream) = stream {
        let mut event_count = 0;
        let mut has_usage = false;
        let mut has_done = false;

        while let Some(event) = stream.next().await {
            event_count += 1;
            if let Ok(event) = event {
                match event {
                    llm_protocol_provider::StreamEvent::Usage { .. } => has_usage = true,
                    llm_protocol_provider::StreamEvent::Done => has_done = true,
                    _ => {}
                }
            }
        }

        chain.add_step(
            "llm-gateway",
            "StreamConsumer::event_count",
            format!("Total events: {}", event_count),
            event_count > 0,
        );
        chain.add_step(
            "llm-gateway",
            "StreamConsumer::has_usage",
            format!("Has usage event: {}", has_usage),
            has_usage,
        );
        chain.add_step(
            "llm-gateway",
            "StreamConsumer::has_done",
            format!("Has done event: {}", has_done),
            has_done,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Fallback 链执行
#[tokio::test]
async fn test_gateway_fallback_chain() {
    use futures::StreamExt;

    let mut chain = VerificationChain::new();

    // 1. 创建失败和成功的 Provider
    let failing_provider = Arc::new(MockProviderFactory::create_failing());
    let success_provider = Arc::new(MockProviderFactory::create_anthropic());

    chain.add_step(
        "integration-tests::mocks",
        "create_failing_provider",
        "Failing provider created",
        true,
    );

    // 2. 尝试失败 Provider
    let request = llm_protocol_provider::UpstreamRequest::new("http://mock", "mock-key", "gpt-4o")
        .with_stream(true);

    // 使用默认 HTTP 传输层
    let transport = llm_protocol_provider::DefaultHttpTransport::new();

    let primary_result = failing_provider
        .stream_chat(&transport, request.clone())
        .await;
    chain.add_step(
        "llm-gateway",
        "primary_provider_failure",
        format!("Primary failed: {}", primary_result.is_err()),
        primary_result.is_err(),
    );

    // 3. Fallback 到备用 Provider
    let fallback_result = success_provider.stream_chat(&transport, request).await;
    chain.add_step(
        "llm-gateway",
        "fallback_provider_success",
        format!("Fallback succeeded: {}", fallback_result.is_ok()),
        fallback_result.is_ok(),
    );

    // 4. 验证 Fallback 流
    if let Ok(mut stream) = fallback_result {
        let mut event_count = 0;
        while let Some(event) = stream.next().await {
            if event.is_ok() {
                event_count += 1;
            }
        }
        chain.add_step(
            "llm-gateway",
            "fallback_stream_complete",
            format!("Fallback events: {}", event_count),
            event_count > 0,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}
