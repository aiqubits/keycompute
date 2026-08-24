//! Routing Engine
//!
//! 路由引擎，双层路由，只读无副作用。
//! 架构约束：只读 Pricing 和状态快照，不写任何状态。
//! 包含 Provider 健康状态管理和账号状态管理。

pub mod account_state;
pub mod provider_health;

pub use account_state::{AccountState, AccountStateStore};
use keycompute_db::{Account, DbRouter};
use keycompute_runtime::{EncryptedApiKey, decrypt_api_key};
use keycompute_types::{
    ExecutionPlan, ExecutionTarget, KeyComputeError, PricingSnapshot, RequestContext, Result,
};
pub use provider_health::{ProviderHealth, ProviderHealthStore};
use sea_orm::ConnectionTrait;
use std::sync::Arc;
use uuid::Uuid;

/// Node 能力索引 trait
///
/// 用于路由引擎检查是否存在 ready 节点
/// 实现方可以是 PostgresNodeIndex 或 mock 实现
///
/// **ready predicate** (与 poll 领取资格使用同一套服务端可验证数据):
/// - `nodes.status = 'online'` (隐含 `consecutive_failure_count < failure_threshold`)
/// - `node_sessions.expires_at > NOW()` (session 未过期)
/// - `node_sessions.revoked_at IS NULL` (session 未撤销)
/// - `nodes.capabilities_json->>'runtime' = 'ollama'` (runtime 类型匹配)
/// - `node_sessions.accepted_models_json` 包含目标模型名 (已在 heartbeat 时校验为注册能力的子集)
/// - 不读取 Redis，不使用客户端自报的负载或本地失败计数
#[async_trait::async_trait]
pub trait NodeCapabilityIndex: Send + Sync {
    /// 检查是否存在 ready 节点可以处理指定模型
    ///
    /// 该方法为异步，因为实际实现需要执行数据库查询 (I/O 操作)。
    async fn has_ready_node(&self, model: &str) -> bool;
}

/// 路由权重常量（硬编码，不可通过配置修改）
const COST_WEIGHT: f64 = 0.3;
const LATENCY_WEIGHT: f64 = 0.25;
const SUCCESS_WEIGHT: f64 = 0.25;
const HEALTH_WEIGHT: f64 = 0.2;
const UNHEALTHY_PENALTY: f64 = 100.0;
const HIGH_LATENCY_THRESHOLD_MS: u64 = 1000;

/// 每个 Provider（协议）最多选入执行计划的账号数
///
/// 协议收敛为两种后，同一协议下可能挂载多个厂商的账号（如 OpenAI 官方 +
/// DeepSeek + Ollama），选入 top-N 账号作为 fallback 链，保持原多厂商回退能力
const MAX_ACCOUNTS_PER_PROVIDER: usize = 3;

/// 路由引擎
///
/// 双层路由：Layer1 模型路由，Layer2 账号路由
/// 集成 ProviderHealthStore 进行健康评分路由
/// 集成 AccountStateStore 进行账号冷却状态检查
/// 集成 NodeCapabilityIndex 进行 Node 路由支持
#[derive(Clone)]
pub struct RoutingEngine {
    /// 账号状态存储（只读）
    account_states: Arc<AccountStateStore>,
    /// Provider 健康状态存储（只读）
    provider_health: Arc<ProviderHealthStore>,
    /// 数据库连接池（可选）
    pool: Option<Arc<DbRouter>>,
    /// 可用 Provider 列表
    providers: Vec<String>,
    /// Node 能力索引（可选）
    node_index: Option<Arc<dyn NodeCapabilityIndex>>,
}

impl std::fmt::Debug for RoutingEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingEngine")
            .field("account_states", &"AccountStateStore")
            .field("provider_health", &"ProviderHealthStore")
            .field("pool", &"DatabaseConnection")
            .field("providers", &self.providers)
            .field(
                "node_index",
                &self.node_index.as_ref().map(|_| "NodeCapabilityIndex"),
            )
            .finish()
    }
}

impl RoutingEngine {
    /// 创建新的路由引擎（无数据库连接）
    ///
    /// # 参数
    /// - `account_states`: 账号状态存储
    /// - `provider_health`: Provider 健康状态存储
    /// - `providers`: Provider 名称列表（从外部传入，确保与 Gateway 一致）
    pub fn new(
        account_states: Arc<AccountStateStore>,
        provider_health: Arc<ProviderHealthStore>,
        providers: Vec<String>,
    ) -> Self {
        Self {
            account_states,
            provider_health,
            pool: None,
            providers,
            node_index: None,
        }
    }

    /// 创建带数据库连接的路由引擎
    ///
    /// # 参数
    /// - `account_states`: 账号状态存储
    /// - `provider_health`: Provider 健康状态存储
    /// - `pool`: 数据库连接池
    /// - `providers`: Provider 名称列表（从外部传入，确保与 Gateway 一致）
    pub fn with_pool(
        account_states: Arc<AccountStateStore>,
        provider_health: Arc<ProviderHealthStore>,
        pool: Arc<DbRouter>,
        providers: Vec<String>,
    ) -> Self {
        Self {
            account_states,
            provider_health,
            pool: Some(pool),
            providers,
            node_index: None,
        }
    }

    /// 创建带 Node 能力索引的路由引擎
    ///
    /// # 参数
    /// - `account_states`: 账号状态存储
    /// - `provider_health`: Provider 健康状态存储
    /// - `pool`: 数据库连接池
    /// - `providers`: Provider 名称列表（从外部传入，确保与 Gateway 一致）
    /// - `node_index`: Node 能力索引，用于检查是否存在 ready 节点
    pub fn with_node_index(
        account_states: Arc<AccountStateStore>,
        provider_health: Arc<ProviderHealthStore>,
        pool: Arc<DbRouter>,
        providers: Vec<String>,
        node_index: Arc<dyn NodeCapabilityIndex>,
    ) -> Self {
        Self {
            account_states,
            provider_health,
            pool: Some(pool),
            providers,
            node_index: Some(node_index),
        }
    }

