use dioxus::prelude::*;
use ui::PageHeader;

use crate::hooks::use_i18n::use_i18n;
use crate::services::user_service;
use crate::stores::auth_store::AuthStore;

#[component]
pub fn UserSettings() -> Element {
    let i18n = use_i18n();
    let auth_store = use_context::<AuthStore>();
    let mut current_pwd = use_signal(String::new);
    let mut new_pwd = use_signal(String::new);
    let mut confirm_pwd = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut success_msg = use_signal(|| Option::<String>::None);
    let mut error_msg = use_signal(|| Option::<String>::None);

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        if current_pwd().is_empty() || new_pwd().is_empty() || confirm_pwd().is_empty() {
            error_msg.set(Some(
                i18n.t("account_settings.fill_all_passwords").to_string(),
            ));
            return;
        }
        if new_pwd() != confirm_pwd() {
            error_msg.set(Some(
                i18n.t("account_settings.password_mismatch").to_string(),
            ));
            return;
        }
        if new_pwd().len() < 8 {
            error_msg.set(Some(
                i18n.t("account_settings.password_too_short").to_string(),
            ));
            return;
        }
        if !new_pwd().chars().any(|c| c.is_uppercase()) {
            error_msg.set(Some(
                i18n.t("account_settings.password_no_uppercase").to_string(),
            ));
            return;
        }
        if !new_pwd().chars().any(|c| c.is_lowercase()) {
            error_msg.set(Some(
                i18n.t("account_settings.password_no_lowercase").to_string(),
            ));
            return;
        }
        if !new_pwd().chars().any(|c| c.is_ascii_digit()) {
            error_msg.set(Some(
                i18n.t("account_settings.password_no_digit").to_string(),
            ));
            return;
        }
        if !new_pwd()
            .chars()
            .any(|c| "!@#$%^&*()_+-=[]{}|;:',.<>?/~`".contains(c))
        {
            error_msg.set(Some(
                i18n.t("account_settings.password_no_special").to_string(),
            ));
            return;
        }
        saving.set(true);
        error_msg.set(None);
        success_msg.set(None);
        let cur = current_pwd();
        let nw = new_pwd();
        spawn(async move {
            let token = auth_store.token().unwrap_or_default();
            match user_service::change_password(&cur, &nw, &token).await {
                Ok(_) => {
                    success_msg.set(Some(
                        i18n.t("account_settings.password_changed").to_string(),
                    ));
                    current_pwd.set(String::new());
                    new_pwd.set(String::new());
                    confirm_pwd.set(String::new());
                    saving.set(false);
                }
                Err(e) => {
                    error_msg.set(Some(format!(
                        "{}：{e}",
                        i18n.t("account_settings.change_failed")
                    )));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        div {
            class: "page-container account-settings-page",
            PageHeader {
                title: i18n.t("page.account_settings").to_string(),
                description: i18n.t("account_settings.subtitle").to_string(),
            }

            if let Some(msg) = success_msg() {
                div { class: "alert alert-success", "{msg}" }
            }
            if let Some(err) = error_msg() {
                div { class: "alert alert-error", "{err}" }
            }

            div { class: "settings-section-card account-settings-card",
                div { class: "settings-section-head",
                    div {
                        h2 { class: "settings-section-title", {i18n.t("account_settings.change_password")} }
                        p { class: "settings-section-description",
                            {i18n.t("account_settings.section_desc")}
                        }
                    }
                }
                div { class: "settings-section-body",
                    form {
                        class: "account-settings-form",
                        onsubmit: on_submit,
                        div { class: "setting-row",
                            div { class: "setting-row-main",
                                div { class: "setting-row-meta",
                                    span { class: "setting-label", {i18n.t("account_settings.current_password")} }
                                    p { class: "setting-description", {i18n.t("account_settings.current_password_desc")} }
                                }
                                input {
                                    class: "form-input setting-control",
                                    r#type: "password",
                                    placeholder: "{i18n.t(\"account_settings.current_password_placeholder\")}",
                                    value: "{current_pwd}",
                                    oninput: move |e| current_pwd.set(e.value()),
                                }
                            }
                        }
                        div { class: "setting-row",
                            div { class: "setting-row-main",
                                div { class: "setting-row-meta",
                                    span { class: "setting-label", {i18n.t("account_settings.new_password")} }
                                    p { class: "setting-description", {i18n.t("account_settings.new_password_desc")} }
                                }
                                input {
                                    class: "form-input setting-control",
                                    r#type: "password",
                                    placeholder: "{i18n.t(\"account_settings.new_password_placeholder\")}",
                                    value: "{new_pwd}",
                                    oninput: move |e| new_pwd.set(e.value()),
                                }
                            }
                        }
                        div { class: "setting-row",
                            div { class: "setting-row-main",
                                div { class: "setting-row-meta",
                                    span { class: "setting-label", {i18n.t("account_settings.confirm_password")} }
                                    p { class: "setting-description", {i18n.t("account_settings.confirm_password_desc")} }
                                }
                                input {
                                    class: "form-input setting-control",
                                    r#type: "password",
                                    placeholder: "{i18n.t(\"account_settings.confirm_password_placeholder\")}",
                                    value: "{confirm_pwd}",
                                    oninput: move |e| confirm_pwd.set(e.value()),
                                }
                            }
                        }
                        div { class: "account-settings-actions",
                            button {
                                class: "btn btn-primary",
                                r#type: "submit",
                                disabled: saving(),
                                if saving() { {i18n.t("form.saving")} } else { {i18n.t("form.save_changes")} }
                            }
                        }
                    }
                }
            }
        }
    }
}
