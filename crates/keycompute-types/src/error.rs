//! 统一错误类型体系
//!
//! 定义所有后端 crate 共享的错误类型，提供统一的错误处理接口。

use thiserror::Error;

/// KeyCompute 统一错误类型
///
/// 涵盖认证、路由、Provider、数据库、配置等所有错误场景。
/// 所有变体都包含描述性信息，便于日志记录和错误追踪。
#[derive(Error, Debug)]
pub enum KeyComputeError {
    // ============ 认证与授权 ============
    /// 认证失败（无效凭证、令牌过期等）
    #[error("authentication failed: {0}")]
    AuthError(String),

    /// 权限不足
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// 验证码/验证流程错误
    #[error("verification failed: {0}")]
    VerificationError(String),

    // ============ 限流 ============
    /// 限流触发
    #[error("rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    // ============ 路由 ============
    /// 路由失败，无可用 Provider
    #[error("routing failed: no available provider for model {0}")]
    RoutingFailed(String),

    /// 无可用 Node 节点
    #[error("no ready node available for model: {0}")]
    NoReadyNode(String),

    // ============ Provider ============
    /// 上游 Provider 错误
    #[error("upstream provider error: {0}")]
    ProviderError(String),

    /// Provider 超时
    #[error("provider timeout after {0}ms: {1}")]
    ProviderTimeout(u64, String),

    /// Structured upstream failure preserved across protocol and gateway layers.
    #[error("upstream failure {stable_code}: {summary}")]
    UpstreamFailure {
        status: Option<u16>,
        stable_code: String,
        retryable: bool,
        summary: String,
    },

    // ============ 数据库 ============
    /// 数据库操作错误
    #[error("database error: {0}")]
    DatabaseError(String),

    // ============ 配置 ============
    /// 配置错误
    #[error("configuration error: {0}")]
    ConfigError(String),

    /// 服务暂时不可用（依赖服务故障、临时降级等）
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    // ============ 请求处理 ============
    /// 内部错误（不应暴露给用户的系统错误）
    #[error("internal error: {0}")]
    Internal(String),

    /// 序列化/反序列化错误
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// 验证错误
    #[error("validation error: {0}")]
    ValidationError(String),

    /// 资源未找到
    #[error("not found: {0}")]
    NotFound(String),

    /// 请求参数错误
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    // ============ 网络 ============
    /// 网络连接错误
    #[error("network error: {0}")]
    NetworkError(String),

    /// 请求超时
    #[error("request timeout: {0}")]
    Timeout(String),
}

/// 统一 Result 类型别名
pub type Result<T> = std::result::Result<T, KeyComputeError>;

// ============ From 实现 ============

impl From<serde_json::Error> for KeyComputeError {
    fn from(err: serde_json::Error) -> Self {
        KeyComputeError::SerializationError(err.to_string())
    }
}

impl From<std::io::Error> for KeyComputeError {
    fn from(err: std::io::Error) -> Self {
        KeyComputeError::Internal(err.to_string())
    }
}

impl From<uuid::Error> for KeyComputeError {
    fn from(err: uuid::Error) -> Self {
        KeyComputeError::InvalidRequest(format!("Invalid UUID: {}", err))
    }
}

impl From<chrono::ParseError> for KeyComputeError {
    fn from(err: chrono::ParseError) -> Self {
        KeyComputeError::InvalidRequest(format!("Invalid datetime format: {}", err))
    }
}

// ============ 辅助方法 ============

impl KeyComputeError {
    /// 判断错误是否可重试
    ///
    /// 认证错误、验证错误、未找到错误不应重试
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            KeyComputeError::ProviderError(_)
                | KeyComputeError::ProviderTimeout(_, _)
                | KeyComputeError::NetworkError(_)
                | KeyComputeError::Timeout(_)
                | KeyComputeError::ServiceUnavailable(_)
                | KeyComputeError::DatabaseError(_)
        ) || matches!(
            self,
            KeyComputeError::UpstreamFailure {
                retryable: true,
                ..
            }
        )
    }

    /// 获取错误类别（用于 API 响应分类）
    pub fn category(&self) -> ErrorCategory {
        match self {
            KeyComputeError::AuthError(_) | KeyComputeError::PermissionDenied(_) => {
                ErrorCategory::Auth
            }
            KeyComputeError::VerificationError(_) => ErrorCategory::Verification,
            KeyComputeError::RateLimitExceeded(_) => ErrorCategory::RateLimit,
            KeyComputeError::RoutingFailed(_) | KeyComputeError::NoReadyNode(_) => {
                ErrorCategory::Routing
            }
            KeyComputeError::ProviderError(_)
            | KeyComputeError::ProviderTimeout(_, _)
            | KeyComputeError::UpstreamFailure { .. } => ErrorCategory::Provider,
            KeyComputeError::DatabaseError(_) => ErrorCategory::Database,
            KeyComputeError::ConfigError(_) => ErrorCategory::Config,
            KeyComputeError::ServiceUnavailable(_) => ErrorCategory::ServiceUnavailable,
            KeyComputeError::ValidationError(_) | KeyComputeError::InvalidRequest(_) => {
                ErrorCategory::Validation
            }
            KeyComputeError::NotFound(_) => ErrorCategory::NotFound,
            KeyComputeError::NetworkError(_) | KeyComputeError::Timeout(_) => {
                ErrorCategory::Network
            }
            KeyComputeError::Internal(_) | KeyComputeError::SerializationError(_) => {
                ErrorCategory::Internal
            }
        }
    }
}

