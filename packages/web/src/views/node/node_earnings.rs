use dioxus::prelude::*;
use ui::{
    Alert, AlertVariant, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, PageHeader, Table,
    TableHead,
};

use crate::hooks::use_i18n::use_i18n;
use crate::services::node_tips_service;
use crate::stores::{auth_store::AuthStore, ui_store::UiStore};
use crate::utils::{display::short_id, format_precise_cny_str, time::format_time};

const HISTORY_PAGE_SIZE: u32 = 20;

/// 计算简单分页当前页可见的记录范围（1-based，含首尾）。
/// `offset` 为已跳过的记录数；结束值截断到 total；total 为 0 时返回 (0, 0)。
fn visible_range(offset: usize, page_size: usize, total: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let start = offset + 1;
    let end = (offset + page_size).min(total);
    (start, end)
}

/// 提现方式
#[derive(Clone, PartialEq)]
enum WithdrawMethod {
    Alipay,
    Balance,
}

impl WithdrawMethod {
    fn value(&self) -> &'static str {
        match self {
            WithdrawMethod::Alipay => "alipay",
            WithdrawMethod::Balance => "balance",
        }
    }
    fn label(&self) -> String {
        let i18n = use_i18n();
        match self {
            WithdrawMethod::Alipay => i18n.t("node_earnings.method_alipay").to_string(),
            WithdrawMethod::Balance => i18n.t("node_earnings.method_balance").to_string(),
        }
    }
}

/// 提现弹窗状态
#[derive(Clone, PartialEq)]
enum WithdrawModalState {
    Closed,
    Open,
}

/// 提现记录状态枚举
#[derive(Clone, PartialEq)]
enum WithdrawalStatus {
    Pending,
    Approved,
    Completed,
    Rejected,
}

impl WithdrawalStatus {
    fn variant(&self) -> BadgeVariant {
        match self {
            WithdrawalStatus::Pending => BadgeVariant::Warning,
            WithdrawalStatus::Approved => BadgeVariant::Info,
            WithdrawalStatus::Completed => BadgeVariant::Success,
            WithdrawalStatus::Rejected => BadgeVariant::Error,
        }
    }
    fn label(&self) -> String {
        let i18n = use_i18n();
        match self {
            WithdrawalStatus::Pending => i18n.t("node_earnings.status_pending").to_string(),
            WithdrawalStatus::Approved => i18n.t("node_earnings.status_approved").to_string(),
            WithdrawalStatus::Completed => i18n.t("node_earnings.status_completed").to_string(),
            WithdrawalStatus::Rejected => i18n.t("node_earnings.status_rejected").to_string(),
        }
    }
}

