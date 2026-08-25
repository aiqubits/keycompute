use dioxus::prelude::*;

use crate::icons::{IconChevronDown, IconHome, IconLogOut, IconMenu, IconSettings, IconUser};
use crate::layout::GITHUB_REPOSITORY_URL;

/// 用户下拉菜单项回调
#[derive(Clone, Copy, PartialEq)]
pub enum UserMenuAction {
    /// 点击个人资料
    Profile,
    /// 点击设置
    Settings,
    /// 点击退出登录
    Logout,
}

/// 顶部栏组件
///
/// # Props
/// - `page_title`：当前页面标题
/// - `user_name`：当前用户名（头像首字母）
/// - `sidebar_collapsed`：侧边栏折叠状态（Signal，点击汉堡菜单时切换）
/// - `sidebar_mobile_open`：移动端侧边栏开关（Signal）
/// - `theme`：当前主题（Signal<String>），值为 "light" / "dark" / "system"
/// - `lang`：当前语言（Signal<String>），值为 "zh" / "en"
/// - `on_user_menu`：用户下拉菜单项点击回调
#[component]
pub fn Header(
    #[props(default)] page_title: String,
    #[props(default)] user_name: String,
    sidebar_collapsed: Signal<bool>,
    sidebar_mobile_open: Signal<bool>,
    theme: Signal<String>,
    lang: Signal<String>,
    #[props(default)] home_title: String,
    #[props(default)] open_menu_title: String,
    #[props(default)] switch_to_light_theme_title: String,
    #[props(default)] switch_to_dark_theme_title: String,
    #[props(default)] switch_to_zh_title: String,
    #[props(default)] switch_to_en_title: String,
    #[props(default)] profile_label: String,
    #[props(default)] account_settings_label: String,
    #[props(default)] logout_label: String,
    #[props(default)] on_user_menu: EventHandler<UserMenuAction>,
) -> Element {
    // 头像首字母
    let avatar_char = user_name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "U".to_string());

    // 主题图标：light 显示月亮（切换到 dark），dark 显示太阳（切换到 light）
    let is_dark = theme() == "dark";
    let theme_title = if is_dark {
        switch_to_light_theme_title
    } else {
        switch_to_dark_theme_title
    };

    let lang_val = lang();
    let lang_label = if lang_val == "zh" { "EN" } else { "中" };
    let lang_title = if lang_val == "zh" {
        switch_to_en_title
    } else {
        switch_to_zh_title
    };

    let title = page_title.clone();

    // 下拉菜单展开状态
    let mut dropdown_open = use_signal(|| false);

    rsx! {
        header { class: "header",
            // 左侧
            div { class: "header-left",
                // PC 端返回首页按钮
                button {
                    class: "header-toggle-btn hide-mobile",
                    title: "{home_title}",
                    onclick: move |_| {
                        let nav = use_navigator();
                        nav.push("/");
                    },
                    IconHome { size: 20 }
                }
                // 移动端汉堡菜单
                button {
                    class: "header-toggle-btn hide-desktop hide-tablet",
                    title: "{open_menu_title}",
                    aria_label: "{open_menu_title}",
                    aria_controls: "app-sidebar",
                    aria_expanded: sidebar_mobile_open(),
                    onclick: move |_| {
                        let cur = sidebar_mobile_open();
                        *sidebar_mobile_open.write() = !cur;
                    },
                    IconMenu { size: 20 }
                }

                // 页面标题
                if !title.is_empty() {
                    h1 { class: "header-page-title", "{title}" }
                }
            }

            // 右侧工具栏
            div { class: "header-right",
                // 移动端返回首页按钮（PC 端左侧已有，此处仅移动端显示）
                button {
                    class: "header-icon-btn header-home-btn-mobile hide-desktop hide-tablet",
                    title: "{home_title}",
                    onclick: move |_| {
                        let nav = use_navigator();
                        nav.push("/");
                    },
                    IconHome { size: 18 }
                }

                // GitHub 仓库链接（全尺寸可见，与首页样式一致，按后台比例缩放）
                a {
                    class: "header-github-link",
                    href: GITHUB_REPOSITORY_URL,
                    target: "_blank",
                    rel: "noopener noreferrer",
                    title: "GitHub",
                    aria_label: "GitHub",
                    svg {
                        width: "18",
                        height: "18",
                        view_box: "0 0 24 24",
                        fill: "currentColor",
                        path { d: "M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.4 3-.405 1.02.005 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" }
                    }
                }

                // 主题切换
                button {
                    class: "header-icon-btn header-theme-btn",
                    title: "{theme_title}",
                    onclick: move |_| {
                        let cur = theme();
                        let next = if cur == "dark" { "light" } else { "dark" };
                        *theme.write() = next.to_string();
                        // 持久化到 localStorage 并触发与首页一致的切换动画
                        #[cfg(target_arch = "wasm32")]
                        {
                            let _ = write_local_storage("keyc_theme", next);
                            trigger_theme_switching_animation();
                        }
                    },
                    if is_dark {
                        svg {
                            width: "18",
                            height: "18",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            path { d: "M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" }
                        }
                    } else {
                        svg {
                            width: "18",
                            height: "18",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            circle { cx: "12", cy: "12", r: "5" }
                            line {
                                x1: "12",
                                y1: "1",
                                x2: "12",
                                y2: "3",
                            }
                            line {
                                x1: "12",
                                y1: "21",
                                x2: "12",
                                y2: "23",
                            }
                            line {
                                x1: "4.22",
                                y1: "4.22",
                                x2: "5.64",
                                y2: "5.64",
                            }
                            line {
                                x1: "18.36",
                                y1: "18.36",
                                x2: "19.78",
                                y2: "19.78",
                            }
                            line {
                                x1: "1",
                                y1: "12",
                                x2: "3",
                                y2: "12",
                            }
                            line {
                                x1: "21",
                                y1: "12",
                                x2: "23",
                                y2: "12",
                            }
                            line {
                                x1: "4.22",
                                y1: "19.78",
                                x2: "5.64",
                                y2: "18.36",
                            }
                            line {
                                x1: "18.36",
                                y1: "5.64",
                                x2: "19.78",
                                y2: "4.22",
                            }
                        }
                    }
                }

                // 语言切换（与首页一致：仅文本 EN / 中）
                button {
                    class: "header-icon-btn header-lang-btn",
                    title: "{lang_title}",
                    onclick: move |_| {
                        let cur = lang();
                        let next = if cur == "zh" { "en" } else { "zh" };
                        *lang.write() = next.to_string();
                        #[cfg(target_arch = "wasm32")]
                        {
                            let _ = write_local_storage("keyc_lang", next);
                        }
                    },
                    span { class: "header-lang-btn-text", "{lang_label}" }
                }

                // 通知功能待实现，暂隐藏铃铛按钮
                // button {
                //     class: "header-icon-btn",
                //     title: "通知",
                //     IconBell { size: 18 }
                // }

                // 用户头像
                div { class: "header-avatar", title: "{user_name}", "{avatar_char}" }

                // 用户名 + 下拉箭头（桌面端）- 带下拉菜单
                div {
                    class: "header-user-dropdown",
                    style: "position: relative;",
                    button {
                        class: "header-icon-btn hide-mobile",
                        style: "gap: 4px; width: auto; padding: 0 8px;",
                        onclick: move |_| {
                            let cur = dropdown_open();
                            *dropdown_open.write() = !cur;
                        },
                        span { style: "font-size: 13px; font-weight: 500; color: var(--text-primary); max-width: 120px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;",
                            "{user_name}"
                        }
                        IconChevronDown { size: 16 }
                    }

                    // 下拉菜单
                    if dropdown_open() {
                        div {
                            class: "dropdown-menu",
                            style: "position: absolute; top: 100%; right: 0; margin-top: 4px; min-width: 160px; background: var(--bg-card, white); border: 1px solid var(--border-color, #e2e8f0); border-radius: 8px; box-shadow: 0 4px 12px rgba(0,0,0,0.15); z-index: 1000; overflow: hidden;",

                            // 个人资料
                            button {
                                class: "dropdown-item",
                                style: "display: flex; align-items: center; gap: 10px; width: 100%; padding: 10px 14px; border: none; background: none; cursor: pointer; font-size: 14px; color: var(--text-primary); text-align: left; transition: background 0.15s;",
                                onmouseenter: move |e| {
                                    let _ = e;
                                },
                                onmouseleave: move |e| {
                                    let _ = e;
                                },
                                onclick: move |_| {
                                    *dropdown_open.write() = false;
                                    on_user_menu.call(UserMenuAction::Profile);
                                },
                                IconUser { size: 16 }
                                span { "{profile_label}" }
                            }

                            // 设置
                            button {
                                class: "dropdown-item",
                                style: "display: flex; align-items: center; gap: 10px; width: 100%; padding: 10px 14px; border: none; background: none; cursor: pointer; font-size: 14px; color: var(--text-primary); text-align: left; transition: background 0.15s;",
                                onmouseenter: move |e| {
                                    let _ = e;
                                },
                                onmouseleave: move |e| {
                                    let _ = e;
                                },
                                onclick: move |_| {
                                    *dropdown_open.write() = false;
                                    on_user_menu.call(UserMenuAction::Settings);
                                },
                                IconSettings { size: 16 }
                                span { "{account_settings_label}" }
                            }

                            // 分隔线
                            div { style: "height: 1px; background: var(--border-color, #e2e8f0); margin: 4px 0;" }

                            // 退出登录
                            button {
                                class: "dropdown-item",
                                style: "display: flex; align-items: center; gap: 10px; width: 100%; padding: 10px 14px; border: none; background: none; cursor: pointer; font-size: 14px; color: var(--danger, #dc2626); text-align: left; transition: background 0.15s;",
                                onmouseenter: move |e| {
                                    let _ = e;
                                },
                                onmouseleave: move |e| {
                                    let _ = e;
                                },
                                onclick: move |_| {
                                    *dropdown_open.write() = false;
                                    on_user_menu.call(UserMenuAction::Logout);
                                },
                                IconLogOut { size: 16 }
                                span { "{logout_label}" }
                            }
                        }
                    }
                }

                // 点击外部关闭下拉菜单覆盖层
                if dropdown_open() {
                    div {
                        style: "position: fixed; inset: 0; z-index: 999;",
                        onclick: move |_| {
                            *dropdown_open.write() = false;
                        },
                    }
                }
            }
        }
    }
}

// ── localStorage 写入 ────────────────────────────
#[cfg(target_arch = "wasm32")]
fn write_local_storage(key: &str, value: &str) -> Option<()> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .set_item(key, value)
        .ok()
}

// ── 主题切换动画（与首页保持完全一致） ─────────────
#[cfg(target_arch = "wasm32")]
fn trigger_theme_switching_animation() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    if let Some(body) = document.body() {
        let class_list = body.class_list();
        let _ = class_list.add_1("kc-theme-switching");
        let body_clone = body.clone();
        let timeout = gloo_timers::callback::Timeout::new(500, move || {
            let _ = body_clone.class_list().remove_1("kc-theme-switching");
        });
        timeout.forget();
    }
}
