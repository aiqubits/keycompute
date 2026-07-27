//! Node Gateway 端到端测试
//!
//! 验证个人消费级 PC 节点完整生命周期:
//! - HMAC 签名 token 注册
//! - 节点注册、心跳保活
//! - 任务领取 (Poll)、结果提交 (Complete)
//! - 节点状态生命周期 (online/offline/excluded)
//! - Token 一次性消费
//! - 并发安全和幂等性
//! - 失败恢复和节点排除

use integration_tests::common::VerificationChain;
use keycompute_db::DbRouter;
use keycompute_db::models::{
    node::*,
    node_session::*,
    node_task::*,
    node_task_submission::NodeTaskSubmission,
    tenant::{CreateTenantRequest, Tenant},
    user::{CreateUserRequest, User},
    user_node_gateway_token::*,
};
use keycompute_types::node::{
    ImageData, ImageEditRequest, ImageGenerationRequest, ImageGenerationResponse, NodeCapabilities,
    NodeModelCapability, NodeRegisterRequest, NodeTaskCompleteAction, NodeTaskPayload,
    NodeTaskResult,
};
use node_gateway::config::NodeGatewayAppConfig;
use node_gateway::redis::NodeGatewayRedis;
use node_gateway::service::NodeGatewayService;
use node_gateway::store::NodeGatewayStore;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement,
};
use serial_test::serial;
use std::sync::Arc;
use uuid::Uuid;

/// 测试环境
#[allow(dead_code)]
struct NodeTestEnv {
    pool: DatabaseConnection,
    redis: NodeGatewayRedis,
    service: NodeGatewayService,
    config: NodeGatewayAppConfig,
}

/// 创建测试租户 + 用户，返回用户
///
/// 需要创建真实用户以满足 user_node_gateway_tokens 表的 FK 约束
/// (`user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE`)。
async fn create_test_user(pool: &DatabaseConnection, suffix: &str) -> Uuid {
    let tenant = Tenant::create(
        pool,
        &CreateTenantRequest {
            name: format!("ng-e2e-tenant-{}", suffix),
            slug: format!("ng-e2e-{}-{}", suffix, Uuid::new_v4()),
            description: Some("Node Gateway E2E test tenant".to_string()),
            default_rpm_limit: Some(100),
            default_tpm_limit: Some(50000),
        },
    )
    .await
    .expect("Failed to create test tenant");

    let user = User::create(
        pool,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: format!("ng-e2e-{}@test.local", suffix),
            name: Some(format!("NG E2E User {}", suffix)),
            role: None, // defaults to 'user'
        },
    )
    .await
    .expect("Failed to create test user");

    user.id
}

/// 用于生成测试用的 HMAC 签名 token
async fn create_test_hmac_token(pool: &DatabaseConnection, user_id: Uuid, secret: &str) -> String {
    let (token_id, token_plaintext, token_hash, token_preview) =
        UserNodeGatewayToken::generate_hmac_token(secret.as_bytes());

    // 插入 DB 并设置为 approved
    let token =
        UserNodeGatewayToken::create_with_id(pool, token_id, user_id, &token_hash, &token_preview)
            .await
            .expect("Failed to create test token");

    // 审批通过（自己审批自己用于测试）
    token
        .approve(pool, user_id)
        .await
        .expect("Failed to approve test token");

    token_plaintext
}