#[component]
pub fn NodeEarnings() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let _ui_store = use_context::<UiStore>();

    // 汇总数据 resource
    let summary_resource = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            let token = auth.token().unwrap_or_default();
            node_tips_service::get_my_tips_summary(&token).await
        }
    });

    // 小费历史
    let mut history_offset = use_signal(|| 0u32);
    let history_resource = use_resource(move || {
        let auth = auth_store.clone();
        let offset = *history_offset.read();
        async move {
            let token = auth.token().unwrap_or_default();
            node_tips_service::get_my_tips_history(&token, HISTORY_PAGE_SIZE, offset).await
        }
    });

    // 当前页可见记录范围（供小费历史简单分页展示）；total 为 0 时返回 (0, 0)
    let history_total = history_resource()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|resp| resp.total.max(0) as usize)
        .unwrap_or(0);
    let (range_start, range_end) = visible_range(
        *history_offset.read() as usize,
        HISTORY_PAGE_SIZE as usize,
        history_total,
    );

    // 提现记录
    let withdrawals_resource = use_resource(move || {
        let auth = auth_store.clone();
        async move {
            let token = auth.token().unwrap_or_default();
            node_tips_service::get_my_withdrawals(&token).await
        }
    });

    // 提现弹窗状态
    #[allow(unused_mut)]
    let mut withdraw_modal = use_signal(|| WithdrawModalState::Closed);
    #[allow(unused_mut)]
    let mut withdraw_method = use_signal(|| WithdrawMethod::Balance);
    #[allow(unused_mut)]
    let mut alipay_account = use_signal(String::new);
    #[allow(unused_mut)]
    let mut real_name = use_signal(String::new);
    #[allow(unused_mut)]
    let mut withdraw_loading = use_signal(|| false);
    #[allow(unused_mut)]
    let mut withdraw_error = use_signal(|| None::<String>);

    let show_modal = matches!(*withdraw_modal.read(), WithdrawModalState::Open);

    rsx! {
        div { class: "page-container node-earnings-page",
            PageHeader {
                title: i18n.t("page.node_earnings").to_string(),
                description: i18n.t("node_earnings.subtitle").to_string(),
                actions: rsx! {
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Medium,
                        onclick: move |_| {
                            withdraw_error.set(None);
                            withdraw_modal.set(WithdrawModalState::Open);
                        },
                        {i18n.t("node_earnings.withdraw_btn")}
                    }
                },
            }

            // 错误提示
            if let Some(Err(ref e)) = summary_resource().as_ref().map(|r| r.as_ref()) {
                Alert { variant: AlertVariant::Error, "{i18n.t(\"common.load_failed\")}：{e}" }
            }

            // 汇总卡片
            div { class: "earnings-summary-grid",
                if let Some(Ok(summary)) = summary_resource().as_ref().map(|r| r.as_ref()) {
                    EarningsCard {
                        label: i18n.t("node_earnings.pending_amount"),
                        value: format_precise_cny_str(&summary.pending_amount),
                        meta: i18n.t_with_args(
                            "node_earnings.pending_count",
                            &[("count", &summary.pending_count.to_string())],
                        ),
                        variant: "warning",
                    }
                    EarningsCard {
                        label: i18n.t("node_earnings.withdrawn_amount"),
                        value: format_precise_cny_str(&summary.withdrawn_amount),
                        meta: i18n.t("node_earnings.withdrawn_meta"),
                        variant: "success",
                    }
                    EarningsCard {
                        label: i18n.t("node_earnings.total_amount"),
                        value: format_precise_cny_str(&summary.total_amount),
                        meta: i18n.t("node_earnings.total_meta"),
                        variant: "info",
                    }
                } else if summary_resource().is_none() {
                    div { class: "earnings-card-skeleton" }
                    div { class: "earnings-card-skeleton" }
                    div { class: "earnings-card-skeleton" }
                }
            }

            // 小费历史
            div { class: "card",
                div { class: "card-header",
                    h3 { class: "card-title", {i18n.t("node_earnings.history_title")} }
                }
                div { class: "card-body",
                    match history_resource().as_ref().map(|r| r.as_ref()) {
                        None => rsx! {
                            div { class: "text-secondary", {i18n.t("table.loading")} }
                        },
                        Some(Err(e)) => rsx! {
                            Alert { variant: AlertVariant::Error, "{e}" }
                        },
                        Some(Ok(resp)) => rsx! {
                            Table {
                                empty: resp.items.is_empty(),
                                empty_text: i18n.t("node_earnings.no_history"),
                                col_count: 5,
                                thead {
                                    tr {
                                        TableHead { {i18n.t("node_earnings.col_time")} }
                                        TableHead { {i18n.t("node_earnings.col_bill_amount")} }
                                        TableHead { {i18n.t("node_earnings.col_tip_amount")} }
                                        TableHead { {i18n.t("node_earnings.col_tip_ratio")} }
                                        TableHead { "ID" }
                                    }
                                }
                                tbody {
                                    for item in resp.items.iter() {
                                        tr {
                                            td { {format_time(&item.created_at)} }
                                            td { {format_precise_cny_str(&item.bill_amount)} }
                                            td { {format_precise_cny_str(&item.tip_amount)} }
                                            td { "{item.tip_ratio}" }
                                            td { class: "text-muted", title: "{item.id}", {short_id(&item.id)} }
                                        }
                                    }
                                }
                            }

                            // 简单分页
                            if resp.total > HISTORY_PAGE_SIZE as i64 {
                                div { class: "pagination-simple",
                                    Button {
                                        variant: ButtonVariant::Ghost,
                                        size: ButtonSize::Small,
                                        disabled: *history_offset.read() == 0,
                                        onclick: move |_| {
                                            let cur = *history_offset.read();
                                            *history_offset.write() = cur.saturating_sub(HISTORY_PAGE_SIZE);
                                        },
                                        {i18n.t("common.back")}
                                    }
                                    span { class: "pagination-info",
                                        "{i18n.t(\"common.range\")} {range_start}-{range_end} / {i18n.t(\"common.total_items\")} {history_total}"
                                    }
                                    Button {
                                        variant: ButtonVariant::Ghost,
                                        size: ButtonSize::Small,
                                        disabled: (*history_offset.read() + HISTORY_PAGE_SIZE) as i64 >= resp.total,
                                        onclick: move |_| {
                                            let cur = *history_offset.read();
                                            *history_offset.write() = cur + HISTORY_PAGE_SIZE;
                                        },
                                        {i18n.t("common.more")}
                                    }
                                }
                            }
                        },
                    }
                }
            }

            // 提现记录
            div { class: "card",
                div { class: "card-header",
                    h3 { class: "card-title", {i18n.t("node_earnings.withdrawals_title")} }
                }
                div { class: "card-body",
                    match withdrawals_resource().as_ref().map(|r| r.as_ref()) {
                        None => rsx! {
                            div { class: "text-secondary", {i18n.t("table.loading")} }
                        },
                        Some(Err(e)) => rsx! {
                            Alert { variant: AlertVariant::Error, "{e}" }
                        },
                        Some(Ok(records)) => rsx! {
                            Table {
                                empty: records.is_empty(),
                                empty_text: i18n.t("node_earnings.no_withdrawals"),
                                col_count: 6,
                                thead {
                                    tr {
                                        TableHead { {i18n.t("node_earnings.col_time")} }
                                        TableHead { {i18n.t("node_earnings.col_amount")} }
                                        TableHead { {i18n.t("node_earnings.col_method")} }
                                        TableHead { {i18n.t("node_earnings.col_status")} }
                                        TableHead { {i18n.t("node_earnings.col_remark")} }
                                        TableHead { "ID" }
                                    }
                                }
                                tbody {
                                    for w in records.iter() {
                                        tr {
                                            td { {format_time(&w.created_at)} }
                                            td { {format_precise_cny_str(&w.total_amount)} }
                                            td {
                                                match w.withdrawal_type.as_str() {
                                                    "alipay" => i18n.t("node_earnings.method_alipay"),
                                                    "balance" => i18n.t("node_earnings.method_balance"),
                                                    _ => w.withdrawal_type.as_str(),
                                                }
                                            }
                                            td {
                                                WithdrawalStatusBadge { status: w.status.clone() }
                                            }
                                            td { class: "text-secondary",
                                                if let Some(ref remark) = w.admin_remark {
                                                    "{remark}"
                                                } else {
                                                    "—"
                                                }
                                            }
                                            td { class: "text-muted", title: "{w.id}", {short_id(&w.id)} }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
            }

            // 提现弹窗
            if show_modal {
                WithdrawModal {
                    withdraw_modal,
                    withdraw_method,
                    alipay_account,
                    real_name,
                    withdraw_loading,
                    withdraw_error,
                }
            }
        }
    }
}

/// 汇总卡片
#[component]
fn EarningsCard(label: String, value: String, meta: String, variant: &'static str) -> Element {
    rsx! {
        div { class: "earnings-card earnings-card-{variant}",
            p { class: "earnings-card-label", "{label}" }
            p { class: "earnings-card-value", "{value}" }
            p { class: "earnings-card-meta", "{meta}" }
        }
    }
}

/// 提现状态 Badge
#[component]
fn WithdrawalStatusBadge(status: String) -> Element {
    let s = match status.as_str() {
        "pending" => WithdrawalStatus::Pending,
        "approved" => WithdrawalStatus::Approved,
        "completed" => WithdrawalStatus::Completed,
        "rejected" => WithdrawalStatus::Rejected,
        _ => WithdrawalStatus::Pending,
    };
    rsx! {
        Badge { variant: s.variant(), "{s.label()}" }
    }
}

/// 提现弹窗
///
/// 提交逻辑直接内联在弹窗的提交按钮中，避免复杂闭包无法转换为 EventHandler<()> 的问题。
#[component]
fn WithdrawModal(
    mut withdraw_modal: Signal<WithdrawModalState>,
    mut withdraw_method: Signal<WithdrawMethod>,
    mut alipay_account: Signal<String>,
    mut real_name: Signal<String>,
    withdraw_loading: Signal<bool>,
    withdraw_error: Signal<Option<String>>,
) -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let ui_store = use_context::<UiStore>();

    rsx! {
        div {
            class: "modal-overlay",
            onclick: move |_| {
                if !withdraw_loading() {
                    withdraw_modal.set(WithdrawModalState::Closed);
                }
            },
            div {
                class: "modal",
                role: "dialog",
                aria_modal: "true",
                aria_label: i18n.t("node_earnings.withdraw_title"),
                // 阻止冒泡
                onclick: move |evt| {
                    evt.stop_propagation();
                },
                div { class: "modal-header",
                    h3 { class: "modal-title", {i18n.t("node_earnings.withdraw_title")} }
                    button {
                        class: "modal-close",
                        r#type: "button",
                        aria_label: i18n.t("common.close"),
                        disabled: withdraw_loading(),
                        onclick: move |_| withdraw_modal.set(WithdrawModalState::Closed),
                        "✕"
                    }
                }
                div { class: "modal-body",
                    if let Some(ref err) = withdraw_error() {
                        Alert { variant: AlertVariant::Error, "{err}" }
                    }

                    // 选择提现方式
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("node_earnings.withdraw_method")} }
                        div { class: "withdraw-method-grid",
                            for method in [WithdrawMethod::Balance, WithdrawMethod::Alipay] {
                                {
                                    let m = method.clone();
                                    let is_active = *withdraw_method.read() == method;
                                    rsx! {
                                        button {
                                            class: if is_active { "withdraw-method-card active" } else { "withdraw-method-card" },
                                            r#type: "button",
                                            onclick: move |_| withdraw_method.set(m.clone()),
                                            strong { "{m.label()}" }
                                            p { class: "text-secondary",
                                                if m == WithdrawMethod::Balance {
                                                    {i18n.t("node_earnings.method_balance_desc")}
                                                } else {
                                                    {i18n.t("node_earnings.method_alipay_desc")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // 支付宝方式需填写账号和姓名
                    if matches!(*withdraw_method.read(), WithdrawMethod::Alipay) {
                        div { class: "form-group",
                            label { class: "form-label", {i18n.t("node_earnings.alipay_account")} }
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: i18n.t("node_earnings.alipay_placeholder"),
                                value: "{alipay_account}",
                                oninput: move |e| alipay_account.set(e.value()),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", {i18n.t("node_earnings.real_name")} }
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: i18n.t("node_earnings.real_name_placeholder"),
                                value: "{real_name}",
                                oninput: move |e| real_name.set(e.value()),
                            }
                        }
                    }

                    p { class: "text-secondary withdraw-hint",
                        {i18n.t("node_earnings.withdraw_hint")}
                    }
                }
                div { class: "modal-footer",
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Medium,
                        disabled: withdraw_loading(),
                        onclick: move |_| withdraw_modal.set(WithdrawModalState::Closed),
                        {i18n.t("form.cancel")}
                    }
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Medium,
                        disabled: withdraw_loading(),
                        onclick: move |_| {
                            let needs_alipay = withdraw_method() == WithdrawMethod::Alipay;
                            if needs_alipay
                                && (alipay_account().trim().is_empty() || real_name().trim().is_empty())
                            {
                                withdraw_error.set(Some(i18n.t("node_earnings.fill_alipay").to_string()));
                                return;
                            }
                            withdraw_loading.set(true);
                            withdraw_error.set(None);

                            let auth = auth_store.clone();
                            let mut ui = ui_store.clone();
                            let i18n_clone = i18n.clone();
                            let method_val = withdraw_method().value().to_string();
                            let alipay_opt = if needs_alipay {
                                Some(alipay_account().to_string())
                            } else {
                                None
                            };
                            let name_opt = if needs_alipay { Some(real_name().to_string()) } else { None };
                            let mut wm = withdraw_modal.clone();
                            let mut aa = alipay_account.clone();
                            let mut rn = real_name.clone();
                            let mut wl = withdraw_loading.clone();
                            spawn(async move {
                                let token = auth.token().unwrap_or_default();
                                let alipay_str = alipay_opt.as_deref();
                                let name_str = name_opt.as_deref();
                                match node_tips_service::create_withdrawal(
                                        &token,
                                        &method_val,
                                        alipay_str,
                                        name_str,
                                    )
                                    .await
                                {
                                    Ok(_) => {
                                        wl.set(false);
                                        wm.set(WithdrawModalState::Closed);
                                        aa.set(String::new());
                                        rn.set(String::new());
                                        let msg = if needs_alipay {
                                            i18n_clone.t("node_earnings.withdraw_alipay_success")
                                        } else {
                                            i18n_clone.t("node_earnings.withdraw_balance_success")
                                        };
                                        ui.show_success(msg);
                                    }
                                    Err(e) => {
                                        wl.set(false);
                                        withdraw_error
                                            .set(
                                                Some(
                                                    format!(
                                                        "{}: {}",
                                                        i18n_clone.t("node_earnings.withdraw_failed"),
                                                        e,
                                                    ),
                                                ),
                                            );
                                    }
                                }
                            });
                        },
                        if withdraw_loading() {
                            span { class: "spinner spinner-sm" }
                            " {i18n.t(\"form.submit\")}"
                        } else {
                            {i18n.t("form.submit")}
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::visible_range;

    #[test]
    fn visible_range_first_page_shows_page_size_items() {
        assert_eq!(visible_range(0, 20, 50), (1, 20));
    }

    #[test]
    fn visible_range_last_page_truncates_to_total() {
        assert_eq!(visible_range(40, 20, 50), (41, 50));
        assert_eq!(visible_range(45, 20, 50), (46, 50));
    }

    #[test]
    fn visible_range_exact_multiples() {
        assert_eq!(visible_range(0, 20, 20), (1, 20));
        assert_eq!(visible_range(20, 20, 40), (21, 40));
    }

    #[test]
    fn visible_range_empty_total_returns_zero() {
        assert_eq!(visible_range(0, 20, 0), (0, 0));
        assert_eq!(visible_range(40, 20, 0), (0, 0));
    }
}
