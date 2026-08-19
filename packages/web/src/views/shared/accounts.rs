use client_api::api::admin::AccountTestResponse;
use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Pagination, Table, TableHead};

const PAGE_SIZE: usize = 20;

use crate::hooks::use_i18n::use_i18n;
use crate::i18n::I18n;
use crate::services::{
    account_service, api_client::with_auto_refresh, debug_service, tenant_service,
};
use crate::stores::auth_store::AuthStore;
use crate::stores::ui_store::UiStore;
use crate::stores::user_store::UserStore;
use crate::utils::time::format_time;

/// 厂商预设列表：(预设 id, 显示名 i18n key, 协议, Base URL 模板)
///
/// 后端仅支持两种协议（openai / anthropic），厂商通过
/// `协议 + base_url + api_key` 接入。选择预设后自动填充协议与
/// Base URL 模板（空串表示使用协议默认端点）。
pub(crate) const PRESETS: &[(&str, &str, &str, &str)] = &[
    (
        "openai",
        "accounts.provider_openai_compatible",
        "openai",
        "",
    ),
    (
        "anthropic",
        "accounts.provider_anthropic_compatible",
        "anthropic",
        "",
    ),
    (
        "deepseek",
        "accounts.provider_deepseek_openai_compatible",
        "openai",
        "https://api.deepseek.com/v1",
    ),
    (
        "gemini",
        "accounts.provider_gemini_openai_compatible",
        "openai",
        "https://generativelanguage.googleapis.com/v1beta/openai",
    ),
    (
        "vllm",
        "accounts.provider_vllm_openai_compatible",
        "openai",
        "http://localhost:8000/v1",
    ),
    (
        "ollama",
        "accounts.provider_ollama_openai_compatible",
        "openai",
        "http://localhost:11434/v1",
    ),
];

/// 根据预设 id 查找预设定义
fn preset_by_id(
    id: &str,
) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    PRESETS.iter().find(|(value, _, _, _)| *value == id)
}

fn provider_label(provider: &str, i18n: I18n) -> String {
    // 账号的 provider 字段存储协议名（openai / anthropic）
    match provider {
        "openai" => i18n.t("accounts.provider_openai_compatible").to_string(),
        "anthropic" => i18n.t("accounts.provider_anthropic_compatible").to_string(),
        other => other.to_string(),
    }
}

/// 根据预设获取模型提示文本
fn models_placeholder_key_for(preset: &str) -> &'static str {
    match preset {
        "openai" => "accounts.models_placeholder_openai",
        "anthropic" => "accounts.models_placeholder_claude",
        "deepseek" => "accounts.models_placeholder_deepseek",
        "gemini" => "accounts.models_placeholder_gemini",
        "vllm" => "accounts.models_placeholder_vllm",
        "ollama" => "accounts.models_placeholder_ollama",
        _ => "accounts.models_placeholder_default",
    }
}

/// 协议默认端点（与后端 ProtocolType::default_endpoint 保持一致）
///
/// web 为独立 WASM 包，无法引用后端 llm-protocol-provider 常量，只能
/// 在此维护硬编码副本；若后端默认端点变更，需同步修改此处
/// （protocol_default_endpoint_maps_known_protocols 测试锁定当前值）。
///
/// 预设模板为空串时表示使用协议默认端点，占位提示显示该值，
/// 暗示用户留空即可使用默认端点。
fn protocol_default_endpoint(protocol: &str) -> &'static str {
    match protocol {
        "anthropic" => "https://api.anthropic.com/v1",
        // openai 及未知协议回落到 OpenAI 默认端点
        _ => "https://api.openai.com/v1",
    }
}

