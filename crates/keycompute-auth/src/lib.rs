//! Auth & User Module
//!
//! 提供 API Key、JWT、邮箱/密码认证功能，以及 User / Tenant 加载。

pub mod api_key;
pub mod jwt;
pub mod password;
pub mod permission;
pub mod user;

// Password 模块重新导出
pub use password::{
    CompleteRegistrationRequest, CompleteRegistrationResponse, EmailConfig, EmailService,
    EmailValidator, LoginRequest, LoginResponse, LoginService, PasswordHasher,
    PasswordResetService, PasswordValidator, RegistrationService, RequestPasswordResetRequest,
    RequestRegistrationCodeRequest, RequestRegistrationCodeResponse, ResetPasswordRequest,
};

pub use api_key::{ProduceAiKeyAuth, ProduceAiKeyValidator};
pub use jwt::{JwtClaims, JwtValidator};
use keycompute_db::DbRouter;
use keycompute_types::{KeyComputeError, Result};
pub use permission::{AuthType, Permission, PermissionChecker, build_permissions};
use std::sync::Arc;
pub use user::{TenantConfig, TenantInfo, UserInfo, UserService};
use uuid::Uuid;

/// 认证上下文
///
/// 包含用户认证后的所有信息
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// 用户 ID
    pub user_id: Uuid,
    /// 租户 ID
    pub tenant_id: Uuid,
    /// Produce AI Key ID（用户访问系统的 API Key）
    pub produce_ai_key_id: Uuid,
    /// 用户角色
    pub role: String,
    /// 权限列表
    pub permissions: Vec<Permission>,
    /// Token 版本号（仅 JWT 认证有意义，用于失效校验；API Key 认证恒为 0）
    pub token_version: i32,
    /// 用户信息（可选，延迟加载）
    pub user_info: Option<UserInfo>,
    /// 租户信息（可选，延迟加载）
    pub tenant_info: Option<TenantInfo>,
}

impl AuthContext {
    /// 创建新的认证上下文
    pub fn new(
        user_id: Uuid,
        tenant_id: Uuid,
        produce_ai_key_id: Uuid,
        role: impl Into<String>,
    ) -> Self {
        Self {
            user_id,
            tenant_id,
            produce_ai_key_id,
            role: role.into(),
            permissions: Vec::new(),
            token_version: 0,
            user_info: None,
            tenant_info: None,
        }
    }

    /// 创建带权限的认证上下文
    pub fn with_permissions(mut self, permissions: Vec<Permission>) -> Self {
        self.permissions = permissions;
        self
    }

    /// 创建带用户信息的认证上下文
    pub fn with_user_info(mut self, user_info: UserInfo) -> Self {
        self.user_info = Some(user_info);
        self
    }

    /// 创建带租户信息的认证上下文
    pub fn with_tenant_info(mut self, tenant_info: TenantInfo) -> Self {
        self.tenant_info = Some(tenant_info);
        self
    }

    /// 检查是否有指定权限
    ///
    /// 权限检查完全基于权限列表，不基于角色。
    /// 这确保了 API Key 认证（仅有 UseApi 权限）无法访问管理功能，
    /// 即使该用户的角色是 admin。
    pub fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions.contains(permission)
    }

    /// 是否是管理员
    pub fn is_admin(&self) -> bool {
        self.role == "admin" || self.role == "system"
    }

    /// 获取用户信息（如果已加载）
    pub fn user_info(&self) -> Option<&UserInfo> {
        self.user_info.as_ref()
    }

    /// 获取租户信息（如果已加载）
    pub fn tenant_info(&self) -> Option<&TenantInfo> {
        self.tenant_info.as_ref()
    }
}

/// 认证服务
///
/// 统一处理 Produce AI Key 和 JWT 认证
#[derive(Clone)]
pub struct AuthService {
    /// Produce AI Key 验证器
    produce_ai_key_validator: ProduceAiKeyValidator,
    /// JWT 验证器
    jwt_validator: Option<JwtValidator>,
    /// 用户服务
    user_service: Option<UserService>,
}

impl std::fmt::Debug for AuthService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthService")
            .field("produce_ai_key_validator", &self.produce_ai_key_validator)
            .field("jwt_validator", &self.jwt_validator)
            .field("user_service", &self.user_service)
            .finish()
    }
}

impl AuthService {
    /// 创建只使用 Produce AI Key 认证的 AuthService
    pub fn new(produce_ai_key_validator: ProduceAiKeyValidator) -> Self {
        Self {
            produce_ai_key_validator,
            jwt_validator: None,
            user_service: None,
        }
    }

    /// 创建支持 JWT 的 AuthService
    pub fn with_jwt(mut self, jwt_validator: JwtValidator) -> Self {
        self.jwt_validator = Some(jwt_validator);
        self
    }

