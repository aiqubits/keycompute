use dioxus::prelude::*;

use crate::app::{AdminLayout, AppLayout};
use crate::views::{
    Billing, Home, NotFound, Usage,
    api_keys::ApiKeyList,
    auth::{ForgotPassword, Login, Register, ResetPassword},
    dashboard::Dashboard,
    distribution::DistributionOverview,
    node::{node_earnings::NodeEarnings, node_token::NodeToken},
    payments::{PaymentsOverview, Recharge},
    shared::{
        Accounts, DistributionRecords, Monitoring, NodeGateway, PaymentOrders, Pricing, Settings,
        System, Tenants, Users,
    },
    user::{UserProfile, UserSettings},
};

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    // 首页（无 AppShell 布局，单独处理认证弹窗）
    #[route("/")]
    Home {},

    // 认证页面（无 AppShell 布局）
    #[route("/auth/login")]
    Login {},
    // 必须在路由上声明 query 段，否则 Router 初始化规范化 URL 时会把 ?ref= 参数抹掉
    #[route("/auth/register?:..query")]
    Register { query: RegisterQuery },
    #[route("/auth/forgot-password")]
    ForgotPassword {},
    #[route("/auth/reset-password/:token")]
    ResetPassword { token: String },

    // 主应用（带 AppShell 布局）
    #[layout(AppLayout)]
        #[route("/dashboard")]
        Dashboard {},
        #[route("/api-keys")]
        ApiKeyList {},
        #[route("/usage")]
        Usage {},
        #[route("/billing")]
        Billing {},
        #[route("/payments")]
        PaymentsOverview {},
        #[route("/payments/recharge")]
        Recharge {},
        #[route("/distribution")]
        DistributionOverview {},
        #[route("/user/profile")]
        UserProfile {},
        #[route("/user/settings")]
        UserSettings {},
        #[route("/node/token")]
        NodeToken {},
        #[route("/node/earnings")]
        NodeEarnings {},

        // Admin 功能页面（额外加一层 AdminLayout 做角色验证）
        #[layout(AdminLayout)]
            #[route("/admin/users")]
            Users {},
            #[route("/admin/accounts")]
            Accounts {},
            #[route("/admin/pricing")]
            Pricing {},
            #[route("/admin/payment-orders")]
            PaymentOrders {},
            #[route("/admin/distribution-records")]
            DistributionRecords {},
            #[route("/admin/tenants")]
            Tenants {},
            #[route("/admin/system")]
            System {},
            #[route("/admin/node-gateway")]
            NodeGateway {},
            #[route("/admin/monitoring")]
            Monitoring {},
            #[route("/admin/settings")]
            Settings {},
        #[end_layout]
    #[end_layout]

    // 404
    #[route("/:..route")]
    NotFound { route: Vec<String> },
}

/// 注册页 query 参数。线上邀请链接格式为 `?ref=<推荐码>`，而 `ref` 是 Rust 关键字，
/// 无法作为命名 query 段的字段声明，因此用 spread query（`?:..query`）手动解析。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RegisterQuery {
    /// 推荐码（来自 `?ref=` 参数，保证非空）
    pub ref_code: Option<String>,
}

impl From<&str> for RegisterQuery {
    fn from(query: &str) -> Self {
        let ref_code = query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == "ref" && !value.is_empty()).then(|| value.to_string())
        });
        Self { ref_code }
    }
}

impl std::fmt::Display for RegisterQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(code) = &self.ref_code {
            write!(f, "ref={code}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// 邀请链接的 ?ref= 参数必须能被解析，且序列化回 URL 时不丢失
    /// （Router 初始化时会用 to_string 规范化地址栏，丢失即意味着推荐码被抹掉）
    #[test]
    fn register_route_preserves_ref_query() {
        let url = "/auth/register?ref=15804e83-f088-44f6-8df0-982dcf8182a1";
        let route = Route::from_str(url).expect("should parse invite link");
        assert_eq!(
            route,
            Route::Register {
                query: RegisterQuery {
                    ref_code: Some("15804e83-f088-44f6-8df0-982dcf8182a1".to_string()),
                },
            }
        );
        assert_eq!(route.to_string(), url);
    }

    #[test]
    fn register_query_ignores_empty_and_unknown_keys() {
        assert_eq!(RegisterQuery::from("ref="), RegisterQuery::default());
        assert_eq!(RegisterQuery::from("foo=1"), RegisterQuery::default());
        assert_eq!(
            RegisterQuery::from("foo=1&ref=abc"),
            RegisterQuery {
                ref_code: Some("abc".to_string()),
            }
        );
    }

    /// dioxus 对 spread query 变体序列化时总会追加 '?'，query 为空时 URL
    /// 为 `/auth/register?`。浏览器视其与无 query 等价且页面会立即重定向，
    /// 此为有意接受的已知行为，用测试固定下来防止升级时静默变化
    #[test]
    fn register_without_ref_produces_trailing_question_mark() {
        let route = Route::Register {
            query: RegisterQuery::default(),
        };
        assert_eq!(route.to_string(), "/auth/register?");
    }

    #[test]
    fn nginx_node_api_rule_does_not_capture_console_routes() {
        let nginx = include_str!("../../../nginx/nginx.conf");
        assert!(nginx.contains("location /node/v1/ {"));
        assert!(!nginx.contains("location /node/ {"));
    }
}
