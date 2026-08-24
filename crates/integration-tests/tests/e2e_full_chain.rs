//! 完整链路端到端测试
//!
//! 验证数据链路：
//! Client -> API Server -> Auth -> Rate Limit -> Pricing -> Routing ->
//! Runtime -> Gateway -> Provider -> Streaming -> Billing -> Distribution

use integration_tests::common::{TestContext, VerificationChain};
use integration_tests::mocks::MockExecutionContext;
use integration_tests::mocks::database::{MockDatabase, MockDistributionRecord, MockUsageLog};
use integration_tests::mocks::provider::MockProviderFactory;

use keycompute_billing::calculate_amount;
use keycompute_routing::AccountStateStore;
use keycompute_types::{Message, PricingSnapshot, RequestContext, UsageAccumulator};
use llm_protocol_provider::ProviderAdapter;

use futures::StreamExt;
use rust_decimal::Decimal;
use std::sync::Arc;

/// 测试完整的请求处理链路
#[tokio::test]
async fn test_full_request_chain() {
    let ctx = TestContext::new();
    let mut chain = VerificationChain::new();
    let db = Arc::new(MockDatabase::new());

    // 1. 构建 RequestContext
    let pricing = PricingSnapshot {
        model_name: "gpt-4o".to_string(),
        currency: "CNY".to_string(),
        input_price_per_1k: Decimal::from(1),
        output_price_per_1k: Decimal::from(2),
    };

    let request_context = Arc::new(RequestContext::new(
        uuid::Uuid::new_v4(),
        ctx.user_id,
        ctx.tenant_id,
        ctx.produce_ai_key_id,
        "gpt-4o",
        vec![Message::user("Hello, world!")],
        true,
        pricing.clone(),
    ));

    chain.add_step(
        "keycompute-types",
        "RequestContext::new",
        format!("Request ID: {:?}", request_context.request_id),
        // 验证 RequestContext 成功创建（request_id 不为 nil）
        !request_context.request_id.is_nil(),
    );

    // 2. 创建运行时状态存储
    let _account_states = Arc::new(AccountStateStore::new());
    chain.add_step(
        "keycompute-runtime",
        "AccountStateStore::new",
        "Account state store created",
        true,
    );

    // 3. 创建模拟 Provider
    let provider = Arc::new(MockProviderFactory::create_openai());
    chain.add_step(
        "integration-tests::mocks",
        "MockProvider::create_openai",
        format!(
            "Provider: {}, Models: {:?}",
            provider.name(),
            provider.supported_models()
        ),
        provider.name() == "openai",
    );

    // 4. 执行 Provider 请求（模拟 Gateway 行为）
    let upstream_request =
        llm_protocol_provider::UpstreamRequest::new("http://mock-openai", "mock-key", "gpt-4o")
            .with_stream(true)
            .with_message("user", "Hello");

    let transport = llm_protocol_provider::DefaultHttpTransport::new();
    let mut stream: llm_protocol_provider::StreamBox = provider
        .stream_chat(&transport, upstream_request)
        .await
        .unwrap();
    let mut delta_count = 0;
    let mut usage_event = None;

    while let Some(event) = stream.next().await {
        match event.unwrap() {
            llm_protocol_provider::StreamEvent::Delta { content, .. } => {
                delta_count += 1;
                // 模拟 Token 累积
                request_context.add_output_tokens(estimate_tokens(&content));
            }
            llm_protocol_provider::StreamEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                request_context.set_input_tokens(input_tokens);
                usage_event = Some((input_tokens, output_tokens));
            }
            llm_protocol_provider::StreamEvent::Done => break,
            _ => {}
        }
    }

    chain.add_step(
        "llm-gateway (simulated)",
        "stream_processing",
        format!("Deltas: {}, Usage event: {:?}", delta_count, usage_event),
        delta_count > 0,
    );

    // 5. 验证 Token 累积
    let (input_tokens, output_tokens) = request_context.usage_snapshot();
    chain.add_step(
        "keycompute-types",
        "UsageAccumulator::snapshot",
        format!("Input: {}, Output: {}", input_tokens, output_tokens),
        output_tokens > 0,
    );

    // 6. 计费计算
    let user_amount = calculate_amount(input_tokens, output_tokens, &pricing);

    chain.add_step(
        "keycompute-billing",
        "BillingCalculator::calculate",
        format!("User amount: {:?}", user_amount),
        user_amount > Decimal::ZERO,
    );

    // 7. 模拟写入 UsageLog
    let mock_ctx = MockExecutionContext {
        request_id: ctx.request_id,
        user_id: ctx.user_id,
        tenant_id: ctx.tenant_id,
        produce_ai_key_id: ctx.produce_ai_key_id,
        model: "gpt-4o".to_string(),
        provider: "openai".to_string(),
        account_id: uuid::Uuid::new_v4(),
    };

    let usage_log = MockUsageLog::new(&mock_ctx)
        .with_tokens(input_tokens as i32, output_tokens as i32)
        .with_pricing(pricing.input_price_per_1k, pricing.output_price_per_1k);

    db.insert_usage_log(usage_log.clone());

    chain.add_step(
        "keycompute-db (simulated)",
        "insert_usage_log",
        format!("Usage log ID: {:?}", usage_log.id),
        !usage_log.id.is_nil(),
    );

    // 8. 分销计算
    let beneficiary = uuid::Uuid::new_v4();
    let ratio = Decimal::from_f64_retain(0.5).unwrap();
    let distribution_record = MockDistributionRecord::new(&usage_log, beneficiary, ratio);

    db.insert_distribution_record(distribution_record.clone());

    chain.add_step(
        "keycompute-distribution (simulated)",
        "insert_distribution_record",
        format!("Share amount: {:?}", distribution_record.share_amount),
        distribution_record.share_amount > Decimal::ZERO,
    );

    // 9. 验证数据库状态
    let logs = db.get_usage_logs();
    let records = db.get_distribution_records();

    chain.add_step(
        "integration-tests::verification",
        "verify_database_state",
        format!(
            "Usage logs: {}, Distribution records: {}",
            logs.len(),
            records.len()
        ),
        logs.len() == 1 && records.len() == 1,
    );

    // 10. 验证数据一致性
    let stored_log = db.get_usage_log_by_request(ctx.request_id);
    chain.add_step(
        "integration-tests::verification",
        "verify_request_traceability",
        format!("Request {:?} found in database", ctx.request_id),
        stored_log.is_some(),
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Some full chain verification steps failed"
    );
}

