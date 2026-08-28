use dioxus::prelude::*;

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;

/// 账单页面（已合并到支付与账单页面）
///
/// 该页面保留以维持 /billing 路由兼容，自动重定向到 /payments
#[component]
pub fn Billing() -> Element {
    let i18n = use_i18n();
    let nav = use_navigator();
    use_effect(move || {
        nav.replace(Route::PaymentsOverview {});
    });
    rsx! {
        div {
            class: "auth-redirect-loading",
            style: "display:flex;align-items:center;justify-content:center;min-height:40vh",
            span { style: "color:var(--text-secondary,#64748b)", {i18n.t("common.redirecting")} }
        }
    }
}
