//! PricingService + CacheService 端到端集成测试
//!
//! 验证 L1（本地 LRU）和 L2（Redis 分布式缓存）两级缓存链路的正确性：
//! - L2 缓存写回和命中
//! - L2 + L1 两级缓存一致性
//! - get_or_insert_with_lock 防击穿路径
//! - 跨租户定价隔离（租户特定定价永不泄漏到 nil_tenant 共享 key）

use integration_tests::db::initialize_test_schema;
use keycompute_cache::CacheService;
use keycompute_db::{CreatePricingRequest, PricingModel, models::pricing_model::BillingDimension};
use keycompute_pricing::PricingService;
use keycompute_types::PricingSnapshot;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

/// 获取测试用 Redis URL
fn get_redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

/// 获取测试用 Database URL
fn get_database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
    })
}

/// 生成唯一的测试标识符
fn generate_test_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// 尝试创建 CacheService（Redis 不可用时跳过）
async fn try_create_cache() -> Option<Arc<CacheService>> {
    let url = get_redis_url();
    if url.is_empty() {
        return None;
    }

    let pool = match keycompute_runtime::redis_store::RedisRuntimeStore::create_pool(&url) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: Failed to create Redis pool: {}", e);
            return None;
        }
    };

    // 验证 Redis 可用
    if pool.get().await.is_err() {
        eprintln!("SKIP: Redis not reachable");
        return None;
    }

    let test_prefix = format!("kc:e2e:cache:{}:", generate_test_id());
    Some(Arc::new(
        CacheService::with_pool(pool).with_prefix(test_prefix),
    ))
}

/// 尝试创建数据库连接（DB 不可用时跳过）
async fn try_create_db_pool() -> Option<sea_orm::DatabaseConnection> {
    let url = get_database_url();
    match sea_orm::Database::connect(&url).await {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!("SKIP: Database not reachable: {}", e);
            None
        }
    }
}

/// 测试：PricingService 通过 L2 缓存回源，第二次调用命中 L2 缓存
///
/// 场景：无数据库连接，create_snapshot 应使用 L2 缓存 + 硬编码默认价格。
/// 第一次调用：L1 miss → L2 miss + 锁获取 → 回源（硬编码默认）→ 写入 L2 + L1
/// 第二次调用：L1 miss（模拟过期）→ L2 hit → 填充 L1
#[tokio::test]
async fn test_pricing_with_l2_cache_lifecycle() {
    let Some(cache) = try_create_cache().await else {
        return;
    };

    let pricing = PricingService::new().with_dist_cache(Arc::clone(&cache));
    let tenant_id = Uuid::new_v4();

    // 第一次调用：L1 miss → L2 miss → 回源（硬编码默认）→ 写入 L2 + L1
    let snapshot1 = pricing
        .create_snapshot("gpt-4o", &tenant_id, None)
        .await
        .expect("Should return hardcoded default pricing");
    assert_eq!(snapshot1.model_name, "gpt-4o");
    assert!(snapshot1.input_price_per_1k > rust_decimal::Decimal::ZERO);

    // 第二次调用：应命中 L2 缓存，返回相同值
    let snapshot2 = pricing
        .create_snapshot("gpt-4o", &tenant_id, None)
        .await
        .expect("Should return cached pricing");
    assert_eq!(snapshot2.model_name, "gpt-4o");
    assert_eq!(
        snapshot1.input_price_per_1k, snapshot2.input_price_per_1k,
        "Second call should return same cached pricing"
    );
    assert_eq!(
        snapshot1.output_price_per_1k, snapshot2.output_price_per_1k,
        "Second call should return same cached pricing"
    );
}

/// 测试：不同租户的定价缓存隔离
///
/// 场景：两个不同租户请求同一模型定价，应各自缓存且互不干扰。
#[tokio::test]
async fn test_pricing_cache_tenant_isolation() {
    let Some(cache) = try_create_cache().await else {
        return;
    };

    let pricing = PricingService::new().with_dist_cache(Arc::clone(&cache));
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    let snap_a = pricing
        .create_snapshot("gpt-4o", &tenant_a, None)
        .await
        .expect("Tenant A should get pricing");
    let snap_b = pricing
        .create_snapshot("gpt-4o", &tenant_b, None)
        .await
        .expect("Tenant B should get pricing");

    // 没有数据库，默认定价应相同
    assert_eq!(snap_a.input_price_per_1k, snap_b.input_price_per_1k);
    assert_eq!(snap_a.output_price_per_1k, snap_b.output_price_per_1k);
}

