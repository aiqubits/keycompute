//! 系统设置处理器
//!
//! 处理需要 Admin 权限的系统设置请求

use crate::{
    error::{ApiError, Result},
    extractors::AuthExtractor,
    handlers::configured_public_base_url,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use keycompute_auth::Permission;
use keycompute_db::models::system_setting::setting_keys;
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};

// ==================== 系统设置 ====================

/// 系统设置（管理员视图，包含所有设置）
#[derive(Debug, Serialize, Deserialize)]
pub struct AdminSystemSettings {
    // 站点设置
    pub site_name: String,
    pub site_description: Option<String>,
    pub site_logo_url: Option<String>,
    pub site_favicon_url: Option<String>,

    // 注册设置
    pub default_user_quota: f64,

    // 限流设置
    pub default_rpm_limit: i32,
    pub default_tpm_limit: i32,

    // 系统状态
    pub maintenance_mode: bool,
    pub maintenance_message: Option<String>,

    // 分销设置
    pub distribution_enabled: bool,
    pub distribution_level1_default_ratio: f64,
    pub distribution_level2_default_ratio: f64,
    pub distribution_min_withdraw: f64,

    // 支付设置
    pub alipay_enabled: bool,
    pub wechatpay_enabled: bool,
    pub min_recharge_amount: f64,
    pub max_recharge_amount: f64,

    // 安全设置
    pub login_failed_limit: i32,
    pub login_lockout_minutes: i32,
    // 密码策略使用硬编码，参见 keycompute-auth/src/password/validator.rs

    // 公告设置
    pub system_notice: Option<String>,
    pub system_notice_enabled: bool,

    // 其他设置
    pub footer_content: Option<String>,
    pub about_content: Option<String>,
    pub terms_of_service_url: Option<String>,
    pub privacy_policy_url: Option<String>,
}

impl AdminSystemSettings {
    /// 从数据库设置列表构建
    pub fn from_settings(settings: &[keycompute_db::SystemSetting]) -> Self {
        use keycompute_db::models::system_setting::setting_keys;

        let get_value = |key: &str| -> Option<String> {
            settings
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.value.clone())
        };

        let get_bool = |key: &str, default: bool| -> bool {
            settings
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.parse_bool())
                .unwrap_or(default)
        };

        let get_int = |key: &str, default: i32| -> i32 {
            settings
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.parse_int().unwrap_or(default))
                .unwrap_or(default)
        };

        let get_decimal = |key: &str, default: f64| -> f64 {
            settings
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.parse_decimal().unwrap_or(default))
                .unwrap_or(default)
        };

        Self {
            site_name: get_value(setting_keys::SITE_NAME)
                .unwrap_or_else(|| "KeyCompute".to_string()),
            site_description: get_value(setting_keys::SITE_DESCRIPTION),
            site_logo_url: get_value(setting_keys::SITE_LOGO_URL),
            site_favicon_url: get_value(setting_keys::SITE_FAVICON_URL),

            default_user_quota: get_decimal(setting_keys::DEFAULT_USER_QUOTA, 0.0),

            default_rpm_limit: get_int(setting_keys::DEFAULT_RPM_LIMIT, 60),
            default_tpm_limit: get_int(setting_keys::DEFAULT_TPM_LIMIT, 100000),

            maintenance_mode: get_bool(setting_keys::MAINTENANCE_MODE, false),
            maintenance_message: get_value(setting_keys::MAINTENANCE_MESSAGE),

            distribution_enabled: get_bool(setting_keys::DISTRIBUTION_ENABLED, false),
            distribution_level1_default_ratio: get_decimal(
                setting_keys::DISTRIBUTION_LEVEL1_DEFAULT_RATIO,
                0.03,
            ),
            distribution_level2_default_ratio: get_decimal(
                setting_keys::DISTRIBUTION_LEVEL2_DEFAULT_RATIO,
                0.02,
            ),
            distribution_min_withdraw: get_decimal(setting_keys::DISTRIBUTION_MIN_WITHDRAW, 10.0),

            alipay_enabled: get_bool(setting_keys::ALIPAY_ENABLED, false),
            wechatpay_enabled: get_bool(setting_keys::WECHATPAY_ENABLED, false),
            min_recharge_amount: get_decimal(setting_keys::MIN_RECHARGE_AMOUNT, 1.0),
            max_recharge_amount: get_decimal(setting_keys::MAX_RECHARGE_AMOUNT, 100000.0),

            login_failed_limit: get_int(setting_keys::LOGIN_FAILED_LIMIT, 5),
            login_lockout_minutes: get_int(setting_keys::LOGIN_LOCKOUT_MINUTES, 30),
            // 密码策略使用硬编码
            system_notice: get_value(setting_keys::SYSTEM_NOTICE),
            system_notice_enabled: get_bool(setting_keys::SYSTEM_NOTICE_ENABLED, false),

            footer_content: get_value(setting_keys::FOOTER_CONTENT),
            about_content: get_value(setting_keys::ABOUT_CONTENT),
            terms_of_service_url: get_value(setting_keys::TERMS_OF_SERVICE_URL),
            privacy_policy_url: get_value(setting_keys::PRIVACY_POLICY_URL),
        }
    }
}

