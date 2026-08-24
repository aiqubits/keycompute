//! 数据库配置

use serde::Deserialize;

/// 默认读库选择策略
fn default_read_strategy() -> String {
    "round_robin".to_string()
}

/// 默认重试次数
const fn default_retry_attempts() -> usize {
    2
}

/// 默认熔断时间（毫秒）
const fn default_circuit_break_ms() -> u64 {
    30000
}

/// 默认是否回退到写库
const fn default_fallback_to_write() -> bool {
    true
}

/// 默认健康检查间隔（秒）
const fn default_health_check_interval_secs() -> u64 {
    15
}

/// 默认读库最大连接数
const fn default_read_max_connections() -> u32 {
    10
}

/// 默认读库最小连接数
const fn default_read_min_connections() -> u32 {
    1
}

/// 默认读库连接超时
const fn default_read_connect_timeout() -> u64 {
    5
}

/// 默认读库空闲超时
const fn default_read_idle_timeout() -> u64 {
    600
}

/// 默认读库获取连接超时
const fn default_read_acquire_timeout() -> u64 {
    10
}

/// 默认读库连接最大生命周期（秒）
const fn default_read_max_lifetime() -> u64 {
    1800
}

/// 读库路由配置
#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseRoutingConfig {
    /// 读库选择策略: round_robin | random | weighted
    #[serde(default = "default_read_strategy")]
    pub strategy: String,
    /// 各读库权重（仅 weighted 策略生效）
    #[serde(default)]
    pub read_weights: Vec<u32>,
    /// 额外重试次数
    #[serde(default = "default_retry_attempts")]
    pub retry_attempts: usize,
    /// 熔断时间（毫秒）
    #[serde(default = "default_circuit_break_ms")]
    pub circuit_break_ms: u64,
    /// 是否回退到写库
    #[serde(default = "default_fallback_to_write")]
    pub fallback_to_write: bool,
    /// 健康检查间隔（秒，0=禁用）
    #[serde(default = "default_health_check_interval_secs")]
    pub health_check_interval_secs: u64,
}

impl Default for DatabaseRoutingConfig {
    fn default() -> Self {
        Self {
            strategy: default_read_strategy(),
            read_weights: Vec::new(),
            retry_attempts: default_retry_attempts(),
            circuit_break_ms: default_circuit_break_ms(),
            fallback_to_write: default_fallback_to_write(),
            health_check_interval_secs: default_health_check_interval_secs(),
        }
    }
}

/// 读库连接池配置
#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseReadConfig {
    /// 最大连接数
    #[serde(default = "default_read_max_connections")]
    pub max_connections: u32,
    /// 最小连接数
    #[serde(default = "default_read_min_connections")]
    pub min_connections: u32,
    /// 连接超时时间（秒）
    #[serde(default = "default_read_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// 连接空闲超时时间（秒）
    #[serde(default = "default_read_idle_timeout")]
    pub idle_timeout_secs: u64,
    /// 获取连接超时时间（秒）
    #[serde(default = "default_read_acquire_timeout")]
    pub acquire_timeout_secs: u64,
    /// 连接最大生命周期（秒，默认 1800）
    #[serde(default = "default_read_max_lifetime")]
    pub max_lifetime_secs: u64,
}

impl Default for DatabaseReadConfig {
    fn default() -> Self {
        Self {
            max_connections: default_read_max_connections(),
            min_connections: default_read_min_connections(),
            connect_timeout_secs: default_read_connect_timeout(),
            idle_timeout_secs: default_read_idle_timeout(),
            acquire_timeout_secs: default_read_acquire_timeout(),
            max_lifetime_secs: default_read_max_lifetime(),
        }
    }
}

/// 数据库配置
#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    /// 写库连接 URL
    pub url: String,
    /// 最大连接数
    pub max_connections: u32,
    /// 最小连接数
    pub min_connections: u32,
    /// 连接超时时间（秒）
    pub connect_timeout_secs: u64,
    /// 连接空闲超时时间（秒）
    pub idle_timeout_secs: u64,
    /// 连接最大生命周期（秒）
    pub max_lifetime_secs: u64,
}

impl DatabaseConfig {
    /// 获取连接超时
    pub fn connect_timeout(&self) -> u64 {
        self.connect_timeout_secs
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://localhost/keycompute".to_string(),
            max_connections: 10,
            min_connections: 2,
            connect_timeout_secs: 30,
            idle_timeout_secs: 600,
            max_lifetime_secs: 1800,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_database_config() {
        let config = DatabaseConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.connect_timeout_secs, 30);
    }

    #[test]
    fn test_default_routing_config() {
        let config = DatabaseRoutingConfig::default();
        assert_eq!(config.strategy, "round_robin");
        assert_eq!(config.retry_attempts, 2);
        assert_eq!(config.circuit_break_ms, 30000);
        assert!(config.fallback_to_write);
    }

    #[test]
    fn test_default_read_config() {
        let config = DatabaseReadConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 1);
        assert_eq!(config.connect_timeout_secs, 5);
    }
}