    /// 生成执行计划（只读操作）
    ///
    /// 根据 RequestContext 路由到最优的 Provider 和账号
    /// 使用租户专属账号池进行路由
    ///
    /// **Node 路由支持**:
    /// - 检测 `model.starts_with("node:")` 前缀
    /// - 去掉前缀得到 actual_model,调用 `node_index.has_ready_node(actual_model)`
    /// - 存在 ready 节点: 返回 `ExecutionTarget::Node { model: actual_model }`
    /// - 不存在: 返回 `NoReadyNode` 错误,不 fallback
    /// - 无前缀: 走现有 Provider 路由逻辑
    ///
    /// **注**: Node 路径支持 stream=true，流式由 handler 层的 `simulate_node_stream()`
    /// 模拟实现（获取完整响应后按块拆分输出），不经过 GatewayExecutor
    pub async fn route(&self, ctx: &RequestContext) -> Result<ExecutionPlan> {
        tracing::info!(
            request_id = %ctx.request_id,
            model = %ctx.model,
            tenant_id = %ctx.tenant_id,
            "route: starting"
        );

        // 检测 Node 路由前缀
        if let Some(actual_model) = ctx.model.strip_prefix("node:") {
            tracing::info!(
                request_id = %ctx.request_id,
                model = %ctx.model,
                actual_model = %actual_model,
                "route: node prefix detected"
            );

            // 检查是否配置了 node_index
            let node_index = self.node_index.as_ref().ok_or_else(|| {
                tracing::error!("route: node_index not configured");
                KeyComputeError::Internal("Node routing not configured".to_string())
            })?;

            // 检查是否存在 ready 节点
            if node_index.has_ready_node(actual_model).await {
                tracing::info!(
                    request_id = %ctx.request_id,
                    model = %actual_model,
                    "route: ready node found, routing to node path"
                );
                return Ok(ExecutionPlan {
                    primary: ExecutionTarget::Node {
                        model: actual_model.to_string(),
                    },
                    fallback_chain: Vec::new(),
                });
            } else {
                tracing::warn!(
                    request_id = %ctx.request_id,
                    model = %actual_model,
                    "route: no ready node available"
                );
                return Err(KeyComputeError::NoReadyNode(actual_model.to_string()));
            }
        }

        // Layer1: 模型路由 - 选择 provider 排序
        // 入口协议隔离：Anthropic 入站（带原生请求体）只从 anthropic 协议账号选路，
        // OpenAI 兼容入站只从 openai 协议账号选路；本协议下无可用账号时直接失败，
        // 不跨协议兜底（避免原生 Anthropic 字段在协议转换中丢失，或 OpenAI 请求
        // 意外落到 Anthropic 上游）。
        let entry_protocol = if ctx.native_anthropic_request.is_some() {
            "anthropic"
        } else {
            "openai"
        };
        let ranked_providers = self
            .rank_providers(&ctx.model, &ctx.pricing_snapshot, entry_protocol)
            .await?;

        tracing::info!(
            request_id = %ctx.request_id,
            ranked_providers = ?ranked_providers,
            "route: providers ranked"
        );

        // Layer2: 账号路由 - 为每个 provider 选择租户专属的最优账号（top-N）
        // 关键改进：只选择支持请求模型的账号；同协议下多个账号（多厂商）
        // 按优先级依次进入 fallback 链，保持原多厂商回退能力
        let mut targets = Vec::new();
        for provider in ranked_providers {
            // 传入 tenant_id 和 model 确保使用租户专属账号池且账号支持该模型
            tracing::info!(
                request_id = %ctx.request_id,
                provider = %provider,
                "route: selecting accounts"
            );
            let provider_targets = self
                .select_account_for_model(&provider, ctx.tenant_id, &ctx.model)
                .await?;
            if provider_targets.is_empty() {
                tracing::info!(
                    request_id = %ctx.request_id,
                    provider = %provider,
                    "route: no account found"
                );
            } else {
                tracing::info!(
                    request_id = %ctx.request_id,
                    provider = %provider,
                    accounts_count = provider_targets.len(),
                    "route: accounts selected"
                );
                targets.extend(provider_targets);
            }
        }

        if targets.is_empty() {
            tracing::error!(
                request_id = %ctx.request_id,
                "route: no targets found, routing failed"
            );
            return Err(KeyComputeError::RoutingFailed(ctx.model.clone()));
        }

        let primary_provider = match &targets[0] {
            ExecutionTarget::ProviderAccount { provider, .. } => provider.clone(),
            ExecutionTarget::Node { model } => format!("node:{}", model),
        };
        tracing::info!(
            request_id = %ctx.request_id,
            primary_provider = %primary_provider,
            targets_count = targets.len(),
            "route: completed"
        );

        Ok(ExecutionPlan {
            primary: targets.remove(0),
            fallback_chain: targets,
        })
    }

    /// Layer1: 模型路由
    ///
    /// 根据模型、价格、延迟、失败率、不健康度对 Provider 排序
    /// 注意：暂时不过滤不健康的 Provider，所有 Provider 都参与路由
    /// 评分规则：所有指标统一为"越高越不优先"，最终分数越低越优先
    /// 综合评分 = weighted_average + unhealthy_penalty
    ///
    /// 入口协议隔离：候选 Provider 仅限入口协议本身（openai / anthropic），
    /// 不跨协议兜底；本协议未配置账号时返回空列表，由 route 报 RoutingFailed
    async fn rank_providers(
        &self,
        _model: &str,
        pricing: &PricingSnapshot,
        entry_protocol: &str,
    ) -> Result<Vec<String>> {
        // 注意：暂时不过滤不健康的 Provider，所有 Provider 都参与路由
        // 健康状态仅用于评分排序，不用于过滤
        let _healthy_providers = self.provider_health.healthy_providers(&self.providers);

        // 候选 Provider 仅限入口协议本身
        let candidates: Vec<&String> = self
            .providers
            .iter()
            .filter(|p| p.eq_ignore_ascii_case(entry_protocol))
            .collect();

        // 计算每个 Provider 的综合评分
        let mut scored_providers: Vec<(String, f64)> = candidates
            .iter()
            .map(|p| {
                let score = self.score_provider(p, pricing);
                ((*p).clone(), score)
            })
            .collect();

        // 按分数排序（分数越低越好）
        scored_providers.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        tracing::debug!(
            provider_scores = ?scored_providers,
            "Provider ranking completed"
        );

        Ok(scored_providers.into_iter().map(|(p, _)| p).collect())
    }

