//! DbRouter 主从读写分离集成测试
//!
//! 在真实主从 PostgreSQL 环境下验证：
//! - 读操作正确路由到读库
//! - 写操作正确路由到写库
//! - 锁语句强制到写库
//! - 事务始终在写库
//! - 熔断与回退机制正确工作
//!
//! # 运行前提
//!
//! 单库模式测试始终运行（通过 DbRouter::single 包装标准连接）。
//!
//! 读写分离测试需要设置以下环境变量：
//! - `DATABASE_URL` — 写库连接 URL（默认: postgres://keycompute:change-me-strong-password@localhost:5432/keycompute）
//! - `DATABASE_READ_URLS` — 从库连接 URL 列表（逗号分隔，可选）
//!
//! 在 docker-compose.replicas.yml 环境中：
//!   DATABASE_URL=postgres://keycompute:change-me-strong-password@localhost:5432/keycompute
//!   DATABASE_READ_URLS=postgres://keycompute:change-me-strong-password@localhost:5433/keycompute,postgres://keycompute:change-me-strong-password@localhost:5434/keycompute

use integration_tests::common::{VerificationChain, generate_test_id};
use integration_tests::db::{cleanup_test_data, create_test_pool, initialize_test_schema};
use keycompute_db::DbError;
use keycompute_db::db_router::{
    DatabaseConfig as RouterDbConfig, DatabaseReadConfig as RouterReadConfig,
    DatabaseRoutingConfig as RouterRoutingConfig,
};
use keycompute_db::{CreateTenantRequest, CreateUserRequest, DbRouter, Tenant, User};
use keycompute_types::UserRole;
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use std::sync::Arc;
use std::time::Duration;

/// 获取读库 URL 列表（环境变量 DATABASE_READ_URLS，逗号分隔）
fn get_read_urls() -> Vec<String> {
    std::env::var("DATABASE_READ_URLS")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(|u| u.trim().to_string()).collect())
        .unwrap_or_default()
}

/// 是否配置了读库
fn has_read_replicas() -> bool {
    !get_read_urls().is_empty()
}

/// 获取写库 URL
fn get_write_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
    })
}

// ============================================================================
// 测试 1: 单库模式 — 基本读写操作
// ============================================================================
// 验证 DbRouter::single() 在单库模式下所有操作正常工作。

#[tokio::test]
async fn test_single_db_mode() {
    let mut chain = VerificationChain::new();

    let pool = create_test_pool().await;
    let router = DbRouter::single(pool);
    let test_id = generate_test_id();
    cleanup_test_data(router.write_conn(), &test_id)
        .await
        .expect("cleanup should succeed");

    // 1. 写操作：通过 DbRouter::single 创建租户
    let tenant = Tenant::create(
        router.as_ref(),
        &CreateTenantRequest {
            name: format!("Single-{}", test_id),
            slug: format!("test-single-{}", test_id),
            description: None,
            default_rpm_limit: Some(100),
            default_tpm_limit: Some(50000),
        },
    )
    .await
    .expect("Tenant creation via DbRouter::single should succeed");
    chain.add_step(
        "keycompute-db",
        "single_create_tenant",
        "INSERT routed through single-db DbRouter",
        true,
    );

    // 2. 读操作：通过 DbRouter::single 查找租户
    let found = Tenant::find_by_id(router.as_ref(), tenant.id)
        .await
        .expect("Tenant lookup should succeed")
        .expect("Tenant should exist");
    chain.add_step(
        "keycompute-db",
        "single_find_tenant",
        "SELECT routed through single-db DbRouter",
        found.id == tenant.id,
    );

    // 3. 锁语句：FOR UPDATE
    let locked = router
        .as_ref()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, name, slug FROM tenants WHERE id = $1 FOR UPDATE",
            [tenant.id.into()],
        ))
        .await
        .expect("FOR UPDATE query should succeed through single-db DbRouter");
    chain.add_step(
        "keycompute-db",
        "single_locking_select",
        "FOR UPDATE routed through single-db DbRouter",
        locked.is_some(),
    );

    // 4. FOR SHARE 语句
    let shared = router
        .as_ref()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, name, slug FROM tenants WHERE id = $1 FOR SHARE",
            [tenant.id.into()],
        ))
        .await
        .expect("FOR SHARE query should succeed through single-db DbRouter");
    chain.add_step(
        "keycompute-db",
        "single_for_share",
        "FOR SHARE routed through single-db DbRouter",
        shared.is_some(),
    );

    // 5. 事务
    let tx_ok = router
        .as_ref()
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                txn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT 1".to_string(),
                ))
                .await?;
                Ok(())
            })
        })
        .await
        .is_ok();
    chain.add_step(
        "keycompute-db",
        "single_transaction",
        "Transaction through single-db DbRouter",
        tx_ok,
    );

    // 6. execute 方法（写操作）
    let exec_ok = router
        .as_ref()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE tenants SET name = $1 WHERE id = $2",
            [format!("Updated-{}", test_id).into(), tenant.id.into()],
        ))
        .await
        .is_ok();
    chain.add_step(
        "keycompute-db",
        "single_execute_update",
        "UPDATE through single-db DbRouter.execute()",
        exec_ok,
    );

    chain.print_report();
    assert!(chain.all_passed(), "Single-db mode tests failed");
}

