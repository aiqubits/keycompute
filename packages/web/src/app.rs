use dioxus::prelude::*;

use crate::i18n::{I18n, Lang};
use crate::router::Route;
use crate::services::{api_client::get_client, settings_service, user_service};
use crate::stores::{
    auth_store::AuthStore,
    public_settings_store::{PublicSettingsState, PublicSettingsStore},
    referral_store::ReferralStore,
    ui_store::{ToastMsg, UiStore},
    user_store::{UserInfo, UserStore},
};
use crate::views::shared::Toast;
use ui::layout::sidebar::NavIcon;
use ui::{AppShell, NavItem, NavSection, ThemeCtx, UserMenuAction};

/// 根组件：提供所有全局 context，挂载路由
#[component]
pub fn App() -> Element {
    // 所有 Signal 必须在组件顶层直接创建，不能在 hook 的闭包里调用 use_signal
    let auth_initial = AuthStore::load_from_storage();
    let auth_state = use_signal(|| auth_initial);
    let user_info = use_signal(|| None::<UserInfo>);
    let user_load_failed = use_signal(|| false);
    let public_settings_state = use_signal(PublicSettingsState::default);
    let toast_signal = use_signal(|| None::<ToastMsg>);
    let lang_signal = use_signal(|| {
        #[cfg(target_arch = "wasm32")]
        {
            read_local_storage("keyc_lang").unwrap_or_else(|| "zh".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "zh".to_string()
        }
    });
    let theme_signal = use_signal(|| {
        #[cfg(target_arch = "wasm32")]
        {
            read_local_storage("keyc_theme").unwrap_or_else(|| "dark".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "dark".to_string()
        }
    });

    let auth_store = use_context_provider(|| AuthStore::new(auth_state));
    let mut user_store = use_context_provider(|| UserStore::new(user_info, user_load_failed));
    let public_settings_store =
        use_context_provider(|| PublicSettingsStore::new(public_settings_state));
    let _ui_store = use_context_provider(|| UiStore::new(toast_signal));
    let _lang = use_context_provider(|| lang_signal);
    let _theme = use_context_provider(|| ThemeCtx(theme_signal));

    // 推荐码存储：分销链接 /auth/register?ref=xxx 中的推荐码在重定向到首页时通过此 store 传递
    let referral_code_signal = use_signal(|| None::<String>);
    let _referral_store = use_context_provider(|| ReferralStore::new(referral_code_signal));

    // 应用启动时同步主题到 HTML data-theme 属性
    use_effect(move || {
        let theme = theme_signal();
        #[cfg(target_arch = "wasm32")]
        {
            apply_theme_to_html(&theme);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = theme;
        }
    });

    use_effect(move || {
        if public_settings_store.loaded() {
            return;
        }

        spawn(async move {
            let mut store = public_settings_store;
            match settings_service::get_public().await {
                Ok(settings) => store.set(settings),
                Err(_) => store.mark_loaded(),
            }
        });
    });

    // App 启动时或登录状态变化时，若已有 token，自动拉取用户信息
    use_effect(move || {
        // 依赖 auth_store 的认证状态，登录/登出时会重新执行
        let is_auth = auth_store.is_authenticated();
        if !is_auth {
            user_store.load_failed.set(false);
            return;
        }

        let Some(token) = auth_store.token() else {
            return;
        };

        // 恢复 token 到 API 客户端
        get_client().set_token(&token);
        user_store.load_failed.set(false);
        spawn(async move {
            match user_service::get_current_user(&token).await {
                Ok(user) => {
                    *user_store.info.write() = Some(UserInfo {
                        id: user.id.to_string(),
                        email: user.email,
                        name: user.name,
                        role: user.role,
                        tenant_id: user.tenant_id.to_string(),
                    });
                    user_store.load_failed.set(false);
                }
                Err(err) if err.is_auth_error() => {
                    let mut auth_store = auth_store;
                    auth_store.logout();
                    get_client().clear_token();
                    *user_store.info.write() = None;
                    user_store.load_failed.set(false);
                }
                Err(_) => user_store.load_failed.set(true),
            }
        });
    });

    rsx! {
        Router::<Route> {}
    }
}

#[cfg(target_arch = "wasm32")]
fn read_local_storage(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(key)
        .ok()?
}

#[cfg(target_arch = "wasm32")]
fn apply_theme_to_html(theme: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(root) = document.document_element() else {
        return;
    };
    let _ = root.set_attribute("data-theme", theme);
    let _ = root.set_attribute("data-build", &build_tag());
}

/// 前端构建标识：命名空间前缀 + 版本号，用于缓存排障与构建溯源
#[cfg(target_arch = "wasm32")]
fn build_tag() -> String {
    const NS: [u8; 4] = [0x6b, 0x65, 0x79, 0x63];
    let ns = std::str::from_utf8(&NS).unwrap_or("app");
    format!("{ns}-{}", env!("CARGO_PKG_VERSION"))
}

/// 带 AppShell 侧边栏布局的页面外壳
/// 内含路由守卫：未登录时立即重定向到登录页，避免闪屏
#[component]
pub fn AppLayout() -> Element {
    let user_store = use_context::<UserStore>();
    let public_settings_store = use_context::<PublicSettingsStore>();
    let mut auth_store = use_context::<AuthStore>();
    let ui_store = use_context::<UiStore>();
    let lang_signal = use_context::<Signal<String>>();
    let i18n = I18n::new(Lang::from_str(&lang_signal()));
    let nav = use_navigator();
    let mut user_store_write = use_context::<UserStore>();

    // 同步检查认证状态：在渲染之前立即判断，未登录则渲染重定向占位符
    // 同时通过 use_effect 执行实际导航（Dioxus 要求导航在 effect 中进行）
    let is_auth = auth_store.is_authenticated();

    use_effect(move || {
        if !auth_store.is_authenticated() {
            nav.replace(Route::Login {});
        }
    });

    // 未登录时渲染全屏加载态，use_effect 会在下一帧立即触发跳转
    // 避免将受保护页面内容闪现给未认证用户
    if !is_auth {
        return rsx! {
            div {
                class: "auth-redirect-loading",
                style: "display:flex;align-items:center;justify-content:center;height:100vh;background:var(--bg-primary,#f8fafc)",
                div { style: "display:flex;flex-direction:column;align-items:center;gap:12px",
                    div {
                        class: "spinner",
                        style: "width:32px;height:32px",
                        role: "status",
                        "aria-label": "{i18n.t(\"common.redirecting\")}",
                    }
                    span { style: "color:var(--text-secondary,#64748b);font-size:14px",
                        {i18n.t("common.redirect_to_login")}
                    }
                }
            }
        };
    }

    let is_admin = user_store.is_admin();
    let user_name = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.display_name().to_string())
        .unwrap_or_default();

    // 路径由 Route 枚举派生，避免硬编码字符串拼写错误
    let r_dashboard = Route::Dashboard {}.to_string();
    let r_api_keys = Route::ApiKeyList {}.to_string();
    let r_usage = Route::Usage {}.to_string();
    let r_payments = Route::PaymentsOverview {}.to_string();
    let r_distribution = Route::DistributionOverview {}.to_string();
    let r_profile = Route::UserProfile {}.to_string();
    let r_settings = Route::UserSettings {}.to_string();
    let r_node_token = Route::NodeToken {}.to_string();
    let r_node_earnings = Route::NodeEarnings {}.to_string();
    let r_admin_users = Route::Users {}.to_string();
    let r_admin_accounts = Route::Accounts {}.to_string();
    let r_admin_pricing = Route::Pricing {}.to_string();
    let r_admin_payment_orders = Route::PaymentOrders {}.to_string();
    let r_admin_distribution = Route::DistributionRecords {}.to_string();
    let r_admin_tenants = Route::Tenants {}.to_string();
    let r_admin_node_gateway = Route::NodeGateway {}.to_string();
    let r_admin_monitoring = Route::Monitoring {}.to_string();
    let r_admin_system_settings = Route::Settings {}.to_string();
    let site_name = public_settings_store
        .site_name()
        .unwrap_or_else(|| "KeyCompute".to_string());
    let site_logo_src = public_settings_store
        .site_logo_url()
        .unwrap_or_else(|| crate::BRAND_LOGO.to_string());
    let show_distribution_nav =
        public_settings_store.loaded() && public_settings_store.distribution_is_enabled();

    let mut billing_nav_items = vec![NavItem::new(
        i18n.t("nav.payments"),
        r_payments,
        NavIcon::Wallet,
    )];
    if show_distribution_nav {
        billing_nav_items.push(NavItem::new(
            i18n.t("nav.distribution"),
            r_distribution,
            NavIcon::Share,
        ));
    }

    let mut nav_sections = vec![
        NavSection {
            title: None,
            items: vec![
                NavItem::new(i18n.t("page.home"), r_dashboard, NavIcon::Home),
                NavItem::new(i18n.t("nav.api_keys"), r_api_keys, NavIcon::Key),
            ],
        },
        NavSection {
            title: Some(i18n.t("nav.group.usage").to_string()),
            items: vec![NavItem::new(
                i18n.t("nav.usage"),
                r_usage,
                NavIcon::BarChart,
            )],
        },
        NavSection {
            title: Some(i18n.t("nav.group.billing").to_string()),
            items: billing_nav_items,
        },
        NavSection {
            title: Some(i18n.t("nav.group.account").to_string()),
            items: vec![
                NavItem::new(i18n.t("nav.user.profile"), r_profile, NavIcon::User),
                NavItem::new(
                    i18n.t("nav.account_settings"),
                    r_settings,
                    NavIcon::Settings,
                ),
            ],
        },
        // 节点分组（所有已登录用户可见）
        NavSection {
            title: Some(i18n.t("nav.group.node").to_string()),
            items: vec![
                NavItem::new(i18n.t("nav.node_token"), r_node_token, NavIcon::Key),
                NavItem::new(
                    i18n.t("nav.node_earnings"),
                    r_node_earnings,
                    NavIcon::Wallet,
                ),
            ],
        },
    ];

    // Admin 专属导航分组（仅 admin 角色可见）
    if is_admin {
        nav_sections.push(NavSection {
            title: Some(i18n.t("nav.group.admin").to_string()),
            items: vec![
                NavItem::new(i18n.t("nav.users"), r_admin_users, NavIcon::User).admin(),
                NavItem::new(i18n.t("nav.accounts"), r_admin_accounts, NavIcon::Key).admin(),
                NavItem::new(i18n.t("nav.pricing"), r_admin_pricing, NavIcon::Wallet).admin(),
                NavItem::new(
                    i18n.t("nav.payment_orders"),
                    r_admin_payment_orders,
                    NavIcon::Wallet,
                )
                .admin(),
                NavItem::new(
                    i18n.t("nav.distribution_records"),
                    r_admin_distribution,
                    NavIcon::Share,
                )
                .admin(),
                NavItem::new(i18n.t("nav.tenants"), r_admin_tenants, NavIcon::Home).admin(),
                NavItem::new(
                    i18n.t("nav.node_gateway"),
                    r_admin_node_gateway,
                    NavIcon::Server,
                )
                .admin(),
                NavItem::new(
                    i18n.t("nav.monitoring"),
                    r_admin_monitoring,
                    NavIcon::Activity,
                )
                .admin(),
                NavItem::new(
                    i18n.t("nav.settings"),
                    r_admin_system_settings,
                    NavIcon::Settings,
                )
                .admin(),
            ],
        });
    }

    let current_route = use_route::<Route>();
    let current_path = current_route.to_string();
    let page_title = route_page_title(&current_route, &i18n);
    let document_title = format!("{page_title} · {site_name}");

    rsx! {
        document::Title { "{document_title}" }

        AppShell {
            nav_sections,
            user_name,
            current_path,
            site_name,
            site_logo_src,
            home_title: i18n.t("layout.back_to_home"),
            open_menu_title: i18n.t("layout.open_menu"),
            close_menu_title: i18n.t("layout.close_menu"),
            switch_to_light_theme_title: i18n.t("layout.switch_to_light"),
            switch_to_dark_theme_title: i18n.t("layout.switch_to_dark"),
            switch_to_zh_title: i18n.t("layout.switch_to_zh"),
            switch_to_en_title: i18n.t("layout.switch_to_en"),
            profile_label: i18n.t("nav.user.profile"),
            user_menu_label: i18n.t("layout.user_menu"),
            account_settings_label: i18n.t("nav.account_settings"),
            logout_label: i18n.t("auth.logout"),
            expand_sidebar_title: i18n.t("layout.expand_sidebar"),
            collapse_sidebar_title: i18n.t("layout.collapse_sidebar"),
            expand_label: i18n.t("common.expand"),
            collapse_label: i18n.t("common.collapse"),
            on_user_menu: move |action: UserMenuAction| match action {
                UserMenuAction::Profile => {
                    nav.push(Route::UserProfile {});
                }
                UserMenuAction::Settings => {
                    nav.push(Route::UserSettings {});
                }
                UserMenuAction::Logout => {
                    auth_store.logout();
                    // 清除 API 客户端 token
                    get_client().clear_token();
                    // 清空用户信息，避免登出后旧数据残留
                    *user_store_write.info.write() = None;
                    nav.replace(Route::Home {});
                }
            },
            Toast { toast: ui_store.toast }
            Outlet::<Route> {}
        }
    }
}

/// Admin 专属路由守卫层
///
/// 嵌套在 AppLayout 内部，仅允许 admin 角色访问 /admin/* 页面。
/// 非 admin 用户会被重定向到首页，同时显示无权提示。
#[component]
pub fn AdminLayout() -> Element {
    let user_store = use_context::<UserStore>();
    let mut ui_store = use_context::<UiStore>();
    let lang_signal = use_context::<Signal<String>>();
    let i18n = I18n::new(Lang::from_str(&lang_signal()));
    let nav = use_navigator();

    let is_admin = user_store.is_admin();
    // 用户信息已加载（info 不为 None）时才做判断，避免初始化闪屏
    let info_loaded = user_store.info.read().is_some();
    let info_load_failed = (user_store.load_failed)();

    use_effect(move || {
        if info_loaded && !user_store.is_admin() {
            ui_store.show_error(i18n.t("common.admin_only_page"));
            nav.replace(Route::Dashboard {});
        }
    });

    // 用户信息尚未加载完成，显示等待占位符
    if !info_loaded && info_load_failed {
        return rsx! {
            div { class: "admin-guard-error alert alert-error",
                p { {i18n.t("common.user_info_load_failed")} }
                button {
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    onclick: move |_| reload_current_page(),
                    {i18n.t("common.retry")}
                }
            }
        };
    }

    if !info_loaded {
        return rsx! {
            div {
                class: "admin-guard-loading",
                style: "display:flex;align-items:center;justify-content:center;padding:60px",
                div { class: "spinner", style: "width:24px;height:24px" }
            }
        };
    }

    // 已加载但不是 admin，显示空内容（effect 会立即跳转）
    if !is_admin {
        return rsx! {};
    }

    rsx! {
        Outlet::<Route> {}
    }
}

fn route_page_title(route: &Route, i18n: &I18n) -> String {
    let key = match route {
        Route::Dashboard {} => "page.home",
        Route::ApiKeyList {} => "page.api_keys",
        Route::Usage {} => "page.usage",
        Route::Billing {} => "page.billing",
        Route::PaymentsOverview {} => "page.payments",
        Route::Recharge {} => "recharge.title",
        Route::DistributionOverview {} => "page.distribution",
        Route::UserProfile {} => "page.profile",
        Route::UserSettings {} => "page.account_settings",
        Route::NodeToken {} => "page.node_token",
        Route::NodeEarnings {} => "page.node_earnings",
        Route::Users {} => "page.users",
        Route::Accounts {} => "page.accounts",
        Route::Pricing {} => "page.pricing",
        Route::PaymentOrders {} => "page.payment_orders",
        Route::DistributionRecords {} => "page.distribution_records",
        Route::Tenants {} => "page.tenants",
        Route::System {} => "page.monitoring",
        Route::NodeGateway {} => "page.node_gateway",
        Route::Monitoring {} => "page.monitoring",
        Route::MonitoringDiagnostics {} => "page.monitoring",
        Route::Settings {} => "page.settings",
        _ => "page.not_found",
    };
    i18n.t(key).to_string()
}

fn reload_current_page() {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

#[cfg(test)]
mod tests {
    use super::route_page_title;
    use crate::{
        i18n::{I18n, Lang},
        router::Route,
    };

    #[test]
    fn every_console_route_has_a_document_title() {
        let i18n = I18n::new(Lang::En);
        let routes = [
            Route::Dashboard {},
            Route::ApiKeyList {},
            Route::Usage {},
            Route::Billing {},
            Route::PaymentsOverview {},
            Route::Recharge {},
            Route::DistributionOverview {},
            Route::UserProfile {},
            Route::UserSettings {},
            Route::NodeToken {},
            Route::NodeEarnings {},
            Route::Users {},
            Route::Accounts {},
            Route::Pricing {},
            Route::PaymentOrders {},
            Route::DistributionRecords {},
            Route::Tenants {},
            Route::System {},
            Route::NodeGateway {},
            Route::Monitoring {},
            Route::MonitoringDiagnostics {},
            Route::Settings {},
        ];

        for route in routes {
            let title = route_page_title(&route, &i18n);
            assert!(!title.is_empty());
            assert_ne!(title, "?");
        }
    }

    #[test]
    fn observability_routes_share_the_merged_page_title() {
        let i18n = I18n::new(Lang::Zh);
        let merged_title = route_page_title(&Route::Monitoring {}, &i18n);

        assert_eq!(
            route_page_title(&Route::MonitoringDiagnostics {}, &i18n),
            merged_title
        );
        assert_eq!(route_page_title(&Route::System {}, &i18n), merged_title);
    }

    #[test]
    fn custom_console_dialogs_expose_dialog_roles() {
        let sources = [
            include_str!("views/api_keys/list.rs"),
            include_str!("views/node/node_earnings.rs"),
            include_str!("views/shared/accounts.rs"),
            include_str!("views/shared/node_gateway.rs"),
            include_str!("views/shared/pricing.rs"),
            include_str!("views/shared/users.rs"),
        ];

        for source in sources {
            for modal in source.split("class: \"modal\"").skip(1) {
                let attributes = &modal[..modal.len().min(180)];
                assert!(
                    attributes.contains("role:"),
                    "custom modal is missing a dialog role: {attributes}"
                );
                assert!(
                    attributes.contains("aria_modal:"),
                    "custom modal is missing aria-modal: {attributes}"
                );
            }

            let close_chunks = source.split("\"✕\"").collect::<Vec<_>>();
            for before_close in close_chunks
                .iter()
                .take(close_chunks.len().saturating_sub(1))
            {
                let attributes = before_close
                    .rsplit("button {")
                    .next()
                    .unwrap_or(before_close);
                assert!(
                    attributes.contains("aria_label:"),
                    "custom modal close button is missing an accessible name: {attributes}"
                );
            }
        }
    }
}
