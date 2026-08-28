use dioxus::prelude::*;

/// 后台页面统一标题区。
///
/// 标题与说明始终作为一个整体占据左侧，页面级操作放在右侧；移动端由
/// `responsive.css` 统一改为纵向排列，避免页面自行拼装 flex 子项造成错位。
#[component]
pub fn PageHeader(
    title: String,
    #[props(default)] description: String,
    #[props(default)] leading: Option<Element>,
    #[props(default)] actions: Option<Element>,
    #[props(default)] class: String,
) -> Element {
    let root_class = if class.trim().is_empty() {
        "page-header".to_string()
    } else {
        format!("page-header {}", class.trim())
    };

    rsx! {
        div { class: "{root_class}",
            div { class: "page-header-main",
                div { class: "page-title-row",
                    if let Some(leading) = leading {
                        div { class: "page-header-leading", {leading} }
                    }
                    h1 { class: "page-title", "{title}" }
                }
                if !description.is_empty() {
                    p { class: "page-description", "{description}" }
                }
            }
            if let Some(actions) = actions {
                div { class: "page-header-actions", {actions} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_header_styles_define_desktop_and_mobile_contracts() {
        let main_css = include_str!("../../../web/assets/main.css");
        let responsive_css = include_str!("../../assets/styling/responsive.css");

        assert!(main_css.contains(".page-header-main"));
        assert!(main_css.contains(".page-header-actions"));
        assert!(responsive_css.contains(".page-header-actions"));
    }
}
