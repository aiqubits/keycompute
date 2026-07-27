//! 权限检查
//!
//! 定义系统中的权限和权限检查逻辑。

use serde::{Deserialize, Serialize};

/// 认证类型
///
/// 用于区分不同的认证场景，决定权限构建策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    /// API Key 认证 - 仅用于 LLM Provider 转发
    ///
    /// API Key 不应具有任何系统管理权限，仅能调用 API
    ApiKey,
    /// JWT 认证 - 用于后台管理系统
    ///
    /// JWT 用于管理后台系统与管理模块，根据角色分配不同的管理权限
    Jwt,
}

/// 权限枚举
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    /// 使用 API
    UseApi,
    /// 查看用量
    ViewUsage,
    /// 管理 API Keys
    ManageApiKeys,
    /// 管理用户
    ManageUsers,
    /// 管理租户设置
    ManageTenant,
    /// 查看账单
    ViewBilling,
    /// 管理当前用户自己的账单与支付订单
    ManageOwnBilling,
    /// 管理账单
    ManageBilling,
    /// 管理定价
    ManagePricing,
    /// 管理 Provider 账号
    ManageProviders,
    /// 管理受保护的全局系统设置
    ManageSystemSettings,
    /// 管理 system 用户、角色及其他受保护用户边界
    ManageProtectedUsers,
    /// 系统管理员权限
    SystemAdmin,
}

impl Permission {
    /// 获取权限字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::UseApi => "api:use",
            Permission::ViewUsage => "usage:view",
            Permission::ManageApiKeys => "api_keys:manage",
            Permission::ManageUsers => "users:manage",
            Permission::ManageTenant => "tenant:manage",
            Permission::ViewBilling => "billing:view",
            Permission::ManageOwnBilling => "billing:self_manage",
            Permission::ManageBilling => "billing:manage",
            Permission::ManagePricing => "pricing:manage",
            Permission::ManageProviders => "providers:manage",
            Permission::ManageSystemSettings => "system_settings:manage",
            Permission::ManageProtectedUsers => "protected_users:manage",
            Permission::SystemAdmin => "system:admin",
        }
    }

    /// 从字符串解析权限
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "api:use" => Some(Permission::UseApi),
            "usage:view" => Some(Permission::ViewUsage),
            "api_keys:manage" => Some(Permission::ManageApiKeys),
            "users:manage" => Some(Permission::ManageUsers),
            "tenant:manage" => Some(Permission::ManageTenant),
            "billing:view" => Some(Permission::ViewBilling),
            "billing:self_manage" => Some(Permission::ManageOwnBilling),
            "billing:manage" => Some(Permission::ManageBilling),
            "pricing:manage" => Some(Permission::ManagePricing),
            "providers:manage" => Some(Permission::ManageProviders),
            "system_settings:manage" => Some(Permission::ManageSystemSettings),
            "protected_users:manage" => Some(Permission::ManageProtectedUsers),
            "system:admin" => Some(Permission::SystemAdmin),
            _ => None,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Self::parse(s)
    }
}

/// 权限检查器
#[derive(Debug, Clone)]
pub struct PermissionChecker;

impl PermissionChecker {
    /// 检查用户是否有权限执行操作
    ///
    /// 权限检查完全基于权限列表，不基于角色。
    /// 这确保了 API Key 认证（仅有 UseApi 权限）无法访问管理功能，
    /// 即使该用户的角色是 admin。
    ///
    /// # Arguments
    /// * `user_role` - 用户角色（此参数已不用于权限判断，保留用于日志/审计）
    /// * `user_permissions` - 用户权限列表
    /// * `required` - 需要的权限
    pub fn check(user_role: &str, user_permissions: &[Permission], required: &Permission) -> bool {
        let _ = user_role; // 不再基于角色判断权限
        user_permissions.contains(required)
    }

    /// 检查是否需要租户隔离
    pub fn requires_tenant_isolation(permission: &Permission) -> bool {
        matches!(
            permission,
            Permission::UseApi
                | Permission::ViewUsage
                | Permission::ManageApiKeys
                | Permission::ViewBilling
                | Permission::ManageOwnBilling
        )
    }
}