fn is_hidden_setting(key: &str) -> bool {
    key == setting_keys::DEFAULT_USER_ROLE
}

fn is_removed_setting(key: &str) -> bool {
    matches!(
        key,
        "allow_registration" | "registration_mode" | "email_verification_required"
    )
}

fn normalize_setting_update(key: &str, value: impl Into<String>) -> Result<String> {
    if is_hidden_setting(key) {
        return Err(ApiError::BadRequest(format!(
            "Setting {} is fixed and cannot be edited",
            key
        )));
    }

    if is_removed_setting(key) {
        return Err(ApiError::BadRequest(format!(
            "Setting {} has been removed and can no longer be edited",
            key
        )));
    }

    let value = value.into();

    if matches!(
        key,
        setting_keys::ALIPAY_ENABLED | setting_keys::WECHATPAY_ENABLED
    ) {
        return match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok("true".to_string()),
            "false" | "0" | "no" => Ok("false".to_string()),
            _ => Err(ApiError::BadRequest(format!(
                "Setting {key} must be a valid boolean"
            ))),
        };
    }

    if matches!(
        key,
        setting_keys::MIN_RECHARGE_AMOUNT | setting_keys::MAX_RECHARGE_AMOUNT
    ) {
        let amount = value.parse::<Decimal>().map_err(|_| {
            ApiError::BadRequest(format!("Setting {key} must be a valid decimal amount"))
        })?;
        let amount = amount.normalize();
        let database_max = Decimal::new(999_999_999_999, 2);
        if amount <= Decimal::ZERO || amount > database_max || amount.scale() > 2 {
            return Err(ApiError::BadRequest(format!(
                "Setting {key} must be positive, use at most two decimal places, and not exceed {database_max}"
            )));
        }
        return Ok(amount.to_string());
    }

    if key == setting_keys::DEFAULT_USER_QUOTA {
        let quota = value.parse::<f64>().map_err(|_| {
            ApiError::BadRequest("Setting default_user_quota must be a valid number".to_string())
        })?;

        if !quota.is_finite() {
            return Err(ApiError::BadRequest(
                "Setting default_user_quota must be a finite number".to_string(),
            ));
        }

        return Ok(quota.to_string());
    }

    if key == setting_keys::JWT_EXPIRE_HOURS {
        let hours = value.parse::<i64>().map_err(|_| {
            ApiError::BadRequest("Setting jwt_expire_hours must be a valid integer".to_string())
        })?;

        if hours <= 0 {
            return Err(ApiError::BadRequest(
                "Setting jwt_expire_hours must be positive".to_string(),
            ));
        }

        if hours > setting_keys::JWT_EXPIRE_HOURS_MAX {
            return Err(ApiError::BadRequest(format!(
                "Setting jwt_expire_hours must be less than or equal to {}",
                setting_keys::JWT_EXPIRE_HOURS_MAX
            )));
        }

        return Ok(hours.to_string());
    }

    Ok(value)
}

fn normalize_settings_map(
    settings: std::collections::HashMap<String, String>,
) -> Result<std::collections::HashMap<String, String>> {
    settings
        .into_iter()
        .map(|(key, value)| Ok((key.clone(), normalize_setting_update(&key, value)?)))
        .collect()
}

