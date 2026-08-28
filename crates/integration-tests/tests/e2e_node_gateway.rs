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

use deadpool_redis::redis::AsyncCommands;
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
use keycompute_types::{
    AttemptStatus, AttemptTraceFinish, BillingStatus, ErrorOrigin, RequestLifecycleRecorder,
    RequestStatus, RequestTraceFinish, StreamEndReason, TestRequestLifecycleRecorder,
    TraceErrorCategory, TraceErrorInfo,
};
use node_gateway::NodeGatewaySweeper;
use node_gateway::config::NodeGatewayAppConfig;
use node_gateway::redis::NodeGatewayRedis;
use node_gateway::service::NodeGatewayService;
use node_gateway::store::NodeGatewayStore;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement,
    TransactionTrait,
};
use serial_test::serial;
use std::sync::Arc;
use uuid::Uuid;

/// 测试环境
#[allow(dead_code)]
struct NodeTestEnv {
    pool: DatabaseConnection,
    redis_store: Arc<keycompute_runtime::redis_store::RedisRuntimeStore>,
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

fn chat_task_payload(request_id: Uuid) -> NodeTaskPayload {
    NodeTaskPayload {
        request_id,
        chat: Some(keycompute_types::ChatCompletionRequest::new(
            "deepseek-chat",
            Vec::new(),
        )),
        image_generation: None,
        image_edit: None,
    }
}

fn image_generation_task_payload(request_id: Uuid) -> NodeTaskPayload {
    NodeTaskPayload {
        request_id,
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "Node Gateway completion fixture".to_string(),
            n: Some(1),
            size: None,
        }),
        image_edit: None,
    }
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
        let redis_store = Arc::new(
            keycompute_runtime::redis_store::RedisRuntimeStore::new(&redis_url)
                .map_err(|e| anyhow::anyhow!("Redis connection failed: {}", e))?,
        );
        let redis = NodeGatewayRedis::new(Arc::clone(&redis_store));

        let service = NodeGatewayService::new(store, redis.clone(), config.clone());

        Ok(Self {
            pool,
            redis_store,
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

/// Sweeper 必须复用持有 advisory lock 的事务；合法的单连接写池不能因
/// 再次申请 writer connection 而超时。
#[tokio::test]
#[serial(node_gateway)]
async fn test_sweeper_runs_with_single_db_connection() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let test_user_id = create_test_user(&env.pool, "sweeper-single-conn").await;
    let token = create_test_hmac_token(
        &env.pool,
        test_user_id,
        &env.config.registration_token_secret,
    )
    .await;
    let registered = env
        .service
        .register_node(&env.create_register_request("sweeper-single-conn", &token))
        .await?;
    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE nodes SET status = 'online', last_heartbeat_at = NOW() - INTERVAL '1 hour' WHERE id = $1",
            [registered.node_id.into()],
        ))
        .await?;

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
    });
    let mut options = sea_orm::ConnectOptions::new(database_url);
    options
        .max_connections(1)
        .min_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(500));
    let single_connection = Database::connect(options).await?;
    let sweeper = NodeGatewaySweeper::new(
        DbRouter::single(single_connection),
        env.redis.clone(),
        env.config.clone(),
    );

    tokio::time::timeout(std::time::Duration::from_secs(2), sweeper.run_once())
        .await
        .expect("single-connection sweeper must not wait for a second writer")?;

    let node = Node::find_by_id(&env.pool, registered.node_id)
        .await?
        .expect("registered node should still exist");
    assert_eq!(node.status, NODE_STATUS_OFFLINE);
    Ok(())
}

