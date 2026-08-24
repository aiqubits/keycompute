//! 管理端监控追踪接口

use crate::{
    error::{ApiError, Result},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{StreamExt, stream};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Serialize, FromQueryResult)]
pub struct MonitoringSummary {
    pub total_usage_logs: i64,
    pub total_node_tasks: i64,
    pub active_node_tasks: i64,
    pub succeeded_node_tasks: i64,
    pub failed_node_tasks: i64,
    pub online_nodes: i64,
    pub avg_node_latency_ms: Option<i64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct MonitoringTraceEntry {
    pub request_id: Uuid,
    pub task_id: Uuid,
    pub model: String,
    pub status: String,
    pub node_id: Option<Uuid>,
    pub node_name: Option<String>,
    pub lease_id: Option<Uuid>,
    pub queued_at: chrono::DateTime<chrono::Utc>,
    pub claimed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub deadline_at: chrono::DateTime<chrono::Utc>,
    pub duration_ms: Option<i64>,
    pub usage_status: Option<String>,
    pub total_tokens: Option<i32>,
    pub amount: Option<String>,
    pub submissions_count: i64,
    pub last_submission_action: Option<String>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct MonitoringNodeHealth {
    pub id: Uuid,
    pub display_name: String,
    pub status: String,
    pub accepted_models_json: serde_json::Value,
    pub last_heartbeat_at: Option<chrono::DateTime<chrono::Utc>>,
    pub active_tasks: i64,
    pub succeeded_tasks: i64,
    pub failed_tasks: i64,
}

#[derive(Debug, Serialize)]
pub struct MonitoringOverviewResponse {
    pub summary: MonitoringSummary,
    pub traces: Vec<MonitoringTraceEntry>,
    pub nodes: Vec<MonitoringNodeHealth>,
}

pub async fn get_monitoring_overview(
    State(state): State<AppState>,
) -> Result<Json<MonitoringOverviewResponse>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;

    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT
            (SELECT COUNT(*) FROM usage_logs)::BIGINT AS total_usage_logs,
            (SELECT COUNT(*) FROM node_tasks)::BIGINT AS total_node_tasks,
            (SELECT COUNT(*) FROM node_tasks WHERE status IN ('queued', 'leased'))::BIGINT AS active_node_tasks,
            (SELECT COUNT(*) FROM node_tasks WHERE status IN ('succeeded', 'image_succeeded'))::BIGINT AS succeeded_node_tasks,
            (SELECT COUNT(*) FROM node_tasks WHERE status IN ('failed', 'expired'))::BIGINT AS failed_node_tasks,
            (SELECT COUNT(*) FROM nodes
             WHERE status = 'online'
               AND (last_heartbeat_at IS NULL
                    OR last_heartbeat_at >= NOW() - INTERVAL '3 minutes')
            )::BIGINT AS online_nodes,
            (
                SELECT AVG(EXTRACT(EPOCH FROM (finished_at - queued_at)) * 1000)::BIGINT
                FROM node_tasks
                WHERE finished_at IS NOT NULL
            ) AS avg_node_latency_ms
        "#,
        [],
    );
    let summary = MonitoringSummary::find_by_statement(stmt)
        .one(pool)
        .await?
        .ok_or_else(|| {
            ApiError::Internal("Failed to load monitoring summary: no data".to_string())
        })?;

    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT
            nt.request_id,
            nt.id AS task_id,
            nt.model,
            nt.status,
            nt.assigned_node_id AS node_id,
            n.display_name AS node_name,
            nt.lease_id,
            nt.queued_at,
            nt.claimed_at,
            nt.finished_at,
            nt.deadline_at,
            CASE
                WHEN nt.finished_at IS NULL THEN NULL
                ELSE (EXTRACT(EPOCH FROM (nt.finished_at - nt.queued_at)) * 1000)::BIGINT
            END AS duration_ms,
            ul.status AS usage_status,
            ul.total_tokens,
            ul.user_amount::TEXT AS amount,
            COALESCE(sub.submissions_count, 0)::BIGINT AS submissions_count,
            sub.last_submission_action
        FROM node_tasks nt
        LEFT JOIN nodes n ON n.id = nt.assigned_node_id
        LEFT JOIN usage_logs ul ON ul.request_id = nt.request_id
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*)::BIGINT AS submissions_count,
                (ARRAY_AGG(action ORDER BY created_at DESC))[1] AS last_submission_action
            FROM node_task_submissions nts
            WHERE nts.task_id = nt.id
        ) sub ON TRUE
        ORDER BY nt.created_at DESC
        LIMIT 50
        "#,
        [],
    );
    let traces = MonitoringTraceEntry::find_by_statement(stmt)
        .all(pool)
        .await?;

    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT
            n.id,
            n.display_name,
            CASE
                WHEN n.status = 'online'
                     AND n.last_heartbeat_at IS NOT NULL
                     AND n.last_heartbeat_at < NOW() - INTERVAL '3 minutes'
                THEN 'offline'
                ELSE n.status
            END AS status,
            COALESCE(latest_session.accepted_models_json, '[]'::jsonb) AS accepted_models_json,
            n.last_heartbeat_at,
            COUNT(nt.id) FILTER (WHERE nt.status IN ('queued', 'leased'))::BIGINT AS active_tasks,
            COUNT(nt.id) FILTER (WHERE nt.status IN ('succeeded', 'image_succeeded'))::BIGINT AS succeeded_tasks,
            COUNT(nt.id) FILTER (WHERE nt.status IN ('failed', 'expired'))::BIGINT AS failed_tasks
        FROM nodes n
        LEFT JOIN LATERAL (
            SELECT accepted_models_json
            FROM node_sessions ns
            WHERE ns.node_id = n.id
            ORDER BY ns.last_seen_at DESC
            LIMIT 1
        ) latest_session ON TRUE
        LEFT JOIN node_tasks nt ON nt.assigned_node_id = n.id
        GROUP BY n.id, n.status, n.last_heartbeat_at, latest_session.accepted_models_json
        ORDER BY n.last_heartbeat_at DESC NULLS LAST, n.updated_at DESC
        LIMIT 20
        "#,
        [],
    );
    let nodes = MonitoringNodeHealth::find_by_statement(stmt)
        .all(pool)
        .await?;

    Ok(Json(MonitoringOverviewResponse {
        summary,
        traces,
        nodes,
    }))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MonitoringRequestQuery {
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    pub tenant_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub produce_ai_key_id: Option<Uuid>,
    pub protocol: Option<String>,
    pub request_path: Option<String>,
    pub model: Option<String>,
    pub is_stream: Option<bool>,
    pub status: Option<String>,
    pub billing_status: Option<String>,
    pub error_category: Option<String>,
    pub route_type: Option<String>,
    pub provider: Option<String>,
    pub account_id: Option<Uuid>,
    pub node_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    pub client_request_id: Option<String>,
    pub upstream_request_id: Option<String>,
    pub attempt_kind: Option<String>,
    pub fallback_only: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
    pub bucket: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromQueryResult)]