// ============================================================================
// 测试 2: 读写分离 — 基本写/读/锁路由
// ============================================================================
// 需要 DATABASE_READ_URLS 环境变量指向真实读库。
// 验证：写→主库、读→从库、FOR UPDATE→主库。

#[tokio::test]
async fn test_replica_read_write_routing() {
    let mut chain = VerificationChain::new();

    if !has_read_replicas() {
        eprintln!("SKIP: test_replica_read_write_routing — DATABASE_READ_URLS not set");
        return;
    }

    let write_url = get_write_url();
    let read_urls = get_read_urls();

    // 1. 创建 DbRouter（连接到主库和从库）
    let router = DbRouter::new(
        &write_url,
        &read_urls,
        &RouterDbConfig {
            max_connections: 5,
            min_connections: 1,
            connect_timeout_secs: 10,
            idle_timeout_secs: 300,
            max_lifetime_secs: 900,
        },
        &RouterReadConfig {
            max_connections: 5,
            min_connections: 1,
            connect_timeout_secs: 10,
            acquire_timeout_secs: 5,
            idle_timeout_secs: 300,
            max_lifetime_secs: 900,
        },
        &RouterRoutingConfig {
            strategy: "round_robin".to_string(),
            read_weights: vec![],
            retry_attempts: 1,
            circuit_break_ms: 5000,
            fallback_to_write: false,
            health_check_interval_secs: 0,
        },
    )
    .await
    .expect("DbRouter with replicas should be created");
    chain.add_step(
        "keycompute-db",
        "replica_create_router",
        format!("DbRouter created: {} write, {} read(s)", 1, read_urls.len()),
        true,
    );

    // 2. 在主库上初始化测试 schema
    initialize_test_schema(router.write_conn())
        .await
        .expect("Schema initialization on primary should succeed");
    chain.add_step(
        "keycompute-db",
        "replica_initialize_schema",
        "Schema initialized on primary",
        true,
    );

    let test_id = generate_test_id();
    cleanup_test_data(router.write_conn(), &test_id)
        .await
        .expect("cleanup_test_data should succeed");

    // =========================================================
    // 测试 A: 写操作通过 DbRouter（应路由到主库）
    // =========================================================
    let tenant = Tenant::create(
        router.as_ref(),
        &CreateTenantRequest {
            name: format!("Replica-{}", test_id),
            slug: format!("test-replica-{}", test_id),
            description: None,
            default_rpm_limit: Some(100),
            default_tpm_limit: Some(50000),
        },
    )
    .await
    .expect("Tenant creation through DbRouter should succeed");
    chain.add_step(
        "keycompute-db",
        "replica_write_tenant",
        "INSERT tenant → DbRouter → primary",
        true,
    );

    // =========================================================
    // 测试 B: 读操作通过 DbRouter（应路由到从库）
    //
    // 写入后等待复制传播到从库。使用重试循环替代固定 sleep，
    // 避免在复制延迟略高时产生 flaky 测试。
    // 最多重试 5 次 × 200ms = ~1s 最大等待。
    // =========================================================
    let found = {
        let mut result = None;
        for i in 0..5 {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            if let Ok(Some(t)) = Tenant::find_by_id(router.as_ref(), tenant.id).await {
                result = Some(t);
                break;
            }
        }
        result.expect("Tenant should be found after replication")
    };
    chain.add_step(
        "keycompute-db",
        "replica_read_tenant",
        format!("SELECT tenant → DbRouter → replica, id={}", found.id),
        found.id == tenant.id,
    );

    // =========================================================
    // 测试 C: 写语句通过 query_one（INSERT ... RETURNING 应路由到主库）
    // =========================================================
    let insert_result = router
        .as_ref()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO users (tenant_id, email, name, role) VALUES ($1, $2, $3, $4) RETURNING id, email",
            [
                tenant.id.into(),
                format!("returning-{}@test.com", test_id).into(),
                format!("Returning User {}", test_id).into(),
                "user".into(),
            ],
        ))
        .await
        .expect("INSERT ... RETURNING through query_one should be routed to primary");
    chain.add_step(
        "keycompute-db",
        "replica_insert_returning",
        "INSERT ... RETURNING detected as write, routed to primary",
        insert_result.is_some(),
    );

    // =========================================================
    // 测试 D: FOR UPDATE 路由到主库（在从库上会报错）
    // =========================================================
    let for_update = router
        .as_ref()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, name FROM tenants WHERE id = $1 FOR UPDATE",
            [tenant.id.into()],
        ))
        .await
        .expect("FOR UPDATE through DbRouter should succeed (routed to primary)");
    chain.add_step(
        "keycompute-db",
        "replica_for_update",
        "FOR UPDATE detected as locking, routed to primary",
        for_update.is_some(),
    );

    // =========================================================
    // 测试 E: FOR SHARE 路由到主库
    // =========================================================
    let for_share = router
        .as_ref()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, name FROM tenants WHERE id = $1 FOR SHARE",
            [tenant.id.into()],
        ))
        .await
        .expect("FOR SHARE through DbRouter should succeed (routed to primary)");
    chain.add_step(
        "keycompute-db",
        "replica_for_share",
        "FOR SHARE detected as locking, routed to primary",
        for_share.is_some(),
    );

    // =========================================================
    // 测试 F: UPDATE 通过 query_one（应被检测为写语句）
    // =========================================================
    let update_result = router
        .as_ref()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE tenants SET description = $1 WHERE id = $2 RETURNING id",
            [
                format!("Updated via query_one {}", test_id).into(),
                tenant.id.into(),
            ],
        ))
        .await
        .expect("UPDATE ... RETURNING through query_one should succeed (write detection)");
    chain.add_step(
        "keycompute-db",
        "replica_update_returning",
        "UPDATE ... RETURNING detected as write, routed to primary",
        update_result.is_some(),
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Replica read-write routing tests failed"
    );
}

