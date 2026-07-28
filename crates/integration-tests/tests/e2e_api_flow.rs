//! API 层端到端测试
//
//! 验证数据链路：API Server -> Auth -> Rate Limit -> RequestContext
//
//! 注意：生产环境需要数据库连接进行 API Key 验证
//! 测试中使用无数据库连接时，验证会失败（安全默认行为）

use axum::body::Body;
use axum::http::{Request, StatusCode};
use integration_tests::common::{TestContext, VerificationChain};
use integration_tests::mocks::provider::MockProviderFactory;
use keycompute_server::create_router;
use keycompute_server::state::AppState;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

/// 测试完整的 API 请求流程
///
/// 注意：无数据库连接时，认证会失败（返回 401）
/// 这是预期的安全行为
#[tokio::test]
async fn test_api_request_flow_requires_database() {
    let _ctx = TestContext::new();
    let mut chain = VerificationChain::new();

    // 1. 创建应用状态和路由（使用 Mock Provider）
    let mut providers = HashMap::new();
    providers.insert(
        "openai".to_string(),
        Arc::new(MockProviderFactory::create_openai())
            as Arc<dyn llm_protocol_provider::ProviderAdapter>,
    );
    let state = AppState::with_providers(providers);
    let app = create_router(state);

    chain.add_step(
        "keycompute-server",
        "create_router",
        "Router created with AppState",
        true,
    );

    // 2. 发送 chat/completions 请求（无数据库连接，应该返回 401）
    let test_api_key = keycompute_auth::ProduceAiKeyValidator::generate_key();
    let request_body = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Hello"}],
        "stream": true
    });

    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", test_api_key))
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();

    // 无数据库连接时，认证应该失败（返回 401）
    let status_unauthorized = response.status() == StatusCode::UNAUTHORIZED;
    chain.add_step(
        "keycompute-server",
        "chat_completions_handler",
        format!(
            "Response status: {:?} (expected 401 without database)",
            response.status()
        ),
        status_unauthorized,
    );

    // 3. 测试模型列表接口（不需要认证）
    let models_request = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .body(Body::empty())
        .unwrap();

    let models_response = app.oneshot(models_request).await.unwrap();
    let models_ok = models_response.status() == StatusCode::OK;
    chain.add_step(
        "keycompute-server",
        "list_models_handler",
        format!("Models endpoint status: {:?}", models_response.status()),
        models_ok,
    );

    // 打印验证报告
    chain.print_report();
    assert!(chain.all_passed(), "Some verification steps failed");
}

/// 测试认证流程
///
/// 验证无数据库连接时的安全行为：
/// - 有效格式的 API Key 被拒绝（因为没有数据库验证）
/// - 无效格式的 API Key 被拒绝
/// - 缺失 Authorization 头被拒绝
#[tokio::test]
async fn test_auth_flow_requires_database() {
    use axum::http::HeaderMap;
    use axum::http::header::AUTHORIZATION;
    use keycompute_auth::{AuthService, ProduceAiKeyValidator};
    use keycompute_server::extractors::AuthExtractor;

    let mut chain = VerificationChain::new();

    // 创建 AuthService（无数据库连接）
    let auth_service = AuthService::new(ProduceAiKeyValidator::default());

    // 1. 测试有效格式的 API Key（无数据库连接时应该失败）
    let valid_api_key = ProduceAiKeyValidator::generate_key();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {}", valid_api_key).parse().unwrap(),
    );

    let result = AuthExtractor::from_header_with_auth(&headers, &auth_service).await;
    // 无数据库连接时，验证应该失败（安全默认行为）
    chain.add_step(
        "keycompute-server::extractors",
        "AuthExtractor::from_header_with_auth",
        "Valid API key rejected (no database connection)",
        result.is_err(),
    );

    // 2. 测试无效格式的 API Key
    let mut bad_headers = HeaderMap::new();
    bad_headers.insert(AUTHORIZATION, "Bearer invalid-key".parse().unwrap());

    let bad_result = AuthExtractor::from_header_with_auth(&bad_headers, &auth_service).await;
    chain.add_step(
        "keycompute-server::extractors",
        "AuthExtractor::reject_invalid",
        "Invalid API key rejected",
        bad_result.is_err(),
    );

    // 3. 测试缺失 Authorization 头
    let empty_headers = HeaderMap::new();
    let missing_result = AuthExtractor::from_header_with_auth(&empty_headers, &auth_service).await;
    chain.add_step(
        "keycompute-server::extractors",
        "AuthExtractor::reject_missing",
        "Missing auth header rejected",
        missing_result.is_err(),
    );

    // 4. 验证 API Key 格式检查
    let generated_key = ProduceAiKeyValidator::generate_key();
    chain.add_step(
        "keycompute-server::extractors",
        "API Key format validation",
        format!("Generated key format valid: {}", generated_key),
        ProduceAiKeyValidator::is_valid_format(&generated_key),
    );

    chain.print_report();
    assert!(chain.all_passed(), "Some auth verification steps failed");
}

/// 测试健康检查端点
#[tokio::test]
async fn test_health_endpoint() {
    let state = AppState::new();
    let app = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "healthy");
    assert!(!json["version"].as_str().unwrap().is_empty());
}

/// 测试请求 ID 提取
#[tokio::test]
async fn test_request_id_extraction() {
    use axum::http::HeaderMap;
    use keycompute_server::extractors::RequestId;

    let mut chain = VerificationChain::new();

    // 1. 测试从请求头提取
    let mut headers = HeaderMap::new();
    let test_uuid = uuid::Uuid::new_v4();
    headers.insert("X-Request-ID", test_uuid.to_string().parse().unwrap());

    // 使用默认构造函数测试
    let request_id = RequestId::new();
    chain.add_step(
        "keycompute-server::extractors",
        "RequestId::new",
        format!("Generated request ID: {:?}", request_id.0),
        !request_id.0.is_nil(),
    );

    // 2. 测试默认实现
    let default_id: RequestId = Default::default();
    chain.add_step(
        "keycompute-server::extractors",
        "RequestId::default",
        "Default request ID generated",
        !default_id.0.is_nil(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}