    /// 计算 Provider 综合评分
    ///
    /// 评分规则：所有指标统一为"越高越不优先"
    /// - 成本越高 → 分数越高
    /// - 延迟越高 → 分数越高
    /// - 成功率越低 → 分数越高
    /// - 健康度越低 → 分数越高
    /// - 不健康 → 额外惩罚分
    ///   最终分数越低越优先选择
    fn score_provider(&self, provider: &str, pricing: &PricingSnapshot) -> f64 {
        // 1. 成本评分 (0-100，越高越不优先)
        let cost_score = self.calculate_cost_score(pricing);

        // 2. 从 ProviderHealthStore 获取健康状态
        let health = self.provider_health.get_health(provider);

        // 3. 延迟评分 (0-100，越高越不优先)
        let latency_score = health
            .as_ref()
            .map(|h| self.calculate_latency_score(h.avg_latency_ms))
            .unwrap_or(50.0); // 默认中等延迟

        // 4. 失败率评分 (0-100，越高越不优先)
        // 成功率越高 → 失败率越低 → 分数越低（越好）
        let failure_score = health
            .as_ref()
            .map(|h| 100.0 - h.success_rate)
            .unwrap_or(0.0); // 默认 0 失败率

        // 5. 不健康度评分 (0-100，越高越不健康)
        // health_score() 越高 → 越健康 → 不健康度越低（越好）
        let unhealthiness_score = health
            .as_ref()
            .map(|h| 100.0 - h.health_score() as f64)
            .unwrap_or(50.0); // 默认中等

        // 6. 不健康额外惩罚
        let unhealthy_penalty = health
            .as_ref()
            .filter(|h| !h.healthy)
            .map(|_| UNHEALTHY_PENALTY)
            .unwrap_or(0.0);

        // 7. 综合评分（加权平均）
        let total_weight = COST_WEIGHT + LATENCY_WEIGHT + SUCCESS_WEIGHT + HEALTH_WEIGHT;
        let weighted_score = (COST_WEIGHT * cost_score
            + LATENCY_WEIGHT * latency_score
            + SUCCESS_WEIGHT * failure_score
            + HEALTH_WEIGHT * unhealthiness_score)
            / total_weight;

        let final_score = weighted_score + unhealthy_penalty;

        tracing::debug!(
            provider = %provider,
            cost_score = cost_score,
            latency_score = latency_score,
            failure_score = failure_score,
            unhealthiness_score = unhealthiness_score,
            unhealthy_penalty = unhealthy_penalty,
            final_score = final_score,
            "Provider scored (lower is better)"
        );

        final_score
    }

    /// 计算成本评分
    fn calculate_cost_score(&self, pricing: &PricingSnapshot) -> f64 {
        // 将价格转换为 f64，价格越高分数越高（越不优先）
        let input_price: f64 = pricing
            .input_price_per_1k
            .to_string()
            .parse()
            .unwrap_or(1.0);
        let output_price: f64 = pricing
            .output_price_per_1k
            .to_string()
            .parse()
            .unwrap_or(2.0);

        // 归一化到 0-100 范围（假设价格范围 0-10）
        let avg_price = (input_price + output_price) / 2.0;
        (avg_price * 10.0).min(100.0)
    }

    /// 计算延迟评分
    fn calculate_latency_score(&self, latency_ms: u64) -> f64 {
        if latency_ms == 0 {
            // 无延迟数据，返回中等分数
            50.0
        } else if latency_ms < 100 {
            10.0 // 优秀
        } else if latency_ms < 300 {
            30.0 // 良好
        } else if latency_ms < HIGH_LATENCY_THRESHOLD_MS {
            60.0 // 一般
        } else {
            90.0 // 较差
        }
    }

    /// Layer2: 账号路由（带模型过滤）
    ///
    /// 为指定 Provider 选择支持特定模型的账号（top-N）
    /// 按优先级排序，跳过冷却中的账号，最多返回 MAX_ACCOUNTS_PER_PROVIDER 个
    /// 注意：暂时不检查 Provider 健康状态，所有 Provider 都可以选择账号
    ///
    /// # 参数
    /// - `provider`: Provider 名称
    /// - `tenant_id`: 租户 ID，用于选择租户专属账号池
    /// - `model`: 请求的模型名称，用于过滤支持该模型的账号
    async fn select_account_for_model(
        &self,
        provider: &str,
        tenant_id: Uuid,
        model: &str,
    ) -> Result<Vec<ExecutionTarget>> {
        // 注意：暂时不检查 Provider 健康状态
        // 即使 Provider 不健康，仍然尝试选择其下的账号
        // 健康状态仅影响 Layer1 的路由排序
        let _is_healthy = self.provider_health.is_healthy(provider);

        // 尝试从数据库加载租户专属账号
        let accounts = if let Some(pool) = &self.pool {
            // 根据是否指定模型选择不同的加载方式
            let result = if model.is_empty() {
                self.load_accounts_from_database(pool.as_ref(), provider, tenant_id)
                    .await
            } else {
                self.load_accounts_for_model(pool.as_ref(), provider, tenant_id, model)
                    .await
            };

            match result {
                Ok(accounts) => accounts,
                Err(e) => {
                    tracing::warn!(
                        provider = %provider,
                        tenant_id = %tenant_id,
                        model = %model,
                        error = %e,
                        "Failed to load accounts from database, using fallback"
                    );
                    return self.select_fallback_account(provider).await;
                }
            }
        } else {
            // 无数据库连接，使用回退逻辑
            return self.select_fallback_account(provider).await;
        };

        // 从账号列表中选择最优账号（top-N）
        self.select_best_accounts(provider, accounts).await
    }

    /// 从数据库加载租户可见的账号（含本租户 + 全局可见）
    ///
    /// # 参数
    /// - `pool`: 数据库连接池
    /// - `provider`: Provider 名称
    /// - `tenant_id`: 租户 ID
    ///
    /// # 返回
    /// 返回该租户专属的 + 全局可见的、支持指定 provider 的启用账号列表
    async fn load_accounts_from_database(
        &self,
        pool: &impl ConnectionTrait,
        provider: &str,
        tenant_id: Uuid,
    ) -> Result<Vec<Account>> {
        // 加载租户专属的启用账号
        let accounts = Account::find_enabled_by_tenant(pool, tenant_id)
            .await
            .map_err(|e| {
                KeyComputeError::DatabaseError(format!("Failed to load accounts: {}", e))
            })?;

        // 过滤出指定 provider 的账号
        let provider_accounts: Vec<Account> = accounts
            .into_iter()
            .filter(|a| a.provider == provider)
            .collect();

        tracing::debug!(
            provider = %provider,
            tenant_id = %tenant_id,
            count = provider_accounts.len(),
            "Loaded visible accounts from database"
        );

        Ok(provider_accounts)
    }

