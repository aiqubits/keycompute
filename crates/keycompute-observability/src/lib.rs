//! KeyCompute 可观测性基础设施
//!
//! 本 crate 提供日志、指标、追踪和主机监控功能，
//! 被所有需要可观测性的后端 crate 依赖。

pub mod host_monitor;
pub mod logger;
pub mod metrics;
pub mod tracing;

// 重新导出常用功能
pub use host_monitor::{
    HealthStatus, HostMetrics, HostMonitor, SystemSnapshot, collect_host_metrics,
};
pub use logger::{
    init_dev_logger, init_logger, init_prod_logger, is_logger_initialized, try_init_logger,
};
pub use metrics::{MetricsCollector, init_metrics, is_metrics_initialized};
pub use tracing::{
    create_provider_span, create_request_span, create_stream_span, events, init_distributed_tracing,
};

/// 初始化所有可观测性组件
///
/// 在生产应用启动时调用此函数，初始化 JSON 日志和指标系统
///
/// # Examples
///
/// ```
/// use keycompute_observability::init_observability;
///
/// #[tokio::main]
/// async fn main() {
///     init_observability();
///     // ...
/// }
/// ```
pub fn init_observability() {
    init_prod_logger();
    init_metrics();
    // 日志系统已初始化，tracing 将通过 subscriber 输出此消息
}

/// 初始化开发环境的可观测性组件
///
/// 使用更易读的日志格式，适合本地开发
pub fn init_dev_observability() {
    init_dev_logger();
    init_metrics();
    // 日志系统已初始化，tracing 将通过 subscriber 输出此消息
}
