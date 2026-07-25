use dioxus::prelude::*;

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;

/// 登录页面 - 重定向到首页（登录功能已移至首页弹窗）
#[component]
pub fn Login() -> Element {
    let i18n = use_i18n();
    let nav = use_navigator();

    // 直接重定向到首页
    use_effect(move || {
        nav.replace(Route::Home {});
    });

    rsx! {
        div { style: "display:flex;align-items:center;justify-content:center;height:100vh;background:var(--bg-primary,#0a0f1a);color:var(--text-primary,#f0f6ff)",
            p { {i18n.t("common.redirect_to_home")} }
        }
    }
}
