//! 用户管理处理器
//!
//! 处理需要 Admin 权限的用户管理请求

use crate::{
    error::{ApiError, Result},
    extractors::AuthExtractor,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use keycompute_auth::Permission;
use keycompute_db::models::api_key::ProduceAiKey;
use keycompute_db::models::tenant::Tenant;
use keycompute_db::models::user::User;
use keycompute_db::models::user_credential::UserCredential;
use keycompute_types::{AssignableUserRole, UserRole};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ==================== 用户管理 ====================

/// 用户信息（Admin 视图）
#[derive(Debug, Serialize)]
pub struct AdminUserInfo {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub tenant_id: Uuid,
    pub tenant_name: String,
    /// 可用余额
    pub balance: f64,
    /// 冻结余额
    pub frozen_balance: f64,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
}

/// 用户列表查询参数
#[derive(Debug, Deserialize)]
pub struct UserListQueryParams {
    /// 租户 ID 过滤（可选）
    pub tenant_id: Option<Uuid>,
    /// 角色过滤（可选）
    pub role: Option<String>,
    /// 搜索关键词（邮箱或名称）
    pub search: Option<String>,
    /// 页码（从 1 开始）
    #[serde(default = "default_page")]
    pub page: i64,
    /// 每页数量
    #[serde(default = "default_page_size")]
    pub page_size: i64,
}

fn default_page() -> i64 {
    1
}

fn default_page_size() -> i64 {
    20
}

/// 一阶保护：禁止缺少受保护用户管理权限的调用方修改 system 用户。
///
/// 无论是改名称、改角色还是余额操作，未授权 caller 一律拒绝。
/// 此校验与 `validate_role_change_request` 互补：
/// - 本函数覆盖"是否能触碰 system 用户"的全局边界
/// - `validate_role_change_request` 覆盖"角色变更"的精细规则
fn validate_not_admin_modifying_system(auth: &AuthExtractor, target_user: &User) -> Result<()> {
    if !auth.has_permission(&Permission::ManageProtectedUsers)
        && target_user.role == UserRole::System.as_str()
    {
        return Err(ApiError::Forbidden(
            "Protected user management permission required".to_string(),
        ));
    }
    Ok(())
}

/// 二阶保护：校验角色变更请求的合法性。
///
/// 规则（按检查顺序）：
/// 1. 未请求变更角色 → 直接通过
/// 2. 仅有受保护用户管理权限的调用方可变更他人角色
/// 3. system 角色不能变更自己的角色
/// 4. system 角色的 role 字段不可被修改（即使 caller 是 system）
///
/// 调用约定：必须和 `validate_not_admin_modifying_system` 搭配使用，
/// 后者负责拦截未授权 caller 对 system 用户的任何修改。
fn validate_role_change_request(
    auth: &AuthExtractor,
    target_user_id: Uuid,
    target_user: &User,
    requested_role: &Option<AssignableUserRole>,
) -> Result<()> {
    if requested_role.is_none() {
        return Ok(());
    }

    if !auth.has_permission(&Permission::ManageProtectedUsers) {
        return Err(ApiError::Forbidden(
            "Protected user management permission required".to_string(),
        ));
    }

    if auth.user_id == target_user_id {
        return Err(ApiError::BadRequest(
            "System cannot modify its own role".to_string(),
        ));
    }

    if target_user.role == UserRole::System.as_str() {
        return Err(ApiError::BadRequest(
            "System role cannot be modified".to_string(),
        ));
    }

    Ok(())
}

fn validate_user_delete_request(
    auth: &AuthExtractor,
    target_user_id: Uuid,
    target_user: &User,
) -> Result<()> {
    if target_user_id == auth.user_id {
        return Err(ApiError::BadRequest("Cannot delete yourself".to_string()));
    }

    if target_user.role == UserRole::System.as_str() {
        return Err(ApiError::BadRequest(
            "System user cannot be deleted".to_string(),
        ));
    }

    if target_user.role == UserRole::Admin.as_str()
        && !auth.has_permission(&Permission::ManageProtectedUsers)
    {
        return Err(ApiError::Forbidden(
            "Protected user management permission required".to_string(),
        ));
    }

    Ok(())
}

/// 用户列表响应（带分页信息）
#[derive(Debug, Serialize)]
pub struct UserListResponse {
    pub users: Vec<AdminUserInfo>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub total_pages: i64,
}

/// 列出所有用户
///
/// GET /api/v1/users
///
/// 支持查询参数：
/// - tenant_id: 租户 ID 过滤
/// - role: 角色过滤
/// - search: 搜索关键词
/// - page: 页码（默认 1）
/// - page_size: 每页数量（默认 20）
///
/// Admin 可以查询所有租户的用户。
///
/// 过滤条件下推到 SQL 层以保证分页准确性。
pub async fn list_all_users(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Query(params): Query<UserListQueryParams>,
) -> Result<Json<UserListResponse>> {
    // 检查权限
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    // 计算分页偏移量
    let offset = (params.page - 1) * params.page_size;

    // 过滤条件下推到 SQL 层，保证分页准确性
    let tenant_id_filter = params.tenant_id;
    let role_filter = params.role.as_deref();
    let search_filter = params.search.as_deref();

    let users = User::find_all_filtered(
        pool,
        tenant_id_filter,
        role_filter,
        search_filter,
        params.page_size,
        offset,
    )
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to query users: {}", e)))?;

    // 统计过滤后的用户总数（同样下推到 SQL）
    let total = User::count_all_filtered(pool, tenant_id_filter, role_filter, search_filter)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to count users: {}", e)))?;

    // 预加载所有租户到 HashMap（避免 N+1 查询）
    let tenants = Tenant::find_all(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query tenants: {}", e)))?;
    let tenant_map: std::collections::HashMap<Uuid, String> =
        tenants.into_iter().map(|t| (t.id, t.name)).collect();

    // 批量预加载余额（避免 N+1 查询）
    let user_ids: Vec<Uuid> = users.iter().map(|u| u.id).collect();
    let balance_map = if let Some(bs) = state.billing.balance_service() {
        bs.find_by_users(&user_ids).await.ok().unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    // 批量预加载用户的最后登录时间（避免 N+1 查询）
    let credential_map: std::collections::HashMap<Uuid, UserCredential> =
        UserCredential::find_by_user_ids(pool, &user_ids)
            .await
            .unwrap_or_default();

    // 构建用户信息列表
    let result: Vec<AdminUserInfo> = users
        .into_iter()
        .map(|user| {
            let balance = balance_map.get(&user.id);
            let tenant_name = tenant_map
                .get(&user.tenant_id)
                .cloned()
                .unwrap_or_else(|| "Unknown".to_string());

            AdminUserInfo {
                id: user.id,
                email: user.email.clone(),
                name: user.name.clone(),
                role: user.role.clone(),
                tenant_id: user.tenant_id,
                tenant_name,
                balance: balance
                    .map(|b| b.available_balance.to_f64().unwrap_or(0.0))
                    .unwrap_or(0.0),
                frozen_balance: balance
                    .map(|b| b.frozen_balance.to_f64().unwrap_or(0.0))
                    .unwrap_or(0.0),
                created_at: user.created_at.to_rfc3339(),
                updated_at: user.updated_at.to_rfc3339(),
                last_login_at: credential_map
                    .get(&user.id)
                    .and_then(|c| c.last_login_at.map(|t| t.to_rfc3339())),
            }
        })
        .collect();

    // 基于过滤后的 total 计算总页数
    let total_pages = (total + params.page_size - 1) / params.page_size;

    Ok(Json(UserListResponse {
        users: result,
        total,
        page: params.page,
        page_size: params.page_size,
        total_pages,
    }))
}

/// 获取指定用户信息
///
/// GET /api/v1/users/{id}
pub async fn get_user_by_id(
    auth: AuthExtractor,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<AdminUserInfo>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let user = User::find_by_id(pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query user: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("User not found: {}", user_id)))?;

    // 获取租户名称
    let tenant = Tenant::find_by_id(pool, user.tenant_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query tenant: {}", e)))?;
    let tenant_name = tenant
        .map(|t| t.name)
        .unwrap_or_else(|| "Unknown".to_string());

    // 获取用户余额
    let balance = if let Some(bs) = state.billing.balance_service() {
        bs.find_by_user(user.id).await.ok().flatten()
    } else {
        None
    };

    // 获取用户最后登录时间
    let last_login = UserCredential::find_by_user_id(pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query credentials: {}", e)))?
        .and_then(|c| c.last_login_at.map(|t| t.to_rfc3339()));

    Ok(Json(AdminUserInfo {
        id: user.id,
        email: user.email,
        name: user.name,
        role: user.role,
        tenant_id: user.tenant_id,
        tenant_name,
        balance: balance
            .as_ref()
            .map(|b| b.available_balance.to_f64().unwrap_or(0.0))
            .unwrap_or(0.0),
        frozen_balance: balance
            .as_ref()
            .map(|b| b.frozen_balance.to_f64().unwrap_or(0.0))
            .unwrap_or(0.0),
        created_at: user.created_at.to_rfc3339(),
        updated_at: user.updated_at.to_rfc3339(),
        last_login_at: last_login,
    }))
}

/// 更新用户请求
#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub role: Option<AssignableUserRole>,
}

/// 更新用户信息
///
/// PUT /api/v1/users/{id}
pub async fn update_user(
    auth: AuthExtractor,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let user = User::find_by_id(pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to find user: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("User not found: {}", user_id)))?;

    // 禁止非 system 角色修改 system 用户（包括仅修改名称）
    validate_not_admin_modifying_system(&auth, &user)?;
    validate_role_change_request(&auth, user_id, &user, &req.role)?;

    let update_req = keycompute_db::models::user::UpdateUserRequest {
        name: req.name,
        role: req.role,
    };

    let updated = user
        .update(pool, &update_req)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update user: {}", e)))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "User updated",
        "user_id": updated.id,
        "email": updated.email,
        "name": updated.name,
        "role": updated.role,
    })))
}