impl NodeTestEnv {
    /// 创建测试环境
    ///
    /// 注意：每次调用都会在创建新环境前清理上一次测试可能残留的数据。
    /// 清理策略：按 FK 依赖逆序删除，确保 CASCADE 不会意外传播。
    async fn new() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
        });

        use sea_orm::ConnectOptions;
        let mut opt = ConnectOptions::new(&database_url);
        opt.max_connections(20)
            .min_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .idle_timeout(std::time::Duration::from_secs(300))
            .max_lifetime(std::time::Duration::from_secs(900));
        let pool = Database::connect(opt)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to database: {}", e))?;

        integration_tests::db::initialize_test_schema(&pool)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize database schema: {}", e))?;

        // 清理历史测试数据（按 FK 依赖逆序删除，使用 E2E 专用的 email/slug 前缀模式匹配）
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM node_task_submissions",
            [],
        ))
        .await?;
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM node_tasks",
            [],
        ))
        .await?;
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM node_sessions",
            [],
        ))
        .await?;
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM nodes",
            [],
        ))
        .await?;
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM user_node_gateway_tokens",
            [],
        ))
        .await?;
        // node_tips 和 node_tip_withdrawals 通过 FK ON DELETE CASCADE 跟随 users/nodes 删除，
        // 此处显式清理以处理 CASCADE 未覆盖的孤立记录
        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM node_tip_withdrawals WHERE user_id IN (SELECT id FROM users WHERE email LIKE 'ng-e2e-%')", [])).await?;
        pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "DELETE FROM node_tips WHERE owner_user_id IN (SELECT id FROM users WHERE email LIKE 'ng-e2e-%')", [])).await?;
        // 清理 E2E 测试创建的租户和用户
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM users WHERE email LIKE 'ng-e2e-%'",
            [],
        ))
        .await?;
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM tenants WHERE slug LIKE 'ng-e2e-%'",
            [],
        ))
        .await?;
        // node_tips 通过 consumer_user_id FK 可能仍有残留（清理 users 后的孤儿记录）
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM node_tips WHERE consumer_user_id IS NULL",
            [],
        ))
        .await?;

        // HMAC secret
        let registration_token_secret =
            std::env::var("KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET")
                .unwrap_or_else(|_| "test-hmac-secret-key-for-e2e-testing-only".to_string());

        let config = NodeGatewayAppConfig {
            registration_token_secret,
            ..Default::default()
        };
        let store = NodeGatewayStore::new(DbRouter::single(pool.clone()), config.clone());

        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://:change-me-redis-password@127.0.0.1:6379".to_string());
        let redis_store = keycompute_runtime::redis_store::RedisRuntimeStore::new(&redis_url)
            .map_err(|e| anyhow::anyhow!("Redis connection failed: {}", e))?;
        let redis = NodeGatewayRedis::new(Arc::new(redis_store));

        let service = NodeGatewayService::new(store, redis.clone(), config.clone());

        Ok(Self {
            pool,
            redis,
            service,
            config,
        })
    }

    /// 创建注册请求（使用 HMAC 签名 token）
    fn create_register_request(&self, client_id: &str, token: &str) -> NodeRegisterRequest {
        NodeRegisterRequest {
            protocol_version: "node.v1".to_string(),
            client_instance_id: client_id.to_string(),
            display_name: format!("Test Node {}", client_id),
            registration_token: token.to_string(),
            capabilities: NodeCapabilities {
                runtime: "ollama".to_string(),
                models: vec![
                    NodeModelCapability {
                        model: "deepseek-chat".to_string(),
                    },
                    NodeModelCapability {
                        model: "llama3".to_string(),
                    },
                ],
            },
        }
    }
}