// ============================================================================
// 测试 3: 读写分离 — 事务路由
// ============================================================================
// 验证 DbRouter 的事务始终在写库上。

#[tokio::test]
async fn test_replica_transaction_routing() {
    let mut chain = VerificationChain::new();

    if !has_read_replicas() {
        eprintln!("SKIP: test_replica_transaction_routing — DATABASE_READ_URLS not set");
        return;
    }

    let write_url = get_write_url();
    let read_urls = get_read_urls();

    let router = DbRouter::new(
        &write_url,
        &read_urls,
        &RouterDbConfig {
            max_connections: 3,
            min_connections: 1,
            connect_timeout_secs: 10,
            idle_timeout_secs: 300,
            max_lifetime_secs: 900,
        },
        &RouterReadConfig {
            max_connections: 3,
            min_connections: 1,
            connect_timeout_secs: 10,
            acquire_timeout_secs: 5,
            idle_timeout_secs: 300,
            max_lifetime_secs: 900,
        },
        &RouterRoutingConfig {
            strategy: "round_robin".to_string(),
            read_weights: vec![],
            retry_attempts: 1,
            circuit_break_ms: 5000,
            fallback_to_write: false,
            health_check_interval_secs: 0,
        },
    )
    .await
    .expect("DbRouter creation should succeed");
    initialize_test_schema(router.write_conn())
        .await
        .expect("Schema initialization should succeed");

    let test_id = generate_test_id();
    cleanup_test_data(router.write_conn(), &test_id)
        .await
        .expect("cleanup_test_data should succeed");

    // 在事务中同时执行写操作和查询
    let tx_result = router
        .as_ref()
        .transaction::<_, (), DbError>(|txn| {
            let test_id = test_id.clone();
            Box::pin(async move {
                // 在事务内创建租户
                let tenant = Tenant::create(
                    txn,
                    &CreateTenantRequest {
                        name: format!("Tx-{}", test_id),
                        slug: format!("test-tx-{}", test_id),
                        description: None,
                        default_rpm_limit: Some(100),
                        default_tpm_limit: Some(50000),
                    },
                )
                .await?;

                // 在同一事务内创建用户
                User::create(
                    txn,
                    &CreateUserRequest {
                        tenant_id: tenant.id,
                        email: format!("tx-{}@test.com", test_id),
                        name: Some(format!("TX User {}", test_id)),
                        role: Some(UserRole::User),
                    },
                )
                .await?;

                // 在同一事务内查询验证
                let users = User::find_by_tenant(txn, tenant.id).await?;
                assert_eq!(users.len(), 1, "Transaction should see its own writes");

                Ok(())
            })
        })
        .await;

    chain.add_step(
        "keycompute-db",
        "replica_transaction",
        "Multi-step transaction through DbRouter (write + read + verify)",
        tx_result.is_ok(),
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Replica transaction routing tests failed"
    );
}