pub struct MonitoringRequestItem {
    pub request_id: Uuid,
    pub client_request_id: Option<String>,
    pub protocol: String,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub produce_ai_key_id: Uuid,
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
    pub received_at: chrono::DateTime<chrono::Utc>,
    pub client_first_content_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_ms: Option<i64>,
    pub provider_ttft_ms: Option<i64>,
    pub provider_name: Option<String>,
    pub account_id: Option<Uuid>,
    pub node_id: Option<Uuid>,
    pub total_tokens: Option<i32>,
    pub amount: Option<String>,
    pub currency: Option<String>,
    pub has_fallback: bool,
}

#[derive(Debug, Serialize)]
pub struct MonitoringRequestPage {
    pub items: Vec<MonitoringRequestItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RequestCursor {
    received_at: chrono::DateTime<chrono::Utc>,
    request_id: Uuid,
}

fn encode_request_cursor(cursor: &RequestCursor) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor).expect("cursor serialization"))
}

fn decode_request_cursor(value: &str) -> Result<RequestCursor> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ApiError::BadRequest("monitoring_invalid_cursor".to_string()))?;
    serde_json::from_slice(&decoded)
        .map_err(|_| ApiError::BadRequest("monitoring_invalid_cursor".to_string()))
}

fn monitoring_range(
    query: &MonitoringRequestQuery,
    max_hours: u32,
) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
    let to = query.to.unwrap_or_else(chrono::Utc::now);
    let from = query
        .from
        .unwrap_or_else(|| to - chrono::Duration::hours(1));
    if from >= to {
        return Err(ApiError::BadRequest(
            "monitoring_invalid_time_range".to_string(),
        ));
    }
    if to - from > chrono::Duration::hours(max_hours.clamp(1, 24) as i64) {
        return Err(ApiError::BadRequest(
            "monitoring_raw_range_exceeded".to_string(),
        ));
    }
    Ok((from, to))
}

fn push_value(values: &mut Vec<Value>, value: Value) -> usize {
    values.push(value);
    values.len()
}

fn request_filters(
    query: &MonitoringRequestQuery,
    include_cursor: bool,
    max_hours: u32,
) -> Result<(String, String, Vec<Value>)> {
    let (from, to) = monitoring_range(query, max_hours)?;
    let mut values = vec![from.into(), to.into()];
    let mut clauses = vec![
        "gr.received_at >= $1".to_string(),
        "gr.received_at < $2".to_string(),
    ];
    macro_rules! exact {
        ($field:literal, $value:expr) => {
            if let Some(value) = $value {
                let n = push_value(&mut values, value.into());
                clauses.push(format!(concat!($field, " = ${}"), n));
            }
        };
    }
    exact!("gr.tenant_id", query.tenant_id);
    exact!("gr.user_id", query.user_id);
    exact!("gr.produce_ai_key_id", query.produce_ai_key_id);
    exact!("gr.protocol", query.protocol.clone());
    exact!("gr.request_path", query.request_path.clone());
    exact!("gr.requested_model", query.model.clone());
    exact!("gr.is_stream", query.is_stream);
    exact!("gr.status", query.status.clone());
    exact!("gr.billing_status", query.billing_status.clone());
    exact!("gr.error_category", query.error_category.clone());
    exact!("gr.route_type", query.route_type.clone());
    exact!("gr.request_id", query.request_id);
    exact!("gr.client_request_id", query.client_request_id.clone());
    let mut attempt_clauses = Vec::new();
    for (column, value) in [
        ("a.provider_name", query.provider.clone().map(Value::from)),
        ("a.account_id", query.account_id.map(Value::from)),
        ("a.node_id", query.node_id.map(Value::from)),
        (
            "a.upstream_request_id",
            query.upstream_request_id.clone().map(Value::from),
        ),
        (
            "a.attempt_kind",
            query.attempt_kind.clone().map(Value::from),
        ),
    ] {
        if let Some(value) = value {
            let n = push_value(&mut values, value);
            attempt_clauses.push(format!("{column}=${n}"));
        }
    }
    if query.fallback_only.unwrap_or(false) {
        attempt_clauses.push("a.attempt_kind='fallback'".to_string());
    }
    let attempt_filters = attempt_clauses.join(" AND ");
    if !attempt_filters.is_empty() {
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM gateway_request_attempts a WHERE a.request_id=gr.request_id AND {})",
            attempt_filters
        ));
    }
    if include_cursor && let Some(cursor) = &query.cursor {
        let cursor = decode_request_cursor(cursor)?;
        let at = push_value(&mut values, cursor.received_at.into());
        let id = push_value(&mut values, cursor.request_id.into());
        clauses.push(format!("(gr.received_at,gr.request_id) < (${at},${id})"));
    }
    Ok((clauses.join(" AND "), attempt_filters, values))
}

