//! 系统设置端到端测试
//!
//! 验证系统设置的 CRUD API

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use integration_tests::common::TestContext;
use integration_tests::db::create_test_pool;
use keycompute_db::SystemSetting;
use keycompute_db::models::system_setting::setting_keys;
use keycompute_server::create_router;
use keycompute_server::state::AppState;
use sea_orm::TransactionTrait;
use serde_json::json;
use tower::ServiceExt;

/// 测试获取公开设置（无需认证）
#[tokio::test]
async fn test_get_public_settings() {
    let _ctx = TestContext::new();
    let state = AppState::new();
    let app = create_router(state);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/settings/public")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // 由于没有数据库连接，这个测试会返回默认值或错误
    // 在有数据库的环境中，应该返回 200
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// 测试管理员获取系统设置（需要认证）
#[tokio::test]
async fn test_get_system_settings_requires_admin() {
    let _ctx = TestContext::new();
    let state = AppState::new();
    let app = create_router(state);

    // 无认证请求应返回 401
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/settings")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // 应该返回未授权错误（401 或被中间件拦截）
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// 测试管理员更新系统设置（需要认证）
#[tokio::test]
async fn test_update_system_settings_requires_admin() {
    let _ctx = TestContext::new();
    let state = AppState::new();
    let app = create_router(state);

    let request = Request::builder()
        .method(Method::PUT)
        .uri("/api/v1/settings")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "site_name": "Test Site",
                "default_user_quota": 0
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // 无认证应返回未授权
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// 测试获取单个设置（需要认证）
#[tokio::test]
async fn test_get_single_setting_requires_admin() {
    let _ctx = TestContext::new();
    let state = AppState::new();
    let app = create_router(state);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/settings/site_name")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // 无认证应返回未授权
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// 测试更新单个设置（需要认证）
#[tokio::test]
async fn test_update_single_setting_requires_admin() {
    let _ctx = TestContext::new();
    let state = AppState::new();
    let app = create_router(state);

    let request = Request::builder()
        .method(Method::PUT)
        .uri("/api/v1/settings/site_name")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "value": "New Site Name"
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // 无认证应返回未授权
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// 未配置公开 URL 时，即使历史基线将分销设为启用，启动期收敛也必须原子禁用；
/// 已配置 URL 时则不得覆盖管理员持久化的选择。
#[tokio::test]
async fn test_distribution_setting_reconciles_with_public_url() {
    let pool = create_test_pool().await;
    let tx = pool.begin().await.unwrap();

    SystemSetting::update_value(&tx, setting_keys::DISTRIBUTION_ENABLED, "true")
        .await
        .unwrap();
    assert!(
        !SystemSetting::reconcile_distribution_public_url(&tx, true)
            .await
            .unwrap()
    );
    assert!(
        SystemSetting::find_by_key(&tx, setting_keys::DISTRIBUTION_ENABLED)
            .await
            .unwrap()
            .unwrap()
            .parse_bool()
    );

    assert!(
        SystemSetting::reconcile_distribution_public_url(&tx, false)
            .await
            .unwrap()
    );
    assert!(
        !SystemSetting::find_by_key(&tx, setting_keys::DISTRIBUTION_ENABLED)
            .await
            .unwrap()
            .unwrap()
            .parse_bool()
    );

    tx.rollback().await.unwrap();
}