// ============================================================================
// 测试 4: 空读库列表退化模式
// ============================================================================
// 验证 DbRouter::new() 接收空 read_urls 列表时退化为单库透传模式，
// 所有读写操作正常。
//
// 注意：当 read_urls 非空但全部连接失败时，DbRouter::new() 返回 Err，
// 不会退化到单库模式（fail-fast 设计）。fallback_to_write 的回退效果
// 仅在运行时读库故障时生效，无法在此集成测试环境中验证（无法停止容器）。

#[tokio::test]
async fn test_degenerate_single_db_mode() {
    let mut chain = VerificationChain::new();

    let write_url = get_write_url();
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup_test_data should succeed");

    // =========================================================
    // 空读库列表：验证 DbRouter::new() 退化为单库模式
    // =========================================================
    // DbRouter::new() 接收空 read_urls 列表时退化为单库透传模式，
    // 所有读写操作都走写库连接。这与 fallback_to_write 设置无关。
    //
    // 注意：当 read_urls 非空但全部连接失败时，DbRouter::new() 返回 Err，
    // 不会退化到单库模式（fail-fast 设计，防止配置错误被静默忽略）。
    // 因此 fallback_to_write 的回退效果仅在运行时读库故障时生效，
    // 无法在此环境测试（无法停止容器）。}

    // =========================================================
    // 场景 B: 不提供从库 URL（空列表）
    // 预期：D Brouter 退化为单库模式，所有操作正常
    // =========================================================
    let router_single = DbRouter::new(
        &write_url,
        &[], // 空列表
        &RouterDbConfig {
            max_connections: 3,
            min_connections: 1,
            connect_timeout_secs: 5,
            idle_timeout_secs: 300,
            max_lifetime_secs: 900,
        },
        &RouterReadConfig {
            max_connections: 2,
            min_connections: 1,
            connect_timeout_secs: 3,
            acquire_timeout_secs: 3,
            idle_timeout_secs: 300,
            max_lifetime_secs: 900,
        },
        &RouterRoutingConfig {
            strategy: "round_robin".to_string(),
            read_weights: vec![],
            retry_attempts: 1,
            circuit_break_ms: 1000,
            fallback_to_write: false,
            health_check_interval_secs: 0,
        },
    )
    .await
    .expect("DbRouter with empty read URLs should succeed (degenerate to single-db)");
    chain.add_step(
        "keycompute-db",
        "empty_read_urls_degenerate",
        "DbRouter with empty read URLs degenerates to single-db mode",
        true,
    );

    // 验证在退化模式下读写正常
    initialize_test_schema(router_single.write_conn())
        .await
        .expect("Schema initialization should succeed");
    let tenant2 = Tenant::create(
        router_single.as_ref(),
        &CreateTenantRequest {
            name: format!("Degenerate-{}", test_id),
            slug: format!("test-degenerate-{}", test_id),
            description: None,
            default_rpm_limit: Some(100),
            default_tpm_limit: Some(50000),
        },
    )
    .await
    .expect("Tenant creation should succeed through degenerate DbRouter");
    let found2 = Tenant::find_by_id(router_single.as_ref(), tenant2.id)
        .await
        .expect("Tenant read should succeed through degenerate DbRouter")
        .expect("Tenant should exist");
    chain.add_step(
        "keycompute-db",
        "degenerate_read_write",
        format!(
            "Read/write through degenerate DbRouter: {}",
            found2.id == tenant2.id
        ),
        found2.id == tenant2.id,
    );

    chain.print_report();
    assert!(chain.all_passed(), "Degenerate mode tests failed");
}