async fn ensure_payment_amount_range(
    db: &impl ConnectionTrait,
    settings: &std::collections::HashMap<String, String>,
) -> Result<()> {
    if !settings.contains_key(setting_keys::MIN_RECHARGE_AMOUNT)
        && !settings.contains_key(setting_keys::MAX_RECHARGE_AMOUNT)
    {
        return Ok(());
    }

    #[derive(FromQueryResult)]
    struct AmountSettingRow {
        key: String,
        value: String,
    }

    // Lock both rows in the same transaction as the update. This prevents two
    // concurrent partial updates from each validating against stale counterparts.
    let rows = AmountSettingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT key, value FROM system_settings WHERE key IN ($1, $2) ORDER BY key FOR UPDATE",
        [
            setting_keys::MIN_RECHARGE_AMOUNT.into(),
            setting_keys::MAX_RECHARGE_AMOUNT.into(),
        ],
    ))
    .all(db)
    .await
    .map_err(|error| {
        tracing::error!(%error, "Failed to lock payment amount settings");
        ApiError::Internal("Failed to validate payment settings".to_string())
    })?;
    let current = |key: &str, default: Decimal| -> Result<Decimal> {
        match rows.iter().find(|row| row.key == key) {
            Some(row) => row.value.parse::<Decimal>().map_err(|error| {
                tracing::error!(%error, key, "Stored payment amount setting is invalid");
                ApiError::Internal("Failed to validate payment settings".to_string())
            }),
            None => Ok(default),
        }
    };

    let min = match settings.get(setting_keys::MIN_RECHARGE_AMOUNT) {
        Some(value) => value
            .parse::<Decimal>()
            .map_err(|_| ApiError::BadRequest("Invalid minimum recharge amount".to_string()))?,
        None => current(setting_keys::MIN_RECHARGE_AMOUNT, Decimal::ONE)?,
    };
    let max = match settings.get(setting_keys::MAX_RECHARGE_AMOUNT) {
        Some(value) => value
            .parse::<Decimal>()
            .map_err(|_| ApiError::BadRequest("Invalid maximum recharge amount".to_string()))?,
        None => current(setting_keys::MAX_RECHARGE_AMOUNT, Decimal::new(100_000, 0))?,
    };
    validate_payment_amount_range(min, max)
}

fn validate_payment_amount_range(min: Decimal, max: Decimal) -> Result<()> {
    if min > max {
        return Err(ApiError::BadRequest(
            "min_recharge_amount must not exceed max_recharge_amount".to_string(),
        ));
    }
    Ok(())
}

fn is_truthy_setting(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes"
    )
}

fn ensure_distribution_has_public_base_url(
    app_base_url: Option<&str>,
    settings: &std::collections::HashMap<String, String>,
) -> Result<()> {
    if settings
        .get(setting_keys::DISTRIBUTION_ENABLED)
        .is_some_and(|value| is_truthy_setting(value))
        && configured_public_base_url(app_base_url).is_none()
    {
        return Err(ApiError::BadRequest(
            "APP_BASE_URL must be configured before enabling distribution".to_string(),
        ));
    }

    Ok(())
}

fn requires_protected_settings_permission(key: &str) -> bool {
    key == setting_keys::DISTRIBUTION_ENABLED
}

fn ensure_admin_permission(auth: &AuthExtractor) -> Result<()> {
    if !auth.has_permission(&Permission::SystemAdmin) {
        return Err(ApiError::Forbidden("Admin permission required".to_string()));
    }
    Ok(())
}

fn ensure_setting_update_allowed(auth: &AuthExtractor, key: &str) -> Result<()> {
    if requires_protected_settings_permission(key)
        && !auth.has_permission(&Permission::ManageSystemSettings)
    {
        return Err(ApiError::Forbidden(
            "Protected system setting permission required".to_string(),
        ));
    }

    Ok(())
}

fn ensure_settings_update_allowed(
    auth: &AuthExtractor,
    settings: &std::collections::HashMap<String, String>,
) -> Result<()> {
    for key in settings.keys() {
        ensure_setting_update_allowed(auth, key)?;
    }

    Ok(())
}

