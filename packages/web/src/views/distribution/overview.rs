use client_api::error::ClientError;
use dioxus::prelude::*;

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;
use crate::services::{
    api_client::{user_error_message, with_auto_refresh},
    distribution_service,
};
use crate::stores::{
    auth_store::AuthStore, public_settings_store::PublicSettingsStore, ui_store::UiStore,
};
use crate::utils::on_copy;
use crate::utils::time::format_time;
use ui::icons::IconCopy;

fn is_distribution_disabled_error<T>(result: &Option<Result<T, ClientError>>) -> bool {
    matches!(
        result,
        Some(Err(ClientError::Forbidden(msg))) if msg.contains("Distribution is disabled")
    )
}

#[component]
pub fn DistributionOverview() -> Element {
    let i18n = use_i18n();
    let public_settings_store = use_context::<PublicSettingsStore>();
    let mut ui_store = use_context::<UiStore>();
    let nav = use_navigator();

    use_effect(move || {
        if public_settings_store.loaded()
            && matches!(public_settings_store.distribution_enabled(), Some(false))
        {
            ui_store.show_error(i18n.t("distribution.disabled_message"));
            nav.replace(Route::Dashboard {});
        }
    });

    if !public_settings_store.loaded() {
        return rsx! {
            div { class: "page-container",
                div {
                    class: "distribution-loading",
                    style: "display:flex;align-items:center;justify-content:center;padding:64px",
                    div { style: "display:flex;align-items:center;gap:12px;color:var(--text-secondary,#64748b)",
                        div { class: "spinner", style: "width:24px;height:24px" }
                        span { {i18n.t("table.loading")} }
                    }
                }
            }
        };
    }

    if matches!(public_settings_store.distribution_enabled(), Some(false)) {
        return rsx! {};
    }

    rsx! {
        DistributionOverviewContent {}

    }
}

