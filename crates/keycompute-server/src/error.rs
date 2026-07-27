//! 错误处理
//!
//! 定义 API 错误类型和响应格式

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::fmt;

/// API 错误类型
#[derive(Debug)]
pub enum ApiError {
    /// 认证错误
    Auth(String),
    /// 限流错误
    RateLimit(String),
    /// 路由错误
    Routing(String),
    /// Provider 错误
    Provider(String),
    /// 配置错误
    Config(String),
    /// 验证流程错误
    Verification(String),
    /// 服务暂时不可用
    ServiceUnavailable(String),
    /// 内部错误
    Internal(String),
    /// 请求参数错误
    BadRequest(String),
    /// 资源未找到
    NotFound(String),
    /// 权限拒绝
    Forbidden(String),
    /// 节点身份不匹配
    NodeIdentityMismatch {
        expected_node_id: uuid::Uuid,
        expected_session_id: uuid::Uuid,
        actual_node_id: uuid::Uuid,
        actual_session_id: uuid::Uuid,
    },
    /// 节点任务提交冲突（409）
    NodeTaskConflict(String),
    /// 通用冲突错误（409），如并发操作冲突
    Conflict(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Auth(msg) => write!(f, "Authentication error: {}", msg),
            ApiError::RateLimit(msg) => write!(f, "Rate limit error: {}", msg),
            ApiError::Routing(msg) => write!(f, "Routing error: {}", msg),
            ApiError::Provider(msg) => write!(f, "Provider error: {}", msg),
            ApiError::Config(msg) => write!(f, "Config error: {}", msg),
            ApiError::Verification(msg) => write!(f, "Verification error: {}", msg),
            ApiError::ServiceUnavailable(msg) => write!(f, "Service unavailable: {}", msg),
            ApiError::Internal(msg) => write!(f, "Internal error: {}", msg),
            ApiError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            ApiError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ApiError::Forbidden(msg) => write!(f, "Forbidden: {}", msg),
            ApiError::NodeIdentityMismatch { .. } => write!(f, "Node identity mismatch"),
            ApiError::NodeTaskConflict(msg) => write!(f, "Node task conflict: {}", msg),
            ApiError::Conflict(msg) => write!(f, "Conflict: {}", msg),
        }
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            ApiError::Auth(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::RateLimit(msg) => (StatusCode::TOO_MANY_REQUESTS, msg.clone()),
            ApiError::Routing(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.clone()),
            ApiError::Provider(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            ApiError::Config(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ApiError::Verification(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg.clone()),
            ApiError::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.clone()),
            ApiError::Internal(msg) => {
                tracing::error!(error = %msg, "Internal API error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            ApiError::NodeIdentityMismatch { .. } => (
                StatusCode::UNAUTHORIZED,
                "Node identity mismatch: node_id or session_id does not match authenticated session"
                    .to_string(),
            ),
            ApiError::NodeTaskConflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
        };

        let body = Json(json!({
            "error": {
                "message": error_message,
                "type": error_type(&self),
                "code": status.as_u16(),
            }
        }));

        (status, body).into_response()
    }
}

fn error_type(error: &ApiError) -> &'static str {
    match error {
        ApiError::Auth(_) => "authentication_error",
        ApiError::RateLimit(_) => "rate_limit_error",
        ApiError::Routing(_) => "routing_error",
        ApiError::Provider(_) => "provider_error",
        ApiError::Config(_) => "config_error",
        ApiError::Verification(_) => "verification_error",
        ApiError::ServiceUnavailable(_) => "service_unavailable_error",
        ApiError::Internal(_) => "internal_error",
        ApiError::BadRequest(_) => "bad_request_error",
        ApiError::NotFound(_) => "not_found_error",
        ApiError::Forbidden(_) => "forbidden_error",
        ApiError::NodeIdentityMismatch { .. } => "node_identity_mismatch_error",
        ApiError::NodeTaskConflict(_) => "node_task_conflict_error",
        ApiError::Conflict(_) => "conflict_error",
    }
}

/// API 结果类型
pub type Result<T> = std::result::Result<T, ApiError>;

/// 从 anyhow 错误转换
impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::Internal(err.to_string())
    }
}