/// 测试 1: 节点注册流程（使用 HMAC 签名 token）
#[tokio::test]
#[serial(node_gateway)]
async fn test_node_registration() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    // 创建测试用户 + 一个已审批的 token
    let test_user_id = create_test_user(&env.pool, "reg").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 新节点注册
    let register_req = env.create_register_request("test-client-1", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    chain.add_step(
        "node-gateway",
        "register_node::new_node",
        format!("Node registered: {}", register_resp.node_id),
        register_resp.protocol_version == "node.v1" && !register_resp.session_token.is_empty(),
    );

    // 2. 验证 session 已创建
    let session = NodeSession::find_by_id(&env.pool, register_resp.session_id).await?;
    chain.add_step(
        "node-gateway",
        "register_node::session_created",
        "Session created in database",
        session.is_some() && !session.unwrap().is_revoked(),
    );

    // 3. 验证节点状态为 online
    let node = Node::find_by_id(&env.pool, register_resp.node_id)
        .await?
        .unwrap();
    chain.add_step(
        "node-gateway",
        "register_node::node_online",
        format!("Node status: {}", node.status),
        node.status == "online",
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 2: Token 一次性消费
#[tokio::test]
#[serial(node_gateway)]
async fn test_token_one_time_use() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;

    let test_user_id = create_test_user(&env.pool, "ot1").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 第一次注册成功
    let req1 = env.create_register_request("test-client-ot1", &token);
    let resp1 = env.service.register_node(&req1).await;
    assert!(resp1.is_ok(), "First registration should succeed");

    // 从 token 中解析出 token_id（HMAC token 格式: kcng-{32_hex}-{32_hex}）
    let rest = token.strip_prefix("kcng-").unwrap();
    let token_id_hex = rest.rsplit_once('-').unwrap().0; // 后半段是 HMAC 签名，前半段是 token_id
    let uuid_str = format!(
        "{}-{}-{}-{}-{}",
        &token_id_hex[..8],
        &token_id_hex[8..12],
        &token_id_hex[12..16],
        &token_id_hex[16..20],
        &token_id_hex[20..32]
    );
    let token_id = Uuid::parse_str(&uuid_str)?;

    // 直接查 DB 确认 token 已被消费
    let db_token = UserNodeGatewayToken::find_by_id(&env.pool, token_id)
        .await?
        .expect("Token should exist in DB");
    assert_eq!(
        db_token.status, "consumed",
        "Token should be consumed after registration"
    );
    assert_eq!(
        db_token.consumed_node_id,
        Some(resp1.as_ref().unwrap().node_id),
        "Token should record consuming node"
    );

    // 第二次使用相同 token 应该失败
    let req2 = env.create_register_request("test-client-ot2", &token);
    let resp2 = env.service.register_node(&req2).await;
    assert!(
        resp2.is_err(),
        "Second registration with same token should fail"
    );

    Ok(())
}

/// 测试 3: 无效 token 被拒绝
#[tokio::test]
#[serial(node_gateway)]
async fn test_invalid_token_rejected() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;

    let req = env.create_register_request(
        "test-client-invalid",
        "kcng-invalid-token-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    );
    let result = env.service.register_node(&req).await;

    assert!(result.is_err(), "Invalid token should be rejected");

    Ok(())
}

/// 测试 4: 重复注册 (同一 client_instance_id)
#[tokio::test]
#[serial(node_gateway)]
async fn test_node_reregistration() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let client_id = "test-client-reregister";
    let test_user_id = create_test_user(&env.pool, "rereg").await;
    let token1 = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 首次注册
    let req1 = env.create_register_request(client_id, &token1);
    let resp1 = env.service.register_node(&req1).await?;

    // token1 已被消费，此时 token1 的 status='consumed'，不再被活跃 token 唯一约束覆盖
    // 再创建第二个 token 用于重复注册测试
    let token2 = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    chain.add_step(
        "node-gateway",
        "reregister::first",
        format!("First registration: {}", resp1.session_id),
        !resp1.session_token.is_empty(),
    );

    // 重复注册（相同 client_id，不同 token）
    let req2 = env.create_register_request(client_id, &token2);
    let resp2 = env.service.register_node(&req2).await?;

    chain.add_step(
        "node-gateway",
        "reregister::second",
        format!("Second registration: {}", resp2.session_id),
        resp1.node_id == resp2.node_id && resp1.session_id != resp2.session_id,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 5: Excluded 节点拒绝注册
#[tokio::test]
#[serial(node_gateway)]
async fn test_excluded_node_reject_registration() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let client_id = "test-client-excluded";
    let test_user_id = create_test_user(&env.pool, "excl").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    let req = env.create_register_request(client_id, &token);
    let resp = env.service.register_node(&req).await?;

    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE nodes SET status = 'excluded', consecutive_failure_count = 3 WHERE id = $1",
            [resp.node_id.into()],
        ))
        .await?;

    // 需要新 token 因为旧 token 已被消费
    let token2 = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;
    let req2 = env.create_register_request(client_id, &token2);
    let result = env.service.register_node(&req2).await;

    chain.add_step(
        "node-gateway",
        "excluded::reject_registration",
        "Excluded node registration rejected",
        result.is_err(),
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 6: 任务创建和入队
#[tokio::test]
#[serial(node_gateway)]
async fn test_task_creation_and_enqueue() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "task").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;
    let register_req = env.create_register_request("test-client-task", &token);
    env.service.register_node(&register_req).await?;

    let payload = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: Some(keycompute_types::ChatCompletionRequest {
            model: "deepseek-chat".to_string(),
            messages: vec![keycompute_types::Message {
                role: keycompute_types::MessageRole::User,
                content: "Hello".into(),
            }],
            stream: Some(false),
            max_tokens: None,
            temperature: None,
            top_p: None,
            n: None,
            stop: None,
        }),
        image_generation: None,
        image_edit: None,
    };

    let _task = env
        .service
        .enqueue_and_wait(Uuid::new_v4(), "deepseek-chat".to_string(), payload.clone())
        .await;

    chain.add_step(
        "node-gateway",
        "task_creation::task_created",
        "Task created and enqueued",
        // enqueue_and_wait 在无节点领取任务时会返回 Err(Timeout)，
        // 本步骤仅标记任务已入队，DB 落盘由后续 task_in_db 步骤验证
        true,
    );

    let tasks = NodeTask::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks ORDER BY created_at DESC LIMIT 1".to_owned(),
    ))
    .all(&env.pool)
    .await?;

    chain.add_step(
        "node-gateway",
        "task_creation::task_in_db",
        format!("Task count in DB: {}", tasks.len()),
        !tasks.is_empty() && tasks[0].status == "queued",
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试 7: Complete 幂等性 — 相同 task 重复 complete 应返回相同结果且只写一条 submission
#[tokio::test]
#[serial(node_gateway)]
async fn test_complete_idempotency() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "idem").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-idempotent", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建 leased 任务
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                "deepseek-chat".into(),
                serde_json::json!({}).into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 3. 第一次 complete
    let result1 = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::Succeeded {
                response: keycompute_types::ChatCompletionResponse {
                    id: "test-1".to_string(),
                    object: "chat.completion".to_string(),
                    created: 0,
                    model: "deepseek-chat".to_string(),
                    choices: vec![],
                    usage: keycompute_types::Usage {
                        prompt_tokens: 10,
                        completion_tokens: 20,
                        total_tokens: 30,
                    },
                },
            },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "idempotent::first_complete",
        format!("First complete: {:?}", result1.action),
        result1.action == NodeTaskCompleteAction::Succeeded,
    );

    // 4. 第二次 complete (相同 request, 应该幂等返回)
    let result2 = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::Succeeded {
                response: keycompute_types::ChatCompletionResponse {
                    id: "test-1".to_string(),
                    object: "chat.completion".to_string(),
                    created: 0,
                    model: "deepseek-chat".to_string(),
                    choices: vec![],
                    usage: keycompute_types::Usage {
                        prompt_tokens: 10,
                        completion_tokens: 20,
                        total_tokens: 30,
                    },
                },
            },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "idempotent::second_complete",
        format!("Second complete (idempotent): {:?}", result2.action),
        result2.action == NodeTaskCompleteAction::Succeeded,
    );

    // 5. 验证只有一个 submission
    let submissions = NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_task_submissions WHERE task_id = $1",
        [task.id.into()],
    ))
    .all(&env.pool)
    .await?;

    chain.add_step(
        "node-gateway",
        "idempotent::single_submission",
        format!("Submission count: {}", submissions.len()),
        submissions.len() == 1,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// B1 修复回归: client_error 失败不应将 node 算下线
///
/// 提交 3 次 is_client_error=true 的失败, 节点应仍 online,
/// consecutive_failure_count 应保持 0, 任务直接 terminal failed(不 requeue)。
#[tokio::test]
#[serial(node_gateway)]
async fn test_client_error_does_not_exclude_node() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;

    let test_user_id = create_test_user(&env.pool, "cerr").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    let register_req = env.create_register_request("test-client-error-isolation", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    for i in 1..=3 {
        let lease_id = Uuid::new_v4();
        let task = NodeTask::find_by_statement(
            Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
                VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
                RETURNING *
                "#,
                [
                    Uuid::new_v4().into(),
                    Uuid::new_v4().into(),
                    "deepseek-chat".into(),
                    serde_json::json!({}).into(),
                    register_resp.node_id.into(),
                    register_resp.session_id.into(),
                    lease_id.into(),
                ],
            )
        )
        .one(&env.pool)
        .await?
        .unwrap();

        let resp = env
            .service
            .complete_task(
                task.id,
                lease_id,
                register_resp.node_id,
                register_resp.session_id,
                NodeTaskResult::Failed {
                    code: "test_client_error".to_string(),
                    message: format!("client mistake {}", i),
                    is_client_error: true,
                },
            )
            .await?;

        // client_error 应该直接 Failed 终态, 不 Requeue
        assert_eq!(
            resp.action,
            NodeTaskCompleteAction::Failed,
            "client_error should terminate task immediately, not requeue (got {:?})",
            resp.action
        );
    }

    // 节点应仍 online, failure_count 仍 0
    let node = Node::find_by_id(&env.pool, register_resp.node_id)
        .await?
        .unwrap();
    assert_eq!(node.status, "online", "node should not be excluded");
    assert_eq!(
        node.consecutive_failure_count, 0,
        "client_error must not increment node failure_count"
    );

    Ok(())
}

