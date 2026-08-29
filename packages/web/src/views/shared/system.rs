use std::collections::{HashSet, VecDeque};
use std::future::Future;

use client_api::{
    api::{admin::AccountInfo, debug::RoutingDebugInfo},
    error::ClientError,
};
use dioxus::prelude::*;
use ui::{Badge, BadgeVariant, Table, TableHead};

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;
use crate::services::{account_service, api_client::with_auto_refresh, debug_service};
use crate::stores::auth_store::AuthStore;
use crate::stores::user_store::UserStore;
use crate::views::shared::accounts::NoPermissionView;

const FALLBACK_ROUTING_PROBE_MODEL: &str = "gpt-4o";
/// 将候选诊断限制在较小常数内，避免异常账号较多时一次页面加载放大调试请求。
const MAX_ROUTING_PROBES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum RoutingEntry {
    OpenAi,
    Anthropic,
}

impl RoutingEntry {
    fn for_provider(provider: &str) -> Option<Self> {
        if provider.eq_ignore_ascii_case("openai") {
            Some(Self::OpenAi)
        } else if provider.eq_ignore_ascii_case("anthropic") {
            Some(Self::Anthropic)
        } else {
            None
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RoutingProbe {
    model: String,
    entry: RoutingEntry,
}

fn fallback_routing_probes() -> Vec<RoutingProbe> {
    vec![RoutingProbe {
        model: FALLBACK_ROUTING_PROBE_MODEL.to_string(),
        entry: RoutingEntry::OpenAi,
    }]
}

/// 生成与真实协议隔离路由一致的探测候选集。
///
/// 管理员账号接口返回跨租户数据，必须应用与真实入口路由一致的租户
/// 可见性和运行状态过滤。每个账号最多贡献一个代表模型，最终候选数量还有页面级
/// 硬上限，避免账号或模型数量把一次页面加载放大为同等数量的调试请求；两种入口
/// 协议轮询取样，同一协议下重复的代表模型会继续向后寻找。定价不在前端过滤：
/// 后端 PricingService 对没有显式价格行的模型使用硬编码回退，诊断必须与真实请求
/// 保持相同语义。
///
/// 完全没有可探测账号时保留原有 `gpt-4o` OpenAI 诊断作为兜底。路由可以失败，
/// 但响应仍携带 Provider 状态表，不能因为候选为空而丢失最需要的排障信息。
fn routing_probes(accounts: &[AccountInfo], tenant_id: &str) -> Vec<RoutingProbe> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for account in accounts.iter().filter(|account| {
        account.is_active
            && account.current_rpm >= 0
            && (account.tenant_id == tenant_id || account.visibility == "global")
    }) {
        let Some(entry) = RoutingEntry::for_provider(&account.provider) else {
            continue;
        };

        for model in &account.models {
            if !model.trim().is_empty() && seen.insert((entry, model.clone())) {
                let probe = RoutingProbe {
                    model: model.clone(),
                    entry,
                };
                candidates.push(probe);
                break;
            }
        }
    }

    if candidates.is_empty() {
        return fallback_routing_probes();
    }
    if candidates.len() <= MAX_ROUTING_PROBES {
        return candidates;
    }

    // 只有需要截断时才重排，并从账号目录中的首个协议开始轮询；这样既防止
    // 探测额度被单一协议独占，也保留小目录和单协议部署的原有候选顺序。
    let first_entry = candidates[0].entry;
    let mut openai_probes = VecDeque::new();
    let mut anthropic_probes = VecDeque::new();
    for probe in candidates {
        match probe.entry {
            RoutingEntry::OpenAi => openai_probes.push_back(probe),
            RoutingEntry::Anthropic => anthropic_probes.push_back(probe),
        }
    }

    let entry_order = match first_entry {
        RoutingEntry::OpenAi => [RoutingEntry::OpenAi, RoutingEntry::Anthropic],
        RoutingEntry::Anthropic => [RoutingEntry::Anthropic, RoutingEntry::OpenAi],
    };
    let mut probes = Vec::with_capacity(MAX_ROUTING_PROBES);
    while probes.len() < MAX_ROUTING_PROBES
        && (!openai_probes.is_empty() || !anthropic_probes.is_empty())
    {
        for entry in entry_order {
            let next = match entry {
                RoutingEntry::OpenAi => openai_probes.pop_front(),
                RoutingEntry::Anthropic => anthropic_probes.pop_front(),
            };
            if let Some(probe) = next {
                probes.push(probe);
                if probes.len() == MAX_ROUTING_PROBES {
                    break;
                }
            }
        }
    }

    probes
}

/// 账号目录用于挑选更贴近真实配置的探测模型，但不能成为诊断页的单点依赖。
/// 非权限类失败时保留原有 OpenAI 兜底诊断；401/403 必须继续向外传播，分别
/// 交给 token 自动刷新和权限错误展示处理。
fn routing_probes_from_catalog(
    result: Result<Vec<AccountInfo>, ClientError>,
    tenant_id: &str,
) -> Result<Vec<RoutingProbe>, ClientError> {
    match result {
        Ok(accounts) => Ok(routing_probes(&accounts, tenant_id)),
        Err(error)
            if matches!(
                &error,
                ClientError::Unauthorized(_) | ClientError::Forbidden(_)
            ) =>
        {
            Err(error)
        }
        Err(_) => Ok(fallback_routing_probes()),
    }
}

#[derive(Debug)]
struct ProbeSearch<T, E> {
    first_diagnostic: Option<T>,
    first_error: Option<E>,
}

impl<T, E> ProbeSearch<T, E> {
    fn new() -> Self {
        Self {
            first_diagnostic: None,
            first_error: None,
        }
    }

    fn consider<P>(&mut self, result: Result<T, E>, is_routed: &P) -> Option<T>
    where
        P: Fn(&T) -> bool,
    {
        match result {
            Ok(result) if is_routed(&result) => Some(result),
            Ok(result) => {
                self.first_diagnostic.get_or_insert(result);
                None
            }
            Err(error) => {
                self.first_error.get_or_insert(error);
                None
            }
        }
    }

    fn finish(self) -> Result<Option<T>, E> {
        if let Some(result) = self.first_diagnostic {
            Ok(Some(result))
        } else if let Some(error) = self.first_error {
            Err(error)
        } else {
            Ok(None)
        }
    }
}

/// 逐个验证候选并在首个真实可路由结果处停止。
///
/// 某个看似适用的账号仍可能因冷却状态变化或无效密钥而无法生成执行计划，
/// 因此不能只选择候选列表第一项。全部失败时保留首个诊断响应用于排障；只有
/// 所有请求本身都失败时才向页面返回请求错误。
async fn first_routable_probe<C, T, E, F, Fut, P>(
    candidates: Vec<C>,
    probe: F,
    is_routed: P,
) -> Result<Option<T>, E>
where
    F: Fn(C) -> Fut,
    Fut: Future<Output = Result<T, E>>,
    P: Fn(&T) -> bool,
{
    let mut search = ProbeSearch::new();

    for candidate in candidates {
        if let Some(result) = search.consider(probe(candidate).await, &is_routed) {
            return Ok(Some(result));
        }
    }

    search.finish()
}

/// 旧“系统诊断”入口只负责兼容历史书签，统一跳转到监控中心的系统诊断视图。
fn legacy_system_destination() -> Route {
    Route::MonitoringDiagnostics {}
}

#[component]
pub fn System() -> Element {
    let i18n = use_i18n();
    let nav = use_navigator();

    use_effect(move || {
        nav.replace(legacy_system_destination());
    });

    rsx! {
        div { class: "monitoring-legacy-redirect text-secondary", {i18n.t("common.redirecting")} }
    }
}

/// 监控中心内的系统诊断视图（仅 Admin 可访问）。
///
/// 历史请求、账号/节点健康和按时间窗聚合的指标由监控概览展示；这里保留来自
/// 当前网关进程的实时状态，以及模拟路由决策、主备链路与定价检查。
#[component]
pub fn SystemDiagnostics() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    if !is_admin {
        return rsx! {
            NoPermissionView { resource: i18n.t("page.monitoring").to_string() }
        };
    }

    let tenant_id = user_store
        .info
        .read()
        .as_ref()
        .map(|user| user.tenant_id.clone())
        .unwrap_or_default();

    // 这两项来自网关进程内状态，不依赖监控聚合表。保持为独立资源，确保数据库
    // 查询异常时仍可查看当前 Provider 和网关运行状态。
    let provider_health = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            debug_service::provider_health(&token).await
        })
        .await
    });

    let gateway_stats = use_resource(move || async move {
        with_auto_refresh(auth_store, |token| async move {
            debug_service::gateway_stats(&token).await
        })
        .await
    });

    let routing_info = use_resource(move || {
        let tenant_id = tenant_id.clone();
        async move {
            with_auto_refresh(auth_store, |token| {
                let tenant_id = tenant_id.clone();
                async move {
                    let probes = routing_probes_from_catalog(
                        account_service::list(None, &token).await,
                        &tenant_id,
                    )?;

                    first_routable_probe(
                        probes,
                        |probe| {
                            let token = token.clone();
                            async move {
                                debug_service::routing(&probe.model, probe.entry.as_str(), &token)
                                    .await
                            }
                        },
                        |info: &RoutingDebugInfo| info.routed,
                    )
                    .await
                }
            })
            .await
        }
    });

    rsx! {
        DiagnosticsSections {
            provider_health: rsx! {
                match provider_health() {
                    None => rsx! {
                        p { class: "text-secondary", {i18n.t("table.loading")} }
                    },
                    Some(Err(ref error)) => rsx! {
                        div { class: "alert alert-error",
                            "{i18n.t(\"common.load_failed\")}: {error}"
                        }
                    },
                    Some(Ok(ref response)) => rsx! {
                        div { class: "health-grid",
                            for name in response.healthy_providers.iter() {
                                HealthItem { name: name.clone() }
                            }
                            if response.healthy_providers.is_empty() {
                                p { class: "text-secondary", {i18n.t("system.no_healthy_provider")} }
                            }
                        }
                    },
                }
            },
            gateway_stats: rsx! {
                match gateway_stats() {
                    None => rsx! {
                        p { class: "text-secondary", {i18n.t("table.loading")} }
                    },
                    Some(Err(ref error)) => rsx! {
                        div { class: "alert alert-error",
                            "{i18n.t(\"common.load_failed\")}: {error}"
                        }
                    },
                    Some(Ok(ref stats)) => rsx! {
                        GatewayRuntimeStats {
                            total_requests: stats.total_requests,
                            successful_requests: stats.successful_requests,
                            avg_latency_ms: stats.avg_latency_ms,
                            fallback_count: stats.fallback_count,
                        }
                    },
                }
            },
            routing_info: rsx! {
                h3 { class: "subsection-title section-body-title", {i18n.t("system.provider_status_diagnosis")} }
                match routing_info() {
                        None => rsx! {
                            p { class: "text-secondary", {i18n.t("table.loading")} }
                        },
                        Some(Err(ref e)) => rsx! {
                            div { class: "alert alert-error",
                                p { "{i18n.t(\"common.load_failed\")}: {e}" }
                            }
                        },
                        Some(Ok(None)) => rsx! {
                            div { class: "alert alert-warning",
                                p { "{i18n.t(\"system.no_routable_probe_model\")}" }
                            }
                        },
                        Some(Ok(Some(ref info))) => rsx! {
                            div {
                                // 路由结果
                                if info.routed {
                                    div { class: "alert alert-success",
                                        p { "✓ {i18n.t(\"system.route_success\")}" }
                                        if let Some(ref primary) = info.primary {
                                            p { class: "text-sm",
                                                "{i18n.t(\"system.primary_target\")}: {primary.provider} ({primary.endpoint})"
                                            }

                                        }
                                        if !info.fallback_chain.is_empty() {
                                            p { class: "text-sm",
                                                "{i18n.t(\"system.fallback_chain\")}: {info.fallback_chain.len()} {i18n.t(\"system.items\")}"
                                            }
                                        }
                                    }
                                } else {
                                    div { class: "alert alert-warning",
                                        p { "✗ {i18n.t(\"system.route_failed\")}" }
                                        if let Some(ref msg) = info.message {
                                            p { class: "text-sm", "{msg}" }
                                        }
                                    }
                                }

                                // Provider 状态表格
                                h4 { class: "subsection-title", {i18n.t("system.provider_status")} }
                                Table {
                                    empty: info.provider_status.is_empty(),
                                    empty_text: i18n.t("system.no_provider_configured"),
                                    col_count: 3,
                                    thead {
                                        tr {
                                            TableHead { {i18n.t("system.provider_column")} }
                                            TableHead { {i18n.t("system.health_status")} }
                                            TableHead { {i18n.t("system.account_count")} }
                                        }
                                    }
                                    tbody {
                                        for ps in info.provider_status.iter() {
                                            tr {
                                                td { "{ps.provider}" }
                                                td {
                                                    if ps.is_healthy {
                                                        Badge { variant: BadgeVariant::Success, {i18n.t("system.healthy")} }
                                                    } else {
                                                        Badge { variant: BadgeVariant::Error, {i18n.t("system.unhealthy")} }
                                                    }
                                                }
                                                td { "{ps.account_count}" }
                                            }
                                        }
                                    }
                                }

                                // 定价信息
                                h4 { class: "subsection-title", {i18n.t("system.pricing_info")} }
                                div { class: "info-grid",
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("pricing.model_name")} }
                                        span { class: "info-value", "{info.pricing.model_name}" }
                                    }
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("common.currency")} }
                                        span { class: "info-value", "{info.pricing.currency}" }
                                    }
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("pricing.input_price")} }
                                        span { class: "info-value", "{info.pricing.input_price_per_1k} / 1K tokens" }
                                    }
                                    div { class: "info-item",
                                        span { class: "info-label", {i18n.t("pricing.output_price")} }
                                        span { class: "info-value", "{info.pricing.output_price_per_1k} / 1K tokens" }
                                    }
                                }
                            }
                        },
                }
            },
        }
    }
}

