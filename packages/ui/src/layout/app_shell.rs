use dioxus::prelude::*;

use super::footer::Footer;
use super::header::{Header, UserMenuAction};
use super::sidebar::{NavSection, Sidebar};

const VARIABLES_CSS: Asset = asset!("/assets/styling/variables.css");
const LAYOUT_CSS: Asset = asset!("/assets/styling/layout.css");
const COMPONENTS_CSS: Asset = asset!("/assets/styling/components.css");
const DARK_CSS: Asset = asset!("/assets/styling/dark.css");
const RESPONSIVE_CSS: Asset = asset!("/assets/styling/responsive.css");

/// 主题 Signal 的 Context 包装，与 web 层共享，避免与 lang 的 Signal<String> 类型冲突
#[derive(Clone, Copy)]
pub struct ThemeCtx(pub Signal<String>);

/// 全局 UI 状态，通过 Context API 向下传递
#[derive(Clone, Copy)]
pub struct UiState {
    /// 侧边栏是否折叠
    pub sidebar_collapsed: Signal<bool>,
    /// 移动端侧边栏是否打开
    pub sidebar_mobile_open: Signal<bool>,
    /// 主题：light / dark — 与 ThemeCtx 共享同一信号源
    pub theme: Signal<String>,
    /// 语言：zh / en
    pub lang: Signal<String>,
}