pub async fn list_monitoring_requests(
    State(state): State<AppState>,
    Query(query): Query<MonitoringRequestQuery>,
) -> Result<Json<MonitoringRequestPage>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;
    let (filters, _, mut values) =
        request_filters(&query, true, state.gateway_config.monitoring_raw_max_hours)?;
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let limit_parameter = push_value(&mut values, ((limit + 1) as i64).into());
    let sql = format!(
        r#"SELECT gr.request_id,gr.client_request_id,gr.protocol,gr.tenant_id,gr.user_id,gr.produce_ai_key_id,gr.request_path,gr.requested_model,gr.is_stream,
        gr.route_type,gr.status,gr.billing_status,gr.error_origin,gr.error_category,gr.error_code,gr.trace_quality,
        gr.received_at,gr.client_first_content_at,gr.finished_at,
        CASE WHEN gr.finished_at IS NULL THEN NULL ELSE (EXTRACT(EPOCH FROM (gr.finished_at-gr.received_at))*1000)::BIGINT END duration_ms,
        final_attempt.provider_ttft_ms,final_attempt.provider_name,final_attempt.account_id,final_attempt.node_id,
        ul.total_tokens,ul.user_amount::TEXT amount,ul.currency,
        EXISTS(SELECT 1 FROM gateway_request_attempts fa WHERE fa.request_id=gr.request_id AND fa.attempt_kind='fallback') has_fallback
      FROM gateway_requests gr
      LEFT JOIN usage_logs ul ON ul.request_id=gr.request_id
      LEFT JOIN LATERAL (SELECT provider_name,account_id,node_id,
          CASE WHEN first_content_at IS NULL THEN NULL ELSE (EXTRACT(EPOCH FROM (first_content_at-started_at))*1000)::BIGINT END provider_ttft_ms
        FROM gateway_request_attempts WHERE request_id=gr.request_id ORDER BY is_final DESC,attempt_no DESC LIMIT 1) final_attempt ON TRUE
      WHERE {filters} ORDER BY gr.received_at DESC,gr.request_id DESC LIMIT ${limit_parameter}"#
    );
    let mut items = MonitoringRequestItem::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .all(pool)
    .await?;
    let has_more = items.len() > limit as usize;
    if has_more {
        items.truncate(limit as usize);
    }
    let next_cursor = if has_more {
        items.last().map(|item| {
            encode_request_cursor(&RequestCursor {
                received_at: item.received_at,
                request_id: item.request_id,
            })
        })
    } else {
        None
    };
    Ok(Json(MonitoringRequestPage { items, next_cursor }))
}

#[derive(Debug, Clone, Serialize, FromQueryResult)]
pub struct MonitoringAttemptDetail {
    pub id: Uuid,
    pub attempt_no: i32,
    pub attempt_kind: String,
    pub route_type: String,
    pub model: String,
    pub status: String,
    pub is_final: bool,
    pub provider_name: Option<String>,
    pub account_id: Option<Uuid>,
    pub account_name: Option<String>,
    pub node_task_id: Option<Uuid>,
    pub node_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub lease_id: Option<Uuid>,
    pub node_name: Option<String>,
    pub upstream_request_id: Option<String>,
    pub http_status: Option<i32>,
    pub retryable: Option<bool>,
    pub error_origin: Option<String>,
    pub error_category: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub headers_received_at: Option<chrono::DateTime<chrono::Utc>>,
    pub first_content_at: Option<chrono::DateTime<chrono::Utc>>,
    pub stream_end_reason: Option<String>,
    pub stream_error_count: Option<i32>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize)]
pub struct MonitoringRequestDetail {
    pub request: MonitoringRequestItem,
    pub attempts: Vec<MonitoringAttemptDetail>,
    pub node_task: Option<serde_json::Value>,
    pub usage: Option<serde_json::Value>,
}

