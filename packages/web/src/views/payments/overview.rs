use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, Table, TableHead};

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;
use crate::services::{api_client::with_auto_refresh, billing_service, payment_service};
use crate::stores::auth_store::AuthStore;
use crate::utils::time::format_time;

/// 支付与账单页面 - /payments
///
/// 包含：账户余额、充值记录和账单统计
#[component]
pub fn PaymentsOverview() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();

    let nav = use_navigator();
    let balance = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            payment_service::get_balance(&token).await
        })
        .await
    });

    let payment_methods = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            payment_service::get_methods(&token).await
        })
        .await
    });

    let orders = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            payment_service::list_orders(None, &token).await
        })
        .await
    });

    // 用量统计（真实数据，来自 usage_logs 表）
    let usage_stats = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            billing_service::stats(&token).await
        })
        .await
    });

    rsx! {
        div {
            class: "page-container",
            div {
                class: "page-header",
                h1 { class: "page-title", {i18n.t("payments.title")} }
                p { class: "page-subtitle", {i18n.t("payments.subtitle")} }
                if matches!(payment_methods(), Some(Ok(ref methods)) if !methods.methods.is_empty()) {
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| { nav.push(Route::Recharge {}); },
                        {i18n.t("payments.recharge_now")}
                    }
                }
            }

            // ─── 账户余额 ───
            div { class: "stats-grid",
                div {
                    class: "stat-card",
                    p { class: "stat-title", {i18n.t("payments.account_balance")} }
                    match balance() {
                        None => rsx! { p { class: "stat-value", {i18n.t("table.loading")} } },
                        Some(Err(e)) => rsx! { p { class: "stat-value text-error", "{i18n.t(\"common.error\")}: {e}" } },
                        Some(Ok(b)) => rsx! {
                            p { class: "stat-value", "¥ {crate::utils::format_money_str(&b.available_balance)}" }
                        },
                    }
                }
                div {
                    class: "stat-card",
                    p { class: "stat-title", {i18n.t("payments.frozen_amount")} }
                    match balance() {
                        Some(Ok(b)) => rsx! { p { class: "stat-value", "¥ {crate::utils::format_money_str(&b.frozen_balance)}" } },
                        _ => rsx! { p { class: "stat-value", "—" } },
                    }
                }
                div {
                    class: "stat-card",
                    p { class: "stat-title", {i18n.t("payments.total_recharge")} }
                    match balance() {
                        Some(Ok(b)) => rsx! { p { class: "stat-value", "¥ {crate::utils::format_money_str(&b.total_recharged)}" } },
                        _ => rsx! { p { class: "stat-value", "—" } },
                    }
                }
                div {
                    class: "stat-card",
                    p { class: "stat-title", {i18n.t("payments.total_consumed")} }
                    match balance() {
                        Some(Ok(b)) => rsx! { p { class: "stat-value", "¥ {crate::utils::format_money_str(&b.total_consumed)}" } },
                        _ => rsx! { p { class: "stat-value", "—" } },
                    }
                }
                match usage_stats() {
                    Some(Ok(s)) => rsx! {
                        div { class: "stat-card",
                            p { class: "stat-title", {i18n.t("payments.usage_requests")} }
                            p { class: "stat-value", "{s.total_requests}" }
                        }
                        div { class: "stat-card",
                            p { class: "stat-title", {i18n.t("payments.input_tokens")} }
                            p { class: "stat-value", "{s.input_tokens}" }
                        }
                        div { class: "stat-card",
                            p { class: "stat-title", {i18n.t("payments.output_tokens")} }
                            p { class: "stat-value", "{s.output_tokens}" }
                        }
                        div { class: "stat-card",
                            p { class: "stat-title", {i18n.t("payments.total_cost")} }
                            p { class: "stat-value", "¥{crate::utils::format_money(s.total_cost)}" }
                        }
                    },
                    _ => rsx! {},
                }
            }

            // ─── 充値记录 ───
            div { class: "section",
                h2 { class: "section-title", {i18n.t("payments.recharge_records")} }
                match orders() {
                    None => rsx! { div { class: "loading-state", {i18n.t("table.loading")} } },
                    Some(Err(e)) => rsx! { div { class: "alert alert-error", "{i18n.t(\"common.load_failed\")}：{e}" } },
                    Some(Ok(list)) => {
                        if list.is_empty() {
                            rsx! { div { class: "empty-state", p { {i18n.t("payments.no_recharge_records")} } } }
                        } else {
                            rsx! {
                                Table {
                                    col_count: 5,
                                    thead {
                                        tr {
                                            TableHead { {i18n.t("payments.order_no")} }
                                            TableHead { {i18n.t("common.amount")} }
                                            TableHead { {i18n.t("payments.subject")} }
                                            TableHead { {i18n.t("table.status")} }
                                            TableHead { {i18n.t("common.time")} }
                                        }
                                    }
                                    tbody {
                                        for order in list.iter() {
                                            tr {
                                                key: "{order.id}",
                                                td { code { "{order.out_trade_no}" } }
                                                td { "¥ {crate::utils::format_money_str(&order.amount)}" }
                                                td { "{order.subject}" }
                                                td {
                                                    Badge {
                                                        variant: payment_status_variant(&order.status),
                                                        "{order.status}"
                                                    }
                                                }
                                                td { { format_time(&order.created_at) } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn payment_status_variant(status: &str) -> BadgeVariant {
    match status {
        "paid" | "success" => BadgeVariant::Success,
        "pending" | "processing" => BadgeVariant::Warning,
        "failed" | "cancelled" => BadgeVariant::Error,
        _ => BadgeVariant::Neutral,
    }
}
