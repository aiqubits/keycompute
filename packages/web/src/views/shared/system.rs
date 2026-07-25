use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, Table, TableHead};

use crate::hooks::use_i18n::use_i18n;
use crate::services::{api_client::with_auto_refresh, debug_service};
use crate::stores::auth_store::AuthStore;
use crate::stores::user_store::UserStore;
use crate::views::shared::accounts::NoPermissionView;

/// 系统诊断页面（仅 Admin 可访问）
///
/// 展示 Provider 健康状态、网关运行统计、路由调试信息（调用 DebugApi）
#[component]
pub fn System() -> Element {
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
            NoPermissionView { resource: i18n.t("page.system").to_string() }
        };
    }

    let provider_health = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            debug_service::provider_health(&token).await
        })
        .await
    });

    let gateway_stats = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            debug_service::gateway_stats(&token).await
        })
        .await
    });

    let routing_info = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            // 使用默认模型进行路由调试
            debug_service::routing("gpt-4o", &token).await
        })
        .await
    });

    let (total_req, success_rate, avg_latency, fallback_count) = match gateway_stats() {
        Some(Ok(ref s)) => (
            s.total_requests.to_string(),
            format!(
                "{:.1}%",
                s.successful_requests as f64 / s.total_requests.max(1) as f64 * 100.0
            ),
            format!("{}ms", s.avg_latency_ms),
            s.fallback_count.to_string(),
        ),
        Some(Err(_)) => (
            i18n.t("common.load_failed").to_string(),
            "—".into(),
            "—".into(),
            "—".into(),
        ),
        None => (
            i18n.t("table.loading").to_string(),
            "—".into(),
            "—".into(),
            "—".into(),
        ),
    };

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", {i18n.t("page.system")} }
            p { class: "page-description", {i18n.t("system.subtitle")} }
        }

        // Provider 健康状态
        div { class: "section",
            h2 { class: "section-title", {i18n.t("system.provider_health")} }
            div { class: "card",
                div { class: "card-body",
                    match provider_health() {
                        None => rsx! {
                            p { class: "text-secondary", {i18n.t("table.loading")} }
                        },
                        Some(Err(_)) => rsx! {
                            p { class: "text-secondary", {i18n.t("common.load_failed")} }
                        },
                        Some(Ok(ref resp)) => rsx! {
                            div { class: "health-grid",
                                for name in resp.healthy_providers.iter() {
                                    HealthItem {
                                        name: name.clone(),
                                        status: "healthy".to_string(),
                                        latency_ms: None,
                                    }
                                }
                                if resp.healthy_providers.is_empty() {
                                    div { class: "text-secondary", {i18n.t("system.no_healthy_provider")} }
                                }
                            }
                        },
                    }
                }
            }
        }

        // 网关运行统计
        div { class: "section",
            h2 { class: "section-title", {i18n.t("system.gateway_stats")} }
            div { class: "stats-grid",
                div { class: "stat-card card",
                    div { class: "card-body",
                        p { class: "stat-label", {i18n.t("system.total_requests")} }
                        p { class: "stat-value", "{total_req}" }
                    }
                }
                div { class: "stat-card card",
                    div { class: "card-body",
                        p { class: "stat-label", {i18n.t("system.success_rate")} }
                        p { class: "stat-value", "{success_rate}" }
                    }
                }
                div { class: "stat-card card",
                    div { class: "card-body",
                        p { class: "stat-label", {i18n.t("system.avg_latency")} }
                        p { class: "stat-value", "{avg_latency}" }
                    }
                }
                div { class: "stat-card card",
                    div { class: "card-body",
                        p { class: "stat-label", {i18n.t("system.fallback_count")} }
                        p { class: "stat-value", "{fallback_count}" }
                    }
                }
            }
        }

        // 路由调试
        div { class: "section",
            h2 { class: "section-title", {i18n.t("system.routing_debug")} }
            div { class: "card",
                div { class: "card-header",
                    h3 { class: "card-title", {i18n.t("system.provider_status_diagnosis")} }
                }
                div { class: "card-body",
                    match routing_info() {
                        None => rsx! {
                            p { class: "text-secondary", {i18n.t("table.loading")} }
                        },
                        Some(Err(ref e)) => rsx! {
                            div { class: "alert alert-error",
                                p { "{i18n.t(\"common.load_failed\")}: {e}" }
                            }
                        },
                        Some(Ok(ref info)) => rsx! {
                            div {
                                // 路由结果
                                if info.routed {
                                    div { class: "alert alert-success",
                                        p { "✓ {i18n.t(\"system.route_success\")}" }
                                        if let Some(ref primary) = info.primary {
                                            p { class: "text-sm",
                                                "{i18n.t(\"system.primary_target\")}: {primary.provider} ({primary.endpoint})"
                                            }

                                        }
                                        if !info.fallback_chain.is_empty() {
                                            p { class: "text-sm",
                                                "{i18n.t(\"system.fallback_chain\")}: {info.fallback_chain.len()} {i18n.t(\"system.items\")}"
                                            }
                                        }
                                    }
                                } else {
                                    div { class: "alert alert-warning",
                                        p { "✗ {i18n.t(\"system.route_failed\")}" }
                                        if let Some(ref msg) = info.message {
                                            p { class: "text-sm", "{msg}" }
                                        }
                                    }
                                }

                                // Provider 状态表格
                                h4 { class: "subsection-title", {i18n.t("system.provider_status")} }
                                Table {
                                    empty: info.provider_status.is_empty(),
                                    empty_text: i18n.t("system.no_provider_configured"),
                                    col_count: 4,
                                    thead {
                                        tr {
                                            TableHead { {i18n.t("system.provider_column")} }
                                            TableHead { {i18n.t("system.health_status")} }
                                            TableHead { {i18n.t("system.account_count")} }
                                            TableHead { {i18n.t("table.status")} }
                                        }
                                    }
                                    tbody {
                                        for ps in info.provider_status.iter() {
                                            tr {
                                                td { "{ps.provider}" }
                                                td {
                                                    if ps.is_healthy {
                                                        Badge { variant: BadgeVariant::Success, {i18n.t("system.healthy")} }
                                                    } else {
                                                        Badge { variant: BadgeVariant::Error, {i18n.t("system.unhealthy")} }
                                                    }
                                                }
                                                td { "{ps.account_count}" }
                                                td { "{ps.status}" }
                                            }
                                        }
                                    }
                                }

                                // 定价信息
                                h4 { class: "subsection-title", {i18n.t("system.pricing_info")} }
                                div { class: "info-grid",
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("pricing.model_name")} }
                                        span { class: "info-value", "{info.pricing.model_name}" }
                                    }
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("common.currency")} }
                                        span { class: "info-value", "{info.pricing.currency}" }
                                    }
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("pricing.input_price")} }
                                        span { class: "info-value", "{info.pricing.input_price_per_1k} / 1K tokens" }
                                    }
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("pricing.output_price")} }
                                        span { class: "info-value", "{info.pricing.output_price_per_1k} / 1K tokens" }
                                    }
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}

// ── 内部组件 ──────────────────────────────────────────────────────

#[component]
fn HealthItem(name: String, status: String, latency_ms: Option<i64>) -> Element {
    let i18n = use_i18n();
    let (status_class, status_text) = match status.as_str() {
        "healthy" => (BadgeVariant::Success, i18n.t("system.healthy")),
        "degraded" => (BadgeVariant::Warning, i18n.t("system.degraded")),
        "unhealthy" => (BadgeVariant::Error, i18n.t("system.abnormal")),
        _ => (BadgeVariant::Neutral, i18n.t("system.unknown")),
    };

    rsx! {
        div { class: "health-item",
            div { class: "health-name", "{name}" }
            div { class: "health-status",
                Badge { variant: status_class, "{status_text}" }
                if let Some(ms) = latency_ms {
                    span { class: "text-secondary text-sm", "{ms}ms" }
                }
            }
        }
    }
}
