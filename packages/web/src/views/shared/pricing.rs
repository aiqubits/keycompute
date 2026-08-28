use client_api::api::admin::CreatePricingRequest;
use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, ConfirmModal, PageHeader, Pagination, Table, TableHead};

const PAGE_SIZE: usize = 20;

use crate::hooks::use_i18n::use_i18n;
use crate::i18n::I18n;
use crate::services::{
    api_client::with_auto_refresh,
    pricing_service::{self, is_global_default},
    tenant_service,
};
use crate::stores::auth_store::AuthStore;
use crate::stores::user_store::UserStore;
use crate::utils::time::format_time;

fn pricing_provider_label<'a>(dimension: &'a str, i18n: &I18n) -> &'a str {
    match dimension {
        "provideraccount" => i18n.t("pricing.label_provider_account"),
        "node" => i18n.t("pricing.label_node"),
        _ => dimension,
    }
}

/// 获取计费维度的 CSS 类名（短格式）
fn pricing_provider_class(dimension: &str) -> &'static str {
    match dimension {
        "provideraccount" => "provider-pa",
        "node" => "provider-node",
        _ => "provider-unknown",
    }
}

fn pricing_col_count(is_admin: bool) -> u32 {
    if is_admin { 7 } else { 6 }
}

/// 定价管理页面
///
/// - 普通用户：只读查看定价策略列表
/// - Admin：完整 CRUD（创建/删除/设置默认）
#[component]
pub fn Pricing() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    // 控制创建弹窗
    let mut show_create = use_signal(|| false);
    let mut editing_pricing = use_signal(|| None as Option<client_api::api::admin::PricingInfo>);
    let mut delete_candidate = use_signal(|| Option::<(String, String)>::None);
    let mut delete_modal_open = use_signal(|| false);
    // 操作结果提示
    let mut op_ok = use_signal(String::new);
    let mut op_err = use_signal(String::new);
    // 刷新触发器
    let mut refresh_tick = use_signal(|| 0u32);
    // 分页
    let mut page = use_signal(|| 1u32);

    let pricing_list = use_resource(move || async move {
        let _tick = refresh_tick();
        with_auto_refresh(auth_store, |token| async move {
            pricing_service::list(&token).await
        })
        .await
    });
    let page_description = if is_admin {
        i18n.t("pricing.admin_desc")
    } else {
        i18n.t("pricing.user_desc")
    };

    let col_count = pricing_col_count(is_admin);

    rsx! {
        div { class: "page-container",
            PageHeader {
                title: i18n.t("page.pricing").to_string(),
                description: page_description.to_string(),
                actions: rsx! {
                    if is_admin {
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| show_create.set(true),
                            {i18n.t("pricing.create")}
                        }
                    }
                },
            }

            // 操作结果提示
            if !op_ok().is_empty() {
                div { class: "alert alert-success", "{op_ok}" }
            }
            if !op_err().is_empty() {
                div { class: "alert alert-error", "{op_err}" }
            }

            {
                let (is_empty, empty_text) = match pricing_list() {
                    None => (true, i18n.t("table.loading")),
                    Some(Err(_)) => (true, i18n.t("common.load_failed")),
                    Some(Ok(ref l)) if l.is_empty() => (true, i18n.t("pricing.empty")),
                    _ => (false, ""),
                };
                let total = pricing_list().and_then(|r| r.ok()).map(|l| l.len()).unwrap_or(0);
                let total_pages = total.div_ceil(PAGE_SIZE).max(1) as u32;
                let start = (page() as usize - 1) * PAGE_SIZE;
                let paged_list: Vec<_> = pricing_list()
                    .and_then(|r| r.ok())
                    .map(|l| l.into_iter().skip(start).take(PAGE_SIZE).collect())
                    .unwrap_or_default();
                rsx! {
                    div { class: "pricing-table-shell",
                        div { class: "pricing-table-intro",
                            div {
                                h2 { class: "pricing-table-title", {i18n.t("pricing.table_title")} }
                                p { class: "pricing-table-subtitle", {i18n.t("pricing.table_subtitle")} }
                            }
                            div { class: "pricing-table-meta",
                                "{i18n.t(\"common.total_items\")} {total} {i18n.t(\"pricing.items_suffix\")}"
                            }
                        }
                        Table {
                            class: "pricing-table".to_string(),
                            empty: is_empty,
                            empty_text: empty_text.to_string(),
                            col_count,
                            thead {
                                tr {
                                    TableHead { {i18n.t("pricing.model_provider")} }
                                    TableHead { {i18n.t("pricing.tenant_id")} }
                                    TableHead { {i18n.t("pricing.input_price")} }
                                    TableHead { {i18n.t("pricing.output_price")} }
                                    TableHead { {i18n.t("pricing.billing_status")} }
                                    TableHead { {i18n.t("common.time")} }
                                    if is_admin {
                                        TableHead { {i18n.t("table.actions")} }
                                    }
                                }
                            }
                            tbody { // 显示租户 ID：nil UUID 表示全局默认 // 显示租户 ID：nil UUID 表示全局默认

                                if pricing_list().and_then(|r| r.ok()).is_some() {
                                    for p in paged_list.iter() {
                                        tr { key: "{p.id}",
                                            td {
                                                div { class: "pricing-model-cell",
                                                    div { class: "pricing-model-row",
                                                        span { class: "pricing-model-name", "{p.model_name}" }
                                                        span { class: "pricing-model-id",
                                                            "#{p.id.chars().take(8).collect::<String>()}"
                                                        }
                                                    }
                                                    div { class: "pricing-provider-row",
                                                        span { class: "pricing-provider-badge {pricing_provider_class(&p.billing_dimension)}",
                                                            "{pricing_provider_label(&p.billing_dimension, &i18n)}"
                                                        }
                                                        span { class: "pricing-provider-code", "{p.billing_dimension}" }
                                                    }
                                                }
                                            }
                                            td {
                                                // 显示租户 ID：nil UUID 表示全局默认
                                                if let Some(tenant_id) = &p.tenant_id {
                                                    if is_global_default(&p.tenant_id) {
                                                        span { class: "pricing-tenant-global", {i18n.t("pricing.global")} }
                                                    } else {
                                                        span { class: "pricing-tenant-code",
                                                            "{tenant_id.chars().take(8).collect::<String>()}"
                                                        }
                                                    }
                                                } else {
                                                    span { class: "pricing-tenant-code", "-" }
                                                }
                                            }
                                            td {
                                                div { class: "pricing-amount-cell",
                                                    div { class: "pricing-amount-value", "{p.input_price_per_1k}" }
                                                    div { class: "pricing-amount-meta",
                                                        "{p.currency} / 1K {i18n.t(\"pricing.input_tokens\")}"
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "pricing-amount-cell",
                                                    div { class: "pricing-amount-value", "{p.output_price_per_1k}" }
                                                    div { class: "pricing-amount-meta",
                                                        "{p.currency} / 1K {i18n.t(\"pricing.output_tokens\")}"
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "pricing-status-cell", // 全局默认定价不允许删除，仅保留编辑按钮
                                                    if p.is_default {
                                                        Badge { variant: BadgeVariant::Success, {i18n.t("pricing.default")} }
                                                    } else {
                                                        Badge { variant: BadgeVariant::Neutral,
                                                            {i18n.t("pricing.alternative")}
                                                        }
                                                    }
                                                    p { class: "pricing-status-note",
                                                        if p.is_default {
                                                            {i18n.t("pricing.default_note")}
                                                        } else {
                                                            {i18n.t("pricing.alternative_note")}
                                                        }
                                                    }
                                                }
                                            }
                                            td {
                                                div { class: "pricing-time-cell",
                                                    span { class: "pricing-time-label",
                                                        {i18n.t("common.created_at_label")}
                                                    }
                                                    span { class: "pricing-time-value", {format_time(&p.created_at)} }
                                                }
                                            }
                                            if is_admin {
                                                td {
                                                    div { class: "action-buttons pricing-actions",
                                                        if !p.is_default {
                                                            {
                                                                let pid = p.id.clone();
                                                                rsx! {
                                                                    button {
                                                                        class: "btn btn-sm btn-secondary",
                                                                        onclick: move |_| {
                                                                            let id = pid.clone();
                                                                            let token = auth_store.token().unwrap_or_default();
                                                                            spawn(async move {
                                                                                match pricing_service::make_default(&id, &token).await {
                                                                                    Ok(_) => {
                                                                                        op_ok.set(i18n.t("pricing.set_default_ok").to_string());
                                                                                        op_err.set(String::new());
                                                                                        *refresh_tick.write() += 1;
                                                                                        spawn(async move {
                                                                                            gloo_timers::future::TimeoutFuture::new(3_000).await;
                                                                                            op_ok.set(String::new());
                                                                                        });
                                                                                    }
                                                                                    Err(e) => {
                                                                                        op_err
                                                                                            .set(format!("{}：{e}", i18n.t("pricing.set_default_failed")));
                                                                                        spawn(async move {
                                                                                            gloo_timers::future::TimeoutFuture::new(3_000).await;
                                                                                            op_err.set(String::new());
                                                                                        });
                                                                                    }
                                                                                }
                                                                            });
                                                                        },
                                                                        {i18n.t("pricing.set_default")}
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        // 编辑按钮：所有定价都可以编辑
                                                        {
                                                            let pricing = p.clone();
                                                            rsx! {
                                                                button {
                                                                    class: "btn btn-sm btn-secondary",
                                                                    onclick: move |_| editing_pricing.set(Some(pricing.clone())),
                                                                    {i18n.t("form.edit")}
                                                                }
                                                            }
                                                        }
                                                        // 全局默认定价不允许删除，仅保留编辑按钮
                                                        if !is_global_default(&p.tenant_id) {
                                                            {
                                                                let pid = p.id.clone();
                                                                let model_name = p.model_name.clone();
                                                                rsx! {
                                                                    button {
                                                                        class: "btn btn-sm btn-danger",
                                                                        onclick: move |_| {
                                                                            delete_candidate.set(Some((pid.clone(), model_name.clone())));
                                                                            delete_modal_open.set(true);
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
                            previous_label: i18n.t("table.previous").to_string(),
                            next_label: i18n.t("table.next").to_string(),
                            on_page_change: move |p| page.set(p),
                        }
                    }
                }
            }

            // 创建定价弹窗
            if show_create() {
                CreatePricingModal {
                    auth_store,
                    on_close: move |_| show_create.set(false),
                    on_created: move |_| {
                        show_create.set(false);
                        op_ok.set(i18n.t("pricing.created").to_string());
                        op_err.set(String::new());
                        page.set(1);
                        *refresh_tick.write() += 1;
                        spawn(async move {
                            gloo_timers::future::TimeoutFuture::new(3_000).await;
                            op_ok.set(String::new());
                        });
                    },
                }
            }

            if let Some(pricing) = editing_pricing() {
                EditPricingModal {
                    auth_store,
                    pricing_id: pricing.id.clone(),
                    pricing_model: pricing.model_name.clone(),
                    pricing_provider: pricing.billing_dimension.clone(),
                    pricing_currency: pricing.currency.clone(),
                    initial_input_price: pricing.input_price_per_1k.clone(),
                    initial_output_price: pricing.output_price_per_1k.clone(),
                    on_close: move |_| editing_pricing.set(None),
                    on_updated: move |_| {
                        editing_pricing.set(None);
                        op_ok.set(i18n.t("pricing.updated").to_string());
                        op_err.set(String::new());
                        *refresh_tick.write() += 1;
                        spawn(async move {
                            gloo_timers::future::TimeoutFuture::new(3_000).await;
                            op_ok.set(String::new());
                        });
                    },
                }
            }

            ConfirmModal {
                open: delete_modal_open,
                title: i18n.t("pricing.delete_confirm_title").to_string(),
                message: delete_candidate()
                    .as_ref()
                    .map(|(_, model)| i18n.t_with_args("pricing.delete_confirm_message", &[("model", model)]))
                    .unwrap_or_default(),
                confirm_text: i18n.t("form.delete").to_string(),
                cancel_text: i18n.t("form.cancel").to_string(),
                close_label: i18n.t("common.close").to_string(),
                danger: true,
                onconfirm: move |_| {
                    let candidate = delete_candidate();
                    delete_candidate.set(None);
                    delete_modal_open.set(false);
                    if let Some((id, _)) = candidate {
                        let token = auth_store.token().unwrap_or_default();
                        spawn(async move {
                            match pricing_service::delete(&id, &token).await {
                                Ok(_) => {
                                    op_ok.set(i18n.t("pricing.deleted").to_string());
                                    op_err.set(String::new());
                                    *refresh_tick.write() += 1;
                                    spawn(async move {
                                        gloo_timers::future::TimeoutFuture::new(3_000).await;
                                        op_ok.set(String::new());
                                    });
                                }
                                Err(e) => {
                                    op_err.set(format!("{}：{e}", i18n.t("pricing.delete_failed")));
                                }
                            }
                        });
                    }
                },
                oncancel: move |_| {
                    delete_candidate.set(None);
                    delete_modal_open.set(false);
                },
            }
        }
    }
}

/// 创建定价弹窗
#[component]
fn CreatePricingModal(
    auth_store: AuthStore,
    on_close: EventHandler,
    on_created: EventHandler,
) -> Element {
    let i18n = use_i18n();
    let mut model = use_signal(String::new);
    let mut provider = use_signal(|| "provideraccount".to_string());
    let mut tenant_id = use_signal(|| None::<String>);
    let mut input_price = use_signal(String::new);
    let mut output_price = use_signal(String::new);
    let mut currency = use_signal(|| "CNY".to_string());
    let mut saving = use_signal(|| false);
    let mut form_err = use_signal(String::new);

    // 获取租户列表
    let tenant_list = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            tenant_service::list(None, &token).await
        })
        .await
    });

    let on_submit = move |_| {
        let mut m = model();
        let p = provider();
        let ip_str = input_price();
        let op_str = output_price();
        let cur = currency();
        let tid = tenant_id();
        if m.is_empty() || p.is_empty() || ip_str.is_empty() || op_str.is_empty() {
            form_err.set(i18n.t("pricing.fill_all").to_string());
            return;
        }
        // 如果选择 node 计费维度，自动补全 node: 前缀
        if p == "node" && !m.starts_with("node:") {
            m = format!("node:{}", m);
        }
        // tenant_id 保持原样（None 表示未选择，Some(id) 表示选择了具体租户）
        // 注意：由于已移除“全局默认”选项，用户无法创建全局默认定价
        let tid = tid.filter(|id| !id.is_empty());
        if ip_str.parse::<f64>().is_err() {
            form_err.set(i18n.t("pricing.invalid_input_price").to_string());
            return;
        }
        if op_str.parse::<f64>().is_err() {
            form_err.set(i18n.t("pricing.invalid_output_price").to_string());
            return;
        }
        if ip_str.parse::<f64>().ok().is_some_and(|v| v < 0.0) {
            form_err.set(i18n.t("pricing.negative_input_price").to_string());
            return;
        }
        if op_str.parse::<f64>().ok().is_some_and(|v| v < 0.0) {
            form_err.set(i18n.t("pricing.negative_output_price").to_string());
            return;
        }
        saving.set(true);
        form_err.set(String::new());
        let token = auth_store.token().unwrap_or_default();
        spawn(async move {
            let req = CreatePricingRequest {
                model_name: m,
                billing_dimension: p,
                tenant_id: tid,
                input_price_per_1k: ip_str,
                output_price_per_1k: op_str,
                currency: cur,
                is_default: false,
                effective_from: None,
                effective_until: None,
            };
            match pricing_service::create(req, &token).await {
                Ok(_) => {
                    saving.set(false);
                    on_created.call(());
                }
                Err(e) => {
                    form_err.set(format!("{}：{e}", i18n.t("pricing.create_failed")));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "modal-backdrop", onclick: move |_| on_close.call(()),
            div { class: "modal", role: "dialog", aria_modal: "true", aria_label: i18n.t("pricing.create_title"), onclick: move |e| e.stop_propagation(),
                div { class: "modal-header",
                    h2 { class: "modal-title", {i18n.t("pricing.create_title")} }
                    button {
                        class: "btn btn-ghost btn-sm",
                        r#type: "button",
                        aria_label: i18n.t("common.close"),
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                div { class: "modal-body",
                    if !form_err().is_empty() {
                        div { class: "alert alert-error", "{form_err}" }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.model_name")} }
                        input {
                            class: "input-field",
                            r#type: "text",
                            placeholder: "{i18n.t(\"pricing.model_placeholder\")}",
                            value: "{model}",
                            oninput: move |e| model.set(e.value()),
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.provider_type")} }
                        select {
                            class: "input-field",
                            value: "{provider}",
                            onchange: move |e| provider.set(e.value()),
                            option { value: "provideraccount",
                                {i18n.t("pricing.label_provider_account")}
                            }
                            option { value: "node", {i18n.t("pricing.label_node")} }
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.tenant_id")} }
                        select {
                            class: "input-field",
                            value: "{tenant_id().unwrap_or_default()}",
                            onchange: move |e| {
                                let val = e.value();
                                if val.is_empty() {
                                    tenant_id.set(None);
                                } else {
                                    tenant_id.set(Some(val));
                                }
                            },
                            // 移除“全局默认”选项，全局默认定价由系统自动管理，不允许手动创建
                            for t in match tenant_list() {
                                Some(Ok(list)) => list,
                                _ => vec![],
                            }
                            {
                                option { value: "{t.id}",
                                    "{t.name} ({t.id.chars().take(8).collect::<String>()})"
                                }
                            }
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.input_price_label")} }
                        input {
                            class: "input-field",
                            r#type: "number",
                            placeholder: "{i18n.t(\"pricing.input_placeholder\")}",
                            step: "0.000001",
                            value: "{input_price}",
                            oninput: move |e| input_price.set(e.value()),
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.output_price_label")} }
                        input {
                            class: "input-field",
                            r#type: "number",
                            placeholder: "{i18n.t(\"pricing.output_placeholder\")}",
                            step: "0.000001",
                            value: "{output_price}",
                            oninput: move |e| output_price.set(e.value()),
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("common.currency")} }
                        select {
                            class: "input-field",
                            value: "{currency}",
                            onchange: move |e| currency.set(e.value()),
                            option { value: "CNY", {i18n.t("pricing.currency_cny")} }
                            option { value: "USD", {i18n.t("pricing.currency_usd")} }
                        }
                    }
                }
                div { class: "modal-footer",
                    button {
                        class: "btn btn-ghost",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {i18n.t("form.cancel")}
                    }
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        disabled: saving(),
                        onclick: on_submit,
                        if saving() {
                            {i18n.t("pricing.creating")}
                        } else {
                            {i18n.t("form.create")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn EditPricingModal(
    auth_store: AuthStore,
    pricing_id: String,
    pricing_model: String,
    pricing_provider: String,
    pricing_currency: String,
    initial_input_price: String,
    initial_output_price: String,
    on_close: EventHandler,
    on_updated: EventHandler,
) -> Element {
    let i18n = use_i18n();
    let mut input_price = use_signal(|| initial_input_price.clone());
    let mut output_price = use_signal(|| initial_output_price.clone());
    let mut saving = use_signal(|| false);
    let mut form_err = use_signal(String::new);

    let on_submit = move |_| {
        let ip_str = input_price();
        let op_str = output_price();

        if ip_str.is_empty() || op_str.is_empty() {
            form_err.set(i18n.t("pricing.fill_all").to_string());
            return;
        }
        if ip_str.parse::<f64>().is_err() {
            form_err.set(i18n.t("pricing.invalid_input_price").to_string());
            return;
        }
        if op_str.parse::<f64>().is_err() {
            form_err.set(i18n.t("pricing.invalid_output_price").to_string());
            return;
        }
        if ip_str.parse::<f64>().ok().is_some_and(|v| v < 0.0) {
            form_err.set(i18n.t("pricing.negative_input_price").to_string());
            return;
        }
        if op_str.parse::<f64>().ok().is_some_and(|v| v < 0.0) {
            form_err.set(i18n.t("pricing.negative_output_price").to_string());
            return;
        }

        saving.set(true);
        form_err.set(String::new());
        let token = auth_store.token().unwrap_or_default();
        let id = pricing_id.clone();
        spawn(async move {
            let req = client_api::api::admin::UpdatePricingRequest::new()
                .with_input_price_per_1k(ip_str)
                .with_output_price_per_1k(op_str);
            match pricing_service::update(&id, req, &token).await {
                Ok(_) => {
                    saving.set(false);
                    on_updated.call(());
                }
                Err(e) => {
                    form_err.set(format!("{}：{e}", i18n.t("pricing.update_failed")));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "modal-backdrop", onclick: move |_| on_close.call(()),
            div { class: "modal", role: "dialog", aria_modal: "true", aria_label: i18n.t("pricing.edit_title"), onclick: move |e| e.stop_propagation(),
                div { class: "modal-header",
                    h2 { class: "modal-title", {i18n.t("pricing.edit_title")} }
                    button {
                        class: "btn btn-ghost btn-sm",
                        r#type: "button",
                        aria_label: i18n.t("common.close"),
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                div { class: "modal-body",
                    if !form_err().is_empty() {
                        div { class: "alert alert-error", "{form_err}" }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.model_name")} }
                        input {
                            class: "input-field",
                            r#type: "text",
                            value: "{pricing_model}",
                            disabled: true,
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.provider_type")} }
                        input {
                            class: "input-field",
                            r#type: "text",
                            value: "{pricing_provider}",
                            disabled: true,
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("common.currency")} }
                        input {
                            class: "input-field",
                            r#type: "text",
                            value: "{pricing_currency}",
                            disabled: true,
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.input_price_label")} }
                        input {
                            class: "input-field",
                            r#type: "number",
                            step: "0.000001",
                            value: "{input_price}",
                            oninput: move |e| input_price.set(e.value()),
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", {i18n.t("pricing.output_price_label")} }
                        input {
                            class: "input-field",
                            r#type: "number",
                            step: "0.000001",
                            value: "{output_price}",
                            oninput: move |e| output_price.set(e.value()),
                        }
                    }
                }
                div { class: "modal-footer",
                    button {
                        class: "btn btn-ghost",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {i18n.t("form.cancel")}
                    }
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        disabled: saving(),
                        onclick: on_submit,
                        if saving() {
                            {i18n.t("form.saving")}
                        } else {
                            {i18n.t("form.save_changes")}
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pricing_col_count;

    #[test]
    fn empty_table_colspan_matches_visible_pricing_columns() {
        assert_eq!(pricing_col_count(true), 7);
        assert_eq!(pricing_col_count(false), 6);
    }
}
