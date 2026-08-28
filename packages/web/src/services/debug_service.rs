#![allow(dead_code)]

use client_api::error::Result;
use client_api::{
    DebugApi,
    api::debug::{
        GatewayStats, GatewayStatus, ProviderHealthCheckResponse, ProviderHealthResponse,
        ResetHealthResponse, RoutingDebugInfo,
    },
};

use super::api_client::get_client;

pub async fn routing(model: &str, entry: &str, token: &str) -> Result<RoutingDebugInfo> {
    let client = get_client();
    DebugApi::new(&client)
        .debug_routing_for_entry(model, entry, token)
        .await
}

pub async fn provider_health(token: &str) -> Result<ProviderHealthResponse> {
    let client = get_client();
    DebugApi::new(&client).get_provider_health(token).await
}

pub async fn gateway_status(token: &str) -> Result<GatewayStatus> {
    let client = get_client();
    DebugApi::new(&client).get_gateway_status(token).await
}

pub async fn gateway_stats(token: &str) -> Result<GatewayStats> {
    let client = get_client();
    DebugApi::new(&client).get_gateway_stats(token).await
}

pub async fn check_health(provider: &str, token: &str) -> Result<ProviderHealthCheckResponse> {
    let client = get_client();
    DebugApi::new(&client)
        .check_provider_health(provider, token)
        .await
}

/// 重置所有 Provider 健康状态和冷却状态
pub async fn reset_health(token: &str) -> Result<ResetHealthResponse> {
    let client = get_client();
    DebugApi::new(&client).reset_health(token).await
}
