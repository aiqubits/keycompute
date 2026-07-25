use dioxus::prelude::*;

use crate::hooks::use_i18n::use_i18n;

#[component]
pub fn LoadingSpinner(#[props(default)] text: String) -> Element {
    let i18n = use_i18n();
    let display_text = if text.is_empty() {
        i18n.t("table.loading").to_string()
    } else {
        text
    };
    rsx! {
        div { class: "loading-container",
            div { class: "spinner" }
            p { class: "loading-text", "{display_text}" }
        }
    }
}
