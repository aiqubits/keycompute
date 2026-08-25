use dioxus::prelude::*;

use super::GITHUB_REPOSITORY_URL;

/// 页脚组件
#[component]
pub fn Footer(#[props(default = "KeyCompute".to_string())] site_name: String) -> Element {
    rsx! {
        footer { class: "footer",
            span { class: "footer-text",
                "© 2026 "
                a {
                    class: "footer-link",
                    href: GITHUB_REPOSITORY_URL,
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "{site_name}"
                }
                ". All Rights Reserved."
            }
        }
    }
}
