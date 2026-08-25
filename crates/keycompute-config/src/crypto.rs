//! 加密配置
//!
//! 用于上游 Provider API Key 的加密存储。

use serde::Deserialize;

/// 加密配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct CryptoConfig {
    /// 加密密钥（Base64 编码的 32 字节密钥）
    ///
    /// 生产环境必须设置，否则服务拒绝启动；仅开发环境在未配置时
    /// 允许回退为明文存储。
    /// 可通过以下方式生成：
    /// - 命令行：`openssl rand -base64 32`
    /// - 代码：`ApiKeyCrypto::generate_key()`
    ///
    /// 环境变量：KC__CRYPTO__SECRET_KEY
    pub secret_key: Option<String>,
}

impl CryptoConfig {
    /// 检查是否配置了加密密钥
    pub fn has_key(&self) -> bool {
        self.secret_key.is_some() && !self.secret_key.as_ref().unwrap().is_empty()
    }

    /// 获取密钥（如果已配置）
    pub fn secret_key(&self) -> Option<&str> {
        self.secret_key.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_crypto_config() {
        let config = CryptoConfig::default();
        assert!(!config.has_key());
        assert!(config.secret_key().is_none());
    }

    #[test]
    fn test_crypto_config_with_key() {
        let config = CryptoConfig {
            secret_key: Some("dGVzdC1rZXktMTIzNDU2Nzg5MGFiY2RlZg==".to_string()),
        };
        assert!(config.has_key());
        assert!(config.secret_key().is_some());
    }
}
