use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use ui::ThemeCtx;

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;
use crate::services::api_client::user_error_message;
use crate::services::auth_service;
use crate::stores::public_settings_store::PublicSettingsStore;

/// 后端返回的“用户名/邮箱不匹配”错误消息（固定中文，用于匹配触发冷却）
const FORGOT_PASSWORD_IDENTITY_ERROR: &str = "邮箱地址或用户名错误，无法发送重置密码链接";
const FORGOT_PASSWORD_ERROR_COOLDOWN_SECS: u32 = 30;

fn start_error_cooldown(mut cooldown_seconds: Signal<u32>) {
    cooldown_seconds.set(FORGOT_PASSWORD_ERROR_COOLDOWN_SECS);
    spawn(async move {
        let mut remaining = FORGOT_PASSWORD_ERROR_COOLDOWN_SECS;
        while remaining > 0 {
            TimeoutFuture::new(1_000).await;
            remaining -= 1;
            cooldown_seconds.set(remaining);
        }
    });
}

#[component]
pub fn ForgotPassword() -> Element {
    let i18n = use_i18n();
    let public_settings_store = use_context::<PublicSettingsStore>();

    // 读取全局主题上下文，判断当前是否为暗色主题
    let ThemeCtx(theme) = try_use_context::<ThemeCtx>()
        .unwrap_or_else(|| ThemeCtx(use_signal(|| "dark".to_string())));
    let is_dark = theme().as_str() == "dark";
    let site_name = use_memo(move || {
        public_settings_store
            .site_name()
            .unwrap_or_else(|| "KeyCompute".to_string())
    });
    let mut name = use_signal(String::new);
    let mut email = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let cooldown_seconds = use_signal(|| 0u32);
    let mut sent = use_signal(|| false);
    let mut error_msg = use_signal(|| Option::<String>::None);

    let nav = use_navigator();

    // 提前提取 &'static str，避免闭包成为 FnOnce
    let t_enter_name = i18n.t("auth.enter_username");
    let t_enter_email = i18n.t("auth.enter_email");
    let t_identity_error = i18n.t("auth.identity_error");
    let t_login_page_tagline_1 = i18n.t("login.tagline_1");
    let t_login_page_tagline_highlight = i18n.t("login.tagline_highlight");
    let t_login_page_tagline_2 = i18n.t("login.tagline_2");
    let t_login_page_tagline_3 = i18n.t("login.tagline_3");
    let t_login_page_desc = i18n.t("login.description");

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        if loading() || cooldown_seconds() > 0 {
            return;
        }
        if name().trim().is_empty() {
            error_msg.set(Some(t_enter_name.to_string()));
            return;
        }
        if email().trim().is_empty() {
            error_msg.set(Some(t_enter_email.to_string()));
            return;
        }
        loading.set(true);
        error_msg.set(None);
        let name_val = name();
        let email_val = email();
        let cooldown_signal = cooldown_seconds;
        spawn(async move {
            match auth_service::forgot_password(&name_val, &email_val).await {
                Ok(_) => {
                    sent.set(true);
                    loading.set(false);
                }
                Err(e) => {
                    let message = user_error_message(&e);
                    if message == FORGOT_PASSWORD_IDENTITY_ERROR {
                        start_error_cooldown(cooldown_signal);
                    }
                    // 显示给用户的错误信息使用 i18n 翻译，而非后端原始中文
                    let display_msg = if message == FORGOT_PASSWORD_IDENTITY_ERROR {
                        t_identity_error.to_string()
                    } else {
                        message
                    };
                    error_msg.set(Some(display_msg));
                    loading.set(false);
                }
            }
        });
    };

    rsx! {
        document::Title { "{site_name}" }

        div { class: "kc-login-page",
            div { class: "kc-login-bg-grid" }
            if is_dark {
                div { class: "kc-login-bg-glow kc-login-glow-one" }
                div { class: "kc-login-bg-glow kc-login-glow-two" }
            }
            div { class: "kc-login-container",
                div { class: "kc-login-brand-panel",
                    div { class: "kc-login-brand-content",
                        div { class: "kc-login-logo",
                            div { class: "kc-login-logo-icon" }
                            div { class: "kc-login-logo-text", "{site_name}" }
                        }
                        h1 { class: "kc-login-tagline",
                            "{t_login_page_tagline_1} "
                            span { "{t_login_page_tagline_highlight}" }
                            " {t_login_page_tagline_2}"
                            br {}
                            "{t_login_page_tagline_3}"
                        }
                        p { class: "kc-login-description", "{t_login_page_desc}" }
                        div { class: "kc-login-features",
                            for label in [
                                i18n.t("login.feature_routing"),
                                i18n.t("login.feature_billing"),
                                i18n.t("login.feature_ha"),
                                i18n.t("login.feature_api"),
                            ]
                            {
                                div { class: "kc-login-feature-badge",
                                    div { class: "kc-login-feature-dot" }
                                    "{label}"
                                }
                            }
                        }
                    }
                    div { class: "kc-login-tech-circles",
                        div { class: "kc-login-circle kc-login-circle-one" }
                        div { class: "kc-login-circle kc-login-circle-two" }
                        div { class: "kc-login-circle kc-login-circle-three" }
                    }
                }

                div { class: "kc-login-panel",
                    div { class: "kc-login-card kc-auth-card",
                        div { class: "kc-login-header",
                            h1 { class: "kc-login-title", {i18n.t("auth.reset_password")} }
                            p { class: "kc-login-subtitle", {i18n.t("auth.reset_subtitle")} }
                        }

                        if sent() {
                            div { class: "kc-auth-success-block",
                                div { class: "kc-login-status kc-login-status-success",
                                    {i18n.t("auth.reset_sent")}
                                }
                                p { class: "kc-auth-support-text", {i18n.t("auth.reset_subtitle")} }
                            }
                        } else {
                            if let Some(err) = error_msg() {
                                div { class: "kc-login-status kc-login-status-error",
                                    "{err}"
                                }
                            }
                            form { onsubmit: on_submit,
                                div { class: "kc-login-form-group",
                                    label { class: "kc-login-form-label", {i18n.t("auth.username")} }
                                    input {
                                        class: "kc-login-form-input",
                                        r#type: "text",
                                        placeholder: i18n.t("auth.reset_username_placeholder"),
                                        value: "{name}",
                                        oninput: move |e| name.set(e.value()),
                                    }
                                    div { class: "kc-login-input-glow" }
                                }
                                div { class: "kc-login-form-group",
                                    label { class: "kc-login-form-label", {i18n.t("auth.email")} }
                                    input {
                                        class: "kc-login-form-input",
                                        r#type: "email",
                                        placeholder: i18n.t("auth.reset_email_placeholder"),
                                        value: "{email}",
                                        oninput: move |e| email.set(e.value()),
                                    }
                                    div { class: "kc-login-input-glow" }
                                }
                                button {
                                    class: "kc-login-button",
                                    r#type: "submit",
                                    disabled: loading() || cooldown_seconds() > 0,
                                    span {
                                        if loading() {
                                            {i18n.t("auth.sending")}
                                        } else if cooldown_seconds() > 0 {
                                            {format!("{} ({}s)", i18n.t("auth.cooldown_retry"), cooldown_seconds())}
                                        } else {
                                            {i18n.t("auth.send_reset_link")}
                                        }
                                    }
                                }
                            }
                        }

                        div { class: "kc-login-signup",
                            button {
                                class: "kc-login-signup-link",
                                r#type: "button",
                                onclick: move |_| {
                                    nav.push(Route::Login {});
                                },
                                {i18n.t("auth.back_to_login")}
                            }
                        }
                    }
                }
            }
        }
    }
}
