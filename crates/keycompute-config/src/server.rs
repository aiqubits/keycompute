//! 服务器配置

use serde::Deserialize;

/// 服务器配置
#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// 绑定地址
    pub bind_addr: String,
    /// 监听端口
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0".to_string(),
            port: 3000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_server_config() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_addr, "0.0.0.0");
        assert_eq!(config.port, 3000);
    }
}
