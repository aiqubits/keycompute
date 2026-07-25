//! User/Tenant 加载
//!
//! 用户和租户信息的加载与管理。

use keycompute_db::{DbRouter, Tenant, User};
use keycompute_types::{KeyComputeError, Result};
use std::sync::Arc;
use uuid::Uuid;

/// 用户信息
#[derive(Debug, Clone)]
pub struct UserInfo {
    /// 用户 ID
    pub id: Uuid,
    /// 租户 ID
    pub tenant_id: Uuid,
    /// 邮箱
    pub email: String,
    /// 名称
    pub name: String,
    /// 角色
    pub role: String,
    /// 是否激活
    pub active: bool,
}

impl UserInfo {
    /// 创建新的用户信息
    pub fn new(
        id: Uuid,
        tenant_id: Uuid,
        email: impl Into<String>,
        name: impl Into<String>,
        role: impl Into<String>,
    ) -> Self {
        Self {
            id,
            tenant_id,
            email: email.into(),
            name: name.into(),
            role: role.into(),
            active: true,
        }
    }

    /// 从数据库 User 模型转换
    pub fn from_db_user(user: &User) -> Self {
        Self {
            id: user.id,
            tenant_id: user.tenant_id,
            email: user.email.clone(),
            name: user.name.clone().unwrap_or_default(),
            role: user.role.clone(),
            active: true,
        }
    }

    /// 检查是否是管理员
    pub fn is_admin(&self) -> bool {
        self.role == "admin" || self.role == "system"
    }

    /// 检查是否是系统管理员
    pub fn is_system_admin(&self) -> bool {
        self.role == "system"
    }
}

/// 租户信息
#[derive(Debug, Clone)]
pub struct TenantInfo {
    /// 租户 ID
    pub id: Uuid,
    /// 租户名称
    pub name: String,
    /// 租户 slug（唯一标识）
    pub slug: String,
    /// 是否激活
    pub active: bool,
    /// 配置
    pub config: TenantConfig,
}

/// 租户配置
#[derive(Debug, Clone, Default)]
pub struct TenantConfig {
    /// 默认 RPM 限制
    pub default_rpm_limit: u32,
    /// 默认 TPM 限制
    pub default_tpm_limit: u32,
}

impl TenantInfo {
    /// 创建新的租户信息
    pub fn new(id: Uuid, name: impl Into<String>, slug: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            slug: slug.into(),
            active: true,
            config: TenantConfig::default(),
        }
    }

    /// 从数据库 Tenant 模型转换
    pub fn from_db_tenant(tenant: &Tenant) -> Self {
        Self {
            id: tenant.id,
            name: tenant.name.clone(),
            slug: tenant.slug.clone(),
            active: tenant.status == "active",
            config: TenantConfig {
                default_rpm_limit: tenant.default_rpm_limit as u32,
                default_tpm_limit: tenant.default_tpm_limit as u32,
            },
        }
    }

    /// 设置配置
    pub fn with_config(mut self, config: TenantConfig) -> Self {
        self.config = config;
        self
    }

    /// 检查租户是否激活
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// 用户服务
#[derive(Clone)]
pub struct UserService {
    /// 数据库连接池（可选）
    pool: Option<Arc<DbRouter>>,
}

impl std::fmt::Debug for UserService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserService")
            .field("pool", &self.pool.as_deref().map(|_| "DatabaseConnection"))
            .finish()
    }
}

impl UserService {
    /// 创建新的用户服务（无数据库连接）
    pub fn new() -> Self {
        Self { pool: None }
    }

    /// 创建带数据库连接的用户服务
    pub fn with_pool(pool: Arc<DbRouter>) -> Self {
        Self { pool: Some(pool) }
    }

    /// 根据 ID 加载用户
    pub async fn load_user(&self, user_id: Uuid) -> Result<UserInfo> {
        tracing::debug!(user_id = %user_id, "Loading user");

        if let Some(pool) = self.pool.as_deref() {
            let user = User::find_by_id(pool, user_id)
                .await
                .map_err(|e| KeyComputeError::DatabaseError(format!("Failed to load user: {}", e)))?
                .ok_or_else(|| {
                    KeyComputeError::AuthError(format!("User not found: {}", user_id))
                })?;

            tracing::info!(user_id = %user.id, tenant_id = %user.tenant_id, role = %user.role, "User loaded");
            return Ok(UserInfo::from_db_user(&user));
        }

        // 无数据库连接，返回模拟数据
        tracing::warn!("No database connection, returning mock user");
        Ok(UserInfo::new(
            user_id,
            Uuid::new_v4(),
            "user@example.com",
            "Test User",
            "user",
        ))
    }

    /// 加载用户当前的 token_version
    ///
    /// 用于 JWT 失效校验。无数据库连接时返回 `None`（跳过校验，保持结构性验证行为）。
    ///
    /// 该读取强制走主库（`write_conn`）：这是安全敏感校验，若走读副本，
    /// 复制延迟会形成一个窗口，使密码重置/登出后本应失效的旧 token 仍被放行。
    pub async fn load_token_version(&self, user_id: Uuid) -> Result<Option<i32>> {
        if let Some(pool) = self.pool.as_deref() {
            let version = User::find_token_version(pool.write_conn(), user_id)
                .await
                .map_err(|e| {
                    KeyComputeError::DatabaseError(format!("Failed to load token version: {}", e))
                })?
                .ok_or_else(|| {
                    KeyComputeError::AuthError(format!("User not found: {}", user_id))
                })?;
            return Ok(Some(version));
        }

        Ok(None)
    }