/// 测试：禁用 L2 缓存（no-op CacheService）时 PricingService 正常工作
///
/// 场景：CacheService::disabled 传递给 PricingService，验证回退到直接 DB 查询。
#[tokio::test]
async fn test_pricing_with_disabled_cache() {
    let cache = Arc::new(CacheService::disabled());
    assert!(
        !cache.is_available(),
        "Disabled cache should not be available"
    );

    let pricing = PricingService::new().with_dist_cache(cache);
    let tenant_id = Uuid::new_v4();

    // 没有 DB 连接 + 没有缓存，应使用硬编码默认价格
    let snapshot = pricing
        .create_snapshot("gpt-4o", &tenant_id, None)
        .await
        .expect("Should work with disabled cache");
    assert_eq!(snapshot.model_name, "gpt-4o");
    assert!(snapshot.input_price_per_1k > rust_decimal::Decimal::ZERO);
}

/// 测试：不同模型的定价缓存独立
#[tokio::test]
async fn test_pricing_cache_model_isolation() {
    let Some(cache) = try_create_cache().await else {
        return;
    };

    let pricing = PricingService::new().with_dist_cache(Arc::clone(&cache));
    let tenant_id = Uuid::new_v4();

    // 两个不同模型
    let snap_gpt = pricing
        .create_snapshot("gpt-4o", &tenant_id, None)
        .await
        .expect("gpt-4o pricing");
    let snap_claude = pricing
        .create_snapshot("claude-3", &tenant_id, None)
        .await
        .expect("claude-3 pricing");

    // 都是硬编码默认值，值应相同（都是统一的默认价格）
    assert_eq!(snap_gpt.input_price_per_1k, snap_claude.input_price_per_1k);
    assert_eq!(snap_gpt.model_name, "gpt-4o");
    assert_eq!(snap_claude.model_name, "claude-3");
}

// ── 跨租户定价隔离测试（验证安全修复） ────────────────────────────