/// 根据认证类型和角色构建权限列表
///
/// # Arguments
/// * `auth_type` - 认证类型（API Key 或 JWT）
/// * `role` - 用户角色
///
/// # Returns
/// 权限列表
///
/// # Examples
/// ```
/// use keycompute_auth::permission::{AuthType, build_permissions, Permission};
///
/// // API Key 认证仅有 UseApi 权限
/// let api_key_perms = build_permissions(AuthType::ApiKey, "admin");
/// assert_eq!(api_key_perms, vec![Permission::UseApi]);
///
/// // JWT 认证根据角色分配权限
/// let jwt_admin_perms = build_permissions(AuthType::Jwt, "admin");
/// assert!(jwt_admin_perms.contains(&Permission::SystemAdmin));
/// ```
pub fn build_permissions(auth_type: AuthType, role: &str) -> Vec<Permission> {
    match auth_type {
        AuthType::ApiKey => build_api_key_permissions(),
        AuthType::Jwt => build_jwt_permissions(role),
    }
}

/// 构建 API Key 认证权限
///
/// API Key 仅用于 LLM Provider 转发，不包含任何系统管理权限
fn build_api_key_permissions() -> Vec<Permission> {
    // API Key 仅有使用 API 的权限，用于转发请求到上游 LLM Provider
    // 不包含：用户管理、计量计费、模块管理、系统设置等任何管理权限
    vec![Permission::UseApi]
}

/// 构建 JWT 认证权限（后台管理权限）
///
/// 根据用户角色分配相应的后台管理权限
fn build_jwt_permissions(role: &str) -> Vec<Permission> {
    match role {
        "admin" => build_admin_permissions(),
        // system 账号额外可修改受保护的全局系统设置。
        "system" => {
            let mut permissions = build_admin_permissions();
            permissions.push(Permission::ManageSystemSettings);
            permissions.push(Permission::ManageProtectedUsers);
            permissions
        }
        // 普通用户：可查看用量并管理自己的账单与支付订单
        "user" => vec![
            Permission::UseApi,
            Permission::ViewUsage,
            Permission::ManageOwnBilling,
        ],
        // 未知角色：最小权限
        _ => vec![Permission::UseApi],
    }
}

fn build_admin_permissions() -> Vec<Permission> {
    vec![
        Permission::UseApi,
        Permission::ViewUsage,
        Permission::ManageApiKeys,
        Permission::ManageUsers,
        Permission::ManageTenant,
        Permission::ViewBilling,
        Permission::ManageOwnBilling,
        Permission::ManageBilling,
        Permission::ManagePricing,
        Permission::ManageProviders,
        Permission::SystemAdmin,
    ]
}

/// 预定义的角色权限（用于 JWT 认证场景）
///
/// 注意：此模块已废弃，请使用 `build_permissions(AuthType::Jwt, role)` 代替
#[deprecated(note = "请使用 build_permissions(AuthType::Jwt, role) 代替")]
pub mod roles {
    use super::Permission;

    /// 普通用户权限
    pub fn user() -> Vec<Permission> {
        vec![
            Permission::UseApi,
            Permission::ViewUsage,
            Permission::ManageOwnBilling,
        ]
    }

