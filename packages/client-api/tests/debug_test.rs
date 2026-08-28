//! 调试接口模块集成测试

use client_api::api::debug::DebugApi;
use client_api::error::ClientError;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

mod common;
use common::{create_test_client, fixtures};

#[tokio::test]
async fn test_debug_routing_success() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/debug/routing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "request_id": "550e8400-e29b-41d4-a716-446655440000",
            "routed": true,
            "primary": {
                "provider": "openai",
                "account_id": "550e8400-e29b-41d4-a716-446655440001",
                "endpoint": "https://api.openai.com/v1"
            },
            "fallback_chain": [],
            "pricing": {
                "model_name": "gpt-4o",
                "currency": "CNY",
                "input_price_per_1k": "0.01",
                "output_price_per_1k": "0.03"
            },
            "provider_status": [
                {
                    "provider": "openai",
                    "is_healthy": true,
                    "account_count": 2,
                    "status": "2 个账号"
                }
            ],
            "message": null
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api
        .debug_routing("gpt-4o", fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let info = result.unwrap();
    assert!(info.routed);
    assert_eq!(info.provider_status.len(), 1);
    assert_eq!(info.provider_status[0].provider, "openai");
}

#[tokio::test]
async fn test_debug_routing_preserves_the_entry_protocol() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/debug/routing"))
        .and(query_param("model", "claude-3-5-sonnet"))
        .and(query_param("entry", "anthropic"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "request_id": "550e8400-e29b-41d4-a716-446655440000",
            "routed": false,
            "primary": null,
            "fallback_chain": [],
            "pricing": {
                "model_name": "claude-3-5-sonnet",
                "currency": "CNY",
                "input_price_per_1k": "0.01",
                "output_price_per_1k": "0.03"
            },
            "provider_status": [],
            "message": "No route"
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api
        .debug_routing_for_entry(
            "claude-3-5-sonnet",
            "anthropic",
            fixtures::TEST_ACCESS_TOKEN,
        )
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_provider_health_success() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/debug/providers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "healthy_providers": ["openai", "anthropic"],
            "account_count": 3
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api
        .get_provider_health(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let health = result.unwrap();
    assert!(health.healthy_providers.contains(&"openai".to_string()));
    assert!(health.healthy_providers.contains(&"anthropic".to_string()));
    assert_eq!(health.account_count, 3);
}

#[tokio::test]
async fn test_get_gateway_status_success() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/debug/gateway/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "available": true,
            "providers": [
                {
                    "name": "openai",
                    "supported_models": ["gpt-4o", "gpt-4o-mini"],
                    "healthy": true
                }
            ],
            "config": {
                "max_retries": 3,
                "timeout_secs": 120,
                "enable_fallback": true
            }
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api
        .get_gateway_status(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let status = result.unwrap();
    assert!(status.available);
    assert_eq!(status.providers.len(), 1);
    assert_eq!(status.providers[0].name, "openai");
    assert_eq!(status.config.max_retries, 3);
}

#[tokio::test]
async fn test_get_gateway_stats_success() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/debug/gateway/stats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "total_requests": 100000,
            "successful_requests": 95000,
            "failed_requests": 5000,
            "fallback_count": 120,
            "avg_latency_ms": 125,
            "provider_stats": {
                "openai": {
                    "requests": 70000,
                    "successes": 68000,
                    "failures": 2000,
                    "avg_latency_ms": 110
                }
            }
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api
        .get_gateway_stats(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let stats = result.unwrap();
    assert_eq!(stats.total_requests, 100000);
    assert_eq!(stats.successful_requests, 95000);
    assert_eq!(stats.fallback_count, 120);
    assert_eq!(stats.avg_latency_ms, 125);
    assert_eq!(stats.provider_stats["openai"].requests, 70000);
}

#[tokio::test]
async fn test_check_provider_health_success() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("POST"))
        .and(path("/api/v1/debug/gateway/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "provider": "openai",
            "healthy": true,
            "latency_ms": 150,
            "error": null,
            "models": ["gpt-4o", "gpt-4.1"]
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api
        .check_provider_health("openai", fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let check = result.unwrap();
    assert_eq!(check.provider, "openai");
    assert!(check.healthy);
    assert_eq!(check.latency_ms, Some(150));
    assert_eq!(check.models.len(), 2);
}

#[tokio::test]
async fn test_debug_endpoints_unauthorized() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/debug/routing"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "Unauthorized"
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api.debug_routing("gpt-4o", "invalid_token").await;

    assert!(matches!(result.unwrap_err(), ClientError::Unauthorized(_)));
}

#[tokio::test]
async fn test_debug_endpoints_forbidden() {
    let (client, mock_server) = create_test_client().await;
    let debug_api = DebugApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/debug/providers"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "error": "Admin access required"
        })))
        .mount(&mock_server)
        .await;

    let result = debug_api
        .get_provider_health(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(matches!(result.unwrap_err(), ClientError::Forbidden(_)));
}