/// 从 sea-orm 错误转换
impl From<sea_orm::DbErr> for ApiError {
    fn from(err: sea_orm::DbErr) -> Self {
        ApiError::Internal(err.to_string())
    }
}

/// 从数据库错误转换
impl From<keycompute_db::DbError> for ApiError {
    fn from(err: keycompute_db::DbError) -> Self {
        match err {
            keycompute_db::DbError::NotFound { entity, id } => {
                ApiError::NotFound(format!("{} not found: {}", entity, id))
            }
            keycompute_db::DbError::DatabaseError(_) => ApiError::Internal(err.to_string()),
            keycompute_db::DbError::Other(msg) => {
                // 节点任务提交相关的冲突错误应该返回 409
                match msg.as_str() {
                    "duplicate_submission_session_mismatch"
                    | "duplicate_submission_conflict"
                    | "grace_period_expired"
                    | "invalid_task_state"
                    | "lease_mismatch"
                    | "task_expired_during_complete" => ApiError::NodeTaskConflict(msg),
                    _ => ApiError::Internal(msg),
                }
            }
            _ => ApiError::Internal(err.to_string()),
        }
    }
}

/// 从 node-gateway 执行错误映射: client_error → 400, 其他 → 500 (B2 修复)
impl From<node_gateway::NodeExecutionError> for ApiError {
    fn from(err: node_gateway::NodeExecutionError) -> Self {
        use node_gateway::NodeExecutionError as NE;
        match err {
            NE::ClientError { code, message } => {
                ApiError::BadRequest(format!("{}: {}", code, message))
            }
            NE::Other(e) => ApiError::Internal(e.to_string()),
        }
    }
}

/// 从 keycompute-types 错误转换
impl From<keycompute_types::KeyComputeError> for ApiError {
    fn from(err: keycompute_types::KeyComputeError) -> Self {
        use keycompute_types::KeyComputeError;
        match err {
            // 认证与授权
            KeyComputeError::AuthError(msg) => ApiError::Auth(msg),
            KeyComputeError::PermissionDenied(msg) => ApiError::Forbidden(msg),
            KeyComputeError::VerificationError(msg) => ApiError::Verification(msg),

            // 限流
            KeyComputeError::RateLimitExceeded(msg) => ApiError::RateLimit(msg),

            // 路由
            KeyComputeError::RoutingFailed(msg) => ApiError::Routing(msg),
            KeyComputeError::NoReadyNode(msg) => ApiError::Routing(msg),

            // Provider
            KeyComputeError::ProviderError(msg) => ApiError::Provider(msg),
            KeyComputeError::ProviderTimeout(ms, msg) => {
                ApiError::Provider(format!("timeout after {}ms: {}", ms, msg))
            }

            // 数据库
            KeyComputeError::DatabaseError(msg) => ApiError::Internal(msg),

            // 配置
            KeyComputeError::ConfigError(msg) => ApiError::Config(msg),
            KeyComputeError::ServiceUnavailable(msg) => ApiError::ServiceUnavailable(msg),

            // 验证与请求
            KeyComputeError::ValidationError(msg) => ApiError::BadRequest(msg),
            KeyComputeError::InvalidRequest(msg) => ApiError::BadRequest(msg),

            // 未找到
            KeyComputeError::NotFound(msg) => ApiError::NotFound(msg),

            // 网络
            KeyComputeError::NetworkError(msg) => ApiError::Provider(msg),
            KeyComputeError::Timeout(msg) => {
                ApiError::Provider(format!("Request timeout: {}", msg))
            }

            // 内部错误
            KeyComputeError::Internal(msg) => ApiError::Internal(msg),
            KeyComputeError::SerializationError(msg) => ApiError::Internal(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_display() {
        let err = ApiError::Auth("Invalid key".to_string());
        assert!(err.to_string().contains("Authentication error"));
    }

    #[test]
    fn test_error_type() {
        assert_eq!(
            error_type(&ApiError::Auth("test".to_string())),
            "authentication_error"
        );
        assert_eq!(
            error_type(&ApiError::RateLimit("test".to_string())),
            "rate_limit_error"
        );
    }

    #[tokio::test]
    async fn internal_error_response_hides_details() {
        let response =
            ApiError::Internal("duplicate key violates constraint secret_table_key".to_string())
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
