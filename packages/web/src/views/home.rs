use dioxus::prelude::*;

use crate::hooks::use_i18n::use_i18n;
use crate::i18n::{I18n, Lang};
use crate::router::Route;
use crate::services::api_client::{get_client, user_error_message};
use crate::services::auth_service;
use crate::services::requirement_service::{RequirementSubmission, submit_requirement};
use crate::stores::auth_store::AuthStore;
use crate::stores::public_settings_store::PublicSettingsStore;
use crate::stores::referral_store::ReferralStore;
use crate::stores::user_store::{UserInfo, UserStore};
use ui::components::modal::Modal;
use ui::{GITHUB_REPOSITORY_URL, ThemeCtx};

// ── 赞助者数据 ───────────────────────────────────
const SPONSOR_DATA: &str = include_str!("sponsors.txt");

/// 从单行解析赞助者名字，仅当金额 >= 10 且名字有效时返回
fn extract_sponsor_name(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // 跳过 "序号. " 前缀
    let content = line.split_once(". ")?.1;
    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    // 最后一个 token 为金额
    let amount: f64 = parts.last()?.parse().ok()?;
    if amount < 10.0 {
        return None;
    }
    // 剩余部分为名字
    let name = parts[..parts.len() - 1].join(" ");
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // 过滤无意义的纯标点符号/空格名字（如 "."、"~"）
    if !name
        .chars()
        .any(|c| c.is_alphabetic() || c.is_ascii_digit())
    {
        return None;
    }
    Some(name.to_string())
}

