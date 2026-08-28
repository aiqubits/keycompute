use dioxus::prelude::*;
use ui::{Button, ButtonVariant, PageHeader};

use crate::hooks::use_i18n::use_i18n;
use crate::services::{api_client::with_auto_refresh, settings_service};
use crate::stores::{
    auth_store::AuthStore, public_settings_store::PublicSettingsStore, user_store::UserStore,
};

/// 系统设置页面
///
/// - 普通用户：无此页面入口（个人偏好通过导航栏按钮切换，存 localStorage）
/// - Admin：全局系统参数配置（调用 SettingsApi，需 Admin token）
#[component]
pub fn Settings() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let current_user = user_store.info.read().clone();
    let is_admin = current_user.as_ref().map(|u| u.is_admin()).unwrap_or(false);
    let is_system = current_user
        .as_ref()
        .map(|u| u.role == "system")
        .unwrap_or(false);

    let settings = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            settings_service::get_all(&token).await
        })
        .await
    });

    let save_error = use_signal(String::new);
    let save_ok = use_signal(|| false);

    let get_val = move |key: &str| -> String {
        match settings() {
            Some(Ok(ref m)) => m.get(key).map(|v| v.to_string_value()).unwrap_or_default(),
            _ => String::new(),
        }
    };

    let platform_name = get_val("site_name");
    let currency = get_val("default_currency");
    let min_recharge = get_val("min_recharge_amount");
    let max_recharge = get_val("max_recharge_amount");
    let alipay_enabled = get_val("alipay_enabled");
    let wechatpay_enabled = get_val("wechatpay_enabled");
    let default_user_quota = get_val("default_user_quota");
    let jwt_expire = get_val("jwt_expire_hours");
    let distribution_enabled = get_val("distribution_enabled");
    let page_description = if is_admin {
        i18n.t("settings.admin_desc")
    } else {
        i18n.t("settings.user_desc")
    };

    rsx! {
        div { class: "page-container settings-console-page",
            PageHeader {
                title: i18n.t("page.settings").to_string(),
                description: page_description.to_string(),
            }

            if !is_admin {
                div { class: "alert alert-info",
                    span { class: "alert-icon", "ℹ" }
                    div { class: "alert-content",
                        p { class: "alert-body",
                            {i18n.t("settings.admin_only_hint")}
                        }
                    }
                }
            }

            match settings() {
                None => rsx! { p { class: "text-secondary", {i18n.t("table.loading")} } },
                Some(Err(_)) => rsx! { p { class: "text-secondary", {i18n.t("settings.load_failed")} } },
                Some(Ok(_)) => rsx! {
                    if save_ok() {
                        div { class: "alert alert-success",
                            span { "✔ {i18n.t(\"settings.saved\")}" }
                        }
                    }
                    if !save_error().is_empty() {
                        div { class: "alert alert-error",
                            span { "{save_error}" }
                        }
                    }

                    div { class: "settings-console-stack",
                        div { class: "settings-section-card",
                            div { class: "settings-section-head",
                                div {
                                    h3 { class: "settings-section-title", {i18n.t("settings.basic_title")} }
                                    p { class: "settings-section-description",
                                        {i18n.t("settings.basic_desc")}
                                    }
                                }
                            }
                            div { class: "settings-section-body",
                                SettingItemText {
                                    label: i18n.t("settings.site_name_label").to_string(),
                                    description: i18n.t("settings.site_name_desc").to_string(),
                                    setting_key: "site_name",
                                    value: platform_name.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error
                                }
                                SettingItemNumber {
                                    label: i18n.t("settings.default_user_quota_label").to_string(),
                                    description: i18n.t("settings.default_user_quota_desc").to_string(),
                                    setting_key: "default_user_quota",
                                    value: default_user_quota.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error,
                                    allow_negative: true
                                }
                                SettingItemSelect {
                                    label: i18n.t("settings.default_currency_label").to_string(),
                                    description: i18n.t("settings.default_currency_desc").to_string(),
                                    setting_key: "default_currency",
                                    value: currency.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error,
                                    options: vec![
                                        ("CNY".to_string(), i18n.t("pricing.currency_cny").to_string()),
                                        ("USD".to_string(), i18n.t("pricing.currency_usd").to_string()),
                                    ]
                                }
                                SettingItemNumber {
                                    label: i18n.t("settings.min_recharge_label").to_string(),
                                    description: i18n.t("settings.min_recharge_desc").to_string(),
                                    setting_key: "min_recharge_amount",
                                    value: min_recharge.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error,
                                    allow_negative: false
                                }
                                SettingItemNumber {
                                    label: i18n.t("settings.max_recharge_label").to_string(),
                                    description: i18n.t("settings.max_recharge_desc").to_string(),
                                    setting_key: "max_recharge_amount",
                                    value: max_recharge.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error,
                                    allow_negative: false
                                }
                            }
                        }

                        div { class: "settings-section-card",
                            div { class: "settings-section-head",
                                div {
                                    h3 { class: "settings-section-title", {i18n.t("settings.payment_title")} }
                                    p { class: "settings-section-description", {i18n.t("settings.payment_desc")} }
                                }
                            }
                            div { class: "settings-section-body",
                                SettingItemToggle {
                                    label: i18n.t("recharge.alipay").to_string(),
                                    description: i18n.t("settings.alipay_enabled_desc").to_string(),
                                    setting_key: "alipay_enabled",
                                    value: alipay_enabled.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error
                                }
                                SettingItemToggle {
                                    label: i18n.t("recharge.wechat_pay").to_string(),
                                    description: i18n.t("settings.wechatpay_enabled_desc").to_string(),
                                    setting_key: "wechatpay_enabled",
                                    value: wechatpay_enabled.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error
                                }
                            }
                        }

                        div { class: "settings-section-card",
                            div { class: "settings-section-head",
                                div {
                                    h3 { class: "settings-section-title", {i18n.t("settings.security_title")} }
                                    p { class: "settings-section-description",
                                        {i18n.t("settings.security_desc")}
                                    }
                                }
                            }
                            div { class: "settings-section-body",
                                SettingItemNumber {
                                    label: i18n.t("settings.jwt_expire_label").to_string(),
                                    description: i18n.t("settings.jwt_expire_desc").to_string(),
                                    setting_key: "jwt_expire_hours",
                                    value: jwt_expire.clone(),
                                    editable: is_admin,
                                    auth_store,
                                    save_ok,
                                    save_error,
                                    allow_negative: false
                                }
                            }
                        }

                        div { class: "settings-section-card",
                            div { class: "settings-section-head",
                                div {
                                    h3 { class: "settings-section-title", {i18n.t("settings.distribution_title")} }
                                    p { class: "settings-section-description",
                                        {i18n.t("settings.distribution_desc")}
                                    }
                                }
                            }
                            div { class: "settings-section-body",
                                SettingItemToggle {
                                    label: i18n.t("settings.distribution_enabled_label").to_string(),
                                    description: if is_system {
                                        i18n.t("settings.distribution_enabled_desc").to_string()
                                    } else {
                                        i18n.t("settings.distribution_enabled_system_only_desc").to_string()
                                    },
                                    setting_key: "distribution_enabled",
                                    value: distribution_enabled.clone(),
                                    editable: is_system,
                                    auth_store,
                                    save_ok,
                                    save_error
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
fn SettingItemText(
    label: String,
    description: String,
    setting_key: String,
    value: String,
    editable: bool,
    auth_store: AuthStore,
    mut save_ok: Signal<bool>,
    mut save_error: Signal<String>,
) -> Element {
    let i18n = use_i18n();
    let mut edit_val = use_signal(|| value.clone());
    let mut saving = use_signal(|| false);

    let value_for_effect = value.clone();
    use_effect(move || {
        *edit_val.write() = value_for_effect.clone();
    });

    let key = setting_key.clone();
    let on_save = move |_| {
        let val = edit_val();
        let k = key.clone();
        let token = auth_store.token().unwrap_or_default();
        *saving.write() = true;
        *save_ok.write() = false;
        *save_error.write() = String::new();
        spawn(async move {
            let json_val = serde_json::Value::String(val);
            match settings_service::update_by_key(&k, &json_val, &token).await {
                Ok(_) => {
                    *save_ok.write() = true;
                    *saving.write() = false;
                }
                Err(e) => {
                    *save_error.write() =
                        format!("{} {}：{}", i18n.t("settings.save_failed"), k, e);
                    *saving.write() = false;
                }
            }
        });
    };

    rsx! {
        div { class: "setting-row",
            div { class: "setting-row-main",
                div { class: "setting-row-meta",
                    span { class: "setting-label", "{label}" }
                    p { class: "setting-description", "{description}" }
                }
                if editable {
                    div { class: "setting-input-row",
                        input {
                            class: "input-field setting-control",
                            value: "{edit_val}",
                            oninput: move |e| *edit_val.write() = e.value(),
                        }
                        Button {
                            variant: ButtonVariant::Secondary,
                            size: ui::ButtonSize::Small,
                            loading: saving(),
                            onclick: on_save,
                            if saving() { {i18n.t("form.saving")} } else { {i18n.t("form.save")} }
                        }
                    }
                } else {
                    span { class: "setting-value setting-readonly",
                        if value.is_empty() { "—" } else { "{value}" }
                    }
                }
            }
        }
    }
}

#[component]
fn SettingItemNumber(
    label: String,
    description: String,
    setting_key: String,
    value: String,
    editable: bool,
    auth_store: AuthStore,
    mut save_ok: Signal<bool>,
    mut save_error: Signal<String>,
    allow_negative: bool,
) -> Element {
    let i18n = use_i18n();
    let mut edit_val = use_signal(|| value.clone());
    let mut saving = use_signal(|| false);
    let mut error_msg = use_signal(String::new);

    let value_for_effect = value.clone();
    use_effect(move || {
        *edit_val.write() = value_for_effect.clone();
    });

    let key = setting_key.clone();
    let on_save = move |_| {
        let val_str = edit_val();
        if let Ok(num) = val_str.parse::<f64>() {
            if !allow_negative && num < 0.0 {
                *error_msg.write() = i18n.t("settings.non_negative").to_string();
                return;
            }
            *error_msg.write() = String::new();
        } else {
            *error_msg.write() = i18n.t("settings.invalid_number").to_string();
            return;
        }

        let k = key.clone();
        let token = auth_store.token().unwrap_or_default();
        *saving.write() = true;
        *save_ok.write() = false;
        *save_error.write() = String::new();
        spawn(async move {
            let json_val = serde_json::Value::String(val_str);
            match settings_service::update_by_key(&k, &json_val, &token).await {
                Ok(_) => {
                    *save_ok.write() = true;
                    *saving.write() = false;
                }
                Err(e) => {
                    *save_error.write() =
                        format!("{} {}：{}", i18n.t("settings.save_failed"), k, e);
                    *saving.write() = false;
                }
            }
        });
    };

    rsx! {
        div { class: "setting-row",
            div { class: "setting-row-main",
                div { class: "setting-row-meta",
                    span { class: "setting-label", "{label}" }
                    p { class: "setting-description", "{description}" }
                }
                if editable {
                    div { class: "setting-input-col",
                        div { class: "setting-input-row",
                            input {
                                class: "input-field setting-control",
                                r#type: "number",
                                min: if allow_negative { None } else { Some("0") },
                                value: "{edit_val}",
                                oninput: move |e| {
                                    *edit_val.write() = e.value();
                                    *error_msg.write() = String::new();
                                },
                            }
                            Button {
                                variant: ButtonVariant::Secondary,
                                size: ui::ButtonSize::Small,
                                loading: saving(),
                                onclick: on_save,
                                if saving() { {i18n.t("form.saving")} } else { {i18n.t("form.save")} }
                            }
                        }
                        if !error_msg().is_empty() {
                            span { class: "error-text text-sm", "{error_msg}" }
                        }
                    }
                } else {
                    span { class: "setting-value setting-readonly",
                        if value.is_empty() { "—" } else { "{value}" }
                    }
                }
            }
        }
    }
}

#[component]
fn SettingItemSelect(
    label: String,
    description: String,
    setting_key: String,
    value: String,
    editable: bool,
    auth_store: AuthStore,
    mut save_ok: Signal<bool>,
    mut save_error: Signal<String>,
    options: Vec<(String, String)>,
) -> Element {
    let i18n = use_i18n();
    let mut edit_val = use_signal(|| value.clone());
    let mut saving = use_signal(|| false);

    let value_for_effect = value.clone();
    use_effect(move || {
        *edit_val.write() = value_for_effect.clone();
    });

    let key = setting_key.clone();
    let on_save = move |_| {
        let val = edit_val();
        let k = key.clone();
        let token = auth_store.token().unwrap_or_default();
        *saving.write() = true;
        *save_ok.write() = false;
        *save_error.write() = String::new();
        spawn(async move {
            let json_val = serde_json::Value::String(val);
            match settings_service::update_by_key(&k, &json_val, &token).await {
                Ok(_) => {
                    *save_ok.write() = true;
                    *saving.write() = false;
                }
                Err(e) => {
                    *save_error.write() =
                        format!("{} {}：{}", i18n.t("settings.save_failed"), k, e);
                    *saving.write() = false;
                }
            }
        });
    };

    rsx! {
        div { class: "setting-row",
            div { class: "setting-row-main",
                div { class: "setting-row-meta",
                    span { class: "setting-label", "{label}" }
                    p { class: "setting-description", "{description}" }
                }
                if editable {
                    div { class: "setting-input-row",
                        select {
                            class: "input-field setting-control",
                            onchange: move |e| *edit_val.write() = e.value(),
                            for (opt_val, opt_label) in options.iter() {
                                option {
                                    value: "{opt_val}",
                                    selected: *opt_val == edit_val(),
                                    "{opt_label}"
                                }
                            }
                        }
                        Button {
                            variant: ButtonVariant::Secondary,
                            size: ui::ButtonSize::Small,
                            loading: saving(),
                            onclick: on_save,
                            if saving() { {i18n.t("form.saving")} } else { {i18n.t("form.save")} }
                        }
                    }
                } else {
                    span { class: "setting-value setting-readonly",
                        if value.is_empty() { "—" } else { "{value}" }
                    }
                }
            }
        }
    }
}

#[component]
fn SettingItemToggle(
    label: String,
    description: String,
    setting_key: String,
    value: String,
    editable: bool,
    auth_store: AuthStore,
    mut save_ok: Signal<bool>,
    mut save_error: Signal<String>,
) -> Element {
    let i18n = use_i18n();
    let public_settings_store = use_context::<PublicSettingsStore>();
    let mut edit_val = use_signal(|| value.clone());
    let mut saving = use_signal(|| false);

    let value_for_effect = value.clone();
    use_effect(move || {
        *edit_val.write() = value_for_effect.clone();
    });

    let is_enabled = edit_val() == "true" || edit_val() == "1";
    let key = setting_key.clone();

    rsx! {
        div { class: "setting-row",
            div { class: "setting-row-main",
                div { class: "setting-row-meta",
                    span { class: "setting-label", "{label}" }
                    p { class: "setting-description", "{description}" }
                }
                if editable {
                    div { class: "setting-toggle-row",
                        label { class: "toggle-switch",
                            input {
                                r#type: "checkbox",
                                checked: is_enabled,
                                onchange: move |e| {
                                    let new_val = if e.checked() { "true" } else { "false" };
                                    let previous_value = edit_val();
                                    *edit_val.write() = new_val.to_string();
                                    let k = key.clone();
                                    let token = auth_store.token().unwrap_or_default();
                                    *saving.write() = true;
                                    *save_ok.write() = false;
                                    *save_error.write() = String::new();
                                    spawn(async move {
                                        let json_val = serde_json::Value::String(new_val.to_string());
                                        match settings_service::update_by_key(&k, &json_val, &token).await {
                                            Ok(_) => {
                                                if k == "distribution_enabled" {
                                                    let mut store = public_settings_store;
                                                    store.set_distribution_enabled(new_val == "true");
                                                }
                                                *save_ok.write() = true;
                                                *saving.write() = false;
                                            }
                                            Err(e) => {
                                                *edit_val.write() = previous_value;
                                                *save_error.write() =
                                                    format!("{} {}：{}", i18n.t("settings.save_failed"), k, e);
                                                *saving.write() = false;
                                            }
                                        }
                                    });
                                },
                            }
                            span { class: "toggle-slider" }
                        }
                        span {
                            class: if is_enabled { "setting-toggle-state is-enabled" } else { "setting-toggle-state" },
                            if saving() { {i18n.t("form.saving")} } else if is_enabled { {i18n.t("common.enabled")} } else { {i18n.t("common.disabled")} }
                        }
                    }
                } else {
                    span {
                        class: if is_enabled { "setting-value setting-readonly is-enabled" } else { "setting-value setting-readonly" },
                        if is_enabled { {i18n.t("common.enabled")} } else { {i18n.t("common.disabled")} }
                    }
                }
            }
        }
    }
}