/// 删除用户
///
/// DELETE /api/v1/users/{id}
pub async fn delete_user(
    auth: AuthExtractor,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let user = User::find_by_id(pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to find user: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("User not found: {}", user_id)))?;

    validate_user_delete_request(&auth, user_id, &user)?;

    user.delete(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete user: {}", e)))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "User deleted",
        "user_id": user_id,
        "deleted_by": auth.user_id,
    })))
}

/// 更新用户余额请求
#[derive(Debug, Deserialize)]
pub struct UpdateBalanceRequest {
    pub amount: String, // 使用字符串避免浮点精度问题
    pub reason: String,
}

/// 余额操作的公共上下文
struct BalanceOpContext {
    pub amount: Decimal,
    pub reason: String,
}

/// 余额操作公共前置校验，返回已校验的上下文
///
/// 统一处理：权限检查、用户查询、system 保护、金额解析、原因校验
async fn validate_balance_request(
    auth: &AuthExtractor,
    state: &AppState,
    user_id: Uuid,
    req: &UpdateBalanceRequest,
    require_positive: bool,
) -> Result<BalanceOpContext> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    // 禁止非 system 角色修改 system 用户的余额
    let target_user = User::find_by_id(pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to find user: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("User not found: {}", user_id)))?;
    validate_not_admin_modifying_system(auth, &target_user)?;

    // 解析金额
    let amount: Decimal = req
        .amount
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid amount format".to_string()))?;

    // 校验精度：最多两位小数（防止绕过前端限制传入过高精度金额）
    if amount != amount.round_dp(2) {
        return Err(ApiError::BadRequest(
            "Amount must have at most 2 decimal places".to_string(),
        ));
    }

    // 金额校验
    if require_positive && amount <= Decimal::ZERO {
        return Err(ApiError::BadRequest("Amount must be positive".to_string()));
    }
    if !require_positive && amount == Decimal::ZERO {
        return Err(ApiError::BadRequest("Amount cannot be zero".to_string()));
    }

    if req.reason.trim().is_empty() {
        return Err(ApiError::BadRequest("Reason is required".to_string()));
    }

    Ok(BalanceOpContext {
        amount,
        reason: req.reason.clone(),
    })
}

