use client_api::api::admin::{
    MonitoringAttemptDetail, MonitoringQuery, MonitoringRequestItem, MonitoringSummaryResponse,
    MonitoringTargetHealthResponse,
};
use dioxus::prelude::*;
use ui::{Badge, BadgeVariant};

use crate::hooks::use_i18n::use_i18n;
use crate::i18n::I18n;
use crate::services::{api_client::with_auto_refresh, monitoring_service};
use crate::stores::{auth_store::AuthStore, user_store::UserStore};
use crate::views::shared::accounts::NoPermissionView;

#[derive(Clone)]
enum MonitoringData {
    Unified {
        summary: MonitoringSummaryResponse,
        requests: client_api::api::admin::MonitoringRequestPage,
        health: MonitoringTargetHealthResponse,
        updated_at: String,
    },
}

#[component]
pub fn Monitoring() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    if !user_store
        .info
        .read()
        .as_ref()
        .map(|user| user.is_admin())
        .unwrap_or(false)
    {
        return rsx! { NoPermissionView { resource: i18n.t("page.monitoring").to_string() } };
    }

    let mut range = use_signal(|| "1h".to_string());
    let mut custom_from = use_signal(String::new);
    let mut custom_to = use_signal(String::new);
    let mut status_filter = use_signal(String::new);
    let mut route_filter = use_signal(String::new);
    let mut paused = use_signal(|| false);
    let mut refresh_tick = use_signal(|| 0u64);
    let mut selected_request = use_signal(String::new);
    let mut cursor = use_signal(String::new);
    let mut relative_to = use_signal(chrono::Utc::now);
    let mut probe_message = use_signal(String::new);
    let mut probing = use_signal(|| false);

    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(15_000).await;
            if !paused() {
                let current_relative_to = relative_to();
                let next_relative_to =
                    refreshed_relative_to(&cursor(), current_relative_to, chrono::Utc::now());
                if next_relative_to != current_relative_to {
                    relative_to.set(next_relative_to);
                }
                refresh_tick += 1;
            }
        }
    });

    let console = use_resource(move || {
        let selected_range = range();
        let selected_from = custom_from();
        let selected_to = custom_to();
        let selected_status = status_filter();
        let selected_route = route_filter();
        let selected_cursor = cursor();
        let selected_relative_to = relative_to();
        let _ = refresh_tick();
        async move {
            with_auto_refresh(auth_store, move |token| {
                let query = build_query(
                    &selected_range,
                    &selected_from,
                    &selected_to,
                    &selected_status,
                    &selected_route,
                    &selected_cursor,
                    selected_relative_to,
                );
                async move {
                    match monitoring_service::summary(&token, &query).await {
                        Ok(summary) => {
                            let requests = monitoring_service::requests(&token, &query).await?;
                            let health = monitoring_service::target_health(&token, &query).await?;
                            Ok(MonitoringData::Unified {
                                summary,
                                requests,
                                health,
                                updated_at: chrono::Utc::now().to_rfc3339(),
                            })
                        }
                        Err(error) => Err(error),
                    }
                }
            })
            .await
        }
    });

    let detail = use_resource(move || {
        let key = detail_resource_key(&selected_request(), refresh_tick());
        async move {
            match key {
                None => None,
                Some((id, _refresh_generation)) => Some(
                    with_auto_refresh(auth_store, move |token| {
                        let id = id.clone();
                        async move { monitoring_service::request_detail(&token, &id).await }
                    })
                    .await,
                ),
            }
        }
    });

    rsx! {
        div { class: "page-container monitoring-page",
            div { class: "page-header",
                div {
                    h1 { class: "page-title", {i18n.t("page.monitoring")} }
                    p { class: "page-description", {i18n.t("monitoring.subtitle")} }
                }
            }

            div { class: "monitoring-toolbar",
                select {
                    value: "{range}",
                    aria_label: i18n.t("monitoring.time_range"),
                    onchange: move |event| {
                        range.set(event.value());
                        cursor.set(String::new());
                        relative_to.set(chrono::Utc::now());
                    },
                    option { value: "1h", {i18n.t("monitoring.range_1h")} }
                    option { value: "6h", {i18n.t("monitoring.range_6h")} }
                    option { value: "24h", {i18n.t("monitoring.range_24h")} }
                    option { value: "custom", {i18n.t("monitoring.range_custom")} }
                }
                if range() == "custom" {
                    div { class: "monitoring-custom-range",
                        input {
                            r#type: "datetime-local",
                            value: "{custom_from}",
                            aria_label: i18n.t("monitoring.utc_from"),
                            title: i18n.t("monitoring.utc_from"),
                            onchange: move |event| {
                                custom_from.set(event.value());
                                cursor.set(String::new());
                                relative_to.set(chrono::Utc::now());
                            },
                        }
                        input {
                            r#type: "datetime-local",
                            value: "{custom_to}",
                            aria_label: i18n.t("monitoring.utc_to"),
                            title: i18n.t("monitoring.utc_to_title"),
                            onchange: move |event| {
                                custom_to.set(event.value());
                                cursor.set(String::new());
                                relative_to.set(chrono::Utc::now());
                            },
                        }
                    }
                }
                select {
                    value: "{status_filter}",
                    aria_label: i18n.t("monitoring.status_filter"),
                    onchange: move |event| {
                        status_filter.set(event.value());
                        cursor.set(String::new());
                        relative_to.set(chrono::Utc::now());
                    },
                    option { value: "", {i18n.t("monitoring.all_statuses")} }
                    option { value: "succeeded", {i18n.t("monitoring.succeeded")} }
                    option { value: "failed", {i18n.t("monitoring.failed")} }
                    option { value: "timed_out", {i18n.t("monitoring.timed_out")} }
                    option { value: "running", {i18n.t("monitoring.running")} }
                    option { value: "queued", {i18n.t("monitoring.queued")} }
                }
                select {
                    value: "{route_filter}",
                    aria_label: i18n.t("monitoring.route_filter"),
                    onchange: move |event| {
                        route_filter.set(event.value());
                        cursor.set(String::new());
                        relative_to.set(chrono::Utc::now());
                    },
                    option { value: "", {i18n.t("monitoring.all_routes")} }
                    option { value: "provider_account", {i18n.t("monitoring.route_provider_account")} }
                    option { value: "node", {i18n.t("monitoring.route_node")} }
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    onclick: move |_| paused.toggle(),
                    if paused() { {i18n.t("monitoring.resume_auto_refresh")} } else { {i18n.t("monitoring.pause_auto_refresh")} }
                }
                button {
                    class: "btn btn-primary",
                    r#type: "button",
                    onclick: move |_| {
                        cursor.set(String::new());
                        relative_to.set(chrono::Utc::now());
                        refresh_tick += 1;
                    },
                    {i18n.t("monitoring.refresh_now")}
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: probing(),
                    onclick: move |_| {
                        if probing() {
                            return;
                        }
                        probing.set(true);
                        spawn(async move {
                            probe_message.set(i18n.t("monitoring.probe_in_progress").to_string());
                            let result = with_auto_refresh(auth_store, move |token| async move {
                                monitoring_service::probe_targets(&token, None).await
                            })
                            .await;
                            probe_message.set(if result.is_ok() {
                                i18n.t("monitoring.probe_done").to_string()
                            } else {
                                i18n.t("monitoring.probe_failed").to_string()
                            });
                            probing.set(false);
                            let current_relative_to = relative_to();
                            let next_relative_to = refreshed_relative_to(
                                &cursor(),
                                current_relative_to,
                                chrono::Utc::now(),
                            );
                            if next_relative_to != current_relative_to {
                                relative_to.set(next_relative_to);
                            }
                            refresh_tick += 1;
                        });
                    },
                    {i18n.t("monitoring.probe_all_accounts")}
                }
                if !probe_message().is_empty() {
                    span { class: "text-secondary monitoring-probe-state", role: "status", "{probe_message}" }
                }
            }

            match console() {
                None => rsx! { p { class: "text-secondary monitoring-load-state", {i18n.t("table.loading")} } },
                Some(Err(ref error)) => rsx! { div { class: "alert alert-error", "{i18n.t(\"common.load_failed\")}: {error}" } },
                Some(Ok(MonitoringData::Unified { ref summary, ref requests, ref health, ref updated_at })) => rsx! {
                    p { class: "text-secondary monitoring-updated-at", "{i18n.t(\"monitoring.last_updated\")}: {updated_at}" }
                    MonitoringSummaryCards { data: summary.clone() }
                    MonitoringTrends { series: summary.series.clone() }

                    h2 { class: "monitoring-request-heading", {i18n.t("monitoring.request")} }
                    if requests.items.is_empty() {
                        div { class: "empty-state monitoring-request-empty",
                            if status_filter().is_empty() && route_filter().is_empty() {
                                {i18n.t("monitoring.empty_range")}
                            } else {
                                {i18n.t("monitoring.empty_filtered")}
                            }
                        }
                    } else {
                        div { class: "table-container monitoring-request-table-container",
                            table { class: "data-table monitoring-request-table", aria_label: i18n.t("monitoring.request_list"),
                                thead { tr {
                                    th { {i18n.t("monitoring.time")} }
                                    th { {i18n.t("monitoring.request_id")} }
                                    th { {i18n.t("monitoring.protocol_model")} }
                                    th { {i18n.t("monitoring.execution_route")} }
                                    th { {i18n.t("monitoring.status")} }
                                    th { {i18n.t("monitoring.duration_ttft")} }
                                    th { {i18n.t("monitoring.tokens")} }
                                    th { {i18n.t("monitoring.amount")} }
                                } }
                                tbody {
                                    for item in requests.items.iter() {
                                        MonitoringRequestRow {
                                            item: item.clone(),
                                            selected: selected_request() == item.request_id,
                                            on_select: move |id| selected_request.set(id),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(ref next) = requests.next_cursor {
                        button {
                            class: "btn btn-secondary monitoring-next-page",
                            r#type: "button",
                            onclick: {
                                let next = next.clone();
                                move |_| cursor.set(next.clone())
                            },
                            {i18n.t("monitoring.next_page")}
                        }
                    }
                    MonitoringHealth { data: health.clone() }
                },
            }
            match detail() {
                Some(Some(Ok(ref value))) => rsx! {
                    div { class: "monitoring-detail card",
                        h2 { {i18n.t("monitoring.request_detail")} }
                        p { class: "mono monitoring-detail-request-id", "{value.request.request_id}" }
                        p { "{i18n.t(\"monitoring.tenant\")}: {value.request.tenant_id} · {i18n.t(\"monitoring.user\")}: {value.request.user_id} · {i18n.t(\"monitoring.key\")}: {value.request.produce_ai_key_id}" }
                        p { "{i18n.t(\"monitoring.status\")}: {status_label(i18n, &value.request.status)} · {i18n.t(\"monitoring.billing\")}: {value.request.billing_status} · {i18n.t(\"monitoring.trace_quality\")}: {quality_label(i18n, &value.request.trace_quality)}" }
                        p {
                            "{i18n.t(\"monitoring.client_first_content\")}: "
                            {value.request.client_first_content_at.clone().unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string())}
                        }
                        h3 { {i18n.t("monitoring.attempts")} }
                        for attempt in value.attempts.iter() {
                            MonitoringAttemptRow { attempt: attempt.clone() }
                        }
                        if let Some(ref task) = value.node_task {
                            h3 { {i18n.t("monitoring.node_task_submissions")} }
                            pre { "{pretty_json(i18n, task)}" }
                        }
                        if let Some(ref usage) = value.usage {
                            h3 { {i18n.t("monitoring.billing_summary")} }
                            pre { "{pretty_json(i18n, usage)}" }
                        }
                    }
                },
                Some(Some(Err(ref error))) => rsx! { div { class: "alert alert-error", "{i18n.t(\"monitoring.detail_load_failed\")}: {error}" } },
                _ => rsx! {},
            }
        }
    }
}

fn detail_resource_key(request_id: &str, refresh_generation: u64) -> Option<(String, u64)> {
    (!request_id.is_empty()).then(|| (request_id.to_string(), refresh_generation))
}

fn build_query(
    range: &str,
    custom_from: &str,
    custom_to: &str,
    status: &str,
    route: &str,
    cursor: &str,
    relative_to: chrono::DateTime<chrono::Utc>,
) -> MonitoringQuery {
    let hours = match range {
        "6h" => 6,
        "24h" => 24,
        _ => 1,
    };
    let normalize = |value: &str| (!value.is_empty()).then(|| format!("{value}:00Z"));
    let (from, to) = if range == "custom" {
        (normalize(custom_from), normalize(custom_to))
    } else {
        (
            Some((relative_to - chrono::Duration::hours(hours)).to_rfc3339()),
            Some(relative_to.to_rfc3339()),
        )
    };
    MonitoringQuery {
        from,
        to,
        status: (!status.is_empty()).then(|| status.to_string()),
        route_type: (!route.is_empty()).then(|| route.to_string()),
        cursor: (!cursor.is_empty()).then(|| cursor.to_string()),
        limit: Some(50),
        bucket: Some(
            if hours == 24 || range == "custom" {
                "1h"
            } else {
                "5m"
            }
            .to_string(),
        ),
        ..Default::default()
    }
}

fn refreshed_relative_to(
    cursor: &str,
    current: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    if cursor.is_empty() { now } else { current }
}

#[component]
fn MonitoringSummaryCards(data: MonitoringSummaryResponse) -> Element {
    let i18n = use_i18n();
    let active = data.summary.active_count.to_string();
    let queued = data.summary.queued_count.to_string();
    let attempt_count = data.summary.attempt_count.to_string();
    let fallback_count = data.summary.fallback_request_count.to_string();
    rsx! { div { class:"monitoring-stat-grid",
        StatCard { label:i18n.t("monitoring.request_count").to_string(),value:data.summary.request_count.to_string(),meta:i18n.t_with_args("monitoring.active_queued",&[("active",&active),("queued",&queued)]) }
        StatCard { label:i18n.t("monitoring.success_rate").to_string(),value:format_rate(i18n,data.success_rate),meta:i18n.t_with_args("monitoring.error_rate_value",&[("rate",&format_rate(i18n,data.error_rate))]) }
        StatCard { label:i18n.t("monitoring.attempt_success_rate").to_string(),value:format_rate(i18n,data.attempt_success_rate),meta:i18n.t_with_args("monitoring.attempt_count",&[("count",&attempt_count)]) }
        StatCard { label:i18n.t("monitoring.fallback_rate").to_string(),value:format_rate(i18n,data.fallback_rate),meta:i18n.t_with_args("monitoring.request_count_meta",&[("count",&fallback_count)]) }
        StatCard { label:i18n.t("monitoring.total_duration_percentiles").to_string(),value:format!("{} / {}",format_ms_f64(i18n,data.summary.p50_duration_ms),format_ms_f64(i18n,data.summary.p95_duration_ms)),meta:format!("P99 {}",format_ms_f64(i18n,data.summary.p99_duration_ms)) }
        StatCard { label:i18n.t("monitoring.provider_ttft").to_string(),value:format!("{} / {}",format_ms_f64(i18n,data.summary.p50_provider_ttft_ms),format_ms_f64(i18n,data.summary.p95_provider_ttft_ms)),meta:i18n.t("monitoring.p50_p95").to_string() }
        StatCard { label:i18n.t("monitoring.node_queue_execution").to_string(),value:format!("{} / {}",format_ms_f64(i18n,data.summary.p50_node_queue_ms),format_ms_f64(i18n,data.summary.p50_node_execution_ms)),meta:i18n.t("monitoring.node_no_ttft").to_string() }
        StatCard { label:i18n.t("monitoring.tokens_amount").to_string(),value:data.summary.total_tokens.map(|v|v.to_string()).unwrap_or_else(||i18n.t("monitoring.not_collected").to_string()),meta:format_currency_amounts(&data.summary.amounts_by_currency,i18n.t("monitoring.not_collected")) }
    } }
}

#[component]
fn MonitoringTrends(series: Vec<serde_json::Value>) -> Element {
    let i18n = use_i18n();
    rsx! {
        section { class: "monitoring-trends",
            h2 { {i18n.t("monitoring.trends")} }
            if series.is_empty() {
                p { class: "text-secondary monitoring-section-empty", {i18n.t("monitoring.no_trends")} }
            } else {
                div { class: "table-container",
                    table { class: "data-table", aria_label: i18n.t("monitoring.trends"),
                        thead { tr {
                            th { {i18n.t("monitoring.utc_time")} }
                            th { {i18n.t("monitoring.request")} }
                            th { {i18n.t("monitoring.succeeded")} }
                            th { {i18n.t("monitoring.tokens")} }
                            th { {i18n.t("monitoring.amount")} }
                        } }
                        tbody {
                            for point in series.iter() {
                                tr {
                                    td { {json_text(i18n, point, "bucket")} }
                                    td { {json_text(i18n, point, "requests")} }
                                    td { {json_text(i18n, point, "succeeded")} }
                                    td { {json_text(i18n, point, "tokens")} }
                                    td { {format_currency_amounts(&point["amounts_by_currency"], i18n.t("monitoring.not_collected"))} }
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
fn MonitoringRequestRow(
    item: MonitoringRequestItem,
    selected: bool,
    on_select: EventHandler<String>,
) -> Element {
    let i18n = use_i18n();
    let id = item.request_id.clone();
    let keyboard_id = id.clone();
    let keyboard_select = on_select.clone();
    let row_class = if selected {
        "monitoring-request-row is-selected"
    } else {
        "monitoring-request-row"
    };
    let is_node = item.route_type.as_deref() == Some("node");
    let route = item
        .route_type
        .clone()
        .unwrap_or_else(|| i18n.t("monitoring.unrouted").to_string());
    let timing = if is_node {
        format_duration(item.duration_ms)
    } else {
        format!(
            "{} / {}",
            format_duration(item.duration_ms),
            format_duration(item.provider_ttft_ms)
        )
    };
    let tokens = item
        .total_tokens
        .map(|value| value.to_string())
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let amount = match (item.currency.as_deref(), item.amount.as_deref()) {
        (Some(currency), Some(amount)) => format!("{currency} {amount}"),
        (_, Some(amount)) => amount.to_string(),
        _ => i18n.t("monitoring.not_collected").to_string(),
    };
    let view_label = i18n.t_with_args(
        "monitoring.view_request",
        &[("request_id", item.request_id.as_str())],
    );
    rsx! {
        tr {
            class: "{row_class}",
            tabindex: "0",
            role: "button",
            aria_label: view_label,
            onclick: move |_| on_select.call(id.clone()),
            onkeydown: move |event| match event.key() {
                Key::Enter => keyboard_select.call(keyboard_id.clone()),
                Key::Character(value) if value == " " => {
                    event.prevent_default();
                    keyboard_select.call(keyboard_id.clone());
                }
                _ => {}
            },
            td { "data-label": i18n.t("monitoring.time"), "{item.received_at}" }
            td { class: "mono", "data-label": i18n.t("monitoring.request_id"), "{short_request_id(&item.request_id)}" }
            td { "data-label": i18n.t("monitoring.protocol_model"), "{item.protocol} / {item.requested_model}" }
            td { "data-label": i18n.t("monitoring.execution_route"), "{route}" }
            td { "data-label": i18n.t("monitoring.status"),
                Badge { variant: status_variant(&item.status), "{status_label(i18n, &item.status)}" }
                if item.trace_quality != "actual" { span { class: "text-secondary", " · {quality_label(i18n, &item.trace_quality)}" } }
            }
            td { "data-label": i18n.t("monitoring.duration_ttft"), "{timing}" }
            td { "data-label": i18n.t("monitoring.tokens"), "{tokens}" }
            td { "data-label": i18n.t("monitoring.amount"), "{amount}" }
        }
    }
}

#[component]
fn MonitoringHealth(data: MonitoringTargetHealthResponse) -> Element {
    let i18n = use_i18n();
    rsx! {
        div { class: "monitoring-health-grid",
            section {
                h2 { {i18n.t("monitoring.provider_health")} }
                a { href: "/admin/accounts", {i18n.t("monitoring.account_probe_link")} }
                if data.providers.is_empty() {
                    p { class: "text-secondary monitoring-section-empty", {i18n.t("monitoring.no_accounts")} }
                }
                for provider in data.providers.iter() {
                    ProviderHealthRow { value: provider.clone() }
                }
            }
            section {
                h2 { {i18n.t("monitoring.node_health")} }
                a { href: "/admin/node-gateway", {i18n.t("monitoring.node_gateway_link")} }
                if data.nodes.is_empty() {
                    p { class: "text-secondary monitoring-section-empty", {i18n.t("monitoring.no_nodes")} }
                }
                for node in data.nodes.iter() {
                    NodeHealthRow { value: node.clone() }
                }
            }
        }
    }
}

#[component]
fn MonitoringAttemptRow(attempt: MonitoringAttemptDetail) -> Element {
    let i18n = use_i18n();
    let target = attempt
        .account_name
        .clone()
        .or_else(|| attempt.node_name.clone())
        .or_else(|| attempt.provider_name.clone())
        .or_else(|| attempt.node_id.clone())
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let http = attempt
        .http_status
        .map(|value| value.to_string())
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let upstream = attempt
        .upstream_request_id
        .clone()
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let ttft = duration_between(
        i18n,
        &attempt.started_at,
        attempt.first_content_at.as_deref(),
    );
    let finished = attempt
        .finished_at
        .clone()
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let headers = attempt
        .headers_received_at
        .clone()
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let first = attempt
        .first_content_at
        .clone()
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let end_reason = attempt
        .stream_end_reason
        .clone()
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let error = attempt.error_code.clone().or(attempt.error_summary.clone());
    let attempt_title = format!(
        "#{} {} · {}",
        attempt.attempt_no,
        attempt.attempt_kind,
        status_label(i18n, &attempt.status)
    );
    let target_line = i18n.t_with_args("monitoring.attempt_target", &[("target", &target)]);
    let timing_line = i18n.t_with_args(
        "monitoring.attempt_timing",
        &[
            ("start", &attempt.started_at),
            ("end", &finished),
            ("reason", &end_reason),
        ],
    );
    let http_line = i18n.t_with_args(
        "monitoring.attempt_http",
        &[("http", &http), ("upstream", &upstream)],
    );
    let provider_timing_line = i18n.t_with_args(
        "monitoring.attempt_provider_timing",
        &[("headers", &headers), ("first", &first), ("ttft", &ttft)],
    );
    rsx! { div { class:"monitoring-stage",
        strong { "{attempt_title}" }
        p { "{target_line}" }
        p { "{timing_line}" }
        p { "{http_line}" }
        if attempt.route_type=="provider_account" { p { "{provider_timing_line}" } }
        if let Some(error)=error { p { class:"text-secondary", "{i18n.t(\"monitoring.error\")}: {error}" } }
    } }
}

#[component]
fn ProviderHealthRow(value: serde_json::Value) -> Element {
    let i18n = use_i18n();
    let name = value
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("-")
        .to_string();
    let probe = value
        .get("last_probe_status")
        .and_then(|value| value.as_str())
        .unwrap_or(i18n.t("monitoring.not_collected"))
        .to_string();
    let probe_at = value
        .get("last_probe_at")
        .and_then(|value| value.as_str())
        .unwrap_or(i18n.t("monitoring.not_collected"))
        .to_string();
    let probe_latency = value
        .get("last_probe_latency_ms")
        .and_then(|value| value.as_i64())
        .map(|value| format!("{value} ms"))
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let probe_error = value
        .get("last_probe_error_code")
        .and_then(|value| value.as_str())
        .unwrap_or(i18n.t("monitoring.none"))
        .to_string();
    let failures = value
        .get("attributable_failures")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let enabled = value
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let success = value
        .get("success_rate")
        .and_then(|value| value.as_f64())
        .map(|value| format!("{:.1}%", value * 100.0))
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let latency = value
        .get("avg_latency_ms")
        .and_then(|value| value.as_f64())
        .map(|value| format!("{value:.0} ms"))
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string());
    let enabled_label = if enabled {
        i18n.t("monitoring.enabled")
    } else {
        i18n.t("monitoring.disabled")
    };
    let health_line = i18n.t_with_args(
        "monitoring.provider_health_line",
        &[("success", &success), ("latency", &latency)],
    );
    let probe_line = i18n.t_with_args(
        "monitoring.provider_probe_line",
        &[
            ("probe", &probe),
            ("at", &probe_at),
            ("latency", &probe_latency),
            ("error", &probe_error),
            ("failures", &failures.to_string()),
        ],
    );
    rsx! { div { class:"monitoring-node-row", strong { "{name}" } span { " {enabled_label}" } span { " · {health_line}" } span { " · {probe_line}" } } }
}

#[component]
fn NodeHealthRow(value: serde_json::Value) -> Element {
    let i18n = use_i18n();
    let name = if value
        .get("is_unassigned")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        i18n.t("monitoring.unassigned_queue").to_string()
    } else {
        value
            .get("display_name")
            .and_then(|value| value.as_str())
            .unwrap_or("-")
            .to_string()
    };
    let status = value
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string();
    let queued = value
        .get("queued")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let running = value
        .get("running")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let succeeded = value
        .get("succeeded")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let failed = value
        .get("failed")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let expired = value
        .get("expired")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let heartbeat = value
        .get("last_heartbeat_at")
        .and_then(|value| value.as_str())
        .unwrap_or(i18n.t("monitoring.not_collected"))
        .to_string();
    let session = value
        .get("session_expires_at")
        .and_then(|value| value.as_str())
        .unwrap_or(i18n.t("monitoring.not_collected"))
        .to_string();
    let models = value
        .get("accepted_models")
        .map(|value| value.to_string())
        .unwrap_or_else(|| "[]".to_string());
    let counts = i18n.t_with_args(
        "monitoring.node_counts",
        &[
            ("queued", &queued.to_string()),
            ("running", &running.to_string()),
            ("succeeded", &succeeded.to_string()),
            ("failed", &failed.to_string()),
            ("expired", &expired.to_string()),
        ],
    );
    let runtime = i18n.t_with_args(
        "monitoring.node_runtime",
        &[
            ("heartbeat", &heartbeat),
            ("session", &session),
            ("models", &models),
        ],
    );
    rsx! { div { class:"monitoring-node-row", strong { "{name}" } span { " {status_label(i18n, &status)}" } span { " · {counts}" } span { " · {runtime}" } } }
}

fn format_rate(i18n: I18n, value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.1}%", v * 100.0))
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string())
}
fn format_ms_f64(i18n: I18n, value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.0} ms"))
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string())
}
fn short_request_id(value: &str) -> String {
    value.chars().take(8).collect()
}
fn status_variant(value: &str) -> BadgeVariant {
    match value {
        "succeeded" => BadgeVariant::Success,
        "failed" | "timed_out" | "cancelled" => BadgeVariant::Error,
        "running" | "queued" | "routing" => BadgeVariant::Warning,
        _ => BadgeVariant::Neutral,
    }
}
fn status_label(i18n: I18n, value: &str) -> String {
    let key = match value {
        "succeeded" => Some("monitoring.succeeded"),
        "failed" => Some("monitoring.failed"),
        "timed_out" => Some("monitoring.timed_out"),
        "running" => Some("monitoring.running"),
        "queued" => Some("monitoring.queued"),
        "routing" => Some("monitoring.routing"),
        "cancelled" => Some("monitoring.cancelled"),
        "received" => Some("monitoring.received"),
        "online" => Some("monitoring.online"),
        "offline" => Some("monitoring.offline"),
        _ => None,
    };
    key.map(|key| i18n.t(key).to_string())
        .unwrap_or_else(|| value.to_string())
}

fn quality_label(i18n: I18n, value: &str) -> &'static str {
    match value {
        "derived" => i18n.t("monitoring.quality_derived"),
        "partial" => i18n.t("monitoring.quality_partial"),
        _ => i18n.t("monitoring.quality_actual"),
    }
}
fn duration_between(i18n: I18n, start: &str, end: Option<&str>) -> String {
    let parsed = chrono::DateTime::parse_from_rfc3339(start)
        .ok()
        .zip(end.and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok()));
    parsed
        .map(|(a, b)| format!("{} ms", (b - a).num_milliseconds()))
        .unwrap_or_else(|| i18n.t("monitoring.not_collected").to_string())
}
fn pretty_json(i18n: I18n, value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| i18n.t("monitoring.not_collected").to_string())
}
fn json_text(i18n: I18n, value: &serde_json::Value, key: &str) -> String {
    match value.get(key) {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Number(value)) => value.to_string(),
        Some(serde_json::Value::Null) | None => i18n.t("monitoring.not_collected").to_string(),
        Some(value) => value.to_string(),
    }
}

fn format_currency_amounts(value: &serde_json::Value, not_collected: &str) -> String {
    let Some(amounts) = value.as_object() else {
        return not_collected.to_string();
    };
    let mut entries = amounts
        .iter()
        .filter_map(|(currency, amount)| {
            amount.as_str().map(|amount| format!("{currency} {amount}"))
        })
        .collect::<Vec<_>>();
    entries.sort();
    if entries.is_empty() {
        not_collected.to_string()
    } else {
        entries.join(" · ")
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

fn format_duration(ms: Option<i64>) -> String {
    match ms {
        Some(value) if value >= 1000 => format!("{:.1}s", value as f64 / 1000.0),
        Some(value) => format!("{}ms", value),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_query, detail_resource_key, format_currency_amounts, refreshed_relative_to};

    fn timestamp(value: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn selected_detail_key_changes_on_refresh() {
        assert_eq!(detail_resource_key("", 1), None);
        assert_ne!(
            detail_resource_key("request-1", 1),
            detail_resource_key("request-1", 2)
        );
    }

    #[test]
    fn cursor_pages_keep_the_original_relative_time_window() {
        let snapshot_to = timestamp("2026-08-23T12:00:00Z");
        let first = build_query("1h", "", "", "", "", "", snapshot_to);
        let next = build_query("1h", "", "", "", "", "next-cursor", snapshot_to);

        assert_eq!(first.from, next.from);
        assert_eq!(first.to, next.to);
        assert_eq!(next.cursor.as_deref(), Some("next-cursor"));
        assert_eq!(first.from.as_deref(), Some("2026-08-23T11:00:00+00:00"));
        assert_eq!(first.to.as_deref(), Some("2026-08-23T12:00:00+00:00"));
    }

    #[test]
    fn auto_refresh_advances_only_an_uncursored_snapshot() {
        let current = timestamp("2026-08-23T12:00:00Z");
        let now = timestamp("2026-08-23T12:15:00Z");

        assert_eq!(refreshed_relative_to("", current, now), now);
        assert_eq!(refreshed_relative_to("next-cursor", current, now), current);
    }

    #[test]
    fn mobile_request_labels_are_translated_dom_attributes() {
        let css = include_str!("../../../assets/main.css");
        assert!(css.contains("content: attr(data-label)"));
        assert!(!css.contains("td:nth-child(1)::before { content:"));
    }

    #[test]
    fn monitoring_route_and_ttft_labels_use_i18n_keys() {
        let source = include_str!("monitoring.rs");
        assert!(source.contains("monitoring.route_provider_account"));
        assert!(source.contains("monitoring.route_node"));
        assert!(source.contains("monitoring.provider_ttft"));
        assert!(source.contains("monitoring.p50_p95"));
        assert!(!source.contains("label:\"Provider TTFT\""));
    }

    #[test]
    fn monetary_totals_keep_currencies_separate() {
        let amounts = serde_json::json!({"USD":"2.50","CNY":"10.00"});
        assert_eq!(
            format_currency_amounts(&amounts, "not collected"),
            "CNY 10.00 · USD 2.50"
        );
        assert_eq!(
            format_currency_amounts(&serde_json::json!({}), "not collected"),
            "not collected"
        );
    }

    #[test]
    fn custom_range_changes_reset_request_pagination() {
        let source = include_str!("monitoring.rs");
        let custom_range = source
            .split("class: \"monitoring-custom-range\"")
            .nth(1)
            .unwrap()
            .split("select {")
            .next()
            .unwrap();
        assert_eq!(custom_range.matches("cursor.set(String::new())").count(), 2);
    }
}
