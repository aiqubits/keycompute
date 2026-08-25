use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use dioxus::prelude::*;

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;
use crate::services::{
    api_client::with_auto_refresh, api_key_service, billing_service, debug_service,
    distribution_service, payment_service,
};
use crate::stores::auth_store::AuthStore;
use crate::stores::public_settings_store::PublicSettingsStore;
use crate::stores::user_store::UserStore;
use crate::utils::time::format_time;

#[derive(Clone)]
struct TrendPoint {
    label: String,
    requests: i32,
}

#[component]
pub fn Dashboard() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let public_settings_store = use_context::<PublicSettingsStore>();

    let user_info = user_store.info.read().clone();
    let is_admin = user_info.as_ref().map(|u| u.is_admin()).unwrap_or(false);
    let distribution_settings_loaded = public_settings_store.loaded();
    let show_distribution_metrics = !is_admin
        && distribution_settings_loaded
        && public_settings_store.distribution_is_enabled();

    let usage_stats = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            with_auto_refresh(
                auth,
                |token| async move { billing_service::stats(&token).await },
            )
            .await
        }
    });

    let balance = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            with_auto_refresh(auth, |token| async move {
                payment_service::get_balance(&token).await
            })
            .await
        }
    });

    let api_keys = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            with_auto_refresh(auth, |token| async move {
                api_key_service::list(false, &token).await
            })
            .await
        }
    });

    let usage_records = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            with_auto_refresh(
                auth,
                |token| async move { billing_service::list(&token).await },
            )
            .await
        }
    });

    let payment_orders = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            with_auto_refresh(auth, |token| async move {
                payment_service::list_orders(None, &token).await
            })
            .await
        }
    });

    let distribution_earnings = use_resource(move || {
        let auth = auth_store.clone();
        let public_settings_store = public_settings_store;
        async move {
            if !public_settings_store.loaded() {
                return None;
            }
            if !public_settings_store.distribution_is_enabled() {
                return Some(Ok(None));
            }

            Some(
                with_auto_refresh(auth, |token| async move {
                    distribution_service::get_earnings(&token).await.map(Some)
                })
                .await,
            )
        }
    });

    let gateway_status = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            if !is_admin {
                return Ok(None);
            }
            with_auto_refresh(auth, |token| async move {
                debug_service::gateway_status(&token).await.map(Some)
            })
            .await
        }
    });

    let gateway_stats = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            if !is_admin {
                return Ok(None);
            }
            with_auto_refresh(auth, |token| async move {
                debug_service::gateway_stats(&token).await.map(Some)
            })
            .await
        }
    });

    let provider_health = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            if !is_admin {
                return Ok(None);
            }
            with_auto_refresh(auth, |token| async move {
                debug_service::provider_health(&token).await.map(Some)
            })
            .await
        }
    });

    let greeting = if let Some(ref u) = user_info {
        format!(
            "{}，{}",
            i18n.t("dashboard.greeting"),
            u.name.as_deref().unwrap_or(&u.email)
        )
    } else {
        i18n.t("dashboard.greeting").to_string()
    };

    let api_call_value = usage_stats()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|stats| stats.total_requests.to_string())
        .unwrap_or_else(|| "—".to_string());

    let total_cost_value = usage_stats()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|stats| format!("¥{}", crate::utils::format_money(stats.total_cost)))
        .unwrap_or_else(|| "—".to_string());

    let balance_value = balance()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|balance| {
            format!(
                "¥{}",
                crate::utils::format_money_str(&balance.available_balance)
            )
        })
        .unwrap_or_else(|| "—".to_string());

    let active_key_value = api_keys()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|keys| keys.iter().filter(|key| key.is_active).count().to_string())
        .unwrap_or_else(|| "—".to_string());

    let active_keys = api_keys()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|keys| {
            keys.iter()
                .filter(|key| key.is_active)
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let recent_usage = usage_records()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|records| records.iter().take(5).cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    let recent_orders = payment_orders()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|orders| orders.iter().take(3).cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    let recent_trend = usage_records()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|records| build_trend_points(records.as_slice()))
        .unwrap_or_default();

    let trend_svg = build_trend_svg(&recent_trend);

    let admin_gateway = gateway_status()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(|value| value.as_ref())
        .cloned();

    let admin_gateway_stats = gateway_stats()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(|value| value.as_ref())
        .cloned();

    let admin_provider_health = provider_health()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(|value| value.as_ref())
        .cloned();
    let distribution_earnings_value = distribution_earnings();
    let distribution_earnings_data = distribution_earnings_value
        .as_ref()
        .and_then(|result| result.as_ref())
        .and_then(|result| result.as_ref().ok())
        .and_then(|earnings| earnings.as_ref());
    let total_distribution_earnings_value = distribution_earnings_data
        .map(|earnings| {
            format!(
                "{} {}",
                earnings.currency,
                crate::utils::format_money_str(&earnings.total_earnings)
            )
        })
        .unwrap_or_else(|| "—".to_string());
    let pending_distribution_earnings_value = distribution_earnings_data
        .map(|earnings| {
            format!(
                "{} {}",
                earnings.currency,
                crate::utils::format_money_str(&earnings.pending_earnings)
            )
        })
        .unwrap_or_else(|| "—".to_string());
    let distribution_referral_count_value = distribution_earnings_data
        .map(|earnings| earnings.referral_count.to_string())
        .unwrap_or_else(|| "—".to_string());

    rsx! {
        div { class: "page-container dashboard-console",
            div { class: "page-header dashboard-console-header",
                div {
                    h1 { class: "page-title dashboard-console-title", "{greeting}" }
                    p { class: "page-subtitle dashboard-console-subtitle", {i18n.t("dashboard.subtitle_long")} }
                }
            }

            div { class: "dashboard-stats-row",
                DashboardStatCard {
                    tone: "blue",
                    icon: "◎",
                    title: i18n.t("dashboard.api_calls").to_string(),
                    value: api_call_value,
                    meta: i18n.t("dashboard.meta_usage").to_string()
                }
                DashboardStatCard {
                    tone: "green",
                    icon: "¥",
                    title: i18n.t("dashboard.balance_available").to_string(),
                    value: balance_value,
                    meta: i18n.t("dashboard.meta_balance").to_string()
                }
                DashboardStatCard {
                    tone: "orange",
                    icon: "K",
                    title: i18n.t("dashboard.active_keys").to_string(),
                    value: active_key_value,
                    meta: i18n.t("dashboard.meta_keys").to_string()
                }
                DashboardStatCard {
                    tone: "violet",
                    icon: "↗",
                    title: i18n.t("dashboard.total_cost").to_string(),
                    value: total_cost_value,
                    meta: i18n.t("dashboard.meta_cost").to_string()
                }
            }

            div { class: "dashboard-two-col",
                div { class: "dashboard-panel dashboard-panel-primary",
                    div { class: "dashboard-panel-head",
                        div {
                            h2 { class: "dashboard-panel-title", {i18n.t("dashboard.recent_active_days")} }
                            p { class: "dashboard-panel-copy", {i18n.t("dashboard.recent_active_days_desc")} }
                        }
                        span { class: "dashboard-panel-tag", {i18n.t("dashboard.live_data")} }
                    }
                    div { class: "dashboard-panel-body" ,
                        div { class: "dashboard-trend-chart",
                            svg {
                                class: "dashboard-trend-svg",
                                view_box: "0 0 640 220",
                                preserve_aspect_ratio: "none",
                                defs {
                                    linearGradient { id: "dashboardTrendFill", x1: "0", y1: "0", x2: "0", y2: "1",
                                        stop { offset: "0%", stop_color: "rgba(71, 132, 255, 0.30)" }
                                        stop { offset: "100%", stop_color: "rgba(71, 132, 255, 0.02)" }
                                    }
                                }
                                rect { class: "dashboard-trend-plot", x: "12", y: "10", width: "616", height: "184", rx: "18" }
                                for guide in trend_svg.guides.iter() {
                                    line {
                                        class: "dashboard-trend-guide",
                                        x1: "24",
                                        y1: "{guide}",
                                        x2: "616",
                                        y2: "{guide}"
                                    }
                                }
                                path { class: "dashboard-trend-area", d: "{trend_svg.area_path}" }
                                path { class: "dashboard-trend-line", d: "{trend_svg.line_path}" }
                                for (index, point) in recent_trend.iter().enumerate() {
                                    circle {
                                        class: if point.requests > 0 {
                                            "dashboard-trend-point dashboard-trend-point-active"
                                        } else {
                                            "dashboard-trend-point"
                                        },
                                        cx: "{trend_svg.x_points[index]}",
                                        cy: "{trend_svg.y_points[index]}",
                                        r: if point.requests > 0 { "4.5" } else { "3.2" }
                                    }
                                    if point.requests > 0 {
                                        text {
                                            class: "dashboard-trend-peak",
                                            x: "{trend_svg.x_points[index]}",
                                            y: "{trend_svg.y_points[index] - 12.0}",
                                            text_anchor: "middle",
                                            "{point.requests}"
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "dashboard-trend-labels",
                            for point in recent_trend.iter() {
                                div {
                                    class: if point.requests > 0 {
                                        "dashboard-trend-label-item dashboard-trend-label-item-active"
                                    } else {
                                        "dashboard-trend-label-item"
                                    },
                                    span { class: "dashboard-trend-label", "{point.label}" }
                                    span { class: "dashboard-trend-value", "{point.requests}" }
                                }
                            }
                        }
                    }
                }

                div { class: "dashboard-panel dashboard-panel-side",
                    div { class: "dashboard-panel-head",
                        div {
                            h2 { class: "dashboard-panel-title", {i18n.t("dashboard.quick_links")} }
                            p { class: "dashboard-panel-copy", {i18n.t("dashboard.quick_links_desc")} }
                        }
                    }
                    div { class: "dashboard-panel-body dashboard-quick-links",
                        DashboardQuickLink {
                            route: Route::ApiKeyList {},
                            tone: "blue",
                            title: i18n.t("dashboard.manage_api_keys").to_string(),
                            description: i18n.t("dashboard.manage_api_keys_desc").to_string()
                        }
                        DashboardQuickLink {
                            route: Route::PaymentsOverview {},
                            tone: "green",
                            title: i18n.t("dashboard.payments").to_string(),
                            description: i18n.t("dashboard.payments_desc").to_string()
                        }
                        DashboardQuickLink {
                            route: Route::Usage {},
                            tone: "orange",
                            title: i18n.t("dashboard.usage_details").to_string(),
                            description: i18n.t("dashboard.usage_details_desc").to_string()
                        }
                        DashboardQuickLink {
                            route: Route::UserProfile {},
                            tone: "violet",
                            title: i18n.t("dashboard.account_settings").to_string(),
                            description: i18n.t("dashboard.account_settings_desc").to_string()
                        }
                    }
                }
            }

            div { class: "dashboard-bottom-row",
                div { class: "dashboard-panel",
                    div { class: "dashboard-panel-head",
                        div {
                            h2 { class: "dashboard-panel-title", {i18n.t("dashboard.recent_calls")} }
                            p { class: "dashboard-panel-copy", {i18n.t("dashboard.recent_calls_desc")} }
                        }
                    }
                    div { class: "dashboard-panel-body" ,
                        if recent_usage.is_empty() {
                            p { class: "dashboard-empty-copy", {i18n.t("dashboard.no_recent_calls")} }
                        } else {
                            div { class: "dashboard-activity-table",
                                div { class: "dashboard-activity-row dashboard-activity-row-head",
                                    div { {i18n.t("usage.model")} }
                                    div { {i18n.t("table.status")} }
                                    div { {i18n.t("common.time")} }
                                    div { {i18n.t("common.cost")} }
                                }
                                for record in recent_usage.iter() {
                                    div { class: "dashboard-activity-row",
                                        div {
                                            div { class: "dashboard-activity-main", "{record.model}" }
                                            div { class: "dashboard-activity-sub", "#{record.request_id.chars().take(8).collect::<String>()}" }
                                        }
                                        div {
                                            span {
                                                class: if record.status == "success" {
                                                    "dashboard-inline-status dashboard-inline-status-ok"
                                                } else {
                                                    "dashboard-inline-status dashboard-inline-status-warn"
                                                },
                                                "{record.status}"
                                            }
                                        }
                                        div { class: "dashboard-activity-time", { format_time(&record.created_at) } }
                                        div { class: "dashboard-activity-time", "¥{crate::utils::format_money(record.cost)}" }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "dashboard-panel",
                    div { class: "dashboard-panel-head",
                        div {
                            h2 { class: "dashboard-panel-title", {i18n.t("dashboard.active_keys_panel")} }
                            p { class: "dashboard-panel-copy", {i18n.t("dashboard.active_keys_panel_desc")} }
                        }
                    }
                    div { class: "dashboard-panel-body" ,
                        if active_keys.is_empty() {
                            p { class: "dashboard-empty-copy", {i18n.t("dashboard.no_active_keys")} }
                        } else {
                            div { class: "dashboard-key-list",
                                for key in active_keys.iter() {
                                    div { class: "dashboard-key-item",
                                        span { class: "dashboard-key-dot" }
                                        div { class: "dashboard-key-main",
                                            div { class: "dashboard-key-name", "{key.name}" }
                                            div { class: "dashboard-key-meta",
                                                code { class: "dashboard-key-code", "{key.key_preview}" }
                                                span { class: "dashboard-key-time",
                                                    { format_active_key_time(
                                                        key.last_used_at.as_deref(),
                                                        i18n.t("dashboard.last_used_prefix"),
                                                        i18n.t("dashboard.no_usage_record"),
                                                    ) }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "dashboard-panel",
                    div { class: "dashboard-panel-head",
                        div {
                            h2 { class: "dashboard-panel-title",
                                if is_admin { {i18n.t("dashboard.system_status")} } else { {i18n.t("dashboard.account_status")} }
                            }
                            p { class: "dashboard-panel-copy",
                                if is_admin {
                                    {i18n.t("dashboard.system_status_desc")}
                                } else {
                                    {i18n.t("dashboard.account_status_desc")}
                                }
                            }
                        }
                        if is_admin {
                            span {
                                class: if admin_gateway.as_ref().map(|g| g.available).unwrap_or(false) {
                                    "dashboard-status-pill dashboard-status-pill-ok"
                                } else {
                                    "dashboard-status-pill"
                                },
                                if admin_gateway.as_ref().map(|g| g.available).unwrap_or(false) { {i18n.t("dashboard.online")} } else { {i18n.t("dashboard.pending_check")} }
                            }
                        }
                    }
                    div { class: "dashboard-panel-body",
                        if is_admin {
                            DashboardStatusMetric {
                                label: i18n.t("dashboard.gateway_providers").to_string(),
                                value: admin_gateway.as_ref()
                                    .map(|gateway| gateway.providers.len().to_string())
                                    .unwrap_or_else(|| "—".to_string()),
                                sub: i18n.t("dashboard.gateway_providers_desc").to_string()
                            }
                            DashboardStatusMetric {
                                label: i18n.t("dashboard.healthy_providers").to_string(),
                                value: admin_provider_health.as_ref()
                                    .map(|health| health.healthy_providers.len().to_string())
                                    .unwrap_or_else(|| "—".to_string()),
                                sub: i18n.t("dashboard.healthy_providers_desc").to_string()
                            }
                            DashboardStatusMetric {
                                label: i18n.t("dashboard.account_cache").to_string(),
                                value: admin_provider_health.as_ref()
                                    .map(|health| health.account_count.to_string())
                                    .unwrap_or_else(|| "—".to_string()),
                                sub: i18n.t("dashboard.account_cache_desc").to_string()
                            }
                            DashboardStatusMetric {
                                label: i18n.t("dashboard.fallback_count").to_string(),
                                value: admin_gateway_stats.as_ref()
                                    .map(|stats| stats.fallback_count.to_string())
                                    .unwrap_or_else(|| "—".to_string()),
                                sub: i18n.t("dashboard.fallback_count_desc").to_string()
                            }
                        } else {
                            if show_distribution_metrics {
                                DashboardStatusMetric {
                                    label: i18n.t("dashboard.total_distribution_earnings").to_string(),
                                    value: total_distribution_earnings_value.clone(),
                                    sub: i18n.t("dashboard.total_distribution_earnings_desc").to_string()
                                }
                                DashboardStatusMetric {
                                    label: i18n.t("dashboard.pending_distribution_earnings").to_string(),
                                    value: pending_distribution_earnings_value.clone(),
                                    sub: i18n.t("dashboard.pending_distribution_earnings_desc").to_string()
                                }
                                DashboardStatusMetric {
                                    label: i18n.t("distribution.referral_count").to_string(),
                                    value: distribution_referral_count_value.clone(),
                                    sub: i18n.t("dashboard.referral_count_desc").to_string()
                                }
                            }
                            DashboardStatusMetric {
                                label: i18n.t("dashboard.latest_order").to_string(),
                                value: recent_orders.first()
                                    .map(|order| order.status.clone())
                                    .unwrap_or_else(|| i18n.t("dashboard.none").to_string()),
                                sub: i18n.t("dashboard.latest_order_desc").to_string()
                            }
                        }
                    }
                }
            }
        }
    }
}

fn build_trend_points(records: &[client_api::api::billing::UsageRecord]) -> Vec<TrendPoint> {
    let mut grouped: BTreeMap<String, i32> = BTreeMap::new();
    for record in records.iter() {
        let day = record.created_at.chars().take(10).collect::<String>();
        *grouped.entry(day).or_insert(0) += 1;
    }

    let today = Utc::now().date_naive();
    (0..10)
        .map(|offset| {
            let day = today - Duration::days((9 - offset) as i64);
            let date_key = day.format("%Y-%m-%d").to_string();
            TrendPoint {
                label: day.format("%m-%d").to_string(),
                requests: grouped.get(&date_key).copied().unwrap_or(0),
            }
        })
        .collect()
}

struct TrendSvg {
    line_path: String,
    area_path: String,
    x_points: Vec<f32>,
    y_points: Vec<f32>,
    guides: Vec<f32>,
}

fn build_trend_svg(points: &[TrendPoint]) -> TrendSvg {
    let width = 640.0_f32;
    let height = 220.0_f32;
    let left = 24.0_f32;
    let right = 24.0_f32;
    let top = 24.0_f32;
    let bottom = 34.0_f32;
    let plot_width = width - left - right;
    let baseline_y = height - bottom;
    let plot_height = baseline_y - top;
    let max_value = points
        .iter()
        .map(|point| point.requests)
        .max()
        .unwrap_or(0)
        .max(4) as f32;
    let step = if points.len() > 1 {
        plot_width / (points.len() - 1) as f32
    } else {
        plot_width
    };

    let x_points: Vec<f32> = points
        .iter()
        .enumerate()
        .map(|(index, _)| left + step * index as f32)
        .collect();
    let y_points: Vec<f32> = points
        .iter()
        .map(|point| baseline_y - ((point.requests as f32 / max_value) * plot_height))
        .collect();

    let line_path = x_points
        .iter()
        .zip(y_points.iter())
        .enumerate()
        .map(|(index, (x, y))| {
            if index == 0 {
                format!("M{:.2},{:.2}", x, y)
            } else {
                format!(" L{:.2},{:.2}", x, y)
            }
        })
        .collect::<String>();

    let first_x = x_points.first().copied().unwrap_or(left);
    let last_x = x_points.last().copied().unwrap_or(width - right);
    let area_path = format!(
        "{} L{:.2},{:.2} L{:.2},{:.2} Z",
        line_path, last_x, baseline_y, first_x, baseline_y
    );

    let guides = (0..4)
        .map(|index| top + plot_height * (index as f32 / 3.0))
        .collect();

    TrendSvg {
        line_path,
        area_path,
        x_points,
        y_points,
        guides,
    }
}

fn format_active_key_time(
    last_used_at: Option<&str>,
    last_used_prefix: &str,
    empty_text: &str,
) -> String {
    last_used_at
        .map(|last_used| format!("{last_used_prefix} {}", format_time(last_used)))
        .unwrap_or_else(|| empty_text.to_string())
}

#[component]
fn DashboardStatCard(
    tone: String,
    icon: String,
    title: String,
    value: String,
    meta: String,
) -> Element {
    rsx! {
        div { class: "dashboard-stat-card dashboard-stat-card-{tone}",
            div { class: "dashboard-stat-top",
                div { class: "dashboard-stat-icon", "{icon}" }
                span { class: "dashboard-stat-kicker", "{title}" }
            }
            div { class: "dashboard-stat-value", "{value}" }
            div { class: "dashboard-stat-meta", "{meta}" }
        }
    }
}

#[component]
fn DashboardQuickLink(route: Route, tone: String, title: String, description: String) -> Element {
    let nav = use_navigator();
    rsx! {
        button {
            class: "dashboard-quick-link dashboard-quick-link-{tone}",
            onclick: move |_| {
                nav.push(route.clone());
            },
            div { class: "dashboard-quick-link-main",
                div { class: "dashboard-quick-link-title", "{title}" }
                div { class: "dashboard-quick-link-desc", "{description}" }
            }
            span { class: "dashboard-quick-link-arrow", "→" }
        }
    }
}

#[component]
fn DashboardStatusMetric(label: String, value: String, sub: String) -> Element {
    rsx! {
        div { class: "dashboard-status-metric",
            div { class: "dashboard-status-label", "{label}" }
            div { class: "dashboard-status-value", "{value}" }
            div { class: "dashboard-status-sub", "{sub}" }
        }
    }
}