/// 更新用户余额
///
/// POST /api/v1/users/{id}/balance
pub async fn update_user_balance(
    auth: AuthExtractor,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<UpdateBalanceRequest>,
) -> Result<Json<serde_json::Value>> {
    let ctx = validate_balance_request(&auth, &state, user_id, &req, false).await?;

    let balance_service = state
        .billing
        .balance_service()
        .ok_or_else(|| ApiError::Internal("Balance service not configured".to_string()))?;

    // 更新余额
    // 注意：余额检查由 BalanceService 内部通过 FOR UPDATE 锁保证原子性
    // 不在此处预检查，避免 TOCTOU 竞争条件
    let (updated_balance, transaction) = if ctx.amount > Decimal::ZERO {
        balance_service
            .recharge(user_id, auth.tenant_id, ctx.amount, None, Some(&ctx.reason))
            .await
            .map_err(ApiError::from)?
    } else {
        // 负数金额视为消费
        balance_service
            .consume(user_id, -ctx.amount, None, Some(&ctx.reason))
            .await
            .map_err(ApiError::from)?
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Balance updated",
        "user_id": user_id,
        "amount": ctx.amount.to_string(),
        "reason": ctx.reason,
        "available_balance_before": transaction.balance_before.to_string(),
        "new_balance": updated_balance.available_balance.to_string(),
        "updated_by": auth.user_id,
    })))
}

