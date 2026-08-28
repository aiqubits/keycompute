use client_api::{AdminApi, api::admin::PaymentQueryParams as AdminPaymentQueryParams};
use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, PageHeader, Pagination, Table, TableHead};

const PAGE_SIZE: usize = 20;

use crate::hooks::use_i18n::use_i18n;
use crate::services::{
    api_client::{get_client, with_auto_refresh},
    payment_service,
};
use crate::stores::auth_store::AuthStore;
use crate::stores::user_store::UserStore;
use crate::utils::display::{
    payment_provider_message, payment_provider_status_label, payment_status_label, short_id,
};
use crate::utils::format_cny_str;
use crate::utils::time::format_time;

/// 支付订单页面
///
/// - 普通用户：仅查看自己的订单
/// - Admin：查看所有订单
#[component]
pub fn PaymentOrders() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    let mut status_filter = use_signal(|| "all".to_string());
    let mut page = use_signal(|| 1u32);

    // 普通用户订单
    let my_orders = use_resource(move || async move {
        if is_admin {
            return Ok(client_api::api::payment::PaymentOrderPage::default());
        }
        let status = status_filter();
        let mut params = client_api::api::payment::PaymentQueryParams::new()
            .with_page(page())
            .with_page_size(PAGE_SIZE as u32);
        if status != "all" {
            params = params.with_status(status.clone());
        }
        with_auto_refresh(auth_store, |token| {
            let value = params.clone();
            async move { payment_service::list_orders_page(Some(value), &token).await }
        })
        .await
    });

    // Admin 订单
    let admin_orders = use_resource(move || async move {
        if !is_admin {
            return Ok(client_api::api::admin::PaymentOrderPage::default());
        }
        let status = status_filter();
        let mut params = AdminPaymentQueryParams::new()
            .with_page(page())
            .with_page_size(PAGE_SIZE as u32);
        if status != "all" {
            params = params.with_status(status.clone());
        }
        with_auto_refresh(auth_store, |token| {
            let value = params.clone();
            async move {
                let client = get_client();
                AdminApi::new(&client)
                    .list_payment_orders_page(Some(&value), &token)
                    .await
            }
        })
        .await
    });

    let mut provider_statuses = use_resource(move || async move {
        if !is_admin {
            return Ok(vec![]);
        }
        with_auto_refresh(auth_store, |token| async move {
            let client = get_client();
            AdminApi::new(&client).get_payment_providers(&token).await
        })
        .await
    });
    let mut verifying_provider = use_signal(|| None::<String>);
    let mut provider_action_error = use_signal(|| None::<String>);

    let filter_labels = [
        ("all", i18n.t("payment_orders.filter_all")),
        ("pending", i18n.t("payment_orders.filter_pending")),
        ("paid", i18n.t("payment_orders.filter_paid")),
        ("failed", i18n.t("payment_orders.filter_failed")),
        ("closed", i18n.t("payment_orders.filter_closed")),
    ];
    let page_description = if is_admin {
        i18n.t("payment_orders.subtitle_admin")
    } else {
        i18n.t("payment_orders.subtitle_user")
    };

    rsx! {
        div { class: "page-container payment-orders-page",
        PageHeader {
            title: i18n.t("page.payment_orders").to_string(),
            description: page_description.to_string(),
        }

        if is_admin {
            if let Some(error) = provider_action_error() {
                div { class: "alert alert-error", "{error}" }
            }
            div { class: "payment-provider-grid",
                match provider_statuses() {
                    None => rsx! { div { class: "loading-state", {i18n.t("table.loading")} } },
                    Some(Err(error)) => rsx! {
                        div { class: "alert alert-error", "{i18n.t(\"common.load_failed\")}：{error}" }
                    },
                    Some(Ok(providers)) => rsx! {
                        for provider in providers {
                            div { class: "payment-provider-card",
                                div { class: "payment-provider-head",
                                    div {
                                        h3 { class: "payment-provider-name",
                                            if provider.code == "alipay" {
                                                {i18n.t("recharge.alipay")}
                                            } else if provider.code == "wechatpay" {
                                                {i18n.t("recharge.wechat_pay")}
                                            } else {
                                                "{provider.display_name}"
                                            }
                                        }
                                        p { class: "payment-provider-scenes", {provider.scenes.join(" · ")} }
                                    }
                                    Badge {
                                        variant: status_to_variant(&provider.status),
                                        {payment_provider_status_label(&provider.status, &i18n)}
                                    }
                                }
                                div { class: "payment-provider-meta",
                                    span { "{i18n.t(\"payment_orders.provider_switch\")}: "
                                        strong { if provider.enabled { {i18n.t("common.enabled")} } else { {i18n.t("common.disabled")} } }
                                    }
                                    span { "{i18n.t(\"payment_orders.provider_config\")}: "
                                        strong { if provider.configured { {i18n.t("common.configured")} } else { {i18n.t("common.not_configured")} } }
                                    }
                                }
                                if let Some(message) = payment_provider_message(&provider.status, &i18n) {
                                    p { class: "payment-provider-message", "{message}" }
                                }
                                if provider.configured && !provider.available {
                                    {
                                        let method = provider.code.clone();
                                        let is_verifying = verifying_provider().as_deref() == Some(method.as_str());
                                        rsx! {
                                            button {
                                                class: "btn btn-primary btn-sm payment-provider-action",
                                                r#type: "button",
                                                disabled: verifying_provider().is_some(),
                                                onclick: move |_| {
                                                    let method = method.clone();
                                                    verifying_provider.set(Some(method.clone()));
                                                    provider_action_error.set(None);
                                                    spawn(async move {
                                                        let result = with_auto_refresh(auth_store, |token| {
                                                            let method = method.clone();
                                                            async move {
                                                                let client = get_client();
                                                                AdminApi::new(&client)
                                                                    .verify_payment_provider(&method, &token)
                                                                    .await
                                                            }
                                                        })
                                                        .await;
                                                        if let Err(error) = result {
                                                            provider_action_error.set(Some(error.to_string()));
                                                        } else {
                                                            provider_statuses.restart();
                                                        }
                                                        verifying_provider.set(None);
                                                    });
                                                },
                                                if is_verifying {
                                                    {i18n.t("payment_orders.verifying_provider")}
                                                } else {
                                                    {i18n.t("payment_orders.verify_provider")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }

        // 状态筛选
        div { class: "toolbar",
            div { class: "toolbar-left",
                div { class: "filter-tabs",
                    for (val , label) in filter_labels {
                        button {
                            class: if status_filter() == val { "filter-tab active" } else { "filter-tab" },
                            r#type: "button",
                            onclick: {
                                let val = val.to_string();
                                move |_| {
                                    *status_filter.write() = val.clone();
                                    page.set(1);
                                }
                            },
                            "{label}"
                        }
                    }
                }
            }
        }

        div { class: "card",
            if is_admin {
                {
                    let (is_empty, empty_text) = match admin_orders() {
                        None => (true, i18n.t("table.loading")),
                        Some(Err(_)) => (true, i18n.t("common.load_failed")),
                        Some(Ok(ref result)) if result.orders.is_empty() => (true, i18n.t("payment_orders.empty")),
                        _ => (false, ""),
                    };
                    rsx! {
                        Table { empty: is_empty, empty_text: empty_text.to_string(), col_count: 6,
                            thead {
                                tr {
                                    TableHead { {i18n.t("payments.order_no")} }
                                    TableHead { {i18n.t("payment_orders.col_user")} }
                                    TableHead { {i18n.t("recharge.payment_method")} }
                                    TableHead { {i18n.t("common.amount")} }
                                    TableHead { {i18n.t("table.status")} }
                                    TableHead { {i18n.t("table.created_at")} }
                                }
                            }
                            tbody {
                                if let Some(Ok(ref result)) = admin_orders() {
                                    for o in &result.orders {
                                        tr {
                                            td {
                                                code { "{o.out_trade_no}" }
                                            }
                                            td {
                                                {
                                                    let uid = o.user_id.clone();
                                                    let short = short_id(&uid);
                                                    rsx! {
                                                        span {
                                                            title: "{uid}",
                                                            style: "cursor:help;font-family:monospace;font-size:13px;",
                                                            "{short}"
                                                        }
                                                    }
                                                }
                                            }
                                            td { "{o.payment_method}" }
                                            td { {format_cny_str(&o.amount)} }
                                            td {
                                                Badge { variant: status_to_variant(&o.status), {payment_status_label(&o.status, &i18n)} }
                                            }
                                            td { {format_time(&o.created_at)} }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                {
                    let (is_empty, empty_text) = match my_orders() {
                        None => (true, i18n.t("table.loading")),
                        Some(Err(_)) => (true, i18n.t("common.load_failed")),
                        Some(Ok(ref result)) if result.orders.is_empty() => (true, i18n.t("payment_orders.empty")),
                        _ => (false, ""),
                    };
                    rsx! {
                        Table { empty: is_empty, empty_text: empty_text.to_string(), col_count: 5,
                            thead {
                                tr {
                                    TableHead { {i18n.t("payments.order_no")} }
                                    TableHead { {i18n.t("common.amount")} }
                                    TableHead { {i18n.t("payments.subject")} }
                                    TableHead { {i18n.t("table.status")} }
                                    TableHead { {i18n.t("table.created_at")} }
                                }
                            }
                            tbody {
                                if let Some(Ok(ref result)) = my_orders() {
                                    for o in &result.orders {
                                        tr {
                                            td {
                                                code { "{o.out_trade_no}" }
                                            }
                                            td { {format_cny_str(&o.amount)} }
                                            td { "{o.subject}" }
                                            td {
                                                Badge { variant: status_to_variant(&o.status), {payment_status_label(&o.status, &i18n)} }
                                            }
                                            td { {format_time(&o.created_at)} }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        {
            let total = if is_admin {
                admin_orders().and_then(|r| r.ok()).map(|result| result.total as usize).unwrap_or(0)
            } else {
                my_orders().and_then(|r| r.ok()).map(|result| result.total as usize).unwrap_or(0)
            };
            let total_pages = if is_admin {
                admin_orders()
                    .and_then(|result| result.ok())
                    .map(|result| result.total_pages.max(1))
                    .unwrap_or(1)
            } else {
                total.div_ceil(PAGE_SIZE).max(1) as u32
            };
            rsx! {
                div { class: "pagination",
                    span { class: "pagination-info",
                        {i18n.t_with_args("payment_orders.pagination", &[("total", &total.to_string())])}
                    }
                    Pagination {
                        current: page(),
                        total_pages,
                        previous_label: i18n.t("table.previous").to_string(),
                        next_label: i18n.t("table.next").to_string(),
                        on_page_change: move |p| page.set(p),
                    }
                }
            }
        }
        }
    }
}

fn status_to_variant(status: &str) -> BadgeVariant {
    match status {
        "paid" | "success" => BadgeVariant::Success,
        "pending" | "processing" => BadgeVariant::Warning,
        "failed" | "cancelled" => BadgeVariant::Error,
        _ => BadgeVariant::Neutral,
    }
}