pub async fn get_monitoring_request(
    State(state): State<AppState>,
    Path(request_id): Path<Uuid>,
) -> Result<Json<MonitoringRequestDetail>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;
    // Detail lookup is exact and intentionally independent of the raw aggregation range.
    let request = MonitoringRequestItem::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, r#"SELECT gr.request_id,gr.client_request_id,gr.protocol,gr.tenant_id,gr.user_id,gr.produce_ai_key_id,gr.request_path,gr.requested_model,gr.is_stream,gr.route_type,gr.status,gr.billing_status,gr.error_origin,gr.error_category,gr.error_code,gr.trace_quality,gr.received_at,gr.client_first_content_at,gr.finished_at,CASE WHEN gr.finished_at IS NULL THEN NULL ELSE (EXTRACT(EPOCH FROM (gr.finished_at-gr.received_at))*1000)::BIGINT END duration_ms,fa.provider_ttft_ms,fa.provider_name,fa.account_id,fa.node_id,ul.total_tokens,ul.user_amount::TEXT amount,ul.currency,EXISTS(SELECT 1 FROM gateway_request_attempts x WHERE x.request_id=gr.request_id AND x.attempt_kind='fallback') has_fallback FROM gateway_requests gr LEFT JOIN usage_logs ul ON ul.request_id=gr.request_id LEFT JOIN LATERAL (SELECT provider_name,account_id,node_id,CASE WHEN first_content_at IS NULL THEN NULL ELSE (EXTRACT(EPOCH FROM (first_content_at-started_at))*1000)::BIGINT END provider_ttft_ms FROM gateway_request_attempts WHERE request_id=gr.request_id ORDER BY is_final DESC,attempt_no DESC LIMIT 1) fa ON TRUE WHERE gr.request_id=$1"#, [request_id.into()])).one(pool).await?.ok_or_else(|| ApiError::NotFound(format!("request {request_id}")))?;
    let attempts = MonitoringAttemptDetail::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT ga.id,ga.attempt_no,ga.attempt_kind,ga.route_type,ga.model,ga.status,ga.is_final,ga.provider_name,ga.account_id,a.name account_name,ga.node_task_id,ga.node_id,n.display_name node_name,ga.session_id,ga.lease_id,ga.upstream_request_id,ga.http_status,ga.retryable,ga.error_origin,ga.error_category,ga.error_code,ga.error_summary,ga.started_at,ga.headers_received_at,ga.first_content_at,ga.stream_end_reason,ga.stream_error_count,ga.finished_at FROM gateway_request_attempts ga LEFT JOIN accounts a ON a.id=ga.account_id LEFT JOIN nodes n ON n.id=ga.node_id WHERE ga.request_id=$1 ORDER BY ga.attempt_no", [request_id.into()])).all(pool).await?;
    let node_task = pool.query_one(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT jsonb_build_object('id',nt.id,'status',nt.status,'model',nt.model,'queued_at',nt.queued_at,'claimed_at',nt.claimed_at,'finished_at',nt.finished_at,'deadline_at',nt.deadline_at,'submissions',(SELECT COALESCE(jsonb_agg(jsonb_build_object('id',s.id,'lease_id',s.lease_id,'result_kind',s.result_kind,'action',s.action,'created_at',s.created_at) ORDER BY s.created_at),'[]'::jsonb) FROM node_task_submissions s WHERE s.task_id=nt.id)) AS value FROM node_tasks nt WHERE nt.request_id=$1", [request_id.into()])).await?.and_then(|row| row.try_get("", "value").ok());
    let usage = pool.query_one(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT jsonb_build_object('id',id,'status',status,'input_tokens',input_tokens,'output_tokens',output_tokens,'total_tokens',total_tokens,'amount',user_amount::TEXT,'currency',currency,'usage_source',usage_source) AS value FROM usage_logs WHERE request_id=$1", [request_id.into()])).await?.and_then(|row| row.try_get("", "value").ok());
    Ok(Json(MonitoringRequestDetail {
        request,
        attempts,
        node_task,
        usage,
    }))
}

#[derive(Debug, Serialize, FromQueryResult)]
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

#[derive(Debug, Serialize)]
pub struct MonitoringSummaryResponse {
    pub summary: MonitoringTraceSummary,
    pub success_rate: Option<f64>,
    pub error_rate: Option<f64>,
    pub fallback_rate: Option<f64>,
    pub attempt_success_rate: Option<f64>,
    pub series: Vec<serde_json::Value>,
}

fn optional_ratio(numerator: i64, denominator: i64) -> Option<f64> {
    (denominator != 0).then(|| numerator as f64 / denominator as f64)
}

const USAGE_AMOUNT_BY_CURRENCY_SQL: &str = "SELECT ul.currency,SUM(ul.user_amount)::TEXT amount FROM usage_logs ul JOIN filtered f ON f.request_id=ul.request_id GROUP BY ul.currency";
const SERIES_AMOUNT_BY_CURRENCY_SQL: &str = "SELECT bucket,currency,SUM(user_amount)::TEXT amount FROM series_base WHERE currency IS NOT NULL GROUP BY bucket,currency";

pub async fn get_monitoring_summary(
    State(state): State<AppState>,
    Query(query): Query<MonitoringRequestQuery>,
) -> Result<Json<MonitoringSummaryResponse>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;
    let (filters, attempt_filters, values) =
        request_filters(&query, false, state.gateway_config.monitoring_raw_max_hours)?;
    // Request-level figures retain their documented "request has a matching
    // attempt" semantics. Attempt figures must only aggregate the matching
    // attempts themselves; otherwise filtering for provider A includes a
    // fallback attempt to provider B from the same request.
    let attempt_filter_clause = if attempt_filters.is_empty() {
        String::new()
    } else {
        format!(" WHERE {attempt_filters}")
    };
    let sql = format!(
        r#"WITH filtered AS (SELECT gr.* FROM gateway_requests gr WHERE {filters}), request_stats AS (
      SELECT COUNT(*)::BIGINT request_count,COUNT(*) FILTER(WHERE status='succeeded')::BIGINT succeeded_count,
       COUNT(*) FILTER(WHERE status IN ('failed','timed_out','cancelled'))::BIGINT failed_count,
       COUNT(*) FILTER(WHERE status IN ('received','routing','running'))::BIGINT active_count,COUNT(*) FILTER(WHERE status='queued')::BIGINT queued_count,
       COUNT(*) FILTER(WHERE EXISTS(SELECT 1 FROM gateway_request_attempts a WHERE a.request_id=filtered.request_id AND a.attempt_kind='fallback'))::BIGINT fallback_request_count,
       COUNT(*) FILTER(WHERE EXISTS(SELECT 1 FROM gateway_request_attempts a WHERE a.request_id=filtered.request_id AND a.route_type='provider_account'))::BIGINT provider_request_count,
       (percentile_cont(0.5) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(finished_at-received_at))*1000) FILTER(WHERE finished_at IS NOT NULL))::DOUBLE PRECISION p50_duration_ms,
       (percentile_cont(0.95) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(finished_at-received_at))*1000) FILTER(WHERE finished_at IS NOT NULL))::DOUBLE PRECISION p95_duration_ms,
       (percentile_cont(0.99) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(finished_at-received_at))*1000) FILTER(WHERE finished_at IS NOT NULL))::DOUBLE PRECISION p99_duration_ms
      FROM filtered), attempt_stats AS (SELECT COUNT(*)::BIGINT attempt_count,
       COUNT(*) FILTER(WHERE a.status<>'running')::BIGINT terminal_attempt_count,
       COUNT(*) FILTER(WHERE a.status='succeeded')::BIGINT succeeded_attempt_count,
       (percentile_cont(0.5) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(a.first_content_at-a.started_at))*1000) FILTER(WHERE a.first_content_at IS NOT NULL AND a.route_type='provider_account'))::DOUBLE PRECISION p50_provider_ttft_ms,
       (percentile_cont(0.95) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(a.first_content_at-a.started_at))*1000) FILTER(WHERE a.first_content_at IS NOT NULL AND a.route_type='provider_account'))::DOUBLE PRECISION p95_provider_ttft_ms
      FROM gateway_request_attempts a JOIN filtered f ON f.request_id=a.request_id{attempt_filter_clause}), node_stats AS (SELECT
       (percentile_cont(0.5) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(nt.claimed_at-nt.queued_at))*1000) FILTER(WHERE nt.claimed_at IS NOT NULL))::DOUBLE PRECISION p50_node_queue_ms,
       (percentile_cont(0.95) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(nt.claimed_at-nt.queued_at))*1000) FILTER(WHERE nt.claimed_at IS NOT NULL))::DOUBLE PRECISION p95_node_queue_ms,
       (percentile_cont(0.5) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(nt.finished_at-nt.claimed_at))*1000) FILTER(WHERE nt.finished_at IS NOT NULL AND nt.claimed_at IS NOT NULL))::DOUBLE PRECISION p50_node_execution_ms,
       (percentile_cont(0.95) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(nt.finished_at-nt.claimed_at))*1000) FILTER(WHERE nt.finished_at IS NOT NULL AND nt.claimed_at IS NOT NULL))::DOUBLE PRECISION p95_node_execution_ms
      FROM node_tasks nt JOIN filtered f ON f.request_id=nt.request_id), usage_by_currency AS (
       {USAGE_AMOUNT_BY_CURRENCY_SQL}
      ), usage_stats AS (SELECT
       (SELECT SUM(ul.total_tokens)::BIGINT FROM usage_logs ul JOIN filtered f ON f.request_id=ul.request_id) total_tokens,
       COALESCE((SELECT jsonb_object_agg(currency,amount) FROM usage_by_currency),'{{}}'::jsonb) amounts_by_currency)
      SELECT * FROM request_stats CROSS JOIN attempt_stats CROSS JOIN node_stats CROSS JOIN usage_stats"#
    );
    let summary = MonitoringTraceSummary::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values.clone(),
    ))
    .one(pool)
    .await?
    .ok_or_else(|| ApiError::Internal("summary unavailable".to_string()))?;
    // Queued/running requests and attempts have not produced an outcome yet;
    // including them in success/error denominators makes health appear worse
    // during normal concurrency or queue buildup.
    let terminal_request_count = summary.succeeded_count + summary.failed_count;
    let success_rate = optional_ratio(summary.succeeded_count, terminal_request_count);
    let error_rate = optional_ratio(summary.failed_count, terminal_request_count);
    let fallback_rate = optional_ratio(
        summary.fallback_request_count,
        summary.provider_request_count,
    );
    let attempt_success_rate = optional_ratio(
        summary.succeeded_attempt_count,
        summary.terminal_attempt_count,
    );
    let bucket_secs = match query.bucket.as_deref().unwrap_or("5m") {
        "1m" => 60,
        "5m" => 300,
        "1h" => 3600,
        _ => {
            return Err(ApiError::BadRequest(
                "monitoring_invalid_bucket".to_string(),
            ));
        }
    };
    let mut series_values = values;
    let bucket_parameter = push_value(&mut series_values, bucket_secs.into());
    let series_sql = format!(
        r#"WITH series_base AS (
          SELECT to_timestamp(floor(EXTRACT(EPOCH FROM gr.received_at)/${bucket_parameter})*${bucket_parameter}) bucket,
                 gr.status,ul.total_tokens,ul.user_amount,ul.currency
          FROM gateway_requests gr LEFT JOIN usage_logs ul ON ul.request_id=gr.request_id
          WHERE {filters}
        ), series_stats AS (
          SELECT bucket,COUNT(*)::BIGINT requests,
                 COUNT(*) FILTER(WHERE status='succeeded')::BIGINT succeeded,
                 SUM(total_tokens)::BIGINT tokens
          FROM series_base GROUP BY bucket
        ), series_currency AS (
          {SERIES_AMOUNT_BY_CURRENCY_SQL}
        ), series_amounts AS (
          SELECT bucket,jsonb_object_agg(currency,amount) amounts_by_currency
          FROM series_currency GROUP BY bucket
        )
        SELECT stats.bucket,stats.requests,stats.succeeded,stats.tokens,
               COALESCE(amounts.amounts_by_currency,'{{}}'::jsonb) amounts_by_currency
        FROM series_stats stats LEFT JOIN series_amounts amounts USING(bucket)
        ORDER BY stats.bucket"#
    );
    let rows = pool
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            series_sql,
            series_values,
        ))
        .await?;
    let series=rows.into_iter().map(|row| serde_json::json!({"bucket":row.try_get::<chrono::DateTime<chrono::Utc>>("","bucket").ok(),"requests":row.try_get::<i64>("","requests").unwrap_or(0),"succeeded":row.try_get::<i64>("","succeeded").unwrap_or(0),"tokens":row.try_get::<i64>("","tokens").ok(),"amounts_by_currency":row.try_get::<serde_json::Value>("","amounts_by_currency").unwrap_or_else(|_|serde_json::json!({}))})).collect();
    Ok(Json(MonitoringSummaryResponse {
        summary,
        success_rate,
        error_rate,
        fallback_rate,
        attempt_success_rate,
        series,
    }))
}

