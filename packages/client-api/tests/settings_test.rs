//! 系统设置模块集成测试

use client_api::api::settings::SettingsApi;
use client_api::error::ClientError;
use std::collections::HashMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

mod common;
use common::{create_test_client, fixtures};

// ==================== Admin 接口测试 ====================

#[tokio::test]
async fn test_get_system_settings_success() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/settings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "site_name": "KeyCompute",
            "maintenance_mode": false,
            "max_api_keys_per_user": 10,
            "default_quota": 1000.0
        })))
        .mount(&mock_server)
        .await;

    let result = settings_api
        .get_system_settings(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let settings = result.unwrap();
    assert!(settings.contains_key("site_name"));
    assert!(settings.contains_key("maintenance_mode"));
}

#[tokio::test]
async fn test_update_system_settings_success() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    Mock::given(method("PUT"))
        .and(path("/api/v1/settings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": "Settings updated successfully"
        })))
        .mount(&mock_server)
        .await;

    let mut updates = HashMap::new();
    updates.insert("site_name".to_string(), serde_json::json!("Updated Name"));
    updates.insert("maintenance_mode".to_string(), serde_json::json!(true));

    let result = settings_api
        .update_system_settings(&updates, fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.message, "Settings updated successfully");
}

#[tokio::test]
async fn test_get_system_setting_by_key_success() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/settings/site_name"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "key": "site_name",
            "value": "KeyCompute",
            "value_type": "string",
            "description": "Platform site name"
        })))
        .mount(&mock_server)
        .await;

    let result = settings_api
        .get_system_setting_by_key("site_name", fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let record = result.unwrap();
    assert_eq!(record.key, "site_name");
    assert_eq!(record.value, "KeyCompute");
}

#[tokio::test]
async fn test_update_system_setting_by_key_success() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    Mock::given(method("PUT"))
        .and(path("/api/v1/settings/site_name"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "key": "site_name",
            "value": "New Site Name",
            "value_type": "string",
            "description": "Platform site name"
        })))
        .mount(&mock_server)
        .await;

    let result = settings_api
        .update_system_setting_by_key(
            "site_name",
            &serde_json::json!("New Site Name"),
            fixtures::TEST_ACCESS_TOKEN,
        )
        .await;

    assert!(result.is_ok());
    let record = result.unwrap();
    assert_eq!(record.key, "site_name");
    assert_eq!(record.value, "New Site Name");
}

#[tokio::test]
async fn test_get_system_settings_unauthorized() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/settings"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "Unauthorized"
        })))
        .mount(&mock_server)
        .await;

    let result = settings_api.get_system_settings("invalid_token").await;

    assert!(matches!(result.unwrap_err(), ClientError::Unauthorized(_)));
}

#[tokio::test]
async fn test_get_system_settings_forbidden() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/settings"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "error": "Admin access required"
        })))
        .mount(&mock_server)
        .await;

    let result = settings_api
        .get_system_settings(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(matches!(result.unwrap_err(), ClientError::Forbidden(_)));
}

// ==================== 公开接口测试 ====================

#[tokio::test]
async fn test_get_public_settings_success() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/settings/public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "site_name": "KeyCompute",
            "site_description": null,
            "site_logo_url": null,
            "site_favicon_url": null,
            "maintenance_mode": false,
            "maintenance_message": null,
            "alipay_enabled": false,
            "wechatpay_enabled": false,
            "system_notice": null,
            "system_notice_enabled": false,
            "footer_content": null,
            "about_content": null,
            "terms_of_service_url": null,
            "privacy_policy_url": null
        })))
        .mount(&mock_server)
        .await;

    let result = settings_api.get_public_settings().await;

    assert!(result.is_ok());
    let settings = result.unwrap();
    assert_eq!(settings.site_name, "KeyCompute");
}

#[tokio::test]
async fn test_get_public_settings_empty() {
    let (client, mock_server) = create_test_client().await;
    let settings_api = SettingsApi::new(&client);

    // 公开设置返回空对象时，必需字段会有默认值
    Mock::given(method("GET"))
        .and(path("/api/v1/settings/public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "site_name": "",
            "maintenance_mode": false,
            "alipay_enabled": false,
            "wechatpay_enabled": false,
            "system_notice_enabled": false
        })))
        .mount(&mock_server)
        .await;

    let result = settings_api.get_public_settings().await;

    assert!(result.is_ok());
    let settings = result.unwrap();
    assert!(settings.site_name.is_empty());
}