    /// 创建带 UserService 的 AuthService
    pub fn with_user_service(mut self, user_service: UserService) -> Self {
        self.user_service = Some(user_service);
        self
    }

    /// 创建完整的 AuthService（带数据库连接）
    pub fn with_pool(pool: Arc<DbRouter>) -> Self {
        let produce_ai_key_validator = ProduceAiKeyValidator::with_pool(Arc::clone(&pool));
        let user_service = UserService::with_pool(Arc::clone(&pool));

        Self {
            produce_ai_key_validator,
            jwt_validator: None,
            user_service: Some(user_service),
        }
    }

    /// 设置 JWT 验证器
    pub fn set_jwt_validator(&mut self, jwt_validator: JwtValidator) {
        self.jwt_validator = Some(jwt_validator);
    }

    /// 获取 JWT 验证器
    pub fn get_jwt_validator(&self) -> Option<&JwtValidator> {
        self.jwt_validator.as_ref()
    }

    /// 验证 Produce AI Key
    pub async fn verify_api_key(&self, key: &str) -> Result<AuthContext> {
        self.produce_ai_key_validator.validate(key).await
    }

    /// 验证 JWT Token（如果配置了 JWT）
    pub fn verify_jwt(&self, token: &str) -> Result<AuthContext> {
        match &self.jwt_validator {
            Some(validator) => validator.validate(token),
            None => Err(KeyComputeError::AuthError(
                "JWT validation not configured".into(),
            )),
        }
    }

    /// 验证 Token（自动检测是 Produce AI Key 还是 JWT）
    pub async fn verify_token(&self, token: &str) -> Result<AuthContext> {
        // Produce AI Key 格式: sk-xxxx
        if token.starts_with("sk-") {
            return self.verify_api_key(token).await;
        }

        // JWT 结构性校验（签名、过期、issuer）
        let ctx = self.verify_jwt(token)?;

        // token_version 失效校验：
        // 与数据库中用户当前的 token_version 比对，密码重置/登出后旧 token 将被拒绝。
        // 仅在配置了 UserService（含数据库连接）时执行；无连接时保持结构性校验行为。
        if let Some(user_service) = &self.user_service {
            validate_user_token_version(&ctx, user_service.load_token_version(ctx.user_id).await?)?;
        }

        Ok(ctx)
    }

    /// 加载用户详细信息
    pub async fn load_user_details(&self, ctx: &mut AuthContext) -> Result<()> {
        if let Some(user_service) = &self.user_service {
            let user_info = user_service.load_user(ctx.user_id).await?;
            ctx.user_info = Some(user_info);
        }
        Ok(())
    }

    /// 加载租户详细信息
    pub async fn load_tenant_details(&self, ctx: &mut AuthContext) -> Result<()> {
        if let Some(user_service) = &self.user_service {
            let tenant_info = user_service.load_tenant(ctx.tenant_id).await?;
            ctx.tenant_info = Some(tenant_info);
        }
        Ok(())
    }

    /// 加载用户和租户详细信息
    pub async fn load_full_context(&self, ctx: &mut AuthContext) -> Result<()> {
        self.load_user_details(ctx).await?;
        self.load_tenant_details(ctx).await?;
        Ok(())
    }

    /// 验证 API Key 并加载完整上下文
    pub async fn verify_api_key_with_context(&self, key: &str) -> Result<AuthContext> {
        let mut ctx = self.verify_api_key(key).await?;
        self.load_full_context(&mut ctx).await?;
        Ok(ctx)
    }

    /// 验证用户是否属于指定租户
    pub async fn validate_user_tenant(
        &self,
        user_id: Uuid,
        expected_tenant_id: Uuid,
    ) -> Result<()> {
        if let Some(user_service) = &self.user_service {
            user_service
                .load_user_with_tenant_validation(user_id, expected_tenant_id)
                .await?;
        }
        Ok(())
    }

    /// 检查租户是否激活
    pub async fn is_tenant_active(&self, tenant_id: Uuid) -> Result<bool> {
        if let Some(user_service) = &self.user_service {
            let tenant = user_service.load_tenant(tenant_id).await?;
            return Ok(tenant.is_active());
        }
        // 无数据库连接时默认返回 true
        Ok(true)
    }

    /// 检查是否已配置数据库连接
    ///
    /// 用于启动时验证配置
    pub fn has_pool(&self) -> bool {
        self.produce_ai_key_validator.has_pool()
    }
}

