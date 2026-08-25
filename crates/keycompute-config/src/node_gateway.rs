//! Node Gateway 配置模块
//!
//! 节点网关的配置结构，包括会话 TTL、心跳间隔、轮询超时等参数。

use serde::Deserialize;

/// Node Gateway 注册 Token 的示例密钥。
///
/// 该值便于开发环境开箱运行。生产环境配置 Redis、从而启用 Node Gateway
/// 时，启动检查会拒绝该值，并要求独立的随机密钥。
pub const DEFAULT_REGISTRATION_TOKEN_SECRET: &str = "change-me-node-registration-token-secret";

/// Node Gateway 配置
#[derive(Debug, Deserialize, Clone)]
pub struct NodeGatewayConfig {
    /// HMAC 签名密钥（用于签发/验证节点注册 token 的 HMAC 签名）
    /// 环境变量: KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET
    /// 默认值: change-me-node-registration-token-secret（仅供开发；生产环境启用 Redis 时必须修改）
    pub registration_token_secret: Option<String>,
    /// 会话 TTL(秒),默认 300
    pub session_ttl_secs: Option<u64>,
    /// 心跳间隔(秒),默认 30
    pub heartbeat_interval_secs: Option<u64>,
    /// 轮询超时(秒),默认 30
    pub poll_timeout_secs: Option<u64>,
    /// 任务 deadline 超时(秒),默认 120
    pub task_deadline_secs: Option<u64>,
    /// 完成宽限期(秒),默认 60
    pub complete_grace_secs: Option<u64>,
    /// 节点失败阈值,默认 3
    pub node_failure_threshold: Option<u32>,
    /// 任务失败阈值,默认 3
    pub task_failure_threshold: Option<u32>,
    /// Sweeper 心跳 TTL(秒),默认 600
    pub sweeper_heartbeat_ttl_secs: Option<u64>,
    /// Sweeper 补推间隔(秒),默认 10
    pub sweeper_repush_interval_secs: Option<u64>,
}

impl Default for NodeGatewayConfig {
    fn default() -> Self {
        Self {
            registration_token_secret: Some(DEFAULT_REGISTRATION_TOKEN_SECRET.to_string()),
            session_ttl_secs: Some(300),
            heartbeat_interval_secs: Some(30),
            poll_timeout_secs: Some(30),
            task_deadline_secs: Some(120),
            complete_grace_secs: Some(60),
            node_failure_threshold: Some(3),
            task_failure_threshold: Some(3),
            sweeper_heartbeat_ttl_secs: Some(600),
            sweeper_repush_interval_secs: Some(10),
        }
    }
}