#[derive(Debug, Serialize)]
pub struct MonitoringTargetHealthResponse {
    pub providers: Vec<serde_json::Value>,
    pub nodes: Vec<serde_json::Value>,
}

fn unassigned_queue_health(queued: i64) -> serde_json::Value {
    serde_json::json!({
        "id": null,
        "display_name": null,
        "is_unassigned": true,
        "status": "queued",
        "last_heartbeat_at": null,
        "session_expires_at": null,
        "accepted_models": [],
        "queued": queued,
        "running": 0,
        "succeeded": 0,
        "failed": 0,
        "expired": 0,
    })
}

const PROVIDER_HEALTH_SQL: &str = r#"
SELECT a.id,a.name,a.provider,a.enabled,
       a.last_probe_at,a.last_probe_latency_ms,a.last_probe_status,a.last_probe_error_code,
       COUNT(ga.id) FILTER (
           WHERE ga.status='succeeded' OR ga.error_origin='upstream'
       )::BIGINT attempts,
       COUNT(ga.id) FILTER (WHERE ga.status='succeeded')::BIGINT succeeded,
       (COUNT(ga.id) FILTER (WHERE ga.status='succeeded'))::DOUBLE PRECISION
         / NULLIF(COUNT(ga.id) FILTER (
             WHERE ga.status='succeeded' OR ga.error_origin='upstream'
           ),0)::DOUBLE PRECISION success_rate,
       COUNT(ga.id) FILTER (
           WHERE ga.status<>'succeeded' AND ga.error_origin='upstream'
       )::BIGINT attributable_failures,
       AVG(EXTRACT(EPOCH FROM(ga.finished_at-ga.started_at))*1000) FILTER (
           WHERE ga.status='succeeded' OR ga.error_origin='upstream'
       )::DOUBLE PRECISION avg_latency_ms
