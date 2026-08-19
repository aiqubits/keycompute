//! 入口协议隔离端到端测试
//!
//! 验证本次路由协议隔离改动的两个 HTTP 面：
//! - `/api/v1/debug/routing?entry=`：entry 参数按入口协议隔离模拟路由
//!   （Anthropic 入口只从 anthropic 协议账号选路，无同协议候选时不跨协议兜底）
//! - `/v1/models?protocol=`：模型列表按入口协议过滤
//!   （缺省按 openai 入口过滤，只列出该协议账号声明的模型）
//!
//! # 运行前提
//!
//! 需要真实 PostgreSQL（`DATABASE_URL`，缺省 localhost:5432/keycompute）；
//! 数据库不可用时测试自动跳过。
//!
//! 并行隔离：各测试使用独立租户与独特模型名（kc-e2e-*），互不干扰；
//! 结尾删除自己创建的账号，避免污染 `find_enabled_all`。

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use integration_tests::common::generate_test_id;
use integration_tests::db::{cleanup_test_data, create_test_tenant};
use keycompute_db::models::account::CreateAccountRequest;
use keycompute_db::{Account, CreateUserRequest, DbRouter, User};
use keycompute_runtime::{ApiKeyCrypto, encrypt_api_key, set_global_crypto};
use keycompute_server::create_router;
use keycompute_server::state::AppState;
use keycompute_types::UserRole;
use sea_orm::{Database, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

/// 获取测试用 Database URL
fn get_database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
    })
}

/// 尝试创建数据库连接（DB 不可用时跳过）
async fn try_create_db_pool() -> Option<DatabaseConnection> {
    let url = get_database_url();
    match Database::connect(&url).await {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!("SKIP: Database not reachable: {}", e);
            None
        }
    }
}

/// 创建测试账号（启用、指定协议与模型），返回账号句柄供结尾清理。
///
/// 上游 key 使用全局加密器加密（与 routing crate 单元测试一致）：
/// `set_global_crypto` 为 OnceLock 幂等设置，`encrypt_api_key` 始终用
/// 当前全局 key 加密/解密，无论并行测试是否先设置了 key，解密必成功。
async fn create_test_account(
    pool: &DatabaseConnection,
    tenant_id: Uuid,
    provider: &str,
    models: &[&str],
) -> Account {
    set_global_crypto(&ApiKeyCrypto::generate_key()).expect("set global crypto");
    let upstream_api_key_encrypted = encrypt_api_key("sk-e2e-test-plain")
        .expect("encrypt test key")
        .into_inner();
    Account::create(
        pool,
        &CreateAccountRequest {
            tenant_id,
            provider: provider.to_string(),
            name: format!("e2e-{provider}"),
            endpoint: format!("https://{provider}.example.com/v1"),
            upstream_api_key_encrypted,
            upstream_api_key_preview: "sk-e2e****".to_string(),
            rpm_limit: Some(60),
            tpm_limit: Some(100_000),
            priority: Some(100),
            models_supported: models.iter().map(|m| m.to_string()).collect(),
            visibility: Some("tenant".to_string()),
        },
    )
    .await
    .expect("create test account")
}

/// 创建租户 + Admin 用户 + 带数据库连接的路由器，返回 (app, admin token)
async fn build_admin_app(
    pool: &DatabaseConnection,
    suffix: &str,
    test_id: &str,
) -> (Router, String) {
    let tenant = create_test_tenant(pool, suffix, test_id).await;
    let admin = User::create(
        pool,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: format!("proto-admin-{}@example.com", test_id),
            name: Some("Protocol Isolation Admin".to_string()),
            role: Some(UserRole::Admin),
        },
    )
    .await
    .expect("create admin user");

    // AppState 使用默认 JWT 配置；从 state 的验证器直接签发 token，
    // secret/issuer 天然一致（避免硬编码与配置漂移）
    let router = DbRouter::single(pool.clone());
    let state = AppState::with_pool(router);
    let app = create_router(state.clone());
    let jwt = state
        .auth
        .get_jwt_validator()
        .expect("jwt validator configured")
        .clone();
    let token = jwt
        .generate_token_with_version(admin.id, admin.tenant_id, &admin.role, admin.token_version)
        .expect("admin token");

    (app, token)
}

