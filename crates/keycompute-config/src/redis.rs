//! Redis 配置

use serde::Deserialize;

const fn default_pool_size() -> u32 {
    10
}

const fn default_connect_timeout_secs() -> u64 {
    5
}

/// Redis 配置
#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    /// Redis 连接 URL
    pub url: String,
    /// 连接池大小
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,
    /// 连接超时（秒）
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            pool_size: default_pool_size(),
            connect_timeout_secs: default_connect_timeout_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_redis_config() {
        let config = RedisConfig::default();
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert_eq!(config.pool_size, 10);
        assert_eq!(config.connect_timeout_secs, 5);
    }
}