    /// 从数据库加载支持指定模型的可见账号（含本租户 + 全局可见）
    ///
    /// # 参数
    /// - `pool`: 数据库连接池
    /// - `provider`: Provider 名称
    /// - `tenant_id`: 租户 ID
    /// - `model`: 请求的模型名称
    ///
    /// # 返回
    /// 返回该租户可见的、支持指定模型和 provider 的启用账号列表
    async fn load_accounts_for_model(
        &self,
        pool: &impl ConnectionTrait,
        provider: &str,
        tenant_id: Uuid,
        model: &str,
    ) -> Result<Vec<Account>> {
        // 直接使用模型查询，更高效
        let accounts = Account::find_by_model(pool, tenant_id, model)
            .await
            .map_err(|e| {
                KeyComputeError::DatabaseError(format!("Failed to load accounts: {}", e))
            })?;

        // 过滤出指定 provider 的账号
        let provider_accounts: Vec<Account> = accounts
            .into_iter()
            .filter(|a| a.provider == provider)
            .collect();

        tracing::debug!(
            provider = %provider,
            tenant_id = %tenant_id,
            model = %model,
            count = provider_accounts.len(),
            "Loaded visible accounts for model from database"
        );

        Ok(provider_accounts)
    }

    /// 选择账号（top-N）
    ///
    /// 按优先级排序，跳过冷却中的账号，最多返回 MAX_ACCOUNTS_PER_PROVIDER 个
    /// 第一个为主选，其余作为同协议下的 fallback（多厂商回退）
    async fn select_best_accounts(
        &self,
        provider: &str,
        accounts: Vec<Account>,
    ) -> Result<Vec<ExecutionTarget>> {
        tracing::info!(
            provider = %provider,
            accounts_count = accounts.len(),
            "select_best_accounts: starting"
        );

        if accounts.is_empty() {
            tracing::warn!(provider = %provider, "No accounts available");
            return Ok(Vec::new());
        }

        // 按优先级排序
        let mut sorted_accounts: Vec<_> = accounts.into_iter().collect();
        sorted_accounts.sort_by_key(|account| std::cmp::Reverse(account.priority));

        let mut targets = Vec::new();
        for account in sorted_accounts {
            if targets.len() >= MAX_ACCOUNTS_PER_PROVIDER {
                break;
            }

            // 检查账号是否在冷却中
            if self.account_states.is_cooling_down(&account.id) {
                let remaining = self.account_states.get(&account.id).cooldown_remaining();
                tracing::warn!(
                    provider = %provider,
                    account_id = %account.id,
                    remaining_secs = remaining.map(|d| d.as_secs()),
                    "Account is cooling down, skipping"
                );
                continue;
            }

            // 解密上游 API Key：单个账号的坏密钥不应中止整条路由，
            // 否则低优先级账号的脏数据会拖垂本可经健康账号成功的请求
            let upstream_api_key =
                match Self::decrypt_upstream_api_key(&account.upstream_api_key_encrypted) {
                    Ok(key) => key,
                    Err(e) => {
                        tracing::warn!(
                            provider = %provider,
                            account_id = %account.id,
                            error = %e,
                            "Failed to decrypt upstream API key, skipping account"
                        );
                        continue;
                    }
                };

            // 空 endpoint 表示使用协议默认端点（与创建/测试接口的语义保持一致，
            // 否则默认预设创建的账号会在执行时因相对 URL 而失败）
            let endpoint = if account.endpoint.is_empty() {
                match llm_protocol_provider::ProtocolType::parse(provider) {
                    Some(p) => p.default_endpoint().to_string(),
                    None => {
                        tracing::warn!(
                            provider = %provider,
                            account_id = %account.id,
                            "Account has empty endpoint and unknown protocol, skipping"
                        );
                        continue;
                    }
                }
            } else {
                account.endpoint
            };

            tracing::info!(
                provider = %provider,
                account_id = %account.id,
                "Account selected"
            );

            targets.push(ExecutionTarget::new_provider(
                provider.to_string(),
                account.id,
                endpoint,
                upstream_api_key,
            ));
        }

        Ok(targets)
    }

    /// 解密上游 API Key
    ///
    /// 尝试解密存储的 API Key。如果全局加密密钥未设置，
    /// 说明系统可能还在使用明文存储，此时回退使用原始值。
    fn decrypt_upstream_api_key(encrypted_value: &str) -> Result<String> {
        // 先检查全局加密密钥是否已设置，避免与并行测试中的竞态
        if keycompute_runtime::global_crypto().is_none() {
            // 全局密钥未设置，回退使用原始值（可能存储的是明文）
            tracing::warn!(
                "Global crypto key not set, using stored value as plaintext. \n\
                 This is acceptable for development but should be fixed in production."
            );
            return Ok(encrypted_value.to_string());
        }

        // 全局密钥已设置，尝试解密
        match decrypt_api_key(&EncryptedApiKey::from(encrypted_value)) {
            Ok(decrypted) => {
                tracing::trace!("Successfully decrypted upstream API key");
                Ok(decrypted)
            }
            Err(e) => {
                // 其他解密错误
                tracing::error!(error = %e, "Failed to decrypt upstream API key");
                Err(KeyComputeError::Internal(format!(
                    "Failed to decrypt upstream API key: {}",
                    e
                )))
            }
        }
    }

    /// 回退账号选择（无数据库时使用）
    async fn select_fallback_account(&self, provider: &str) -> Result<Vec<ExecutionTarget>> {
        let account_id = Uuid::new_v4();

        // 检查账号是否在冷却中
        if self.account_states.is_cooling_down(&account_id) {
            let remaining = self.account_states.get(&account_id).cooldown_remaining();
            tracing::debug!(
                provider = %provider,
                account_id = %account_id,
                remaining_secs = remaining.map(|d| d.as_secs()),
                "Account is cooling down, skipping"
            );
            return Ok(Vec::new());
        }

        // 构建执行目标（endpoint 为 Base URL，路径由协议层拼接）
        let target = ExecutionTarget::new_provider(
            provider.to_string(),
            account_id,
            format!("https://api.{}.com/v1", provider),
            "mock-api-key",
        );

        Ok(vec![target])
    }

    /// 获取 Provider 健康状态存储（只读访问）
    pub fn provider_health(&self) -> &Arc<ProviderHealthStore> {
        &self.provider_health
    }

    /// 获取指定 Provider 的健康评分
    pub fn get_provider_health_score(&self, provider: &str) -> u64 {
        self.provider_health.get_score(provider)
    }

    /// 检查 Provider 是否健康
    pub fn is_provider_healthy(&self, provider: &str) -> bool {
        self.provider_health.is_healthy(provider)
    }

    /// 获取账号状态存储（只读访问）
    pub fn account_states(&self) -> &Arc<AccountStateStore> {
        &self.account_states
    }