fn settings_internal_error(operation: &'static str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(operation, %error, "System settings operation failed");
    ApiError::Internal("System settings operation failed".to_string())
}

fn payload_to_settings_map(
    payload: serde_json::Value,
) -> Result<std::collections::HashMap<String, String>> {
    if let serde_json::Value::Object(obj) = payload {
        Ok(obj
            .into_iter()
            .filter_map(|(key, value)| {
                if key == setting_keys::DEFAULT_USER_QUOTA && value.is_null() {
                    return None;
                }

                let value_str = match value {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Null => return None,
                    other => other.to_string(),
                };

                Some((key, value_str))
            })
            .collect())
    } else {
        Err(ApiError::BadRequest("Invalid request body".to_string()))
    }
}

/// 获取系统设置（管理员）
///
/// GET /api/v1/admin/settings
pub async fn get_system_settings(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<std::collections::HashMap<String, serde_json::Value>>> {
    ensure_admin_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let settings = keycompute_db::SystemSetting::find_all(pool)
        .await
        .map_err(|error| settings_internal_error("find_all", error))?;

    // 将设置列表转换为 HashMap<key, value>
    // value 根据 value_type 转换为对应的 JSON 类型
    let map: std::collections::HashMap<String, serde_json::Value> = settings
        .into_iter()
        .filter(|s| !is_hidden_setting(&s.key) && !is_removed_setting(&s.key))
        .map(|s| {
            let val = match s.value_type.as_str() {
                "bool" => match s.value.as_str() {
                    "true" | "1" | "yes" => serde_json::Value::Bool(true),
                    _ => serde_json::Value::Bool(false),
                },
                "int" | "decimal" => {
                    if let Ok(n) = s.value.parse::<f64>() {
                        serde_json::json!(n)
                    } else {
                        serde_json::Value::String(s.value)
                    }
                }
                _ => serde_json::Value::String(s.value),
            };
            (s.key, val)
        })
        .collect();

    Ok(Json(map))
}

/// 更新系统设置（管理员）
///
/// PUT /api/v1/admin/settings
pub async fn update_system_settings(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    ensure_admin_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let settings_map = normalize_settings_map(payload_to_settings_map(payload)?)?;
    ensure_settings_update_allowed(&auth, &settings_map)?;
    ensure_distribution_has_public_base_url(state.app_base_url.as_deref(), &settings_map)?;
    let txn = pool
        .begin()
        .await
        .map_err(|error| settings_internal_error("begin_batch_update", error))?;
    ensure_payment_amount_range(&txn, &settings_map).await?;

    // Validation and updates share one transaction and the payment rows remain locked.
    let updated = keycompute_db::SystemSetting::batch_update_tx(&txn, &settings_map)
        .await
        .map_err(|error| settings_internal_error("batch_update", error))?;
    txn.commit()
        .await
        .map_err(|error| settings_internal_error("commit_batch_update", error))?;

    tracing::info!(
        user_id = %auth.user_id,
        count = updated.len(),
        "System settings updated by admin"
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("{} settings updated", updated.len()),
        "updated_by": auth.user_id,
    })))
}

