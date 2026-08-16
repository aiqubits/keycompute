//! 渠道账号管理相关类型

use serde::{Deserialize, Serialize};

use crate::api::common::encode_query_value;

/// 账号查询参数
#[derive(Debug, Clone, Serialize, Default)]
pub struct AccountQueryParams {
    pub provider: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

impl AccountQueryParams {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_limit(mut self, limit: i32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn with_offset(mut self, offset: i32) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn to_query_string(&self) -> String {
        let mut params = Vec::new();
        if let Some(ref provider) = self.provider {
            params.push(format!("provider={}", encode_query_value(provider)));
        }
        if let Some(ref status) = self.status {
            params.push(format!("status={}", encode_query_value(status)));
        }
        if let Some(limit) = self.limit {
            params.push(format!("limit={}", limit));
        }
        if let Some(offset) = self.offset {
            params.push(format!("offset={}", offset));
        }
        params.join("&")
    }
}

/// 账号信息
#[derive(Debug, Clone, Deserialize)]
pub struct AccountInfo {
    pub id: String,
    /// 所属租户 ID
    pub tenant_id: String,
    pub name: String,
    pub provider: String,
    pub api_key_preview: String,
    /// 自定义 Base URL（Provider 端点地址）
    pub api_base: Option<String>,
    pub models: Vec<String>,
    pub rpm_limit: i32,
    pub current_rpm: i32,
    pub is_active: bool,
    pub is_healthy: bool,
    pub priority: i32,
    /// 可见性：'tenant' = 仅本租户可见，'global' = 所有租户可见
    pub visibility: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// 创建账号请求
#[derive(Debug, Clone, Serialize)]
pub struct CreateAccountRequest {
    pub name: String,
    pub provider: String,
    pub api_key: String,
    pub api_base: Option<String>,
    pub models: Vec<String>,
}

impl CreateAccountRequest {
    pub fn new(
        name: impl Into<String>,
        provider: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            provider: provider.into(),
            api_key: api_key.into(),
            api_base: None,
            models: Vec::new(),
        }
    }

    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = Some(api_base.into());
        self
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }
}

/// 更新账号请求
#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateAccountRequest {
    pub tenant_id: Option<String>,
    pub name: Option<String>,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub is_active: Option<bool>,
    /// 可见性：'tenant' = 仅本租户可见，'global' = 所有租户可见
    pub visibility: Option<String>,
}

impl UpdateAccountRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_is_active(mut self, is_active: bool) -> Self {
        self.is_active = Some(is_active);
        self
    }

    pub fn with_visibility(mut self, visibility: impl Into<String>) -> Self {
        self.visibility = Some(visibility.into());
        self
    }
}

/// 账号测试响应
#[derive(Debug, Clone, Deserialize)]
pub struct AccountTestResponse {
    pub success: bool,
    pub message: String,
    pub latency_ms: Option<i64>,
}

/// 账号模型刷新响应
#[derive(Debug, Clone, Deserialize)]
pub struct AccountRefreshResponse {
    pub success: bool,
    pub message: String,
    pub account_id: String,
    pub refreshed_by: String,
    pub previous_models: Vec<String>,
    pub updated_models: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{AccountRefreshResponse, AccountTestResponse};

    #[test]
    fn account_test_response_reads_top_level_latency() {
        let response: AccountTestResponse = serde_json::from_str(
            r#"{
                "success": false,
                "message": "Account connection test failed",
                "latency_ms": 123,
                "test_result": {"is_healthy": false}
            }"#,
        )
        .unwrap();

        assert!(!response.success);
        assert_eq!(response.latency_ms, Some(123));
    }

    #[test]
    fn account_refresh_response_matches_server_payload() {
        let response: AccountRefreshResponse = serde_json::from_str(
            r#"{
                "success": true,
                "message": "Account refreshed",
                "account_id": "account-id",
                "refreshed_by": "admin-id",
                "previous_models": ["model-a"],
                "updated_models": ["model-b"]
            }"#,
        )
        .unwrap();

        assert!(response.success);
        assert_eq!(response.account_id, "account-id");
        assert_eq!(response.refreshed_by, "admin-id");
        assert_eq!(response.previous_models, ["model-a"]);
        assert_eq!(response.updated_models, ["model-b"]);
    }
}