    /// 检查账号是否在冷却中
    pub fn is_account_cooling(&self, account_id: &Uuid) -> bool {
        self.account_states.is_cooling_down(account_id)
    }

    /// 获取账号冷却剩余时间
    pub fn account_cooldown_remaining(&self, account_id: &Uuid) -> Option<std::time::Duration> {
        self.account_states.get(account_id).cooldown_remaining()
    }

    /// 获取配置的所有 Provider 列表
    pub fn configured_providers(&self) -> &[String] {
        &self.providers
    }

    /// 获取当前健康的 Provider 列表
    pub fn healthy_providers(&self) -> Vec<String> {
        self.provider_health.healthy_providers(&self.providers)
    }

    /// 添加 Provider
    pub fn add_provider(&mut self, provider: impl Into<String>) {
        let provider = provider.into();
        if !self.providers.contains(&provider) {
            self.providers.push(provider);
        }
    }

    /// 移除 Provider
    pub fn remove_provider(&mut self, provider: &str) {
        self.providers.retain(|p| p != provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keycompute_types::PricingSnapshot;
    use rust_decimal::Decimal;
    use sea_orm::DatabaseConnection;

    /// Mock Node 能力索引,用于测试
    struct MockNodeIndex {
        ready_models: Vec<String>,
    }

    #[async_trait::async_trait]
    impl NodeCapabilityIndex for MockNodeIndex {
        async fn has_ready_node(&self, model: &str) -> bool {
            self.ready_models.contains(&model.to_string())
        }
    }

    fn create_test_context() -> RequestContext {
        RequestContext::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "gpt-4o",
            vec![],
            true,
            PricingSnapshot {
                model_name: "gpt-4o".to_string(),
                currency: "CNY".to_string(),
                input_price_per_1k: Decimal::from(1),
                output_price_per_1k: Decimal::from(2),
            },
        )
    }

    fn create_test_engine() -> RoutingEngine {
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let providers = vec![
            "openai".to_string(),
            "deepseek".to_string(),
            "claude".to_string(),
            "gemini".to_string(),
        ];
        RoutingEngine::new(account_states, provider_health, providers)
    }

    #[tokio::test]
    async fn test_routing_engine_new() {
        let engine = create_test_engine();

        // 验证 Provider 数量与传入的测试列表一致（4个）
        assert_eq!(engine.configured_providers().len(), 4);
    }

    #[tokio::test]
    async fn test_route() {
        let engine = create_test_engine();
        let ctx = create_test_context();

        let plan = engine.route(&ctx).await;
        assert!(plan.is_ok());

        let plan = plan.unwrap();
        // 验证 primary target 是 ProviderAccount 变体
        match &plan.primary {
            ExecutionTarget::ProviderAccount { provider, .. } => {
                assert!(!provider.is_empty());
            }
            ExecutionTarget::Node { .. } => {
                panic!("Expected ProviderAccount variant in test");
            }
        }
    }

    #[tokio::test]
    async fn test_route_openai_entry_selects_openai_only() {
        // openai 入口（无原生 Anthropic 请求体）只从 openai 协议选账号，
        // 即使引擎同时注册了其他协议也不跨协议路由。
        // openai 不排首位：若入口隔离被移除，primary 会落到排首的 deepseek，测试即失败。
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let providers = vec![
            "deepseek".to_string(),
            "openai".to_string(),
            "claude".to_string(),
            "gemini".to_string(),
        ];
        let engine = RoutingEngine::new(account_states, provider_health, providers);
        let ctx = create_test_context();

        let plan = engine.route(&ctx).await.unwrap();
        match &plan.primary {
            ExecutionTarget::ProviderAccount { provider, .. } => {
                assert_eq!(provider, "openai");
            }
            ExecutionTarget::Node { .. } => panic!("Expected ProviderAccount variant in test"),
        }
        // 无数据库时 select_fallback_account 只生成 1 个 target，fallback 链
        // 必然为空。fallback 链的协议隔离由 rank_providers 的过滤保证（见
        // test_rank_providers_filters_by_entry_protocol），此处显式断言空链，
        // 避免先前空循环断言形同虚设。
        assert!(plan.fallback_chain.is_empty());
    }

    #[tokio::test]
    async fn test_route_anthropic_entry_selects_anthropic_only() {
        // anthropic 入口（带原生请求体）只从 anthropic 协议选账号，
        // 即使 openai 协议也在候选注册表中
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let providers = vec!["openai".to_string(), "anthropic".to_string()];
        let engine = RoutingEngine::new(account_states, provider_health, providers);

        let mut ctx = create_test_context();
        ctx.native_anthropic_request = Some(Arc::new(serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}]
        })));

        let plan = engine.route(&ctx).await.unwrap();
        match &plan.primary {
            ExecutionTarget::ProviderAccount { provider, .. } => {
                assert_eq!(provider, "anthropic");
            }
            ExecutionTarget::Node { .. } => panic!("Expected ProviderAccount variant in test"),
        }
    }

    #[tokio::test]
    async fn test_route_anthropic_entry_fails_without_anthropic_accounts() {
        // 本协议未配置账号时不跨协议兜底，直接路由失败
        let engine = create_test_engine(); // providers 不含 anthropic
        let mut ctx = create_test_context();
        ctx.native_anthropic_request = Some(Arc::new(serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}]
        })));

        let err = engine.route(&ctx).await.unwrap_err();
        assert!(matches!(err, KeyComputeError::RoutingFailed(_)));
    }

    #[tokio::test]
    async fn test_rank_providers_filters_by_entry_protocol() {
        // 候选 Provider 仅限入口协议本身：fallback 链隔离的根源是
        // rank_providers 的过滤，而非账号选择阶段
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let providers = vec![
            "deepseek".to_string(),
            "openai".to_string(),
            "anthropic".to_string(),
            "claude".to_string(),
        ];
        let engine = RoutingEngine::new(account_states, provider_health, providers);
        let pricing = PricingSnapshot {
            model_name: "gpt-4o".to_string(),
            currency: "CNY".to_string(),
            input_price_per_1k: Decimal::from(1),
            output_price_per_1k: Decimal::from(2),
        };

        let openai_ranked = engine
            .rank_providers("gpt-4o", &pricing, "openai")
            .await
            .unwrap();
        assert_eq!(openai_ranked, vec!["openai".to_string()]);

        let anthropic_ranked = engine
            .rank_providers("gpt-4o", &pricing, "anthropic")
            .await
            .unwrap();
        assert_eq!(anthropic_ranked, vec!["anthropic".to_string()]);
    }

    #[tokio::test]
    async fn test_rank_providers_anthropic_entry_selects_anthropic_only() {
        // anthropic 入口只选 anthropic 协议候选：即使 openai 等其他协议注册在前，
        // 候选列表仍只含 anthropic，且保持注册顺序（隔离发生在 rank 阶段，
        // 而非账号选择阶段）。
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let providers = vec![
            "openai".to_string(),
            "deepseek".to_string(),
            "anthropic".to_string(),
            "claude".to_string(),
        ];
        let engine = RoutingEngine::new(account_states, provider_health, providers);
        let pricing = PricingSnapshot {
            model_name: "gpt-4o".to_string(),
            currency: "CNY".to_string(),
            input_price_per_1k: Decimal::from(1),
            output_price_per_1k: Decimal::from(2),
        };

        let anthropic_ranked = engine
            .rank_providers("gpt-4o", &pricing, "anthropic")
            .await
            .unwrap();
        assert_eq!(anthropic_ranked, vec!["anthropic".to_string()]);

        // 大小写不敏感：route() 传入值固定为小写，此处为防御性验证
        // （eq_ignore_ascii_case 匹配），防止将来传入规范化前的用户输入
        let mixed_case_ranked = engine
            .rank_providers("gpt-4o", &pricing, "Anthropic")
            .await
            .unwrap();
        assert_eq!(mixed_case_ranked, vec!["anthropic".to_string()]);
    }

    #[tokio::test]
    async fn test_rank_providers_returns_empty_without_protocol_candidates() {
        // 无同协议候选时不跨协议兜底：anthropic 入口 + 引擎仅注册 openai 等
        // 其他协议 → 返回空列表（route 据此报 RoutingFailed），而不是回退到
        // openai 候选（避免原生 Anthropic 字段在协议转换中丢失）。
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let providers = vec![
            "openai".to_string(),
            "deepseek".to_string(),
            "claude".to_string(),
        ];
        let engine = RoutingEngine::new(account_states, provider_health, providers);
        let pricing = PricingSnapshot {
            model_name: "gpt-4o".to_string(),
            currency: "CNY".to_string(),
            input_price_per_1k: Decimal::from(1),
            output_price_per_1k: Decimal::from(2),
        };

        // 本协议未注册：空列表，不跨协议兜底
        let anthropic_ranked = engine
            .rank_providers("gpt-4o", &pricing, "anthropic")
            .await
            .unwrap();
        assert!(anthropic_ranked.is_empty());

        // 对照组：openai 入口正常返回 openai 候选（过滤未误伤其他协议）
        let openai_ranked = engine
            .rank_providers("gpt-4o", &pricing, "openai")
            .await
            .unwrap();
        assert_eq!(openai_ranked, vec!["openai".to_string()]);

        // 未知协议名同样返回空列表（防御性，避免静默路由到错误协议）
        let unknown_ranked = engine
            .rank_providers("gpt-4o", &pricing, "unknown")
            .await
            .unwrap();
        assert!(unknown_ranked.is_empty());
    }

    #[test]
    fn test_score_provider() {
        let engine = create_test_engine();
        let pricing = PricingSnapshot {
            model_name: "test".to_string(),
            currency: "CNY".to_string(),
            input_price_per_1k: Decimal::from(1),
            output_price_per_1k: Decimal::from(2),
        };

        let openai_score = engine.score_provider("openai", &pricing);
        let other_score = engine.score_provider("other", &pricing);

        // 两者应该都有合理的分数（0-200 范围）
        assert!((0.0..=200.0).contains(&openai_score));
        assert!((0.0..=200.0).contains(&other_score));
    }

    #[test]
    fn test_provider_health_integration() {
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        // 模拟一些请求数据
        provider_health.record_success("openai", 100);
        provider_health.record_success("openai", 150);
        provider_health.record_failure("claude");

        let providers = vec!["openai".to_string(), "claude".to_string()];
        let engine = RoutingEngine::new(account_states, provider_health, providers);

        // 检查健康状态
        assert!(engine.is_provider_healthy("openai"));
        // claude 只有一次失败，仍然健康（成功率 0%，但没有达到 10 次阈值）
        assert!(engine.is_provider_healthy("claude"));

        // 检查评分
        let openai_score = engine.get_provider_health_score("openai");
        assert!(openai_score > 50, "OpenAI should have good health score");
    }

    #[test]
    fn test_unhealthy_provider_marking() {
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        // 让 claude 多次失败变得不健康
        for _ in 0..10 {
            provider_health.record_failure("claude");
        }

        let providers = vec!["openai".to_string(), "claude".to_string()];
        let engine = RoutingEngine::new(account_states, provider_health, providers);

        // claude 应该被标记为不健康
        assert!(!engine.is_provider_healthy("claude"));

        // 健康列表应该不包含 claude（但路由时不会过滤，只是评分靠后）
        let healthy = engine.healthy_providers();
        assert!(!healthy.contains(&"claude".to_string()));
    }

    #[test]
    fn test_routing_constants() {
        // 验证路由权重常量总和为 1.0
        let total = COST_WEIGHT + LATENCY_WEIGHT + SUCCESS_WEIGHT + HEALTH_WEIGHT;
        assert!(
            (total - 1.0).abs() < 0.001,
            "Routing weights should sum to 1.0"
        );
        assert_eq!(COST_WEIGHT, 0.3);
        assert_eq!(LATENCY_WEIGHT, 0.25);
        assert_eq!(UNHEALTHY_PENALTY, 100.0);
    }

    #[test]
    fn test_calculate_cost_score() {
        let engine = create_test_engine();

        let cheap_pricing = PricingSnapshot {
            model_name: "test".to_string(),
            currency: "CNY".to_string(),
            input_price_per_1k: Decimal::from(1),
            output_price_per_1k: Decimal::from(2),
        };

        let expensive_pricing = PricingSnapshot {
            model_name: "test".to_string(),
            currency: "CNY".to_string(),
            input_price_per_1k: Decimal::from(5),
            output_price_per_1k: Decimal::from(10),
        };

        let cheap_score = engine.calculate_cost_score(&cheap_pricing);
        let expensive_score = engine.calculate_cost_score(&expensive_pricing);

        // 贵的应该分数更高（越不优先）
        assert!(expensive_score > cheap_score);
    }

    #[test]
    fn test_calculate_latency_score() {
        let engine = create_test_engine();

        assert!(engine.calculate_latency_score(50) < engine.calculate_latency_score(200));
        assert!(engine.calculate_latency_score(200) < engine.calculate_latency_score(500));
        assert!(engine.calculate_latency_score(500) < engine.calculate_latency_score(1500));
    }

    #[test]
    fn test_account_cooldown_check() {
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        let providers = vec!["test".to_string()];
        let engine = RoutingEngine::new(account_states.clone(), provider_health, providers);

        let account_id = Uuid::new_v4();

        // 初始状态
        assert!(!engine.is_account_cooling(&account_id));

        // 设置账号冷却
        account_states.set_cooldown(account_id, 30);

        // 现在应该在冷却中
        assert!(engine.is_account_cooling(&account_id));
        assert!(engine.account_cooldown_remaining(&account_id).is_some());
    }

    #[test]
    fn test_decrypt_upstream_api_key_without_global_key() {
        // 当全局密钥未设置时，应该回退使用原始值
        // 注意：如果其他测试先设置了全局密钥，此测试跳过
        if keycompute_runtime::global_crypto().is_some() {
            // 全局密钥已被其他测试设置，跳过此测试
            return;
        }
        let result = RoutingEngine::decrypt_upstream_api_key("test-api-key");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-api-key");
    }

    #[test]
    fn test_decrypt_upstream_api_key_with_global_key() {
        // 设置全局密钥
        let key = keycompute_runtime::ApiKeyCrypto::generate_key();
        keycompute_runtime::set_global_crypto(&key).expect("Failed to set global crypto");

        // 加密一个 API Key
        let plaintext = "sk-test-secret-key-123";
        let encrypted = keycompute_runtime::encrypt_api_key(plaintext).expect("Failed to encrypt");

        // 解密应该返回原始值
        let result = RoutingEngine::decrypt_upstream_api_key(encrypted.as_str());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), plaintext);
    }

    #[tokio::test]
    async fn test_node_routing_with_ready_node() {
        // 创建带有 ready node 的引擎
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let pool = DbRouter::single(DatabaseConnection::Disconnected);
        let providers = vec!["openai".to_string()];
        let node_index = Arc::new(MockNodeIndex {
            ready_models: vec!["deepseek-chat".to_string()],
        });

        let engine = RoutingEngine::with_node_index(
            account_states,
            provider_health,
            pool,
            providers,
            node_index,
        );

        // 创建请求上下文,使用 node: 前缀
        let mut ctx = create_test_context();
        ctx.model = "node:deepseek-chat".to_string();
        ctx.stream = false;

        // 应该路由到 Node
        let plan = engine.route(&ctx).await;
        assert!(plan.is_ok());

        let plan = plan.unwrap();
        match &plan.primary {
            ExecutionTarget::Node { model } => {
                assert_eq!(model, "deepseek-chat");
            }
            ExecutionTarget::ProviderAccount { .. } => {
                panic!("Expected Node variant");
            }
        }

        // 应该没有 fallback
        assert!(plan.fallback_chain.is_empty());
    }

    #[tokio::test]
    async fn test_route_node_model_ignores_entry_protocol_isolation() {
        // node 模型走 node 路由分支，不受入口协议隔离影响；对应 debug_routing
        // 的 entry=anthropic + node: 模型组合（哨兵 body 仅让 route() 判定入口协议，
        // node 分支在协议判定之前返回）。
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let pool = DbRouter::single(DatabaseConnection::Disconnected);
        let providers = vec!["openai".to_string()];
        let node_index = Arc::new(MockNodeIndex {
            ready_models: vec!["deepseek-chat".to_string()],
        });
        let engine = RoutingEngine::with_node_index(
            account_states,
            provider_health,
            pool,
            providers,
            node_index,
        );

        let mut ctx = create_test_context();
        ctx.model = "node:deepseek-chat".to_string();
        ctx.stream = false;
        // anthropic 入口（debug_routing entry=anthropic 注入的哨兵 body）
        ctx.native_anthropic_request = Some(Arc::new(serde_json::json!({ "messages": [] })));

        let plan = engine.route(&ctx).await.unwrap();
        match &plan.primary {
            ExecutionTarget::Node { model } => {
                assert_eq!(model, "deepseek-chat");
            }
            ExecutionTarget::ProviderAccount { .. } => {
                panic!("Expected Node variant");
            }
        }
        assert!(plan.fallback_chain.is_empty());
    }

    #[tokio::test]
    async fn test_node_routing_without_ready_node() {
        // 创建没有 ready node 的引擎
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let pool = DbRouter::single(DatabaseConnection::Disconnected);
        let providers = vec!["openai".to_string()];
        let node_index = Arc::new(MockNodeIndex {
            ready_models: vec![], // 没有 ready 模型
        });

        let engine = RoutingEngine::with_node_index(
            account_states,
            provider_health,
            pool,
            providers,
            node_index,
        );

        // 创建请求上下文,使用 node: 前缀
        let mut ctx = create_test_context();
        ctx.model = "node:deepseek-chat".to_string();
        ctx.stream = false;

        // 应该返回 NoReadyNode 错误
        let plan = engine.route(&ctx).await;
        assert!(plan.is_err());

        match plan.unwrap_err() {
            KeyComputeError::NoReadyNode(model) => {
                assert_eq!(model, "deepseek-chat");
            }
            _ => panic!("Expected NoReadyNode error"),
        }
    }

    #[tokio::test]
    async fn test_node_routing_streaming_not_supported() {
        // 创建带有 ready node 的引擎
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let pool = DbRouter::single(DatabaseConnection::Disconnected);
        let providers = vec!["openai".to_string()];
        let node_index = Arc::new(MockNodeIndex {
            ready_models: vec!["deepseek-chat".to_string()],
        });

        let engine = RoutingEngine::with_node_index(
            account_states,
            provider_health,
            pool,
            providers,
            node_index,
        );

        // 创建请求上下文,使用 node: 前缀但 stream=true
        let mut ctx = create_test_context();
        ctx.model = "node:deepseek-chat".to_string();
        ctx.stream = true; // 流式请求现在应该被允许

        // 应该成功路由到 Node,服务端会模拟流式输出
        let plan = engine.route(&ctx).await;
        assert!(plan.is_ok());

        match plan.unwrap().primary {
            ExecutionTarget::Node { model } => {
                assert_eq!(model, "deepseek-chat");
            }
            _ => panic!("Expected Node target"),
        }
    }

    #[tokio::test]
    async fn test_node_routing_without_node_index() {
        // 创建没有 node_index 的引擎
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let providers = vec!["openai".to_string()];

        let engine = RoutingEngine::new(account_states, provider_health, providers);

        // 创建请求上下文,使用 node: 前缀
        let mut ctx = create_test_context();
        ctx.model = "node:deepseek-chat".to_string();
        ctx.stream = false;

        // 应该返回 Internal 错误(node_index not configured)
        let plan = engine.route(&ctx).await;
        assert!(plan.is_err());

        match plan.unwrap_err() {
            KeyComputeError::Internal(msg) => {
                assert!(msg.contains("Node routing not configured"));
            }
            _ => panic!("Expected Internal error"),
        }
    }

    fn create_test_account(provider: &str, endpoint: &str, priority: i32) -> Account {
        // 先确保全局加密密钥已设置（OnceLock 首次设置生效、重复调用静默忽略），
        // 再存加密值，避免与并行测试中设置密钥的时序竞态
        keycompute_runtime::set_global_crypto(&keycompute_runtime::ApiKeyCrypto::generate_key())
            .expect("Failed to set global crypto");
        let upstream_key = keycompute_runtime::encrypt_api_key("sk-test-plain")
            .expect("Failed to encrypt test key")
            .into_inner();
        let now = chrono::Utc::now();
        Account {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            provider: provider.to_string(),
            name: format!("{}-account", provider),
            endpoint: endpoint.to_string(),
            upstream_api_key_encrypted: upstream_key,
            upstream_api_key_preview: "sk-t****".to_string(),
            rpm_limit: 60,
            tpm_limit: 100_000,
            priority,
            enabled: true,
            models_supported: vec!["gpt-4o".to_string()],
            visibility: "tenant".to_string(),
            last_probe_at: None,
            last_probe_latency_ms: None,
            last_probe_status: None,
            last_probe_error_code: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_select_best_accounts_empty_endpoint_uses_protocol_default() {
        // 空 endpoint 的账号应回落到协议默认 Base URL，
        // 而非带着空 endpoint 进入执行层导致相对 URL 失败
        let engine = create_test_engine();
        let accounts = vec![create_test_account("openai", "", 10)];

        let targets = engine
            .select_best_accounts("openai", accounts)
            .await
            .unwrap();

        assert_eq!(targets.len(), 1);
        match &targets[0] {
            ExecutionTarget::ProviderAccount { endpoint, .. } => {
                assert_eq!(
                    endpoint,
                    llm_protocol_provider::ProtocolType::Openai.default_endpoint()
                );
            }
            _ => panic!("Expected ProviderAccount target"),
        }
    }

    #[tokio::test]
    async fn test_select_best_accounts_skips_undecryptable_account() {
        // 单个账号密钥损坏不应中止整条路由，其余健康账号仍应入选
        let engine = create_test_engine();
        let healthy_high = create_test_account("openai", "https://high.example.com/v1", 100);
        let mut broken = create_test_account("openai", "https://broken.example.com/v1", 50);
        // 非 Base64/非合法密文，解密必失败（全局密钥已在 create_test_account 中设置）
        broken.upstream_api_key_encrypted = "!!not-valid-ciphertext!!".to_string();
        let healthy_low = create_test_account("openai", "https://low.example.com/v1", 1);

        let targets = engine
            .select_best_accounts("openai", vec![healthy_high, broken, healthy_low])
            .await
            .unwrap();

        let endpoints: Vec<&str> = targets
            .iter()
            .map(|t| match t {
                ExecutionTarget::ProviderAccount { endpoint, .. } => endpoint.as_str(),
                _ => panic!("Expected ProviderAccount target"),
            })
            .collect();
        assert_eq!(
            endpoints,
            vec!["https://high.example.com/v1", "https://low.example.com/v1"],
            "broken account should be skipped, healthy ones kept in priority order"
        );
    }

    #[tokio::test]
    async fn test_select_best_accounts_priority_order_and_top_n() {
        // 同协议多账号按优先级降序选取，最多 MAX_ACCOUNTS_PER_PROVIDER 个
        let engine = create_test_engine();
        let accounts = vec![
            create_test_account("openai", "https://low.example.com/v1", 1),
            create_test_account("openai", "https://high.example.com/v1", 100),
            create_test_account("openai", "https://mid.example.com/v1", 50),
            create_test_account("openai", "https://lowest.example.com/v1", 0),
        ];

        let targets = engine
            .select_best_accounts("openai", accounts)
            .await
            .unwrap();

        // 截断到 top-N，且首选为最高优先级账号
        assert_eq!(targets.len(), MAX_ACCOUNTS_PER_PROVIDER);
        let endpoints: Vec<&str> = targets
            .iter()
            .map(|t| match t {
                ExecutionTarget::ProviderAccount { endpoint, .. } => endpoint.as_str(),
                _ => panic!("Expected ProviderAccount target"),
            })
            .collect();
        assert_eq!(
            endpoints,
            vec![
                "https://high.example.com/v1",
                "https://mid.example.com/v1",
                "https://low.example.com/v1",
            ]
        );
    }

    #[tokio::test]
    async fn test_select_best_accounts_empty_when_all_cooling() {
        // 全部账号冷却时返回空 targets，route() 据此报 RoutingFailed（不跨协议兜底）；
        // 冷却耗尽是"账号池临时不可用"的一种来源，验证空结果语义而非静默选错账号
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let engine = RoutingEngine::new(
            account_states.clone(),
            provider_health,
            vec!["openai".to_string()],
        );
        let accounts = vec![
            create_test_account("openai", "https://high.example.com/v1", 100),
            create_test_account("openai", "https://low.example.com/v1", 1),
        ];
        for account in &accounts {
            account_states.set_cooldown(account.id, 60);
        }

        let targets = engine
            .select_best_accounts("openai", accounts)
            .await
            .unwrap();
        assert!(targets.is_empty());
    }

    #[tokio::test]
    async fn test_provider_routing_without_node_prefix() {
        // 创建带有 node_index 的引擎
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());
        let pool = DbRouter::single(DatabaseConnection::Disconnected);
        let providers = vec!["openai".to_string()];
        let node_index = Arc::new(MockNodeIndex {
            ready_models: vec!["deepseek-chat".to_string()],
        });

        let engine = RoutingEngine::with_node_index(
            account_states,
            provider_health,
            pool,
            providers,
            node_index,
        );

        // 创建请求上下文,不使用 node: 前缀
        let ctx = create_test_context(); // 默认 model = "gpt-4o"

        // 应该走 Provider 路由
        let plan = engine.route(&ctx).await;
        assert!(plan.is_ok());

        let plan = plan.unwrap();
        match &plan.primary {
            ExecutionTarget::ProviderAccount { provider, .. } => {
                assert!(!provider.is_empty());
            }
            ExecutionTarget::Node { .. } => {
                panic!("Expected ProviderAccount variant for non-node prefix");
            }
        }
    }
}
