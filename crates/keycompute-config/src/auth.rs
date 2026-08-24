//! 认证配置

use serde::Deserialize;

/// JWT 默认密钥（生产环境强烈建议修改）
pub const DEFAULT_JWT_SECRET: &str = "change-me-in-production";

/// 认证配置
#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    /// JWT 密钥（用于签名和验证）
    pub jwt_secret: String,
    /// JWT 签发者
    pub jwt_issuer: String,
    /// JWT 过期时间（秒）
    pub jwt_expiry_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: DEFAULT_JWT_SECRET.to_string(),
            jwt_issuer: "keycompute".to_string(),
            jwt_expiry_secs: 3600,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_auth_config() {
        let config = AuthConfig::default();
        assert_eq!(config.jwt_issuer, "keycompute");
        assert_eq!(config.jwt_expiry_secs, 3600);
    }
}
