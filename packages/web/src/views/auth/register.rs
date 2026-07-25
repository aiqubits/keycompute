use dioxus::prelude::*;

use crate::hooks::use_i18n::use_i18n;
use crate::router::{RegisterQuery, Route};
use crate::stores::referral_store::ReferralStore;

/// 注册页面 - 读取推荐码后重定向到首页（注册功能已移至首页弹窗）
#[component]
pub fn Register(query: RegisterQuery) -> Element {
    let i18n = use_i18n();
    let nav = use_navigator();
    let mut referral_store = use_context::<ReferralStore>();

    // 从路由 query 参数读取推荐码存入全局 store，然后重定向到首页。
    // 注意不能读 window.location.search：Router 初始化时会规范化地址栏，
    // 未在路由声明的 query 参数在组件渲染前就已被抹掉。
    use_effect(move || {
        if let Some(ref_code) = query.ref_code.clone() {
            referral_store.set_code(ref_code);
        }
        nav.replace(Route::Home {});
    });

    rsx! {
        div { style: "display:flex;align-items:center;justify-content:center;height:100vh;background:var(--bg-primary,#0a0f1a);color:var(--text-primary,#f0f6ff)",
            p { {i18n.t("common.redirect_to_home")} }
        }
    }
}
