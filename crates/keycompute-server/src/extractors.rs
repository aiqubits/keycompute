//! 提取器
//!
//! 自定义 Axum 提取器，用于从请求中提取认证信息等

use crate::{
    error::{ApiError, Result},
    state::AppState,
};
use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, request::Parts},
};
use keycompute_auth::{AuthContext, Permission};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::Arc;
use uuid::Uuid;

/// 认证提取器
///
/// 从请求头中提取 API Key 并解析用户信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthExtractor {
    /// 用户 ID
    pub user_id: Uuid,
    /// 租户 ID
    pub tenant_id: Uuid,
    /// Produce AI Key ID
    pub produce_ai_key_id: Uuid,
    /// 用户角色
    pub role: String,
    /// 用户权限列表
    pub permissions: Vec<Permission>,
}

impl AuthExtractor {
    /// 创建新的认证提取器（用于测试）
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
        }
    }

    /// 创建带权限的认证提取器（用于测试）
    pub fn with_permissions(mut self, permissions: Vec<Permission>) -> Self {
        self.permissions = permissions;
        self
    }

    /// 从 Authorization 头和 AuthService 解析
    pub async fn from_header_with_auth(
        headers: &HeaderMap,
        auth_service: &keycompute_auth::AuthService,
    ) -> Result<Self> {
        let auth_header = headers
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| ApiError::Auth("Missing Authorization header".to_string()))?;

        // 解析 Bearer token
        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| ApiError::Auth("Invalid Authorization format".to_string()))?;

        // 使用 AuthService 验证 Token（自动检测 JWT 或 API Key）
        // 注意：通过 From 转换而非手动拼接前缀，避免与 KeyComputeError 的
        // Display 前缀（"authentication failed: "）叠加产生重复文案
        let auth_context = auth_service
            .verify_token(token)
            .await
            .map_err(ApiError::from)?;

        Ok(Self::from_auth_context(auth_context))
    }

    /// 从 AuthContext 创建
    pub fn from_auth_context(ctx: AuthContext) -> Self {
        Self {
            user_id: ctx.user_id,
            tenant_id: ctx.tenant_id,
            produce_ai_key_id: ctx.produce_ai_key_id,
            role: ctx.role,
            permissions: ctx.permissions,
        }
    }

    /// 检查是否是管理员
    pub fn is_admin(&self) -> bool {
        self.role == "admin" || self.role == "system"
    }
}

impl FromRequestParts<AppState> for AuthExtractor {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = std::result::Result<Self, Self::Rejection>> + Send {
        let auth_service = Arc::clone(&state.auth);
        let headers = parts.headers.clone();

        async move { Self::from_header_with_auth(&headers, &auth_service).await }
    }
}

/// 请求 ID 提取器
#[derive(Debug, Clone)]
pub struct RequestId(pub Uuid);

impl RequestId {
    /// 创建新的请求 ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> FromRequestParts<S> for RequestId
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        // 尝试从 X-Request-ID 头获取，否则生成新的
        let id = parts
            .headers
            .get("X-Request-ID")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::new_v4);

        Ok(Self(id))
    }
}

/// 节点会话认证提取器
///
/// 从 Authorization header 中提取 session token 并验证
/// 认证成功后返回 node_id 和 session_id
pub struct NodeSessionAuth {
    /// 节点 ID
    pub node_id: Uuid,
    /// 会话 ID
    pub session_id: Uuid,
}

impl FromRequestParts<AppState> for NodeSessionAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        // 1. 从 Authorization header 提取 token
        let auth_header = parts
            .headers
            .get("Authorization")
            .ok_or_else(|| ApiError::Auth("Missing authorization header".to_string()))?;

        let token = auth_header
            .to_str()
            .map_err(|_| ApiError::Auth("Invalid authorization header".to_string()))?
            .strip_prefix("Bearer ")
            .ok_or_else(|| ApiError::Auth("Invalid bearer token".to_string()))?;

        // 2. 计算 token hash (SHA-256)
        let token_hash = compute_sha256_hash(token);

        // 3. 从 state 获取 pool
        let pool = state
            .pool
            .as_deref()
            .ok_or_else(|| ApiError::Internal("Database pool not configured".to_string()))?;

        // 4. 查询 node_sessions, 匹配 session_token_hash
        let row = pool.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT node_id, id FROM node_sessions WHERE session_token_hash = $1 AND revoked_at IS NULL",
            [token_hash.as_str().into()],
        ))
        .await
        .map_err(|e| ApiError::Internal(format!("Database query failed: {}", e)))?
        .ok_or_else(|| ApiError::Auth("Invalid session token".to_string()))?;

        let node_id: Uuid = row
            .try_get_by_index(0)
            .map_err(|e| ApiError::Internal(format!("Failed to parse node_id: {}", e)))?;
        let session_id: Uuid = row
            .try_get_by_index(1)
            .map_err(|e| ApiError::Internal(format!("Failed to parse session_id: {}", e)))?;

        Ok(NodeSessionAuth {
            node_id,
            session_id,
        })
    }
}

/// 计算 SHA-256 hash
fn compute_sha256_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[tokio::test]
    async fn test_auth_extractor_from_header_valid_format() {
        // 测试格式正确的 API Key（无数据库连接时会失败）
        // 这是预期行为：生产环境需要数据库连接
        let mut headers = HeaderMap::new();
        let api_key = keycompute_auth::ProduceAiKeyValidator::generate_key();
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", api_key)).unwrap(),
        );

        let auth_service =
            keycompute_auth::AuthService::new(keycompute_auth::ProduceAiKeyValidator::default());
        let result = AuthExtractor::from_header_with_auth(&headers, &auth_service).await;

        // 无数据库连接时应该返回配置错误
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiError::Auth(_)));
        assert!(err.to_string().contains("not properly configured"));
    }

    #[test]
    fn test_api_key_format_validation() {
        // 测试 API Key 格式验证
        let api_key = keycompute_auth::ProduceAiKeyValidator::generate_key();
        assert!(keycompute_auth::ProduceAiKeyValidator::is_valid_format(
            &api_key
        ));

        // 测试带前缀的格式
        let prefixed_key = keycompute_auth::ProduceAiKeyValidator::generate_key_with_prefix("proj");
        assert!(keycompute_auth::ProduceAiKeyValidator::is_valid_format(
            &prefixed_key
        ));

        // 测试无效格式
        assert!(!keycompute_auth::ProduceAiKeyValidator::is_valid_format(
            "invalid-key"
        ));
        assert!(!keycompute_auth::ProduceAiKeyValidator::is_valid_format(
            "sk-short"
        ));
    }

    #[tokio::test]
    async fn test_auth_extractor_from_header_missing() {
        let headers = HeaderMap::new();
        let auth_service =
            keycompute_auth::AuthService::new(keycompute_auth::ProduceAiKeyValidator::default());
        let result = AuthExtractor::from_header_with_auth(&headers, &auth_service).await;
        assert!(matches!(result, Err(ApiError::Auth(_))));
    }

    #[tokio::test]
    async fn test_auth_extractor_from_header_invalid_format() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );

        let auth_service =
            keycompute_auth::AuthService::new(keycompute_auth::ProduceAiKeyValidator::default());
        let result = AuthExtractor::from_header_with_auth(&headers, &auth_service).await;
        assert!(matches!(result, Err(ApiError::Auth(_))));
    }

    #[test]
    fn test_request_id_new() {
        let id = RequestId::new();
        assert_ne!(id.0, Uuid::nil());
    }
}