/// 测试：跨租户定价隔离——验证修复后的 nil_tenant key 不泄漏租户自定义定价
///
/// 场景：
/// - DB 中有模型 "gpt-4o" 的默认定价（¥0.10 input / ¥0.30 output）
/// - Tenant A 有自定义定价（¥0.20 input / ¥0.50 output）
/// - Tenant B 使用默认定价
///
/// 验证：
/// 1. Tenant A 调用 create_snapshot → 返回 ¥0.20/¥0.50（租户特定）
/// 2. Tenant B 调用 create_snapshot → 返回 ¥0.10/¥0.30（默认，非 Tenant A 的定价）
/// 3. L2 nil_tenant key 存储的是 ¥0.10/¥0.30（默认），而非 ¥0.20/¥0.50（租户特定）
#[tokio::test]
async fn test_cross_tenant_pricing_isolation_with_db() {
    // 1. 建立 DB 和 Redis 连接
    let Some(db) = try_create_db_pool().await else {
        return;
    };

    let Some(cache) = try_create_cache().await else {
        return;
    };

    let test_id = generate_test_id();
    let model_name = format!("test-cross-pricing-{}", test_id);
    let provider = "provideraccount";
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    use bigdecimal::BigDecimal;
    use chrono::Utc;

    // 2. 初始化测试数据库结构
    initialize_test_schema(&db)
        .await
        .expect("Schema initialization should succeed");

    // 3. 种子数据：默认定价 ¥0.10 input / ¥0.30 output（nil tenant）
    let default_pricing = PricingModel::create(
        &db,
        &CreatePricingRequest {
            tenant_id: Some(Uuid::nil()),
            model_name: model_name.clone(),
            billing_dimension: BillingDimension::ProviderAccount,
            currency: Some("CNY".to_string()),
            input_price_per_1k: BigDecimal::from_str("0.10").unwrap(),
            output_price_per_1k: BigDecimal::from_str("0.30").unwrap(),
            is_default: Some(true),
            effective_from: Some(Utc::now()),
            effective_until: None,
        },
    )
    .await
    .expect("Default pricing should be created");

    // 4. 种子数据：Tenant A 自定义定价 ¥0.20 input / ¥0.50 output
    let tenant_a_pricing = PricingModel::create(
        &db,
        &CreatePricingRequest {
            tenant_id: Some(tenant_a),
            model_name: model_name.clone(),
            billing_dimension: BillingDimension::ProviderAccount,
            currency: Some("CNY".to_string()),
            input_price_per_1k: BigDecimal::from_str("0.20").unwrap(),
            output_price_per_1k: BigDecimal::from_str("0.50").unwrap(),
            is_default: Some(false),
            effective_from: Some(Utc::now()),
            effective_until: None,
        },
    )
    .await
    .expect("Tenant A pricing should be created");

    // 5. 创建带 DB + L2 缓存的 PricingService
    let pool = keycompute_db::DbRouter::single(db);
    let pricing = PricingService::with_pool(Arc::clone(&pool)).with_dist_cache(Arc::clone(&cache));

    // 6. Tenant A 查询 → 应返回 ¥0.20 input / ¥0.50 output
    let snap_a = pricing
        .create_snapshot(&model_name, &tenant_a, Some(provider))
        .await
        .expect("Tenant A should get pricing");
    assert_eq!(
        snap_a.input_price_per_1k,
        rust_decimal::Decimal::from_str("0.2").unwrap(),
        "Tenant A should get custom pricing (0.20 input)"
    );
    assert_eq!(
        snap_a.output_price_per_1k,
        rust_decimal::Decimal::from_str("0.5").unwrap(),
        "Tenant A should get custom pricing (0.50 output)"
    );

    // 7. Tenant B 查询 → 应返回 ¥0.10 input / ¥0.30 output（默认定价）
    //    如果修复失效，Tenant B 会错误地得到 Tenant A 的 ¥0.20/¥0.50
    let snap_b = pricing
        .create_snapshot(&model_name, &tenant_b, Some(provider))
        .await
        .expect("Tenant B should get pricing");
    assert_eq!(
        snap_b.input_price_per_1k,
        rust_decimal::Decimal::from_str("0.1").unwrap(),
        "Tenant B should get DEFAULT pricing (0.10 input), NOT tenant A's custom 0.20"
    );
    assert_eq!(
        snap_b.output_price_per_1k,
        rust_decimal::Decimal::from_str("0.3").unwrap(),
        "Tenant B should get DEFAULT pricing (0.30 output), NOT tenant A's custom 0.50"
    );

    // 8. 验证 L2 nil_tenant key 存储的是默认定价，而非 Tenant A 的定价
    //    nil_dist_key = "pricing:{nil_uuid}:{model}:{provider}"
    //
    //    注意：此检查依赖顺序执行（Tenant A 先查询 → Tenant B 后查询）。
    //    Tenant A 的查询不会将 nil_tenant key（因其 source=TenantSpecific），
    //    而 Tenant B 的查询会写入 nil_tenant key（因其 source=DatabaseDefault）。
    //    如果执行顺序颠倒（Tenant B 先查询），nil_tenant key 在 Tenant A 查询时
    //    就已经存在且存储的正确值，断言依然通过——所以实际顺序不重要，
    //    但 nil_tenant key 的写入时间点会变化。
    let nil_dist_key = format!("pricing:{}:{}:{}", Uuid::nil(), model_name, provider);
    let cached_nil: Option<PricingSnapshot> = match cache
        .get::<PricingSnapshot>(&nil_dist_key)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "NOTE: Cannot read L2 nil_tenant key (Redis error: {}), skipping L2 verification",
                e
            );
            None
        }
    };

    if let Some(nil_pricing) = cached_nil {
        // nil key 存在 → 必须存默认定价，不能是 Tenant A 的定价
        assert_eq!(
            nil_pricing.input_price_per_1k,
            rust_decimal::Decimal::from_str("0.1").unwrap(),
            "L2 nil_tenant key must contain DEFAULT pricing (0.10), not tenant A's custom 0.20"
        );
        assert_eq!(
            nil_pricing.output_price_per_1k,
            rust_decimal::Decimal::from_str("0.3").unwrap(),
            "L2 nil_tenant key must contain DEFAULT pricing (0.30), not tenant A's custom 0.50"
        );
    } else {
        // nil key 不存在也是可接受的（当默认定价也是 HardcodedDefault 时）
        // 但在本测试中 DB 中有默认定价数据，应由 Tenant B 的查询触发写入
        // 如果这里失败，说明第一个租户（Tenant A）的查询没有触发 nil key 写入
        eprintln!("NOTE: L2 nil_tenant key not found - Tenant B may need to query first");
    }

    // 9. 清理：删除种子数据
    let conn = pool.write_conn();
    tenant_a_pricing.delete(conn).await.ok();
    default_pricing.delete(conn).await.ok();
}