/// 首页组件 - 现代化自适应设计
#[component]
pub fn Home() -> Element {
    let nav = use_navigator();

    // 弹窗状态管理
    let mut show_login_modal = use_signal(|| false);
    let mut show_register_modal = use_signal(|| false);
    let mut show_requirement_modal = use_signal(|| false);

    // 推荐码：从 ReferralStore 中读取（由 /auth/register?ref=xxx 重定向时设置）
    let mut referral_store = use_context::<ReferralStore>();
    let mut referral_code = use_signal(|| None::<String>);

    // 如果有推荐码，自动打开注册弹窗。
    // 注意：take_code() 会读取并清空 ReferralStore 内部 signal，因而本 effect 依赖该 signal
    // 会被再次触发；但 take_code() 以 is_some() 为守卫，第二次返回 None 且不再写入，
    // 故 effect 会自然收敛，不会形成无限循环。
    use_effect(move || {
        if let Some(code) = referral_store.take_code() {
            referral_code.set(Some(code));
            show_register_modal.set(true);
        }
    });

    // 移动端菜单折叠状态
    let mut nav_menu_open = use_signal(|| false);

    // 主题状态（通过 ThemeCtx 包装，避免与 lang 的 Signal<String> 类型冲突）
    // 使用 try_use_context + fallback，确保脱离 App 根组件时不会 panic
    let ThemeCtx(mut theme) = try_use_context::<ThemeCtx>()
        .unwrap_or_else(|| ThemeCtx(use_signal(|| "dark".to_string())));
    let is_dark = theme().as_str() == "dark";

    // 语言状态：首页本地维护，从 localStorage 读取初值
    let mut lang = use_signal(|| {
        #[cfg(target_arch = "wasm32")]
        {
            read_lang_from_storage().unwrap_or_else(|| "zh".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "zh".to_string()
        }
    });
    let current_lang = lang();
    let i18n = I18n::new(Lang::from_str(&current_lang));
    let is_zh = current_lang == "zh";

    // 获取 App() 提供的语言上下文信号，用于同步到模态框
    let mut ctx_lang = use_context::<Signal<String>>();

    // 检查是否已登录（使用 memo 响应 auth_store.state 的变化）
    let auth_store = use_context::<AuthStore>();
    let is_authenticated = use_memo(move || auth_store.is_authenticated());

    // 平台名称（从公开设置读取，动态加载）
    let public_settings_store = use_context::<PublicSettingsStore>();
    let site_name = use_memo(move || {
        public_settings_store
            .site_name()
            .unwrap_or_else(|| "KeyCompute".to_string())
    });

    // 赞助者列表（编译时从 sponsors.txt 解析）
    let sponsors = use_memo(|| {
        SPONSOR_DATA
            .lines()
            .filter_map(extract_sponsor_name)
            .collect::<Vec<_>>()
    });

    // 提前提取所有 i18n 文本
    let t_toggle_theme = i18n.t("home.toggle_theme");
    let t_toggle_lang = if is_zh {
        i18n.t("layout.switch_to_en")
    } else {
        i18n.t("layout.switch_to_zh")
    };
    let t_login = i18n.t("home.login");
    let t_register = i18n.t("home.register");
    let t_tagline_1 = i18n.t("login.tagline_1");
    let t_tagline_highlight = i18n.t("login.tagline_highlight");
    let t_tagline_2 = i18n.t("login.tagline_2");
    let t_description = i18n.t("login.description");
    let t_features_title = i18n.t("home.features.title");
    let t_feature_routing = i18n.t("login.feature_routing");
    let t_feature_billing = i18n.t("login.feature_billing");
    let t_feature_ha = i18n.t("login.feature_ha");
    let t_feature_api = i18n.t("login.feature_api");
    let t_feature_metering = i18n.t("login.feature_metering");
    let t_feature_custom = i18n.t("login.feature_custom");
    let t_routing_title = i18n.t("home.features.routing.title");
    let t_routing_desc = i18n.t("home.features.routing.desc");
    let t_billing_title = i18n.t("home.features.billing.title");
    let t_billing_desc = i18n.t("home.features.billing.desc");
    let t_cluster_title = i18n.t("home.features.cluster.title");
    let t_cluster_desc = i18n.t("home.features.cluster.desc");
    let t_node_rental_title = i18n.t("home.features.node_rental.title");
    let t_node_rental_desc = i18n.t("home.features.node_rental.desc");
    let t_distribution_title = i18n.t("home.features.distribution.title");
    let t_distribution_desc = i18n.t("home.features.distribution.desc");
    let t_custom_title = i18n.t("home.features.custom.title");
    let t_custom_desc = i18n.t("home.features.custom.desc");
    let t_menu = i18n.t("home.menu");
    let t_lang_switch = i18n.t("home.lang_switch_label");
    let t_tip_title = i18n.t("home.tip.title");
    let t_tip_subtitle_prefix = i18n.t("home.tip.subtitle_prefix");
    let t_tip_subtitle_suffix = i18n.t("home.tip.subtitle_suffix");
    let t_tip_wechat_qr_alt = i18n.t("home.tip.wechat_qr_alt");
    let t_tip_wechat_label = i18n.t("home.tip.wechat_label");
    let t_tip_alipay_qr_alt = i18n.t("home.tip.alipay_qr_alt");
    let t_tip_alipay_label = i18n.t("home.tip.alipay_label");
    let t_contributors_title = i18n.t("home.contributors.title");
    let t_contributors_subtitle = i18n.t("home.contributors.subtitle");
    let t_sponsors_title = i18n.t("home.sponsors.title");
    let t_sponsors_subtitle = i18n.t("home.sponsors.subtitle");
    let t_req_bubble = i18n.t("req.bubble");

    rsx! {
        document::Title { "{site_name}" }

        div { class: "kc-home-page",
            // 背景动画效果 - 根据主题显示不同背景
            if is_dark {
                // 黑暗主题：星空背景
                div { class: "kc-home-stars-container",
                    // 生成星星（减少到50个，增大尺寸，增强缩放效果）
                    {
                        (0..50)
                            .map(|i| {
                                let style = format!(
                                    "--star-left: {}%; --star-top: {}%; --star-size: {}; --star-delay: {}s; --star-duration: {}s;",
                                    (i * 37 % 100),
                                    (i * 53 % 100),
                                    if i % 5 == 0 {
                                        "4px"
                                    } else if i % 5 == 1 {
                                        "5px"
                                    } else if i % 5 == 2 {
                                        "3px"
                                    } else {
                                        "2px"
                                    },
                                    (i % 10) as f64 * 0.5,
                                    (2 + (i % 4)) as f64,
                                );
                                rsx! {
                                    div { class: "kc-home-star", style: "{style}" }
                                }
                            })
                    }
                }
                div { class: "kc-home-shooting-stars",
                    {
                        (0..5)
                            .map(|i| {
                                let style = format!(
                                    "--shooting-delay: {}s; --shooting-duration: {}s; --shooting-top: {}%;",
                                    (i * 3) as f64 * 1.5,
                                    1.5 + (i % 3) as f64 * 0.5,
                                    10 + (i * 20) % 70,
                                );
                                rsx! {
                                    div { class: "kc-home-shooting-star", style: "{style}" }
                                }
                            })
                    }
                }
            } else {
                // 明亮主题：晴空白云
                div { class: "kc-home-sky" }
                div { class: "kc-home-clouds-container",
                    {
                        (0..8)
                            .map(|i| {
                                let style = format!(
                                    "--cloud-left: {}%; --cloud-top: {}%; --cloud-scale: {}; --cloud-delay: {}s; --cloud-duration: {}s; --cloud-opacity: {};",
                                    (i * 47 % 100),
                                    5 + (i * 23 % 60),
                                    0.4 + (i % 5) as f64 * 0.2,
                                    (i % 8) as f64 * 2.0,
                                    30.0 + (i % 5) as f64 * 10.0,
                                    0.6 + (i % 4) as f64 * 0.1,
                                );
                                rsx! {
                                    div { class: "kc-home-cloud", style: "{style}" }
                                }
                            })
                    }
                }
            }
            div { class: "kc-home-grid-overlay" }

            // 导航栏
            header { class: "kc-home-header",
                div { class: "kc-home-nav container",
                    // Logo + 标题区域
                    div { class: "kc-home-logo",
                        img {
                            src: crate::BRAND_LOGO,
                            alt: "KeyCompute",
                        }
                        span { class: "kc-home-logo-text", "{site_name}" }
                    }

                    // 移动端汉堡菜单按钮
                    button {
                        class: "kc-home-mobile-menu-btn",
                        r#type: "button",
                        title: "{t_menu}",
                        onclick: move |_| nav_menu_open.toggle(),
                        if nav_menu_open() {
                            svg {
                                width: "22",
                                height: "22",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "2",
                                stroke_linecap: "round",
                                line {
                                    x1: "18",
                                    y1: "6",
                                    x2: "6",
                                    y2: "18",
                                }
                                line {
                                    x1: "6",
                                    y1: "6",
                                    x2: "18",
                                    y2: "18",
                                }
                            }
                        } else {
                            svg {
                                width: "22",
                                height: "22",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "2",
                                stroke_linecap: "round",
                                line {
                                    x1: "3",
                                    y1: "6",
                                    x2: "21",
                                    y2: "6",
                                }
                                line {
                                    x1: "3",
                                    y1: "12",
                                    x2: "21",
                                    y2: "12",
                                }
                                line {
                                    x1: "3",
                                    y1: "18",
                                    x2: "21",
                                    y2: "18",
                                }
                            }
                        }
                    }

                    // 导航功能区（移动端可折叠）
                    div { class: if nav_menu_open() { "kc-home-nav-actions kc-home-nav-actions-open" } else { "kc-home-nav-actions" },
                        // GitHub 仓库链接
                        a {
                            class: "kc-home-github-link",
                            href: GITHUB_REPOSITORY_URL,
                            target: "_blank",
                            rel: "noopener noreferrer",
                            title: "GitHub",
                            aria_label: "GitHub",
                            svg {
                                width: "20",
                                height: "20",
                                view_box: "0 0 24 24",
                                fill: "currentColor",
                                path { d: "M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.4 3-.405 1.02.005 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" }
                            }
                        }
                        // 主题切换
                        button {
                            class: "kc-home-theme-toggle",
                            r#type: "button",
                            onclick: move |_| {
                                let new_theme = if theme().as_str() == "dark" {
                                    "light".to_string()
                                } else {
                                    "dark".to_string()
                                };
                                *theme.write() = new_theme.clone();

                                // 添加切换动画类
                                #[cfg(target_arch = "wasm32")]
                                {
                                    apply_theme_with_animation(&new_theme);
                                }
                            },
                            title: "{t_toggle_theme}",
                            if is_dark {
                                svg {
                                    width: "20",
                                    height: "20",
                                    view_box: "0 0 24 24",
                                    fill: "none",
                                    stroke: "currentColor",
                                    stroke_width: "2",
                                    path { d: "M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" }
                                }
                            } else {
                                svg {
                                    width: "20",
                                    height: "20",
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

                        // 语言切换
                        button {
                            class: "kc-home-lang-toggle",
                            r#type: "button",
                            title: "{t_toggle_lang}",
                            onclick: move |_| {
                                let new_lang = if lang().as_str() == "zh" {
                                    "en".to_string()
                                } else {
                                    "zh".to_string()
                                };
                                *lang.write() = new_lang.clone();
                                *ctx_lang.write() = new_lang.clone();
                                #[cfg(target_arch = "wasm32")]
                                {
                                    save_lang_to_storage(&new_lang);
                                }
                            },
                            span { class: "kc-home-lang-toggle-text", "{t_lang_switch}" }
                        }

                        // 登录/注册按钮 或 控制台按钮
                        if is_authenticated() {
                            button {
                                class: "kc-home-btn-register",
                                r#type: "button",
                                onclick: move |_| {
                                    nav.push(Route::Dashboard {});
                                },
                                "{i18n.t(\"home.console\")}"
                            }
                        } else {
                            div { class: "kc-home-auth-buttons",
                                button {
                                    class: "kc-home-btn-login",
                                    r#type: "button",
                                    onclick: move |_| show_login_modal.set(true),
                                    "{t_login}"
                                }
                                button {
                                    class: "kc-home-btn-register",
                                    r#type: "button",
                                    onclick: move |_| show_register_modal.set(true),
                                    "{t_register}"
                                }
                            }
                        }
                    }
                }
            }

            // 主要内容区域
            main { class: "kc-home-main",
                // Hero 区域
                section { class: "kc-home-hero",
                    div { class: "container",
                        div { class: "kc-home-hero-content",
                            h1 { class: "kc-home-hero-title",
                                span { class: "kc-home-hero-highlight",
                                    "{t_tagline_1} {t_tagline_highlight}"
                                }
                            }
                            if !t_tagline_2.is_empty() {
                                p { class: "kc-home-hero-slogan", "{t_tagline_2}" }
                            }
                            p { class: "kc-home-hero-description", "{t_description}" }

                            // 功能特性标签
                            div { class: "kc-home-features-tags",
                                span { class: "kc-home-feature-tag", "{t_feature_routing}" }
                                span { class: "kc-home-feature-tag", "{t_feature_billing}" }
                                span { class: "kc-home-feature-tag", "{t_feature_ha}" }
                                span { class: "kc-home-feature-tag", "{t_feature_api}" }
                                span { class: "kc-home-feature-tag", "{t_feature_metering}" }
                                span { class: "kc-home-feature-tag", "{t_feature_custom}" }
                            }
                        }
                    }
                }

                // 核心功能区域
                section { class: "kc-home-features",
                    div { class: "container",
                        h2 { class: "kc-home-section-title", "{t_features_title}" }
                        div { class: "kc-home-features-grid",
                            // 智能路由卡片
                            div { class: "kc-home-feature-card",
                                div { class: "kc-home-feature-icon",
                                    svg {
                                        width: "32",
                                        height: "32",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        path { d: "M22 12h-4l-3 9L9 3l-3 9H2" }
                                    }
                                }
                                h3 { "{t_routing_title}" }
                                p { "{t_routing_desc}" }
                            }

                            // 实时计费卡片
                            div { class: "kc-home-feature-card",
                                div { class: "kc-home-feature-icon",
                                    svg {
                                        width: "32",
                                        height: "32",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        line {
                                            x1: "12",
                                            y1: "1",
                                            x2: "12",
                                            y2: "23",
                                        }
                                        path { d: "M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6" }
                                    }
                                }
                                h3 { "{t_billing_title}" }
                                p { "{t_billing_desc}" }
                            }

                            // 分布式集群卡片
                            div { class: "kc-home-feature-card",
                                div { class: "kc-home-feature-icon",
                                    svg {
                                        width: "32",
                                        height: "32",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        stroke_linecap: "round",
                                        stroke_linejoin: "round",
                                        rect {
                                            x: "2",
                                            y: "3",
                                            width: "20",
                                            height: "5",
                                            rx: "1",
                                            ry: "1",
                                        }
                                        rect {
                                            x: "2",
                                            y: "10",
                                            width: "20",
                                            height: "5",
                                            rx: "1",
                                            ry: "1",
                                        }
                                        rect {
                                            x: "2",
                                            y: "17",
                                            width: "20",
                                            height: "5",
                                            rx: "1",
                                            ry: "1",
                                        }
                                        line {
                                            x1: "6",
                                            y1: "5.5",
                                            x2: "6.01",
                                            y2: "5.5",
                                        }
                                        line {
                                            x1: "6",
                                            y1: "12.5",
                                            x2: "6.01",
                                            y2: "12.5",
                                        }
                                        line {
                                            x1: "6",
                                            y1: "19.5",
                                            x2: "6.01",
                                            y2: "19.5",
                                        }
                                    }
                                }
                                h3 { "{t_cluster_title}" }
                                p { "{t_cluster_desc}" }
                            }

                            // 节点租赁卡片
                            div { class: "kc-home-feature-card",
                                div { class: "kc-home-feature-icon",
                                    svg {
                                        width: "32",
                                        height: "32",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        rect {
                                            x: "2",
                                            y: "4",
                                            width: "20",
                                            height: "12",
                                            rx: "2",
                                            ry: "2",
                                        }
                                        line {
                                            x1: "6",
                                            y1: "20",
                                            x2: "18",
                                            y2: "20",
                                        }
                                        line {
                                            x1: "12",
                                            y1: "16",
                                            x2: "12",
                                            y2: "20",
                                        }
                                        line {
                                            x1: "6",
                                            y1: "10",
                                            x2: "10",
                                            y2: "10",
                                        }
                                    }
                                }
                                h3 { "{t_node_rental_title}" }
                                p { "{t_node_rental_desc}" }
                            }

                            // 传播裂变卡片（二级分销）
                            div { class: "kc-home-feature-card",
                                div { class: "kc-home-feature-icon",
                                    svg {
                                        width: "32",
                                        height: "32",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        circle { cx: "18", cy: "5", r: "3" }
                                        circle { cx: "6", cy: "12", r: "3" }
                                        circle { cx: "18", cy: "19", r: "3" }
                                        line {
                                            x1: "8.59",
                                            y1: "13.51",
                                            x2: "15.42",
                                            y2: "17.49",
                                        }
                                        line {
                                            x1: "15.41",
                                            y1: "6.51",
                                            x2: "8.59",
                                            y2: "10.49",
                                        }
                                    }
                                }
                                h3 { "{t_distribution_title}" }
                                p { "{t_distribution_desc}" }
                            }

                            // 定制服务卡片
                            div { class: "kc-home-feature-card",
                                div { class: "kc-home-feature-icon",
                                    svg {
                                        width: "32",
                                        height: "32",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        circle { cx: "12", cy: "12", r: "3" }
                                        path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" }
                                    }
                                }
                                h3 { "{t_custom_title}" }
                                p { "{t_custom_desc}" }
                            }
                        }
                    }
                }
            }

            // 赞赏码区域
            section { class: "kc-home-tip-section",
                div { class: "container",
                    h2 { class: "kc-home-tip-title",
                        span { "☕ " }
                        "{t_tip_title}"
                    }
                    p { class: "kc-home-tip-subtitle",
                        "{t_tip_subtitle_prefix}"
                        {site_name}
                        "{t_tip_subtitle_suffix}"
                    }
                    div { class: "kc-home-tip-cards",
                        div { class: "kc-home-tip-card",
                            img {
                                class: "kc-home-tip-qr",
                                src: asset!("/assets/wechat_tip.jpg"),
                                alt: "{t_tip_wechat_qr_alt}",
                            }
                            span { class: "kc-home-tip-label", "{t_tip_wechat_label}" }
                        }
                        div { class: "kc-home-tip-card",
                            img {
                                class: "kc-home-tip-qr",
                                src: asset!("/assets/alipay_tip.png"),
                                alt: "{t_tip_alipay_qr_alt}",
                            }
                            span { class: "kc-home-tip-label", "{t_tip_alipay_label}" }
                        }
                    }
                }
            }

            // 社区贡献者区域
            section { class: "kc-home-tip-section",
                a {
                    href: "https://github.com/keycompute/keycompute/graphs/contributors?all=1",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    class: "kc-home-contributors-wrap",
                    div { class: "container",
                        h2 { class: "kc-home-tip-title",
                            span { "👥 " }
                            "{t_contributors_title}"
                        }
                        p { class: "kc-home-tip-subtitle", "{t_contributors_subtitle}" }
                        img {
                            src: "https://contrib.rocks/image?repo=keycompute/keycompute",
                            alt: "KeyCompute Contributors",
                            class: "kc-home-contributors-img",
                        }
                    }
                }
            }

            // 社区赞助者区域
            section { class: "kc-home-tip-section",
                div { class: "container",
                    h2 { class: "kc-home-tip-title",
                        span { "🌟 " }
                        "{t_sponsors_title}"
                    }
                    p { class: "kc-home-tip-subtitle", "{t_sponsors_subtitle}" }
                    div { class: "kc-home-sponsors-grid",
                        for name in sponsors() {
                            div { class: "kc-home-sponsor-card", "{name}" }
                        }
                    }
                }
            }

            // Footer
            footer { class: "footer",
                span { class: "footer-text",
                    "© 2026 "
                    a {
                        class: "footer-link",
                        href: GITHUB_REPOSITORY_URL,
                        target: "_blank",
                        rel: "noopener noreferrer",
                        {site_name}
                    }
                    ". All Rights Reserved."
                }
            }

            // 登录弹窗
            LoginModal {
                open: show_login_modal,
                onclose: move |_| show_login_modal.set(false),
                on_switch_to_register: move |_| {
                    show_login_modal.set(false);
                    show_register_modal.set(true);
                },
            }

            // 注册弹窗
            RegisterModal {
                open: show_register_modal,
                referral_code,
                onclose: move |_| show_register_modal.set(false),
                on_switch_to_login: move |_| {
                    show_register_modal.set(false);
                    show_login_modal.set(true);
                },
            }

            // 需求收集弹窗
            RequirementModal {
                open: show_requirement_modal,
                onclose: move |_| show_requirement_modal.set(false),
            }

            // 右下角常驻"需求咨询"悬浮入口；弹窗打开后隐藏，避免与遮罩层抢焦点。
            if !show_requirement_modal() {
                button {
                    class: "kc-req-bubble",
                    r#type: "button",
                    onclick: move |_| show_requirement_modal.set(true),
                    svg {
                        width: "20",
                        height: "20",
                        view_box: "0 0 24 24",
                        fill: "none",
                        stroke: "currentColor",
                        stroke_width: "2",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        path { d: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" }
                    }
                    span { "{t_req_bubble}" }
                }
            }
        }
    }
}

/// 登录弹窗组件
#[component]
fn LoginModal(
    open: ReadSignal<bool>,
    onclose: EventHandler<()>,
    on_switch_to_register: EventHandler<()>,
) -> Element {
    let i18n = use_i18n();
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let mut error_msg = use_signal(|| Option::<String>::None);
    let mut show_password = use_signal(|| false);
    let mut remember_me = use_signal(|| false);
    let mut auth_store = use_context::<AuthStore>();
    let mut user_store = use_context::<UserStore>();
    let nav = use_navigator();

    // 提取 i18n 文本
    let t_title = i18n.t("login.title");
    let t_subtitle = i18n.t("login.subtitle");
    let t_email = i18n.t("auth.email");
    let t_email_placeholder = i18n.t("auth.email_placeholder");
    let t_password = i18n.t("auth.password");
    let t_password_placeholder = i18n.t("auth.password_placeholder");
    let t_remember_me = i18n.t("auth.remember_me");
    let t_remember_me_hint = i18n.t("auth.remember_me_hint");
    let t_forgot_password = i18n.t("auth.forgot_password");
    let t_fill_all = i18n.t("auth.fill_all");
    let t_login_failed = i18n.t("auth.login_failed");
    let t_verifying = i18n.t("login.verifying");
    let t_submit = i18n.t("login.submit");
    let t_no_account = i18n.t("auth.no_account");
    let t_register_now = i18n.t("auth.register_now");

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        let email_val = email();
        let password_val = password();
        if email_val.is_empty() || password_val.is_empty() {
            error_msg.set(Some(t_fill_all.to_string()));
            return;
        }
        loading.set(true);
        error_msg.set(None);
        let should_remember = remember_me();
        spawn(async move {
            match auth_service::login(&email_val, &password_val).await {
                Ok(resp) => {
                    get_client().set_token(&resp.access_token);
                    auth_store.login_with_persist(resp.access_token.clone(), should_remember);
                    *user_store.info.write() = Some(UserInfo {
                        id: resp.user_id.clone(),
                        email: resp.email.clone(),
                        name: None,
                        role: resp.role.clone(),
                        tenant_id: resp.tenant_id.clone(),
                    });
                    onclose.call(());
                    nav.replace(Route::Dashboard {});
                }
                Err(e) => {
                    let err_text = user_error_message(&e);
                    error_msg.set(Some(format!("{t_login_failed}：{err_text}")));
                    loading.set(false);
                }
            }
        });
    };

    let password_type = if show_password() { "text" } else { "password" };

    rsx! {
        Modal {
            open,
            title: t_title.to_string(),
            onclose,
            max_width: "420px".to_string(),
            div { class: "kc-auth-modal",
                p { class: "kc-auth-modal-subtitle", "{t_subtitle}" }

                if let Some(err) = error_msg() {
                    div { class: "kc-auth-status kc-auth-status-error", "{err}" }
                }

                form { autocomplete: "on", onsubmit: on_submit,
                    div { class: "kc-auth-form-group",
                        label { class: "kc-auth-form-label", "{t_email}" }
                        input {
                            class: "kc-auth-form-input",
                            r#type: "email",
                            autocomplete: "username",
                            placeholder: "{t_email_placeholder}",
                            value: "{email}",
                            oninput: move |e| email.set(e.value()),
                        }
                    }

                    div { class: "kc-auth-form-group",
                        label { class: "kc-auth-form-label", "{t_password}" }
                        div { class: "kc-auth-password-wrapper",
                            input {
                                class: "kc-auth-form-input",
                                r#type: "{password_type}",
                                autocomplete: "current-password",
                                placeholder: "{t_password_placeholder}",
                                value: "{password}",
                                oninput: move |e| password.set(e.value()),
                            }
                            button {
                                class: "kc-auth-toggle-password",
                                r#type: "button",
                                onclick: move |_| show_password.set(!show_password()),
                                if show_password() {
                                    svg {
                                        width: "18",
                                        height: "18",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        path { d: "M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" }
                                        line {
                                            x1: "1",
                                            y1: "1",
                                            x2: "23",
                                            y2: "23",
                                        }
                                    }
                                } else {
                                    svg {
                                        width: "18",
                                        height: "18",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        path { d: "M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" }
                                        circle { cx: "12", cy: "12", r: "3" }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "kc-auth-form-options",
                        div { class: "kc-auth-remember-group",
                            label { class: "kc-auth-checkbox-label",
                                input {
                                    r#type: "checkbox",
                                    checked: remember_me(),
                                    onchange: move |e| remember_me.set(e.checked()),
                                }
                                span { "{t_remember_me}" }
                            }
                            p { class: "kc-auth-remember-hint", "{t_remember_me_hint}" }
                        }
                        button {
                            class: "kc-auth-link-btn",
                            r#type: "button",
                            onclick: move |_| {
                                onclose.call(());
                                nav.push(Route::ForgotPassword {});
                            },
                            "{t_forgot_password}"
                        }
                    }

                    button {
                        class: "kc-auth-submit-btn",
                        r#type: "submit",
                        disabled: loading(),
                        if loading() {
                            span { class: "kc-auth-spinner" }
                            " {t_verifying}"
                        } else {
                            "{t_submit}"
                        }
                    }
                }

                div { class: "kc-auth-footer",
                    "{t_no_account} "
                    button {
                        class: "kc-auth-link-btn kc-auth-link-btn-primary",
                        r#type: "button",
                        onclick: move |_| on_switch_to_register.call(()),
                        "{t_register_now}"
                    }
                }
            }
        }
    }
}

/// 注册弹窗组件
#[component]
fn RegisterModal(
    open: ReadSignal<bool>,
    referral_code: ReadSignal<Option<String>>,
    onclose: EventHandler<()>,
    on_switch_to_login: EventHandler<()>,
) -> Element {
    let i18n = use_i18n();
    let nav = use_navigator();

    let mut name = use_signal(String::new);
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut verification_code = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let mut code_requested = use_signal(|| false);
    let mut completed = use_signal(|| false);
    let mut error_msg = use_signal(|| Option::<String>::None);
    let mut success_msg = use_signal(|| Option::<String>::None);
    let mut saved_email = use_signal(String::new);
    let mut saved_password = use_signal(String::new);
    let mut auth_store = use_context::<AuthStore>();
    let mut user_store = use_context::<UserStore>();

    // 提取 i18n 文本
    let t_register = i18n.t("auth.register");
    let t_register_subtitle = i18n.t("auth.register_subtitle");
    let t_name = i18n.t("auth.name");
    let t_name_placeholder = i18n.t("auth.name_placeholder");
    let t_email = i18n.t("auth.email");
    let t_email_placeholder = i18n.t("auth.email_placeholder");
    let t_password = i18n.t("auth.password");
    let t_password_min8 = i18n.t("auth.password_min8");
    let t_confirm_password = i18n.t("auth.confirm_password");
    let t_confirm_password_placeholder = i18n.t("auth.confirm_password_placeholder");
    let t_verification_code = i18n.t("auth.verification_code");
    let t_verification_code_placeholder = i18n.t("auth.verification_code_placeholder");
    let t_code_sent_hint = i18n.t("auth.code_sent_hint");
    let t_fill_required = i18n.t("auth.fill_required");
    let t_pwd_mismatch = i18n.t("form.password_mismatch");
    let t_register_failed = i18n.t("auth.register_failed");
    let t_request_code_failed = i18n.t("auth.request_code_failed");
    let t_code_required = i18n.t("auth.code_required");
    let t_registering = i18n.t("auth.registering");
    let t_requesting_code = i18n.t("auth.requesting_code");
    let t_complete_registration = i18n.t("auth.complete_registration");
    let t_request_code = i18n.t("auth.request_code");
    let t_registration_success = i18n.t("auth.registration_success");
    let t_login_now = i18n.t("auth.login_now");
    let t_login_failed = i18n.t("auth.login_failed");
    let t_has_account = i18n.t("auth.has_account");

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        if completed() {
            return;
        }

        if !code_requested() {
            if name().is_empty() || email().is_empty() || password().is_empty() {
                error_msg.set(Some(t_fill_required.to_string()));
                return;
            }

            if password() != confirm_password() {
                error_msg.set(Some(t_pwd_mismatch.to_string()));
                return;
            }

            loading.set(true);
            error_msg.set(None);
            success_msg.set(None);

            let email_val = email();
            let ref_code = referral_code();
            spawn(async move {
                match auth_service::request_registration_code(&email_val, ref_code.as_deref()).await
                {
                    Ok(resp) => {
                        email.set(resp.email.clone());
                        code_requested.set(true);
                        success_msg.set(Some(resp.message));
                        loading.set(false);
                    }
                    Err(e) => {
                        error_msg.set(Some(format!(
                            "{t_request_code_failed}：{}",
                            user_error_message(&e)
                        )));
                        loading.set(false);
                    }
                }
            });
            return;
        }

        if verification_code().is_empty() {
            error_msg.set(Some(t_code_required.to_string()));
            return;
        }

        loading.set(true);
        error_msg.set(None);

        let name_val = name();
        let email_val = email();
        let password_val = password();
        let code_val = verification_code();

        spawn(async move {
            match auth_service::complete_registration(
                &email_val,
                &code_val,
                &password_val,
                Some(name_val.as_str()),
            )
            .await
            {
                Ok(_resp) => {
                    completed.set(true);
                    code_requested.set(false);
                    success_msg.set(Some(t_registration_success.to_string()));
                    verification_code.set(String::new());
                    saved_email.set(email_val.clone());
                    saved_password.set(password_val.clone());
                    loading.set(false);
                }
                Err(e) => {
                    error_msg.set(Some(format!(
                        "{t_register_failed}：{}",
                        user_error_message(&e)
                    )));
                    loading.set(false);
                }
            }
        });
    };

    rsx! {
        Modal {
            open,
            title: t_register.to_string(),
            onclose,
            max_width: "420px".to_string(),
            div { class: "kc-auth-modal",
                p { class: "kc-auth-modal-subtitle", "{t_register_subtitle}" }

                if let Some(msg) = success_msg() {
                    div { class: "kc-auth-status kc-auth-status-success", "{msg}" }
                }

                if let Some(err) = error_msg() {
                    div { class: "kc-auth-status kc-auth-status-error", "{err}" }
                }

                if completed() {
                    div { class: "kc-auth-success-block",
                        button {
                            class: "kc-auth-submit-btn",
                            r#type: "button",
                            disabled: loading(),
                            onclick: move |_| {
                                loading.set(true);
                                error_msg.set(None);
                                let email_val = saved_email();
                                let password_val = saved_password();
                                spawn(async move {
                                    match auth_service::login(&email_val, &password_val).await {
                                        Ok(resp) => {
                                            get_client().set_token(&resp.access_token);
                                            auth_store.login_with_persist(resp.access_token.clone(), false);
                                            *user_store.info.write() = Some(UserInfo {
                                                id: resp.user_id.clone(),
                                                email: resp.email.clone(),
                                                name: None,
                                                role: resp.role.clone(),
                                                tenant_id: resp.tenant_id.clone(),
                                            });
                                            onclose.call(());
                                            nav.replace(Route::Dashboard {});
                                        }
                                        Err(e) => {
                                            let err_text = user_error_message(&e);
                                            error_msg.set(Some(format!("{t_login_failed}：{err_text}")));
                                            loading.set(false);
                                        }
                                    }
                                });
                            },
                            if loading() {
                                span { class: "kc-auth-spinner" }
                                " {t_login_now}"
                            } else {
                                "{t_login_now}"
                            }
                        }
                    }
                } else {
                    form { onsubmit: on_submit,
                        div { class: "kc-auth-form-group",
                            label { class: "kc-auth-form-label", "{t_name}" }
                            input {
                                class: "kc-auth-form-input",
                                r#type: "text",
                                placeholder: "{t_name_placeholder}",
                                value: "{name}",
                                oninput: move |e| name.set(e.value()),
                            }
                        }

                        div { class: "kc-auth-form-group",
                            label { class: "kc-auth-form-label", "{t_email}" }
                            input {
                                class: "kc-auth-form-input",
                                r#type: "email",
                                placeholder: "{t_email_placeholder}",
                                value: "{email}",
                                disabled: code_requested(),
                                oninput: move |e| email.set(e.value()),
                            }
                        }

                        div { class: "kc-auth-form-group",
                            label { class: "kc-auth-form-label", "{t_password}" }
                            input {
                                class: "kc-auth-form-input",
                                r#type: "password",
                                placeholder: "{t_password_min8}",
                                value: "{password}",
                                oninput: move |e| password.set(e.value()),
                            }
                        }

                        div { class: "kc-auth-form-group",
                            label { class: "kc-auth-form-label", "{t_confirm_password}" }
                            input {
                                class: "kc-auth-form-input",
                                r#type: "password",
                                placeholder: "{t_confirm_password_placeholder}",
                                value: "{confirm_password}",
                                oninput: move |e| confirm_password.set(e.value()),
                            }
                        }

                        if code_requested() {
                            div { class: "kc-auth-form-group",
                                label { class: "kc-auth-form-label", "{t_verification_code}" }
                                input {
                                    class: "kc-auth-form-input",
                                    r#type: "text",
                                    maxlength: "6",
                                    placeholder: "{t_verification_code_placeholder}",
                                    value: "{verification_code}",
                                    oninput: move |e| verification_code.set(e.value()),
                                }
                            }
                            p { class: "kc-auth-hint", "{t_code_sent_hint}" }
                        }

                        button {
                            class: "kc-auth-submit-btn",
                            r#type: "submit",
                            disabled: loading(),
                            if loading() {
                                if code_requested() {
                                    "{t_registering}"
                                } else {
                                    "{t_requesting_code}"
                                }
                            } else if code_requested() {
                                "{t_complete_registration}"
                            } else {
                                "{t_request_code}"
                            }
                        }
                    }
                }

                div { class: "kc-auth-footer",
                    "{t_has_account} "
                    button {
                        class: "kc-auth-link-btn kc-auth-link-btn-primary",
                        r#type: "button",
                        onclick: move |_| on_switch_to_login.call(()),
                        "{t_login_now}"
                    }
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn apply_theme_with_animation(theme: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };

    // 设置 <html data-theme="...">
    if let Some(root) = document.document_element() {
        let _ = root.set_attribute("data-theme", theme);
    }

    // 添加切换动画类到 body，500ms 后移除
    if let Some(body) = document.body() {
        let class_list = body.class_list();
        let _ = class_list.add_1("kc-theme-switching");
        let body_clone = body.clone();
        let timeout = gloo_timers::callback::Timeout::new(500, move || {
            let _ = body_clone.class_list().remove_1("kc-theme-switching");
        });
        timeout.forget();
    }

    // 保存到 localStorage
    if let Ok(Some(storage)) = window.local_storage() {
        let _ = storage.set_item("keyc_theme", theme);
    }
}

#[cfg(target_arch = "wasm32")]
fn read_lang_from_storage() -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .get_item("keyc_lang")
        .ok()?
}

#[cfg(target_arch = "wasm32")]
fn save_lang_to_storage(lang: &str) {
    if let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok()?) {
        let _ = storage.set_item("keyc_lang", lang);
    }
}

/// 需求收集弹窗组件
///
/// 首页"提交算力需求"表单：收集需求类型、模型、规模、部署方案、联系方式与备注，
/// 提交至后端公开接口，由后端发邮件给配置的接收人。
#[component]
fn RequirementModal(open: ReadSignal<bool>, onclose: EventHandler<()>) -> Element {
    let i18n = use_i18n();

    // 表单状态
    let mut requirement_type = use_signal(|| i18n.t("req.type.api"));
    let mut model = use_signal(String::new);
    let mut usage_scale = use_signal(|| "");
    let mut deployment = use_signal(|| i18n.t("req.deploy.image"));
    let mut contact_method = use_signal(|| "wechat");
    let mut contact_value = use_signal(String::new);
    let mut note = use_signal(String::new);
    let mut loading = use_signal(|| false);
    // 联系方式字段级校验错误（显示在联系方式输入框下方）
    let mut error_msg = use_signal(|| Option::<String>::None);
    // 提交动作级错误（如网络/服务失败，显示在提交按钮上方）
    let mut submit_error = use_signal(|| Option::<String>::None);
    let mut success_message = use_signal(String::new);
    let mut success = use_signal(|| false);

    // i18n 文本
    let t_title = i18n.t("req.title");
    let t_subtitle = i18n.t("req.subtitle");
    let t_single = i18n.t("req.single_choice");
    let t_type_label = i18n.t("req.type.label");
    let t_model_label = i18n.t("req.model.label");
    let t_model_ph = i18n.t("req.model.placeholder");
    let t_scale_label = i18n.t("req.scale.label");
    let t_deploy_label = i18n.t("req.deploy.label");
    let t_deploy_image = i18n.t("req.deploy.image");
    let t_deploy_recommended = i18n.t("req.deploy.recommended");
    let t_deploy_image_desc = i18n.t("req.deploy.image_desc");
    let t_deploy_binary = i18n.t("req.deploy.binary");
    let t_deploy_binary_desc = i18n.t("req.deploy.binary_desc");
    let t_contact_label = i18n.t("req.contact.label");
    let t_note_label = i18n.t("req.note.label");
    let t_note_optional = i18n.t("req.note.optional");
    let t_note_ph = i18n.t("req.note.placeholder");
    let t_submit = i18n.t("req.submit");
    let t_submitting = i18n.t("req.submitting");
    let t_privacy = i18n.t("req.privacy");
    let t_success = i18n.t("req.success");
    let t_err_required = i18n.t("req.err.contact_required");
    let t_err_invalid = i18n.t("req.err.contact_invalid");
    let t_err_failed = i18n.t("req.err.failed");

    // 选项集合（直接存储展示文案，提交时即所见即所得）
    let type_options = vec![
        i18n.t("req.type.api"),
        i18n.t("req.type.private"),
        i18n.t("req.type.rental"),
        i18n.t("req.type.distributed"),
        i18n.t("req.type.cost"),
        i18n.t("req.type.other"),
    ];
    let scale_options = vec![
        i18n.t("req.scale.test"),
        i18n.t("req.scale.lt1w"),
        i18n.t("req.scale.10w"),
        i18n.t("req.scale.100w"),
        i18n.t("req.scale.unknown"),
    ];
    // 联系方式：(key, 标签, 占位符)
    let contact_tabs = vec![
        (
            "wechat",
            i18n.t("req.contact.wechat"),
            i18n.t("req.contact.placeholder.wechat"),
        ),
        (
            "email",
            i18n.t("req.contact.email"),
            i18n.t("req.contact.placeholder.email"),
        ),
        (
            "telegram",
            i18n.t("req.contact.telegram"),
            i18n.t("req.contact.placeholder.telegram"),
        ),
        (
            "phone",
            i18n.t("req.contact.phone"),
            i18n.t("req.contact.placeholder.phone"),
        ),
    ];

    // 当前联系方式占位符（随 tab 切换）
    let current_method = contact_method();
    let contact_placeholder = contact_tabs
        .iter()
        .find(|(k, _, _)| *k == current_method)
        .map(|(_, _, ph)| *ph)
        .unwrap_or_default();

    let deploy_image_label = t_deploy_image;
    let deploy_binary_label = t_deploy_binary;

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        submit_error.set(None);
        let contact = contact_value().trim().to_string();
        if contact.is_empty() {
            error_msg.set(Some(t_err_required.to_string()));
            return;
        }
        let method = contact_method();
        let valid = match method {
            "email" => contact.contains('@') && contact.contains('.'),
            "phone" => contact.chars().filter(|c| c.is_ascii_digit()).count() == 11,
            _ => true,
        };
        if !valid {
            error_msg.set(Some(t_err_invalid.to_string()));
            return;
        }
        error_msg.set(None);
        loading.set(true);

        let model_val = model();
        let scale_val = usage_scale();
        let note_val = note();
        let submission = RequirementSubmission {
            requirement_type: requirement_type().to_string(),
            model: if model_val.trim().is_empty() {
                None
            } else {
                Some(model_val)
            },
            usage_scale: if scale_val.is_empty() {
                None
            } else {
                Some(scale_val.to_string())
            },
            deployment: deployment().to_string(),
            contact_method: method.to_string(),
            contact_value: contact,
            note: if note_val.trim().is_empty() {
                None
            } else {
                Some(note_val)
            },
        };
        spawn(async move {
            match submit_requirement(&submission).await {
                Ok(resp) => {
                    let message = if resp.message.trim().is_empty() {
                        t_success.to_string()
                    } else {
                        resp.message
                    };
                    success_message.set(message);
                    success.set(true);
                    loading.set(false);
                }
                Err(_) => {
                    submit_error.set(Some(t_err_failed.to_string()));
                    loading.set(false);
                }
            }
        });
    };

    rsx! {
        Modal {
            open,
            title: t_title.to_string(),
            onclose,
            max_width: "620px".to_string(),
            div { class: "kc-req-modal",
                if success() {
                    div { class: "kc-req-success",
                        svg {
                            width: "48",
                            height: "48",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            path { d: "M22 11.08V12a10 10 0 1 1-5.93-9.14" }
                            path { d: "M22 4 12 14.01l-3-3" }
                        }
                        p { "{success_message}" }
                    }
                } else {
                    p { class: "kc-req-subtitle", "{t_subtitle}" }

                    form { onsubmit: on_submit,

                        // 1. 需求类型
                        div { class: "kc-req-group",
                            label { class: "kc-req-label",
                                "1. {t_type_label} "
                                span { class: "kc-req-hint", "({t_single})" }
                            }
                            div { class: "kc-req-chips",
                                {
                                    type_options
                                        .into_iter()
                                        .map(|label| {
                                            let active = requirement_type() == label;
                                            rsx! {
                                                button {
                                                    key: "{label}",
                                                    r#type: "button",
                                                    class: if active { "kc-req-chip kc-req-chip-active" } else { "kc-req-chip" },
                                                    onclick: move |_| requirement_type.set(label),
                                                    span { class: "kc-req-choice-dot" }
                                                    "{label}"
                                                }
                                            }
                                        })
                                }
                            }
                        }

                        // 2. 模型需求
                        div { class: "kc-req-group",
                            label { class: "kc-req-label", "2. {t_model_label}" }
                            input {
                                class: "kc-auth-form-input kc-req-input",
                                placeholder: "{t_model_ph}",
                                value: "{model}",
                                oninput: move |e| model.set(e.value()),
                            }
                        }

                        // 3. 预计使用规模
                        div { class: "kc-req-group",
                            label { class: "kc-req-label",
                                "3. {t_scale_label} "
                                span { class: "kc-req-hint", "({t_single})" }
                            }
                            div { class: "kc-req-chips",
                                {
                                    scale_options
                                        .into_iter()
                                        .map(|label| {
                                            let active = usage_scale() == label;
                                            rsx! {
                                                button {
                                                    key: "{label}",
                                                    r#type: "button",
                                                    class: if active { "kc-req-chip kc-req-chip-active" } else { "kc-req-chip" },
                                                    onclick: move |_| usage_scale.set(label),
                                                    span { class: "kc-req-choice-dot" }
                                                    "{label}"
                                                }
                                            }
                                        })
                                }
                            }
                        }

                        // 4. 节点部署方案
                        div { class: "kc-req-group",
                            label { class: "kc-req-label",
                                "4. {t_deploy_label} "
                                span { class: "kc-req-hint", "({t_single})" }
                            }
                            div { class: "kc-req-deploy-cards",
                                {
                                    let active = deployment() == deploy_image_label;
                                    rsx! {
                                        button {
                                            r#type: "button",
                                            class: if active { "kc-req-deploy-card kc-req-deploy-card-active" } else { "kc-req-deploy-card" },
                                            onclick: move |_| deployment.set(deploy_image_label),
                                            span { class: "kc-req-deploy-check" }
                                            div { class: "kc-req-deploy-head",
                                                span { class: "kc-req-deploy-icon kc-req-deploy-icon-image",
                                                    svg {
                                                        width: "22",
                                                        height: "22",
                                                        view_box: "0 0 24 24",
                                                        fill: "none",
                                                        stroke: "currentColor",
                                                        stroke_width: "2",
                                                        stroke_linecap: "round",
                                                        stroke_linejoin: "round",
                                                        path { d: "M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z" }
                                                        path { d: "m3.3 7 8.7 5 8.7-5" }
                                                        path { d: "M12 22V12" }
                                                    }
                                                }
                                                span { class: "kc-req-deploy-title", "{t_deploy_image}" }
                                                span { class: "kc-req-deploy-badge", "{t_deploy_recommended}" }
                                            }
                                            p { class: "kc-req-deploy-desc", "{t_deploy_image_desc}" }
                                        }
                                    }
                                }
                                {
                                    let active = deployment() == deploy_binary_label;
                                    rsx! {
                                        button {
                                            r#type: "button",
                                            class: if active { "kc-req-deploy-card kc-req-deploy-card-active" } else { "kc-req-deploy-card" },
                                            onclick: move |_| deployment.set(deploy_binary_label),
                                            span { class: "kc-req-deploy-check" }
                                            div { class: "kc-req-deploy-head",
                                                span { class: "kc-req-deploy-icon kc-req-deploy-icon-binary",
                                                    svg {
                                                        width: "22",
                                                        height: "22",
                                                        view_box: "0 0 24 24",
                                                        fill: "none",
                                                        stroke: "currentColor",
                                                        stroke_width: "2",
                                                        stroke_linecap: "round",
                                                        stroke_linejoin: "round",
                                                        path { d: "M4 17 10 11 4 5" }
                                                        path { d: "M12 19h8" }
                                                    }
                                                }
                                                span { class: "kc-req-deploy-title", "{t_deploy_binary}" }
                                            }
                                            p { class: "kc-req-deploy-desc", "{t_deploy_binary_desc}" }
                                        }
                                    }
                                }
                            }
                        }

                        // 5. 联系方式
                        div { class: "kc-req-group",
                            label { class: "kc-req-label", "5. {t_contact_label}" }
                            div { class: "kc-req-tabs",
                                {
                                    contact_tabs
                                        .into_iter()
                                        .map(|(key, label, _)| {
                                            let active = contact_method() == key;
                                            rsx! {
                                                button {
                                                    key: "{key}",
                                                    r#type: "button",
                                                    class: if active { "kc-req-tab kc-req-tab-active" } else { "kc-req-tab" },
                                                    onclick: move |_| contact_method.set(key),
                                                    span { class: "kc-req-tab-icon" }
                                                    "{label}"
                                                }
                                            }
                                        })
                                }
                            }
                            input {
                                class: "kc-auth-form-input kc-req-input",
                                placeholder: "{contact_placeholder}",
                                value: "{contact_value}",
                                oninput: move |e| contact_value.set(e.value()),
                            }
                            if let Some(err) = error_msg() {
                                p { class: "kc-req-field-error", "{err}" }
                            }
                        }

                        // 6. 补充说明
                        div { class: "kc-req-group",
                            label { class: "kc-req-label",
                                "6. {t_note_label} "
                                span { class: "kc-req-hint", "({t_note_optional})" }
                            }
                            textarea {
                                class: "kc-auth-form-input kc-req-textarea",
                                rows: "3",
                                maxlength: "500",
                                placeholder: "{t_note_ph}",
                                value: "{note}",
                                oninput: move |e| note.set(e.value()),
                            }
                        }

                        if let Some(err) = submit_error() {
                            div { class: "kc-auth-status kc-auth-status-error", "{err}" }
                        }

                        button {
                            class: "kc-auth-submit-btn",
                            r#type: "submit",
                            disabled: loading(),
                            if loading() {
                                span { class: "kc-auth-spinner" }
                                " {t_submitting}"
                            } else {
                                svg {
                                    width: "20",
                                    height: "20",
                                    view_box: "0 0 24 24",
                                    fill: "none",
                                    stroke: "currentColor",
                                    stroke_width: "2",
                                    stroke_linecap: "round",
                                    stroke_linejoin: "round",
                                    path { d: "M22 2 11 13" }
                                    path { d: "m22 2-7 20-4-9-9-4 20-7Z" }
                                }
                                "{t_submit}"
                            }
                        }

                        p { class: "kc-req-privacy", "{t_privacy}" }
                    }
                }
            }
        }
    }
}