/// 根据预设获取 Base URL 占位提示（仅创建表单使用）
///
/// 预设模板非空时直接复用模板 URL；openai / anthropic 模板为空，
/// 提示协议默认端点（留空即使用）。编辑表单直接按账号协议取
/// protocol_default_endpoint，不经过预设查找。
fn base_url_placeholder_for(preset: &str) -> String {
    preset_by_id(preset)
        .map(|(_, _, protocol, base_url)| {
            if base_url.is_empty() {
                protocol_default_endpoint(protocol).to_string()
            } else {
                base_url.to_string()
            }
        })
        .unwrap_or_else(|| protocol_default_endpoint("openai").to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum AccountTestOutcome {
    Success,
    Failure,
}

fn account_test_outcome(response: AccountTestResponse) -> AccountTestOutcome {
    if response.success {
        AccountTestOutcome::Success
    } else {
        // The server message is intentionally generic and not localized. Keep
        // presentation localized instead of combining it with the UI language.
        AccountTestOutcome::Failure
    }
}

/// 账号管理页面（LLM 渠道配置）
///
/// - 普通用户：无权限提示
/// - Admin：管理 LLM Provider 渠道，支持测试连接、刷新状态
#[component]
pub fn Accounts() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    if is_admin {
        rsx! {
            AdminAccountsView {}
        }
    } else {
        rsx! {
            NoPermissionView { resource: i18n.t("page.accounts").to_string() }
        }
    }
}

// ── Admin 视图 ────────────────────────────────────────────────────────

#[component]
fn AdminAccountsView() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let mut ui_store = use_context::<UiStore>();
    let mut show_create = use_signal(|| false);
    let mut create_name = use_signal(String::new);
    // 预设 id（驱动下拉选择与默认模型/提示），提交的 provider 为协议名
    let mut create_preset = use_signal(|| "openai".to_string());
    let mut create_provider = use_signal(|| "openai".to_string());
    let mut create_api_key = use_signal(String::new);
    let mut create_api_base = use_signal(String::new);
    let mut create_models_input = use_signal(String::new); // 逗号分隔的模型列表
    let mut saving = use_signal(|| false);
    let mut error_msg = use_signal(String::new);
    let mut page = use_signal(|| 1u32);

    // 全局重置健康状态
    let mut resetting = use_signal(|| false);

    // 编辑弹窗状态
    let mut edit_id = use_signal(String::new);
    let mut edit_name = use_signal(String::new);
    let mut edit_provider = use_signal(String::new);
    let mut edit_api_key = use_signal(String::new);
    let mut edit_api_base = use_signal(String::new);
    let mut edit_reset_api_base = use_signal(|| false);
    let mut edit_is_active = use_signal(|| true);
    let mut edit_visibility = use_signal(|| "tenant".to_string());
    let mut edit_tenant_id = use_signal(String::new);
    let mut show_tenant_dropdown = use_signal(|| false);
    let mut show_edit = use_signal(|| false);
    let mut edit_saving = use_signal(|| false);
    let mut edit_error = use_signal(String::new);

    // 删除确认弹窗状态
    let mut delete_id = use_signal(String::new);
    let mut delete_name = use_signal(String::new);
    let mut show_delete = use_signal(|| false);
    let mut deleting = use_signal(|| false);

    let mut accounts = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            account_service::list(None, &token).await
        })
        .await
    });

    // 租户列表（编辑弹窗下拉选项）
    let tenants = use_resource(move || async move {
        let token = auth_store.token().unwrap_or_default();
        tenant_service::list(None, &token).await
    });

    // 全局重置健康状态处理函数
    let on_reset_health = move |_| {
        let auth = auth_store.clone();
        let mut ui = ui_store.clone();
        resetting.set(true);
        spawn(async move {
            let token = auth.token().unwrap_or_default();
            match debug_service::reset_health(&token).await {
                Ok(resp) => {
                    ui.show_success(&resp.message);
                }
                Err(e) => {
                    ui.show_error(format!("{}: {}", i18n.t("accounts.reset_failed"), e));
                }
            }
            resetting.set(false);
        });
    };

    let on_submit = move |_| {
        let name = create_name();
        let provider = create_provider();
        let api_key_val = create_api_key();
        let api_base = create_api_base();
        let models_str = create_models_input();
        // 解析模型列表（逗号分隔，去空格，去空项）
        let models: Vec<String> = models_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if name.is_empty() || provider.is_empty() || api_key_val.is_empty() || models.is_empty() {
            *error_msg.write() = i18n.t("accounts.fill_required").to_string();
            return;
        }
        let token = auth_store.token().unwrap_or_default();
        *saving.write() = true;
        *error_msg.write() = String::new();
        spawn(async move {
            use client_api::api::admin::CreateAccountRequest;
            let mut req =
                CreateAccountRequest::new(name, provider, api_key_val).with_models(models);
            if !api_base.is_empty() {
                req = req.with_api_base(api_base);
            }
            match account_service::create(req, &token).await {
                Ok(_) => {
                    *show_create.write() = false;
                    create_name.write().clear();
                    *create_preset.write() = "openai".to_string();
                    *create_provider.write() = "openai".to_string();
                    create_api_key.write().clear();
                    create_api_base.write().clear();
                    create_models_input.write().clear();
                    page.set(1);
                    accounts.restart();
                    ui_store.show_success(i18n.t("accounts.created"));
                }
                Err(e) => {
                    *error_msg.write() = format!("{}: {}", i18n.t("accounts.create_failed"), e);
                }
            }
            *saving.write() = false;
        });
    };

    // 提交编辑
    let on_edit_save = move |_| {
        let id = edit_id();
        let name_val = edit_name();
        let key_val = edit_api_key();
        let base_val = edit_api_base();
        let reset_base = edit_reset_api_base();
        let active = edit_is_active();
        let visibility = edit_visibility();
        let tenant_id = edit_tenant_id();
        if name_val.trim().is_empty() {
            *edit_error.write() = i18n.t("accounts.name_required").to_string();
            return;
        }
        let token = auth_store.token().unwrap_or_default();
        edit_saving.set(true);
        *edit_error.write() = String::new();
        spawn(async move {
            use client_api::api::admin::UpdateAccountRequest;
            let mut req = UpdateAccountRequest::new()
                .with_name(name_val)
                .with_is_active(active)
                .with_visibility(visibility);
            if !tenant_id.trim().is_empty() {
                req = req.with_tenant_id(tenant_id);
            }
            if !key_val.trim().is_empty() {
                req = req.with_api_key(key_val);
            }
            if reset_base {
                // 显式空串 = 重置为协议默认端点（后端约定）
                req.api_base = Some(String::new());
            } else if !base_val.trim().is_empty() {
                req.api_base = Some(base_val);
            }
            match account_service::update(&id, req, &token).await {
                Ok(_) => {
                    show_edit.set(false);
                    accounts.restart();
                    ui_store.show_success(i18n.t("accounts.updated"));
                }
                Err(e) => {
                    *edit_error.write() = format!("{}: {}", i18n.t("accounts.update_failed"), e);
                }
            }
            edit_saving.set(false);
        });
    };

    let reset_health_label = if resetting() {
        i18n.t("accounts.resetting")
    } else {
        i18n.t("accounts.reset_health")
    };
    let create_save_label = if saving() {
        i18n.t("form.saving")
    } else {
        i18n.t("form.save")
    };
    let edit_save_label = if edit_saving() {
        i18n.t("form.saving")
    } else {
        i18n.t("form.save")
    };
    let delete_confirm_label = if deleting() {
        i18n.t("accounts.deleting")
    } else {
        i18n.t("accounts.confirm_delete")
    };

    rsx! {
        div { class: "page-container",
            div { class: "page-header",
                h1 { class: "page-title", {i18n.t("page.accounts")} }
                p { class: "page-description", {i18n.t("accounts.subtitle")} }
                button {
                    class: "btn btn-primary",
                    onclick: move |_| *show_create.write() = true,
                    {i18n.t("accounts.add_channel")}
                }
            }

            // 操作工具栏
            div { class: "toolbar",
                div { class: "toolbar-left",
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        loading: resetting(),
                        onclick: on_reset_health,
                        "{reset_health_label}"
                    }
                }
            }

            {
                let (is_empty, empty_text) = match accounts() {
                    None => (true, i18n.t("table.loading")),
                    Some(Err(_)) => (true, i18n.t("common.load_failed")),
                    Some(Ok(ref l)) if l.is_empty() => (true, i18n.t("accounts.empty")),
                    _ => (false, ""),
                };
                let total = accounts().and_then(|r| r.ok()).map(|l| l.len()).unwrap_or(0);
                let total_pages = total.div_ceil(PAGE_SIZE).max(1) as u32;
                let start = (page() as usize - 1) * PAGE_SIZE;
                let paged_list: Vec<_> = accounts()
                    .and_then(|r| r.ok())
                    .map(|l| l.into_iter().skip(start).take(PAGE_SIZE).collect())
                    .unwrap_or_default();
                rsx! {
                    div { class: "accounts-table-shell",
                        div { class: "accounts-table-intro",
                            div {
                                h2 { class: "accounts-table-title", {i18n.t("accounts.table_title")} }
                                p { class: "accounts-table-subtitle", {i18n.t("accounts.table_subtitle")} }
                            }
                            div { class: "accounts-table-meta",
                                "{i18n.t(\"common.total_items\")} {total} {i18n.t(\"accounts.channels_suffix\")}"
                            }
                        }
                        Table {
                            class: "accounts-table".to_string(),
                            empty: is_empty,
                            empty_text: empty_text.to_string(),
                            col_count: 7,
                            thead {
                                tr {
                                    TableHead { {i18n.t("accounts.channel")} }
                                    TableHead { {i18n.t("accounts.provider_model")} }
                                    TableHead { {i18n.t("accounts.runtime_status")} }
                                    TableHead { {i18n.t("accounts.rate_quota")} }
                                    TableHead { {i18n.t("accounts.tenant_id")} }
                                    TableHead { {i18n.t("common.time")} }
                                    TableHead { {i18n.t("table.actions")} }
                                }
                            }
                            tbody {
                                if accounts().and_then(|r| r.ok()).is_some() {
                                    for acc in paged_list.iter() {
                                        tr {
                                            td {
                                                div { class: "account-cell-main",
                                                    div { class: "account-name-row",
                                                        span { class: "account-name", "{acc.name}" }
                                                        span { class: "account-id",
                                                            "#{acc.id.chars().take(8).collect::<String>()}"
                                                        }
                                                    }
                                                    div { class: "account-subline",
                                                        span { class: "account-secret-label",
                                                            {i18n.t("accounts.key_preview")}
                                                        }
                                                        code { class: "account-key-preview", "{acc.api_key_preview}" }
                                                    }
                                                    if let Some(api_base) = &acc.api_base {
                                                        p { class: "account-endpoint", "{api_base}" }
                                                    } else {
                                                        p { class: "account-endpoint account-endpoint-muted",
                                                            {i18n.t("accounts.default_endpoint")}
                                                        }
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "account-provider-cell",
                                                    span { class: "account-provider-badge account-provider-{acc.provider}",
                                                        "{provider_label(&acc.provider, i18n)}"
                                                    }
                                                    p { class: "account-provider-code", "{acc.provider}" }
                                                    div { class: "account-models",
                                                        if acc.models.is_empty() {
                                                            span { class: "account-model-chip account-model-chip-muted",
                                                                {i18n.t("accounts.no_models")}
                                                            }
                                                        } else {
                                                            for model in acc.models.iter().take(2) {
                                                                span { class: "account-model-chip", "{model}" }
                                                            }
                                                            if acc.models.len() > 2 {
                                                                span { class: "account-model-chip account-model-chip-muted",
                                                                    "+{acc.models.len() - 2}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "account-status-stack",
                                                    div { class: "account-status-row",
                                                        if acc.is_active {
                                                            Badge { variant: BadgeVariant::Success,
                                                                {i18n.t("common.enabled")}
                                                            }
                                                        } else {
                                                            Badge { variant: BadgeVariant::Neutral,
                                                                {i18n.t("common.disabled")}
                                                            }
                                                        }
                                                        if acc.is_healthy {
                                                            Badge { variant: BadgeVariant::Success,
                                                                {i18n.t("system.healthy")}
                                                            }
                                                        } else {
                                                            Badge { variant: BadgeVariant::Warning,
                                                                {i18n.t("dashboard.pending_check")}
                                                            }
                                                        }
                                                    }
                                                    p { class: "account-status-note",
                                                        if acc.is_active && acc.is_healthy {
                                                            {i18n.t("accounts.route_ready")}
                                                        } else if acc.is_active {
                                                            {i18n.t("accounts.enabled_but_unhealthy")}
                                                        } else {
                                                            {i18n.t("accounts.not_routed")}
                                                        }
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "account-rpm-cell",
                                                    div { class: "account-rpm-metric",
                                                        span { class: "account-rpm-current", "{acc.current_rpm}" }
                                                        span { class: "account-rpm-divider", "/" }
                                                        span { class: "account-rpm-limit", "{acc.rpm_limit}" }
                                                    }
                                                    p { class: "account-rpm-label", {i18n.t("accounts.rpm_label")} }
                                                }
                                            }
                                            td {
                                                div { class: "account-tenant-cell",
                                                    span { class: "account-tenant-id",
                                                        "{acc.tenant_id.chars().take(8).collect::<String>()}"
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "account-time-cell",
                                                    div { class: "account-time-block",
                                                        span { class: "account-time-label",
                                                            {i18n.t("common.created_at_label")}
                                                        }
                                                        span { class: "account-time-value", {format_time(&acc.created_at)} }
                                                    }
                                                    div { class: "account-time-block",
                                                        span { class: "account-time-label", {i18n.t("accounts.last_used")} }
                                                        span { class: "account-time-value",
                                                            {
                                                                if let Some(last_used) = &acc.last_used_at {
                                                                    format_time(last_used)
                                                                } else {
                                                                    i18n.t("accounts.no_usage_record").to_string()
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "accounts-actions",
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: ButtonSize::Small,
                                                        onclick: {
                                                            let id = acc.id.clone();
                                                            let name = acc.name.clone();
                                                            let provider = acc.provider.clone();
                                                            let active = acc.is_active;
                                                            let visibility = acc.visibility.clone();
                                                            let tenant_id = acc.tenant_id.clone();
                                                            move |_| {
                                                                edit_id.set(id.clone());
                                                                edit_name.set(name.clone());
                                                                edit_provider.set(provider.clone());
                                                                edit_api_key.set(String::new());
                                                                edit_api_base.set(String::new());
                                                                edit_reset_api_base.set(false);
                                                                edit_is_active.set(active);
                                                                edit_visibility.set(visibility.clone());
                                                                edit_tenant_id.set(tenant_id.clone());
                                                                *edit_error.write() = String::new();
                                                                show_edit.set(true);
                                                            }
                                                        },
                                                        {i18n.t("form.edit")}
                                                    }
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: ButtonSize::Small,
                                                        onclick: {
                                                            let id = acc.id.clone();
                                                            move |_| {
                                                                let token = auth_store.token().unwrap_or_default();
                                                                let id = id.clone();
                                                                spawn(async move {
                                                                    match account_service::test(&id, &token).await {
                                                                        Ok(response) => {
                                                                            match account_test_outcome(response) {
                                                                                AccountTestOutcome::Success => {
                                                                                    ui_store.show_success(i18n.t("accounts.test_success"));
                                                                                }
                                                                                AccountTestOutcome::Failure => {
                                                                                    ui_store.show_error(i18n.t("accounts.test_failed"));
                                                                                }
                                                                            }
                                                                        }
                                                                        Err(e) => {
                                                                            ui_store
                                                                                .show_error(
                                                                                    format!("{}: {}", i18n.t("accounts.test_failed"), e),
                                                                                )
                                                                        }
                                                                    }
                                                                    accounts.restart();
                                                                });
                                                            }
                                                        },
                                                        {i18n.t("accounts.test")}
                                                    }
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: ButtonSize::Small,
                                                        onclick: {
                                                            let id = acc.id.clone();
                                                            move |_| {
                                                                let token = auth_store.token().unwrap_or_default();
                                                                let id = id.clone();
                                                                spawn(async move {
                                                                    match account_service::refresh(&id, &token).await {
                                                                        Ok(_) => ui_store.show_success(i18n.t("accounts.refresh_success")),
                                                                        Err(e) => {
                                                                            ui_store
                                                                                .show_error(
                                                                                    format!("{}: {}", i18n.t("accounts.refresh_failed"), e),
                                                                                )
                                                                        }
                                                                    }
                                                                    accounts.restart();
                                                                });
                                                            }
                                                        },
                                                        {i18n.t("common.refresh")}
                                                    }
                                                    Button {
                                                        variant: ButtonVariant::Danger,
                                                        size: ButtonSize::Small,
                                                        onclick: {
                                                            let id = acc.id.clone();
                                                            let name = acc.name.clone();
                                                            move |_| {
                                                                delete_id.set(id.clone());
                                                                delete_name.set(name.clone());
                                                                show_delete.set(true);
                                                            }
                                                        },
                                                        {i18n.t("form.delete")}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "pagination",
                            span { class: "pagination-info",
                                "{i18n.t(\"common.total_items\")} {total} {i18n.t(\"pricing.items_suffix\")}"
                            }
                            Pagination {
                                current: page(),
                                total_pages,
                                on_page_change: move |p| page.set(p),
                            }
                        }
                    }
                }
            }

            // 新增渠道弹窗
            if show_create() {
                div {
                    class: "modal-backdrop",
                    onclick: move |_| *show_create.write() = false,
                    div {
                        class: "modal",
                        onclick: move |e| e.stop_propagation(),
                        div { class: "modal-header",
                            h2 { class: "modal-title", {i18n.t("accounts.create_title")} }
                            button {
                                class: "btn btn-ghost btn-sm",
                                r#type: "button",
                                onclick: move |_| *show_create.write() = false,
                                "✕"
                            }
                        }
                        div { class: "modal-body",
                            if !error_msg().is_empty() {
                                div { class: "alert alert-error",
                                    span { "{error_msg}" }
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.channel_name")} }
                                input {
                                    class: "input-field",
                                    placeholder: "{i18n.t(\"accounts.channel_name_placeholder\")}",
                                    value: "{create_name}",
                                    oninput: move |e| *create_name.write() = e.value(),
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.provider")} }
                                select {
                                    class: "input-field",
                                    value: "{create_preset}",
                                    onchange: move |e| {
                                        let preset_id = e.value();
                                        // 选择预设时自动填充协议与 Base URL 模板（模型由用户手动填写）
                                        if let Some((_, _, protocol, base_url)) = preset_by_id(&preset_id) {
                                            *create_provider.write() = protocol.to_string();
                                            *create_api_base.write() = base_url.to_string();
                                        }
                                        *create_preset.write() = preset_id;
                                    },
                                    for (value , label_key , _ , _) in PRESETS {
                                        option {
                                            value: "{value}",
                                            selected: *value == create_preset(),
                                            "{i18n.t(label_key)}"
                                        }
                                    }
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.supported_models")} }
                                input {
                                    class: "input-field",
                                    placeholder: "{i18n.t(models_placeholder_key_for(&create_preset()))}",
                                    value: "{create_models_input}",
                                    oninput: move |e| *create_models_input.write() = e.value(),
                                }
                                small { class: "form-hint", {i18n.t("accounts.models_hint")} }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.api_key")} }
                                input {
                                    class: "input-field",
                                    r#type: "password",
                                    placeholder: "sk-...",
                                    value: "{create_api_key}",
                                    oninput: move |e| *create_api_key.write() = e.value(),
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.custom_base_url")} }
                                input {
                                    class: "input-field",
                                    placeholder: "{base_url_placeholder_for(&create_preset())}",
                                    value: "{create_api_base}",
                                    oninput: move |e| *create_api_base.write() = e.value(),
                                }
                            }
                        }
                        div { class: "modal-footer",
                            Button {
                                variant: ButtonVariant::Ghost,
                                onclick: move |_| *show_create.write() = false,
                                {i18n.t("form.cancel")}
                            }
                            Button {
                                variant: ButtonVariant::Primary,
                                loading: saving(),
                                onclick: on_submit,
                                "{create_save_label}"
                            }
                        }
                    }
                }
            }
            // 编辑渠道弹窗
            if show_edit() {
                div {
                    class: "modal-backdrop",
                    onclick: move |_| show_edit.set(false),
                    div {
                        class: "modal",
                        onclick: move |e| e.stop_propagation(),
                        div { class: "modal-header",
                            h2 { class: "modal-title", {i18n.t("accounts.edit_title")} }
                            button {
                                class: "btn btn-ghost btn-sm",
                                r#type: "button",
                                onclick: move |_| show_edit.set(false),
                                "✕"
                            }
                        }
                        div { class: "modal-body",
                            if !edit_error().is_empty() {
                                div { class: "alert alert-error",
                                    span { "{edit_error}" }
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.channel_name")} }
                                input {
                                    class: "input-field",
                                    value: "{edit_name}",
                                    oninput: move |e| *edit_name.write() = e.value(),
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.new_api_key")} }
                                input {
                                    class: "input-field",
                                    r#type: "password",
                                    placeholder: "{i18n.t(\"accounts.new_api_key_placeholder\")}",
                                    value: "{edit_api_key}",
                                    oninput: move |e| *edit_api_key.write() = e.value(),
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label",
                                    {i18n.t("accounts.custom_base_url_optional")}
                                }
                                input {
                                    class: "input-field",
                                    placeholder: "{protocol_default_endpoint(&edit_provider())}",
                                    disabled: edit_reset_api_base(),
                                    value: "{edit_api_base}",
                                    oninput: move |e| *edit_api_base.write() = e.value(),
                                }
                                label { class: "form-label",
                                    input {
                                        r#type: "checkbox",
                                        checked: edit_reset_api_base(),
                                        onchange: move |e| edit_reset_api_base.set(e.checked()),
                                        style: "margin-right:6px",
                                    }
                                    {i18n.t("accounts.reset_base_url")}
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("accounts.tenant_id_label")} }
                                {
                                    let tenant_list = tenants().and_then(|r| r.ok()).unwrap_or_default();
                                    let tenant_id = edit_tenant_id();
                                    let selected_label = if tenant_id.is_empty() {
                                        i18n.t("accounts.tenant_id_keep").to_string()
                                    } else {
                                        tenant_list
                                            .iter()
                                            .find(|t| t.id == tenant_id)
                                            .map(|t| format!("{} ({})", t.name, &t.id[..t.id.len().min(8)]))
                                            .unwrap_or_else(|| tenant_id.clone())
                                    };
                                    rsx! {
                                        div { class: "custom-select",
                                            div {
                                                class: "custom-select-trigger",
                                                onclick: move |_| show_tenant_dropdown.set(!show_tenant_dropdown()),
                                                span { "{selected_label}" }
                                                span { class: "custom-select-arrow", "▼" }
                                            }
                                            if show_tenant_dropdown() {
                                                div { class: "custom-select-dropdown",
                                                    div {
                                                        class: "custom-select-option",
                                                        onclick: move |_| {
                                                            edit_tenant_id.set(String::new());
                                                            show_tenant_dropdown.set(false);
                                                        },
                                                        {i18n.t("accounts.tenant_id_keep")}
                                                    }
                                                    for t in &tenant_list {
                                                        div {
                                                            class: "custom-select-option",
                                                            onclick: {
                                                                let id = t.id.clone();
                                                                move |_| {
                                                                    edit_tenant_id.set(id.clone());
                                                                    show_tenant_dropdown.set(false);
                                                                }
                                                            },
                                                            "{t.name} ({t.id.chars().take(8).collect::<String>()}...)"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                small { class: "form-hint", {i18n.t("accounts.tenant_id_hint")} }
                            }
                            div { class: "form-group",
                                label { class: "form-label",
                                    input {
                                        r#type: "checkbox",
                                        checked: edit_is_active(),
                                        onchange: move |e| edit_is_active.set(e.checked()),
                                        style: "margin-right:6px",
                                    }
                                    {i18n.t("accounts.enable_channel")}
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label",
                                    input {
                                        r#type: "checkbox",
                                        checked: edit_visibility() == "global",
                                        onchange: move |e| {
                                            if e.checked() {
                                                edit_visibility.set("global".to_string());
                                            } else {
                                                edit_visibility.set("tenant".to_string());
                                            }
                                        },
                                        style: "margin-right:6px",
                                    }
                                    {i18n.t("accounts.global_visibility")}
                                }
                                small { class: "form-hint", {i18n.t("accounts.global_visibility_hint")} }
                            }
                        }
                        div { class: "modal-footer",
                            Button {
                                variant: ButtonVariant::Ghost,
                                onclick: move |_| show_edit.set(false),
                                {i18n.t("form.cancel")}
                            }
                            Button {
                                variant: ButtonVariant::Primary,
                                loading: edit_saving(),
                                onclick: on_edit_save,
                                "{edit_save_label}"
                            }
                        }
                    }
                }
            }

            // 删除确认弹窗
            if show_delete() {
                div {
                    class: "modal-backdrop",
                    onclick: move |_| show_delete.set(false),
                    div {
                        class: "modal",
                        onclick: move |e| e.stop_propagation(),
                        div { class: "modal-header",
                            h2 { class: "modal-title", {i18n.t("accounts.delete_confirm_title")} }
                            button {
                                class: "btn btn-ghost btn-sm",
                                r#type: "button",
                                onclick: move |_| show_delete.set(false),
                                "✕"
                            }
                        }
                        div { class: "modal-body",
                            p {
                                "{i18n.t(\"accounts.delete_confirm_prefix\")}"
                                strong { "{delete_name}" }
                                "{i18n.t(\"accounts.delete_confirm_suffix\")}"
                            }
                        }
                        div { class: "modal-footer",
                            Button {
                                variant: ButtonVariant::Ghost,
                                onclick: move |_| show_delete.set(false),
                                {i18n.t("form.cancel")}
                            }
                            Button {
                                variant: ButtonVariant::Danger,
                                loading: deleting(),
                                onclick: move |_| {
                                    let id = delete_id();
                                    let token = auth_store.token().unwrap_or_default();
                                    deleting.set(true);
                                    spawn(async move {
                                        match account_service::delete(&id, &token).await {
                                            Ok(_) => {
                                                ui_store.show_success(i18n.t("accounts.deleted"));
                                                accounts.restart();
                                            }
                                            Err(e) => {
                                                ui_store
                                                    .show_error(
                                                        format!("{}: {}", i18n.t("accounts.delete_failed"), e),
                                                    );
                                            }
                                        }
                                        deleting.set(false);
                                        show_delete.set(false);
                                    });
                                },
                                "{delete_confirm_label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── 无权限视图（共用组件）──────────────────────────────────────────────

#[component]
pub fn NoPermissionView(resource: String) -> Element {
    let i18n = use_i18n();
    let no_permission_desc = i18n
        .t("accounts.no_permission_desc")
        .replace("{resource}", &resource);
    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "{resource}" }
        }
        div { class: "empty-state",
            div { class: "empty-icon", "🔒" }
            h3 { class: "empty-title", {i18n.t("accounts.no_permission_title")} }
            p { class: "empty-description", "{no_permission_desc}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(success: bool, message: &str) -> AccountTestResponse {
        AccountTestResponse {
            success,
            message: message.to_string(),
            latency_ms: None,
        }
    }

    #[test]
    fn account_test_success_uses_success_feedback() {
        assert_eq!(
            account_test_outcome(response(true, "Account connection test passed")),
            AccountTestOutcome::Success
        );
    }

    #[test]
    fn account_test_failure_uses_localized_failure_feedback_even_when_http_request_succeeds() {
        assert_eq!(
            account_test_outcome(response(false, "Account connection test failed")),
            AccountTestOutcome::Failure
        );
    }

    #[test]
    fn base_url_placeholder_follows_preset_template_or_protocol_default() {
        // 有模板的预设直接显示模板 URL
        assert_eq!(
            base_url_placeholder_for("deepseek"),
            "https://api.deepseek.com/v1"
        );
        // 模板为空的预设（openai / anthropic）显示协议默认端点
        assert_eq!(
            base_url_placeholder_for("openai"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            base_url_placeholder_for("anthropic"),
            "https://api.anthropic.com/v1"
        );
        // 未知预设回落到 OpenAI 默认端点
        assert_eq!(
            base_url_placeholder_for("unknown"),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn protocol_default_endpoint_maps_known_protocols() {
        // 编辑表单按账号协议取默认端点（与后端 ProtocolType::default_endpoint 一致）
        assert_eq!(
            protocol_default_endpoint("openai"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            protocol_default_endpoint("anthropic"),
            "https://api.anthropic.com/v1"
        );
        // 未知协议回落到 OpenAI 默认端点
        assert_eq!(
            protocol_default_endpoint("unknown"),
            "https://api.openai.com/v1"
        );
    }
}