    /// 系统管理员权限
    pub fn system_admin() -> Vec<Permission> {
        vec![
            Permission::UseApi,
            Permission::ViewUsage,
            Permission::ManageApiKeys,
            Permission::ManageUsers,
            Permission::ManageTenant,
            Permission::ViewBilling,
            Permission::ManageOwnBilling,
            Permission::ManageBilling,
            Permission::ManagePricing,
            Permission::ManageProviders,
            Permission::ManageSystemSettings,
            Permission::ManageProtectedUsers,
            Permission::SystemAdmin,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_as_str() {
        assert_eq!(Permission::UseApi.as_str(), "api:use");
        assert_eq!(Permission::ManageOwnBilling.as_str(), "billing:self_manage");
        assert_eq!(
            Permission::ManageSystemSettings.as_str(),
            "system_settings:manage"
        );
        assert_eq!(
            Permission::ManageProtectedUsers.as_str(),
            "protected_users:manage"
        );
        assert_eq!(Permission::SystemAdmin.as_str(), "system:admin");
    }

    #[test]
    fn test_permission_from_str() {
        assert_eq!(Permission::parse("api:use"), Some(Permission::UseApi));
        assert_eq!(
            Permission::parse("billing:self_manage"),
            Some(Permission::ManageOwnBilling)
        );
        assert_eq!(
            Permission::parse("system_settings:manage"),
            Some(Permission::ManageSystemSettings)
        );
        assert_eq!(
            Permission::parse("protected_users:manage"),
            Some(Permission::ManageProtectedUsers)
        );
        assert_eq!(Permission::parse("invalid"), None);
    }

    #[test]
    fn test_permission_checker_admin() {
        // 权限检查完全基于权限列表，不基于角色
        // admin 角色如果没有 ManageUsers 权限，应该返回 false
        let perms = vec![Permission::UseApi];
        assert!(!PermissionChecker::check(
            "admin",
            &perms,
            &Permission::ManageUsers
        ));

        // admin 角色如果有 ManageUsers 权限，应该返回 true
        let admin_perms = build_permissions(AuthType::Jwt, "admin");
        assert!(PermissionChecker::check(
            "admin",
            &admin_perms,
            &Permission::ManageUsers
        ));
    }

    #[test]
    fn test_permission_checker_user() {
        let perms = vec![Permission::UseApi, Permission::ViewUsage];
        assert!(PermissionChecker::check(
            "user",
            &perms,
            &Permission::UseApi
        ));
        assert!(!PermissionChecker::check(
            "user",
            &perms,
            &Permission::ManageUsers
        ));
    }

    #[test]
    fn test_roles() {
        // 使用新的 build_permissions 函数测试
        let user_perms = build_permissions(AuthType::Jwt, "user");
        assert!(user_perms.contains(&Permission::UseApi));
        assert!(!user_perms.contains(&Permission::ManageUsers));

        let admin_perms = build_permissions(AuthType::Jwt, "admin");
        assert!(admin_perms.contains(&Permission::ManageApiKeys));
    }

    // ==================== 新增测试：权限构建函数 ====================

    #[test]
    fn test_build_permissions_api_key() {
        // API Key 认证 - 无论什么角色，都只有 UseApi 权限
        let admin_perms = build_permissions(AuthType::ApiKey, "admin");
        assert_eq!(admin_perms, vec![Permission::UseApi]);
        assert!(!admin_perms.contains(&Permission::ManageUsers));
        assert!(!admin_perms.contains(&Permission::SystemAdmin));

        let user_perms = build_permissions(AuthType::ApiKey, "user");
        assert_eq!(user_perms, vec![Permission::UseApi]);

        let system_perms = build_permissions(AuthType::ApiKey, "system");
        assert_eq!(system_perms, vec![Permission::UseApi]);
    }

    #[test]
    fn test_build_permissions_jwt_admin() {
        // JWT 认证 - admin 角色拥有所有权限
        let perms = build_permissions(AuthType::Jwt, "admin");
        assert!(perms.contains(&Permission::UseApi));
        assert!(perms.contains(&Permission::ViewUsage));
        assert!(perms.contains(&Permission::ManageUsers));
        assert!(perms.contains(&Permission::ManageBilling));
        assert!(perms.contains(&Permission::ManageOwnBilling));
        assert!(perms.contains(&Permission::ManagePricing));
        assert!(perms.contains(&Permission::ManageProviders));
        assert!(perms.contains(&Permission::SystemAdmin));
        assert!(!perms.contains(&Permission::ManageSystemSettings));
        assert!(!perms.contains(&Permission::ManageProtectedUsers));
    }

    #[test]
    fn test_build_permissions_jwt_system() {
        // JWT 认证 - system 角色与 admin 相同权限
        let perms = build_permissions(AuthType::Jwt, "system");
        assert!(perms.contains(&Permission::SystemAdmin));
        assert!(perms.contains(&Permission::ManageProviders));
        assert!(perms.contains(&Permission::ManageSystemSettings));
        assert!(perms.contains(&Permission::ManageProtectedUsers));
    }

    #[test]
    fn test_build_permissions_jwt_user() {
        // JWT 认证 - user 角色仅有基本权限
        let perms = build_permissions(AuthType::Jwt, "user");
        assert!(perms.contains(&Permission::UseApi));
        assert!(perms.contains(&Permission::ViewUsage));
        assert!(perms.contains(&Permission::ManageOwnBilling));
        assert!(!perms.contains(&Permission::ManageUsers));
        assert!(!perms.contains(&Permission::ViewBilling));
    }

    #[test]
    fn test_build_permissions_jwt_unknown_role() {
        // JWT 认证 - 未知角色仅有 UseApi
        let perms = build_permissions(AuthType::Jwt, "unknown");
        assert_eq!(perms, vec![Permission::UseApi]);
    }

    #[test]
    fn test_auth_type_equality() {
        assert_eq!(AuthType::ApiKey, AuthType::ApiKey);
        assert_eq!(AuthType::Jwt, AuthType::Jwt);
        assert_ne!(AuthType::ApiKey, AuthType::Jwt);
    }
}