/// 冻结用户余额
///
/// POST /api/v1/users/{id}/balance/freeze
pub async fn freeze_user_balance(
    auth: AuthExtractor,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<UpdateBalanceRequest>,
) -> Result<Json<serde_json::Value>> {
    let ctx = validate_balance_request(&auth, &state, user_id, &req, true).await?;

    let balance_service = state
        .billing
        .balance_service()
        .ok_or_else(|| ApiError::Internal("Balance service not configured".to_string()))?;

    let (updated_balance, transaction) = balance_service
        .freeze(user_id, ctx.amount, Some(&ctx.reason))
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Balance frozen",
        "user_id": user_id,
        "amount": ctx.amount.to_string(),
        "reason": ctx.reason,
        "available_balance_before": transaction.balance_before.to_string(),
        "new_available_balance": updated_balance.available_balance.to_string(),
        "new_frozen_balance": updated_balance.frozen_balance.to_string(),
        "updated_by": auth.user_id,
    })))
}

/// 解冻用户余额
///
/// POST /api/v1/users/{id}/balance/unfreeze
pub async fn unfreeze_user_balance(
    auth: AuthExtractor,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<UpdateBalanceRequest>,
) -> Result<Json<serde_json::Value>> {
    let ctx = validate_balance_request(&auth, &state, user_id, &req, true).await?;

    let balance_service = state
        .billing
        .balance_service()
        .ok_or_else(|| ApiError::Internal("Balance service not configured".to_string()))?;

    let (updated_balance, transaction) = balance_service
        .unfreeze(user_id, ctx.amount, Some(&ctx.reason))
        .await
        .map_err(ApiError::from)?;

    // transaction.balance_before 跟踪的是操作前可用余额（available_balance），
    // 不含冻结余额信息。由于 FOR UPDATE 行锁保证并发安全，可通过更新后余额反推：
    // frozen_before = updated.frozen_balance + amount（因为解冻操作: frozen -= amount）
    let frozen_before = updated_balance.frozen_balance + ctx.amount;
    let _ = transaction; // balance_before 在此场景无直接用途，明确忽略

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Balance unfrozen",
        "user_id": user_id,
        "amount": ctx.amount.to_string(),
        "reason": ctx.reason,
        "frozen_balance_before": frozen_before.to_string(),
        "new_available_balance": updated_balance.available_balance.to_string(),
        "new_frozen_balance": updated_balance.frozen_balance.to_string(),
        "updated_by": auth.user_id,
    })))
}

/// 列出用户的所有 API Keys（Admin 视图）
///
/// GET /api/v1/users/{id}/api-keys
pub async fn list_all_api_keys(
    auth: AuthExtractor,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let keys = ProduceAiKey::find_by_user(pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch API keys: {}", e)))?;

    let result: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|k| {
            serde_json::json!({
                "id": k.id,
                "user_id": k.user_id,
                "name": k.name,
                "key_preview": k.produce_ai_key_preview,
                "is_active": !k.revoked,
                "revoked": k.revoked,
                "revoked_at": k.revoked_at.map(|t| t.to_rfc3339()),
                "created_at": k.created_at.to_rfc3339(),
                "last_used_at": k.last_used_at.map(|t| t.to_rfc3339()),
            })
        })
        .collect();

    Ok(Json(result))
}

// ==================== 租户管理 ====================

/// 租户信息
#[derive(Debug, Serialize)]
pub struct TenantInfo {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub user_count: i64,
    pub is_active: bool,
    pub created_at: String,
}