/// 诊断区块采用独立内容槽位。任一区块失败时只替换自己的内容，不能阻断另外两项
/// 来自不同数据源的运行状态。
#[component]
fn DiagnosticsSections(
    provider_health: Element,
    gateway_stats: Element,
    routing_info: Element,
) -> Element {
    let i18n = use_i18n();

    rsx! {
        div { class: "system-diagnostics",
            div { class: "section",
                h2 { class: "section-title", {i18n.t("system.provider_health")} }
                div { class: "section-body", {provider_health} }
            }
            div { class: "section",
                h2 { class: "section-title", {i18n.t("system.gateway_stats")} }
                div { class: "section-body", {gateway_stats} }
            }
            div { class: "section",
                h2 { class: "section-title", {i18n.t("system.routing_debug")} }
                div { class: "section-body", {routing_info} }
            }
        }
    }
}

#[component]
fn HealthItem(name: String) -> Element {
    let i18n = use_i18n();
    rsx! {
        div { class: "health-item",
            div { class: "health-name", "{name}" }
            div { class: "health-status",
                Badge { variant: BadgeVariant::Success, {i18n.t("system.healthy")} }
            }
        }
    }
}

#[component]
fn GatewayRuntimeStats(
    total_requests: i64,
    successful_requests: i64,
    avg_latency_ms: u64,
    fallback_count: i64,
) -> Element {
    let i18n = use_i18n();
    let success_rate = format!(
        "{:.1}%",
        successful_requests as f64 / total_requests.max(1) as f64 * 100.0
    );

    rsx! {
        div { class: "stats-grid",
            RuntimeStatCard { label: i18n.t("system.total_requests").to_string(), value: total_requests.to_string() }
            RuntimeStatCard { label: i18n.t("system.success_rate").to_string(), value: success_rate }
            RuntimeStatCard { label: i18n.t("system.avg_latency").to_string(), value: format!("{avg_latency_ms}ms") }
            RuntimeStatCard { label: i18n.t("system.fallback_count").to_string(), value: fallback_count.to_string() }
        }
    }
}