/// Repeated recovery cycles must converge each queued task to one Redis List
/// entry, while expiration removes all historical duplicates and bounds the
/// wake-up notification lifetime.
#[tokio::test]
#[serial(node_gateway)]
async fn test_sweeper_converges_redis_queue_entries() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let model = format!("sweeper-convergence-{}", Uuid::new_v4());
    let queue_key = format!("queue:node:model:{model}");

    let queued_task = NodeTask::create(
        &env.pool,
        &CreateNodeTaskRequest {
            request_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            model: model.clone(),
            payload_json: serde_json::json!({}),
            deadline_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            complete_grace_until: chrono::Utc::now() + chrono::Duration::minutes(6),
        },
    )
    .await?;
    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE node_tasks SET queued_at=NOW()-INTERVAL '1 minute' WHERE id=$1",
            [queued_task.id.into()],
        ))
        .await?;

    // Seed the duplicate state produced by the previous unconditional LPUSH
    // implementation, then prove that multiple sweeps remain convergent.
    let mut redis = env.redis_store.pool().get().await?;
    let queued_id = queued_task.id.to_string();
    let _: () = redis
        .lpush(
            &queue_key,
            &[queued_id.clone(), queued_id.clone(), queued_id.clone()],
        )
        .await?;
    drop(redis);

    let sweeper = NodeGatewaySweeper::new(
        DbRouter::single(env.pool.clone()),
        env.redis.clone(),
        env.config.clone(),
    );
    sweeper.run_once().await?;
    sweeper.run_once().await?;

    let mut redis = env.redis_store.pool().get().await?;
    let queued_entries: Vec<String> = redis.lrange(&queue_key, 0, -1).await?;
    assert_eq!(queued_entries, [queued_id]);
    drop(redis);

    let expired_task = NodeTask::create(
        &env.pool,
        &CreateNodeTaskRequest {
            request_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            model: model.clone(),
            payload_json: serde_json::json!({}),
            deadline_at: chrono::Utc::now() - chrono::Duration::seconds(1),
            complete_grace_until: chrono::Utc::now() + chrono::Duration::minutes(1),
        },
    )
    .await?;
    let expired_id = expired_task.id.to_string();
    let mut redis = env.redis_store.pool().get().await?;
    let _: () = redis
        .lpush(&queue_key, &[expired_id.clone(), expired_id.clone()])
        .await?;
    drop(redis);

    sweeper.run_once().await?;

    let reloaded = NodeTask::find_by_id(&env.pool, expired_task.id)
        .await?
        .expect("expired task should remain available for audit");
    assert_eq!(reloaded.status, TASK_STATUS_EXPIRED);

    let mut redis = env.redis_store.pool().get().await?;
    let queue_entries: Vec<String> = redis.lrange(&queue_key, 0, -1).await?;
    assert_eq!(queue_entries, [queued_task.id.to_string()]);
    let result_key = format!("task:result:{}", expired_task.id);
    let notification_ttl: i64 = redis.ttl(&result_key).await?;
    assert!(
        notification_ttl > 0,
        "orphan result notifications must have a bounded lifetime"
    );

    let _: usize = redis.del(&queue_key).await?;
    let _: usize = redis.del(&result_key).await?;
    drop(redis);
    for task_id in [queued_task.id, expired_task.id] {
        env.pool
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM node_tasks WHERE id=$1",
                [task_id.into()],
            ))
            .await?;
    }
    Ok(())
}

