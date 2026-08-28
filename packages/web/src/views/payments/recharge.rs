use dioxus::prelude::*;
use gloo_timers::future::sleep;
use std::time::Duration;

use chrono::{DateTime, Utc};
use client_api::api::payment::{CreatePaymentOrderRequest, PaymentMethodsResponse};
use rust_decimal::Decimal;
use ui::PageHeader;

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;
use crate::services::api_client::with_auto_refresh;
use crate::services::payment_service;
use crate::stores::auth_store::AuthStore;
use crate::stores::ui_store::UiStore;

/// 支付方式枚举
#[derive(Clone, PartialEq)]
enum PayMethod {
    Alipay,
    WechatPay,
}

impl PayMethod {
    fn code(&self) -> &'static str {
        match self {
            PayMethod::Alipay => "alipay",
            PayMethod::WechatPay => "wechatpay",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "alipay" => Some(Self::Alipay),
            "wechatpay" => Some(Self::WechatPay),
            _ => None,
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            PayMethod::Alipay => "💳",
            PayMethod::WechatPay => "📱",
        }
    }
}

/// 订单状态
#[derive(Clone, PartialEq)]
enum OrderState {
    /// 尚未创建订单
    Idle,
    /// 创建成功，等待支付（含 pay_url）
    Pending {
        order_id: String,
        out_trade_no: String,
        payment_method: String,
        pay_url: Option<String>,
        payment_type: String,
        qr_code: Option<String>,
        qr_code_image_url: Option<String>,
        expired_at: String,
    },
    /// 支付完成
    Paid { out_trade_no: String },
    /// 支付失败
    Failed { reason: String },
}

