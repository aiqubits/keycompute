//! Node Gateway - 节点网关核心实现
//!
//! 负责节点注册、心跳、任务入队、任务领取、结果提交和同步等待。
//!
//! ## 模块结构
//!
//! - `config`: 配置模块
//! - `store`: 数据库操作层
//! - `service`: 业务逻辑层
//! - `redis`: Redis 队列管理
//! - `sweeper`: 后台维护任务
//! - `node_index`: Node 能力索引实现

pub mod config;
mod metrics;
pub mod node_index;
pub mod redis;
pub mod service;
pub mod store;
pub mod sweeper;
mod trace;

// 重新导出关键类型
pub use config::NodeGatewayAppConfig;
pub use node_index::PostgresNodeIndex;
pub use redis::NodeGatewayRedis;
pub use service::{NodeExecutionError, NodeGatewayService};
pub use store::NodeGatewayStore;
pub use sweeper::NodeGatewaySweeper;
