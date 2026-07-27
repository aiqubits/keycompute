//! 小费和提现流程端到端测试
//!
//! 验证完整业务流程:
//! - Token 审批流程 (approve/reject)
//! - 小费计算流程 (create_from_usage_log)
//! - 提现流程 (创建、审批、完成、拒绝)

use bigdecimal::BigDecimal;
use chrono::Utc;
use dotenv::dotenv;
use integration_tests::common::VerificationChain;
use keycompute_db::models::system_setting::setting_keys::NODE_TIP_RATIO;
use keycompute_db::models::{
    api_key::{CreateProduceAiKeyRequest, ProduceAiKey},
    node::*,
    node_tip::*,
    node_tip_withdrawal::*,
    system_setting::SystemSetting,
    tenant::*,
    usage_log::{CreateUsageLogRequest, UsageLog},
    user::*,
    user_node_gateway_token::*,
};
use keycompute_types::UserRole;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

/// 测试环境
#[allow(dead_code)]
struct TipWithdrawalTestEnv {
    pool: DatabaseConnection,
    test_user_id: Uuid,
    admin_user_id: Uuid,
    tenant_id: Uuid,
    produce_ai_key_id: Uuid,
    node_id: Uuid,
}

impl TipWithdrawalTestEnv {
    /// 创建测试环境
    async fn new(suffix: &str) -> anyhow::Result<Self> {
        // 加载 .env 文件（如果存在）
        dotenv().ok();

        // 使用 PgConnectOptions 构建连接（正确处理密码中的特殊字符）
        let db = std::env::var("POSTGRES_DB").unwrap_or_else(|_| "keycompute".to_string());
        let user = std::env::var("POSTGRES_USER").unwrap_or_else(|_| "keycompute".to_string());
        let password =
            std::env::var("POSTGRES_PASSWORD").unwrap_or_else(|_| "password".to_string());

        // 调试输出
        println!(
            "Debug: db={}, user={}, password length={}",
            db,
            user,
            password.len()
        );

        let database_url = format!("postgres://{}:{}@localhost:5432/{}", user, password, db);
        use sea_orm::ConnectOptions;
        let mut opt = ConnectOptions::new(&database_url);
        opt.max_connections(5);
        let pool = Database::connect(opt)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to database: {}", e))?;

        // 初始化测试库结构；失败时直接终止，避免在不完整 schema 上运行测试。
        integration_tests::db::initialize_test_schema(&pool)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize database schema: {}", e))?;

        // 清理历史测试数据
        Self::cleanup_test_data(&pool).await?;

        // 创建测试租户
        let tenant = Tenant::create(
            &pool,
            &CreateTenantRequest {
                name: format!("tip-test-tenant-{}", suffix),
                slug: format!("tip-test-{}-{}", suffix, Uuid::new_v4()),
                description: Some("Tip withdrawal test tenant".to_string()),
                default_rpm_limit: Some(100),
                default_tpm_limit: Some(50000),
            },
        )
        .await?;

        // 创建测试用户（普通用户）
        let test_user = User::create(
            &pool,
            &CreateUserRequest {
                tenant_id: tenant.id,
                email: format!("tip-test-user-{}@test.local", suffix),
                name: Some(format!("Tip Test User {}", suffix)),
                role: None, // 默认为 user
            },
        )
        .await?;

        // 创建管理员用户
        let admin_user = User::create(
            &pool,
            &CreateUserRequest {
                tenant_id: tenant.id,
                email: format!("tip-test-admin-{}@test.local", suffix),
                name: Some(format!("Tip Test Admin {}", suffix)),
                role: Some(UserRole::Admin),
            },
        )
        .await?;

        // 为测试用户创建 API Key（用于创建 usage_log）
        let api_key = ProduceAiKey::create(
            &pool,
            &CreateProduceAiKeyRequest {
                tenant_id: tenant.id,
                user_id: test_user.id,
                name: format!("tip-test-key-{}", suffix),
                produce_ai_key_hash: format!("hash-tip-{}-{}", suffix, Uuid::new_v4()),
                produce_ai_key_preview: "sk-tip-****".to_string(),
                expires_at: None,
            },
        )
        .await?;

        // 创建测试节点（属于普通用户）
        let node = Node::create(
            &pool,
            &CreateNodeRequest {
                owner_user_id: test_user.id,
                client_instance_id: format!("test-client-{}", suffix),
                display_name: format!("test-node-{}", suffix),
                capabilities_json: serde_json::json!({
                    "runtime": "ollama",
                    "models": [{"model": "deepseek-chat"}]
                }),
            },
        )
        .await?;

        // 设置小费比例（如果没有设置）
        let tip_ratio = SystemSetting::get_string(&pool, NODE_TIP_RATIO, "0.90").await;
        if tip_ratio == "0.90" {
            SystemSetting::update_value(&pool, NODE_TIP_RATIO, "0.90").await?;
        }

        Ok(Self {
            pool,
            test_user_id: test_user.id,
            admin_user_id: admin_user.id,
            tenant_id: tenant.id,
            produce_ai_key_id: api_key.id,
            node_id: node.id,
        })
    }