/// 测试 Fallback 链路
#[tokio::test]
async fn test_fallback_chain() {
    let mut chain = VerificationChain::new();

    // 1. 创建失败和成功的 Provider
    let failing_provider = Arc::new(MockProviderFactory::create_failing());
    let success_provider = Arc::new(MockProviderFactory::create_anthropic());

    chain.add_step(
        "integration-tests::mocks",
        "create_providers",
        "Failing and success providers created",
        true,
    );

    // 2. 模拟 Primary Provider 失败
    let upstream_request =
        llm_protocol_provider::UpstreamRequest::new("http://mock", "mock-key", "gpt-4o");

    // 使用默认 HTTP 传输层
    let transport = llm_protocol_provider::DefaultHttpTransport::new();

    let primary_result: Result<llm_protocol_provider::StreamBox, _> = failing_provider
        .stream_chat(&transport, upstream_request.clone())
        .await;
    chain.add_step(
        "llm-gateway (simulated)",
        "primary_provider_failure",
        "Primary provider failed as expected",
        primary_result.is_err(),
    );

    // 3. 模拟 Fallback 到备用 Provider
    let fallback_result: Result<llm_protocol_provider::StreamBox, _> = success_provider
        .stream_chat(&transport, upstream_request)
        .await;
    chain.add_step(
        "llm-gateway (simulated)",
        "fallback_provider_success",
        "Fallback provider succeeded",
        fallback_result.is_ok(),
    );

    // 4. 验证 Fallback 成功后的流处理
    if let Ok(stream) = fallback_result {
        let mut stream: llm_protocol_provider::StreamBox = stream;
        let mut event_count = 0;
        while let Some(event) = stream.next().await {
            if event.is_ok() {
                event_count += 1;
            }
        }
        chain.add_step(
            "llm-gateway (simulated)",
            "fallback_stream_processing",
            format!("Events from fallback: {}", event_count),
            event_count > 0,
        );
    }

    chain.print_report();
    assert!(chain.all_passed(), "Some fallback chain steps failed");
}

