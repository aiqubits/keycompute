use client_api::{
    AdminApi,
    api::admin::{
        MonitoringQuery, MonitoringRequestDetail, MonitoringRequestPage, MonitoringSummaryResponse,
        MonitoringTargetHealthResponse,
    },
    error::Result,
};

use super::api_client::get_client;

pub async fn requests(token: &str, query: &MonitoringQuery) -> Result<MonitoringRequestPage> {
    AdminApi::new(&get_client())
        .monitoring_requests(query, token)
        .await
}
pub async fn request_detail(token: &str, id: &str) -> Result<MonitoringRequestDetail> {
    AdminApi::new(&get_client())
        .monitoring_request(id, token)
        .await
}
pub async fn summary(token: &str, query: &MonitoringQuery) -> Result<MonitoringSummaryResponse> {
    AdminApi::new(&get_client())
        .monitoring_summary(query, token)
        .await
}
pub async fn target_health(
    token: &str,
    query: &MonitoringQuery,
) -> Result<MonitoringTargetHealthResponse> {
    AdminApi::new(&get_client())
        .monitoring_target_health(query, token)
        .await
}
pub async fn probe_targets(
    token: &str,
    account_ids: Option<Vec<String>>,
) -> Result<serde_json::Value> {
    AdminApi::new(&get_client())
        .probe_monitoring_targets(account_ids, token)
        .await
}
