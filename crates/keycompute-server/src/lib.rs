//! KeyCompute Server
//!
//! API Gateway Layer：主 Axum 服务器，OpenAI-compatible API 入口。
//! 负责 HTTP 路由、中间件编排、SSE 输出，不含业务逻辑。

pub mod error;
pub mod extractors;
pub mod handlers;
mod lifecycle_metrics;
pub mod middleware;
pub mod payment_registry;
pub mod providers;
pub mod router;
pub mod state;

pub use error::{ApiError, Result};
pub use extractors::AuthExtractor;
pub use router::create_router;
pub use state::{AppState, AppStateConfig, init_global_crypto};

use std::net::SocketAddr;
use tracing::info;

// 从 keycompute-config 导入 ServerConfig
pub use keycompute_config::ServerConfig;

/// 运行服务器
pub async fn run(config: ServerConfig, state: AppState) -> crate::error::Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.bind_addr, config.port)
        .parse()
        .map_err(|e| crate::error::ApiError::Config(format!("Invalid address: {}", e)))?;

    let app = create_router(state);

    info!("KeyCompute server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| crate::error::ApiError::Internal(format!("Failed to bind: {}", e)))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| crate::error::ApiError::Internal(format!("Server error: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_addr, "0.0.0.0");
        assert_eq!(config.port, 3000);
    }
}