    /// 清理测试数据
    async fn cleanup_test_data(pool: &DatabaseConnection) -> anyhow::Result<()> {
        // 按 FK 依赖逆序删除
        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM node_tip_withdrawals WHERE user_id IN (SELECT id FROM users WHERE email LIKE 'tip-test-%')", []))
            .await?;

        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM node_tips WHERE owner_user_id IN (SELECT id FROM users WHERE email LIKE 'tip-test-%')", []))
            .await?;

        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM usage_logs WHERE user_id IN (SELECT id FROM users WHERE email LIKE 'tip-test-%')", []))
            .await?;

        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM node_tasks WHERE user_id IN (SELECT id FROM users WHERE email LIKE 'tip-test-%')", []))
            .await?;

        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM nodes WHERE owner_user_id IN (SELECT id FROM users WHERE email LIKE 'tip-test-%')", []))
            .await?;

        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM user_node_gateway_tokens WHERE user_id IN (SELECT id FROM users WHERE email LIKE 'tip-test-%')", []))
            .await?;

        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM produce_ai_keys WHERE user_id IN (SELECT id FROM users WHERE email LIKE 'tip-test-%')", []))
            .await?;

        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM users WHERE email LIKE 'tip-test-%'",
            [],
        ))
        .await?;

        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM tenants WHERE slug LIKE 'tip-test-%'",
            [],
        ))
        .await?;

        Ok(())
    }

    /// 创建模拟的 usage_log 和对应的 node_task，触发小费计算
    /// consumer 设置为管理员用户，与节点所有者不同，从而确保小费产生
    async fn create_usage_log_with_tip(&self, bill_amount: f64) -> anyhow::Result<Uuid> {
        // 1. 创建 usage_log
        let request_id = Uuid::new_v4();

        let now = Utc::now();
        let user_amount =
            BigDecimal::try_from(bill_amount).map_err(|e| anyhow::anyhow!("{:?}", e))?;

        let usage_log = UsageLog::create(
            &self.pool,
            &CreateUsageLogRequest {
                request_id,
                tenant_id: self.tenant_id,
                user_id: self.admin_user_id,
                produce_ai_key_id: self.produce_ai_key_id,
                model_name: "deepseek-chat".to_string(),
                provider_name: "openai".to_string(),
                account_id: Uuid::new_v4(),
                input_tokens: 100,
                output_tokens: 50,
                input_unit_price_snapshot: BigDecimal::from(1),
                output_unit_price_snapshot: BigDecimal::from(2),
                user_amount,
                currency: "CNY".to_string(),
                usage_source: "api".to_string(),
                status: "success".to_string(),
                started_at: now,
                finished_at: now,
            },
        )
        .await?;

        // 2. 创建对应的 node_task（成功的任务，关联到测试节点）
        self.pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id)
            VALUES ($1, $2, 'deepseek-chat', '{}', 'succeeded', $3)
            "#,
            [request_id.into(), self.admin_user_id.into(), self.node_id.into()],
        ))
        .await?;

        Ok(usage_log.id)
    }
}