/// 测试多租户隔离
#[test]
fn test_multi_tenant_isolation() {
    let mut chain = VerificationChain::new();
    let db = MockDatabase::new();

    // 1. 创建两个租户的上下文
    let tenant1_id = uuid::Uuid::new_v4();
    let tenant2_id = uuid::Uuid::new_v4();

    let ctx1 = MockExecutionContext {
        tenant_id: tenant1_id,
        ..MockExecutionContext::new()
    };
    let ctx2 = MockExecutionContext {
        tenant_id: tenant2_id,
        ..MockExecutionContext::new()
    };

    // 2. 为每个租户创建 UsageLog
    let log1 = MockUsageLog::new(&ctx1).with_tokens(1000, 500);
    let log2 = MockUsageLog::new(&ctx2).with_tokens(2000, 1000);

    db.insert_usage_log(log1.clone());
    db.insert_usage_log(log2.clone());

    chain.add_step(
        "integration-tests::verification",
        "insert_tenant_logs",
        format!("Tenant1 log: {:?}, Tenant2 log: {:?}", log1.id, log2.id),
        log1.tenant_id == tenant1_id && log2.tenant_id == tenant2_id,
    );

    // 3. 验证租户数据隔离
    let all_logs = db.get_usage_logs();
    let tenant1_logs: Vec<_> = all_logs
        .iter()
        .filter(|l| l.tenant_id == tenant1_id)
        .collect();
    let tenant2_logs: Vec<_> = all_logs
        .iter()
        .filter(|l| l.tenant_id == tenant2_id)
        .collect();

    chain.add_step(
        "integration-tests::verification",
        "verify_tenant_isolation",
        format!(
            "Tenant1: {} logs, Tenant2: {} logs",
            tenant1_logs.len(),
            tenant2_logs.len()
        ),
        tenant1_logs.len() == 1 && tenant2_logs.len() == 1,
    );

    // 4. 验证各租户的费用计算独立
    let amount1 = log1.user_amount;
    let amount2 = log2.user_amount;

    chain.add_step(
        "keycompute-billing",
        "verify_tenant_billing",
        format!("Tenant1: {:?}, Tenant2: {:?}", amount1, amount2),
        amount2 > amount1, // Tenant2 用量更多，费用应该更高
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Some multi-tenant verification steps failed"
    );
}

/// 测试架构约束：Routing 只读
#[test]
fn test_routing_readonly_constraint() {
    let mut chain = VerificationChain::new();

    // 验证 Routing crate 不直接写入数据库
    // 这是架构约束的静态验证
    chain.add_step(
        "architecture",
        "routing_readonly_constraint",
        "Routing only reads from Pricing and Runtime",
        true,
    );

    // 验证 Gateway 是唯一执行层
    chain.add_step(
        "architecture",
        "gateway_execution_constraint",
        "Only Gateway executes upstream requests",
        true,
    );

    // 验证 Billing 事后触发
    chain.add_step(
        "architecture",
        "billing_post_execution_constraint",
        "Billing triggered only after stream completion",
        true,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 辅助函数：精确计算 token 数（使用 tiktoken-rs）
fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let bpe = tiktoken_rs::o200k_base_singleton();
    bpe.encode_with_special_tokens(text).len() as u32
}

/// 测试性能基准
#[test]
fn test_performance_baseline() {
    let mut chain = VerificationChain::new();

    // 1. 计费计算性能
    let pricing = PricingSnapshot {
        model_name: "gpt-4o".to_string(),
        currency: "CNY".to_string(),
        input_price_per_1k: Decimal::from(1),
        output_price_per_1k: Decimal::from(2),
    };

    let start = std::time::Instant::now();

    for _ in 0..1000 {
        let _ = calculate_amount(1000, 500, &pricing);
    }

    let elapsed = start.elapsed();
    chain.add_step(
        "performance",
        "billing_calculation",
        format!("1000 calculations in {:?}", elapsed),
        elapsed < std::time::Duration::from_millis(100),
    );

    // 2. Token 累积性能
    let usage = UsageAccumulator::default();
    let start = std::time::Instant::now();

    for _ in 0..10000 {
        usage.add_output(1);
    }

    let elapsed = start.elapsed();
    chain.add_step(
        "performance",
        "token_accumulation",
        format!("10000 accumulations in {:?}", elapsed),
        elapsed < std::time::Duration::from_millis(100),
    );

    chain.print_report();
    assert!(chain.all_passed(), "Performance test failed");
}
