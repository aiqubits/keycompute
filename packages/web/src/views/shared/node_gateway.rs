use dioxus::prelude::*;
use ui::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, ConfirmModal, PageHeader, Table,
    TableHead,
};

use crate::hooks::use_i18n::use_i18n;
use crate::services::{api_client::with_auto_refresh, node_gateway_service};
use crate::stores::{auth_store::AuthStore, ui_store::UiStore, user_store::UserStore};
use crate::utils::time::{format_time, format_time_opt};
use crate::views::shared::accounts::NoPermissionView;

#[component]
pub fn NodeGateway() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let ui_store = use_context::<UiStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    if !is_admin {
        return rsx! { NoPermissionView { resource: i18n.t("page.node_gateway").to_string() } };
    }

    // 触发刷新
    let tokens_key = use_signal(|| 0u64);
    let overview_key = use_signal(|| 0u64);
    // 审批弹窗控制
    let mut modal_open = use_signal(|| false);
    let mut modal_token_id = use_signal(String::new);
    let mut modal_action = use_signal(String::new);
    // 吊销节点弹窗控制（含原因输入）
    let mut revoke_modal_open = use_signal(|| false);
    let mut revoke_node_id = use_signal(String::new);
    let mut revoke_reason = use_signal(String::new);
    // 删除节点弹窗控制
    let mut delete_modal_open = use_signal(|| false);
    let mut delete_node_id = use_signal(String::new);
    // 恢复节点弹窗控制
    let mut recover_modal_open = use_signal(|| false);
    let mut recover_node_id = use_signal(String::new);

    let overview = use_resource(move || {
        let _key = overview_key();
        async move {
            with_auto_refresh(auth_store, |token| async move {
                node_gateway_service::overview(&token).await
            })
            .await
        }
    });

    let pending_tokens = use_resource(move || {
        let _key = tokens_key();
        async move {
            with_auto_refresh(auth_store, |token| async move {
                node_gateway_service::list_pending_tokens(&token).await
            })
            .await
        }
    });

    let do_approve = move |token_id: String, action: String| {
        let mut ui_store = ui_store;
        #[allow(unused_mut)]
        let mut tokens_key = tokens_key;
        let i18n = i18n;
        spawn(async move {
            let req = client_api::api::admin::ApproveTokenRequest {
                action: action.clone(),
            };
            let action_display = req.action.clone();
            let tid = token_id.clone();
            let result = with_auto_refresh(auth_store, |token| {
                let tid = tid.clone();
                let req = req.clone();
                async move { node_gateway_service::approve_token(&tid, &req, &token).await }
            })
            .await;
            match result {
                Ok(_) => {
                    let msg = if action_display == "approve" {
                        i18n.t("node_gateway.approve_success")
                    } else {
                        i18n.t("node_gateway.reject_success")
                    };
                    ui_store.show_success(msg);
                    tokens_key.with_mut(|v| *v += 1);
                }
                Err(e) => {
                    let err_msg = format!("{}: {}", i18n.t("node_gateway.approve_failed"), e);
                    ui_store.show_error(err_msg);
                }
            }
        });
    };

    let do_revoke = move |node_id: String, reason: String| {
        let mut ui_store = ui_store;
        let mut overview_key = overview_key;
        let i18n = i18n;
        spawn(async move {
            let result = with_auto_refresh(auth_store, |token| {
                let nid = node_id.clone();
                let r = reason.clone();
                async move { node_gateway_service::revoke_node_token(&nid, &r, &token).await }
            })
            .await;
            match result {
                Ok(_) => {
                    ui_store.show_success(i18n.t("node_gateway.revoke_success"));
                    overview_key.with_mut(|v| *v += 1);
                }
                Err(e) => {
                    let err_msg = format!("{}: {}", i18n.t("node_gateway.revoke_failed"), e);
                    ui_store.show_error(err_msg);
                }
            }
        });
    };

    let do_delete = move |node_id: String| {
        let mut ui_store = ui_store;
        let mut overview_key = overview_key;
        let i18n = i18n;
        spawn(async move {
            let result = with_auto_refresh(auth_store, |token| {
                let nid = node_id.clone();
                async move { node_gateway_service::delete_node(&nid, &token).await }
            })
            .await;
            match result {
                Ok(_) => {
                    ui_store.show_success(i18n.t("node_gateway.delete_success"));
                    overview_key.with_mut(|v| *v += 1);
                }
                Err(e) => {
                    let err_msg = format!("{}: {}", i18n.t("node_gateway.delete_failed"), e);
                    ui_store.show_error(err_msg);
                }
            }
        });
    };

    let do_recover = move |node_id: String| {
        let mut ui_store = ui_store;
        let mut overview_key = overview_key;
        let i18n = i18n;
        spawn(async move {
            let result = with_auto_refresh(auth_store, |token| {
                let nid = node_id.clone();
                async move { node_gateway_service::recover_node(&nid, &token).await }
            })
            .await;
            match result {
                Ok(_) => {
                    ui_store.show_success(i18n.t("node_gateway.recover_success"));
                    overview_key.with_mut(|v| *v += 1);
                }
                Err(e) => {
                    let err_msg = format!("{}: {}", i18n.t("node_gateway.recover_failed"), e);
                    ui_store.show_error(err_msg);
                }
            }
        });
    };

    rsx! {
        // 审批确认弹窗
        ConfirmModal {
            open: modal_open,
            title: if modal_action() == "approve" {
                i18n.t("node_gateway.approve_confirm_title")
            } else {
                i18n.t("node_gateway.reject_confirm_title")
            },
            message: if modal_action() == "approve" {
                i18n.t("node_gateway.approve_confirm_msg")
            } else {
                i18n.t("node_gateway.reject_confirm_msg")
            },
            confirm_text: if modal_action() == "approve" {
                i18n.t("node_gateway.approve")
            } else {
                i18n.t("node_gateway.reject")
            },
            cancel_text: i18n.t("form.cancel"),
            close_label: i18n.t("common.close"),
            danger: modal_action() == "reject",
            onconfirm: move |_| {
                let token_id = modal_token_id();
                let action = modal_action();
                modal_open.set(false);
                do_approve(token_id, action);
            },
            oncancel: move |_| {
                modal_open.set(false);
            },
        }

        // 吊销节点弹窗（自定义，含原因输入框）
        if revoke_modal_open() {
            div { class: "modal-backdrop",
                onclick: move |_| {
                    revoke_modal_open.set(false);
                    revoke_reason.set(String::new());
                },
                div {
                    class: "modal",
                    role: "dialog",
                    aria_modal: "true",
                    aria_label: i18n.t("node_gateway.revoke_confirm_title"),
                    onclick: move |e| e.stop_propagation(),
                    div { class: "modal-header",
                        h2 { class: "modal-title", {i18n.t("node_gateway.revoke_confirm_title")} }
                        button {
                            class: "btn btn-ghost btn-sm",
                            r#type: "button",
                            aria_label: i18n.t("common.close"),
                            onclick: move |_| {
                                revoke_modal_open.set(false);
                                revoke_reason.set(String::new());
                            },
                            "✕"
                        }
                    }
                    div { class: "modal-body",
                        p { class: "text-secondary", {i18n.t("node_gateway.revoke_confirm_msg")} }
                        div { class: "form-group",
                            label { class: "form-label",
                                {i18n.t("node_gateway.revoke_reason_label")}
                                span { class: "required-mark", " *" }
                            }
                            textarea {
                                class: "input-field",
                                placeholder: "{i18n.t(\"node_gateway.revoke_reason_placeholder\")}",
                                value: "{revoke_reason}",
                                oninput: move |e| revoke_reason.set(e.value()),
                            }
                        }
                    }
                    div { class: "modal-footer",
                        Button {
                            variant: ButtonVariant::Ghost,
                            onclick: move |_| {
                                revoke_modal_open.set(false);
                                revoke_reason.set(String::new());
                            },
                            {i18n.t("form.cancel")}
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            disabled: revoke_reason().trim().is_empty(),
                            onclick: move |_| {
                                let node_id = revoke_node_id();
                                let reason = revoke_reason();
                                if reason.trim().is_empty() {
                                    return;
                                }
                                revoke_modal_open.set(false);
                                revoke_reason.set(String::new());
                                do_revoke(node_id, reason);
                            },
                            {i18n.t("node_gateway.revoke")}
                        }
                    }
                }
            }
        }

        // 删除节点确认弹窗
        ConfirmModal {
            open: delete_modal_open,
            title: i18n.t("node_gateway.delete_confirm_title"),
            message: i18n.t("node_gateway.delete_confirm_msg"),
            confirm_text: i18n.t("node_gateway.delete"),
            cancel_text: i18n.t("form.cancel"),
            close_label: i18n.t("common.close"),
            danger: true,
            onconfirm: move |_| {
                let node_id = delete_node_id();
                delete_modal_open.set(false);
                do_delete(node_id);
            },
            oncancel: move |_| {
                delete_modal_open.set(false);
            },
        }

        // 恢复节点确认弹窗
        ConfirmModal {
            open: recover_modal_open,
            title: i18n.t("node_gateway.recover_confirm_title"),
            message: i18n.t("node_gateway.recover_confirm_msg"),
            confirm_text: i18n.t("node_gateway.recover"),
            cancel_text: i18n.t("form.cancel"),
            close_label: i18n.t("common.close"),
            danger: false,
            onconfirm: move |_| {
                let node_id = recover_node_id();
                recover_modal_open.set(false);
                do_recover(node_id);
            },
            oncancel: move |_| {
                recover_modal_open.set(false);
            },
        }

        div { class: "page-container node-gateway-page",
            PageHeader {
                title: i18n.t("page.node_gateway").to_string(),
                description: i18n.t("node_gateway.subtitle").to_string(),
            }

            match overview() {
                None => rsx! { p { class: "text-secondary", {i18n.t("table.loading")} } },
                Some(Err(ref e)) => rsx! {
                    div { class: "alert alert-error",
                        p { "{i18n.t(\"common.load_failed\")}: {e}" }
                    }
                },
                Some(Ok(ref data)) => rsx! {
                    div { class: "section",
                        div { class: "node-gateway-status-row",
                            div {
                                h2 { class: "section-title", {i18n.t("node_gateway.runtime_status")} }
                                p { class: "text-secondary", {i18n.t("node_gateway.runtime_desc")} }
                            }
                            if data.enabled {
                                Badge { variant: BadgeVariant::Success, {i18n.t("node_gateway.enabled")} }
                            } else {
                                Badge { variant: BadgeVariant::Warning, {i18n.t("node_gateway.disabled")} }
                            }
                        }
                        div { class: "stats-grid",
                            StatCard { label: i18n.t("node_gateway.nodes_total").to_string(), value: data.node_stats.total.to_string(), meta: i18n.t("node_gateway.nodes_total_desc").to_string() }
                            StatCard { label: i18n.t("node_gateway.nodes_online").to_string(), value: data.node_stats.online.to_string(), meta: i18n.t("node_gateway.nodes_online_desc").to_string() }
                            StatCard { label: i18n.t("node_gateway.tasks_active").to_string(), value: (data.task_stats.queued + data.task_stats.leased).to_string(), meta: i18n.t("node_gateway.tasks_active_desc").to_string() }
                            StatCard { label: i18n.t("node_gateway.tasks_done").to_string(), value: data.task_stats.succeeded.to_string(), meta: i18n.t("node_gateway.tasks_done_desc").to_string() }
                        }
                    }

                    div { class: "section",
                        h2 { class: "section-title", {i18n.t("node_gateway.protocol_title")} }
                        div { class: "node-gateway-protocol-grid",
                            ProtocolItem { method: "POST".to_string(), path: "/node/v1/register".to_string(), desc: i18n.t("node_gateway.protocol_register").to_string() }
                            ProtocolItem { method: "POST".to_string(), path: "/node/v1/heartbeat".to_string(), desc: i18n.t("node_gateway.protocol_heartbeat").to_string() }
                            ProtocolItem { method: "POST".to_string(), path: "/node/v1/tasks/poll".to_string(), desc: i18n.t("node_gateway.protocol_poll").to_string() }
                            ProtocolItem { method: "POST".to_string(), path: "/node/v1/tasks/{task_id}/complete".to_string(), desc: i18n.t("node_gateway.protocol_complete").to_string() }
                        }
                    }

                    // ── 注册令牌审批（Admin）──────────────────
                    div { class: "section",
                        div { class: "node-gateway-status-row",
                            div {
                                h2 { class: "section-title", {i18n.t("node_gateway.token_approval_title")} }
                                p { class: "text-secondary", {i18n.t("node_gateway.token_approval_desc")} }
                            }
                            {
                                match pending_tokens() {
                                    Some(Ok(ref tokens)) if !tokens.is_empty() => rsx! {
                                        Badge { variant: BadgeVariant::Warning,
                                            {i18n.t("node_gateway.token_approval_pending_count").replace("{count}", &tokens.len().to_string())}
                                        }
                                    },
                                    _ => rsx! {},
                                }
                            }
                        }

                        match pending_tokens() {
                            None => rsx! {
                                p { class: "text-secondary", {i18n.t("table.loading")} }
                            },
                            Some(Err(ref e)) => rsx! {
                                div { class: "alert alert-warning",
                                    p { "{i18n.t(\"common.load_failed\")}: {e}" }
                                }
                            },
                            Some(Ok(ref tokens)) => rsx! {
                                Table {
                                    empty: tokens.is_empty(),
                                    empty_text: i18n.t("node_gateway.no_pending_tokens"),
                                    col_count: 5,
                                    thead {
                                        tr {
                                            TableHead { {i18n.t("node_gateway.token_approval_email")} }
                                            TableHead { {i18n.t("node_gateway.token_approval_preview")} }
                                            TableHead { {i18n.t("node_gateway.token_approval_apply_time")} }
                                            TableHead { {i18n.t("table.status")} }
                                            TableHead { {i18n.t("table.actions")} }
                                        }
                                    }
                                    tbody {
                                        for t in tokens.iter() {
                                            tr {
                                                td {
                                                    div { class: "account-cell-main",
                                                        span { class: "account-name", "{t.user_email}" }
                                                    }
                                                }
                                                td {
                                                    code { "{t.token_preview}" }
                                                }
                                                td {
                                                    span { class: "account-time-value", "{t.issued_at}" }
                                                }
                                                td {
                                                    Badge { variant: BadgeVariant::Warning, {i18n.t("node_gateway.token_status_pending")} }
                                                }
                                                td {
                                                    div { style: "display: flex; gap: 8px; align-items: center;",
                                                        Button {
                                                            variant: ButtonVariant::Primary,
                                                            size: ButtonSize::Small,
                                                            disabled: modal_open(),
                                                            onclick: {
                                                                let token_id = t.id.clone();
                                                                move |_| {
                                                                    modal_token_id.set(token_id.clone());
                                                                    modal_action.set("approve".to_string());
                                                                    modal_open.set(true);
                                                                }
                                                            },
                                                            {i18n.t("node_gateway.approve")}
                                                        }
                                                        Button {
                                                            variant: ButtonVariant::Ghost,
                                                            size: ButtonSize::Small,
                                                            disabled: modal_open(),
                                                            onclick: {
                                                                let token_id = t.id.clone();
                                                                move |_| {
                                                                    modal_token_id.set(token_id.clone());
                                                                    modal_action.set("reject".to_string());
                                                                    modal_open.set(true);
                                                                }
                                                            },
                                                            {i18n.t("node_gateway.reject")}
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

                    div { class: "section",
                        h2 { class: "section-title", {i18n.t("node_gateway.nodes_title")} }
                        Table {
                            empty: data.nodes.is_empty(),
                            empty_text: i18n.t("node_gateway.no_nodes"),
                            col_count: 8,
                            thead {
                                tr {
                                    TableHead { {i18n.t("node_gateway.node")} }
                                    TableHead { {i18n.t("table.status")} }
                                    TableHead { {i18n.t("node_gateway.models")} }
                                    TableHead { {i18n.t("node_gateway.failures")} }
                                    TableHead { {i18n.t("node_gateway.heartbeat")} }
                                    TableHead { {i18n.t("node_gateway.token_preview")} }
                                    TableHead { "ID" }
                                    TableHead { {i18n.t("table.actions")} }
                                }
                            }
                            tbody {
                                for node in data.nodes.iter() {
                                    tr {
                                        td {
                                            div { class: "account-cell-main",
                                                div { class: "account-name-row",
                                                    span { class: "account-name", "{node.display_name}" }
                                                }
                                                div { class: "account-subline", "{node.client_instance_id}" }
                                            }
                                        }
                                        td { NodeStatusBadge { status: node.status.clone() } }
                                        td {
                                            div { class: "account-models",
                                                for model in accepted_models(&node.accepted_models_json).iter() {
                                                    span { class: "account-model-chip", "{model}" }
                                                }
                                                if accepted_models(&node.accepted_models_json).is_empty() {
                                                    span { class: "account-model-chip account-model-chip-muted", {i18n.t("node_gateway.no_models")} }
                                                }
                                            }
                                        }
                                        td { "{node.consecutive_failure_count}/{node.failure_threshold}" }
                                        td { span { class: "account-time-value", {format_time_opt(node.last_heartbeat_at.as_deref())} } }
                                        td {
                                            if let Some(ref preview) = node.token_preview {
                                                code { "{preview}" }
                                            } else {
                                                span { class: "text-secondary", "—" }
                                            }
                                        }
                                        td { span { class: "account-id", "{short_id(&node.id)}" } }
                                        td {
                                            div { class: "accounts-actions",
                                                if node.status == "excluded" {
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: ButtonSize::Small,
                                                        disabled: recover_modal_open(),
                                                        onclick: {
                                                            let node_id = node.id.clone();
                                                            move |_| {
                                                                recover_node_id.set(node_id.clone());
                                                                recover_modal_open.set(true);
                                                            }
                                                        },
                                                        {i18n.t("node_gateway.recover")}
                                                    }
                                                } else {
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: ButtonSize::Small,
                                                        disabled: revoke_modal_open(),
                                                        onclick: {
                                                            let node_id = node.id.clone();
                                                            move |_| {
                                                                revoke_node_id.set(node_id.clone());
                                                                revoke_reason.set(String::new());
                                                                revoke_modal_open.set(true);
                                                            }
                                                        },
                                                        {i18n.t("node_gateway.revoke")}
                                                    }
                                                }
                                                Button {
                                                    variant: ButtonVariant::Danger,
                                                    size: ButtonSize::Small,
                                                    disabled: delete_modal_open(),
                                                    onclick: {
                                                        let node_id = node.id.clone();
                                                        move |_| {
                                                            delete_node_id.set(node_id.clone());
                                                            delete_modal_open.set(true);
                                                        }
                                                    },
                                                    {i18n.t("node_gateway.delete")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "section",
                        h2 { class: "section-title", {i18n.t("node_gateway.tasks_title")} }
                        Table {
                            empty: data.recent_tasks.is_empty(),
                            empty_text: i18n.t("node_gateway.no_tasks"),
                            col_count: 6,
                            thead {
                                tr {
                                    TableHead { {i18n.t("pricing.model_name")} }
                                    TableHead { {i18n.t("table.status")} }
                                    TableHead { {i18n.t("node_gateway.assigned_node")} }
                                    TableHead { {i18n.t("node_gateway.failures")} }
                                    TableHead { {i18n.t("node_gateway.deadline")} }
                                    TableHead { "ID" }
                                }
                            }
                            tbody {
                                for task in data.recent_tasks.iter() {
                                    tr {
                                        td { "{task.model}" }
                                        td { TaskStatusBadge { status: task.status.clone() } }
                                        td { "{task.assigned_node_id.as_deref().map(short_id).unwrap_or_else(|| \"—\".to_string())}" }
                                        td { "{task.failure_count}/{task.failure_threshold}" }
                                        td { span { class: "account-time-value", {format_time(&task.deadline_at)} } }
                                        td { span { class: "account-id", "{short_id(&task.id)}" } }
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

#[component]
fn StatCard(label: String, value: String, meta: String) -> Element {
    rsx! {
        div { class: "stat-card",
            p { class: "stat-label", "{label}" }
            p { class: "stat-value", "{value}" }
            p { class: "stat-meta", "{meta}" }
        }
    }
}

#[component]
fn ProtocolItem(method: String, path: String, desc: String) -> Element {
    rsx! {
        div { class: "node-gateway-protocol-item",
            div { class: "node-gateway-protocol-head",
                span { class: "node-gateway-method", "{method}" }
                code { class: "node-gateway-path", "{path}" }
            }
            p { class: "text-secondary", "{desc}" }
        }
    }
}

#[component]
fn NodeStatusBadge(status: String) -> Element {
    let i18n = use_i18n();
    let variant = match status.as_str() {
        "online" => BadgeVariant::Success,
        "excluded" => BadgeVariant::Error,
        "offline" => BadgeVariant::Warning,
        _ => BadgeVariant::Neutral,
    };
    let label = match status.as_str() {
        "online" => i18n.t("node_gateway.status_online"),
        "offline" => i18n.t("node_gateway.status_offline"),
        "excluded" => i18n.t("node_gateway.status_excluded"),
        _ => status.as_str(),
    };
    rsx! { Badge { variant, "{label}" } }
}

#[component]
fn TaskStatusBadge(status: String) -> Element {
    let i18n = use_i18n();
    let variant = match status.as_str() {
        "succeeded" => BadgeVariant::Success,
        "failed" | "expired" => BadgeVariant::Error,
        "leased" => BadgeVariant::Warning,
        "queued" => BadgeVariant::Neutral,
        _ => BadgeVariant::Neutral,
    };
    let label = match status.as_str() {
        "queued" => i18n.t("node_gateway.task_queued"),
        "leased" => i18n.t("node_gateway.task_leased"),
        "succeeded" => i18n.t("node_gateway.task_succeeded"),
        "failed" => i18n.t("node_gateway.task_failed"),
        "expired" => i18n.t("node_gateway.task_expired"),
        _ => status.as_str(),
    };
    rsx! { Badge { variant, "{label}" } }
}

fn accepted_models(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter_map(|model| model.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