#[component]
fn RuntimeStatCard(label: String, value: String) -> Element {
    rsx! {
        div { class: "stat-card",
            p { class: "stat-label", "{label}" }
            p { class: "stat-value", "{value}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticsSections, FALLBACK_ROUTING_PROBE_MODEL, MAX_ROUTING_PROBES, ProbeSearch,
        RoutingEntry, RoutingProbe, routing_probes, routing_probes_from_catalog,
    };
    use client_api::api::admin::AccountInfo;
    use client_api::error::ClientError;
    use dioxus::history::{History, MemoryHistory};
    use dioxus::prelude::*;
    use dioxus::router::components::HistoryProvider;
    use std::rc::Rc;

    const TENANT: &str = "11111111-1111-1111-1111-111111111111";
    const OTHER_TENANT: &str = "22222222-2222-2222-2222-222222222222";

    #[derive(Clone, Debug, Routable, PartialEq)]
    enum RedirectHarnessRoute {
        #[route("/admin/system")]
        LegacySystemHarness {},
        #[route("/admin/monitoring/diagnostics")]
        MergedDiagnosticsHarness {},
    }

    #[component]
    fn LegacySystemHarness() -> Element {
        rsx! { super::System {} }
    }

    #[component]
    fn MergedDiagnosticsHarness() -> Element {
        rsx! { div { "merged-diagnostics" } }
    }

    #[derive(Clone)]
    struct SharedHistory(Rc<MemoryHistory>);

    impl PartialEq for SharedHistory {
        fn eq(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    #[component]
    fn RedirectHarness(history: SharedHistory) -> Element {
        rsx! {
            HistoryProvider {
                history: move |_| history.0.clone() as Rc<dyn History>,
                Router::<RedirectHarnessRoute> {}
            }
        }
    }

    #[test]
    fn legacy_system_route_replaces_history_with_the_merged_diagnostics_view() {
        let history = Rc::new(MemoryHistory::with_initial_path("/admin/system"));
        let mut dom = VirtualDom::new_with_props(
            RedirectHarness,
            RedirectHarnessProps {
                history: SharedHistory(history.clone()),
            },
        );

        dom.rebuild_in_place();
        dom.render_immediate(&mut dioxus::prelude::dioxus_core::NoOpMutations);

        assert_eq!(history.current_route(), "/admin/monitoring/diagnostics");
        assert!(!history.can_go_back(), "redirect must replace, not push");
    }

    fn diagnostics_failure_fixture() -> Element {
        let provider = "provider-runtime-ready".to_string();
        let gateway = "gateway-runtime-ready".to_string();
        let routing = "routing-database-failed".to_string();

        rsx! {
            DiagnosticsSections {
                provider_health: rsx! { p { "{provider}" } },
                gateway_stats: rsx! { p { "{gateway}" } },
                routing_info: rsx! { p { "{routing}" } },
            }
        }
    }

    #[test]
    fn runtime_sections_render_when_database_backed_routing_fails() {
        let mut dom = VirtualDom::new(diagnostics_failure_fixture);
        let mutations = dom.rebuild_to_vec();
        let rendered_text = mutations
            .edits
            .iter()
            .filter_map(|edit| match edit {
                dioxus::prelude::dioxus_core::Mutation::CreateTextNode { value, .. } => {
                    Some(value.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(rendered_text.contains(&"provider-runtime-ready"));
        assert!(rendered_text.contains(&"gateway-runtime-ready"));
        assert!(rendered_text.contains(&"routing-database-failed"));
    }

    fn account(
        tenant_id: &str,
        visibility: &str,
        provider: &str,
        is_active: bool,
        current_rpm: i32,
        models: &[&str],
    ) -> AccountInfo {
        AccountInfo {
            id: format!("account-{tenant_id}-{provider}"),
            tenant_id: tenant_id.to_string(),
            name: "Test account".to_string(),
            provider: provider.to_string(),
            api_key_preview: "sk-***".to_string(),
            api_base: None,
            models: models.iter().map(|model| (*model).to_string()).collect(),
            rpm_limit: 60,
            current_rpm,
            is_active,
            is_healthy: true,
            priority: 1,
            visibility: visibility.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used_at: None,
        }
    }

    #[test]
    fn probe_catalog_uses_current_tenant_accounts_without_requiring_pricing_rows() {
        let accounts = vec![
            account(
                TENANT,
                "tenant",
                "openai",
                true,
                0,
                &["tenant-priced", "fallback-priced", ""],
            ),
            account(
                OTHER_TENANT,
                "global",
                "openai",
                true,
                0,
                &["global-visible"],
            ),
            account(
                OTHER_TENANT,
                "tenant",
                "openai",
                true,
                0,
                &["other-private"],
            ),
            account(TENANT, "tenant", "anthropic", true, 0, &["anthropic-only"]),
            account(TENANT, "tenant", "openai", false, 0, &["disabled"]),
            account(TENANT, "tenant", "openai", true, -1, &["cooling"]),
        ];

        assert_eq!(
            routing_probes(&accounts, TENANT),
            vec![
                RoutingProbe {
                    model: "tenant-priced".to_string(),
                    entry: RoutingEntry::OpenAi,
                },
                RoutingProbe {
                    model: "global-visible".to_string(),
                    entry: RoutingEntry::OpenAi,
                },
                RoutingProbe {
                    model: "anthropic-only".to_string(),
                    entry: RoutingEntry::Anthropic,
                },
            ]
        );
    }

    #[test]
    fn probe_catalog_uses_one_unique_representative_per_account() {
        let many_models = (0..100)
            .map(|index| format!("model-{index}"))
            .collect::<Vec<_>>();
        let accounts = vec![AccountInfo {
            id: "many-models".to_string(),
            tenant_id: TENANT.to_string(),
            name: "Many models".to_string(),
            provider: "openai".to_string(),
            api_key_preview: "sk-***".to_string(),
            api_base: None,
            models: many_models,
            rpm_limit: 60,
            current_rpm: 0,
            is_active: true,
            is_healthy: true,
            priority: 1,
            visibility: "tenant".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used_at: None,
        }];

        assert_eq!(
            routing_probes(&accounts, TENANT),
            vec![RoutingProbe {
                model: "model-0".to_string(),
                entry: RoutingEntry::OpenAi,
            }]
        );
    }

    #[test]
    fn probe_catalog_keeps_protocols_separate_and_avoids_duplicate_requests() {
        let accounts = vec![
            account(TENANT, "tenant", "openai", true, 0, &["shared"]),
            account(
                TENANT,
                "tenant",
                "openai",
                true,
                0,
                &["shared", "openai-second"],
            ),
            account(TENANT, "tenant", "anthropic", true, 0, &["shared"]),
        ];

        assert_eq!(
            routing_probes(&accounts, TENANT),
            vec![
                RoutingProbe {
                    model: "shared".to_string(),
                    entry: RoutingEntry::OpenAi,
                },
                RoutingProbe {
                    model: "openai-second".to_string(),
                    entry: RoutingEntry::OpenAi,
                },
                RoutingProbe {
                    model: "shared".to_string(),
                    entry: RoutingEntry::Anthropic,
                },
            ]
        );
    }

    #[test]
    fn probe_catalog_caps_requests_without_starving_either_protocol() {
        let mut accounts = (0..MAX_ROUTING_PROBES)
            .map(|index| {
                account(
                    TENANT,
                    "tenant",
                    "anthropic",
                    true,
                    0,
                    &[&format!("anthropic-{index}")],
                )
            })
            .collect::<Vec<_>>();
        accounts.extend((0..MAX_ROUTING_PROBES).map(|index| {
            account(
                TENANT,
                "tenant",
                "openai",
                true,
                0,
                &[&format!("openai-{index}")],
            )
        }));

        let probes = routing_probes(&accounts, TENANT);

        assert_eq!(probes.len(), MAX_ROUTING_PROBES);
        assert!(
            probes
                .iter()
                .step_by(2)
                .all(|probe| probe.entry == RoutingEntry::Anthropic)
        );
        assert!(
            probes
                .iter()
                .skip(1)
                .step_by(2)
                .all(|probe| probe.entry == RoutingEntry::OpenAi)
        );
    }

    #[test]
    fn probe_catalog_allows_a_single_protocol_to_use_the_full_limit() {
        let accounts = (0..MAX_ROUTING_PROBES + 2)
            .map(|index| {
                account(
                    TENANT,
                    "tenant",
                    "openai",
                    true,
                    0,
                    &[&format!("openai-{index}")],
                )
            })
            .collect::<Vec<_>>();

        let probes = routing_probes(&accounts, TENANT);

        assert_eq!(probes.len(), MAX_ROUTING_PROBES);
        assert!(
            probes
                .iter()
                .all(|probe| probe.entry == RoutingEntry::OpenAi)
        );
    }

    #[test]
    fn probe_catalog_retains_a_diagnostic_fallback_when_no_account_is_eligible() {
        let accounts = vec![
            account(TENANT, "tenant", "openai", false, 0, &["disabled"]),
            account(TENANT, "tenant", "anthropic", true, -1, &["cooling"]),
        ];

        assert_eq!(
            routing_probes(&accounts, TENANT),
            vec![RoutingProbe {
                model: FALLBACK_ROUTING_PROBE_MODEL.to_string(),
                entry: RoutingEntry::OpenAi,
            }]
        );
    }

    #[test]
    fn probe_catalog_failure_falls_back_without_hiding_access_errors() {
        assert_eq!(
            routing_probes_from_catalog(
                Err(ClientError::ServerError("catalog unavailable".to_string())),
                TENANT,
            )
            .unwrap(),
            vec![RoutingProbe {
                model: FALLBACK_ROUTING_PROBE_MODEL.to_string(),
                entry: RoutingEntry::OpenAi,
            }]
        );

        let unauthorized = routing_probes_from_catalog(
            Err(ClientError::Unauthorized("expired".to_string())),
            TENANT,
        )
        .unwrap_err();
        assert!(matches!(unauthorized, ClientError::Unauthorized(_)));

        let forbidden = routing_probes_from_catalog(
            Err(ClientError::Forbidden("admin required".to_string())),
            TENANT,
        )
        .unwrap_err();
        assert!(matches!(forbidden, ClientError::Forbidden(_)));
    }

    #[test]
    fn probe_search_prefers_routable_results_and_keeps_the_first_failure() {
        let mut search = ProbeSearch::<String, String>::new();
        assert!(
            search
                .consider(Ok("first failure".into()), &|result| result == "routable")
                .is_none()
        );
        assert!(
            search
                .consider(Err("request error".into()), &|_| false)
                .is_none()
        );
        assert_eq!(
            search
                .consider(Ok("routable".into()), &|result| result == "routable")
                .as_deref(),
            Some("routable")
        );

        let mut failures = ProbeSearch::<String, String>::new();
        failures.consider(Ok("first failure".into()), &|_| false);
        failures.consider(Ok("second failure".into()), &|_| false);
        assert_eq!(failures.finish().unwrap().as_deref(), Some("first failure"));
    }
}