/// 列出所有租户
///
/// GET /api/v1/tenants
pub async fn list_tenants(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<Vec<TenantInfo>>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let tenants = Tenant::find_all(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query tenants: {}", e)))?;

    // 批量统计各租户用户数量（避免 N+1 查询）
    let tenant_ids: Vec<Uuid> = tenants.iter().map(|t| t.id).collect();
    let user_counts = User::count_by_tenants(pool, &tenant_ids)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to count users: {}", e)))?;

    let result: Vec<TenantInfo> = tenants
        .into_iter()
        .map(|tenant| {
            let is_active = tenant.is_active();
            let description = tenant.description.clone();

            TenantInfo {
                id: tenant.id,
                name: tenant.name,
                description,
                user_count: user_counts.get(&tenant.id).copied().unwrap_or(0),
                is_active,
                created_at: tenant.created_at.to_rfc3339(),
            }
        })
        .collect();

    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_user_info_serialization() {
        let user = AdminUserInfo {
            id: Uuid::new_v4(),
            email: "admin@example.com".to_string(),
            name: Some("Admin".to_string()),
            role: "admin".to_string(),
            tenant_id: Uuid::new_v4(),
            tenant_name: "Test".to_string(),
            balance: 1000.0,
            frozen_balance: 0.0,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            last_login_at: None,
        };

        let json = serde_json::to_string(&user).unwrap();
        assert!(json.contains("admin@example.com"));
    }

    fn make_test_user(id: Uuid, role: &str) -> User {
        use chrono::Utc;

        User {
            id,
            tenant_id: Uuid::new_v4(),
            email: "target@example.com".to_string(),
            name: Some("Target".to_string()),
            role: role.to_string(),
            token_version: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_validate_role_change_request_requires_protected_user_permission() {
        let auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "admin");
        let target = make_test_user(Uuid::new_v4(), "user");
        let err = validate_role_change_request(
            &auth,
            target.id,
            &target,
            &Some(AssignableUserRole::Admin),
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(msg) if msg.contains("permission required")));
    }

    #[test]
    fn test_validate_role_change_request_rejects_self_role_change() {
        let user_id = Uuid::new_v4();
        let auth = AuthExtractor::new(user_id, Uuid::new_v4(), Uuid::new_v4(), "system")
            .with_permissions(vec![Permission::ManageProtectedUsers]);
        let target = make_test_user(user_id, "system");
        let err = validate_role_change_request(
            &auth,
            target.id,
            &target,
            &Some(AssignableUserRole::Admin),
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("own role")));
    }

    #[test]
    fn test_validate_role_change_request_rejects_modifying_system_role() {
        let auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "system")
            .with_permissions(vec![Permission::ManageProtectedUsers]);
        let target = make_test_user(Uuid::new_v4(), "system");
        let err = validate_role_change_request(
            &auth,
            target.id,
            &target,
            &Some(AssignableUserRole::Admin),
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("cannot be modified")));
    }

    #[test]
    fn test_validate_user_delete_request_rejects_self_delete() {
        let user_id = Uuid::new_v4();
        let auth = AuthExtractor::new(user_id, Uuid::new_v4(), Uuid::new_v4(), "admin");
        let target = make_test_user(user_id, "admin");
        let err = validate_user_delete_request(&auth, target.id, &target).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("yourself")));
    }

    #[test]
    fn test_validate_user_delete_request_rejects_system_user() {
        let auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "admin");
        let target = make_test_user(Uuid::new_v4(), "system");
        let err = validate_user_delete_request(&auth, target.id, &target).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(msg) if msg.contains("cannot be deleted")));
    }

    #[test]
    fn test_validate_user_delete_request_requires_protected_user_permission_for_admin_target() {
        let auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "admin");
        let target = make_test_user(Uuid::new_v4(), "admin");
        let err = validate_user_delete_request(&auth, target.id, &target).unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(msg) if msg.contains("permission required")));
    }

    #[test]
    fn test_validate_user_delete_request_allows_system_to_delete_admin_target() {
        let auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "user")
            .with_permissions(vec![Permission::ManageProtectedUsers]);
        let target = make_test_user(Uuid::new_v4(), "admin");
        assert!(validate_user_delete_request(&auth, target.id, &target).is_ok());
    }
}
