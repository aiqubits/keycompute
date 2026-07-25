use client_api::api::admin::{MonitoringOverviewResponse, MonitoringTraceEntry};
use dioxus::prelude::*;
use ui::{Badge, BadgeVariant};

use crate::hooks::use_i18n::use_i18n;
use crate::services::{api_client::with_auto_refresh, monitoring_service};
use crate::stores::{auth_store::AuthStore, user_store::UserStore};
use crate::views::shared::accounts::NoPermissionView;

#[component]
pub fn Monitoring() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    if !is_admin {
        return rsx! {
            NoPermissionView { resource: i18n.t("page.monitoring").to_string() }
        };
    }

    let overview = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            monitoring_service::overview(&token).await
        })
        .await
    });

    rsx! {
        div { class: "page-container monitoring-page",
            div { class: "page-header",
                div {
                    h1 { class: "page-title", {i18n.t("page.monitoring")} }
                    p { class: "page-description", {i18n.t("monitoring.subtitle")} }
                }
            }

            match overview() {
                None => rsx! {
                    p { class: "text-secondary", {i18n.t("table.loading")} }
                },
                Some(Err(ref e)) => rsx! {
                    div { class: "alert alert-error",
                        p { "{i18n.t(\"common.load_failed\")}: {e}" }
                    }
                },
                Some(Ok(ref data)) => rsx! {
                    MonitoringConsole { data: data.clone() }
                },
            }
        }
    }
}