/// 调用 debug_routing 并解析响应（期望 200，诊断接口路由失败也返回 200）
async fn get_debug_routing(app: &Router, token: &str, model: &str, entry: Option<&str>) -> Value {
    let uri = match entry {
        Some(e) => format!("/api/v1/debug/routing?model={model}&entry={e}"),
        None => format!("/api/v1/debug/routing?model={model}"),
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&uri)
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("debug_routing request");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "debug_routing should return 200 with diagnostics, uri={uri}"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// 调用 /v1/models 并返回模型 id 列表（缺省或显式协议）
async fn get_model_ids(app: &Router, protocol: Option<&str>) -> Vec<String> {
    let uri = match protocol {
        Some(p) => format!("/v1/models?protocol={p}"),
        None => "/v1/models".to_string(),
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("list models request");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "list models should return 200, uri={uri}"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

// ============================================================================
// 测试 1: debug_routing 的 entry 参数按入口协议隔离路由
// ============================================================================

#[tokio::test]
async fn test_debug_routing_entry_parameter_isolates_protocol() {
    let Some(pool) = try_create_db_pool().await else {
        return;
    };
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup should succeed");

    // 同一租户配置 openai 与 anthropic 账号，均声明同一模型
    let tenant = create_test_tenant(&pool, "proto-entry", &test_id).await;
    let model = "kc-e2e-route-model";
    let openai_account = create_test_account(&pool, tenant.id, "openai", &[model]).await;
    let anthropic_account = create_test_account(&pool, tenant.id, "anthropic", &[model]).await;

    let (app, token) = build_admin_app(&pool, "proto-entry", &test_id).await;

    // openai 入口（缺省 entry）：primary 必须是 openai 协议账号
    let body = get_debug_routing(&app, &token, model, None).await;
    assert_eq!(body["routed"], true, "openai entry should route");
    assert_eq!(
        body["primary"]["provider"], "openai",
        "openai entry must not route to anthropic accounts"
    );

    // anthropic 入口（entry=anthropic）：primary 必须是 anthropic 协议账号
    let body = get_debug_routing(&app, &token, model, Some("anthropic")).await;
    assert_eq!(body["routed"], true, "anthropic entry should route");
    assert_eq!(
        body["primary"]["provider"], "anthropic",
        "anthropic entry must not route to openai accounts"
    );

    // 清理本测试账号，避免污染 /v1/models 的 find_enabled_all
    openai_account.delete(&pool).await.expect("cleanup openai");
    anthropic_account
        .delete(&pool)
        .await
        .expect("cleanup anthropic");
}

// ============================================================================
// 测试 2: debug_routing 的 anthropic 入口无同协议候选时不跨协议兜底
// ============================================================================

#[tokio::test]
async fn test_debug_routing_anthropic_entry_does_not_fallback_across_protocols() {
    let Some(pool) = try_create_db_pool().await else {
        return;
    };
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup should succeed");

    // 仅配置 openai 协议账号（无 anthropic 账号）
    let tenant = create_test_tenant(&pool, "proto-fallback", &test_id).await;
    let model = "kc-e2e-route-fallback";
    let openai_account = create_test_account(&pool, tenant.id, "openai", &[model]).await;

    let (app, token) = build_admin_app(&pool, "proto-fallback", &test_id).await;

    // 对照组：openai 入口正常路由
    let body = get_debug_routing(&app, &token, model, None).await;
    assert_eq!(body["routed"], true, "openai entry should route");
    assert_eq!(body["primary"]["provider"], "openai");

    // anthropic 入口：本协议无账号 → 不跨协议兜底到 openai，
    // routed=false 且返回带模型名的诊断提示（非静默 500）
    let body = get_debug_routing(&app, &token, model, Some("anthropic")).await;
    assert_eq!(
        body["routed"], false,
        "anthropic entry without anthropic accounts must not fall back to openai"
    );
    assert!(body["primary"].is_null(), "no primary target on failure");
    let message = body["message"]
        .as_str()
        .expect("failure message should be present");
    assert!(
        message.contains(model),
        "diagnostic message should name the model, got: {message}"
    );

    openai_account.delete(&pool).await.expect("cleanup openai");
}

// ============================================================================
// 测试 3: /v1/models?protocol= 按入口协议过滤模型列表
// ============================================================================

#[tokio::test]
async fn test_list_models_filters_by_protocol_param() {
    let Some(pool) = try_create_db_pool().await else {
        return;
    };
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup should succeed");

    // openai 账号声明两个模型，anthropic 账号声明一个模型
    let tenant = create_test_tenant(&pool, "proto-models", &test_id).await;
    let openai_models = ["kc-e2e-models-openai-a", "kc-e2e-models-openai-b"];
    let anthropic_models = ["kc-e2e-models-anthropic-a"];
    let openai_account = create_test_account(&pool, tenant.id, "openai", &openai_models).await;
    let anthropic_account =
        create_test_account(&pool, tenant.id, "anthropic", &anthropic_models).await;

    let router = DbRouter::single(pool.clone());
    let state = AppState::with_pool(router);
    let app = create_router(state);

    // 缺省（openai 入口）：只列出 openai 协议模型
    let models = get_model_ids(&app, None).await;
    assert!(models.contains(&openai_models[0].to_string()));
    assert!(models.contains(&openai_models[1].to_string()));
    assert!(
        !models.contains(&anthropic_models[0].to_string()),
        "default /v1/models must not list anthropic-protocol models"
    );

    // 显式 protocol=openai：与缺省一致
    let models = get_model_ids(&app, Some("openai")).await;
    assert!(models.contains(&openai_models[0].to_string()));
    assert!(!models.contains(&anthropic_models[0].to_string()));

    // protocol=anthropic：只列出 anthropic 协议模型
    let models = get_model_ids(&app, Some("anthropic")).await;
    assert!(models.contains(&anthropic_models[0].to_string()));
    assert!(
        !models.contains(&openai_models[0].to_string()),
        "anthropic list must not include openai-protocol models"
    );
    assert!(!models.contains(&openai_models[1].to_string()));

    // 非法协议名：400（显式拒绝，而非静默返回空列表）
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/models?protocol=deepseek")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("invalid protocol request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    openai_account.delete(&pool).await.expect("cleanup openai");
    anthropic_account
        .delete(&pool)
        .await
        .expect("cleanup anthropic");
}