#[component]
pub fn Recharge() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let mut ui_store = use_context::<UiStore>();
    let nav = use_navigator();

    let mut amount = use_signal(String::new);
    let mut pay_method = use_signal(|| PayMethod::Alipay);
    let mut loading = use_signal(|| false);
    let mut order_state = use_signal(|| OrderState::Idle);
    // 订单手动轮询计数器，变化时触发 use_resource 重执行
    let mut poll_tick = use_signal(|| 0u32);
    // 轮询世代计数器：每次启动新轮询循环时递增，实现防竞态
    // 当 loop 中读到的 gen 与当前不一致时，说明旧 loop 应退出
    let mut poll_gen = use_signal(|| 0u32);
    // 自动轮询是否激活（进入 Pending 后开始，离开后停止）
    let mut auto_poll_active = use_signal(|| false);
    let methods = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            payment_service::get_methods(&token).await
        })
        .await
    });

    use_effect(move || {
        if let Some(Ok(response)) = methods() {
            let preferred = response
                .methods
                .iter()
                .find(|method| method.is_default)
                .or_else(|| response.methods.first());
            if let Some(method) = preferred
                && let Some(value) = PayMethod::from_code(&method.code)
            {
                let current_is_valid = response
                    .methods
                    .iter()
                    .any(|item| item.code == pay_method().code());
                if !current_is_valid {
                    pay_method.set(value);
                }
            }
        }
    });

    // 手动触发的订单状态查询
    let _poll = use_resource(move || async move {
        let tick = poll_tick();
        if tick == 0 {
            return;
        }
        let (order_id, no) = match order_state() {
            OrderState::Pending {
                ref order_id,
                ref out_trade_no,
                ..
            } => (order_id.clone(), out_trade_no.clone()),
            _ => return,
        };
        let result = with_auto_refresh(auth_store, |token| {
            let order_id = order_id.clone();
            async move { payment_service::sync_order(&order_id, &token).await }
        })
        .await;
        if let Ok(order) = result {
            match order.status.as_str() {
                "paid" | "success" => {
                    auto_poll_active.set(false);
                    order_state.set(OrderState::Paid {
                        out_trade_no: no.clone(),
                    });
                    ui_store.show_success(i18n.t("recharge.pay_success"));
                }
                status if is_terminal_payment_failure(status) => {
                    auto_poll_active.set(false);
                    order_state.set(OrderState::Failed {
                        reason: i18n
                            .t_with_args("recharge.order_status", &[("status", &order.status)]),
                    });
                }
                _ => {} // 仍在处理中
            }
        }
    });

    // 自动轮询：进入 Pending 状态后每 5 秒自动检查一次。网络连续失败时指数退避，
    // 达到重试上限或订单过期后停止，避免后台任务永久存活。
    // 防竞态：每次开启时捕证当前 gen，循环中检测 gen 变化即退出旧 loop
    use_effect(move || {
        if auto_poll_active() {
            // 单调递增，捕证本次开启对应的 generation
            let my_gen = poll_gen();
            spawn(async move {
                let mut delay_secs = 5;
                let mut consecutive_failures = 0;
                loop {
                    sleep(Duration::from_secs(delay_secs)).await;
                    // gen 发生变化（新轮询已开启），旧 loop 直接退出
                    if poll_gen() != my_gen {
                        break;
                    }
                    // 若状态已不是 Pending，停止轮询
                    match order_state() {
                        OrderState::Pending {
                            ref order_id,
                            ref out_trade_no,
                            ref expired_at,
                            ..
                        } => {
                            if payment_polling_expired(expired_at, Utc::now()) {
                                auto_poll_active.set(false);
                                break;
                            }
                            let order_id = order_id.clone();
                            let no = out_trade_no.clone();
                            let result = with_auto_refresh(auth_store, |token| {
                                let order_id = order_id.clone();
                                async move { payment_service::sync_order(&order_id, &token).await }
                            })
                            .await;
                            match result {
                                Ok(order) => {
                                    consecutive_failures = 0;
                                    delay_secs = 5;
                                    match order.status.as_str() {
                                        "paid" | "success" => {
                                            order_state.set(OrderState::Paid { out_trade_no: no });
                                            ui_store.show_success(
                                                i18n.t("recharge.pay_success_credited"),
                                            );
                                            auto_poll_active.set(false);
                                            break;
                                        }
                                        status if is_terminal_payment_failure(status) => {
                                            order_state.set(OrderState::Failed {
                                                reason: i18n.t_with_args(
                                                    "recharge.order_expired",
                                                    &[("status", &order.status)],
                                                ),
                                            });
                                            auto_poll_active.set(false);
                                            break;
                                        }
                                        _ => {} // 继续轮询
                                    }
                                }
                                Err(_) => {
                                    consecutive_failures += 1;
                                    if consecutive_failures >= 5 {
                                        ui_store.show_error(i18n.t("common.load_failed"));
                                        auto_poll_active.set(false);
                                        break;
                                    }
                                    delay_secs = (delay_secs * 2).min(60);
                                }
                            }
                        }
                        _ => break, // 状态已变更，停止
                    }
                }
            });
        }
    });

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        let amount_str = amount();
        if amount_str.is_empty() {
            ui_store.show_error(i18n.t("recharge.enter_amount"));
            return;
        }
        let methods_response = methods().and_then(Result::ok);
        let Some(methods_response) = methods_response else {
            ui_store.show_error(i18n.t("recharge.no_payment_methods"));
            return;
        };
        let Some(limits) = payment_limits(&methods_response) else {
            ui_store.show_error(i18n.t("recharge.invalid_amount"));
            return;
        };
        if !amount_within_limits(&amount_str, limits) {
            ui_store.show_error(i18n.t("recharge.invalid_amount"));
            return;
        }
        let selected = pay_method();
        let method_info = methods_response
            .methods
            .into_iter()
            .find(|method| method.code == selected.code());
        let Some(method_info) = method_info else {
            ui_store.show_error(i18n.t("recharge.no_payment_methods"));
            return;
        };
        let payment_type = method_info.recommended_scene;
        let payment_method = method_info.code;
        loading.set(true);
        order_state.set(OrderState::Idle);
        spawn(async move {
            let req =
                CreatePaymentOrderRequest::new_for_method(amount_str, payment_method, payment_type);
            let result = with_auto_refresh(auth_store, |token| {
                let req = req.clone();
                async move { payment_service::create_order(req, &token).await }
            })
            .await;
            match result {
                Ok(order) => {
                    loading.set(false);
                    order_state.set(OrderState::Pending {
                        order_id: order.order_id.clone(),
                        out_trade_no: order.out_trade_no.clone(),
                        payment_method: order.payment_method.clone(),
                        pay_url: order.pay_url.clone(),
                        payment_type: order.payment_type.clone(),
                        qr_code: order.qr_code.clone(),
                        qr_code_image_url: order.qr_code_image_url.clone(),
                        expired_at: order.expired_at.clone(),
                    });
                    amount.set(String::new());
                    // 递增 gen，使旧轮询 loop 自动退出，再将 active 设为 true 开启新轮询
                    *poll_gen.write() += 1;
                    auto_poll_active.set(true);
                }
                Err(e) => {
                    loading.set(false);
                    order_state.set(OrderState::Failed {
                        reason: i18n
                            .t_with_args("recharge.create_failed", &[("error", &e.to_string())]),
                    });
                }
            }
        });
    };

    rsx! {
        div { class: "page-container recharge-page",
            PageHeader {
                title: i18n.t("recharge.title").to_string(),
                leading: rsx! {
                    button {
                        class: "btn btn-ghost btn-sm",
                        r#type: "button",
                        aria_label: i18n.t("common.back"),
                        onclick: move |_| {
                            nav.push(Route::PaymentsOverview {});
                        },
                        "←"
                    }
                },
            }

            // 充値表单区
            match order_state() {
                OrderState::Idle | OrderState::Failed { .. } => rsx! {
                    div { class: "card",
                        div { class: "card-header",
                            h3 { class: "card-title", {i18n.t("recharge.select_method")} }
                        }
                        div { class: "card-body",
                            // 失败提示
                            if let OrderState::Failed { ref reason } = order_state() {
                                div { class: "alert alert-error",
                                    span { class: "alert-icon", "✕" }
                                    div { class: "alert-content",
                                        p { class: "alert-body", "{reason}" }
                                    }
                                }
                            }

                            // 支付方式选择：服务端只返回真正可接受新订单的渠道。
                            match methods() {
                                None => rsx! { div { class: "loading-state", {i18n.t("table.loading")} } },
                                Some(Err(error)) => rsx! {
                                    div { class: "alert alert-error", "{i18n.t(\"common.load_failed\")}：{error}" }
                                },
                                Some(Ok(response)) if response.methods.is_empty() => rsx! {
                                    div { class: "empty-state",
                                        p { {i18n.t("recharge.no_payment_methods")} }
                                    }
                                },
                                Some(Ok(response)) => rsx! {
                                    div { class: "form-group",
                                        label { class: "form-label", {i18n.t("recharge.payment_method")} }
                                        div { class: "pay-method-grid",
                                            for info in response.methods {
                                                if let Some(method) = PayMethod::from_code(&info.code) {
                                                    {
                                                        let is_active = pay_method() == method;
                                                        let selected_method = method.clone();
                                                        rsx! {
                                                            button {
                                                                key: "{info.code}",
                                                                class: if is_active { "pay-method-card active" } else { "pay-method-card" },
                                                                r#type: "button",
                                                                onclick: move |_| pay_method.set(selected_method.clone()),
                                                                span { class: "pay-method-icon", "{method.icon()}" }
                                                                span { class: "pay-method-label",
                                                                    if info.code == "alipay" {
                                                                        {i18n.t("recharge.alipay")}
                                                                    } else if info.code == "wechatpay" {
                                                                        {i18n.t("recharge.wechat_pay")}
                                                                    } else {
                                                                        "{info.display_name}"
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                },
                            }

                            form { onsubmit: on_submit,
                                // 常用金额选择
                                div { class: "form-group",
                                    label { class: "form-label", {i18n.t("recharge.amount_label")} }
                                    div { class: "amount-presets",
                                        for preset in ["10", "30", "50", "100", "200", "500"] {
                                            if methods()
                                                .and_then(Result::ok)
                                                .and_then(|response| payment_limits(&response))
                                                .is_some_and(|limits| amount_within_limits(preset, limits))
                                            {
                                                button {
                                                    class: if amount() == preset { "btn btn-primary btn-sm" } else { "btn btn-secondary btn-sm" },
                                                    r#type: "button",
                                                    onclick: move |_| amount.set(preset.to_string()),
                                                    "¥{preset}"
                                                }
                                            }
                                        }
                                    }
                                    input {
                                        class: "form-input",
                                        style: "margin-top: 8px",
                                        r#type: "number",
                                        min: methods().and_then(Result::ok).map(|response| response.min_amount).unwrap_or_else(|| "0.01".to_string()),
                                        max: methods().and_then(Result::ok).map(|response| response.max_amount).unwrap_or_default(),
                                        step: "0.01",
                                        placeholder: i18n.t("recharge.custom_amount"),
                                        value: "{amount}",
                                        oninput: move |e| amount.set(e.value()),
                                    }
                                }

                                // 提交按钮
                                button {
                                    class: "btn btn-primary btn-full",
                                    r#type: "submit",
                                    disabled: loading() || methods().map(|result| result.map(|value| value.methods.is_empty()).unwrap_or(true)).unwrap_or(true),
                                    if loading() {
                                        {i18n.t("recharge.creating_order")}
                                    } else {
                                        {
                                            let amt_label = if amount().is_empty() {
                                                String::new()
                                            } else {
                                                format!(" CNY {}", amount())
                                            };
                                            format!(
                                                "{} {}{}",
                                                pay_method().icon(),
                                                i18n.t("recharge.confirm_recharge"),
                                                amt_label,
                                            )
                                        }
                                    }
                                }
                            }

                            // 说明
                            div { class: "alert alert-info", style: "margin-top: 16px",
                                span { class: "alert-icon", "ℹ" }
                                div { class: "alert-content",
                                    p { class: "alert-body", {i18n.t("recharge.hint")} }
                                }
                            }
                        }
                    }
                },
                OrderState::Pending {
                    ref out_trade_no,
                    ref payment_method,
                    ref pay_url,
                    ref payment_type,
                    ref qr_code,
                    ref qr_code_image_url,
                    ..
                } => rsx! {
                    div { class: "card",
                        div { class: "card-header",
                            h3 { class: "card-title", {i18n.t("recharge.pay_title")} }
                        }
                        div { class: "card-body",
                            div { class: "alert alert-warning",
                                span { class: "alert-icon", "⏳" }
                                div { class: "alert-content",
                                    p { class: "alert-title", {i18n.t("recharge.order_created")} }
                                    p { class: "alert-body",
                                        {i18n.t("recharge.order_no_label")}
                                        code { "{out_trade_no}" }
                                    }
                                }
                            }

                            // 如果有支付跳转链接
                            if let Some(url) = pay_url {
                                div { class: "pay-qr-area",
                                    p { class: "pay-qr-tip",
                                        if payment_type == "page" {
                                            {i18n.t("recharge.pay_alipay_page")}
                                        } else if payment_type == "wap" {
                                            {i18n.t("recharge.pay_wap")}
                                        } else {
                                            {i18n.t("recharge.pay_other")}
                                        }
                                    }
                                    a {
                                        href: "{url}",
                                        target: "_blank",
                                        rel: "noopener noreferrer",
                                        class: "btn btn-primary btn-full",
                                        style: "text-decoration:none;display:block;text-align:center",
                                        {i18n.t("recharge.open_payment")}
                                    }
                                    p { style: "font-size:12px;color:var(--text-secondary);margin-top:8px;text-align:center",
                                        {i18n.t("recharge.refresh_hint")}
                                    }
                                }
                            }

                            if let Some(image_url) = qr_code_image_url {
                                div { class: "pay-qr-area",
                                    p { class: "pay-qr-tip",
                                        if payment_method == "wechatpay" {
                                            {i18n.t("recharge.scan_wechat")}
                                        } else {
                                            {i18n.t("recharge.scan_alipay")}
                                        }
                                    }
                                    img {
                                        src: "{image_url}",
                                        alt: i18n.t("recharge.qr_code_alt"),
                                        style: "width:220px;height:220px;display:block;margin:0 auto;border-radius:16px;border:1px solid var(--border-color);background:white;padding:12px",
                                    }
                                    if let Some(code) = qr_code {
                                        p { style: "font-size:12px;color:var(--text-secondary);margin-top:8px;text-align:center;word-break:break-all",
                                            {i18n.t("recharge.qr_code_content")}
                                            "{code}"
                                        }
                                    }
                                }
                            }

                            // 轮询按钮
                            div { class: "pay-actions",
                                button {
                                    class: "btn btn-primary",
                                    r#type: "button",
                                    onclick: move |_| *poll_tick.write() += 1,
                                    {i18n.t("recharge.confirm_paid")}
                                }
                                button {
                                    class: "btn btn-ghost",
                                    r#type: "button",
                                    onclick: move |_| {
                                        auto_poll_active.set(false);
                                        order_state.set(OrderState::Idle);
                                    },
                                    {i18n.t("recharge.pay_later")}
                                }
                            }
                        }
                    }
                },
                OrderState::Paid { ref out_trade_no } => rsx! {
                    div { class: "card",
                        div { class: "card-body",
                            div { class: "pay-success",
                                div { class: "pay-success-icon", "✅" }
                                h3 { class: "pay-success-title", {i18n.t("recharge.success_title")} }
                                p { class: "pay-success-no",
                                    {i18n.t("recharge.order_no_label")}
                                    code { "{out_trade_no}" }
                                }
                                p { style: "color:var(--text-secondary);margin-bottom:24px",
                                    {i18n.t("recharge.success_desc")}
                                }
                                div { class: "pay-success-actions",
                                    button {
                                        class: "btn btn-primary",
                                        r#type: "button",
                                        onclick: move |_| {
                                            nav.push(Route::PaymentsOverview {});
                                        },
                                        {i18n.t("recharge.view_balance")}
                                    }
                                    button {
                                        class: "btn btn-ghost",
                                        r#type: "button",
                                        onclick: move |_| {
                                            order_state.set(OrderState::Idle);
                                            amount.set(String::new());
                                        },
                                        {i18n.t("recharge.continue_recharge")}
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

fn is_terminal_payment_failure(status: &str) -> bool {
    matches!(status, "failed" | "cancelled" | "closed")
}

fn payment_polling_expired(expired_at: &str, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(expired_at)
        .map(|expiry| expiry <= now)
        // 服务端契约要求 RFC 3339；无效值应 fail closed，不能产生永久轮询。
        .unwrap_or(true)
}

fn payment_limits(response: &PaymentMethodsResponse) -> Option<(Decimal, Decimal)> {
    let min = response.min_amount.parse::<Decimal>().ok()?;
    let max = response.max_amount.parse::<Decimal>().ok()?;
    (min > Decimal::ZERO && min <= max).then_some((min, max))
}

fn amount_within_limits(amount: &str, (min, max): (Decimal, Decimal)) -> bool {
    amount
        .parse::<Decimal>()
        .is_ok_and(|value| value.normalize().scale() <= 2 && value >= min && value <= max)
}

#[cfg(test)]
mod tests {
    use super::{
        amount_within_limits, is_terminal_payment_failure, payment_limits, payment_polling_expired,
    };
    use chrono::{TimeZone, Utc};
    use client_api::api::payment::PaymentMethodsResponse;

    #[test]
    fn closed_orders_stop_payment_polling() {
        assert!(is_terminal_payment_failure("closed"));
        assert!(is_terminal_payment_failure("failed"));
        assert!(!is_terminal_payment_failure("pending"));
        assert!(!is_terminal_payment_failure("paid"));
    }

    #[test]
    fn payment_polling_stops_at_expiry_or_on_invalid_deadline() {
        let now = Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap();
        assert!(payment_polling_expired("2026-07-26T12:00:00Z", now));
        assert!(payment_polling_expired("2026-07-26T11:59:59Z", now));
        assert!(!payment_polling_expired("2026-07-26T12:00:01Z", now));
        assert!(payment_polling_expired("not-a-deadline", now));
    }

    #[test]
    fn configured_recharge_limits_drive_client_validation() {
        let response = PaymentMethodsResponse {
            methods: vec![],
            min_amount: "30.50".to_string(),
            max_amount: "100.25".to_string(),
            currency: "CNY".to_string(),
        };
        let limits = payment_limits(&response).unwrap();
        assert!(amount_within_limits("30.50", limits));
        assert!(amount_within_limits("100.25", limits));
        assert!(!amount_within_limits("30.49", limits));
        assert!(!amount_within_limits("100.26", limits));
        assert!(!amount_within_limits("30.501", limits));
        assert!(amount_within_limits("30.500", limits));
    }
}
