use super::examples::{
    DEFAULT_CLAUDE_MODEL, anthropic_examples, openai_examples, pick_claude_model, pick_sample_model,
};
use crate::hooks::use_i18n::use_i18n;
use crate::services::{api_client::with_auto_refresh, api_key_service, model_service};
use crate::stores::auth_store::AuthStore;
use crate::stores::ui_store::UiStore;
use crate::utils::on_copy;
use crate::utils::time::format_time;
use dioxus::prelude::*;
use ui::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Pagination, Table, TableHead,
    icons::{IconCopy, IconPlus},
};

const PAGE_SIZE: usize = 20;

fn model_display_rank(model: &str) -> usize {
    match model {
        "gpt-4-turbo" => 0,
        "gpt-4o-mini" => 1,
        "qwen3-14b" => 2,
        "deepseek-chat" => 3,
        model if model.starts_with("claude-3-5-sonnet") => 4,
        "gpt-4o" => 5,
        "gpt-3.5-turbo" => 6,
        _ => 100,
    }
}

#[component]
pub fn ApiKeyList() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let ui_store = use_context::<UiStore>();
    let mut show_create = use_signal(|| false);
    let mut new_key_name = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let mut create_error = use_signal(|| Option::<String>::None);
    let mut new_key_value = use_signal(|| Option::<String>::None);
    let mut page = use_signal(|| 1u32);
    // 是否显示已撤销的 Key（默认不显示）
    let mut include_revoked = use_signal(|| false);
    // 复制状态
    let mut copied = use_signal(|| false);
    let mut example_tab = use_signal(|| "env".to_string());
    // 示例协议（openai / anthropic），与后端双协议网关对应
    let mut example_protocol = use_signal(|| "openai".to_string());
    let create_failed = i18n.t("api_keys.create_failed");

    // 获取模型列表（用于显示用法示例）
    let models = use_resource(move || async move { model_service::list_models().await.ok() });

    // 拉取 key 列表
    let mut keys = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            api_key_service::list(include_revoked(), &token).await
        })
        .await
    });

    let on_create = move |evt: Event<FormData>| {
        evt.prevent_default();
        let name = new_key_name();
        if name.is_empty() {
            return;
        }
        creating.set(true);
        create_error.set(None);
        spawn(async move {
            let token = auth_store.token().unwrap_or_default();
            match api_key_service::create(&name, &token).await {
                Ok(resp) => {
                    new_key_value.set(Some(resp.api_key));
                    show_create.set(false);
                    new_key_name.set(String::new());
                    creating.set(false);
                    page.set(1);
                    // 重新拉取列表
                    keys.restart();
                }
                Err(e) => {
                    create_error.set(Some(format!("{create_failed}：{e}")));
                    creating.set(false);
                }
            }
        });
    };

    let on_delete = move |id: String| {
        spawn(async move {
            let token = auth_store.token().unwrap_or_default();
            if api_key_service::delete(&id, &token).await.is_ok() {
                keys.restart();
            }
        });
    };

    rsx! {
        div { class: "page-container kc-api-page",
            div { class: "page-header kc-api-header",
                div { class: "kc-api-heading",
                    h1 { class: "page-title", {i18n.t("page.api_keys")} }
                    p { class: "page-subtitle", {i18n.t("api_keys.subtitle")} }
                }
                div { class: "kc-api-actions",
                    Button {
                        variant: ButtonVariant::Primary,
                        onclick: move |_| {
                            show_create.set(true);
                            new_key_value.set(None);
                            example_protocol.set("openai".to_string());
                        },
                        IconPlus { size: 16 }
                        {i18n.t("api_keys.create")}
                    }
                }
            }

            // 筛选工具栏
            div { class: "toolbar kc-api-toolbar",
                div { class: "toolbar-left",
                    div { class: "filter-tabs",
                        button {
                            class: if !include_revoked() { "filter-tab active" } else { "filter-tab" },
                            r#type: "button",
                            onclick: move |_| {
                                include_revoked.set(false);
                                page.set(1);
                                keys.restart();
                            },
                            {i18n.t("api_keys.active")}
                        }
                        button {
                            class: if include_revoked() { "filter-tab active" } else { "filter-tab" },
                            r#type: "button",
                            onclick: move |_| {
                                include_revoked.set(true);
                                page.set(1);
                                keys.restart();
                            },
                            {i18n.t("api_keys.all_with_revoked")}
                        }
                    }
                }
            }

            // 新建成功后展示完整密钥（仅一次）
            if let Some(key) = new_key_value() {
                {
                    // 同域部署时从浏览器地址解析完整 origin，避免示例里只显示相对路径 /v1
                    let api_url = crate::services::api_client::public_openai_api_base_url();
                    // Anthropic SDK 会在 base_url 后自行追加 /v1/messages，示例需用不含 /v1 的根路径
                    let api_root = crate::services::api_client::public_api_root_url();

                    let mut available_models = models()
                        .flatten()
                        .map(|m| {
                            let mut data = m.data;
                            data.sort_by(|a, b| {
                                model_display_rank(&a.id)
                                    .cmp(&model_display_rank(&b.id))
                                    .then_with(|| a.id.cmp(&b.id))
                            });
                            data
                        })
                        .unwrap_or_default();

                    if available_models.is_empty() {
                        available_models = vec![
                            client_api::api::openai::ModelInfo {
                                id: "deepseek-chat".to_string(),
                                object: "model".to_string(),
                                created: 0,
                                owned_by: "deepseek".to_string(),
                            },
                        ];
                    }
                    let sample_model = pick_sample_model(&available_models);
                    let selected_tab = example_tab();
                    let is_anthropic = example_protocol() == "anthropic";
                    let claude_model = pick_claude_model(&available_models);

                    let examples = if is_anthropic {
                        anthropic_examples(
                            &api_root,
                            &key,
                            &claude_model,
                            i18n.t("api_keys.example_env_comment"),
                        )
                    } else {
                        openai_examples(
                            &api_url,
                            &key,
                            &sample_model,
                            i18n.t("api_keys.example_env_comment"),
                        )
                    };

                    let example_text = examples.for_tab(&selected_tab).to_string();
                    // 预先计算 anthropic 提示文案（含模型名参数），避免在 rsx 内嵌多行表达式节点。
                    // 注意：{model} 参数是文案中"列表无 Claude 时回退"的默认模型，与示例实际使用的
                    // claude_model（列表第一个 Claude 模型）语义不同；列表含 Claude 时两者必然不同，
                    // 属预期行为，勿改为传入 claude_model。
                    let anthropic_note = i18n
                        .t_with_args(
                            "api_keys.example_note_anthropic",
                            &[("model", DEFAULT_CLAUDE_MODEL)],
                        );
                    let copied_label = i18n.t("api_keys.copied");
                    let copy_hint = i18n.t("api_keys.copy_hint");
                    let copy_manual_hint = i18n.t("common.copy_manual_hint");
                    rsx! {
                        div { class: "kc-api-success-panel",
                            div { class: "kc-api-success-head",
                                div { class: "kc-api-success-icon", "✓" }
                                div {
                                    h2 { {i18n.t("api_keys.created_title")} }
                                    p { {i18n.t("api_keys.created_once")} }
                                }
                            }

                            div { class: "kc-api-success-grid",
                                section { class: "kc-api-model-panel",
                                    h3 { {i18n.t("api_keys.models_title")} }
                                    p {
                                        {i18n.t("api_keys.models_desc_prefix")}
                                        code { "API_MODEL" }
                                        {i18n.t("api_keys.models_desc_suffix")}
                                    }
                                    div { class: "kc-api-model-list",
                                        for (idx , model) in available_models.iter().take(6).enumerate() {
                                            div { class: "kc-api-model-row",
                                                span { class: "kc-api-model-name", "{model.id}" }
                                                span { class: if idx == 0 { "kc-api-model-badge is-default" } else { "kc-api-model-badge" },
                                                    if idx == 0 {
                                                        {i18n.t("api_keys.default_model")}
                                                    } else {
                                                        "{model.owned_by}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if available_models.len() > 6 {
                                        p { class: "kc-api-model-more",
                                            "+{available_models.len() - 6} {i18n.t(\"api_keys.more_models\")}"
                                        }
                                    }
                                }

                                section { class: "kc-api-example-panel",
                                    h3 { {i18n.t("api_keys.quick_example")} }
                                    p { {i18n.t("api_keys.quick_example_desc")} }

                                    div { class: "kc-api-example-box",
                                        div { class: "kc-api-example-protocols",
                                            for (value , label) in [
                                                ("openai", i18n.t("api_keys.example_protocol_openai")),
                                                ("anthropic", i18n.t("api_keys.example_protocol_anthropic")),
                                            ]
                                            {
                                                button {
                                                    class: if example_protocol() == value { "kc-api-example-protocol active" } else { "kc-api-example-protocol" },
                                                    r#type: "button",
                                                    onclick: move |_| {
                                                        example_protocol.set(value.to_string());
                                                        copied.set(false);
                                                    },
                                                    "{label}"
                                                }
                                            }
                                        }
                                        div { class: "kc-api-example-tabs",
                                            for (value , label) in [
                                                ("env", i18n.t("api_keys.example_env")),
                                                (
                                                    "python",
                                                    if is_anthropic {
                                                        i18n.t("api_keys.example_python_anthropic")
                                                    } else {
                                                        i18n.t("api_keys.example_python")
                                                    },
                                                ),
                                                (
                                                    "node",
                                                    if is_anthropic {
                                                        i18n.t("api_keys.example_node_anthropic")
                                                    } else {
                                                        i18n.t("api_keys.example_node")
                                                    },
                                                ),
                                                ("curl", i18n.t("api_keys.example_curl")),
                                            ]
                                            {
                                                button {
                                                    class: if selected_tab == value { "kc-api-example-tab active" } else { "kc-api-example-tab" },
                                                    r#type: "button",
                                                    onclick: move |_| {
                                                        example_tab.set(value.to_string());
                                                        copied.set(false);
                                                    },
                                                    "{label}"
                                                }
                                            }
                                        }
                                        div { class: "kc-api-copy-block",
                                            pre {
                                                class: if copied() { "kc-api-example copied" } else { "kc-api-example" },
                                                title: if copied() { copied_label } else { copy_hint },
                                                "{example_text}"
                                            }
                                            button {
                                                class: "kc-api-copy-button",
                                                r#type: "button",
                                                onclick: on_copy(example_text.clone(), copy_manual_hint.to_string(), ui_store, copied),
                                                IconCopy { size: 15 }
                                                if copied() {
                                                    {copied_label}
                                                } else {
                                                    {i18n.t("api_keys.copy")}
                                                }
                                            }
                                        }
                                    }
                                    p { class: "kc-api-secret-note",
                                        if is_anthropic {
                                            {anthropic_note}
                                        } else {
                                            {i18n.t("api_keys.example_note_prefix")}
                                            code { "API_MODEL" }
                                            {i18n.t("api_keys.example_note_suffix")}
                                        }
                                    }
                                }
                            }
                            div { class: "kc-api-success-actions",
                                Button {
                                    variant: ButtonVariant::Ghost,
                                    size: ButtonSize::Small,
                                    onclick: move |_| {
                                        new_key_value.set(None);
                                        copied.set(false);
                                    },
                                    {i18n.t("api_keys.close_saved")}
                                }
                            }
                        }
                    }
                }
            }

            // 创建弹窗
            if show_create() {
                div { class: "modal-overlay",
                    div { class: "modal",
                        h2 { class: "modal-title", {i18n.t("api_keys.create_title")} }
                        if let Some(err) = create_error() {
                            div { class: "alert alert-error", "{err}" }
                        }
                        form { onsubmit: on_create,
                            div { class: "form-group",
                                label { class: "form-label", {i18n.t("api_keys.name")} }
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "{i18n.t(\"api_keys.name_placeholder\")}",
                                    value: "{new_key_name}",
                                    oninput: move |e| new_key_name.set(e.value()),
                                }
                            }
                            div { class: "modal-actions",
                                Button {
                                    variant: ButtonVariant::Ghost,
                                    r#type: "button".to_string(),
                                    onclick: move |_| show_create.set(false),
                                    {i18n.t("form.cancel")}
                                }
                                Button {
                                    variant: ButtonVariant::Primary,
                                    r#type: "submit".to_string(),
                                    loading: creating(),
                                    if creating() {
                                        {i18n.t("api_keys.creating")}
                                    } else {
                                        {i18n.t("form.create")}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            match keys() {
                None => rsx! {
                    div { class: "loading-state", {i18n.t("table.loading")} }
                },
                Some(Err(e)) => rsx! {
                    div { class: "alert alert-error", "{i18n.t(\"api_keys.loading_failed\")}：{e}" }
                },
                Some(Ok(list)) => {
                    let total = list.len();
                    let total_pages = total.div_ceil(PAGE_SIZE).max(1) as u32;
                    let start = (page() as usize - 1) * PAGE_SIZE;
                    let paged: Vec<_> = list.iter().skip(start).take(PAGE_SIZE).collect();
                    if paged.is_empty() && total == 0 {
                        rsx! {
                            div { class: "kc-api-table-panel",
                                div { class: "kc-api-table-meta",
                                    div {
                                        span { {i18n.t("api_keys.registry")} }
                                        strong { "0" }
                                    }
                                    p { {i18n.t("api_keys.empty_meta")} }
                                }
                                Table {
                                    class: "kc-api-table".to_string(),
                                    col_count: 5,
                                    empty: true,
                                    empty_text: i18n.t("api_keys.empty").to_string(),
                                    thead {
                                        tr {
                                            TableHead { "" }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! {
                            div { class: "kc-api-table-panel",
                                div { class: "kc-api-table-meta",
                                    div {
                                        span { {i18n.t("api_keys.registry")} }
                                        strong { "{total}" }
                                    }
                                    p {
                                        if include_revoked() {
                                            {i18n.t("api_keys.all_meta")}
                                        } else {
                                            {i18n.t("api_keys.active_meta")}
                                        }
                                    }
                                }
                                Table { class: "kc-api-table".to_string(), col_count: 5,
                                    thead {
                                        tr {
                                            TableHead { {i18n.t("table.name")} }
                                            TableHead { {i18n.t("api_keys.prefix")} }
                                            TableHead { {i18n.t("table.status")} }
                                            TableHead { {i18n.t("table.created_at")} }
                                            TableHead { {i18n.t("table.actions")} }
                                        }
                                    }
                                    tbody {
                                        for key in paged.iter() {
                                            tr { key: "{key.id}",
                                                td { class: "kc-api-key-name", "{key.name}" }
                                                td {
                                                    code { class: "kc-api-key-preview", "{key.key_preview}" }
                                                }
                                                td {
                                                    Badge { variant: if key.revoked() { BadgeVariant::Error } else { BadgeVariant::Success },
                                                        if key.revoked() {
                                                            {i18n.t("api_keys.revoked")}
                                                        } else {
                                                            {i18n.t("api_keys.active")}
                                                        }
                                                    }
                                                }
                                                td { {format_time(&key.created_at)} }
                                                td {
                                                    Button {
                                                        variant: ButtonVariant::Danger,
                                                        size: ButtonSize::Small,
                                                        onclick: {
                                                            let id = key.id.to_string();
                                                            move |_| on_delete(id.clone())
                                                        },
                                                        {i18n.t("form.delete")}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                div { class: "pagination kc-api-pagination",
                                    span { class: "pagination-info", "{i18n.t(\"dashboard.total\")} {total}" }
                                    Pagination {
                                        current: page(),
                                        total_pages,
                                        on_page_change: move |p| page.set(p),
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