FROM accounts a
LEFT JOIN gateway_request_attempts ga
  ON ga.account_id=a.id AND ga.started_at >= $1 AND ga.started_at < $2
GROUP BY a.id
ORDER BY a.name
"#;

const NODE_HEALTH_SQL: &str = r#"
SELECT n.id,
       n.display_name,
       CASE
           WHEN n.status = 'online'
                AND (
                    n.last_heartbeat_at IS NULL
                    OR n.last_heartbeat_at < NOW() - INTERVAL '3 minutes'
                    OR active_session.expires_at IS NULL
                )
           THEN 'offline'
           ELSE n.status
       END status,
       n.last_heartbeat_at,
       active_session.expires_at,
       COALESCE(active_session.accepted_models_json, '[]'::jsonb) accepted_models,
       COUNT(nt.id) FILTER (WHERE nt.status='queued')::BIGINT queued,
       COUNT(nt.id) FILTER (WHERE nt.status='leased')::BIGINT running,
       COUNT(nt.id) FILTER (WHERE nt.status IN ('succeeded','image_succeeded'))::BIGINT succeeded,
       COUNT(nt.id) FILTER (WHERE nt.status='failed')::BIGINT failed,
       COUNT(nt.id) FILTER (WHERE nt.status='expired')::BIGINT expired
FROM nodes n
LEFT JOIN LATERAL (
    SELECT expires_at, accepted_models_json
    FROM node_sessions
    WHERE node_id = n.id
      AND revoked_at IS NULL
      AND expires_at > NOW()
    ORDER BY last_seen_at DESC
    LIMIT 1
) active_session ON TRUE
LEFT JOIN node_tasks nt
  ON nt.assigned_node_id = n.id
 AND nt.created_at >= $1
 AND nt.created_at < $2
GROUP BY n.id, active_session.expires_at, active_session.accepted_models_json
ORDER BY n.display_name
"#;

