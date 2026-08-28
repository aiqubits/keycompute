use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use ui::{
    Alert, AlertVariant, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, ConfirmModal,
    PageHeader, icons::IconCopy,
};

use crate::hooks::use_i18n::use_i18n;
use crate::i18n::I18n;
use crate::services::node_gateway_token_service;
use crate::stores::{auth_store::AuthStore, ui_store::UiStore};
use crate::utils::on_copy;
use crate::utils::time::format_time;

type TokenDetail = client_api::api::node_gateway_token::NodeGatewayTokenDetail;

// ── 状态指示灯颜色映射 ──
fn status_dot_color(status: &str) -> &'static str {
    match status {
        "approved" => "#00c853",
        "pending" => "#ff9800",
        "consumed" => "#2196f3",
        "rejected" => "#f44336",
        _ => "#9e9e9e",
    }
}

fn registered_node_status_label(status: &str, i18n: &I18n) -> String {
    match status {
        "online" => i18n.t("node_gateway.status_online").to_string(),
        "offline" => i18n.t("node_gateway.status_offline").to_string(),
        "excluded" => i18n.t("node_gateway.status_excluded").to_string(),
        _ => status.to_string(),
    }
}

/// 状态指示灯纯 SVG 组件
#[component]
fn StatusDot(color: String, #[props(default = 10u32)] size: u32) -> Element {
    rsx! {
        svg {
            class: "node-token-status-dot",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 10 10",
            fill: "none",
            circle {
                cx: "5",
                cy: "5",
                r: "4",
                fill: "{color}",
            }
            circle {
                cx: "5",
                cy: "5",
                r: "3",
                fill: "{color}",
                opacity: "0.35",
            }
        }
    }
}