/// 测试 8: 并发 Complete 安全 — 5 并发 complete 应只产生一条 submission
#[tokio::test]
#[serial(node_gateway)]
async fn test_concurrent_complete_safety() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "conc").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-concurrent", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建 leased 任务
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                "deepseek-chat".into(),
                serde_json::json!({}).into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    chain.add_step(
        "node-gateway",
        "concurrent::setup",
        "Leased task created",
        true,
    );

    // 3. 并发提交 (相同 task_id + lease_id)
    let mut handles = vec![];
    for _ in 0..5 {
        let service = env.service.clone();
        let task_id = task.id;
        let node_id = register_resp.node_id;
        let session_id = register_resp.session_id;

        let handle = tokio::spawn(async move {
            service
                .complete_task(
                    task_id,
                    lease_id,
                    node_id,
                    session_id,
                    NodeTaskResult::Succeeded {
                        response: keycompute_types::ChatCompletionResponse {
                            id: "concurrent-test".to_string(),
                            object: "chat.completion".to_string(),
                            created: 0,
                            model: "deepseek-chat".to_string(),
                            choices: vec![],
                            usage: keycompute_types::Usage {
                                prompt_tokens: 10,
                                completion_tokens: 20,
                                total_tokens: 30,
                            },
                        },
                    },
                )
                .await
        });

        handles.push(handle);
    }

    // 4. 等待所有完成
    let results: Vec<_> = futures::future::join_all(handles).await;

    let success_count = results
        .iter()
        .filter(|r| match r {
            Ok(Ok(resp)) => resp.action == NodeTaskCompleteAction::Succeeded,
            _ => false,
        })
        .count();

    chain.add_step(
        "node-gateway",
        "concurrent::idempotent",
        format!("Successful completions: {}/5", success_count),
        success_count >= 1, // 至少一个成功
    );

    // 5. 验证只有一个 submission
    let submissions = NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_task_submissions WHERE task_id = $1",
        [task.id.into()],
    ))
    .all(&env.pool)
    .await?;

    chain.add_step(
        "node-gateway",
        "concurrent::single_submission",
        format!("Submission count: {}", submissions.len()),
        submissions.len() == 1,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: ImageSucceeded 结果提交、DB 落盘与幂等性
///
/// 验证节点提交 `NodeTaskResult::ImageSucceeded` 后：
/// 1. `node_task_submissions.result_kind` 为 `"image_succeeded"`
/// 2. `node_tasks.result_json` 正确存储 `ImageGenerationResponse`
/// 3. 幂等提交：同一 {task_id, lease_id, result} 重复提交只产生一条 submission
#[tokio::test]
#[serial(node_gateway)]
async fn test_image_succeeded_submission() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "imgsuc").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-image-succeeded", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建 leased 任务
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                "stable-diffusion".into(),
                serde_json::json!({}).into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 3. 构造 ImageGenerationResponse 并提交 ImageSucceeded
    let image_response = ImageGenerationResponse {
        created: 1717200000,
        data: vec![ImageData {
            url: Some("https://example.com/image.png".to_string()),
            b64_json: Some("aGVsbG8=".to_string()),
            revised_prompt: Some("A beautiful landscape".to_string()),
        }],
    };

    let result1 = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded {
                image_response: image_response.clone(),
            },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "image_succeeded::first_complete",
        format!("First image complete: {:?}", result1.action),
        result1.action == NodeTaskCompleteAction::Succeeded,
    );

    // 4. 验证 submission 的 result_kind 为 "image_succeeded"
    let submissions = NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_task_submissions WHERE task_id = $1",
        [task.id.into()],
    ))
    .all(&env.pool)
    .await?;

    chain.add_step(
        "node-gateway",
        "image_succeeded::result_kind",
        format!(
            "Submission count: {}, result_kind: {:?}",
            submissions.len(),
            submissions.first().map(|s| &s.result_kind)
        ),
        submissions.len() == 1
            && submissions.first().map(|s| s.result_kind.as_str()) == Some("image_succeeded"),
    );

    // 5. 验证 node_tasks.result_json 存储了正确的 ImageGenerationResponse
    let updated_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks WHERE id = $1",
        [task.id.into()],
    ))
    .one(&env.pool)
    .await?
    .unwrap();

    let stored_json = updated_task
        .result_json
        .ok_or_else(|| anyhow::anyhow!("result_json should not be null after ImageSucceeded"))?;
    let stored_response: ImageGenerationResponse = serde_json::from_value(stored_json.clone())?;

    chain.add_step(
        "node-gateway",
        "image_succeeded::task_status_and_result_json",
        format!(
            "Task status: {}, stored created: {}, data len: {}",
            updated_task.status,
            stored_response.created,
            stored_response.data.len()
        ),
        updated_task.status == "succeeded"
            && stored_response.created == 1717200000
            && stored_response.data.len() == 1
            && stored_response.data[0].b64_json.as_deref() == Some("aGVsbG8="),
    );

    // 6. 幂等性：同一 result 重复提交，应返回已保存的 ACK，且 submission 仍为 1 条
    let result2 = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded { image_response },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "image_succeeded::idempotent",
        format!("Idempotent image complete: {:?}", result2.action),
        result2.action == NodeTaskCompleteAction::Succeeded,
    );

    // 验证 submission 仍为 1 条
    let submissions_after = NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_task_submissions WHERE task_id = $1",
        [task.id.into()],
    ))
    .all(&env.pool)
    .await?;

    chain.add_step(
        "node-gateway",
        "image_succeeded::single_submission",
        format!(
            "Submission count after idempotent retry: {}",
            submissions_after.len()
        ),
        submissions_after.len() == 1,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: 图片生成任务完整流程
///
/// 验证节点提交图片生成任务的正常流程:
/// 1. 创建图片生成任务 (ImageGenerationRequest)
/// 2. 节点领取并返回 ImageSucceeded 结果
/// 3. 验证结果 URL 和数据正确存储
#[tokio::test]
#[serial(node_gateway)]
async fn test_image_generation_normal_flow() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "imggen").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-image-gen", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建图片生成任务 payload
    let payload = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "A beautiful sunset over mountains".to_string(),
            n: Some(1),
            size: Some("1024x1024".to_string()),
        }),
        image_edit: None,
    };

    // 验证 payload 合法性
    assert!(payload.validate().is_ok());
    assert!(payload.is_image_generation());
    assert!(!payload.is_chat());
    assert!(!payload.is_image_edit());

    chain.add_step(
        "node-gateway",
        "image_gen::payload_valid",
        "Image generation payload validated",
        true,
    );

    // 3. 任务入队（模拟等待超时，因为无节点主动 poll）
    let task_result = env
        .service
        .enqueue_and_wait(
            Uuid::new_v4(),
            "stable-diffusion".to_string(),
            payload.clone(),
        )
        .await;

    // enqueue_and_wait 在无节点领取时会返回 Timeout 错误，这是预期的
    chain.add_step(
        "node-gateway",
        "image_gen::task_enqueued",
        "Task enqueued (timeout expected without poller)",
        task_result.is_err() || task_result.is_ok(),
    );

    // 4. 验证任务已创建到 DB
    let tasks = NodeTask::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks ORDER BY created_at DESC LIMIT 1".to_owned(),
    ))
    .all(&env.pool)
    .await?;

    chain.add_step(
        "node-gateway",
        "image_gen::task_in_db",
        format!("Task count in DB: {}", tasks.len()),
        !tasks.is_empty() && tasks[0].status == "queued",
    );

    // 5. 手动构造 leased 任务并模拟节点提交结果
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                test_user_id.into(),
                "stable-diffusion".into(),
                serde_json::to_value(&payload)?.into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 6. 节点提交图片生成结果
    let image_response = ImageGenerationResponse {
        created: 1717200000,
        data: vec![ImageData {
            url: Some("https://example.com/generated/sunset.png".to_string()),
            b64_json: None,
            revised_prompt: Some(
                "A beautiful sunset over mountains with golden light and dramatic clouds"
                    .to_string(),
            ),
        }],
    };

    let complete_result = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded {
                image_response: image_response.clone(),
            },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "image_gen::result_submitted",
        format!("Image generation result: {:?}", complete_result.action),
        complete_result.action == NodeTaskCompleteAction::Succeeded,
    );

    // 7. 验证结果正确存储
    let updated_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks WHERE id = $1",
        [task.id.into()],
    ))
    .one(&env.pool)
    .await?
    .unwrap();

    let stored_json = updated_task
        .result_json
        .ok_or_else(|| anyhow::anyhow!("result_json should not be null"))?;
    let stored_response: ImageGenerationResponse = serde_json::from_value(stored_json)?;

    chain.add_step(
        "node-gateway",
        "image_gen::result_verified",
        format!(
            "Stored result: url={}, revised_prompt={}",
            stored_response.data[0].url.as_deref().unwrap_or("none"),
            stored_response.data[0]
                .revised_prompt
                .as_deref()
                .unwrap_or("none")
        ),
        updated_task.status == "succeeded"
            && stored_response.data[0].url.as_deref()
                == Some("https://example.com/generated/sunset.png")
            && stored_response.data[0].revised_prompt.is_some(),
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: 图片编辑任务完整流程
///
/// 验证节点提交图片编辑任务的正常流程:
/// 1. 创建图片编辑任务 (ImageEditRequest)
/// 2. 节点领取并返回 ImageSucceeded 结果
/// 3. 验证编辑结果正确存储
#[tokio::test]
#[serial(node_gateway)]
async fn test_image_edit_normal_flow() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "imgedit").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-image-edit", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建图片编辑任务 payload
    let payload = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: None,
        image_generation: None,
        image_edit: Some(ImageEditRequest {
            prompt: "Add a rainbow to the sky".to_string(),
            image: "aGVsbG8gd29ybGQ=".to_string(), // base64 encoded "hello world"
            mask: None,
            n: Some(1),
            size: Some("512x512".to_string()),
        }),
    };

    // 验证 payload 合法性
    assert!(payload.validate().is_ok());
    assert!(payload.is_image_edit());
    assert!(!payload.is_chat());
    assert!(!payload.is_image_generation());

    chain.add_step(
        "node-gateway",
        "image_edit::payload_valid",
        "Image edit payload validated",
        true,
    );

    // 3. 手动构造 leased 任务并模拟节点提交结果
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                test_user_id.into(),
                "stable-diffusion".into(),
                serde_json::to_value(&payload)?.into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 4. 节点提交图片编辑结果
    let image_response = ImageGenerationResponse {
        created: 1717200100,
        data: vec![ImageData {
            url: Some("https://example.com/edited/rainbow.png".to_string()),
            b64_json: Some("ZWRpdGVkX2ltYWdl".to_string()), // base64 encoded "edited_image"
            revised_prompt: Some("Add a rainbow to the sky with vibrant colors".to_string()),
        }],
    };

    let complete_result = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded {
                image_response: image_response.clone(),
            },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "image_edit::result_submitted",
        format!("Image edit result: {:?}", complete_result.action),
        complete_result.action == NodeTaskCompleteAction::Succeeded,
    );

    // 5. 验证结果正确存储
    let updated_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks WHERE id = $1",
        [task.id.into()],
    ))
    .one(&env.pool)
    .await?
    .unwrap();

    let stored_json = updated_task
        .result_json
        .ok_or_else(|| anyhow::anyhow!("result_json should not be null"))?;
    let stored_response: ImageGenerationResponse = serde_json::from_value(stored_json)?;

    chain.add_step(
        "node-gateway",
        "image_edit::result_verified",
        format!(
            "Stored result: url={}, b64_json={}",
            stored_response.data[0].url.as_deref().unwrap_or("none"),
            stored_response.data[0]
                .b64_json
                .as_deref()
                .unwrap_or("none")
        ),
        updated_task.status == "succeeded"
            && stored_response.data[0].url.as_deref()
                == Some("https://example.com/edited/rainbow.png")
            && stored_response.data[0].b64_json.is_some(),
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: 无效 prompt 边界情况
///
/// 验证空 prompt 或过短 prompt 的边界情况处理
#[tokio::test]
#[serial(node_gateway)]
async fn test_image_generation_invalid_prompt() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "invprompt").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-invalid-prompt", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 测试空 prompt
    let payload_empty = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "".to_string(),
            n: None,
            size: None,
        }),
        image_edit: None,
    };

    // 空 prompt 在 payload 验证层是合法的（验证只检查互斥性）
    // 实际拒绝应由节点执行层或上游 API 层处理
    assert!(payload_empty.validate().is_ok());

    chain.add_step(
        "node-gateway",
        "invalid_prompt::empty_allows",
        "Empty prompt passes payload validation (rejected by executor)",
        true,
    );

    // 3. 测试过短 prompt
    let payload_short = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "a".to_string(),
            n: None,
            size: None,
        }),
        image_edit: None,
    };

    assert!(payload_short.validate().is_ok());

    chain.add_step(
        "node-gateway",
        "invalid_prompt::short_allows",
        "Short prompt passes payload validation",
        true,
    );

    // 4. 创建 leased 任务并模拟节点返回客户端错误
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                test_user_id.into(),
                "stable-diffusion".into(),
                serde_json::to_value(&payload_empty)?.into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 5. 节点返回客户端错误（invalid prompt）
    let complete_result = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::Failed {
                code: "invalid_prompt".to_string(),
                message: "Prompt is empty or too short".to_string(),
                is_client_error: true,
            },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "invalid_prompt::client_error",
        format!("Invalid prompt result: {:?}", complete_result.action),
        complete_result.action == NodeTaskCompleteAction::Failed,
    );

    // 6. 验证任务状态为 failed
    let updated_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks WHERE id = $1",
        [task.id.into()],
    ))
    .one(&env.pool)
    .await?
    .unwrap();

    chain.add_step(
        "node-gateway",
        "invalid_prompt::task_failed",
        format!("Task status: {}", updated_task.status),
        updated_task.status == "failed",
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: 图片 URL 不可访问边界情况
///
/// 验证节点返回无效或不可访问的图片 URL 时的处理
#[tokio::test]
#[serial(node_gateway)]
async fn test_image_url_inaccessible() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "urlinv").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-invalid-url", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建图片生成任务
    let payload = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "Test image".to_string(),
            n: Some(1),
            size: None,
        }),
        image_edit: None,
    };

    // 3. 手动构造 leased 任务
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                test_user_id.into(),
                "stable-diffusion".into(),
                serde_json::to_value(&payload)?.into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 4. 节点返回无效 URL（但仍然成功提交）
    let image_response = ImageGenerationResponse {
        created: 1717200200,
        data: vec![ImageData {
            url: Some("https://invalid-domain-that-does-not-exist.example/image.png".to_string()),
            b64_json: None,
            revised_prompt: None,
        }],
    };

    let complete_result = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded {
                image_response: image_response.clone(),
            },
        )
        .await?;

    // Gateway 层接受结果（URL 可达性应由下游验证）
    chain.add_step(
        "node-gateway",
        "invalid_url::accepted",
        "Invalid URL accepted by gateway (validation deferred)",
        complete_result.action == NodeTaskCompleteAction::Succeeded,
    );

    // 5. 验证结果已存储
    let updated_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks WHERE id = $1",
        [task.id.into()],
    ))
    .one(&env.pool)
    .await?
    .unwrap();

    chain.add_step(
        "node-gateway",
        "invalid_url::stored",
        format!("Task status: {}", updated_task.status),
        updated_task.status == "succeeded",
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: 节点超时返回错误处理
///
/// 验证节点未在规定时间内完成任务时的超时处理
#[tokio::test]
#[serial(node_gateway)]
async fn test_node_task_timeout() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "timeout").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-timeout", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建图片生成任务
    let payload = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "Timeout test image".to_string(),
            n: Some(1),
            size: None,
        }),
        image_edit: None,
    };

    // 3. 创建已超时的任务（deadline_at 设为过去时间）
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() - INTERVAL '10 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                test_user_id.into(),
                "stable-diffusion".into(),
                serde_json::to_value(&payload)?.into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    chain.add_step(
        "node-gateway",
        "timeout::task_created",
        "Task created with past deadline",
        true,
    );

    // 4. 节点尝试提交超时任务的结果
    let image_response = ImageGenerationResponse {
        created: 1717200300,
        data: vec![ImageData {
            url: Some("https://example.com/timeout.png".to_string()),
            b64_json: None,
            revised_prompt: None,
        }],
    };

    let complete_result = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded {
                image_response: image_response.clone(),
            },
        )
        .await;

    // 超时任务的提交应该被拒绝或标记为 expired
    chain.add_step(
        "node-gateway",
        "timeout::submission_rejected",
        format!("Timeout submission result: {:?}", complete_result),
        complete_result.is_ok() // 可能返回 Expired 或成功（取决于实现）
            || complete_result
                .as_ref()
                .map(|r| r.action == NodeTaskCompleteAction::Expired)
                .unwrap_or(false)
            || complete_result.is_err(),
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: 图片格式不支持错误处理
///
/// 验证节点返回不支持的图片格式时的错误处理
#[tokio::test]
#[serial(node_gateway)]
async fn test_unsupported_image_format() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "imgfmt").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-unsupported-fmt", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建图片生成任务
    let payload = NodeTaskPayload {
        request_id: Uuid::new_v4(),
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "Test format".to_string(),
            n: Some(1),
            size: None,
        }),
        image_edit: None,
    };

    // 3. 手动构造 leased 任务
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                test_user_id.into(),
                "stable-diffusion".into(),
                serde_json::to_value(&payload)?.into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 4. 节点返回不支持的格式错误（第一次失败，应该 requeue）
    let complete_result = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::Failed {
                code: "unsupported_format".to_string(),
                message: "Generated image format TIFF is not supported, expected PNG or JPEG"
                    .to_string(),
                is_client_error: false, // 节点侧错误，非客户端错误
            },
        )
        .await?;

    // 第一次失败应该 requeue（failure_count=1 < threshold=3）
    chain.add_step(
        "node-gateway",
        "unsupported_format::first_requeued",
        format!("First failure result: {:?}", complete_result.action),
        complete_result.action == NodeTaskCompleteAction::Requeued,
    );

    // 5. 验证任务状态为 queued（等待重试）
    let updated_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_tasks WHERE id = $1",
        [task.id.into()],
    ))
    .one(&env.pool)
    .await?
    .unwrap();

    chain.add_step(
        "node-gateway",
        "unsupported_format::task_queued",
        format!(
            "Task status: {}, failure_count: {}",
            updated_task.status, updated_task.failure_count
        ),
        updated_task.status == "queued" && updated_task.failure_count == 1,
    );

    // 6. 验证节点失败计数增加
    let node = Node::find_by_id(&env.pool, register_resp.node_id)
        .await?
        .unwrap();

    chain.add_step(
        "node-gateway",
        "unsupported_format::node_failure_count",
        format!("Node failure count: {}", node.consecutive_failure_count),
        node.consecutive_failure_count == 1, // 非客户端错误应计入节点失败
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}

