use client_api::api::tenant::TenantInfo;
use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, PageHeader, Pagination, Table, TableHead};

const PAGE_SIZE: usize = 20;

use crate::hooks::use_i18n::use_i18n;
use crate::services::{api_client::with_auto_refresh, tenant_service};
use crate::stores::auth_store::AuthStore;
use crate::stores::user_store::UserStore;
use crate::utils::display::short_id;
use crate::utils::time::format_time;
use crate::views::shared::accounts::NoPermissionView;

/// 租户管理页面（仅 Admin 可访问）
///
/// - 普通用户：无权限提示
/// - Admin：查看全平台租户列表（调用 TenantApi）
#[component]
pub fn Tenants() -> Element {
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
        return rsx! { NoPermissionView { resource: i18n.t("page.tenants").to_string() } };
    }

    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 1u32);

    let tenants = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            tenant_service::list(None, &token).await
        })
        .await
    });

    let filtered = move || -> Vec<TenantInfo> {
        let q = search().to_lowercase();
        match tenants() {
            Some(Ok(ref list)) => list
                .iter()
                .filter(|t| {
                    q.is_empty()
                        || t.id.to_lowercase().contains(&q)
                        || t.name.to_lowercase().contains(&q)
                })
                .cloned()
                .collect::<Vec<_>>(),
            _ => vec![],
        }
    };

    let total_pages = move || {
        let len = filtered().len();
        len.div_ceil(PAGE_SIZE).max(1) as u32
    };

    let paged = move || {
        let p = page() as usize;
        let all = filtered();
        let start = (p - 1) * PAGE_SIZE;
        all.into_iter()
            .skip(start)
            .take(PAGE_SIZE)
            .collect::<Vec<_>>()
    };

    rsx! {
        div { class: "page-container tenants-page",
        PageHeader {
            title: i18n.t("page.tenants").to_string(),
            description: i18n.t("tenants.subtitle").to_string(),
        }

        div { class: "toolbar",
            div { class: "toolbar-left",
                div { class: "input-wrapper",
                    input {
                        class: "input-field",
                        r#type: "search",
                        placeholder: "{i18n.t(\"tenants.search_placeholder\")}",
                        value: "{search}",
                        oninput: move |e| {
                            *search.write() = e.value();
                            page.set(1);
                        },
                    }
                }
            }
        }

        {
            let (is_empty, empty_text) = match tenants() {
                None => (true, i18n.t("table.loading")),
                Some(Err(_)) => (true, i18n.t("common.load_failed")),
                Some(Ok(_)) if filtered().is_empty() => (true, i18n.t("tenants.empty")),
                _ => (false, ""),
            };
            rsx! {
                Table {
                    empty: is_empty,
                    empty_text: empty_text.to_string(),
                    col_count: 4,
                    thead {
                        tr {
                            TableHead { {i18n.t("tenants.tenant_id")} }
                            TableHead { {i18n.t("table.name")} }
                            TableHead { {i18n.t("table.status")} }
                            TableHead { {i18n.t("table.created_at")} }
                        }
                    }
                    tbody {
                        for t in paged().iter() {
                            tr {
                                td { code { title: "{t.id}", {short_id(&t.id)} } }
                                td { "{t.name}" }
                                td {
                                    if t.is_active {
                                        Badge { variant: BadgeVariant::Success, {i18n.t("tenants.active")} }
                                    } else {
                                        Badge { variant: BadgeVariant::Neutral, {i18n.t("common.disabled")} }
                                    }
                                }
                                td { { format_time(&t.created_at) } }
                            }
                        }
                    }
                }
            }
        }

        div { class: "pagination",
            span { class: "pagination-info",
                "{i18n.t(\"common.total_items\")} {filtered().len()} {i18n.t(\"pricing.items_suffix\")}"
            }
            Pagination {
                current: page(),
                total_pages: total_pages(),
                previous_label: i18n.t("table.previous").to_string(),
                next_label: i18n.t("table.next").to_string(),
                on_page_change: move |p| page.set(p),
            }
        }
        }
    }
}