/// 获取单个设置（管理员）
///
/// GET /api/v1/admin/settings/:key
pub async fn get_system_setting_by_key(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<keycompute_db::SystemSettingResponse>> {
    ensure_admin_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    if is_hidden_setting(&key) || is_removed_setting(&key) {
        return Err(ApiError::NotFound(format!("Setting not found: {}", key)));
    }

    let setting = keycompute_db::SystemSetting::find_by_key(pool, &key)
        .await
        .map_err(|error| settings_internal_error("find_by_key", error))?
        .ok_or_else(|| ApiError::NotFound(format!("Setting not found: {}", key)))?;

    Ok(Json(setting.into()))
}

/// 更新单个设置（管理员）
///
/// PUT /api/v1/admin/settings/:key
pub async fn update_system_setting_by_key(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(payload): Json<keycompute_db::UpdateSystemSettingRequest>,
) -> Result<Json<keycompute_db::SystemSettingResponse>> {
    ensure_admin_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    if is_hidden_setting(&key) || is_removed_setting(&key) {
        return Err(ApiError::NotFound(format!("Setting not found: {}", key)));
    }

    ensure_setting_update_allowed(&auth, &key)?;

    let normalized_value = normalize_setting_update(&key, payload.value)?;
    let mut settings_map = std::collections::HashMap::new();
    settings_map.insert(key.clone(), normalized_value.clone());
    ensure_distribution_has_public_base_url(state.app_base_url.as_deref(), &settings_map)?;
    let txn = pool
        .begin()
        .await
        .map_err(|error| settings_internal_error("begin_single_update", error))?;
    ensure_payment_amount_range(&txn, &settings_map).await?;

    let setting = keycompute_db::SystemSetting::update_value(&txn, &key, &normalized_value)
        .await
        .map_err(|error| settings_internal_error("single_update", error))?;
    txn.commit()
        .await
        .map_err(|error| settings_internal_error("commit_single_update", error))?;

    tracing::info!(
        user_id = %auth.user_id,
        key = %key,
        "System setting updated by admin"
    );

    Ok(Json(setting.into()))
}

// ==================== 公开设置（无需认证） ====================

/// 获取公开系统设置
///
/// GET /api/v1/settings/public
///
/// 返回前端需要的非敏感系统设置，无需认证
pub async fn get_public_settings(
    State(state): State<AppState>,
) -> Result<Json<keycompute_db::PublicSettings>> {
    // 如果数据库未配置，返回默认设置
    let settings = if let Some(pool) = state.pool.as_deref() {
        keycompute_db::SystemSetting::get_public_settings(pool).await
    } else {
        keycompute_db::PublicSettings::default()
    };

    Ok(Json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_hidden_setting_marks_default_user_role() {
        assert!(is_hidden_setting(setting_keys::DEFAULT_USER_ROLE));
        assert!(!is_hidden_setting("site_name"));
    }

    #[test]
    fn test_removed_setting_marks_allow_registration() {
        assert!(is_removed_setting("allow_registration"));
        assert!(!is_removed_setting("site_name"));
    }

    #[test]
    fn test_normalize_setting_update_rejects_default_user_role() {
        let err = normalize_setting_update(setting_keys::DEFAULT_USER_ROLE, "user").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("cannot be edited")));
    }

    #[test]
    fn test_normalize_setting_update_accepts_normal_setting() {
        assert_eq!(
            normalize_setting_update("site_name", "KeyCompute").unwrap(),
            "KeyCompute"
        );
    }

    #[test]
    fn test_normalize_setting_update_rejects_removed_setting() {
        let err = normalize_setting_update("allow_registration", "false").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("removed")));
    }

    #[test]
    fn test_normalize_setting_update_accepts_negative_default_user_quota() {
        assert_eq!(
            normalize_setting_update(setting_keys::DEFAULT_USER_QUOTA, "-1").unwrap(),
            "-1"
        );
    }

    #[test]
    fn test_normalize_setting_update_rejects_invalid_default_user_quota() {
        let err =
            normalize_setting_update(setting_keys::DEFAULT_USER_QUOTA, "not-a-number").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("default_user_quota")));
    }

    #[test]
    fn test_normalize_setting_update_rejects_invalid_jwt_expire_hours() {
        let err = normalize_setting_update(setting_keys::JWT_EXPIRE_HOURS, "0").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("jwt_expire_hours")));
    }

    #[test]
    fn test_normalize_setting_update_rejects_non_numeric_jwt_expire_hours() {
        let err = normalize_setting_update(setting_keys::JWT_EXPIRE_HOURS, "abc").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("jwt_expire_hours")));
    }

    #[test]
    fn test_normalize_setting_update_rejects_negative_jwt_expire_hours() {
        let err = normalize_setting_update(setting_keys::JWT_EXPIRE_HOURS, "-1").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("jwt_expire_hours")));
    }

    #[test]
    fn test_normalize_setting_update_rejects_too_large_jwt_expire_hours() {
        let too_large = (setting_keys::JWT_EXPIRE_HOURS_MAX + 1).to_string();
        let err = normalize_setting_update(setting_keys::JWT_EXPIRE_HOURS, too_large).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("jwt_expire_hours")));
    }

    #[test]
    fn test_normalize_setting_update_accepts_valid_jwt_expire_hours() {
        assert_eq!(
            normalize_setting_update(setting_keys::JWT_EXPIRE_HOURS, "12").unwrap(),
            "12"
        );
    }

    #[test]
    fn test_payment_toggles_require_real_boolean_values() {
        assert_eq!(
            normalize_setting_update(setting_keys::ALIPAY_ENABLED, "YES").unwrap(),
            "true"
        );
        assert_eq!(
            normalize_setting_update(setting_keys::WECHATPAY_ENABLED, "0").unwrap(),
            "false"
        );
        let error = normalize_setting_update(setting_keys::ALIPAY_ENABLED, "enabled").unwrap_err();
        assert!(matches!(error, ApiError::BadRequest(message) if message.contains("boolean")));
    }

    #[test]
    fn test_recharge_amount_settings_enforce_database_precision() {
        assert_eq!(
            normalize_setting_update(setting_keys::MIN_RECHARGE_AMOUNT, "1.2300").unwrap(),
            "1.23"
        );
        for invalid in ["0", "-1", "1.001", "10000000000"] {
            assert!(
                normalize_setting_update(setting_keys::MAX_RECHARGE_AMOUNT, invalid).is_err(),
                "{invalid} should be rejected"
            );
        }
    }

    #[test]
    fn test_recharge_minimum_cannot_exceed_maximum() {
        assert!(validate_payment_amount_range(Decimal::new(1, 0), Decimal::new(10, 0)).is_ok());
        let error =
            validate_payment_amount_range(Decimal::new(11, 0), Decimal::new(10, 0)).unwrap_err();
        assert!(
            matches!(error, ApiError::BadRequest(message) if message.contains("must not exceed"))
        );
    }

    #[test]
    fn test_payload_to_settings_map_ignores_null_default_user_quota() {
        let payload = serde_json::json!({
            "default_user_quota": null,
            "site_name": "KeyCompute"
        });

        let map = payload_to_settings_map(payload).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("site_name"), Some(&"KeyCompute".to_string()));
        assert!(!map.contains_key(setting_keys::DEFAULT_USER_QUOTA));
    }

    #[test]
    fn test_enable_distribution_requires_public_base_url() {
        let mut settings = std::collections::HashMap::new();
        settings.insert(
            setting_keys::DISTRIBUTION_ENABLED.to_string(),
            "true".to_string(),
        );

        let err = ensure_distribution_has_public_base_url(None, &settings).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("APP_BASE_URL")));
    }

    #[test]
    fn test_enable_distribution_accepts_public_base_url() {
        let mut settings = std::collections::HashMap::new();
        settings.insert(
            setting_keys::DISTRIBUTION_ENABLED.to_string(),
            "true".to_string(),
        );

        assert!(
            ensure_distribution_has_public_base_url(Some("https://app.example.com"), &settings)
                .is_ok()
        );
    }

    #[test]
    fn test_distribution_toggle_requires_protected_settings_permission() {
        let auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "system")
            .with_permissions(vec![Permission::SystemAdmin]);

        let err =
            ensure_setting_update_allowed(&auth, setting_keys::DISTRIBUTION_ENABLED).unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(msg) if msg.contains("permission required")));
    }

    #[test]
    fn test_distribution_toggle_allows_protected_settings_permission() {
        let auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "admin")
            .with_permissions(vec![Permission::ManageSystemSettings]);

        assert!(ensure_setting_update_allowed(&auth, setting_keys::DISTRIBUTION_ENABLED).is_ok());
    }

    #[test]
    fn admin_settings_access_uses_permissions_instead_of_role() {
        let role_only =
            AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "system");
        assert!(ensure_admin_permission(&role_only).is_err());

        let permission_only =
            AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "user")
                .with_permissions(vec![Permission::SystemAdmin]);
        assert!(ensure_admin_permission(&permission_only).is_ok());
    }

    #[tokio::test]
    async fn settings_database_errors_are_hidden_from_responses() {
        use axum::response::IntoResponse;

        let response = settings_internal_error(
            "test_operation",
            "duplicate key violates constraint secret_table_key",
        )
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("Internal server error"));
        assert!(!body.contains("secret_table_key"));
        assert!(!body.contains("duplicate key"));
    }
}