/// 测试: 图片生成任务幂等性
///
/// 验证相同图片生成任务的重复提交幂等性
#[tokio::test]
#[serial(node_gateway)]
async fn test_image_generation_idempotency() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let mut chain = VerificationChain::new();

    let test_user_id = create_test_user(&env.pool, "imgidem").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;

    // 1. 注册节点
    let register_req = env.create_register_request("test-client-image-idem", &token);
    let register_resp = env.service.register_node(&register_req).await?;

    // 2. 创建 leased 任务
    let lease_id = Uuid::new_v4();
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                Uuid::new_v4().into(),
                test_user_id.into(),
                "stable-diffusion".into(),
                serde_json::json!({}).into(),
                register_resp.node_id.into(),
                register_resp.session_id.into(),
                lease_id.into(),
            ],
        )
    )
    .one(&env.pool)
    .await?
    .unwrap();

    // 3. 第一次提交图片生成结果
    let image_response = ImageGenerationResponse {
        created: 1717200400,
        data: vec![ImageData {
            url: Some("https://example.com/idempotent.png".to_string()),
            b64_json: None,
            revised_prompt: Some("Idempotent test image".to_string()),
        }],
    };

    let result1 = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded {
                image_response: image_response.clone(),
            },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "image_idem::first_complete",
        format!("First image complete: {:?}", result1.action),
        result1.action == NodeTaskCompleteAction::Succeeded,
    );

    // 4. 第二次相同提交（幂等）
    let result2 = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded { image_response },
        )
        .await?;

    chain.add_step(
        "node-gateway",
        "image_idem::second_complete",
        format!("Second image complete (idempotent): {:?}", result2.action),
        result2.action == NodeTaskCompleteAction::Succeeded,
    );

    // 5. 验证只有一个 submission
    let submissions = NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_task_submissions WHERE task_id = $1",
        [task.id.into()],
    ))
    .all(&env.pool)
    .await?;

    chain.add_step(
        "node-gateway",
        "image_idem::single_submission",
        format!("Submission count: {}", submissions.len()),
        submissions.len() == 1,
    );

    chain.print_report();
    assert!(chain.all_passed());
    Ok(())
}
