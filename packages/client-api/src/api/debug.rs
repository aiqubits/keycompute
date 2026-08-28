//! 调试接口模块
//!
//! 提供路由调试、Provider 健康状态等调试功能（Admin）

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use std::collections::HashMap;

/// 调试 API 客户端
#[derive(Debug, Clone)]
pub struct DebugApi {
    client: ApiClient,
}

impl DebugApi {
    /// 创建新的调试 API 客户端
    pub fn new(client: &ApiClient) -> Self {
        Self {
            client: client.clone(),
        }
    }

    /// 获取路由调试信息
    ///
    /// # 参数
    /// - `model`: 模型名称，用于模拟路由决策
    /// - `token`: 认证令牌
    pub async fn debug_routing(&self, model: &str, token: &str) -> Result<RoutingDebugInfo> {
        self.debug_routing_request(model, None, token).await
    }

    /// 获取指定入口协议的路由调试信息。
    ///
    /// `entry` 只应传入服务端支持的稳定协议标识（当前为 `openai` / `anthropic`）。
    pub async fn debug_routing_for_entry(
        &self,
        model: &str,
        entry: &str,
        token: &str,
    ) -> Result<RoutingDebugInfo> {
        self.debug_routing_request(model, Some(entry), token).await
    }

    async fn debug_routing_request(
        &self,
        model: &str,
        entry: Option<&str>,
        token: &str,
    ) -> Result<RoutingDebugInfo> {
        let mut path = format!("/api/v1/debug/routing?model={}", urlencoding::encode(model));
        if let Some(entry) = entry {
            path.push_str("&entry=");
            path.push_str(&urlencoding::encode(entry));
        }
        self.client.get_json(&path, Some(token)).await
    }

    /// 获取 Provider 健康状态
    pub async fn get_provider_health(&self, token: &str) -> Result<ProviderHealthResponse> {
        self.client
            .get_json("/api/v1/debug/providers", Some(token))
            .await
    }

    /// 获取网关状态
    pub async fn get_gateway_status(&self, token: &str) -> Result<GatewayStatus> {
        self.client
            .get_json("/api/v1/debug/gateway/status", Some(token))
            .await
    }

    /// 获取网关统计
    pub async fn get_gateway_stats(&self, token: &str) -> Result<GatewayStats> {
        self.client
            .get_json("/api/v1/debug/gateway/stats", Some(token))
            .await
    }

    /// 检查 Provider 健康
    pub async fn check_provider_health(
        &self,
        provider: &str,
        token: &str,
    ) -> Result<ProviderHealthCheckResponse> {
        self.client
            .post_json(
                "/api/v1/debug/gateway/health",
                &serde_json::json!({ "provider": provider }),
                Some(token),
            )
            .await
    }

    /// 重置所有 Provider 健康状态和冷却状态
    ///
    /// 用于调试，清除所有 Provider 和账号的健康状态和冷却状态
    pub async fn reset_health(&self, token: &str) -> Result<ResetHealthResponse> {
        self.client
            .post_json(
                "/api/v1/debug/providers/reset",
                &serde_json::json!({}),
                Some(token),
            )
            .await
    }
}

/// 路由调试信息
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingDebugInfo {
    /// 请求 ID
    pub request_id: String,
    /// 是否成功路由
    pub routed: bool,
    /// 主目标（路由成功时有值）
    pub primary: Option<RoutingTargetInfo>,
    /// 备用链路
    pub fallback_chain: Vec<RoutingTargetInfo>,
    /// 定价信息
    pub pricing: PricingInfo,
    /// Provider 状态列表
    pub provider_status: Vec<ProviderStatusInfo>,
    /// 提示信息
    pub message: Option<String>,
}

/// 路由目标信息
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingTargetInfo {
    pub provider: String,
    pub account_id: String,
    pub endpoint: String,
}

/// 定价信息
#[derive(Debug, Clone, Deserialize)]
pub struct PricingInfo {
    pub model_name: String,
    pub currency: String,
    pub input_price_per_1k: String,
    pub output_price_per_1k: String,
}

/// Provider 状态信息
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderStatusInfo {
    pub provider: String,
    pub is_healthy: bool,
    pub account_count: i64,
    pub status: String,
}

/// Provider 健康响应
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderHealthResponse {
    pub healthy_providers: Vec<String>,
    pub account_count: usize,
}

/// 网关状态
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayStatus {
    pub available: bool,
    pub providers: Vec<GatewayProvider>,
    pub config: GatewayConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayProvider {
    pub name: String,
    pub supported_models: Vec<String>,
    pub healthy: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    pub max_retries: u32,
    pub timeout_secs: u64,
    pub enable_fallback: bool,
}

/// 网关统计
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayStats {
    pub total_requests: i64,
    pub successful_requests: i64,
    pub failed_requests: i64,
    pub fallback_count: i64,
    pub avg_latency_ms: u64,
    pub provider_stats: HashMap<String, ProviderStats>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderStats {
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub avg_latency_ms: u64,
}

/// 健康检查响应
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderHealthCheckResponse {
    pub provider: String,
    pub healthy: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub models: Vec<String>,
}

/// 重置健康状态响应
#[derive(Debug, Clone, Deserialize)]
pub struct ResetHealthResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
}
