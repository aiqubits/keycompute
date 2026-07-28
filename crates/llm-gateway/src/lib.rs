//! LLM Gateway
//!
//! 唯一执行层，控制 retry/fallback/streaming 生命周期。
//! 架构约束：只此一层能执行上游请求。
//!
//! ## 模块结构
//! - `executor`: 核心执行器，管理请求生命周期
//! - `proxy`: Internal HTTP Proxy Module，统一上游连接管理
//! - `failover`: 故障转移管理
//! - `retry`: 重试策略
//! - `streaming`: 流处理管道

pub mod executor;
pub mod failover;
pub mod proxy;
pub mod retry;
pub mod streaming;

pub use executor::GatewayExecutor;
pub use failover::FailoverManager;
pub use proxy::{HttpClient, HttpProxy, ProxyConfig, ProxyRequest, ProxySelector};
pub use retry::RetryPolicy;
pub use streaming::{StreamPipeline, StreamingContext};

use llm_protocol_provider::ProviderAdapter;
use std::collections::HashMap;
use std::sync::Arc;

/// Gateway 配置
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// 最大重试次数
    pub max_retries: u32,
    /// 请求超时时间（秒）
    pub timeout_secs: u64,
    /// 是否启用 fallback
    pub enable_fallback: bool,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            timeout_secs: 120,
            enable_fallback: true,
        }
    }
}

/// Gateway 构建器
#[derive(Debug)]
pub struct GatewayBuilder {
    config: GatewayConfig,
    providers: HashMap<String, Arc<dyn ProviderAdapter>>,
    /// Internal HTTP Proxy
    http_proxy: Option<Arc<HttpProxy>>,
}

impl GatewayBuilder {
    /// 创建新的 Gateway 构建器
    pub fn new() -> Self {
        Self {
            config: GatewayConfig::default(),
            providers: HashMap::new(),
            http_proxy: None,
        }
    }

    /// 设置配置
    pub fn with_config(mut self, config: GatewayConfig) -> Self {
        self.config = config;
        self
    }

    /// 添加 Provider
    pub fn add_provider(
        mut self,
        name: impl Into<String>,
        provider: Arc<dyn ProviderAdapter>,
    ) -> Self {
        self.providers.insert(name.into(), provider);
        self
    }

    /// 设置 HTTP Proxy
    ///
    /// 用于统一上游连接管理、多代理出口、请求追踪
    pub fn with_http_proxy(mut self, proxy: Arc<HttpProxy>) -> Self {
        self.http_proxy = Some(proxy);
        self
    }

    /// 设置 HTTP Proxy 配置
    ///
    /// 使用默认代理选择器创建 HttpProxy
    pub fn with_proxy_config(mut self, config: ProxyConfig) -> Self {
        self.http_proxy = Some(Arc::new(HttpProxy::new(config)));
        self
    }

    /// 构建 GatewayExecutor
    pub fn build(self) -> GatewayExecutor {
        if let Some(proxy) = self.http_proxy {
            GatewayExecutor::with_proxy(self.config, self.providers, proxy)
        } else {
            GatewayExecutor::new(self.config, self.providers)
        }
    }
}

impl Default for GatewayBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_config_default() {
        let config = GatewayConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.timeout_secs, 120);
        assert!(config.enable_fallback);
    }

    #[test]
    fn test_gateway_builder() {
        let builder = GatewayBuilder::new();
        assert!(builder.providers.is_empty());
    }
}