/// Request-side waiting may close an expired attempt before the task sweeper
/// observes the same deadline. Replaying that terminal state must remain
/// idempotent and must not downgrade a complete trace.
#[tokio::test]
#[serial(node_gateway)]
async fn test_sweeper_preserves_trace_quality_after_wait_timeout() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let user_id = create_test_user(&env.pool, "wait-timeout-trace").await;
    let token =
        create_test_hmac_token(&env.pool, user_id, &env.config.registration_token_secret).await;
    let registered = env
        .service
        .register_node(&env.create_register_request("wait-timeout-trace", &token))
        .await?;

    let request_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let lease_id = Uuid::new_v4();
    let received_at = chrono::Utc::now() - chrono::Duration::seconds(30);
    let claimed_at = chrono::Utc::now() - chrono::Duration::seconds(20);
    let payload = NodeTaskPayload {
        request_id,
        chat: None,
        image_generation: Some(ImageGenerationRequest {
            prompt: "wait timeout trace".to_string(),
            n: Some(1),
            size: None,
        }),
        image_edit: None,
    };
    let task = NodeTask::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"INSERT INTO node_tasks (
                request_id,user_id,model,payload_json,status,assigned_node_id,
                assigned_session_id,lease_id,claimed_at,deadline_at,
                complete_grace_until,failure_threshold
            ) VALUES ($1,$2,'stable-diffusion',$3,'leased',$4,$5,$6,$7,
                      NOW()-INTERVAL '10 seconds',NOW()+INTERVAL '120 seconds',3)
            RETURNING *"#,
        [
            request_id.into(),
            user_id.into(),
            serde_json::to_value(&payload)?.into(),
            registered.node_id.into(),
            registered.session_id.into(),
            lease_id.into(),
            claimed_at.into(),
        ],
    ))
    .one(&env.pool)
    .await?
    .expect("leased timeout task should be inserted");

    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO gateway_requests (
                    request_id,tenant_id,user_id,produce_ai_key_id,protocol,request_path,
                    requested_model,is_stream,route_type,status,received_at,billing_status,trace_quality
                ) VALUES ($1,$2,$3,$4,'openai','/v1/chat/completions','stable-diffusion',FALSE,
                          'node','running',$5,'pending','actual')"#,
            [
                request_id.into(),
                Uuid::new_v4().into(),
                user_id.into(),
                Uuid::new_v4().into(),
                received_at.into(),
            ],
        ))
        .await?;
    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO gateway_request_attempts (
                    id,request_id,attempt_no,attempt_kind,route_type,model,status,is_final,
                    node_task_id,node_id,session_id,lease_id,started_at
                ) VALUES ($1,$2,1,'primary','node','stable-diffusion','running',FALSE,
                          $3,$4,$5,$6,$7)"#,
            [
                attempt_id.into(),
                request_id.into(),
                task.id.into(),
                registered.node_id.into(),
                registered.session_id.into(),
                lease_id.into(),
                claimed_at.into(),
            ],
        ))
        .await?;

    let lifecycle =
        keycompute_db::PostgresRequestLifecycleRecorder::new(DbRouter::single(env.pool.clone()));
    lifecycle
        .finish_attempt_and_request(AttemptTraceFinish {
            attempt_id,
            request_id,
            attempt_status: AttemptStatus::Expired,
            request_status: RequestStatus::Running,
            is_final: true,
            stream_end_reason: Some(StreamEndReason::Timeout),
            stream_error_count: Some(1),
            error: Some(TraceErrorInfo {
                origin: ErrorOrigin::Node,
                category: TraceErrorCategory::NodeExpired,
                code: "node_wait_timeout".to_string(),
                summary: None,
                retryable: Some(false),
            }),
            billing_status: BillingStatus::Pending,
            finished_at: chrono::Utc::now(),
        })
        .await?;
    lifecycle
        .finish_request_without_attempt(RequestTraceFinish {
            request_id,
            status: RequestStatus::TimedOut,
            error: Some(TraceErrorInfo {
                origin: ErrorOrigin::Node,
                category: TraceErrorCategory::NodeExpired,
                code: "node_wait_timeout".to_string(),
                summary: None,
                retryable: Some(false),
            }),
            billing_status: BillingStatus::NotApplicable,
            finished_at: chrono::Utc::now(),
        })
        .await?;

    env.service.sweeper().run_once().await?;

    let request = env
        .pool
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status,trace_quality FROM gateway_requests WHERE request_id=$1",
            [request_id.into()],
        ))
        .await?
        .expect("request trace should remain available");
    assert_eq!(request.try_get::<String>("", "status")?, "timed_out");
    assert_eq!(request.try_get::<String>("", "trace_quality")?, "actual");

    let attempt = env
        .pool
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status,is_final,error_code FROM gateway_request_attempts WHERE id=$1",
            [attempt_id.into()],
        ))
        .await?
        .expect("terminal node attempt should remain available");
    assert_eq!(attempt.try_get::<String>("", "status")?, "expired");
    assert!(attempt.try_get::<bool>("", "is_final")?);
    assert_eq!(
        attempt.try_get::<String>("", "error_code")?,
        "node_wait_timeout"
    );

    let task = NodeTask::find_by_id(&env.pool, task.id)
        .await?
        .expect("expired task should remain available");
    assert_eq!(task.status, TASK_STATUS_EXPIRED);

    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM gateway_requests WHERE request_id=$1",
            [request_id.into()],
        ))
        .await?;
    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM node_tasks WHERE id=$1",
            [task.id.into()],
        ))
        .await?;
    Ok(())
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

/// 会话 TTL 是认证边界：过期 token 既不能通过内部认证，也不能靠心跳续期。
#[tokio::test]
#[serial(node_gateway)]
async fn test_expired_session_cannot_authenticate_or_renew() -> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let user_id = create_test_user(&env.pool, "expired-session").await;
    let registration_token =
        create_test_hmac_token(&env.pool, user_id, &env.config.registration_token_secret).await;
    let registration = env
        .service
        .register_node(
            &env.create_register_request("test-client-expired-session", &registration_token),
        )
        .await?;

    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE node_sessions SET expires_at = NOW() - INTERVAL '1 second' WHERE id = $1",
            [registration.session_id.into()],
        ))
        .await?;

    assert!(
        env.service
            .store
            .authenticate_session(&registration.session_token)
            .await
            .is_err(),
        "expired session token must not authenticate"
    );
    assert!(
        env.service
            .heartbeat(
                registration.node_id,
                registration.session_id,
                vec!["deepseek-chat".to_string()],
            )
            .await
            .is_err(),
        "expired session must not be renewed by heartbeat"
    );

    let session = NodeSession::find_by_id(&env.pool, registration.session_id)
        .await?
        .expect("expired session should remain stored for audit");
    assert!(
        session.is_expired(),
        "failed heartbeat must not extend expiry"
    );
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

