//! 高并发压力测试
//!
//! 使用 tokio::spawn + JoinSet 进行高并发请求测试
//!
//! 测试覆盖：
//! - 高并发路由引擎测试
//! - 高并发限流器测试
//! - 高并发 Provider 请求测试
//! - 混合负载压力测试
//! - 冷却状态并发访问测试

use futures::StreamExt;
use integration_tests::common::VerificationChain;
use integration_tests::mocks::provider::MockProviderFactory;
use keycompute_ratelimit::{RateLimitKey, RateLimitService};
use keycompute_routing::{AccountStateStore, ProviderHealthStore, RoutingEngine};
use keycompute_types::{ExecutionTarget, PricingSnapshot, RequestContext};
use llm_protocol_provider::{ProviderAdapter, UpstreamRequest};
use rust_decimal::Decimal;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use uuid::Uuid;

// ============================================================================
// 辅助函数
// ============================================================================

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

// ============================================================================
// 第一部分：高并发路由引擎测试
// ============================================================================

/// 测试高并发路由请求
#[tokio::test]
async fn test_concurrent_routing_requests() {
    let mut chain = VerificationChain::new();
    let concurrent_requests = 100;

    // 1. 创建共享的路由引擎
    let engine = Arc::new(create_test_engine());
    chain.add_step(
        "keycompute-routing",
        "RoutingEngine::shared",
        format!(
            "Shared engine created for {} concurrent requests",
            concurrent_requests
        ),
        true,
    );

    // 2. 启动并发请求
    let mut tasks = JoinSet::new();
    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let total_latency_ms = Arc::new(AtomicU64::new(0));

    let start = Instant::now();

    for _ in 0..concurrent_requests {
        let engine = engine.clone();
        let success = success_count.clone();
        let error = error_count.clone();
        let latency = total_latency_ms.clone();

        tasks.spawn(async move {
            let ctx = create_test_context();
            let req_start = Instant::now();
            let result = engine.route(&ctx).await;
            let req_latency = req_start.elapsed().as_millis() as u64;

            match result {
                Ok(plan) => {
                    success.fetch_add(1, Ordering::Relaxed);
                    latency.fetch_add(req_latency, Ordering::Relaxed);
                    Some(plan)
                }
                Err(_) => {
                    error.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
        });
    }

    // 3. 等待所有请求完成
    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(Some(plan)) = result {
            results.push(plan);
        }
    }

    let total_time = start.elapsed();
    let success = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let avg_latency = total_latency_ms.load(Ordering::Relaxed) as f64 / success as f64;

    // 4. 验证结果
    chain.add_step(
        "keycompute-routing",
        "ConcurrentRouting::success_rate",
        format!(
            "Success: {}/{}, Error: {}, Total time: {:?}",
            success, concurrent_requests, errors, total_time
        ),
        success == concurrent_requests as u64,
    );

    chain.add_step(
        "keycompute-routing",
        "ConcurrentRouting::avg_latency",
        format!("Average latency: {:.2}ms", avg_latency),
        avg_latency < 100.0, // 平均延迟应该低于 100ms
    );

    chain.add_step(
        "keycompute-routing",
        "ConcurrentRouting::throughput",
        format!(
            "Throughput: {:.2} req/s",
            concurrent_requests as f64 / total_time.as_secs_f64()
        ),
        total_time.as_secs_f64() < 5.0, // 100 个请求应该在 5 秒内完成
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试路由引擎并发健康状态检查
#[tokio::test]
async fn test_concurrent_routing_health_checks() {
    let mut chain = VerificationChain::new();
    let concurrent_requests = 50;

    // 1. 创建路由引擎
    let engine = Arc::new(create_test_engine());

    // 2. 一半请求执行路由，一半请求检查健康状态
    let mut tasks = JoinSet::new();
    let route_count = Arc::new(AtomicU64::new(0));
    let health_check_count = Arc::new(AtomicU64::new(0));

    for i in 0..concurrent_requests {
        let engine = engine.clone();
        let route = route_count.clone();
        let health = health_check_count.clone();

        tasks.spawn(async move {
            if i % 2 == 0 {
                // 路由请求
                let ctx = create_test_context();
                let _ = engine.route(&ctx).await;
                route.fetch_add(1, Ordering::Relaxed);
            } else {
                // 检查 Provider 健康状态
                let _ = engine.is_provider_healthy("openai");
                let _ = engine.is_provider_healthy("claude");
                health.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    // 3. 等待所有任务完成
    while tasks.join_next().await.is_some() {}

    let routes = route_count.load(Ordering::Relaxed);
    let health_checks = health_check_count.load(Ordering::Relaxed);

    // 4. 验证
    chain.add_step(
        "keycompute-routing",
        "ConcurrentRoutingHealth::route_count",
        format!("Route requests: {}", routes),
        routes == concurrent_requests / 2,
    );

    chain.add_step(
        "keycompute-routing",
        "ConcurrentRoutingHealth::health_check_count",
        format!("Health check operations: {}", health_checks),
        health_checks == concurrent_requests / 2,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第二部分：高并发限流器测试
// ============================================================================

/// 测试高并发限流
#[tokio::test]
async fn test_concurrent_rate_limiting() {
    let mut chain = VerificationChain::new();
    let concurrent_requests = 100;

    // 1. 创建限流服务（每分钟 3000 个请求）
    let service = Arc::new(RateLimitService::default_memory());

    chain.add_step(
        "keycompute-ratelimit",
        "RateLimitService::created",
        "Rate limiter created (default config)",
        true,
    );

    // 2. 并发请求
    let mut tasks = JoinSet::new();
    let allowed_count = Arc::new(AtomicU64::new(0));
    let denied_count = Arc::new(AtomicU64::new(0));

    for _ in 0..concurrent_requests {
        let service = service.clone();
        let allowed = allowed_count.clone();
        let denied = denied_count.clone();

        tasks.spawn(async move {
            let key = RateLimitKey::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
            let result = service.check_and_record(&key).await;

            match result {
                Ok(()) => allowed.fetch_add(1, Ordering::Relaxed),
                Err(_) => denied.fetch_add(1, Ordering::Relaxed),
            };
        });
    }

    // 3. 等待完成
    while tasks.join_next().await.is_some() {}

    let allowed = allowed_count.load(Ordering::Relaxed);
    let denied = denied_count.load(Ordering::Relaxed);

    // 4. 验证
    chain.add_step(
        "keycompute-ratelimit",
        "ConcurrentRateLimit::allowed",
        format!("Allowed requests: {}", allowed),
        allowed > 0,
    );

    chain.add_step(
        "keycompute-ratelimit",
        "ConcurrentRateLimit::total",
        format!("Total: allowed={}, denied={}", allowed, denied),
        allowed + denied == concurrent_requests as u64,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试多租户并发限流
#[tokio::test]
async fn test_concurrent_multi_tenant_rate_limiting() {
    let mut chain = VerificationChain::new();
    let tenants = 10;
    let requests_per_tenant = 20;

    // 1. 创建限流服务
    let service = Arc::new(RateLimitService::default_memory());

    // 2. 为每个租户创建并发请求
    let mut tasks = JoinSet::new();
    let tenant_results: Arc<std::sync::Mutex<Vec<(Uuid, u64, u64)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    for _tenant_idx in 0..tenants {
        let service = service.clone();
        let results = tenant_results.clone();
        let tenant_id = Uuid::new_v4();

        for _ in 0..requests_per_tenant {
            let service = service.clone();
            let results = results.clone();
            let tid = tenant_id;

            tasks.spawn(async move {
                let key = RateLimitKey::new(tid, Uuid::new_v4(), Uuid::new_v4());
                let result = service.check_and_record(&key).await;
                let is_allowed = result.is_ok();

                let mut guard = results.lock().unwrap();
                let entry = guard.iter_mut().find(|(id, _, _)| *id == tid);
                if let Some((_, allowed, denied)) = entry {
                    if is_allowed {
                        *allowed += 1;
                    } else {
                        *denied += 1;
                    }
                } else {
                    guard.push((
                        tid,
                        if is_allowed { 1 } else { 0 },
                        if !is_allowed { 1 } else { 0 },
                    ));
                }
            });
        }
    }

    // 3. 等待完成
    while tasks.join_next().await.is_some() {}

    // 4. 验证每个租户的限流独立
    let results = tenant_results.lock().unwrap();
    chain.add_step(
        "keycompute-ratelimit",
        "ConcurrentMultiTenant::tenant_count",
        format!("Active tenants: {}", results.len()),
        results.len() == tenants,
    );

    let total_allowed: u64 = results.iter().map(|(_, a, _)| a).sum();
    let total_denied: u64 = results.iter().map(|(_, _, d)| d).sum();

    chain.add_step(
        "keycompute-ratelimit",
        "ConcurrentMultiTenant::totals",
        format!(
            "Total: allowed={}, denied={}, expected={}",
            total_allowed,
            total_denied,
            tenants * requests_per_tenant
        ),
        total_allowed + total_denied == (tenants * requests_per_tenant) as u64,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第三部分：高并发 Provider 请求测试
// ============================================================================

/// 测试高并发 Provider 流请求
#[tokio::test]
async fn test_concurrent_provider_requests() {
    let mut chain = VerificationChain::new();
    let concurrent_requests = 50;

    // 1. 创建 Provider
    let provider = Arc::new(MockProviderFactory::create_openai());
    let transport = Arc::new(llm_protocol_provider::DefaultHttpTransport::new());

    chain.add_step(
        "integration-tests::mocks",
        "MockProvider::shared",
        format!(
            "Shared provider for {} concurrent requests",
            concurrent_requests
        ),
        true,
    );

    // 2. 并发请求
    let mut tasks = JoinSet::new();
    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let total_events = Arc::new(AtomicU64::new(0));

    for _ in 0..concurrent_requests {
        let provider = provider.clone();
        let transport = transport.clone();
        let success = success_count.clone();
        let error = error_count.clone();
        let events = total_events.clone();

        tasks.spawn(async move {
            let request =
                UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
            let result = provider.stream_chat(&*transport, request).await;

            match result {
                Ok(mut stream) => {
                    success.fetch_add(1, Ordering::Relaxed);
                    let mut count = 0u64;
                    while let Some(event) = stream.next().await {
                        if event.is_ok() {
                            count += 1;
                        }
                    }
                    events.fetch_add(count, Ordering::Relaxed);
                }
                Err(_) => {
                    error.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }

    // 3. 等待完成
    while tasks.join_next().await.is_some() {}

    let success = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let events = total_events.load(Ordering::Relaxed);

    // 4. 验证
    chain.add_step(
        "llm-protocol-provider",
        "ConcurrentProvider::success_rate",
        format!(
            "Success: {}/{}, Errors: {}",
            success, concurrent_requests, errors
        ),
        success == concurrent_requests as u64,
    );

    chain.add_step(
        "llm-protocol-provider",
        "ConcurrentProvider::events_per_request",
        format!(
            "Total events: {}, Avg per request: {:.1}",
            events,
            events as f64 / success as f64
        ),
        events >= success * 3, // 每个请求至少 3 个事件
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试高并发混合 Provider（成功 + 失败 + 超时）
#[tokio::test]
async fn test_concurrent_mixed_providers() {
    let mut chain = VerificationChain::new();
    let _total_requests = 60;

    // 1. 创建不同类型的 Provider
    let success_provider = Arc::new(MockProviderFactory::create_openai());
    let failing_provider = Arc::new(MockProviderFactory::create_failing());
    let timeout_provider = Arc::new(MockProviderFactory::create_timeout());
    let flaky_provider = Arc::new(MockProviderFactory::create_flaky(2));

    let transport = Arc::new(llm_protocol_provider::DefaultHttpTransport::new());

    // 2. 并发请求（每种类型 15 个）
    let mut tasks = JoinSet::new();
    let results: Arc<std::sync::Mutex<(u64, u64, u64, u64)>> =
        Arc::new(std::sync::Mutex::new((0, 0, 0, 0))); // (success, fail, timeout, recovered)

    for _ in 0..15 {
        // Success
        {
            let p = success_provider.clone();
            let t = transport.clone();
            let r = results.clone();
            tasks.spawn(async move {
                let request =
                    UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
                if p.stream_chat(&*t, request).await.is_ok() {
                    r.lock().unwrap().0 += 1;
                }
            });
        }

        // Failing
        {
            let p = failing_provider.clone();
            let t = transport.clone();
            let r = results.clone();
            tasks.spawn(async move {
                let request =
                    UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
                if p.stream_chat(&*t, request).await.is_err() {
                    r.lock().unwrap().1 += 1;
                }
            });
        }

        // Timeout
        {
            let p = timeout_provider.clone();
            let t = transport.clone();
            let r = results.clone();
            tasks.spawn(async move {
                let request =
                    UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
                if p.stream_chat(&*t, request).await.is_err() {
                    r.lock().unwrap().2 += 1;
                }
            });
        }

        // Flaky (前 2 次失败，之后成功)
        {
            let p = flaky_provider.clone();
            let t = transport.clone();
            let r = results.clone();
            tasks.spawn(async move {
                let request =
                    UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
                let result = p.stream_chat(&*t, request).await;
                if result.is_ok() {
                    r.lock().unwrap().3 += 1;
                }
            });
        }
    }

    // 3. 等待完成
    while tasks.join_next().await.is_some() {}

    // 4. 验证
    let guard = results.lock().unwrap();
    let (success, fail, timeout, recovered) = *guard;

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentMixed::success",
        format!("Success provider requests: {}", success),
        success == 15,
    );

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentMixed::failures",
        format!("Failed requests: {}", fail),
        fail == 15,
    );

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentMixed::timeouts",
        format!("Timeout requests: {}", timeout),
        timeout == 15,
    );

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentMixed::recovered",
        format!("Recovered (flaky) requests: {}", recovered),
        recovered > 0, // 部分请求成功恢复
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第四部分：冷却状态并发访问测试
// ============================================================================

/// 测试冷却状态并发读写
#[tokio::test]
async fn test_concurrent_cooldown_access() {
    let mut chain = VerificationChain::new();
    let concurrent_ops = 100;

    // 1. 创建账号状态存储
    let account_states = Arc::new(AccountStateStore::new());

    // 2. 并发操作:设置冷却 + 检查冷却
    let mut tasks = JoinSet::new();
    let set_count = Arc::new(AtomicU64::new(0));
    let check_count = Arc::new(AtomicU64::new(0));

    for i in 0..concurrent_ops {
        let states = account_states.clone();
        let set = set_count.clone();
        let check = check_count.clone();
        let account_id = Uuid::from_u128(i as u128 % 10);

        tasks.spawn(async move {
            if i % 3 == 0 {
                // 设置冷却
                states.set_cooldown(account_id, 60);
                set.fetch_add(1, Ordering::Relaxed);
            } else {
                // 检查冷却
                let _ = states.is_cooling_down(&account_id);
                check.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    // 3. 等待完成
    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            tracing::error!("Task panicked: {:?}", e);
        }
    }

    let sets = set_count.load(Ordering::Relaxed);
    let checks = check_count.load(Ordering::Relaxed);

    // 4. 验证
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::concurrent_ops",
        format!("Set operations: {}, Check operations: {}", sets, checks),
        sets + checks == concurrent_ops as u64,
    );

    let cooling_accounts = account_states.cooling_accounts();
    chain.add_step(
        "keycompute-routing",
        "AccountStateStore::accounts_cooling",
        format!("Accounts in cooldown: {}", cooling_accounts.len()),
        !cooling_accounts.is_empty(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试并发冷却过期
#[tokio::test]
async fn test_concurrent_cooldown_expiry() {
    let mut chain = VerificationChain::new();
    let accounts = 20;

    // 1. 创建账号状态存储
    let account_states = Arc::new(AccountStateStore::new());

    // 2. 设置所有账号冷却
    for i in 0..accounts {
        let account_id = Uuid::from_u128(i as u128);
        account_states.set_cooldown(account_id, 60);
    }

    chain.add_step(
        "keycompute-routing",
        "ConcurrentExpiry::all_set",
        format!("All {} accounts set to cooldown", accounts),
        true,
    );

    // 3. 并发检查冷却状态
    let mut tasks = JoinSet::new();
    let initial_cooling = Arc::new(AtomicU64::new(0));

    for i in 0..accounts {
        let states = account_states.clone();
        let cooling = initial_cooling.clone();
        let account_id = Uuid::from_u128(i as u128);

        tasks.spawn(async move {
            if states.is_cooling_down(&account_id) {
                cooling.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            tracing::error!("Task panicked: {:?}", e);
        }
    }

    let before = initial_cooling.load(Ordering::Relaxed);
    chain.add_step(
        "keycompute-routing",
        "ConcurrentExpiry::before_expiry",
        format!("Accounts cooling before expiry: {}", before),
        before == accounts as u64,
    );

    // 4. 清除冷却状态
    for i in 0..accounts {
        let account_id = Uuid::from_u128(i as u128);
        account_states.clear_cooldown(account_id);
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    // 5. 再次并发检查
    let mut tasks = JoinSet::new();
    let after_cooling = Arc::new(AtomicU64::new(0));

    for i in 0..accounts {
        let states = account_states.clone();
        let cooling = after_cooling.clone();
        let account_id = Uuid::from_u128(i as u128);

        tasks.spawn(async move {
            if states.is_cooling_down(&account_id) {
                cooling.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            tracing::error!("Task panicked: {:?}", e);
        }
    }

    let after = after_cooling.load(Ordering::Relaxed);
    chain.add_step(
        "keycompute-routing",
        "ConcurrentExpiry::after_expiry",
        format!("Accounts cooling after clear: {}", after),
        after == 0, // 全部清除
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第五部分：混合负载压力测试
// ============================================================================

/// 测试完整链路并发压力
#[tokio::test]
async fn test_full_chain_concurrent_pressure() {
    let mut chain = VerificationChain::new();
    let concurrent_requests = 80;

    // 1. 创建所有组件
    let engine = Arc::new(create_test_engine());
    let provider = Arc::new(MockProviderFactory::create_openai());
    let transport = Arc::new(llm_protocol_provider::DefaultHttpTransport::new());

    chain.add_step(
        "integration-tests",
        "FullChain::components",
        "All components created for full chain test",
        true,
    );

    // 2. 统计
    let stats = Arc::new(std::sync::Mutex::new(FullChainStats::default()));

    // 3. 并发执行完整链路
    let mut tasks = JoinSet::new();
    let start = Instant::now();

    for _i in 0..concurrent_requests {
        let engine = engine.clone();
        let provider = provider.clone();
        let transport = transport.clone();
        let stats = stats.clone();

        tasks.spawn(async move {
            let ctx = create_test_context();
            let mut success = false;

            // Step 1: Routing
            if let Ok(plan) = engine.route(&ctx).await {
                // Step 2: Check account cooldown (only for ProviderAccount)
                if let ExecutionTarget::ProviderAccount {
                    account_id,
                    endpoint,
                    upstream_api_key,
                    ..
                } = &plan.primary
                {
                    if !engine.is_account_cooling(account_id) {
                        // Step 3: Provider request
                        let request =
                            UpstreamRequest::new(endpoint, upstream_api_key.clone(), &ctx.model)
                                .with_stream(true);

                        if let Ok(mut stream) = provider.stream_chat(&*transport, request).await {
                            // Step 4: Consume stream
                            let mut event_count = 0u64;
                            while let Some(event) = stream.next().await {
                                if event.is_ok() {
                                    event_count += 1;
                                }
                            }
                            success = event_count > 0;

                            // Step 5: Record success
                            if success {
                                stats.lock().unwrap().provider_successes += 1;
                            }
                        } else {
                            // Provider failed
                            stats.lock().unwrap().provider_failures += 1;
                        }
                    } else {
                        stats.lock().unwrap().cooldown_skips += 1;
                    }
                } // Close the if let ExecutionTarget::ProviderAccount
            }

            if success {
                stats.lock().unwrap().complete_successes += 1;
            }
        });
    }

    // 4. 等待完成
    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            tracing::error!("Task panicked: {:?}", e);
        }
    }

    let elapsed = start.elapsed();
    let guard = stats.lock().unwrap();

    // 5. 验证
    chain.add_step(
        "integration-tests",
        "FullChain::throughput",
        format!(
            "{} requests in {:?} = {:.1} req/s",
            concurrent_requests,
            elapsed,
            concurrent_requests as f64 / elapsed.as_secs_f64()
        ),
        elapsed.as_secs_f64() < 10.0,
    );

    chain.add_step(
        "integration-tests",
        "FullChain::success_rate",
        format!(
            "Complete successes: {}, Provider successes: {}, Failures: {}, Cooldown skips: {}",
            guard.complete_successes,
            guard.provider_successes,
            guard.provider_failures,
            guard.cooldown_skips
        ),
        guard.complete_successes > 0,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

#[derive(Debug, Default)]
struct FullChainStats {
    complete_successes: u64,
    provider_successes: u64,
    provider_failures: u64,
    cooldown_skips: u64,
}

/// 测试突发流量处理
#[tokio::test]
async fn test_burst_traffic_handling() {
    let mut chain = VerificationChain::new();

    // 1. 创建组件
    let provider = Arc::new(MockProviderFactory::create_openai());
    let transport = Arc::new(llm_protocol_provider::DefaultHttpTransport::new());

    // 2. 分批发送请求模拟突发流量
    let batches = 5;
    let batch_size = 20;
    let mut all_tasks = JoinSet::new();
    let total_processed = Arc::new(AtomicU64::new(0));

    let start = Instant::now();

    for batch in 0..batches {
        // 模拟突发：每批几乎同时发送
        let provider = provider.clone();
        let transport = transport.clone();
        let processed = total_processed.clone();

        for _ in 0..batch_size {
            let provider = provider.clone();
            let transport = transport.clone();
            let processed = processed.clone();

            all_tasks.spawn(async move {
                let request =
                    UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
                if provider.stream_chat(&*transport, request).await.is_ok() {
                    processed.fetch_add(1, Ordering::Relaxed);
                }
            });
        }

        // 批次间短暂间隔
        if batch < batches - 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // 3. 等待所有请求
    while all_tasks.join_next().await.is_some() {}

    let elapsed = start.elapsed();
    let processed = total_processed.load(Ordering::Relaxed);
    let total_requests = batches * batch_size;

    // 4. 验证
    chain.add_step(
        "integration-tests",
        "BurstTraffic::total_requests",
        format!("Total requests: {}", total_requests),
        true,
    );

    chain.add_step(
        "integration-tests",
        "BurstTraffic::processed",
        format!("Successfully processed: {}", processed),
        processed == total_requests as u64,
    );

    chain.add_step(
        "integration-tests",
        "BurstTraffic::time",
        format!("Total time: {:?}", elapsed),
        elapsed.as_secs_f64() < 15.0,
    );

    chain.add_step(
        "integration-tests",
        "BurstTraffic::throughput",
        format!(
            "Average throughput: {:.1} req/s",
            total_requests as f64 / elapsed.as_secs_f64()
        ),
        true,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试持续高负载
#[tokio::test]
async fn test_sustained_high_load() {
    let mut chain = VerificationChain::new();
    let duration_secs = 3;
    let target_rps = 50; // 目标每秒请求数

    // 1. 创建组件
    let provider = Arc::new(MockProviderFactory::create_openai());
    let transport = Arc::new(llm_protocol_provider::DefaultHttpTransport::new());

    // 2. 持续发送请求
    let total_requests = duration_secs * target_rps;
    let mut tasks = JoinSet::new();
    let success_count = Arc::new(AtomicU64::new(0));

    let start = Instant::now();

    for _ in 0..total_requests {
        let provider = provider.clone();
        let transport = transport.clone();
        let success = success_count.clone();

        tasks.spawn(async move {
            let request =
                UpstreamRequest::new("http://test", "test-key", "gpt-4o").with_stream(true);
            if provider.stream_chat(&*transport, request).await.is_ok() {
                success.fetch_add(1, Ordering::Relaxed);
            }
        });

        // 控制请求速率
        tokio::time::sleep(Duration::from_millis(1000 / target_rps as u64)).await;
    }

    // 3. 等待所有请求完成
    while tasks.join_next().await.is_some() {}

    let elapsed = start.elapsed();
    let success = success_count.load(Ordering::Relaxed);

    // 4. 验证
    chain.add_step(
        "integration-tests",
        "SustainedLoad::duration",
        format!("Target duration: {}s, Actual: {:?}", duration_secs, elapsed),
        elapsed.as_secs() >= duration_secs as u64,
    );

    chain.add_step(
        "integration-tests",
        "SustainedLoad::success_rate",
        format!("Success: {}/{}", success, total_requests),
        success >= (total_requests as f64 * 0.99) as u64, // 99% 成功率
    );

    chain.add_step(
        "integration-tests",
        "SustainedLoad::actual_rps",
        format!(
            "Target RPS: {}, Actual RPS: {:.1}",
            target_rps,
            total_requests as f64 / elapsed.as_secs_f64()
        ),
        true,
    );

    chain.print_report();
    assert!(chain.all_passed());
}