pub async fn get_monitoring_target_health(
    State(state): State<AppState>,
    Query(query): Query<MonitoringRequestQuery>,
) -> Result<Json<MonitoringTargetHealthResponse>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;
    let (from, to) = monitoring_range(&query, state.gateway_config.monitoring_raw_max_hours)?;
    let provider_rows = pool
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            PROVIDER_HEALTH_SQL,
            [from.into(), to.into()],
        ))
        .await?;
    let providers=provider_rows.into_iter().map(|r|serde_json::json!({"id":r.try_get::<Uuid>("","id").ok(),"name":r.try_get::<String>("","name").ok(),"provider":r.try_get::<String>("","provider").ok(),"enabled":r.try_get::<bool>("","enabled").ok(),"attempts":r.try_get::<i64>("","attempts").unwrap_or(0),"succeeded":r.try_get::<i64>("","succeeded").unwrap_or(0),"success_rate":r.try_get::<f64>("","success_rate").ok(),"attributable_failures":r.try_get::<i64>("","attributable_failures").unwrap_or(0),"avg_latency_ms":r.try_get::<f64>("","avg_latency_ms").ok(),"last_probe_at":r.try_get::<chrono::DateTime<chrono::Utc>>("","last_probe_at").ok(),"last_probe_latency_ms":r.try_get::<i64>("","last_probe_latency_ms").ok(),"last_probe_status":r.try_get::<String>("","last_probe_status").ok(),"last_probe_error_code":r.try_get::<String>("","last_probe_error_code").ok()})).collect();
    let node_rows = pool
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            NODE_HEALTH_SQL,
            [from.into(), to.into()],
        ))
        .await?;
    let mut nodes=node_rows.into_iter().map(|r|serde_json::json!({"id":r.try_get::<Uuid>("","id").ok(),"display_name":r.try_get::<String>("","display_name").ok(),"status":r.try_get::<String>("","status").ok(),"last_heartbeat_at":r.try_get::<chrono::DateTime<chrono::Utc>>("","last_heartbeat_at").ok(),"session_expires_at":r.try_get::<chrono::DateTime<chrono::Utc>>("","expires_at").ok(),"accepted_models":r.try_get::<serde_json::Value>("","accepted_models").unwrap_or_else(|_|serde_json::json!([])),"queued":r.try_get::<i64>("","queued").unwrap_or(0),"running":r.try_get::<i64>("","running").unwrap_or(0),"succeeded":r.try_get::<i64>("","succeeded").unwrap_or(0),"failed":r.try_get::<i64>("","failed").unwrap_or(0),"expired":r.try_get::<i64>("","expired").unwrap_or(0)})).collect::<Vec<_>>();
    let unassigned_queued=pool.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT COUNT(*)::BIGINT count FROM node_tasks WHERE assigned_node_id IS NULL AND status='queued' AND created_at >= $1 AND created_at < $2",[from.into(),to.into()])).await?.and_then(|row|row.try_get::<i64>("","count").ok()).unwrap_or(0);
    if unassigned_queued > 0 {
        nodes.insert(0, unassigned_queue_health(unassigned_queued));
    }
    Ok(Json(MonitoringTargetHealthResponse { providers, nodes }))
}

#[derive(Debug, Deserialize)]
pub struct BatchProbeRequest {
    pub account_ids: Option<Vec<Uuid>>,
}

const MAX_BATCH_PROBE_ACCOUNTS: usize = 50;

fn normalize_probe_account_ids(
    account_ids: Vec<Uuid>,
    max_accounts: Option<usize>,
) -> Result<Vec<Uuid>> {
    let mut seen = HashSet::with_capacity(
        max_accounts
            .map(|max| account_ids.len().min(max + 1))
            .unwrap_or(account_ids.len()),
    );
    let account_ids = account_ids
        .into_iter()
        .filter(|account_id| seen.insert(*account_id))
        .collect::<Vec<_>>();
    if max_accounts.is_some_and(|max| account_ids.len() > max) {
        return Err(ApiError::BadRequest(
            "monitoring_probe_batch_too_large".to_string(),
        ));
    }
    Ok(account_ids)
}

