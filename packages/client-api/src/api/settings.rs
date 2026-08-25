//! 系统设置模块
//!
//! 处理系统设置的查询和更新（Admin）以及公开设置查询

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use std::collections::HashMap;

pub use super::common::MessageResponse;

/// 设置 API 客户端
#[derive(Debug, Clone)]
pub struct SettingsApi {
    client: ApiClient,
}

impl SettingsApi {
    /// 创建新的设置 API 客户端
    pub fn new(client: &ApiClient) -> Self {
        Self {
            client: client.clone(),
        }
    }

    // ==================== Admin 接口 ====================

    /// 获取所有系统设置（Admin）
    pub async fn get_system_settings(&self, token: &str) -> Result<HashMap<String, SettingValue>> {
        self.client.get_json("/api/v1/settings", Some(token)).await
    }

    /// 批量更新系统设置（Admin）
    pub async fn update_system_settings(
        &self,
        settings: &HashMap<String, serde_json::Value>,
        token: &str,
    ) -> Result<MessageResponse> {
        self.client
            .put_json("/api/v1/settings", settings, Some(token))
            .await
    }

    /// 获取指定设置（Admin）
    pub async fn get_system_setting_by_key(
        &self,
        key: &str,
        token: &str,
    ) -> Result<SystemSettingRecord> {
        self.client
            .get_json(&format!("/api/v1/settings/{}", key), Some(token))
            .await
    }

    /// 更新指定设置（Admin）
    ///
    /// 后端期望接收 `{ "value": "..." }` 结构
    pub async fn update_system_setting_by_key(
        &self,
        key: &str,
        value: &serde_json::Value,
        token: &str,
    ) -> Result<SystemSettingRecord> {
        // 将值包装在 { "value": ... } 结构中
        let payload = serde_json::json!({
            "value": value.as_str().unwrap_or(&value.to_string())
        });
        self.client
            .put_json(&format!("/api/v1/settings/{}", key), &payload, Some(token))
            .await
    }

    // ==================== 公开接口 ====================

    /// 获取公开设置（无需认证）
    pub async fn get_public_settings(&self) -> Result<PublicSettings> {
        self.client.get_json("/api/v1/settings/public", None).await
    }

    /// 获取公开设置（HashMap 格式，向后兼容）
    pub async fn get_public_settings_map(&self) -> Result<HashMap<String, SettingValue>> {
        self.client.get_json("/api/v1/settings/public", None).await
    }
}

/// 公开系统设置
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PublicSettings {
    pub site_name: String,
    pub site_description: Option<String>,
    pub site_logo_url: Option<String>,
    pub site_favicon_url: Option<String>,
    pub maintenance_mode: bool,
    pub maintenance_message: Option<String>,
    pub distribution_enabled: bool,
    pub alipay_enabled: bool,
    pub wechatpay_enabled: bool,
    pub system_notice: Option<String>,
    pub system_notice_enabled: bool,
    pub footer_content: Option<String>,
    pub about_content: Option<String>,
    pub terms_of_service_url: Option<String>,
    pub privacy_policy_url: Option<String>,
}

impl Default for PublicSettings {
    fn default() -> Self {
        Self {
            site_name: "KeyCompute".to_string(),
            site_description: Some("AI token compute service platform".to_string()),
            site_logo_url: None,
            site_favicon_url: None,
            maintenance_mode: false,
            maintenance_message: None,
            distribution_enabled: false,
            alipay_enabled: false,
            wechatpay_enabled: false,
            system_notice: None,
            system_notice_enabled: false,
            footer_content: None,
            about_content: None,
            terms_of_service_url: None,
            privacy_policy_url: None,
        }
    }
}

/// 单个系统设置记录
#[derive(Debug, Clone, Deserialize)]
pub struct SystemSettingRecord {
    pub key: String,
    pub value: String,
    pub value_type: String,
    pub description: Option<String>,
}

/// 设置值
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    /// JSON null 必须放在最前，#[serde(untagged)] 按顺序尝试反序列化
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<serde_json::Value>),
    Object(serde_json::Map<String, serde_json::Value>),
}

impl SettingValue {
    /// 是否为 null
    pub fn is_null(&self) -> bool {
        matches!(self, SettingValue::Null)
    }

    /// 获取字符串值（自动转换各种类型）
    pub fn as_string(&self) -> Option<&str> {
        match self {
            SettingValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// 将任意类型值转换为字符串
    pub fn to_string_value(&self) -> String {
        match self {
            SettingValue::String(s) => s.clone(),
            SettingValue::Boolean(b) => b.to_string(),
            SettingValue::Number(n) => {
                // 整数直接显示为整数
                if n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    n.to_string()
                }
            }
            SettingValue::Null => String::new(),
            _ => String::new(),
        }
    }

    /// 获取数字值
    pub fn as_number(&self) -> Option<f64> {
        match self {
            SettingValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// 获取布尔值
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            SettingValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }
}
