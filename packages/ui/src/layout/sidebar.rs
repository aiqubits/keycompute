use dioxus::prelude::*;

use crate::icons::{
    IconActivity, IconBarChart, IconBuilding, IconChevronLeft, IconChevronRight, IconHome, IconKey,
    IconReceipt, IconServer, IconSettings, IconShare, IconTag, IconUser, IconUsers, IconWallet,
    IconX,
};

/// 单条导航项
#[derive(Clone, PartialEq)]
pub struct NavItem {
    /// 显示标签
    pub label: String,
    /// 路由路径（用于跳转和高亮判断）
    pub path: String,
    /// 图标名称，对应 IconXxx 组件
    pub icon: NavIcon,
    /// 是否需要 Admin 权限（仅影响视觉提示，不拦截路由）
    pub admin_only: bool,
}

impl NavItem {
    pub fn new(label: impl Into<String>, path: impl Into<String>, icon: NavIcon) -> Self {
        Self {
            label: label.into(),
            path: path.into(),
            icon,
            admin_only: false,
        }
    }

    pub fn admin(mut self) -> Self {
        self.admin_only = true;
        self
    }
}

/// 导航分组
#[derive(Clone, PartialEq)]
pub struct NavSection {
    /// 分组标题（显示在侧边栏）
    pub title: Option<String>,
    pub items: Vec<NavItem>,
}

/// 图标枚举，对应 icons.rs 中的组件
#[derive(Clone, PartialEq)]
pub enum NavIcon {
    Home,
    Key,
    Wallet,
    Users,
    Settings,
    BarChart,
    Receipt,
    Share,
    User,
    Server,
    Tag,
    Building,
    Activity,
}

/// 侧边导航栏
///
/// # Props
/// - `sections`：导航分组列表
/// - `collapsed`：是否折叠状态（Signal）
/// - `mobile_open`：移动端是否打开（Signal）
/// - `current_path`：当前活跃路径
#[component]
pub fn Sidebar(
    #[props(default)] sections: Vec<NavSection>,
    collapsed: Signal<bool>,
    mut mobile_open: Signal<bool>,
    #[props(default)] current_path: String,
    #[props(default)] expand_sidebar_title: String,
    #[props(default)] collapse_sidebar_title: String,
    #[props(default)] expand_label: String,
    #[props(default)] collapse_label: String,
    #[props(default)] close_menu_title: String,
    #[props(default = "KeyCompute".to_string())] site_name: String,
) -> Element {
    let is_collapsed = collapsed();
    let is_mobile_open = mobile_open();

    let sidebar_class = {
        let mut cls = "sidebar".to_string();
        if is_collapsed {
            cls.push_str(" collapsed");
        }
        if is_mobile_open {
            cls.push_str(" mobile-open");
        }
        cls
    };

    // 折叠/展开图标
    let toggle_icon = if is_collapsed {
        rsx! { IconChevronRight { size: 16 } }
    } else {
        rsx! { IconChevronLeft { size: 16 } }
    };

    rsx! {
        nav { id: "app-sidebar", class: "{sidebar_class}", aria_label: "Main navigation",
            // Logo 区域
            div { class: "sidebar-logo",
                div { class: "sidebar-logo-icon", "K" }
                div { class: "sidebar-logo-copy",
                    span { class: "sidebar-logo-text", "{site_name}" }
                    span { class: "sidebar-logo-kicker", "AI token platform" }
                }
                button {
                    class: "sidebar-mobile-close",
                    r#type: "button",
                    title: "{close_menu_title}",
                    aria_label: "{close_menu_title}",
                    onclick: move |_| mobile_open.set(false),
                    IconX { size: 20 }
                }
            }

            // 导航分组
            div { class: "sidebar-nav",
                for section in sections.iter() {
                    div { class: "sidebar-section",
                        if let Some(title) = &section.title {
                            div { class: "sidebar-section-title", "{title}" }
                        }
                        for item in section.items.iter() {
                            SidebarNavItem {
                                item: item.clone(),
                                collapsed: is_collapsed,
                                mobile_open,
                                current_path: current_path.clone(),
                            }
                        }
                    }
                }
            }

            // 底部折叠按钮
            div { class: "sidebar-footer",
                button {
                    class: "sidebar-item",
                    title: if is_collapsed { expand_sidebar_title.clone() } else { collapse_sidebar_title.clone() },
                    onclick: move |_| {
                        let cur = collapsed();
                        *collapsed.write() = !cur;
                    },
                    span { class: "sidebar-item-icon", {toggle_icon} }
                    span { class: "sidebar-item-label",
                        if is_collapsed { {expand_label.clone()} } else { {collapse_label.clone()} }
                    }
                }
            }
        }
    }
}

/// 单条导航项组件（内部组件）
#[component]
fn SidebarNavItem(
    item: NavItem,
    collapsed: bool,
    mut mobile_open: Signal<bool>,
    current_path: String,
) -> Element {
    let is_active =
        current_path == item.path || (item.path != "/" && current_path.starts_with(&item.path));

    let item_class = if is_active {
        "sidebar-item active"
    } else {
        "sidebar-item"
    };

    let icon_el = match item.icon {
        NavIcon::Home => rsx! { IconHome { size: 20 } },
        NavIcon::Key => rsx! { IconKey { size: 20 } },
        NavIcon::Wallet => rsx! { IconWallet { size: 20 } },
        NavIcon::Users => rsx! { IconUsers { size: 20 } },
        NavIcon::Settings => rsx! { IconSettings { size: 20 } },
        NavIcon::BarChart => rsx! { IconBarChart { size: 20 } },
        NavIcon::Receipt => rsx! { IconReceipt { size: 20 } },
        NavIcon::Share => rsx! { IconShare { size: 20 } },
        NavIcon::User => rsx! { IconUser { size: 20 } },
        NavIcon::Server => rsx! { IconServer { size: 20 } },
        NavIcon::Tag => rsx! { IconTag { size: 20 } },
        NavIcon::Building => rsx! { IconBuilding { size: 20 } },
        NavIcon::Activity => rsx! { IconActivity { size: 20 } },
    };

    let title_attr = item.label.clone();
    let label = item.label.clone();
    let path = item.path.clone();
    let nav = use_navigator();

    rsx! {
        button {
            class: "{item_class}",
            title: "{title_attr}",
            onclick: move |_| {
                mobile_open.set(false);
                nav.push(path.as_str());
            },
            span { class: "sidebar-item-icon", {icon_el} }
            span { class: "sidebar-item-label",
                "{label}"
                // Admin 专属标记
                if item.admin_only && !collapsed {
                    span {
                        class: "sidebar-admin-badge",
                        style: "margin-left: 4px; font-size: 10px; padding: 1px 4px; border-radius: 3px; background: var(--warning-light, #fef3c7); color: var(--warning, #d97706); font-weight: 600;",
                        "A"
                    }
                }
            }
        }
    }
}