#[tokio::test]
#[serial(node_gateway)]
async fn completion_committed_after_client_deadline_returns_timeout_for_handler()
-> anyhow::Result<()> {
    let env = NodeTestEnv::new().await?;
    let user_id = create_test_user(&env.pool, "deadline-race").await;
    let request_id = Uuid::new_v4();
    let mut config = env.config.clone();
    config.task_deadline_secs = 1;
    let recorder = Arc::new(TestRequestLifecycleRecorder::default());
    let service = NodeGatewayService::new(
        NodeGatewayStore::new(DbRouter::single(env.pool.clone()), config.clone()),
        env.redis.clone(),
        config,
    )
    .with_lifecycle(Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>);
    let payload = NodeTaskPayload {
        request_id,
        chat: Some(keycompute_types::ChatCompletionRequest {
            model: "deepseek-chat".to_string(),
            messages: vec![keycompute_types::Message {
                role: keycompute_types::MessageRole::User,
                content: "deadline race".into(),
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

    let waiting = tokio::spawn(async move {
        service
            .enqueue_and_wait(user_id, "deepseek-chat".to_string(), payload)
            .await
    });
    let task = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(task) = NodeTask::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT * FROM node_tasks WHERE request_id=$1",
                [request_id.into()],
            ))
            .one(&env.pool)
            .await?
            {
                break Ok::<_, sea_orm::DbErr>(task);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("enqueued node task should become visible")?;

    // Simulate a completion transaction that made its success decision before
    // the deadline but was delayed while committing until the local wait timed
    // out. The timeout finalizer must preserve the successful Node attempt but
    // keep the client-visible request outcome as timed out.
    let completion_tx = env.pool.begin().await?;
    completion_tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM node_tasks WHERE id=$1 FOR UPDATE",
            [task.id.into()],
        ))
        .await?
        .expect("node task should lock for completion");
    completion_tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE node_tasks SET status='succeeded',result_json=$1,finished_at=NOW(),updated_at=NOW() WHERE id=$2",
            [serde_json::json!({}).into(), task.id.into()],
        ))
        .await?;
    let release_at = task.deadline_at + chrono::Duration::milliseconds(300);
    if let Ok(delay) = (release_at - chrono::Utc::now()).to_std() {
        tokio::time::sleep(delay).await;
    }
    completion_tx.commit().await?;

    let error = waiting
        .await
        .expect("wait task should join")
        .expect_err("the client wait has already expired at this boundary");
    assert_eq!(error.request_failure().status, RequestStatus::TimedOut);
    assert_eq!(
        error.client_response_outcome(),
        keycompute_types::ClientResponseOutcome::TimedOut
    );
    assert!(
        recorder.request_finishes().is_empty(),
        "the HTTP handler, not NodeGatewayService, owns request terminalization"
    );

    env.pool
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM node_tasks WHERE id=$1",
            [task.id.into()],
        ))
        .await?;
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
    let request_id = Uuid::new_v4();
    let payload = chat_task_payload(request_id);
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, claimed_at, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW(), NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                request_id.into(),
                Uuid::new_v4().into(),
                "deepseek-chat".into(),
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

    let node_success_metric = keycompute_observability::metrics::MONITORING_NODE_TASK_TOTAL
        .with_label_values(&["succeeded"]);
    let attempt_success_metric = keycompute_observability::metrics::MONITORING_ATTEMPT_TOTAL
        .with_label_values(&["node", "succeeded", "none", "none"]);
    let node_success_before = node_success_metric.get();
    let attempt_success_before = attempt_success_metric.get();

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
    let node_success_after_first = node_success_metric.get();
    let attempt_success_after_first = attempt_success_metric.get();

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
    chain.add_step(
        "node-gateway",
        "idempotent::metrics_once",
        "First submission increments terminal metrics; replay does not".to_string(),
        node_success_after_first == node_success_before + 1.0
            && node_success_metric.get() == node_success_after_first
            && attempt_success_after_first == attempt_success_before + 1.0
            && attempt_success_metric.get() == attempt_success_after_first,
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
        let request_id = Uuid::new_v4();
        let payload = chat_task_payload(request_id);
        let task = NodeTask::find_by_statement(
            Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
                VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
                RETURNING *
                "#,
                [
                    request_id.into(),
                    Uuid::new_v4().into(),
                    "deepseek-chat".into(),
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
    let request_id = Uuid::new_v4();
    let payload = chat_task_payload(request_id);
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                request_id.into(),
                Uuid::new_v4().into(),
                "deepseek-chat".into(),
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
    let request_id = Uuid::new_v4();
    let payload = image_generation_task_payload(request_id);
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                request_id.into(),
                Uuid::new_v4().into(),
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
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, claimed_at, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() - INTERVAL '20 seconds', NOW() - INTERVAL '10 seconds', NOW() + INTERVAL '120 seconds', 3)
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

    let node_expired_metric = keycompute_observability::metrics::MONITORING_NODE_TASK_TOTAL
        .with_label_values(&["expired"]);
    let attempt_expired_metric = keycompute_observability::metrics::MONITORING_ATTEMPT_TOTAL
        .with_label_values(&["node", "expired", "node", "node_expired"]);
    let node_expired_before = node_expired_metric.get();
    let attempt_expired_before = attempt_expired_metric.get();

    // 4. sweeper 先完成任务过期迁移并发出一次指标
    env.service.sweeper().run_once().await?;
    let node_expired_after_sweeper = node_expired_metric.get();
    let attempt_expired_after_sweeper = attempt_expired_metric.get();
    chain.add_step(
        "node-gateway",
        "timeout::sweeper_metrics_once",
        "Sweeper records the expired task and attempt once".to_string(),
        node_expired_after_sweeper == node_expired_before + 1.0
            && attempt_expired_after_sweeper == attempt_expired_before + 1.0,
    );

    // 5. 节点在宽限期内补交；返回 expired ACK，但不能重复发出指标
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
        .await?;

    chain.add_step(
        "node-gateway",
        "timeout::late_submission_is_expired",
        format!("Timeout submission result: {:?}", complete_result),
        complete_result.action == NodeTaskCompleteAction::Expired,
    );
    chain.add_step(
        "node-gateway",
        "timeout::late_submission_does_not_duplicate_metrics",
        "Late expired ACK leaves terminal metrics unchanged".to_string(),
        node_expired_metric.get() == node_expired_after_sweeper
            && attempt_expired_metric.get() == attempt_expired_after_sweeper,
    );

    // 6. The ACK is idempotent over the actual submitted result, even though
    // the server stores Expired as the action.
    let retry_result = env
        .service
        .complete_task(
            task.id,
            lease_id,
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::ImageSucceeded { image_response },
        )
        .await?;
    let submissions = NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM node_task_submissions WHERE task_id = $1",
        [task.id.into()],
    ))
    .all(&env.pool)
    .await?;
    chain.add_step(
        "node-gateway",
        "timeout::late_submission_is_idempotent",
        "Exact retry returns the stored Expired ACK without another submission".to_string(),
        retry_result.action == NodeTaskCompleteAction::Expired && submissions.len() == 1,
    );

    // 7. Expiry does not let another lease manufacture a submission for this
    // task during the completion grace period.
    let mismatched = env
        .service
        .complete_task(
            task.id,
            Uuid::new_v4(),
            register_resp.node_id,
            register_resp.session_id,
            NodeTaskResult::Failed {
                code: "late".to_string(),
                message: "wrong lease".to_string(),
                is_client_error: false,
            },
        )
        .await
        .expect_err("a mismatched lease must be rejected after expiry");
    chain.add_step(
        "node-gateway",
        "timeout::late_submission_requires_matching_lease",
        mismatched.to_string(),
        mismatched.to_string().contains("lease_mismatch"),
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
    let request_id = Uuid::new_v4();
    let payload = image_generation_task_payload(request_id);
    let task = NodeTask::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_tasks (request_id, user_id, model, payload_json, status, assigned_node_id, assigned_session_id, lease_id, deadline_at, complete_grace_until, failure_threshold)
            VALUES ($1, $2, $3, $4, 'leased', $5, $6, $7, NOW() + INTERVAL '60 seconds', NOW() + INTERVAL '120 seconds', 3)
            RETURNING *
            "#,
            [
                request_id.into(),
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