/// 应用外壳组件，包含侧边栏 + 顶部栏 + 内容区 + 页脚
///
/// # Props
/// - `nav_sections`：侧边栏导航分组列表
/// - `user_name`：当前登录用户名（显示头像首字母）
/// - `site_logo_src`：侧边栏站点 Logo 地址
/// - `children`：主内容区内容
/// - `on_user_menu`：用户下拉菜单操作回调
#[component]
pub fn AppShell(
    #[props(default)] nav_sections: Vec<NavSection>,
    #[props(default)] user_name: String,
    #[props(default)] current_path: String,
    #[props(default)] home_title: String,
    #[props(default)] open_menu_title: String,
    #[props(default)] close_menu_title: String,
    #[props(default)] switch_to_light_theme_title: String,
    #[props(default)] switch_to_dark_theme_title: String,
    #[props(default)] switch_to_zh_title: String,
    #[props(default)] switch_to_en_title: String,
    #[props(default)] profile_label: String,
    #[props(default)] user_menu_label: String,
    #[props(default)] account_settings_label: String,
    #[props(default)] logout_label: String,
    #[props(default)] expand_sidebar_title: String,
    #[props(default)] collapse_sidebar_title: String,
    #[props(default)] expand_label: String,
    #[props(default)] collapse_label: String,
    #[props(default = "KeyCompute".to_string())] site_name: String,
    #[props(default)] site_logo_src: String,
    #[props(default)] on_user_menu: EventHandler<UserMenuAction>,
    children: Element,
) -> Element {
    let sidebar_collapsed = use_signal(|| false);
    let mut sidebar_mobile_open = use_signal(|| false);

    // 从 App 根组件的 ThemeCtx context 获取主题信号，避免多源同步问题
    // 使用 try_use_context 并带 fallback，确保在独立使用 ui 库时不 panic
    let ThemeCtx(theme) = try_use_context::<ThemeCtx>().unwrap_or_else(|| {
        ThemeCtx(use_signal(|| {
            #[cfg(target_arch = "wasm32")]
            {
                read_local_storage("keyc_theme").unwrap_or_else(|| "dark".to_string())
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                "dark".to_string()
            }
        }))
    });

    let fallback_lang = use_signal(|| {
        #[cfg(target_arch = "wasm32")]
        {
            read_local_storage("keyc_lang").unwrap_or_else(|| "zh".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "zh".to_string()
        }
    });
    let lang = try_use_context::<Signal<String>>().unwrap_or(fallback_lang);

    use_context_provider(|| UiState {
        sidebar_collapsed,
        sidebar_mobile_open,
        theme,
        lang,
    });

    // 重新提供 ThemeCtx，确保子组件也能通过 ThemeCtx 访问主题信号
    use_context_provider(|| ThemeCtx(theme));

    // 将主题同步到 HTML data-theme 属性，确保 CSS 变量生效
    // 当 App 根组件已提供同步时此 effect 冗余但无害，在独立使用 ui 库时则必不可少
    use_effect(move || {
        let val = theme();
        #[cfg(target_arch = "wasm32")]
        {
            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(doc) = window.document() else {
                return;
            };
            let Some(html) = doc.document_element() else {
                return;
            };
            let _ = html.set_attribute("data-theme", &val);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = val;
    });

    let collapsed = sidebar_collapsed();
    let mobile_open = sidebar_mobile_open();

    let main_class = if collapsed {
        "main-area sidebar-collapsed"
    } else {
        "main-area"
    };

    let overlay_class = if mobile_open {
        "sidebar-overlay visible"
    } else {
        "sidebar-overlay"
    };

    rsx! {
        document::Link { rel: "stylesheet", href: VARIABLES_CSS }
        document::Link { rel: "stylesheet", href: LAYOUT_CSS }
        document::Link { rel: "stylesheet", href: COMPONENTS_CSS }
        document::Link { rel: "stylesheet", href: DARK_CSS }
        document::Link { rel: "stylesheet", href: RESPONSIVE_CSS }

        div {
            class: "app-shell",
            onkeydown: move |event| {
                if event.key() == Key::Escape && sidebar_mobile_open() {
                    sidebar_mobile_open.set(false);
                }
            },
            div {
                class: "{overlay_class}",
                onclick: move |_| {
                    *sidebar_mobile_open.write() = false;
                },
            }

            Sidebar {
                sections: nav_sections.clone(),
                collapsed: sidebar_collapsed,
                mobile_open: sidebar_mobile_open,
                current_path: current_path.clone(),
                expand_sidebar_title: expand_sidebar_title.clone(),
                collapse_sidebar_title: collapse_sidebar_title.clone(),
                expand_label: expand_label.clone(),
                collapse_label: collapse_label.clone(),
                close_menu_title: close_menu_title.clone(),
                site_name: site_name.clone(),
                site_logo_src: site_logo_src.clone(),
            }

            div { class: "{main_class}",
                Header {
                    user_name: user_name.clone(),
                    sidebar_collapsed,
                    sidebar_mobile_open,
                    theme,
                    lang,
                    home_title: home_title.clone(),
                    open_menu_title: open_menu_title.clone(),
                    switch_to_light_theme_title: switch_to_light_theme_title.clone(),
                    switch_to_dark_theme_title: switch_to_dark_theme_title.clone(),
                    switch_to_zh_title: switch_to_zh_title.clone(),
                    switch_to_en_title: switch_to_en_title.clone(),
                    profile_label: profile_label.clone(),
                    user_menu_label: user_menu_label.clone(),
                    account_settings_label: account_settings_label.clone(),
                    logout_label: logout_label.clone(),
                    on_user_menu,
                }

                main { class: "content-area",
                    div { class: "content-inner", {children} }
                }

                Footer { site_name: site_name.clone() }
            }
        }
    }
}

// ── 工具函数 ──────────────────────────────────────────────
#[cfg(target_arch = "wasm32")]
fn read_local_storage(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(key)
        .ok()?
}

#[cfg(test)]
mod tests {
    #[test]
    fn mobile_sidebar_overrides_desktop_collapsed_state() {
        let css = include_str!("../../assets/styling/responsive.css");
        assert!(css.contains(".sidebar.collapsed {"));
        assert!(css.contains(".sidebar.collapsed .sidebar-logo-copy"));
        assert!(css.contains(".sidebar.collapsed .sidebar-section-title"));
        assert!(css.contains(".sidebar.collapsed .sidebar-item-label"));
        assert!(css.contains(".sidebar.collapsed .sidebar-item {"));
    }

    #[test]
    fn visibility_helpers_preserve_component_display_outside_their_breakpoint() {
        let css = include_str!("../../assets/styling/responsive.css");
        assert!(css.contains("@media (max-width: 639px) { .hide-mobile"));
        assert!(!css.contains(".hide-mobile  { display: revert; }"));
    }
}
