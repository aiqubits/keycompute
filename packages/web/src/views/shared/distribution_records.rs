use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, PageHeader, Pagination, Table, TableHead};

const PAGE_SIZE: usize = 20;

use crate::hooks::use_i18n::use_i18n;
use crate::services::{
    api_client::{user_error_message, with_auto_refresh},
    distribution_service,
};
use crate::stores::auth_store::AuthStore;
use crate::stores::user_store::UserStore;
use crate::utils::display::{distribution_status_label, short_id};
use crate::utils::format_precise_cny_str;
use crate::utils::time::format_time;

/// 分销记录页面
///
/// - 普通用户：查看自己的推荐用户明细（真实表格数据）
/// - Admin：查看全平台分销记录，展示分销规则（只读，当前由后端硬编码）
#[component]
pub fn DistributionRecords() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    // 收益数据（普通用户）
    let earnings = use_resource(move || async move {
        if is_admin {
            return Ok(None);
        }
        with_auto_refresh(auth_store, |token| async move {
            distribution_service::get_earnings(&token).await.map(Some)
        })
        .await
    });

    // 普通用户：推荐明细列表
    let referrals = use_resource(move || async move {
        if is_admin {
            return Ok(vec![]);
        }
        with_auto_refresh(auth_store, |token| async move {
            distribution_service::get_referrals(&token).await
        })
        .await
    });

    // Admin：全平台分销记录
    let admin_records = use_resource(move || async move {
        if !is_admin {
            return Ok(vec![]);
        }
        with_auto_refresh(auth_store, |token| async move {
            use crate::services::api_client::get_client;
            use client_api::DistributionApi;
            let client = get_client();
            DistributionApi::new(&client)
                .list_distribution_records(None, &token)
                .await
        })
        .await
    });

    // Admin：分销规则列表（只读展示，后端硬编码）
    let rules = use_resource(move || async move {
        if !is_admin {
            return Ok(vec![]);
        }
        with_auto_refresh(auth_store, |token| async move {
            use crate::services::api_client::get_client;
            use client_api::DistributionApi;
            let client = get_client();
            DistributionApi::new(&client)
                .list_distribution_rules(&token)
                .await
        })
        .await
    });

    let total_earnings = match earnings() {
        Some(Ok(Some(ref e))) => format_precise_cny_str(&e.total_earnings),
        _ => "¥0.00".to_string(),
    };
    let available = match earnings() {
        Some(Ok(Some(ref e))) => format_precise_cny_str(&e.available_earnings),
        _ => "¥0.00".to_string(),
    };
    let pending = match earnings() {
        Some(Ok(Some(ref e))) => format_precise_cny_str(&e.pending_earnings),
        _ => "¥0.00".to_string(),
    };

    let mut page = use_signal(|| 1u32);
    let page_desc = if is_admin {
        i18n.t("distribution_records.admin_desc")
    } else {
        i18n.t("distribution_records.user_desc")
    };

    rsx! {
        div { class: "page-container distribution-records-page",
        PageHeader {
            title: i18n.t("page.distribution_records").to_string(),
            description: page_desc.to_string(),
        }

        // 收益统计只属于个人视图；管理员页不混入当前管理员的个人收益。
        if !is_admin { div { class: "stats-grid",
            div { class: "stat-card card",
                div { class: "card-body",
                    p { class: "stat-label", {i18n.t("distribution.total_earnings")} }
                    p { class: "stat-value", "{total_earnings}" }
                }
            }
            div { class: "stat-card card",
                div { class: "card-body",
                    p { class: "stat-label", {i18n.t("distribution.available_balance")} }
                    p { class: "stat-value", "{available}" }
                }
            }
            div { class: "stat-card card",
                div { class: "card-body",
                    p { class: "stat-label", {i18n.t("distribution.pending")} }
                    p { class: "stat-value", "{pending}" }
                }
            }
        } }

        // 分销规则只读展示（Admin 可见）
        if is_admin {
            div { class: "section",
                h2 { class: "section-title", {i18n.t("distribution_records.rules_title")} }
                div { class: "alert alert-info", style: "margin-bottom: 12px",
                    span { class: "alert-icon", "ℹ" }
                    div { class: "alert-content",
                        p { class: "alert-body",
                            {i18n.t("distribution_records.rules_hint")}
                        }
                    }
                }
                match rules() {
                    None => rsx! { p { class: "text-secondary", {i18n.t("table.loading")} } },
                    Some(Err(ref e)) => rsx! { p { class: "text-secondary", {user_error_message(e)} } },
                    Some(Ok(ref list)) if list.is_empty() => rsx! {
                        p { class: "text-secondary", {i18n.t("distribution_records.no_rules")} }
                    },
                    Some(Ok(ref list)) => rsx! {
                        Table {
                            col_count: 4,
                            thead {
                                tr {
                                    TableHead { {i18n.t("distribution_records.rule_name")} }
                                    TableHead { {i18n.t("distribution_records.commission_rate")} }
                                    TableHead { {i18n.t("table.status")} }
                                    TableHead { {i18n.t("table.created_at")} }
                                }
                            }
                            tbody {
                                for r in list.iter() {
                                    tr {
                                        td { "{r.name}" }
                                        td { { format!("{:.1}%", r.commission_rate * 100.0) } }
                                        td {
                                            if r.is_active {
                                                Badge { variant: BadgeVariant::Success, {i18n.t("common.enabled")} }
                                            } else {
                                                Badge { variant: BadgeVariant::Neutral, {i18n.t("common.disabled")} }
                                            }
                                        }
                                        td { { format_time(&r.created_at) } }
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }

        // 表格：admin 视图 / 普通用户视图分别渲染
        if is_admin {
            {
                let (is_empty, empty_text) = match admin_records() {
                    None => (true, i18n.t("table.loading").to_string()),
                    Some(Err(ref e)) => (true, user_error_message(e)),
                    Some(Ok(ref l)) if l.is_empty() => {
                        (true, i18n.t("distribution_records.empty_admin").to_string())
                    }
                    _ => (false, String::new()),
                };
                let admin_start = (page() as usize - 1) * PAGE_SIZE;
                rsx! {
                    Table {
                        empty: is_empty,
                        empty_text,
                        col_count: 7u32,
                        thead {
                            tr {
                                TableHead { {i18n.t("distribution_records.record_id")} }
                                TableHead { {i18n.t("distribution_records.source_user_id")} }
                                TableHead { {i18n.t("distribution_records.amount_spent")} }
                                TableHead { {i18n.t("distribution_records.commission_amount")} }
                                TableHead { {i18n.t("table.status")} }
                                TableHead { {i18n.t("table.created_at")} }
                                TableHead { {i18n.t("distribution_records.referrer_id")} }
                            }
                        }
                        tbody {
                            if let Some(Ok(ref list)) = admin_records() {
                                for rec in list.iter().skip(admin_start).take(PAGE_SIZE) {
                                    tr {
                                        td { code { title: "{rec.id}", {short_id(&rec.id)} } }
                                        td {
                                            // 截取 UUID 前 8 位＋全量 tooltip
                                            span {
                                                title: "{rec.referred_id}",
                                                style: "cursor: help; font-family: monospace; font-size: 13px;",
                                                {short_id(&rec.referred_id)}
                                            }
                                        }
                                        td { {format_precise_cny_str(&rec.amount)} }
                                        td { {format_precise_cny_str(&rec.commission)} }
                                        td {
                                            Badge {
                                                variant: dist_status_variant(&rec.status),
                                                {distribution_status_label(&rec.status, &i18n)}
                                            }
                                        }
                                        td { { format_time(&rec.created_at) } }
                                        td {
                                            span {
                                                title: "{rec.referrer_id}",
                                                style: "cursor: help; font-family: monospace; font-size: 13px;",
                                                {short_id(&rec.referrer_id)}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else {
            {
                let (is_empty, empty_text) = match referrals() {
                    None => (true, i18n.t("table.loading").to_string()),
                    Some(Err(ref e)) => (true, user_error_message(e)),
                    Some(Ok(ref l)) if l.is_empty() => {
                        (true, i18n.t("distribution_records.empty_user").to_string())
                    }
                    _ => (false, String::new()),
                };
                let ref_start = (page() as usize - 1) * PAGE_SIZE;
                rsx! {
                    Table {
                        empty: is_empty,
                        empty_text,
                        col_count: 4u32,
                        thead {
                            tr {
                                TableHead { {i18n.t("distribution_records.referred_user")} }
                                TableHead { {i18n.t("distribution.joined_at")} }
                                TableHead { {i18n.t("distribution.total_spent")} }
                                TableHead { {i18n.t("distribution.my_earnings")} }
                            }
                        }
                        tbody {
                            if let Some(Ok(ref list)) = referrals() {
                                for r in list.iter().skip(ref_start).take(PAGE_SIZE) {
                                    tr {
                                        td {
                                            div { class: "user-cell",
                                                span { class: "user-name",
                                                    { r.name.clone().unwrap_or_else(|| r.email.clone()) }
                                                }
                                                span { class: "user-email text-secondary", "{r.email}" }
                                            }
                                        }
                                        td { { format_time(&r.joined_at) } }
                                        td { {format_precise_cny_str(&r.total_spent)} }
                                        td { {format_precise_cny_str(&r.earnings_from_referral)} }
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
                admin_records().and_then(|r| r.ok()).map(|l| l.len()).unwrap_or(0)
            } else {
                referrals().and_then(|r| r.ok()).map(|l| l.len()).unwrap_or(0)
            };
            let total_pages = total.div_ceil(PAGE_SIZE).max(1) as u32;
            rsx! {
                div { class: "pagination",
                    span { class: "pagination-info", "{i18n.t(\"common.total_items\")} {total} {i18n.t(\"pricing.items_suffix\")}" }
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

fn dist_status_variant(status: &str) -> BadgeVariant {
    match status {
        "settled" | "paid" => BadgeVariant::Success,
        "pending" => BadgeVariant::Warning,
        "cancelled" | "failed" => BadgeVariant::Error,
        _ => BadgeVariant::Neutral,
    }
}