    /// 根据 ID 加载租户
    pub async fn load_tenant(&self, tenant_id: Uuid) -> Result<TenantInfo> {
        tracing::debug!(tenant_id = %tenant_id, "Loading tenant");

        if let Some(pool) = self.pool.as_deref() {
            let tenant = Tenant::find_by_id(pool, tenant_id)
                .await
                .map_err(|e| {
                    KeyComputeError::DatabaseError(format!("Failed to load tenant: {}", e))
                })?
                .ok_or_else(|| {
                    KeyComputeError::AuthError(format!("Tenant not found: {}", tenant_id))
                })?;

            // 检查租户状态：具体状态值属于内部信息只进日志，不拼入错误消息
            if tenant.status != "active" {
                tracing::warn!(tenant_id = %tenant.id, status = %tenant.status, "Tenant is not active");
                return Err(KeyComputeError::AuthError("Tenant is not active".into()));
            }

            tracing::info!(tenant_id = %tenant.id, name = %tenant.name, status = %tenant.status, "Tenant loaded");
            return Ok(TenantInfo::from_db_tenant(&tenant));
        }

        // 无数据库连接，返回模拟数据
        tracing::warn!("No database connection, returning mock tenant");
        Ok(TenantInfo::new(tenant_id, "Test Tenant", "test-tenant"))
    }

    /// 根据 Produce AI Key ID 加载用户信息
    pub async fn load_by_produce_ai_key(&self, produce_ai_key_id: Uuid) -> Result<UserInfo> {
        tracing::debug!(produce_ai_key_id = %produce_ai_key_id, "Loading user by Produce AI key");

        if let Some(pool) = self.pool.as_deref() {
            // 通过 Produce AI Key 查找用户
            use keycompute_db::ProduceAiKey;
            let produce_ai_key = ProduceAiKey::find_by_id(pool, produce_ai_key_id)
                .await
                .map_err(|e| {
                    KeyComputeError::DatabaseError(format!("Failed to load Produce AI key: {}", e))
                })?
                .ok_or_else(|| {
                    KeyComputeError::AuthError(format!(
                        "Produce AI key not found: {}",
                        produce_ai_key_id
                    ))
                })?;

            // 检查 Produce AI Key 是否有效
            if !produce_ai_key.is_valid() {
                return Err(KeyComputeError::AuthError(
                    "Produce AI key is revoked or expired".into(),
                ));
            }

            // 加载用户
            return self.load_user(produce_ai_key.user_id).await;
        }

        // 无数据库连接，返回模拟数据
        Ok(UserInfo::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "api@example.com",
            "API User",
            "user",
        ))
    }

    /// 加载用户并验证租户归属
    pub async fn load_user_with_tenant_validation(
        &self,
        user_id: Uuid,
        expected_tenant_id: Uuid,
    ) -> Result<UserInfo> {
        let user = self.load_user(user_id).await?;

        if user.tenant_id != expected_tenant_id {
            tracing::warn!(
                user_id = %user_id,
                expected_tenant_id = %expected_tenant_id,
                actual_tenant_id = %user.tenant_id,
                "User does not belong to expected tenant"
            );
            return Err(KeyComputeError::AuthError(
                "User does not belong to the expected tenant".into(),
            ));
        }

        Ok(user)
    }

    /// 加载用户和租户信息
    pub async fn load_user_and_tenant(&self, user_id: Uuid) -> Result<(UserInfo, TenantInfo)> {
        let user = self.load_user(user_id).await?;
        let tenant = self.load_tenant(user.tenant_id).await?;
        Ok((user, tenant))
    }
}

impl Default for UserService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_info() {
        let user = UserInfo::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "test@example.com",
            "Test User",
            "user",
        );

        assert_eq!(user.email, "test@example.com");
        assert!(!user.is_admin());
        assert!(user.active);
    }

    #[test]
    fn test_user_info_admin() {
        let user = UserInfo::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "admin@example.com",
            "Admin User",
            "admin",
        );

        assert!(user.is_admin());
    }

    #[test]
    fn test_tenant_info() {
        let tenant = TenantInfo::new(Uuid::new_v4(), "Test", "test");
        assert_eq!(tenant.name, "Test");
        assert_eq!(tenant.slug, "test");
        assert!(tenant.active);
        assert!(tenant.is_active());
    }

    #[test]
    fn test_tenant_config() {
        let config = TenantConfig {
            default_rpm_limit: 100,
            default_tpm_limit: 10000,
        };
        let tenant = TenantInfo::new(Uuid::new_v4(), "Test", "test").with_config(config);
        assert_eq!(tenant.config.default_rpm_limit, 100);
        assert_eq!(tenant.config.default_tpm_limit, 10000);
    }

    #[tokio::test]
    async fn test_user_service_no_db() {
        let service = UserService::new();
        let user_id = Uuid::new_v4();

        let user = service.load_user(user_id).await;
        assert!(user.is_ok());
        assert_eq!(user.unwrap().id, user_id);
    }

    #[tokio::test]
    async fn test_user_service_load_tenant_no_db() {
        let service = UserService::new();
        let tenant_id = Uuid::new_v4();

        let tenant = service.load_tenant(tenant_id).await;
        assert!(tenant.is_ok());
        assert_eq!(tenant.unwrap().id, tenant_id);
    }

    #[tokio::test]
    async fn test_user_service_load_user_and_tenant_no_db() {
        let service = UserService::new();
        let user_id = Uuid::new_v4();

        let result = service.load_user_and_tenant(user_id).await;
        assert!(result.is_ok());
        let (user, _tenant) = result.unwrap();
        assert_eq!(user.id, user_id);
    }

    #[tokio::test]
    async fn test_user_service_load_token_version_no_db() {
        // 无数据库连接时返回 None（跳过 token_version 校验）
        let service = UserService::new();
        let result = service.load_token_version(Uuid::new_v4()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }
}