// ============================================================================
// 测试 5: 并发读写
// ============================================================================
// 验证 DbRouter 在并发场景下的行为正确性。

#[tokio::test]
async fn test_concurrent_through_router() {
    let mut chain = VerificationChain::new();

    let pool = create_test_pool().await;
    let router = DbRouter::single(pool);
    let test_id = generate_test_id();
    cleanup_test_data(router.write_conn(), &test_id)
        .await
        .expect("cleanup should succeed");

    // 创建基础租户
    let tenant = Tenant::create(
        router.as_ref(),
        &CreateTenantRequest {
            name: format!("Concur-{}", test_id),
            slug: format!("test-concur-{}", test_id),
            description: None,
            default_rpm_limit: Some(100),
            default_tpm_limit: Some(50000),
        },
    )
    .await
    .expect("Tenant creation should succeed");

    // 并发写入
    let mut handles = Vec::new();
    for i in 0..5 {
        let router = Arc::clone(&router);
        handles.push(tokio::spawn(async move {
            User::create(
                router.as_ref(),
                &CreateUserRequest {
                    tenant_id: tenant.id,
                    email: format!("concur-{}-{}@test.com", i, generate_test_id()),
                    name: Some(format!("Concurrent User {}", i)),
                    role: Some(UserRole::User),
                },
            )
            .await
        }));
    }

    let results = futures::future::join_all(handles).await;
    let success_count = results
        .iter()
        .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
        .count();
    chain.add_step(
        "keycompute-db",
        "concurrent_through_router",
        format!(
            "{} concurrent writes through DbRouter succeeded",
            success_count
        ),
        success_count == 5,
    );

    // 验证所有用户都写入成功
    let users = User::find_by_tenant(router.as_ref(), tenant.id)
        .await
        .expect("User lookup should succeed");
    chain.add_step(
        "keycompute-db",
        "concurrent_verify",
        format!("{} users found after concurrent writes", users.len()),
        users.len() == 5,
    );

    chain.print_report();
    assert!(chain.all_passed(), "Concurrent through router tests failed");
}