#[component]
fn DistributionOverviewContent() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let ui_store = use_context::<UiStore>();
    // 收益数据
    let earnings = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            distribution_service::get_earnings(&token).await
        })
        .await
    });

    // 推荐码
    let referral_code = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            distribution_service::get_referral_code(&token).await
        })
        .await
    });

    // 推荐列表
    let referrals = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            distribution_service::get_referrals(&token).await
        })
        .await
    });

    let total_earnings = match earnings() {
        Some(Ok(ref e)) => format!("¥{}", e.total_earnings),
        Some(Err(ref e)) => user_error_message(e),
        None => i18n.t("table.loading").to_string(),
    };
    let available_earnings = match earnings() {
        Some(Ok(ref e)) => format!("¥{}", e.available_earnings),
        _ => "—".to_string(),
    };
    let pending_earnings = match earnings() {
        Some(Ok(ref e)) => format!("¥{}", e.pending_earnings),
        _ => "—".to_string(),
    };
    let referral_count = match earnings() {
        Some(Ok(ref e)) => e.referral_count.to_string(),
        _ => "—".to_string(),
    };
    let invite_link = match referral_code() {
        Some(Ok(ref r)) => r.referral_link.clone(),
        Some(Err(ref e)) => user_error_message(e),
        None => i18n.t("table.loading").to_string(),
    };
    let link_ready = matches!(referral_code(), Some(Ok(_)));
    let invite_link_text = invite_link.clone();
    let copied = use_signal(|| false);
    let copied_label = i18n.t("common.copied");
    let copy_label = i18n.t("common.copy");
    let copy_manual_hint = i18n.t("common.copy_manual_hint");
    let distribution_disabled = is_distribution_disabled_error(&earnings())
        || is_distribution_disabled_error(&referral_code())
        || is_distribution_disabled_error(&referrals());

    rsx! {
        div { class: "page-container",
            div { class: "page-header",
                h1 { class: "page-title", {i18n.t("distribution.title")} }
                p { class: "page-subtitle", {i18n.t("distribution.subtitle")} }
            }

            if distribution_disabled {
                div { class: "card",
                    div { class: "card-body",
                        div { class: "empty-state",
                            div { class: "empty-icon", "⛔" }
                            h3 { class: "empty-title", {i18n.t("distribution.disabled_title")} }
                            p { class: "empty-text", {i18n.t("distribution.disabled_desc")} }
                        }
                    }
                }
            } else {
                // 收益统计
                div { class: "stats-grid",
                    div { class: "stat-card card",
                        div { class: "card-body",
                            p { class: "stat-label", {i18n.t("distribution.total_earnings")} }
                            p { class: "stat-value", "{total_earnings}" }
                        }
                    }
                    div { class: "stat-card card",
                        div { class: "card-body",
                            p { class: "stat-label", {i18n.t("distribution.available_balance")} }
                            p { class: "stat-value", "{available_earnings}" }
                        }
                    }
                    div { class: "stat-card card",
                        div { class: "card-body",
                            p { class: "stat-label", {i18n.t("distribution.pending")} }
                            p { class: "stat-value", "{pending_earnings}" }
                        }
                    }
                    div { class: "stat-card card",
                        div { class: "card-body",
                            p { class: "stat-label", {i18n.t("distribution.referral_count")} }
                            p { class: "stat-value", "{referral_count}" }
                        }
                    }
                }

                // 我的邀请链接
                div { class: "card",
                    div { class: "card-header",
                        h3 { class: "card-title", {i18n.t("distribution.my_invite_link")} }
                    }
                    div { class: "card-body",
                        // 链接就绪时才渲染复制块，避免加载中/错误文案以邀请链接样式展示误导用户
                        if link_ready {
                            div { class: "distribution-invite-copy-section",
                                div { class: "kc-api-copy-block",
                                    pre {
                                        class: if copied() { "kc-api-example copied" } else { "kc-api-example" },
                                        title: if copied() { copied_label } else { copy_label },
                                        "{invite_link_text}"
                                    }
                                    button {
                                        class: "kc-api-copy-button",
                                        r#type: "button",
                                        onclick: on_copy(invite_link_text.clone(), copy_manual_hint.to_string(), ui_store, copied),
                                        IconCopy { size: 15 }
                                        if copied() {
                                            {copied_label}
                                        } else {
                                            {copy_label}
                                        }
                                    }
                                }
                            }
                        } else {
                            p { class: "empty-text", "{invite_link_text}" }
                        }
                    }
                }

                // 推荐列表
                div { class: "card",
                    div { class: "card-header",
                        h3 { class: "card-title", {i18n.t("distribution.referral_users")} }
                    }
                    div { class: "table-container",
                        table { class: "table",
                            thead {
                                tr {
                                    th { {i18n.t("distribution.user")} }
                                    th { {i18n.t("distribution.joined_at")} }
                                    th { {i18n.t("distribution.total_spent")} }
                                    th { {i18n.t("distribution.my_earnings")} }
                                }
                            }
                            tbody {
                                match referrals() {
                                    Some(Ok(ref list)) if !list.is_empty() => rsx! {
                                        for r in list.iter() {
                                            tr {
                                                td {
                                                    div { class: "user-cell",
                                                        span { class: "user-name", {r.name.clone().unwrap_or_else(|| r.email.clone())} }
                                                        span { class: "user-email", "{r.email}" }
                                                    }
                                                }
                                                td { {format_time(&r.joined_at)} }
                                                td { "¥{r.total_spent}" }
                                                td { "¥{r.earnings_from_referral}" }
                                            }
                                        }
                                    },
                                    Some(Err(_)) => rsx! {
                                        tr {
                                            td { colspan: "4", class: "table-empty", {i18n.t("common.load_failed")} }
                                        }
                                    },
                                    _ => rsx! {
                                        tr {
                                            td { colspan: "4", class: "table-empty", {i18n.t("distribution.no_referrals")} }
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
