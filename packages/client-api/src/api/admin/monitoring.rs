use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringOverviewResponse {
    pub summary: MonitoringSummary,
    pub traces: Vec<MonitoringTraceEntry>,
    pub nodes: Vec<MonitoringNodeHealth>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringSummary {
    pub total_usage_logs: i64,
    pub total_node_tasks: i64,
    pub active_node_tasks: i64,
    pub succeeded_node_tasks: i64,
    pub failed_node_tasks: i64,
    pub online_nodes: i64,
    pub avg_node_latency_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringTraceEntry {
    pub request_id: String,
    pub task_id: String,
    pub model: String,
    pub status: String,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
    pub lease_id: Option<String>,
    pub queued_at: String,
    pub claimed_at: Option<String>,
    pub finished_at: Option<String>,
    pub deadline_at: String,
    pub duration_ms: Option<i64>,
    pub usage_status: Option<String>,
    pub total_tokens: Option<i32>,
    pub amount: Option<String>,
    pub submissions_count: i64,
    pub last_submission_action: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringNodeHealth {
    pub id: String,
    pub display_name: String,
    pub status: String,
    pub accepted_models_json: serde_json::Value,
    pub last_heartbeat_at: Option<String>,
    pub active_tasks: i64,
    pub succeeded_tasks: i64,
    pub failed_tasks: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringRequestPage {
    pub items: Vec<MonitoringRequestItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringRequestItem {
    pub request_id: String,
    pub client_request_id: Option<String>,
    pub protocol: String,
    pub tenant_id: String,
    pub user_id: String,
    pub produce_ai_key_id: String,
    pub request_path: String,
    pub requested_model: String,
    pub is_stream: bool,
    pub route_type: Option<String>,
    pub status: String,
    pub billing_status: String,
    pub error_origin: Option<String>,
    pub error_category: Option<String>,
    pub error_code: Option<String>,
    pub trace_quality: String,
    pub received_at: String,
    pub client_first_content_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub provider_ttft_ms: Option<i64>,
    pub provider_name: Option<String>,
    pub account_id: Option<String>,
    pub node_id: Option<String>,
    pub total_tokens: Option<i32>,
    pub amount: Option<String>,
    pub currency: Option<String>,
    pub has_fallback: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringAttemptDetail {
    pub id: String,
    pub attempt_no: i32,
    pub attempt_kind: String,
    pub route_type: String,
    pub model: String,
    pub status: String,
    pub is_final: bool,
    pub provider_name: Option<String>,
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub node_task_id: Option<String>,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
    pub session_id: Option<String>,
    pub lease_id: Option<String>,
    pub upstream_request_id: Option<String>,
    pub http_status: Option<i32>,
    pub retryable: Option<bool>,
    pub error_origin: Option<String>,
    pub error_category: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub started_at: String,
    pub headers_received_at: Option<String>,
    pub first_content_at: Option<String>,
    pub stream_end_reason: Option<String>,
    pub stream_error_count: Option<i32>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringRequestDetail {
    pub request: MonitoringRequestItem,
    pub attempts: Vec<MonitoringAttemptDetail>,
    pub node_task: Option<serde_json::Value>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringTraceSummary {
    pub request_count: i64,
    pub succeeded_count: i64,
    pub failed_count: i64,
    pub active_count: i64,
    pub queued_count: i64,
    pub fallback_request_count: i64,
    pub provider_request_count: i64,
    pub attempt_count: i64,
    pub terminal_attempt_count: i64,
    pub succeeded_attempt_count: i64,
    pub p50_duration_ms: Option<f64>,
    pub p95_duration_ms: Option<f64>,
    pub p99_duration_ms: Option<f64>,
    pub p50_provider_ttft_ms: Option<f64>,
    pub p95_provider_ttft_ms: Option<f64>,
    pub p50_node_queue_ms: Option<f64>,
    pub p95_node_queue_ms: Option<f64>,
    pub p50_node_execution_ms: Option<f64>,
    pub p95_node_execution_ms: Option<f64>,
    pub total_tokens: Option<i64>,
    pub amounts_by_currency: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringSummaryResponse {
    pub summary: MonitoringTraceSummary,
    pub success_rate: Option<f64>,
    pub error_rate: Option<f64>,
    pub fallback_rate: Option<f64>,
    pub attempt_success_rate: Option<f64>,
    pub series: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MonitoringTargetHealthResponse {
    pub providers: Vec<serde_json::Value>,
    pub nodes: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MonitoringProbeRequest {
    pub account_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MonitoringQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub tenant_id: Option<String>,
    pub user_id: Option<String>,
    pub produce_ai_key_id: Option<String>,
    pub protocol: Option<String>,
    pub request_path: Option<String>,
    pub model: Option<String>,
    pub is_stream: Option<bool>,
    pub status: Option<String>,
    pub billing_status: Option<String>,
    pub error_category: Option<String>,
    pub route_type: Option<String>,
    pub provider: Option<String>,
    pub account_id: Option<String>,
    pub node_id: Option<String>,
    pub request_id: Option<String>,
    pub client_request_id: Option<String>,
    pub upstream_request_id: Option<String>,
    pub attempt_kind: Option<String>,
    pub fallback_only: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
    pub bucket: Option<String>,
}
impl MonitoringQuery {
    pub fn to_query_string(&self) -> String {
        let mut pairs = Vec::new();
        macro_rules! push {
            ($name:literal,$value:expr) => {
                if let Some(value) = $value {
                    pairs.push(format!(
                        "{}={}",
                        $name,
                        urlencoding::encode(&value.to_string())
                    ));
                }
            };
        }
        push!("from", self.from.as_ref());
        push!("to", self.to.as_ref());
        push!("tenant_id", self.tenant_id.as_ref());
        push!("user_id", self.user_id.as_ref());
        push!("produce_ai_key_id", self.produce_ai_key_id.as_ref());
        push!("protocol", self.protocol.as_ref());
        push!("request_path", self.request_path.as_ref());
        push!("model", self.model.as_ref());
        push!("is_stream", self.is_stream);
        push!("status", self.status.as_ref());
        push!("billing_status", self.billing_status.as_ref());
        push!("error_category", self.error_category.as_ref());
        push!("route_type", self.route_type.as_ref());
        push!("provider", self.provider.as_ref());
        push!("account_id", self.account_id.as_ref());
        push!("node_id", self.node_id.as_ref());
        push!("request_id", self.request_id.as_ref());
        push!("client_request_id", self.client_request_id.as_ref());
        push!("upstream_request_id", self.upstream_request_id.as_ref());
        push!("attempt_kind", self.attempt_kind.as_ref());
        push!("fallback_only", self.fallback_only);
        push!("cursor", self.cursor.as_ref());
        push!("limit", self.limit);
        push!("bucket", self.bucket.as_ref());
        pairs.join("&")
    }
}