#[component]
fn MonitoringConsole(data: MonitoringOverviewResponse) -> Element {
    let i18n = use_i18n();
    let initial_trace_id = data
        .traces
        .first()
        .map(|trace| trace.request_id.clone())
        .unwrap_or_default();
    let mut selected_trace_id = use_signal(|| initial_trace_id);
    let selected_id = selected_trace_id();
    let selected_trace = data
        .traces
        .iter()
        .find(|trace| trace.request_id == selected_id)
        .or_else(|| data.traces.first())
        .cloned();
    let total_tokens: i32 = data
        .traces
        .iter()
        .filter_map(|trace| trace.total_tokens)
        .sum();

    rsx! {
        div { class: "monitoring-toolbar",
            div {
                p { class: "monitoring-kicker", {i18n.t("monitoring.control_plane")} }
                p { class: "monitoring-toolbar-copy", {i18n.t("monitoring.control_plane_desc")} }
            }
            Badge { variant: BadgeVariant::Neutral, {i18n.t("monitoring.read_only")} }
        }

        div { class: "monitoring-stat-grid",
            StatCard {
                label: i18n.t("monitoring.total_node_tasks").to_string(),
                value: data.summary.total_node_tasks.to_string(),
                meta: i18n.t("monitoring.total_usage_logs").to_string(),
            }
            StatCard {
                label: i18n.t("monitoring.succeeded_tasks").to_string(),
                value: data.summary.succeeded_node_tasks.to_string(),
                meta: i18n.t("monitoring.succeeded_tasks_desc").to_string(),
            }
            StatCard {
                label: i18n.t("monitoring.failed_tasks").to_string(),
                value: data.summary.failed_node_tasks.to_string(),
                meta: i18n.t("monitoring.active_tasks_desc").to_string(),
            }
            StatCard {
                label: i18n.t("monitoring.avg_latency").to_string(),
                value: format_duration(data.summary.avg_node_latency_ms),
                meta: i18n.t("monitoring.avg_latency_desc").to_string(),
            }
            StatCard {
                label: i18n.t("monitoring.tokens").to_string(),
                value: compact_number(total_tokens as i64),
                meta: i18n.t("monitoring.total_usage_logs").to_string(),
            }
        }

        div { class: "monitoring-trace-shell",
            aside { class: "monitoring-request-pane",
                div { class: "monitoring-pane-head",
                    h2 { class: "section-title", {i18n.t("monitoring.traces_title")} }
                }
                div { class: "monitoring-request-list",
                    if data.traces.is_empty() {
                        div { class: "table-empty-content",
                            div { class: "table-empty-mark" }
                            p { class: "table-empty-text", {i18n.t("monitoring.no_traces")} }
                        }
                    }
                    for trace in data.traces.iter() {
                        RequestListItem {
                            trace: trace.clone(),
                            selected: selected_trace
                                .as_ref()
                                .map(|active| active.request_id == trace.request_id)
                                .unwrap_or(false),
                            onselect: move |request_id| selected_trace_id.set(request_id),
                        }
                    }
                }
            }

            main { class: "monitoring-detail-pane",
                if let Some(trace) = selected_trace {
                    TraceDetail { trace: trace.clone() }
                    TraceMap { trace: trace.clone() }
                    div { class: "monitoring-detail-bottom",
                        RequestPayload { trace: trace.clone() }
                        BasicInfo { trace: trace.clone() }
                        MetricInfo { trace: trace.clone() }
                    }
                } else {
                    div { class: "table-empty-content monitoring-empty-detail",
                        div { class: "table-empty-mark" }
                        p { class: "table-empty-text", {i18n.t("monitoring.no_traces")} }
                    }
                }
            }
        }

        div { class: "monitoring-node-strip",
            div { class: "monitoring-pane-head",
                h2 { class: "section-title", {i18n.t("monitoring.health_title")} }
            }
            div { class: "monitoring-health-list",
                if data.nodes.is_empty() {
                    div { class: "table-empty-content",
                        div { class: "table-empty-mark" }
                        p { class: "table-empty-text", {i18n.t("monitoring.no_nodes")} }
                    }
                }
                for node in data.nodes.iter() {
                    div { class: "monitoring-node-card",
                        div { class: "monitoring-node-head",
                            div {
                                p { class: "monitoring-node-name", "{node.display_name}" }
                                p { class: "monitoring-node-meta", "{short_id(&node.id)}" }
                            }
                            NodeStatusBadge { status: node.status.clone() }
                        }
                        div { class: "monitoring-node-stats",
                            span { "{i18n.t(\"monitoring.active\")}: {node.active_tasks}" }
                            span { "{i18n.t(\"monitoring.succeeded\")}: {node.succeeded_tasks}" }
                            span { "{i18n.t(\"monitoring.failed\")}: {node.failed_tasks}" }
                        }
                        div { class: "account-models",
                            for model in accepted_models(&node.accepted_models_json).iter() {
                                span { class: "account-model-chip", "{model}" }
                            }
                            if accepted_models(&node.accepted_models_json).is_empty() {
                                span { class: "account-model-chip account-model-chip-muted",
                                    {i18n.t("node_gateway.no_models")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StatCard(label: String, value: String, meta: String) -> Element {
    rsx! {
        div { class: "stat-card",
            p { class: "stat-label", "{label}" }
            p { class: "stat-value", "{value}" }
            p { class: "stat-meta", "{meta}" }
        }
    }
}

#[component]
fn RequestListItem(
    trace: MonitoringTraceEntry,
    selected: bool,
    onselect: EventHandler<String>,
) -> Element {
    let cls = if selected {
        "monitoring-request-item active"
    } else {
        "monitoring-request-item"
    };
    let request_id = trace.request_id.clone();
    rsx! {
        button {
            class: "{cls}",
            r#type: "button",
            onclick: move |_| onselect.call(request_id.clone()),
            div {
                span { class: "monitoring-request-id", "{short_id(&trace.request_id)}" }
                p { class: "monitoring-request-node", "{trace.node_name.as_deref().unwrap_or(\"-\")}" }
                p { class: "monitoring-request-time", "{compact_time(&trace.queued_at)}" }
            }
            TaskStatusBadge { status: trace.status.clone() }
            span { class: "monitoring-request-duration", "{format_duration(trace.duration_ms)}" }
        }
    }
}

#[component]
fn TraceDetail(trace: MonitoringTraceEntry) -> Element {
    let i18n = use_i18n();
    rsx! {
        section { class: "monitoring-detail-card monitoring-detail-summary",
            div {
                div { class: "monitoring-detail-title-row",
                    h2 { class: "section-title", {i18n.t("monitoring.records_title")} }
                    TaskStatusBadge { status: trace.status.clone() }
                }
                p { class: "monitoring-detail-id", "{short_id(&trace.request_id)}" }
            }
            div { class: "monitoring-detail-meta",
                span { "{i18n.t(\"monitoring.queued_at\")}: {trace.queued_at}" }
                span { "{i18n.t(\"monitoring.duration\")}: {format_duration(trace.duration_ms)}" }
                span { "{i18n.t(\"monitoring.node\")}: {trace.node_name.as_deref().unwrap_or(\"-\")}" }
                span { "{i18n.t(\"pricing.model_name\")}: {trace.model}" }
            }
            div { class: "monitoring-timeline",
                TraceStage {
                    label: i18n.t("monitoring.stage_queued").to_string(),
                    value: compact_time(&trace.queued_at),
                    active: true,
                }
                TraceStage {
                    label: i18n.t("monitoring.stage_claimed").to_string(),
                    value: trace.claimed_at.as_deref().map(compact_time).unwrap_or_else(|| "-".to_string()),
                    active: trace.claimed_at.is_some(),
                }
                TraceStage {
                    label: i18n.t("monitoring.stage_finished").to_string(),
                    value: trace.finished_at.as_deref().map(compact_time).unwrap_or_else(|| "-".to_string()),
                    active: trace.finished_at.is_some(),
                }
                TraceStage {
                    label: i18n.t("monitoring.stage_usage").to_string(),
                    value: trace.usage_status.clone().unwrap_or_else(|| "-".to_string()),
                    active: trace.usage_status.is_some(),
                }
            }
        }
    }
}

#[component]
fn TraceMap(trace: MonitoringTraceEntry) -> Element {
    let i18n = use_i18n();
    rsx! {
        section { class: "monitoring-detail-card monitoring-map",
            Lane {
                title: i18n.t("monitoring.map_gateway").to_string(),
                subtitle: i18n.t("monitoring.map_gateway_subtitle").to_string(),
                MapPill {
                    label: i18n.t("monitoring.map_receive_request").to_string(),
                    value: compact_time(&trace.queued_at),
                    class_name: "at-start".to_string(),
                }
                MapPill {
                    label: i18n.t("monitoring.map_return_client").to_string(),
                    value: trace.finished_at.as_deref().map(compact_time).unwrap_or_else(|| "-".to_string()),
                    class_name: "at-end".to_string(),
                }
            }
            Lane {
                title: i18n.t("monitoring.map_router").to_string(),
                subtitle: i18n.t("monitoring.map_router_subtitle").to_string(),
                MapPill {
                    label: i18n.t("monitoring.map_match_route").to_string(),
                    value: trace.claimed_at.as_deref().map(compact_time).unwrap_or_else(|| "-".to_string()),
                    class_name: "at-mid".to_string(),
                }
            }
            Lane {
                title: i18n.t("monitoring.map_node").to_string(),
                subtitle: trace.node_name.clone().unwrap_or_else(|| "-".to_string()),
                MapPill {
                    label: i18n.t("monitoring.map_process_request").to_string(),
                    value: format_duration(trace.duration_ms),
                    class_name: "at-run".to_string(),
                }
                MapPill {
                    label: i18n.t("monitoring.map_submit_result").to_string(),
                    value: trace.last_submission_action.clone().unwrap_or_else(|| "-".to_string()),
                    class_name: "at-done".to_string(),
                }
            }
            Lane {
                title: i18n.t("monitoring.map_model_service").to_string(),
                subtitle: trace.model.clone(),
                MapPill {
                    label: i18n.t("monitoring.map_model_response").to_string(),
                    value: trace
                        .total_tokens
                        .map(|v| format!("{} tokens", v))
                        .unwrap_or_else(|| "-".to_string()),
                    class_name: "at-run".to_string(),
                }
            }
        }
    }
}

#[component]
fn Lane(title: String, subtitle: String, children: Element) -> Element {
    rsx! {
        div { class: "monitoring-map-lane",
            div { class: "monitoring-lane-head",
                span { class: "monitoring-lane-icon" }
                div {
                    p { class: "monitoring-lane-title", "{title}" }
                    p { class: "monitoring-lane-subtitle", "{subtitle}" }
                }
            }
            div { class: "monitoring-lane-body", {children} }
        }
    }
}

#[component]
fn MapPill(label: String, value: String, class_name: String) -> Element {
    rsx! {
        div { class: "monitoring-map-pill {class_name}",
            span { "{label}" }
            small { "{value}" }
        }
    }
}

#[component]
fn TraceStage(label: String, value: String, active: bool) -> Element {
    let cls = if active {
        "monitoring-trace-stage active"
    } else {
        "monitoring-trace-stage"
    };
    rsx! {
        div { class: "{cls}",
            span { class: "monitoring-stage-dot" }
            div {
                p { class: "monitoring-stage-label", "{label}" }
                p { class: "monitoring-stage-value", "{value}" }
            }
        }
    }
}

#[component]
fn RequestPayload(trace: MonitoringTraceEntry) -> Element {
    let i18n = use_i18n();
    rsx! {
        section { class: "monitoring-detail-card monitoring-json-card",
            div { class: "monitoring-card-head",
                h3 { {i18n.t("monitoring.request_payload")} }
            }
            pre { class: "monitoring-json-block",
                code { "{payload_json(&trace)}" }
            }
        }
    }
}

#[component]
fn BasicInfo(trace: MonitoringTraceEntry) -> Element {
    let i18n = use_i18n();
    rsx! {
        section { class: "monitoring-detail-card monitoring-info-card",
            div { class: "monitoring-card-head",
                h3 { {i18n.t("monitoring.basic_info")} }
            }
            InfoRow {
                label: i18n.t("monitoring.request").to_string(),
                value: short_id(&trace.request_id),
            }
            InfoRow {
                label: i18n.t("table.status").to_string(),
                value: trace.status.clone(),
            }
            InfoRow {
                label: i18n.t("monitoring.queued_at").to_string(),
                value: trace.queued_at.clone(),
            }
            InfoRow {
                label: i18n.t("monitoring.duration").to_string(),
                value: format_duration(trace.duration_ms),
            }
            InfoRow {
                label: i18n.t("monitoring.node").to_string(),
                value: trace.node_name.clone().unwrap_or_else(|| "-".to_string()),
            }
            InfoRow {
                label: i18n.t("pricing.model_name").to_string(),
                value: trace.model.clone(),
            }
        }
    }
}

#[component]
fn MetricInfo(trace: MonitoringTraceEntry) -> Element {
    let i18n = use_i18n();
    rsx! {
        section { class: "monitoring-detail-card monitoring-info-card",
            div { class: "monitoring-card-head",
                h3 { {i18n.t("monitoring.request_metrics")} }
            }
            InfoRow {
                label: i18n.t("monitoring.tokens").to_string(),
                value: trace.total_tokens.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()),
            }
            InfoRow {
                label: i18n.t("monitoring.submissions").to_string(),
                value: trace.submissions_count.to_string(),
            }
            InfoRow {
                label: i18n.t("monitoring.amount").to_string(),
                value: trace.amount.clone().unwrap_or_else(|| "-".to_string()),
            }
            InfoRow {
                label: i18n.t("monitoring.stage_usage").to_string(),
                value: trace.usage_status.clone().unwrap_or_else(|| "-".to_string()),
            }
        }
    }
}

#[component]
fn InfoRow(label: String, value: String) -> Element {
    rsx! {
        div { class: "monitoring-info-row",
            span { "{label}" }
            strong { "{value}" }
        }
    }
}

#[component]
fn NodeStatusBadge(status: String) -> Element {
    let i18n = use_i18n();
    let variant = match status.as_str() {
        "online" => BadgeVariant::Success,
        "excluded" => BadgeVariant::Error,
        "offline" => BadgeVariant::Warning,
        _ => BadgeVariant::Neutral,
    };
    let label = match status.as_str() {
        "online" => i18n.t("node_gateway.status_online"),
        "offline" => i18n.t("node_gateway.status_offline"),
        "excluded" => i18n.t("node_gateway.status_excluded"),
        _ => status.as_str(),
    };
    rsx! {
        Badge { variant, "{label}" }
    }
}

#[component]
fn TaskStatusBadge(status: String) -> Element {
    let i18n = use_i18n();
    let variant = match status.as_str() {
        "succeeded" => BadgeVariant::Success,
        "failed" | "expired" => BadgeVariant::Error,
        "leased" => BadgeVariant::Warning,
        "queued" => BadgeVariant::Neutral,
        _ => BadgeVariant::Neutral,
    };
    let label = match status.as_str() {
        "queued" => i18n.t("node_gateway.task_queued"),
        "leased" => i18n.t("node_gateway.task_leased"),
        "succeeded" => i18n.t("node_gateway.task_succeeded"),
        "failed" => i18n.t("node_gateway.task_failed"),
        "expired" => i18n.t("node_gateway.task_expired"),
        _ => status.as_str(),
    };
    rsx! {
        Badge { variant, "{label}" }
    }
}

fn accepted_models(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter_map(|model| model.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn format_duration(ms: Option<i64>) -> String {
    match ms {
        Some(value) if value >= 1000 => format!("{:.1}s", value as f64 / 1000.0),
        Some(value) => format!("{}ms", value),
        None => "—".to_string(),
    }
}

fn compact_number(value: i64) -> String {
    if value >= 1000 {
        format!("{:.1}K", value as f64 / 1000.0)
    } else {
        value.to_string()
    }
}

fn compact_time(value: &str) -> String {
    value
        .split('T')
        .nth(1)
        .unwrap_or(value)
        .trim_end_matches('Z')
        .chars()
        .take(12)
        .collect()
}

fn payload_json(trace: &MonitoringTraceEntry) -> String {
    let payload = serde_json::json!({
        "id": short_id(&trace.request_id),
        "method": "POST",
        "path": "/v1/chat/completions",
        "model": trace.model,
        "node": trace.node_name.as_deref().unwrap_or("-"),
        "status": trace.status,
    });

    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
