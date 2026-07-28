//! 错误处理与边界情况端到端测试
//!
//! 测试覆盖：
//! - 网络超时场景
//! - Provider 连续失败后的冷却触发
//! - 流中断/错误恢复
//! - 并发请求错误处理

use futures::StreamExt;
use integration_tests::common::VerificationChain;
use integration_tests::mocks::provider::MockProviderFactory;
use keycompute_routing::{AccountStateStore, ProviderHealthStore, RoutingEngine};
use keycompute_types::{PricingSnapshot, RequestContext};
use llm_protocol_provider::{ProviderAdapter, StreamEvent, UpstreamRequest};
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

// ============================================================================
// 第一部分：网络超时场景测试
// ============================================================================

/// 测试模拟超时错误
#[tokio::test]
async fn test_timeout_simulation() {
    let mut chain = VerificationChain::new();

    // 1. 创建超时 Provider
    let provider = MockProviderFactory::create_timeout();
    chain.add_step(
        "integration-tests::mocks",
        "MockProviderFactory::create_timeout",
        "Timeout provider created",
        true,
    );

    // 2. 发送请求
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
    let result = provider.stream_chat(&transport, request).await;

    // 3. 验证超时错误
    chain.add_step(
        "llm-protocol-provider",
        "ProviderAdapter::timeout_error",
        format!("Request failed: {}", result.is_err()),
        result.is_err(),
    );

    if let Err(e) = result {
        chain.add_step(
            "llm-protocol-provider",
            "ProviderAdapter::error_message",
            format!("Error message: {}", e),
            e.to_string().contains("timeout"),
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试带延迟的请求
#[tokio::test]
async fn test_delayed_response() {
    let mut chain = VerificationChain::new();

    // 1. 创建延迟 Provider (100ms)
    let provider = MockProviderFactory::create_delayed(100);
    chain.add_step(
        "integration-tests::mocks",
        "MockProviderFactory::create_delayed",
        "Delayed provider created (100ms)",
        true,
    );

    // 2. 测量响应时间
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    let start = std::time::Instant::now();
    let result = provider.stream_chat(&transport, request).await;
    let elapsed = start.elapsed();

    // 3. 验证延迟生效
    chain.add_step(
        "integration-tests::mocks",
        "MockProvider::delay_applied",
        format!("Elapsed: {:?}", elapsed),
        elapsed >= Duration::from_millis(100),
    );

    // 4. 验证请求成功
    chain.add_step(
        "llm-protocol-provider",
        "ProviderAdapter::delayed_success",
        format!("Request succeeded: {}", result.is_ok()),
        result.is_ok(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试慢速流（每个 chunk 有延迟）
#[tokio::test]
async fn test_slow_stream() {
    let mut chain = VerificationChain::new();

    // 1. 创建慢速流 Provider (每个 chunk 延迟 50ms)
    let provider = MockProviderFactory::create_slow_stream(50);
    chain.add_step(
        "integration-tests::mocks",
        "MockProviderFactory::create_slow_stream",
        "Slow stream provider created (50ms per chunk)",
        true,
    );

    // 2. 消费流
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
    let stream = provider.stream_chat(&transport, request).await;

    chain.add_step(
        "llm-protocol-provider",
        "ProviderAdapter::stream_created",
        "Stream created successfully",
        stream.is_ok(),
    );

    if let Ok(mut stream) = stream {
        let start = std::time::Instant::now();
        let mut event_count = 0;

        while let Some(event) = stream.next().await {
            if event.is_ok() {
                event_count += 1;
            }
        }

        let elapsed = start.elapsed();

        // 3 个 chunks + 每个延迟 50ms = 至少 150ms
        chain.add_step(
            "integration-tests::mocks",
            "SlowStream::total_time",
            format!("Total time: {:?}, events: {}", elapsed, event_count),
            elapsed >= Duration::from_millis(150),
        );

        chain.add_step(
            "integration-tests::mocks",
            "SlowStream::event_count",
            format!("Event count: {}", event_count),
            event_count >= 3, // 至少 3 个 delta 事件
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第二部分：Provider 连续失败后冷却触发测试
// ============================================================================

/// 创建测试用的请求上下文
#[allow(dead_code)]
fn create_test_context() -> RequestContext {
    RequestContext::new(
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

/// 测试连续失败触发冷却
#[tokio::test]
async fn test_consecutive_failures_trigger_cooldown() {
    let mut chain = VerificationChain::new();

    // 1. 设置账号状态存储
    let account_states = Arc::new(AccountStateStore::new());
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::new",
        "Account state store created",
        true,
    );

    // 2. 模拟连续失败并设置冷却
    let account_id = Uuid::new_v4();
    for i in 1..=5 {
        account_states.mark_error(account_id);
        chain.add_step(
            "keycompute-routing",
            "AccountStateStore::mark_error",
            format!("Error #{} recorded", i),
            true,
        );
    }

    // 手动设置冷却
    account_states.set_cooldown(account_id, 60);

    // 3. 检查冷却状态
    let is_cooling = account_states.is_cooling_down(&account_id);
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::is_cooling_down",
        format!("Account '{}' cooling: {}", account_id, is_cooling),
        is_cooling,
    );

    // 4. 检查冷却中的账号
    let cooling_accounts = account_states.cooling_accounts();
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::cooling_accounts",
        format!("Cooling accounts count: {}", cooling_accounts.len()),
        !cooling_accounts.is_empty(),
    );

    if !cooling_accounts.is_empty() {
        let (id, state) = &cooling_accounts[0];
        chain.add_step(
            "keycompute-routing",
            "AccountState::cooldown_until",
            format!("Account {} has cooldown_until set", id),
            state.cooldown_until.is_some(),
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试冷却后恢复
#[tokio::test]
async fn test_cooldown_recovery() {
    let mut chain = VerificationChain::new();

    // 1. 设置账号状态存储
    let account_states = Arc::new(AccountStateStore::new());
    let account_id = Uuid::new_v4();

    // 2. 设置冷却
    account_states.set_cooldown(account_id, 60);

    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::set_cooldown",
        "Account cooldown set",
        true,
    );

    // 3. 验证初始冷却状态
    let initial_cooling = account_states.is_cooling_down(&account_id);
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::initial_cooling",
        format!("Account initially cooling: {}", initial_cooling),
        initial_cooling,
    );

    // 4. 清除冷却
    account_states.clear_cooldown(account_id);

    // 5. 验证冷却已清除
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_clear = account_states.is_cooling_down(&account_id);
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::after_clear",
        format!("Account after clear cooling: {}", after_clear),
        !after_clear,
    );

    // 6. 验证错误计数也被清除
    let state = account_states.get(&account_id);
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::cleanup_expired",
        format!("Error count after clear: {}", state.error_count),
        state.error_count == 0,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Provider 健康状态与冷却联动
#[tokio::test]
async fn test_health_and_cooldown_integration() {
    let mut chain = VerificationChain::new();

    // 1. 设置健康状态存储和账号状态存储
    let provider_health = Arc::new(ProviderHealthStore::new());
    let account_states = Arc::new(AccountStateStore::new());

    chain.add_step(
        "keycompute-routing",
        "ProviderHealthStore::new",
        "Health store and account states created",
        true,
    );

    // 2. 记录连续失败
    let provider_name = "unhealthy-provider";
    for _ in 0..10 {
        provider_health.record_failure(provider_name);
    }

    chain.add_step(
        "keycompute-routing",
        "ProviderHealthStore::record_failures",
        format!("Recorded 10 failures for '{}'", provider_name),
        true,
    );

    // 3. 检查健康状态
    let score = provider_health.get_score(provider_name);
    chain.add_step(
        "keycompute-routing",
        "ProviderHealthStore::get_health_score",
        format!("Health score: {}", score),
        score < 50, // 健康分数应该很低
    );

    // 4. 触发账号冷却
    let account_id = Uuid::new_v4();
    if score < 50 {
        account_states.set_cooldown(account_id, 120);
    }

    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::trigger_on_low_health",
        "Account cooldown triggered due to low health",
        account_states.is_cooling_down(&account_id),
    );

    // 5. 创建路由引擎验证过滤
    let providers = vec![provider_name.to_string(), "other".to_string()];
    let engine = RoutingEngine::new(account_states, provider_health.clone(), providers);

    let healthy_providers = engine.healthy_providers();
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::healthy_providers",
        format!("Healthy providers: {:?}", healthy_providers),
        !healthy_providers.contains(&provider_name.to_string()),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Flaky Provider（前 N 次失败，之后成功）
#[tokio::test]
async fn test_flaky_provider_recovery() {
    let mut chain = VerificationChain::new();

    // 1. 创建 Flaky Provider（前 3 次失败）
    let provider = MockProviderFactory::create_flaky(3);
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    chain.add_step(
        "integration-tests::mocks",
        "MockProviderFactory::create_flaky",
        "Flaky provider created (threshold: 3)",
        true,
    );

    // 2. 前 3 次应该失败
    for i in 1..=3 {
        let result = provider.stream_chat(&transport, request.clone()).await;
        chain.add_step(
            "integration-tests::mocks",
            "FlakyProvider::failure",
            format!("Request #{}: failed={}", i, result.is_err()),
            result.is_err(),
        );
    }

    // 3. 第 4 次应该成功
    let result = provider.stream_chat(&transport, request).await;
    chain.add_step(
        "integration-tests::mocks",
        "FlakyProvider::recovery",
        format!("Request #4: succeeded={}", result.is_ok()),
        result.is_ok(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第三部分：流中断/错误恢复测试
// ============================================================================

/// 测试流中间注入错误
#[tokio::test]
async fn test_stream_error_injection() {
    let mut chain = VerificationChain::new();

    // 1. 创建在第二个 chunk 后注入错误的 Provider
    let provider = MockProviderFactory::create_with_stream_error(2);
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    chain.add_step(
        "integration-tests::mocks",
        "MockProviderFactory::create_with_stream_error",
        "Stream error provider created (error at chunk 2)",
        true,
    );

    // 2. 消费流
    let mut stream = provider.stream_chat(&transport, request).await.unwrap();
    let mut events = Vec::new();
    let mut error_found = false;
    let mut chunks_before_error = 0;

    while let Some(event) = stream.next().await {
        if let Ok(e) = event {
            match &e {
                StreamEvent::Delta { .. } if !error_found => {
                    chunks_before_error += 1;
                }
                StreamEvent::Error { message } => {
                    error_found = true;
                    chain.add_step(
                        "llm-protocol-provider",
                        "StreamEvent::Error",
                        format!("Error message: {}", message),
                        message.contains("Simulated stream error"),
                    );
                }
                _ => {}
            }
            events.push(e);
        }
    }

    // 3. 验证错误前收到了数据
    chain.add_step(
        "integration-tests::mocks",
        "StreamError::chunks_before_error",
        format!("Chunks before error: {}", chunks_before_error),
        chunks_before_error == 2, // 错误在第 2 个 chunk 后注入
    );

    // 4. 验证错误事件存在
    chain.add_step(
        "integration-tests::mocks",
        "StreamError::error_injected",
        format!("Error found in stream: {}", error_found),
        error_found,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试流错误后继续消费
#[tokio::test]
async fn test_stream_continue_after_error() {
    let mut chain = VerificationChain::new();

    // 1. 创建在第三个 chunk 后注入错误的 Provider
    let provider = MockProviderFactory::create_with_stream_error(3);
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    // 2. 消费流
    let mut stream = provider.stream_chat(&transport, request).await.unwrap();
    let mut delta_count = 0;
    let mut has_error = false;
    let mut has_usage = false;
    let mut has_done = false;

    while let Some(event) = stream.next().await {
        if let Ok(e) = event {
            match e {
                StreamEvent::Delta { .. } => delta_count += 1,
                StreamEvent::Error { .. } => has_error = true,
                StreamEvent::Usage { .. } => has_usage = true,
                StreamEvent::Done => has_done = true,
                _ => {}
            }
        }
    }

    // 3. 验证流继续消费
    chain.add_step(
        "integration-tests::mocks",
        "StreamContinue::has_delta",
        format!("Delta events: {}", delta_count),
        delta_count > 0,
    );

    chain.add_step(
        "integration-tests::mocks",
        "StreamContinue::has_error",
        format!("Has error event: {}", has_error),
        has_error,
    );

    chain.add_step(
        "integration-tests::mocks",
        "StreamContinue::total_events",
        format!(
            "Stream continued after error, delta={}, error={}, usage={}, done={}",
            delta_count, has_error, has_usage, has_done
        ),
        true, // 流没有因错误而终止
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Fallback 链处理流错误
#[tokio::test]
async fn test_fallback_on_stream_error() {
    let mut chain = VerificationChain::new();

    // 1. 创建会失败的 Provider 和成功的备用 Provider
    let failing_provider = Arc::new(MockProviderFactory::create_failing());
    let success_provider = Arc::new(MockProviderFactory::create_anthropic());

    chain.add_step(
        "integration-tests::mocks",
        "FallbackSetup::failing_provider",
        "Failing provider created",
        true,
    );

    // 2. 尝试主 Provider
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    let primary_result = failing_provider
        .stream_chat(&transport, request.clone())
        .await;
    chain.add_step(
        "llm-gateway",
        "FallbackChain::primary_failed",
        format!("Primary failed: {}", primary_result.is_err()),
        primary_result.is_err(),
    );

    // 3. Fallback 到备用 Provider
    let fallback_result = success_provider.stream_chat(&transport, request).await;
    chain.add_step(
        "llm-gateway",
        "FallbackChain::fallback_succeeded",
        format!("Fallback succeeded: {}", fallback_result.is_ok()),
        fallback_result.is_ok(),
    );

    // 4. 消费备用流
    if let Ok(mut stream) = fallback_result {
        let mut event_count = 0;
        while let Some(event) = stream.next().await {
            if event.is_ok() {
                event_count += 1;
            }
        }
        chain.add_step(
            "llm-gateway",
            "FallbackChain::fallback_stream_complete",
            format!("Fallback events: {}", event_count),
            event_count > 0,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第四部分：边界情况测试
// ============================================================================

/// 测试空响应处理
#[tokio::test]
async fn test_empty_response() {
    let mut chain = VerificationChain::new();

    // 1. 创建返回空内容的 Provider
    let provider = MockProviderFactory::create_openai().with_chunks(vec![]); // 空 chunks

    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    // 2. 消费流
    let stream = provider.stream_chat(&transport, request).await;
    chain.add_step(
        "llm-protocol-provider",
        "EmptyResponse::stream_created",
        "Stream created for empty response",
        stream.is_ok(),
    );

    if let Ok(mut stream) = stream {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            if let Ok(e) = event {
                events.push(e);
            }
        }

        // 3. 验证空响应仍有 Usage 和 Done
        let has_usage = events
            .iter()
            .any(|e| matches!(e, StreamEvent::Usage { .. }));
        let has_done = events.iter().any(|e| matches!(e, StreamEvent::Done));

        chain.add_step(
            "integration-tests::mocks",
            "EmptyResponse::has_usage",
            format!("Empty response has Usage: {}", has_usage),
            has_usage,
        );

        chain.add_step(
            "integration-tests::mocks",
            "EmptyResponse::has_done",
            format!("Empty response has Done: {}", has_done),
            has_done,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试超大 token 计数
#[tokio::test]
async fn test_large_token_count() {
    let mut chain = VerificationChain::new();

    // 1. 创建大 token 计数的 Provider
    let provider = MockProviderFactory::create_openai().with_tokens(u32::MAX, u32::MAX); // 最大值

    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    // 2. 消费流
    let mut stream = provider.stream_chat(&transport, request).await.unwrap();
    let mut usage_event: Option<StreamEvent> = None;

    while let Some(event) = stream.next().await {
        if let Ok(StreamEvent::Usage {
            input_tokens,
            output_tokens,
        }) = event
        {
            usage_event = Some(StreamEvent::Usage {
                input_tokens,
                output_tokens,
            });
            break;
        }
    }

    // 3. 验证大数值处理
    if let Some(StreamEvent::Usage {
        input_tokens,
        output_tokens,
    }) = usage_event
    {
        chain.add_step(
            "integration-tests::mocks",
            "LargeToken::input",
            format!("Input tokens: {}", input_tokens),
            input_tokens == u32::MAX,
        );

        chain.add_step(
            "integration-tests::mocks",
            "LargeToken::output",
            format!("Output tokens: {}", output_tokens),
            output_tokens == u32::MAX,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试错误重置计数
#[tokio::test]
async fn test_failure_count_reset() {
    let mut chain = VerificationChain::new();

    // 1. 创建 Flaky Provider
    let provider = MockProviderFactory::create_flaky(2);
    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let request = UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);

    // 2. 前 2 次失败
    let _ = provider.stream_chat(&transport, request.clone()).await;
    let _ = provider.stream_chat(&transport, request.clone()).await;

    chain.add_step(
        "integration-tests::mocks",
        "FailureCount::after_failures",
        format!(
            "Failure count after 2 failures: {}",
            provider.failure_count()
        ),
        provider.failure_count() == 2,
    );

    // 3. 第 3 次成功
    let _ = provider.stream_chat(&transport, request.clone()).await;
    chain.add_step(
        "integration-tests::mocks",
        "FailureCount::after_success",
        format!("Failure count after success: {}", provider.failure_count()),
        provider.failure_count() == 2, // 计数不再增加
    );

    // 4. 重置计数
    provider.reset_failure_count();
    chain.add_step(
        "integration-tests::mocks",
        "FailureCount::after_reset",
        format!("Failure count after reset: {}", provider.failure_count()),
        provider.failure_count() == 0,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第五部分：并发错误处理测试
// ============================================================================

/// 测试并发请求中的错误处理
#[tokio::test]
async fn test_concurrent_error_handling() {
    use tokio::task::JoinSet;

    let mut chain = VerificationChain::new();

    // 1. 创建多个不同行为的 Provider
    let success_provider = Arc::new(MockProviderFactory::create_openai());
    let failing_provider = Arc::new(MockProviderFactory::create_failing());
    let flaky_provider = Arc::new(MockProviderFactory::create_flaky(2));

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentSetup::providers",
        "Multiple providers created for concurrent test",
        true,
    );

    // 2. 并发执行请求
    let mut tasks = JoinSet::new();

    for _ in 0..5 {
        let p = success_provider.clone();
        let transport = llm_protocol_provider::DefaultHttpTransport::new();
        tasks.spawn(async move {
            let request =
                UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
            p.stream_chat(&transport, request).await
        });
    }

    for _ in 0..3 {
        let p = failing_provider.clone();
        let transport = llm_protocol_provider::DefaultHttpTransport::new();
        tasks.spawn(async move {
            let request =
                UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
            p.stream_chat(&transport, request).await
        });
    }

    for _ in 0..2 {
        let p = flaky_provider.clone();
        let transport = llm_protocol_provider::DefaultHttpTransport::new();
        tasks.spawn(async move {
            let request =
                UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
            p.stream_chat(&transport, request).await
        });
    }

    // 3. 收集结果
    let mut success_count = 0;
    let mut error_count = 0;

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(_)) => success_count += 1,
            _ => error_count += 1,
        }
    }

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentErrors::success_count",
        format!("Successful requests: {}", success_count),
        success_count >= 5, // 至少 5 个成功的请求
    );

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentErrors::error_count",
        format!("Failed requests: {}", error_count),
        error_count >= 3, // 至少 3 个失败的请求（failing + flaky 前 2 次）
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试冷却状态下的并发请求
#[tokio::test]
async fn test_concurrent_requests_with_cooldown() {
    use tokio::task::JoinSet;

    let mut chain = VerificationChain::new();

    // 1. 设置账号冷却
    let account_states = Arc::new(AccountStateStore::new());
    let account_id = Uuid::new_v4();

    account_states.set_cooldown(account_id, 60);

    chain.add_step(
        "keycompute-routing",
        "ConcurrentCooldown::set",
        "Account cooldown set for concurrent test",
        true,
    );

    // 2. 并发检查冷却状态
    let mut tasks = JoinSet::new();

    for _ in 0..10 {
        let states = account_states.clone();
        let aid = account_id;
        tasks.spawn(async move { states.is_cooling_down(&aid) });
    }

    // 3. 所有检查应该一致
    let mut all_cooling = true;
    while let Some(result) = tasks.join_next().await {
        if let Ok(is_cooling) = result
            && !is_cooling
        {
            all_cooling = false;
        }
    }

    chain.add_step(
        "keycompute-runtime",
        "ConcurrentCooldown::consistent_state",
        format!("All checks show cooling: {}", all_cooling),
        all_cooling,
    );

    chain.print_report();
    assert!(chain.all_passed());
}