pub async fn probe_monitoring_targets(
    State(state): State<AppState>,
    Json(request): Json<BatchProbeRequest>,
) -> Result<Json<serde_json::Value>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not configured".to_string()))?;
    let (account_ids, enabled_only) = match request.account_ids {
        Some(ids) => (
            normalize_probe_account_ids(ids, Some(MAX_BATCH_PROBE_ACCOUNTS))?,
            false,
        ),
        None => (
            normalize_probe_account_ids(
                keycompute_db::Account::find_enabled_all(pool.write_conn())
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(|account| account.id)
                    .collect(),
                None,
            )?,
            true,
        ),
    };
    let results = stream::iter(account_ids.into_iter().map(|account_id| {
        let state = state.clone();
        async move {
            let result = if enabled_only {
                crate::handlers::admin_account::probe_enabled_account_for_monitoring(
                    &state, account_id,
                )
                .await
            } else {
                crate::handlers::admin_account::probe_account_for_monitoring(&state, account_id)
                    .await
                    .map(Some)
            };
            match result {
                Ok(Some(value)) => serde_json::json!({
                    "account_id": account_id,
                    "success": value.get("success").and_then(|value| value.as_bool()).unwrap_or(false),
                    "result": value,
                }),
                Ok(None) => serde_json::json!({
                    "account_id": account_id,
                    "success": false,
                    "skipped": true,
                    "reason": "account_disabled_or_deleted",
                }),
                Err(error) => {
                    tracing::warn!(%account_id, %error, "manual account probe failed");
                    serde_json::json!({
                        "account_id": account_id,
                        "success": false,
                        "result": null,
                    })
                }
            }
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    Ok(Json(serde_json::json!({"results":results})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_cursor_is_opaque_stable_and_round_trips() {
        let cursor = RequestCursor {
            received_at: chrono::DateTime::parse_from_rfc3339("2026-08-21T12:34:56Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            request_id: Uuid::parse_str("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
        };
        let encoded = encode_request_cursor(&cursor);
        assert!(!encoded.contains(' '));
        assert_eq!(encoded, encode_request_cursor(&cursor));
        let decoded = decode_request_cursor(&encoded).unwrap();
        assert_eq!(decoded.received_at, cursor.received_at);
        assert_eq!(decoded.request_id, cursor.request_id);
        assert!(decode_request_cursor("not a cursor").is_err());
    }

    #[test]
    fn monitoring_range_is_half_open_and_honors_24_hour_ceiling() {
        let to = chrono::Utc::now();
        let query = MonitoringRequestQuery {
            from: Some(to - chrono::Duration::hours(24)),
            to: Some(to),
            ..Default::default()
        };
        assert_eq!(
            monitoring_range(&query, 48).unwrap(),
            (query.from.unwrap(), to)
        );

        let too_wide = MonitoringRequestQuery {
            from: Some(to - chrono::Duration::hours(24) - chrono::Duration::seconds(1)),
            to: Some(to),
            ..Default::default()
        };
        assert!(matches!(
            monitoring_range(&too_wide, 48),
            Err(ApiError::BadRequest(code)) if code == "monitoring_raw_range_exceeded"
        ));
    }

    #[test]
    fn provider_health_only_uses_upstream_attributable_attempts() {
        let predicate = "ga.status='succeeded' OR ga.error_origin='upstream'";
        assert_eq!(PROVIDER_HEALTH_SQL.matches(predicate).count(), 3);
        assert!(PROVIDER_HEALTH_SQL.contains("AVG(EXTRACT"));
        assert!(PROVIDER_HEALTH_SQL.contains("FILTER ("));
    }

    #[test]
    fn node_health_only_advertises_fresh_active_sessions() {
        assert!(NODE_HEALTH_SQL.contains("n.last_heartbeat_at < NOW() - INTERVAL '3 minutes'"));
        assert!(NODE_HEALTH_SQL.contains("revoked_at IS NULL"));
        assert!(NODE_HEALTH_SQL.contains("expires_at > NOW()"));
        assert!(NODE_HEALTH_SQL.contains("active_session.expires_at IS NULL"));
        assert!(NODE_HEALTH_SQL.contains(
            "COALESCE(active_session.accepted_models_json, '[]'::jsonb) accepted_models"
        ));
    }

    #[test]
    fn node_health_counts_chat_and_image_completions_as_successes() {
        assert!(NODE_HEALTH_SQL.contains("nt.status IN ('succeeded','image_succeeded')"));
    }

    #[test]
    fn success_rates_only_use_terminal_outcomes() {
        assert_eq!(optional_ratio(8, 10), Some(0.8));
        assert_eq!(optional_ratio(0, 0), None);
        let succeeded = 8;
        let failed = 2;
        let active_or_queued = 90;
        assert_eq!(optional_ratio(succeeded, succeeded + failed), Some(0.8));
        assert_ne!(
            optional_ratio(succeeded, succeeded + failed + active_or_queued),
            Some(0.8)
        );
    }

    #[test]
    fn unassigned_queue_health_uses_a_localizable_marker() {
        let value = unassigned_queue_health(3);
        assert_eq!(value["is_unassigned"], true);
        assert_eq!(value["queued"], 3);
        assert!(value["display_name"].is_null());
        assert!(!value.to_string().contains("未分配"));
    }

    #[test]
    fn attempt_filters_apply_to_the_same_attempt() {
        let query = MonitoringRequestQuery {
            provider: Some("openai".to_string()),
            attempt_kind: Some("fallback".to_string()),
            ..Default::default()
        };
        let (filters, attempt_filters, _) = request_filters(&query, false, 24).unwrap();
        assert_eq!(
            filters
                .matches("EXISTS (SELECT 1 FROM gateway_request_attempts a")
                .count(),
            1
        );
        assert!(filters.contains("a.provider_name=$3 AND a.attempt_kind=$4"));
        assert_eq!(attempt_filters, "a.provider_name=$3 AND a.attempt_kind=$4");
    }

    #[test]
    fn attempt_summary_filters_do_not_include_other_attempts_from_a_request() {
        let query = MonitoringRequestQuery {
            provider: Some("primary".to_string()),
            ..Default::default()
        };
        let (filters, attempt_filters, _) = request_filters(&query, false, 24).unwrap();
        let attempt_filter_clause = format!(" WHERE {attempt_filters}");

        assert!(filters.contains("EXISTS (SELECT 1 FROM gateway_request_attempts a"));
        assert_eq!(attempt_filter_clause, " WHERE a.provider_name=$3");
        assert!(
            format!(
                "FROM gateway_request_attempts a JOIN filtered f ON f.request_id=a.request_id{attempt_filter_clause}"
            )
            .contains("WHERE a.provider_name=$3")
        );
    }

    #[test]
    fn batch_probe_deduplicates_and_limits_accounts() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        assert_eq!(
            normalize_probe_account_ids(
                vec![first, second, first],
                Some(MAX_BATCH_PROBE_ACCOUNTS),
            )
            .unwrap(),
            vec![first, second]
        );

        let all_enabled_accounts = (0..=MAX_BATCH_PROBE_ACCOUNTS)
            .map(|value| Uuid::from_u128(value as u128 + 1))
            .collect::<Vec<_>>();
        assert!(matches!(
            normalize_probe_account_ids(
                all_enabled_accounts.clone(),
                Some(MAX_BATCH_PROBE_ACCOUNTS),
            ),
            Err(ApiError::BadRequest(code)) if code == "monitoring_probe_batch_too_large"
        ));
        assert_eq!(
            normalize_probe_account_ids(all_enabled_accounts, None)
                .expect("server-selected accounts are not an untrusted request batch")
                .len(),
            MAX_BATCH_PROBE_ACCOUNTS + 1
        );
    }

    #[test]
    fn monetary_summaries_group_each_currency_independently() {
        assert!(USAGE_AMOUNT_BY_CURRENCY_SQL.contains("GROUP BY ul.currency"));
        assert!(SERIES_AMOUNT_BY_CURRENCY_SQL.contains("GROUP BY bucket,currency"));
        assert!(!USAGE_AMOUNT_BY_CURRENCY_SQL.contains("total_amount"));
        assert!(!SERIES_AMOUNT_BY_CURRENCY_SQL.contains("total_amount"));
    }
}