/// 错误类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Auth,
    Verification,
    RateLimit,
    Routing,
    Provider,
    Database,
    Config,
    ServiceUnavailable,
    Validation,
    NotFound,
    Network,
    Internal,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCategory::Auth => write!(f, "authentication_error"),
            ErrorCategory::Verification => write!(f, "verification_error"),
            ErrorCategory::RateLimit => write!(f, "rate_limit_error"),
            ErrorCategory::Routing => write!(f, "routing_error"),
            ErrorCategory::Provider => write!(f, "provider_error"),
            ErrorCategory::Database => write!(f, "database_error"),
            ErrorCategory::Config => write!(f, "config_error"),
            ErrorCategory::ServiceUnavailable => write!(f, "service_unavailable_error"),
            ErrorCategory::Validation => write!(f, "validation_error"),
            ErrorCategory::NotFound => write!(f, "not_found_error"),
            ErrorCategory::Network => write!(f, "network_error"),
            ErrorCategory::Internal => write!(f, "internal_error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = KeyComputeError::AuthError("invalid token".to_string());
        assert!(err.to_string().contains("authentication failed"));

        let err = KeyComputeError::RoutingFailed("gpt-4".to_string());
        assert!(err.to_string().contains("gpt-4"));
    }

    #[test]
    fn test_is_retryable() {
        // 可重试
        assert!(KeyComputeError::ProviderError("timeout".into()).is_retryable());
        assert!(KeyComputeError::NetworkError("connection reset".into()).is_retryable());
        assert!(KeyComputeError::DatabaseError("deadlock".into()).is_retryable());
        assert!(KeyComputeError::ServiceUnavailable("smtp unavailable".into()).is_retryable());

        // 不可重试
        assert!(!KeyComputeError::AuthError("invalid".into()).is_retryable());
        assert!(!KeyComputeError::VerificationError("bad code".into()).is_retryable());
        assert!(!KeyComputeError::ValidationError("bad input".into()).is_retryable());
        assert!(!KeyComputeError::NotFound("missing".into()).is_retryable());
    }

    #[test]
    fn test_error_category() {
        assert_eq!(
            KeyComputeError::AuthError("test".into()).category(),
            ErrorCategory::Auth
        );
        assert_eq!(
            KeyComputeError::VerificationError("test".into()).category(),
            ErrorCategory::Verification
        );
        assert_eq!(
            KeyComputeError::RateLimitExceeded("test".into()).category(),
            ErrorCategory::RateLimit
        );
        assert_eq!(
            KeyComputeError::ProviderError("test".into()).category(),
            ErrorCategory::Provider
        );
        assert_eq!(
            KeyComputeError::ServiceUnavailable("test".into()).category(),
            ErrorCategory::ServiceUnavailable
        );
    }

    #[test]
    fn test_category_display() {
        assert_eq!(ErrorCategory::Auth.to_string(), "authentication_error");
        assert_eq!(
            ErrorCategory::Verification.to_string(),
            "verification_error"
        );
        assert_eq!(ErrorCategory::RateLimit.to_string(), "rate_limit_error");
        assert_eq!(
            ErrorCategory::ServiceUnavailable.to_string(),
            "service_unavailable_error"
        );
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err: KeyComputeError = json_err.into();
        assert!(matches!(err, KeyComputeError::SerializationError(_)));
    }

    #[test]
    fn test_from_uuid_error() {
        let uuid_err = uuid::Uuid::parse_str("not-a-uuid").unwrap_err();
        let err: KeyComputeError = uuid_err.into();
        assert!(matches!(err, KeyComputeError::InvalidRequest(_)));
    }
}