/// 测试 1: Token 审批流程
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_token_approval_workflow() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("token_approve").await?;
    let mut chain = VerificationChain::new();

    // 1. 创建一个 pending token
    let (token_id, _token_plaintext, token_hash, token_preview) =
        UserNodeGatewayToken::generate_hmac_token(b"test-secret-key");

    let token = UserNodeGatewayToken::create_with_id(
        &env.pool,
        token_id,
        env.test_user_id,
        &token_hash,
        &token_preview,
    )
    .await?;

    chain.add_step(
        "keycompute-db",
        "token::create",
        format!("Token created: {}", token.id),
        token.status == TOKEN_STATUS_PENDING && token.user_id == env.test_user_id,
    );

    // 2. 审批 token
    let approve_result = token.approve(&env.pool, env.admin_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "token::approve",
        "Token approved".to_string(),
        approve_result,
    );

    // 3. 验证 token 状态变为 approved
    let approved_token = UserNodeGatewayToken::find_by_id(&env.pool, token_id)
        .await?
        .unwrap();

    chain.add_step(
        "keycompute-db",
        "token::status_approved",
        format!("Token status: {}", approved_token.status),
        approved_token.status == TOKEN_STATUS_APPROVED,
    );

    // 4. 拒绝另一个 token
    let (token_id2, _, token_hash2, token_preview2) =
        UserNodeGatewayToken::generate_hmac_token(b"test-secret-key-2");

    let token2 = UserNodeGatewayToken::create_with_id(
        &env.pool,
        token_id2,
        env.test_user_id,
        &token_hash2,
        &token_preview2,
    )
    .await?;

    let reject_result = token2.reject(&env.pool, env.admin_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "token::reject",
        "Token rejected".to_string(),
        reject_result,
    );

    // 5. 验证 token 状态变为 rejected
    let rejected_token = UserNodeGatewayToken::find_by_id(&env.pool, token_id2)
        .await?
        .unwrap();

    chain.add_step(
        "keycompute-db",
        "token::status_rejected",
        format!("Token status: {}", rejected_token.status),
        rejected_token.status == TOKEN_STATUS_REJECTED,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 2: 小费计算流程
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_tip_calculation() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("tip_calc").await?;
    let mut chain = VerificationChain::new();

    // 1. 创建 usage_log（账单金额 100 CNY）
    let usage_log_id = env.create_usage_log_with_tip(100.0).await?;

    chain.add_step(
        "keycompute-db",
        "tip::create_usage_log",
        format!("Usage log created: {}", usage_log_id),
        true,
    );

    // 2. 调用小费计算
    let tip = NodeTip::create_from_usage_log(&env.pool, usage_log_id).await?;

    chain.add_step(
        "keycompute-db",
        "tip::create_from_usage_log",
        format!("Tip created: {:?}", tip.as_ref().map(|t| t.id)),
        tip.is_some(),
    );

    // 3. 验证小费金额（100 * 0.9 = 90）
    if let Some(ref t) = tip {
        let expected_tip = rust_decimal::Decimal::from_f64_retain(90.0).unwrap();

        chain.add_step(
            "keycompute-db",
            "tip::amount_calculation",
            format!("Tip amount: {} (expected: 90)", t.tip_amount),
            t.tip_amount == expected_tip,
        );

        chain.add_step(
            "keycompute-db",
            "tip::owner_user_id",
            format!("Owner user ID: {}", t.owner_user_id),
            t.owner_user_id == env.test_user_id,
        );
    }

    // 4. 验证小费汇总
    let summary = NodeTip::get_summary(&env.pool, env.test_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "tip::summary",
        format!(
            "Pending amount: {}, Total amount: {}",
            summary.pending_amount, summary.total_amount
        ),
        summary.pending_amount > rust_decimal::Decimal::ZERO,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 3: 小费计算幂等性（重复调用不会创建多条记录）
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_tip_calculation_idempotency() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("tip_idempotent").await?;
    let mut chain = VerificationChain::new();

    // 1. 创建 usage_log
    let usage_log_id = env.create_usage_log_with_tip(50.0).await?;

    // 2. 第一次调用小费计算
    let tip1 = NodeTip::create_from_usage_log(&env.pool, usage_log_id).await?;

    chain.add_step(
        "keycompute-db",
        "tip::first_creation",
        format!("First tip: {:?}", tip1.as_ref().map(|t| t.id)),
        tip1.is_some(),
    );

    // 3. 第二次调用（应该返回 None，因为 ON CONFLICT DO NOTHING）
    let tip2 = NodeTip::create_from_usage_log(&env.pool, usage_log_id).await?;

    chain.add_step(
        "keycompute-db",
        "tip::second_creation_idempotent",
        format!("Second tip: {:?} (should be None)", tip2),
        tip2.is_none(),
    );

    // 4. 验证只有一条小费记录
    let tips_count = NodeTip::count_by_user(&env.pool, env.test_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "tip::count_check",
        format!("Tips count: {} (expected: 1)", tips_count),
        tips_count == 1,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 4: 创建提现申请（balance 方式）
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_create_balance_withdrawal() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("balance_withdrawal").await?;
    let mut chain = VerificationChain::new();

    // 1. 创建小费记录
    let usage_log_id = env.create_usage_log_with_tip(100.0).await?;
    let _tip = NodeTip::create_from_usage_log(&env.pool, usage_log_id).await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::tip_created",
        "Tip created for withdrawal".to_string(),
        true,
    );

    // 2. 验证有待提现小费
    let summary = NodeTip::get_summary(&env.pool, env.test_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::pending_check",
        format!("Pending amount: {}", summary.pending_amount),
        summary.pending_amount > rust_decimal::Decimal::ZERO,
    );

    // 注意：由于 create_tip_withdrawal 是 handler 层函数，需要模拟 Axum 请求
    // 这里直接测试数据库层面的操作

    // 3. 创建提现记录（模拟 balance 方式）
    let tx = env.pool.begin().await?;

    let withdrawal = NodeTipWithdrawal::create(
        &tx,
        env.test_user_id,
        WITHDRAWAL_TYPE_BALANCE,
        summary.pending_amount,
        None, // 不需要加密的支付宝账号
        None, // 不需要加密的真实姓名
    )
    .await?;

    // balance 方式自动完成
    let completed_withdrawal = NodeTipWithdrawal::mark_completed(
        &tx,
        withdrawal.id,
        None, // 自助提现，无 admin
        None, // 保留 admin_remark
        None, // 使用创建时的金额
    )
    .await?;

    tx.commit().await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::balance_completed",
        format!("Withdrawal status: {}", completed_withdrawal.status),
        completed_withdrawal.status == WITHDRAWAL_STATUS_COMPLETED,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 5: 创建提现申请（alipay 方式）并审批流程
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_alipay_withdrawal_approval_workflow() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("alipay_approval").await?;
    let mut chain = VerificationChain::new();

    // 1. 创建小费记录
    let usage_log_id = env.create_usage_log_with_tip(200.0).await?;
    let _tip = NodeTip::create_from_usage_log(&env.pool, usage_log_id).await?;

    // 2. 创建 alipay 提现申请
    let tx = env.pool.begin().await?;

    let summary = NodeTip::get_summary(&env.pool, env.test_user_id).await?;

    // 模拟加密的支付宝账号和姓名
    let encrypted_alipay = Some("encrypted_alipay_account_data".to_string());
    let encrypted_name = Some("encrypted_real_name_data".to_string());

    let withdrawal = NodeTipWithdrawal::create(
        &tx,
        env.test_user_id,
        WITHDRAWAL_TYPE_ALIPAY,
        summary.pending_amount,
        encrypted_alipay.as_deref(),
        encrypted_name.as_deref(),
    )
    .await?;

    tx.commit().await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::alipay_created",
        format!(
            "Withdrawal created: {}, status: {}",
            withdrawal.id, withdrawal.status
        ),
        withdrawal.status == WITHDRAWAL_STATUS_PENDING,
    );

    // 3. 管理员审批通过
    let tx2 = env.pool.begin().await?;

    let approved_withdrawal = NodeTipWithdrawal::approve(
        &tx2,
        withdrawal.id,
        env.admin_user_id,
        Some("Approved by admin"),
    )
    .await?;

    tx2.commit().await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::approve",
        format!(
            "Withdrawal status after approve: {}",
            approved_withdrawal.status
        ),
        approved_withdrawal.status == WITHDRAWAL_STATUS_APPROVED,
    );

    // 4. 管理员完成提现（线下打款后）
    let tx3 = env.pool.begin().await?;

    let completed_withdrawal = NodeTipWithdrawal::mark_completed(
        &tx3,
        withdrawal.id,
        Some(env.admin_user_id),
        Some("Payment sent"),
        None, // 使用创建时的金额
    )
    .await?;

    tx3.commit().await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::complete",
        format!(
            "Withdrawal status after complete: {}",
            completed_withdrawal.status
        ),
        completed_withdrawal.status == WITHDRAWAL_STATUS_COMPLETED,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 6: 拒绝提现申请
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_reject_withdrawal() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("reject_withdrawal").await?;
    let mut chain = VerificationChain::new();

    // 1. 创建小费记录
    let usage_log_id = env.create_usage_log_with_tip(150.0).await?;
    let _tip = NodeTip::create_from_usage_log(&env.pool, usage_log_id).await?;

    // 2. 创建 alipay 提现申请
    let tx = env.pool.begin().await?;

    let summary = NodeTip::get_summary(&env.pool, env.test_user_id).await?;

    let withdrawal = NodeTipWithdrawal::create(
        &tx,
        env.test_user_id,
        WITHDRAWAL_TYPE_ALIPAY,
        summary.pending_amount,
        Some("encrypted_alipay"),
        Some("encrypted_name"),
    )
    .await?;

    tx.commit().await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::created_for_reject",
        format!("Withdrawal created: {}", withdrawal.id),
        withdrawal.status == WITHDRAWAL_STATUS_PENDING,
    );

    // 3. 拒绝提现
    let tx2 = env.pool.begin().await?;

    let rejected_withdrawal = NodeTipWithdrawal::reject(
        &tx2,
        withdrawal.id,
        env.admin_user_id,
        Some("Rejected by admin"),
    )
    .await?;

    tx2.commit().await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::reject",
        format!(
            "Withdrawal status after reject: {}",
            rejected_withdrawal.status
        ),
        rejected_withdrawal.status == WITHDRAWAL_STATUS_REJECTED,
    );

    // 4. 验证 rejected 提现不影响待提现金额
    let summary = NodeTip::get_summary(&env.pool, env.test_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::pending_amount_after_reject",
        format!("Pending amount after reject: {}", summary.pending_amount),
        summary.pending_amount > rust_decimal::Decimal::ZERO,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 7: 小费计算边界条件（自己消费自己的节点不产生小费）
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_tip_self_consumption_no_tip() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("self_consumption").await?;
    let mut chain = VerificationChain::new();

    // 创建 usage_log，但 consumer = test_user_id（与节点所有者相同，自消费不产生小费）
    let request_id = Uuid::new_v4();
    let now = Utc::now();

    let usage_log = UsageLog::create(
        &env.pool,
        &CreateUsageLogRequest {
            request_id,
            tenant_id: env.tenant_id,
            user_id: env.test_user_id, // 消费者 = test_user = 节点所有者
            produce_ai_key_id: env.produce_ai_key_id,
            model_name: "deepseek-chat".to_string(),
            provider_name: "openai".to_string(),
            account_id: Uuid::new_v4(),
            input_tokens: 100,
            output_tokens: 50,
            input_unit_price_snapshot: BigDecimal::from(1),
            output_unit_price_snapshot: BigDecimal::from(2),
            user_amount: BigDecimal::from(100),
            currency: "CNY".to_string(),
            usage_source: "api".to_string(),
            status: "success".to_string(),
            started_at: now,
            finished_at: now,
        },
    )
    .await?;

    // 创建 node_task，assigned_node_id 指向测试节点（owner = test_user）
    // node_task.user_id 也是 test_user_id → 等于节点所有者 → 自消费，不产生小费
    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
        INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id)
        VALUES ($1, $2, 'deepseek-chat', '{}', 'succeeded', $3)
        "#,
            [
                request_id.into(),
                env.test_user_id.into(),
                env.node_id.into(),
            ],
        ))
        .await?;

    // 调用小费计算
    let tip = NodeTip::create_from_usage_log(&env.pool, usage_log.id).await?;

    chain.add_step(
        "keycompute-db",
        "tip::self_consumption",
        format!("Tip should be None for self-consumption: {:?}", tip),
        tip.is_none(),
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 8: 提现后 pending_amount 计算正确性（取代旧的并发锁测试）
// 需要正确初始化的数据库（运行 docker compose up -d postgres）
#[ignore = "需要数据库运行 (docker compose up -d postgres)"]
#[tokio::test]
async fn test_withdrawal_pending_amount_calculation() -> anyhow::Result<()> {
    let env = TipWithdrawalTestEnv::new("pending_calc").await?;
    let mut chain = VerificationChain::new();

    // 1. 创建多笔小费
    for i in 0..5 {
        let _ = env.create_usage_log_with_tip(10.0 + i as f64).await?;
    }

    // 2. 计算小费（批量）
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM usage_logs WHERE user_id = $1 ORDER BY created_at",
        [env.admin_user_id.into()],
    );
    let usage_logs = UsageLog::find_by_statement(stmt).all(&env.pool).await?;

    for log in usage_logs {
        let _ = NodeTip::create_from_usage_log(&env.pool, log.id).await?;
    }

    let summary_before = NodeTip::get_summary(&env.pool, env.test_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::multiple_tips_created",
        format!("Total pending amount: {}", summary_before.pending_amount),
        summary_before.pending_count >= 5,
    );

    // 3. 创建提现（balance 方式）
    let tx = env.pool.begin().await?;

    let withdrawal = NodeTipWithdrawal::create(
        &tx,
        env.test_user_id,
        WITHDRAWAL_TYPE_BALANCE,
        summary_before.pending_amount,
        None,
        None,
    )
    .await?;

    NodeTipWithdrawal::mark_completed(&tx, withdrawal.id, None, None, None).await?;

    tx.commit().await?;

    // 4. 验证 pending_amount 变为 0
    let summary_after = NodeTip::get_summary(&env.pool, env.test_user_id).await?;

    chain.add_step(
        "keycompute-db",
        "withdrawal::pending_amount_zero_after",
        format!(
            "Pending amount after withdrawal: {}",
            summary_after.pending_amount
        ),
        summary_after.pending_amount == rust_decimal::Decimal::ZERO,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}