fn validate_user_token_version(ctx: &AuthContext, current_version: Option<i32>) -> Result<()> {
    // None means UserService has no database connection. A missing database
    // subject is already returned as AuthError by load_token_version(), so keep
    // the documented structural-validation behavior for this case.
    let Some(current_version) = current_version else {
        return Ok(());
    };
    if current_version != ctx.token_version {
        tracing::warn!(
            user_id = %ctx.user_id,
            token_version = ctx.token_version,
            current_version,
            "JWT token_version mismatch; token has been invalidated"
        );
        return Err(KeyComputeError::AuthError(
            "Token has been invalidated".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_context() {
        let ctx = AuthContext::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "user");

        assert!(!ctx.is_admin());
        assert!(!ctx.has_permission(&Permission::ManageUsers));
    }

    /// 测试 API Key 认证的 admin 用户不会有管理权限
    /// API Key 仅用于 LLM 转发，即使角色是 admin，也只有 UseApi 权限
    #[test]
    fn test_auth_context_api_key_admin_no_admin_permissions() {
        // 模拟 API Key 认证场景：admin 角色但只有 UseApi 权限
        let ctx = AuthContext::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "admin")
            .with_permissions(build_permissions(AuthType::ApiKey, "admin"));

        // 角色检查仍然返回 true
        assert!(ctx.is_admin());
        // 但权限检查只基于权限列表
        assert!(ctx.has_permission(&Permission::UseApi));
        assert!(!ctx.has_permission(&Permission::ManageUsers));
        assert!(!ctx.has_permission(&Permission::SystemAdmin));
        assert!(!ctx.has_permission(&Permission::ManageBilling));
    }

    #[test]
    fn test_auth_context_admin() {
        // 测试 admin 角色的权限检查
        // 注意：权限检查完全基于权限列表，而非角色
        // admin 权限应该通过 build_permissions(AuthType::Jwt, "admin") 构建
        let ctx = AuthContext::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "admin")
            .with_permissions(build_permissions(AuthType::Jwt, "admin"));

        assert!(ctx.is_admin());
        assert!(ctx.has_permission(&Permission::ManageUsers));
        assert!(ctx.has_permission(&Permission::SystemAdmin));
    }

    #[test]
    fn test_auth_context_with_permissions() {
        let ctx = AuthContext::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "user")
            .with_permissions(vec![Permission::UseApi, Permission::ViewUsage]);

        assert!(ctx.has_permission(&Permission::UseApi));
        assert!(ctx.has_permission(&Permission::ViewUsage));
        assert!(!ctx.has_permission(&Permission::ManageUsers));
    }

    #[tokio::test]
    async fn test_auth_service_verify_api_key_requires_database() {
        // 无数据库连接时，验证应该失败（安全默认行为）
        let auth_service = AuthService::new(ProduceAiKeyValidator::default());
        let key = ProduceAiKeyValidator::generate_key();
        let result = auth_service.verify_api_key(&key).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not properly configured"));
    }

    #[tokio::test]
    async fn test_auth_service_verify_token_api_key_requires_database() {
        // 无数据库连接时，验证应该失败（安全默认行为）
        let auth_service = AuthService::new(ProduceAiKeyValidator::default());
        let key = ProduceAiKeyValidator::generate_key();
        let result = auth_service.verify_token(&key).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not properly configured"));
    }

    #[test]
    fn test_auth_service_verify_jwt() {
        let jwt_validator = JwtValidator::new("secret", "keycompute");
        let auth_service =
            AuthService::new(ProduceAiKeyValidator::default()).with_jwt(jwt_validator);

        let user_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4();
        let token = auth_service
            .jwt_validator
            .as_ref()
            .unwrap()
            .generate_token(user_id, tenant_id, "user")
            .unwrap();

        let result = auth_service.verify_jwt(&token);
        assert!(result.is_ok());
    }

    #[test]
    fn token_version_validation_skips_when_database_is_unavailable() {
        let ctx = AuthContext::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::nil(), "admin");
        assert!(validate_user_token_version(&ctx, None).is_ok());
    }

    #[tokio::test]
    async fn jwt_with_database_less_user_service_keeps_structural_validation() {
        let jwt_validator = JwtValidator::new("secret", "keycompute");
        let token = jwt_validator
            .generate_token(Uuid::new_v4(), Uuid::new_v4(), "user")
            .unwrap();
        let auth_service = AuthService::new(ProduceAiKeyValidator::default())
            .with_jwt(jwt_validator)
            .with_user_service(UserService::new());

        assert!(auth_service.verify_token(&token).await.is_ok());
    }

    #[test]
    fn token_version_validation_requires_current_version() {
        let mut ctx = AuthContext::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::nil(), "user");
        ctx.token_version = 4;
        assert!(validate_user_token_version(&ctx, Some(4)).is_ok());
        assert!(validate_user_token_version(&ctx, Some(3)).is_err());
    }
}