#[component]
pub fn NodeToken() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let ui_store = use_context::<UiStore>();

    let list_key = use_signal(|| 0u64);
    // 提供给子组件用于刷新列表
    use_context_provider(|| list_key);
    let mut applying = use_signal(|| false);

    // 定时刷新信号：每 30 秒自动拉取最新令牌列表，确保节点状态与 admin 端一致
    let mut refresh_tick = use_signal(|| 0u64);
    let _timer = use_resource(move || async move {
        loop {
            TimeoutFuture::new(30_000).await;
            refresh_tick.with_mut(|v| *v += 1);
        }
    });

    let tokens_resource = use_resource(move || {
        let _key = list_key();
        let _tick = refresh_tick();
        let auth = auth_store.clone();
        async move {
            let token = auth.token().unwrap_or_default();
            node_gateway_token_service::list_my_tokens(&token).await
        }
    });

    // 检查是否已有阻止新申请的令牌
    // 阻止状态: pending(待审批) / approved(已通过) / consumed(已使用) / rejected+revoke_reason(已吊销)
    let can_apply = use_memo(move || {
        match tokens_resource().as_ref().map(|r| r.as_ref()) {
            Some(Ok(tokens)) => !tokens.iter().any(|t| {
                let s = t.token.status.as_str();
                match s {
                    "pending" | "approved" | "consumed" => true,
                    "rejected" => t.token.revoke_reason.is_some(),
                    _ => false,
                }
            }),
            _ => false, // 加载中或出错时不允许申请（安全优先）
        }
    });

    let on_apply = move |_: Event<MouseData>| {
        applying.set(true);
        spawn({
            let auth = auth_store.clone();
            let mut ui = ui_store.clone();
            let i18n = i18n.clone();
            let mut list_key = list_key;
            async move {
                let token = match auth.token() {
                    Some(t) => t,
                    None => {
                        ui.show_error(i18n.t("common.error"));
                        applying.set(false);
                        return;
                    }
                };
                match node_gateway_token_service::create_my_token(&token).await {
                    Ok(_) => {
                        ui.show_success(i18n.t("node_token.apply_success"));
                        list_key.with_mut(|v| *v += 1);
                    }
                    Err(e) => {
                        ui.show_error(format!("{}: {}", i18n.t("node_token.apply_failed"), e))
                    }
                }
                applying.set(false);
            }
        });
    };

    rsx! {
        div { class: "page-container node-token-page",
            PageHeader {
                title: i18n.t("page.node_token").to_string(),
                description: i18n.t("node_token.subtitle").to_string(),
            }

            if let Some(Err(ref e)) = tokens_resource().as_ref().map(|r| r.as_ref()) {
                Alert { variant: AlertVariant::Error, "{i18n.t(\"common.load_failed\")}: {e}" }
            }

            // ── 主操作卡片 ──
            div { class: "card node-token-main-card",
                match tokens_resource().as_ref().map(|r| r.as_ref()) {
                    None => rsx! {
                        div { class: "card-body",
                            div { class: "node-token-loading",
                                p { class: "text-secondary", {i18n.t("table.loading")} }
                            }
                        }
                    },
                    Some(Ok(tokens)) => {
                        if tokens.is_empty() {
                            // ── 空状态：居中 CTA ──
                            rsx! {
                                div { class: "card-body node-token-empty-body",
                                    div { class: "node-token-empty",
                                        div { class: "node-token-empty-icon",
                                            svg {
                                                width: "64",
                                                height: "64",
                                                view_box: "0 0 24 24",
                                                fill: "none",
                                                stroke: "currentColor",
                                                stroke_width: "1.5",
                                                stroke_linecap: "round",
                                                stroke_linejoin: "round",
                                                rect {
                                                    x: "3",
                                                    y: "11",
                                                    width: "18",
                                                    height: "11",
                                                    rx: "2",
                                                    ry: "2",
                                                }
                                                path { d: "M7 11V7a5 5 0 0 1 10 0v4" }
                                                circle { cx: "12", cy: "16", r: "1" }
                                            }
                                        }
                                        h3 { class: "node-token-empty-title", {i18n.t("node_token.empty_title")} }
                                        p { class: "node-token-empty-desc", {i18n.t("node_token.empty_desc")} }
                                        Button {
                                            variant: ButtonVariant::Primary,
                                            size: ButtonSize::Large,
                                            disabled: applying(),
                                            onclick: on_apply,
                                            if applying() {
                                                span { class: "spinner spinner-sm" }
                                                " {i18n.t(\"node_token.applying\")}"
                                            } else {
                                                {i18n.t("node_token.apply")}
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // ── 有令牌：header + 列表 ──
                            rsx! {
                                div { class: "card-header node-token-card-header",
                                    div { class: "node-gateway-status-row",
                                        h3 { class: "card-title", {i18n.t("node_token.title")} }
                                        if can_apply() {
                                            Button {
                                                variant: ButtonVariant::Primary,
                                                size: ButtonSize::Small,
                                                disabled: applying(),
                                                onclick: on_apply,
                                                if applying() {
                                                    span { class: "spinner spinner-sm" }
                                                    " {i18n.t(\"node_token.applying\")}"
                                                } else {
                                                    {i18n.t("node_token.apply")}
                                                }
                                            }
                                        }
                                    }
                                    if !can_apply() {
                                        span { class: "node-token-approved-hint", {i18n.t("node_token.cannot_apply")} }
                                    }
                                }
                                div { class: "card-body node-token-list-body",
                                    for detail in tokens.iter() {
                                        TokenListItem { detail: detail.clone() }
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(_)) => rsx! {},
                }
            }

            NodeTokenHelp {}
        }
    }
}

#[component]
fn TokenListItem(detail: TokenDetail) -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let ui_store = use_context::<UiStore>();
    let mut expanded = use_signal(|| false);
    let mut delete_modal_open = use_signal(|| false);

    // 是否可删除：rejected 且无 revoke_reason（管理员拒绝申请，非吊销）
    let can_delete = detail.token.status == "rejected"
        && detail.token.revoke_reason.is_none()
        && detail.registered_node.is_none();

    // 获取父组件提供的列表刷新信号
    let list_key: Signal<u64> = use_context();

    // 若 token 状态为 approved 但已有注册节点（consumed_node_id 非空），
    // 则按 consumed 展示（蓝色"已使用"），不区分节点在线/离线。
    // 仅首次批准且从未注册节点时才显示绿色"已通过"。
    let effective_status: &str =
        if detail.token.status == "approved" && detail.registered_node.is_some() {
            "consumed"
        } else {
            &detail.token.status
        };

    let (status_badge_variant, status_label) = match effective_status {
        "pending" => (
            BadgeVariant::Warning,
            i18n.t("node_token.status_pending").to_string(),
        ),
        "approved" => (
            BadgeVariant::Success,
            i18n.t("node_token.status_approved").to_string(),
        ),
        "consumed" => (
            BadgeVariant::Info,
            i18n.t("node_token.status_consumed").to_string(),
        ),
        "rejected" => {
            if detail.registered_node.is_some() {
                (
                    BadgeVariant::Error,
                    i18n.t("node_token.status_revoked").to_string(),
                )
            } else {
                (
                    BadgeVariant::Error,
                    i18n.t("node_token.status_rejected").to_string(),
                )
            }
        }
        _ => (BadgeVariant::Neutral, detail.token.status.clone()),
    };

    let dot_color = status_dot_color(effective_status);
    let expand_label = if expanded() {
        i18n.t("common.collapse")
    } else {
        i18n.t("node_token.expand")
    };

    rsx! {
        div { class: "node-token-card",
            // ── 摘要行 ──
            div {
                class: "node-token-card-summary",
                onclick: move |_| expanded.toggle(),
                div { class: "node-token-card-summary-main",
                    StatusDot { color: dot_color.to_string() }
                    span { class: "node-token-card-status-badge",
                        Badge { variant: status_badge_variant, "{status_label}" }
                    }
                    code { class: "node-token-card-preview", "{detail.token.token_preview}" }
                }
                div { class: "node-token-card-summary-meta",
                    span { class: "node-token-card-time", "{detail.token.issued_at}" }
                    span { class: "node-token-card-expand", "{expand_label}" }
                }
            }

            // ── 展开详情区 ──
            if expanded() {
                div { class: "node-token-card-detail",
                    match effective_status {
                        "approved" => rsx! {
                            TokenApprovedDetail {
                                plaintext: detail.registration_token.clone().unwrap_or_default(),
                                is_revealed: detail.token.is_revealed,
                            }
                        },
                        "consumed" => rsx! {
                            TokenConsumedDetail { node: detail.registered_node.clone() }
                        },
                        "rejected" => rsx! {
                            TokenRejectedDetail {
                                node: detail.registered_node.clone(),
                                revoke_reason: detail.token.revoke_reason.clone(),
                            }
                        },
                        "pending" => rsx! {
                            TokenPendingDetail { message: detail.message.clone() }
                        },
                        _ => rsx! {},
                    }
                    // 通用元信息
                    div { class: "node-token-card-meta",
                        div { class: "node-token-card-meta-row",
                            span { class: "node-token-card-meta-label", {i18n.t("node_token.preview")} }
                            code { class: "node-token-card-meta-value", "{detail.token.token_preview}" }
                        }
                        div { class: "node-token-card-meta-row",
                            span { class: "node-token-card-meta-label",
                                {i18n.t("node_token.issued_at")}
                            }
                            span { class: "node-token-card-meta-value", "{detail.token.issued_at}" }
                        }
                    }

                    // 删除按钮（仅"已拒绝"且无吊销原因时可删除）
                    if can_delete {
                        div { class: "node-token-card-actions",
                            Button {
                                variant: ButtonVariant::Danger,
                                size: ButtonSize::Small,
                                onclick: move |_| delete_modal_open.set(true),
                                {i18n.t("node_token.delete")}
                            }
                        }
                    }
                }
            }

            // 删除确认弹窗
            ConfirmModal {
                open: delete_modal_open,
                title: i18n.t("node_token.delete_confirm_title"),
                message: i18n.t("node_token.delete_confirm_msg"),
                confirm_text: i18n.t("node_token.delete"),
                cancel_text: i18n.t("form.cancel"),
                close_label: i18n.t("common.close"),
                danger: true,
                onconfirm: {
                    let token_id = detail.token.id.clone();
                    let auth = auth_store.clone();
                    let mut ui = ui_store.clone();
                    let i18n = i18n.clone();
                    let mut list_key = list_key.clone();
                    move |_| {
                        delete_modal_open.set(false);
                        spawn({
                            let token_id = token_id.clone();
                            async move {
                                let token = match auth.token() {
                                    Some(t) => t,
                                    None => {
                                        ui.show_error(i18n.t("common.error"));
                                        return;
                                    }
                                };
                                match node_gateway_token_service::delete_my_token(&token_id, &token)
                                    .await
                                {
                                    Ok(_) => {
                                        ui.show_success(i18n.t("node_token.delete_success"));
                                        list_key.with_mut(|v| *v += 1);
                                    }
                                    Err(e) => {
                                        ui.show_error(
                                            format!("{}: {}", i18n.t("node_token.delete_failed"), e),
                                        );
                                    }
                                }
                            }
                        });
                    }
                },
                oncancel: move |_| delete_modal_open.set(false),
            }
        }
    }
}

// ── 详情子组件 ──

#[component]
fn TokenPendingDetail(message: Option<String>) -> Element {
    let i18n = use_i18n();
    rsx! {
        div { class: "node-token-card-status-section",
            div { class: "node-token-card-status-icon pending",
                svg {
                    width: "24",
                    height: "24",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    circle { cx: "12", cy: "12", r: "10" }
                    polyline { points: "12 6 12 12 16 14" }
                }
            }
            div { class: "node-token-card-status-text",
                p { class: "node-token-card-status-title", {i18n.t("node_token.pending_desc")} }
                if let Some(msg) = &message {
                    p { class: "node-token-card-status-desc", "{msg}" }
                }
            }
        }
    }
}

#[component]
fn TokenApprovedDetail(plaintext: String, is_revealed: bool) -> Element {
    let i18n = use_i18n();
    let ui_store = use_context::<UiStore>();
    let copied = use_signal(|| false);

    let copied_label = i18n.t("node_token.copied");
    let copy_text = i18n.t("node_token.copy");
    let copy_hint = i18n.t("node_token.copy_hint");
    let copy_manual_hint = i18n.t("common.copy_manual_hint");
    let plaintext_clone = plaintext.clone();
    let plaintext_for_button = plaintext.clone();

    rsx! {
        if is_revealed {
            Alert { variant: AlertVariant::Warning, {i18n.t("node_token.revealed_warning")} }
        } else {
            Alert { variant: AlertVariant::Info, {i18n.t("node_token.first_view_hint")} }
        }

        div { class: "node-token-copy-section",
            div { class: "kc-api-copy-block",
                pre {
                    class: if copied() { "kc-api-example copied" } else { "kc-api-example" },
                    title: if copied() { copied_label } else { copy_hint },
                    "{plaintext_clone}"
                }
                button {
                    class: "kc-api-copy-button",
                    r#type: "button",
                    onclick: on_copy(
                        plaintext_for_button.clone(),
                        copy_manual_hint.to_string(),
                        ui_store,
                        copied,
                    ),
                    IconCopy { size: 15 }
                    if copied() {
                        {copied_label}
                    } else {
                        {copy_text}
                    }
                }
            }
        }

        p { class: "node-token-hint-text", {i18n.t("node_token.token_hint")} }

        div { class: "node-token-card-status-section",
            div { class: "node-token-card-status-icon info",
                svg {
                    width: "24",
                    height: "24",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    circle { cx: "12", cy: "12", r: "10" }
                    line {
                        x1: "12",
                        y1: "16",
                        x2: "12",
                        y2: "12",
                    }
                    line {
                        x1: "12",
                        y1: "8",
                        x2: "12.01",
                        y2: "8",
                    }
                }
            }
            div { class: "node-token-card-status-text",
                p { class: "node-token-card-status-title", {i18n.t("node_token.no_revoke_hint")} }
            }
        }
    }
}

#[component]
fn TokenConsumedDetail(
    node: Option<client_api::api::node_gateway_token::RegisteredNodeInfo>,
) -> Element {
    let i18n = use_i18n();
    rsx! {
        div { class: "node-token-card-status-section",
            div { class: "node-token-card-status-icon info",
                svg {
                    width: "24",
                    height: "24",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    polyline { points: "20 6 9 17 4 12" }
                }
            }
            div { class: "node-token-card-status-text",
                p { class: "node-token-card-status-title", {i18n.t("node_token.consumed_desc")} }
            }
        }

        if let Some(ref n) = node {
            div { class: "node-token-registered-node",
                div { class: "node-token-registered-node-header",
                    span { class: "node-token-card-meta-label", {i18n.t("node_token.registered_node")} }
                    span { class: "node-token-card-meta-value", "{n.display_name}" }
                }
                div { class: "node-token-registered-node-row",
                    span { class: "node-token-card-meta-label", {i18n.t("node_token.node_status")} }
                    span {
                        class: match n.status.as_str() {
                            "online" => "node-token-node-status node-status-online",
                            "offline" => "node-token-node-status node-status-offline",
                            "excluded" => "node-token-node-status node-status-excluded",
                            _ => "node-token-node-status",
                        },
                        {registered_node_status_label(&n.status, &i18n)}
                    }
                }
                if let Some(ref hb) = n.last_heartbeat_at {
                    div { class: "node-token-registered-node-row",
                        span { class: "node-token-card-meta-label",
                            {i18n.t("node_token.last_heartbeat")}
                        }
                        span { class: "node-token-card-meta-value", {format_time(hb)} }
                    }
                }
            }
        } else {
            p { class: "node-token-hint-text", {i18n.t("node_token.consumed_hint")} }
        }
    }
}

#[component]
fn TokenRejectedDetail(
    node: Option<client_api::api::node_gateway_token::RegisteredNodeInfo>,
    revoke_reason: Option<String>,
) -> Element {
    let i18n = use_i18n();
    let mut show_reason = use_signal(|| false);

    rsx! {
        if let Some(ref n) = node {
            // 有关联节点：token 被吊销但节点仍存在
            div { class: "node-token-card-status-section",
                div { class: "node-token-card-status-icon error",
                    svg {
                        width: "24",
                        height: "24",
                        view_box: "0 0 24 24",
                        fill: "none",
                        stroke: "currentColor",
                        stroke_width: "2",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        circle { cx: "12", cy: "12", r: "10" }
                        line {
                            x1: "4.93",
                            y1: "4.93",
                            x2: "19.07",
                            y2: "19.07",
                        }
                    }
                }
                div { class: "node-token-card-status-text",
                    p { class: "node-token-card-status-title", {i18n.t("node_token.revoked_desc")} }
                }
            }

            div { class: "node-token-registered-node",
                div { class: "node-token-registered-node-header",
                    span { class: "node-token-card-meta-label", {i18n.t("node_token.registered_node")} }
                    span { class: "node-token-card-meta-value", "{n.display_name}" }
                }
                div { class: "node-token-registered-node-row",
                    span { class: "node-token-card-meta-label", {i18n.t("node_token.node_status")} }
                    span {
                        class: match n.status.as_str() {
                            "online" => "node-token-node-status node-status-online",
                            "offline" => "node-token-node-status node-status-offline",
                            "excluded" => "node-token-node-status node-status-excluded",
                            _ => "node-token-node-status",
                        },
                        {registered_node_status_label(&n.status, &i18n)}
                    }
                }
                if let Some(ref hb) = n.last_heartbeat_at {
                    div { class: "node-token-registered-node-row",
                        span { class: "node-token-card-meta-label",
                            {i18n.t("node_token.last_heartbeat")}
                        }
                        span { class: "node-token-card-meta-value", {format_time(hb)} }
                    }
                }
            }

            if let Some(ref reason) = revoke_reason {
                if !reason.is_empty() {
                    div { class: "node-token-revoke-reason",
                        Button {
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Small,
                            onclick: move |_| show_reason.toggle(),
                            if show_reason() {
                                {i18n.t("common.collapse")}
                            } else {
                                {i18n.t("node_token.view_reason")}
                            }
                        }
                        if show_reason() {
                            Alert { variant: AlertVariant::Warning, "{reason}" }
                        }
                    }
                }
            }

            if n.status == "excluded" {
                Alert { variant: AlertVariant::Warning, {i18n.t("node_token.node_excluded_hint")} }
            } else if n.status == "online" {
                Alert { variant: AlertVariant::Info, {i18n.t("node_token.node_online_hint")} }
            }

            p { class: "node-token-hint-text", {i18n.t("node_token.reapply_hint")} }
        } else {
            // 无关联节点：申请被拒绝
            div { class: "node-token-card-status-section",
                div { class: "node-token-card-status-icon error",
                    svg {
                        width: "24",
                        height: "24",
                        view_box: "0 0 24 24",
                        fill: "none",
                        stroke: "currentColor",
                        stroke_width: "2",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        circle { cx: "12", cy: "12", r: "10" }
                        line {
                            x1: "15",
                            y1: "9",
                            x2: "9",
                            y2: "15",
                        }
                        line {
                            x1: "9",
                            y1: "9",
                            x2: "15",
                            y2: "15",
                        }
                    }
                }
                div { class: "node-token-card-status-text",
                    p { class: "node-token-card-status-title", {i18n.t("node_token.rejected_desc")} }
                }
            }
        }
    }
}

#[component]
fn NodeTokenHelp() -> Element {
    let i18n = use_i18n();
    rsx! {
        div { class: "card node-token-help-card",
            div { class: "card-header",
                h3 { class: "card-title", {i18n.t("node_token.help_title")} }
            }
            div { class: "card-body",
                ol { class: "node-token-help-list",
                    li {
                        div { class: "node-token-help-step-num", "1" }
                        div { class: "node-token-help-step-text", {i18n.t("node_token.help_1")} }
                    }
                    li {
                        div { class: "node-token-help-step-num", "2" }
                        div { class: "node-token-help-step-text", {i18n.t("node_token.help_2")} }
                    }
                    li {
                        div { class: "node-token-help-step-num", "3" }
                        div { class: "node-token-help-step-text", {i18n.t("node_token.help_3")} }
                    }
                    li {
                        div { class: "node-token-help-step-num", "4" }
                        div { class: "node-token-help-step-text", {i18n.t("node_token.help_4")} }
                    }
                }
            }
        }
    }
}
