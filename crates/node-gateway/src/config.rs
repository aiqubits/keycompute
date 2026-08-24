//! Node Gateway 配置模块
//!
//! 定义节点网关的配置结构，包括会话 TTL、心跳间隔、轮询超时等参数。

use keycompute_config::{DEFAULT_REGISTRATION_TOKEN_SECRET, NodeGatewayConfig};
use std::time::Duration;

/// Node Gateway 配置
#[derive(Debug, Clone)]
pub struct NodeGatewayAppConfig {
    /// HMAC 签名密钥（用于验证节点注册 token 的 HMAC 签名）
    pub registration_token_secret: String,
    /// 会话 TTL(秒)
    pub session_ttl_secs: u64,
    /// 心跳间隔(秒)
    pub heartbeat_interval_secs: u64,
    /// 轮询超时(秒)
    pub poll_timeout_secs: u64,
    /// 任务 deadline 超时(秒)
    pub task_deadline_secs: u64,
    /// 完成宽限期(秒)
    pub complete_grace_secs: u64,
    /// 节点失败阈值
    pub node_failure_threshold: u32,
    /// 任务失败阈值
    pub task_failure_threshold: u32,
    /// Sweeper 心跳 TTL(秒) - 超过此时间未心跳的 online 节点标记为 offline
    pub sweeper_heartbeat_ttl_secs: u64,
    /// Sweeper 补推间隔(秒) - 创建超过此时间的 queued 任务补推到 Redis
    pub sweeper_repush_interval_secs: u64,
}

impl Default for NodeGatewayAppConfig {
    fn default() -> Self {
        Self {
            registration_token_secret: DEFAULT_REGISTRATION_TOKEN_SECRET.to_string(),
            session_ttl_secs: 300, // 5 分钟
            heartbeat_interval_secs: 30,
            poll_timeout_secs: 30,
            task_deadline_secs: 120, // 2 分钟
            complete_grace_secs: 60, // 1 分钟
            node_failure_threshold: 3,
            task_failure_threshold: 3,
            sweeper_heartbeat_ttl_secs: 600, // 10 分钟
            sweeper_repush_interval_secs: 10,
        }
    }
}

impl NodeGatewayAppConfig {
    /// 从配置文件中加载
    ///
    /// 缺失、空值或示例密钥不会阻止服务启动；生产环境会记录安全建议。
    pub fn from_config(config: &NodeGatewayConfig) -> Self {
        let secret = config
            .registration_token_secret
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_REGISTRATION_TOKEN_SECRET.to_string());

        if secret == DEFAULT_REGISTRATION_TOKEN_SECRET {
            tracing::warn!(
                "Node Gateway 正在使用示例 registration_token_secret；生产环境强烈建议设置独立随机密钥"
            );
        }

        Self {
            registration_token_secret: secret,
            session_ttl_secs: config.session_ttl_secs.unwrap_or(300),
            heartbeat_interval_secs: config.heartbeat_interval_secs.unwrap_or(30),
            poll_timeout_secs: config.poll_timeout_secs.unwrap_or(30),
            task_deadline_secs: config.task_deadline_secs.unwrap_or(120),
            complete_grace_secs: config.complete_grace_secs.unwrap_or(60),
            node_failure_threshold: config.node_failure_threshold.unwrap_or(3),
            task_failure_threshold: config.task_failure_threshold.unwrap_or(3),
            sweeper_heartbeat_ttl_secs: config.sweeper_heartbeat_ttl_secs.unwrap_or(600),
            sweeper_repush_interval_secs: config.sweeper_repush_interval_secs.unwrap_or(10),
        }
    }

    /// 获取会话 TTL 的 Duration
    pub fn session_ttl(&self) -> Duration {
        Duration::from_secs(self.session_ttl_secs)
    }

    /// 获取心跳间隔的 Duration
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.heartbeat_interval_secs)
    }

    /// 获取轮询超时的 Duration
    pub fn poll_timeout(&self) -> Duration {
        Duration::from_secs(self.poll_timeout_secs)
    }

    /// 获取任务 deadline 的 Duration
    pub fn task_deadline(&self) -> Duration {
        Duration::from_secs(self.task_deadline_secs)
    }

    /// 获取完成宽限期的 Duration
    pub fn complete_grace(&self) -> Duration {
        Duration::from_secs(self.complete_grace_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_registration_secret_uses_advisory_default() {
        let config = NodeGatewayConfig {
            registration_token_secret: None,
            ..Default::default()
        };

        let resolved = NodeGatewayAppConfig::from_config(&config);

        assert_eq!(
            resolved.registration_token_secret,
            DEFAULT_REGISTRATION_TOKEN_SECRET
        );
    }

    #[test]
    fn configured_registration_secret_is_preserved() {
        let config = NodeGatewayConfig {
            registration_token_secret: Some("independent-random-registration-secret".to_string()),
            ..Default::default()
        };

        let resolved = NodeGatewayAppConfig::from_config(&config);

        assert_eq!(
            resolved.registration_token_secret,
            "independent-random-registration-secret"
        );
    }

    #[test]
    fn configured_short_registration_secret_is_advisory() {
        let config = NodeGatewayConfig {
            registration_token_secret: Some("dev".to_string()),
            ..Default::default()
        };

        let resolved = NodeGatewayAppConfig::from_config(&config);

        assert_eq!(resolved.registration_token_secret, "dev");
    }
}
