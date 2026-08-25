//! Usage Log 构建与写入
//!
//! 构建并写入 usage_logs 主账本

use crate::balance::BalanceService;
use crate::calculator::calculate_amount;
use crate::usage_source::UsageSource;
use chrono::{DateTime, Utc};
use keycompute_db::{CreateUsageLogRequest, DbRouter, UsageLog, models::node_tip::NodeTip};
use keycompute_distribution::{
    DistributionContext, DistributionService, calculator::calculate_shares,
};
use keycompute_types::{KeyComputeError, RequestContext, Result};
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

const MARK_BILLING_FAILED_SQL: &str = "UPDATE gateway_requests SET billing_status='failed',updated_at=NOW() WHERE request_id=$1 AND billing_status='pending'";

async fn mark_billing_failed_best_effort(pool: &DbRouter, request_id: Uuid) {
    let update = pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        MARK_BILLING_FAILED_SQL,
        [request_id.into()],
    ));
    match tokio::time::timeout(Duration::from_millis(250), update).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            // Preserve the original billing error. The stale-trace reconciler
            // retries this transition after the database becomes available.
            tracing::warn!(%request_id, %error, "failed to mark billing trace failed; reconciliation will retry");
        }
        Err(_) => {
            tracing::warn!(%request_id, "timed out marking billing trace failed; reconciliation will retry");
        }
    }
}

/// 计费服务
#[derive(Clone)]
pub struct BillingService {
    /// 数据库连接（可选）
    pool: Option<Arc<DbRouter>>,
    /// 分销服务（可选）
    distribution: Option<DistributionService>,
    /// 余额服务（可选）
    balance: Option<BalanceService>,
}

impl std::fmt::Debug for BillingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BillingService")
            .field("pool", &"DatabaseConnection")
            .field(
                "distribution",
                &self.distribution.as_ref().map(|_| "DistributionService"),
            )
            .field("balance", &self.balance.as_ref().map(|_| "BalanceService"))
            .finish()
    }
}

impl BillingService {
    /// 创建新的计费服务（无数据库连接）
    pub fn new() -> Self {
        Self {
            pool: None,
            distribution: None,
            balance: None,
        }
    }

    /// 创建带数据库连接的计费服务
    pub fn with_pool(pool: Arc<DbRouter>) -> Self {
        Self {
            pool: Some(Arc::clone(&pool)),
            distribution: Some(DistributionService::with_pool(Arc::clone(&pool))),
            balance: Some(BalanceService::new(pool)),
        }
    }

    /// 创建带数据库连接和自定义分销服务的计费服务
    pub fn with_pool_and_distribution(
        pool: Arc<DbRouter>,
        distribution: DistributionService,
    ) -> Self {
        Self {
            pool: Some(Arc::clone(&pool)),
            distribution: Some(distribution),
            balance: Some(BalanceService::new(pool)),
        }
    }

    /// 获取余额服务
    ///
    /// 用于需要直接操作余额的场景
    pub fn balance_service(&self) -> Option<&BalanceService> {
        self.balance.as_ref()
    }

    /// 流结束后执行结算
    ///
    /// 输入: usage + pricing_snapshot + request metadata
    /// 输出: 返回构建的 NewUsageLog（实际写入由调用方执行）
    pub async fn finalize(
        &self,
        ctx: &RequestContext,
        provider_name: &str,
        account_id: Uuid,
        status: &str,
    ) -> Result<NewUsageLog> {
        // 获取用量快照
        let (input_tokens, output_tokens) = ctx.usage_snapshot();
        let total_tokens = input_tokens + output_tokens;

        // 计算用户应付金额。应付金额始终基于请求开始时冻结的定价快照：定价按
        // model + 计费维度（"node"/"provideraccount"）查找，与真实 provider 无关，
        // 因此 provider_name/account_id 归属到 fallback 账号后金额依然一致。若未来
        // 引入按真实 provider 定价，必须在归属（可能为 fallback 账号）时刷新快照。
        let user_amount = calculate_amount(input_tokens, output_tokens, &ctx.pricing_snapshot);

        // 确定用量来源
        // 只要任一计量侧被 Provider 精确值锁定即标记 ProviderReported：
        // 例如 Anthropic 在 message_start 即上报精确输入（InputUsage），
        // 或兼容网关上报 Usage{input:0, output:N}（输入被跳过保留估算）；
        // 这两种半精确状态都不应被标注为纯网关估算。
        let usage_source = if ctx.is_input_finalized() || ctx.is_output_finalized() {
            UsageSource::ProviderReported
        } else {
            UsageSource::GatewayAccumulated
        };

        let log = NewUsageLog {
            request_id: ctx.request_id,
            tenant_id: ctx.tenant_id,
            user_id: ctx.user_id,
            produce_ai_key_id: ctx.produce_ai_key_id,
            model_name: ctx.model.clone(),
            provider_name: provider_name.to_string(),
            account_id,
            input_tokens: input_tokens as i32,
            output_tokens: output_tokens as i32,
            total_tokens: total_tokens as i32,
            input_unit_price_snapshot: ctx.pricing_snapshot.input_price_per_1k,
            output_unit_price_snapshot: ctx.pricing_snapshot.output_price_per_1k,
            user_amount,
            currency: ctx.pricing_snapshot.currency.clone(),
            usage_source: usage_source.as_str().to_string(),
            status: status.to_string(),
            started_at: ctx.started_at,
            finished_at: Utc::now(),
        };

        tracing::info!(
            request_id = %ctx.request_id,
            user_amount = %user_amount,
            "Billing finalized"
        );

        Ok(log)
    }

    /// 流结束后执行结算并写入数据库
    ///
    /// 输入: usage + pricing_snapshot + request metadata
    /// 输出: 写入数据库后的 UsageLog
    pub async fn finalize_and_save(
        &self,
        ctx: &RequestContext,
        provider_name: &str,
        account_id: Uuid,
        status: &str,
    ) -> Result<UsageLog> {
        // 先执行结算
        let new_log = self
            .finalize(ctx, provider_name, account_id, status)
            .await?;

        // 写入数据库
        let Some(pool) = &self.pool else {
            // 无数据库连接，返回模拟的 UsageLog
            return Ok(UsageLog {
                id: Uuid::new_v4(),
                request_id: new_log.request_id,
                tenant_id: new_log.tenant_id,
                user_id: new_log.user_id,
                produce_ai_key_id: new_log.produce_ai_key_id,
                model_name: new_log.model_name,
                provider_name: new_log.provider_name,
                account_id: new_log.account_id,
                input_tokens: new_log.input_tokens,
                output_tokens: new_log.output_tokens,
                total_tokens: new_log.total_tokens,
                input_unit_price_snapshot: decimal_to_bigdecimal(
                    &new_log.input_unit_price_snapshot,
                )?,
                output_unit_price_snapshot: decimal_to_bigdecimal(
                    &new_log.output_unit_price_snapshot,
                )?,
                user_amount: decimal_to_bigdecimal(&new_log.user_amount)?,
                currency: new_log.currency,
                usage_source: new_log.usage_source,
                status: new_log.status,
                started_at: new_log.started_at,
                finished_at: new_log.finished_at,
                created_at: Utc::now(),
            });
        };

        let create_req = CreateUsageLogRequest {
            request_id: new_log.request_id,
            tenant_id: new_log.tenant_id,
            user_id: new_log.user_id,
            produce_ai_key_id: new_log.produce_ai_key_id,
            model_name: new_log.model_name,
            provider_name: new_log.provider_name,
            account_id: new_log.account_id,
            input_tokens: new_log.input_tokens,
            output_tokens: new_log.output_tokens,
            input_unit_price_snapshot: decimal_to_bigdecimal(&new_log.input_unit_price_snapshot)?,
            output_unit_price_snapshot: decimal_to_bigdecimal(&new_log.output_unit_price_snapshot)?,
            user_amount: decimal_to_bigdecimal(&new_log.user_amount)?,
            currency: new_log.currency,
            usage_source: new_log.usage_source,
            status: new_log.status,
            started_at: new_log.started_at,
            finished_at: new_log.finished_at,
        };

        let tx = match pool.begin().await {
            Ok(tx) => tx,
            Err(error) => {
                keycompute_observability::metrics::BILLING_WRITE_FAILURE_TOTAL.inc();
                mark_billing_failed_best_effort(pool.as_ref(), ctx.request_id).await;
                return Err(KeyComputeError::DatabaseError(format!(
                    "Failed to begin billing transaction: {error}"
                )));
            }
        };
        let saved_log = match UsageLog::create(&tx, &create_req).await {
            Ok(log) => log,
            Err(error) => {
                keycompute_observability::metrics::BILLING_WRITE_FAILURE_TOTAL.inc();
                let _ = tx.rollback().await;
                mark_billing_failed_best_effort(pool.as_ref(), ctx.request_id).await;
                return Err(KeyComputeError::DatabaseError(format!(
                    "Failed to save usage log: {error}"
                )));
            }
        };
        if let Err(error) = tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE gateway_requests SET billing_status='succeeded',updated_at=NOW() WHERE request_id=$1",
            [ctx.request_id.into()],
        )).await {
            keycompute_observability::metrics::BILLING_WRITE_FAILURE_TOTAL.inc();
            let _ = tx.rollback().await;
            mark_billing_failed_best_effort(pool.as_ref(), ctx.request_id).await;
            return Err(KeyComputeError::DatabaseError(format!("Failed to update billing trace: {error}")));
        }
        if let Err(error) = tx.commit().await {
            keycompute_observability::metrics::BILLING_WRITE_FAILURE_TOTAL.inc();
            mark_billing_failed_best_effort(pool.as_ref(), ctx.request_id).await;
            return Err(KeyComputeError::DatabaseError(format!(
                "Failed to commit billing transaction: {error}"
            )));
        }

        tracing::info!(
            request_id = %ctx.request_id,
            usage_log_id = %saved_log.id,
            user_amount = %saved_log.user_amount,
            "Usage log saved to database"
        );

        Ok(saved_log)
    }

    /// 流结束后执行结算并触发分销
    ///
    /// 输入: usage + pricing_snapshot + request metadata
    /// 输出: 写入数据库后的 UsageLog，并触发 Distribution 处理
    ///
    /// 计费流程：
    /// 1. 计算费用并写入 usage_logs 表
    /// 2. 扣除用户余额（记录欠费但不影响执行结果）
    /// 3. 查询用户的推荐关系（user_referrals 表）
    /// 4. 查询租户的分销规则（tenant_distribution_rules 表）
    /// 5. 计算分成并保存
    ///
    /// 架构约束：Billing 不反向影响执行结果，余额扣除失败仅记录错误
    pub async fn finalize_and_trigger_distribution(
        &self,
        ctx: &RequestContext,
        provider_name: &str,
        account_id: Uuid,
        status: &str,
        user_id: Uuid,
    ) -> Result<UsageLog> {
        // 先执行结算并保存 usage_log
        let usage_log = self
            .finalize_and_save(ctx, provider_name, account_id, status)
            .await?;

        let user_amount = bigdecimal_to_decimal(&usage_log.user_amount)?;

        // 扣除用户余额（失败不影响主流程）
        self.deduct_balance_if_configured(
            ctx.request_id,
            user_id,
            user_amount,
            usage_log.id,
            &ctx.model,
        )
        .await;

        // 触发分销处理（失败不影响主流程）
        self.process_distribution_if_configured(ctx, &usage_log, user_id, user_amount)
            .await;

        // 触发节点租赁小费（失败不影响主流程）
        self.process_tips_if_configured(usage_log.id).await;

        Ok(usage_log)
    }

    /// 扣除用户余额（如果已配置余额服务）
    ///
    /// 架构约束：失败不影响主流程，仅记录错误
    async fn deduct_balance_if_configured(
        &self,
        request_id: Uuid,
        user_id: Uuid,
        user_amount: Decimal,
        usage_log_id: Uuid,
        model_name: &str,
    ) {
        let Some(balance) = &self.balance else {
            tracing::debug!(
                request_id = %request_id,
                "No balance service configured, skipping balance deduction"
            );
            return;
        };

        match balance
            .consume(
                user_id,
                user_amount,
                Some(usage_log_id),
                Some(&format!("API调用: {}", model_name)),
            )
            .await
        {
            Ok((updated_balance, _transaction)) => {
                tracing::info!(
                    request_id = %request_id,
                    user_id = %user_id,
                    amount = %user_amount,
                    new_balance = %updated_balance.available_balance,
                    "User balance deducted successfully"
                );
            }
            Err(e) => {
                // 根据架构约束，Billing 不反向影响执行结果
                // 扣除失败时仅记录错误，不抛出异常
                tracing::error!(
                    request_id = %request_id,
                    user_id = %user_id,
                    amount = %user_amount,
                    error = %e,
                    "Failed to deduct user balance (recorded as debt)"
                );
            }
        }
    }

    /// 处理分销（如果已配置分销服务）
    ///
    /// 架构约束：分销失败不影响主流程，仅记录错误
    async fn process_distribution_if_configured(
        &self,
        ctx: &RequestContext,
        usage_log: &UsageLog,
        user_id: Uuid,
        user_amount: Decimal,
    ) {
        let (Some(distribution), Some(pool)) = (&self.distribution, &self.pool) else {
            tracing::debug!(
                request_id = %ctx.request_id,
                "No distribution service configured, skipping distribution"
            );
            return;
        };

        // 检查分销系统是否启用（开关关闭时彻底停止分销计算）
        let distribution_enabled = keycompute_db::SystemSetting::get_bool(
            pool.as_ref(),
            keycompute_db::models::system_setting::setting_keys::DISTRIBUTION_ENABLED,
            false,
        )
        .await;
        if !distribution_enabled {
            tracing::debug!(
                request_id = %ctx.request_id,
                "Distribution is disabled via system settings, skipping"
            );
            return;
        }

        // 创建分销上下文
        let dist_ctx = DistributionContext::new(
            usage_log.id,
            ctx.tenant_id,
            user_amount,
            &usage_log.currency,
        );

        // 查询用户的推荐关系
        let (level1_beneficiary, level2_beneficiary) =
            match keycompute_db::UserReferral::find_by_user(pool.as_ref(), user_id).await {
                Ok(Some(referral)) => (referral.level1_referrer_id, referral.level2_referrer_id),
                Ok(None) => (None, None),
                Err(e) => {
                    tracing::warn!(
                        user_id = %user_id,
                        error = %e,
                        "Failed to find user referral, proceeding without distribution"
                    );
                    (None, None)
                }
            };

        // 如果没有推荐关系，跳过分销
        let Some(l1_id) = level1_beneficiary else {
            tracing::debug!(
                user_id = %user_id,
                "No referral relationship found, skipping distribution"
            );
            return;
        };

        // 查询租户的分销规则
        let rules = match keycompute_db::TenantDistributionRule::find_by_tenant(
            pool.as_ref(),
            ctx.tenant_id,
        )
        .await
        {
            Ok(rules) => rules,
            Err(e) => {
                tracing::warn!(
                    tenant_id = %ctx.tenant_id,
                    error = %e,
                    "Failed to find distribution rules, using default ratios"
                );
                vec![]
            }
        };

        // 确定分成比例（优先使用规则表，否则使用配置默认值）
        // 使用字符串桥接（string-bridge）转换 f64 -> Decimal，避免 from_f64_retain 引入
        // 浮点噪声（如 0.03 -> 0.0299999…）以及潜在的 unwrap panic：f64::to_string() 输出
        // 最短可往返表示（0.03 -> "0.03"），再解析为精确 Decimal，解析失败时回退到硬编码默认。
        let dist_config = keycompute_config::DistributionConfig::default();
        let default_level1_ratio = dist_config
            .level1_ratio()
            .to_string()
            .parse::<Decimal>()
            .unwrap_or_else(|_| Decimal::new(3, 2)); // 0.03
        let default_level2_ratio = dist_config
            .level2_ratio()
            .to_string()
            .parse::<Decimal>()
            .unwrap_or_else(|_| Decimal::new(2, 2)); // 0.02

        // 按优先级匹配规则（rules 已按 priority DESC, created_at ASC 排序）：
        // 1. 优先匹配特定受益人的规则（per-user override）
        // 2. 否则使用最高优先级的全局规则（beneficiary_id == nil，对所有用户生效）
        // 3. 都无匹配时使用配置默认值
        //
        // 优先级约定（Priority Convention）：
        //   - 100: Admin 通过 API 配置的全局覆盖规则
        //   -  10: 系统初始化的一级分销默认规则
        //   -   5: 系统初始化的二级分销默认规则
        //   -   0: 默认优先级（per-user 规则）
        // Billing 匹配：L1 = 最高优先级全局规则（find），L2 = 最低优先级全局规则（rfind，排除 L1）
        let nil_id = Uuid::nil();
        let l1_global_rule = rules.iter().find(|r| r.beneficiary_id == nil_id);
        let level1_ratio = rules
            .iter()
            .find(|r| r.beneficiary_id == l1_id)
            .or(l1_global_rule)
            .and_then(|r| bigdecimal_to_decimal(&r.commission_rate).ok())
            .unwrap_or(default_level1_ratio);

        let level2_ratio = level2_beneficiary
            .and_then(|l2_id| {
                rules
                    .iter()
                    .find(|r| r.beneficiary_id == l2_id)
                    .and_then(|r| bigdecimal_to_decimal(&r.commission_rate).ok())
            })
            .or_else(|| {
                // L2 回退：使用优先级最低的全局规则，但排除 L1 已命中的规则（避免只有一条规则时两级佣金率相同）
                // NOTE: 当 level2_beneficiary 为 None 时此闭包也会执行，但 calculate_shares 内部
                // 以 level2_beneficiary.is_some() 为 guard，因此此处解析出的 ratio 会被安全忽略。
                let l1_rule_id = l1_global_rule.map(|r| r.id);
                rules
                    .iter()
                    .rfind(|r| r.beneficiary_id == nil_id && Some(r.id) != l1_rule_id)
                    .and_then(|r| bigdecimal_to_decimal(&r.commission_rate).ok())
            })
            .unwrap_or(default_level2_ratio);

        // 计算分成
        let shares = calculate_shares(
            user_amount,
            level1_ratio,
            level2_ratio,
            l1_id,
            level2_beneficiary,
        );

        // 处理并保存分销记录
        match distribution.process_and_save(&dist_ctx, &shares).await {
            Ok(records) => {
                tracing::info!(
                    request_id = %ctx.request_id,
                    usage_log_id = %usage_log.id,
                    distribution_records = records.len(),
                    level1_ratio = %level1_ratio,
                    level2_ratio = %level2_ratio,
                    "Distribution processed successfully"
                );
            }
            Err(e) => {
                // 分销失败不影响主计费流程，只记录错误
                tracing::error!(
                    request_id = %ctx.request_id,
                    usage_log_id = %usage_log.id,
                    error = %e,
                    "Distribution processing failed"
                );
            }
        }
    }

    /// 触发节点租赁小费（如果已配置数据库连接）
    ///
    /// 根据 usage_log 查询对应的 node_task，为节点所有者创建小费记录。
    /// 架构约束：小费创建失败不影响主计费流程，仅记录错误。
    async fn process_tips_if_configured(&self, usage_log_id: Uuid) {
        let Some(pool) = &self.pool else {
            tracing::debug!(
                %usage_log_id,
                "No database pool configured, skipping tips processing"
            );
            return;
        };

        match NodeTip::create_from_usage_log(pool.as_ref(), usage_log_id).await {
            Ok(Some(tip)) => {
                tracing::info!(
                    %usage_log_id,
                    tip_id = %tip.id,
                    owner_user_id = %tip.owner_user_id,
                    tip_amount = %tip.tip_amount,
                    "Node tip created successfully"
                );
            }
            Ok(None) => {
                tracing::debug!(
                    %usage_log_id,
                    "No tip created (not a node gateway request or ratio is zero)"
                );
            }
            Err(e) => {
                // 架构约束：小费创建失败不影响主计费流程
                tracing::error!(
                    %usage_log_id,
                    error = %e,
                    "Failed to create node tip"
                );
            }
        }
    }

    /// 检查是否已配置数据库连接
    ///
    /// 用于启动时验证配置
    pub fn has_pool(&self) -> bool {
        self.pool.is_some()
    }
}

impl Default for BillingService {
    fn default() -> Self {
        Self::new()
    }
}

/// 新的 Usage Log 记录
///
/// 对应 usage_logs 表的字段
#[derive(Debug, Clone)]
pub struct NewUsageLog {
    /// 请求 ID
    pub request_id: Uuid,
    /// 租户 ID
    pub tenant_id: Uuid,
    /// 用户 ID
    pub user_id: Uuid,
    /// Produce AI Key ID（用户访问系统的 API Key）
    pub produce_ai_key_id: Uuid,
    /// 模型名称
    pub model_name: String,
    /// Provider 名称
    pub provider_name: String,
    /// 账号 ID
    pub account_id: Uuid,
    /// 输入 token 数
    pub input_tokens: i32,
    /// 输出 token 数
    pub output_tokens: i32,
    /// 总 token 数
    pub total_tokens: i32,
    /// 输入单价快照（每 1k tokens）
    pub input_unit_price_snapshot: Decimal,
    /// 输出单价快照（每 1k tokens）
    pub output_unit_price_snapshot: Decimal,
    /// 用户应付金额
    pub user_amount: Decimal,
    /// 货币
    pub currency: String,
    /// 用量来源
    pub usage_source: String,
    /// 状态
    pub status: String,
    /// 开始时间
    pub started_at: DateTime<Utc>,
    /// 结束时间
    pub finished_at: DateTime<Utc>,
}

impl NewUsageLog {
    /// 创建 Builder 模式构建器
    pub fn builder(request_id: Uuid) -> NewUsageLogBuilder {
        NewUsageLogBuilder::new(request_id)
    }
}

/// Usage Log 构建器
#[derive(Debug)]
pub struct NewUsageLogBuilder {
    request_id: Uuid,
    tenant_id: Option<Uuid>,
    user_id: Option<Uuid>,
    produce_ai_key_id: Option<Uuid>,
    model_name: Option<String>,
    provider_name: Option<String>,
    account_id: Option<Uuid>,
    input_tokens: i32,
    output_tokens: i32,
    input_unit_price_snapshot: Option<Decimal>,
    output_unit_price_snapshot: Option<Decimal>,
    user_amount: Option<Decimal>,
    currency: String,
    usage_source: String,
    status: String,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
}

impl NewUsageLogBuilder {
    /// 创建新的构建器
    pub fn new(request_id: Uuid) -> Self {
        Self {
            request_id,
            tenant_id: None,
            user_id: None,
            produce_ai_key_id: None,
            model_name: None,
            provider_name: None,
            account_id: None,
            input_tokens: 0,
            output_tokens: 0,
            input_unit_price_snapshot: None,
            output_unit_price_snapshot: None,
            user_amount: None,
            currency: "CNY".to_string(),
            usage_source: "gateway_accumulated".to_string(),
            status: "success".to_string(),
            started_at: None,
            finished_at: None,
        }
    }

    /// 设置租户 ID
    pub fn tenant_id(mut self, id: Uuid) -> Self {
        self.tenant_id = Some(id);
        self
    }

    /// 设置用户 ID
    pub fn user_id(mut self, id: Uuid) -> Self {
        self.user_id = Some(id);
        self
    }

    /// 设置 Produce AI Key ID
    pub fn produce_ai_key_id(mut self, id: Uuid) -> Self {
        self.produce_ai_key_id = Some(id);
        self
    }

    /// 设置模型名称
    pub fn model_name(mut self, name: impl Into<String>) -> Self {
        self.model_name = Some(name.into());
        self
    }

    /// 设置 Provider 名称
    pub fn provider_name(mut self, name: impl Into<String>) -> Self {
        self.provider_name = Some(name.into());
        self
    }

    /// 设置账号 ID
    pub fn account_id(mut self, id: Uuid) -> Self {
        self.account_id = Some(id);
        self
    }

    /// 设置 token 数量
    pub fn tokens(mut self, input: u32, output: u32) -> Self {
        self.input_tokens = input as i32;
        self.output_tokens = output as i32;
        self
    }

    /// 设置价格快照
    pub fn pricing(
        mut self,
        input_price: Decimal,
        output_price: Decimal,
        currency: impl Into<String>,
    ) -> Self {
        self.input_unit_price_snapshot = Some(input_price);
        self.output_unit_price_snapshot = Some(output_price);
        self.currency = currency.into();
        self
    }

    /// 设置金额
    pub fn user_amount(mut self, amount: Decimal) -> Self {
        self.user_amount = Some(amount);
        self
    }

    /// 设置状态
    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    /// 设置时间
    pub fn timing(mut self, started_at: DateTime<Utc>, finished_at: DateTime<Utc>) -> Self {
        self.started_at = Some(started_at);
        self.finished_at = Some(finished_at);
        self
    }

    /// 构建 NewUsageLog
    pub fn build(self) -> Result<NewUsageLog> {
        let total_tokens = self.input_tokens + self.output_tokens;

        // 如果没有设置金额，自动计算
        let user_amount = self.user_amount.unwrap_or_else(|| {
            let input_price = self.input_unit_price_snapshot.unwrap_or_default();
            let output_price = self.output_unit_price_snapshot.unwrap_or_default();
            let input_cost = Decimal::from(self.input_tokens) / Decimal::from(1000) * input_price;
            let output_cost =
                Decimal::from(self.output_tokens) / Decimal::from(1000) * output_price;
            input_cost + output_cost
        });

        Ok(NewUsageLog {
            request_id: self.request_id,
            tenant_id: self
                .tenant_id
                .ok_or_else(|| KeyComputeError::Internal("tenant_id required".into()))?,
            user_id: self
                .user_id
                .ok_or_else(|| KeyComputeError::Internal("user_id required".into()))?,
            produce_ai_key_id: self
                .produce_ai_key_id
                .ok_or_else(|| KeyComputeError::Internal("produce_ai_key_id required".into()))?,
            model_name: self
                .model_name
                .ok_or_else(|| KeyComputeError::Internal("model_name required".into()))?,
            provider_name: self
                .provider_name
                .ok_or_else(|| KeyComputeError::Internal("provider_name required".into()))?,
            account_id: self
                .account_id
                .ok_or_else(|| KeyComputeError::Internal("account_id required".into()))?,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens,
            input_unit_price_snapshot: self.input_unit_price_snapshot.unwrap_or_default(),
            output_unit_price_snapshot: self.output_unit_price_snapshot.unwrap_or_default(),
            user_amount,
            currency: self.currency,
            usage_source: self.usage_source,
            status: self.status,
            started_at: self.started_at.unwrap_or_else(Utc::now),
            finished_at: self.finished_at.unwrap_or_else(Utc::now),
        })
    }
}

/// 将 Decimal 转换为 BigDecimal
///
/// 通过字符串桥接以避免 f64 精度损失。
///
/// # Errors
/// 理论上不会失败（两种类型都使用字符串表示），但若失败则返回错误而非静默归零。
fn decimal_to_bigdecimal(value: &Decimal) -> Result<bigdecimal::BigDecimal> {
    value
        .to_string()
        .parse()
        .map_err(|e: bigdecimal::ParseBigDecimalError| {
            tracing::error!(
                value = %value,
                error = %e,
                "Critical: Failed to convert Decimal to BigDecimal"
            );
            KeyComputeError::Internal(format!(
                "Decimal → BigDecimal conversion failed for value {}: {}",
                value, e
            ))
        })
}

/// 将 BigDecimal 转换为 Decimal
///
/// 通过字符串桥接以避免 f64 精度损失。
///
/// # Errors
/// 理论上不会失败，但若失败则返回错误而非静默归零（计费核心路径不允许静默数据损坏）。
fn bigdecimal_to_decimal(value: &bigdecimal::BigDecimal) -> Result<Decimal> {
    value.to_string().parse().map_err(|e: rust_decimal::Error| {
        tracing::error!(
            value = %value,
            error = %e,
            "Critical: Failed to convert BigDecimal to Decimal"
        );
        KeyComputeError::Internal(format!(
            "BigDecimal → Decimal conversion failed for value {}: {}",
            value, e
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use keycompute_types::PricingSnapshot;
    use rust_decimal::Decimal;

    #[test]
    fn billing_failure_transition_never_overwrites_a_committed_status() {
        assert!(MARK_BILLING_FAILED_SQL.contains("billing_status='failed'"));
        assert!(MARK_BILLING_FAILED_SQL.contains("billing_status='pending'"));
    }

    #[tokio::test]
    async fn finalize_records_attributed_provider_account() {
        // 调用方传入的 provider/account（fallback 场景由 ctx.billing_target
        // 提供实际完成账号）必须原样写入 usage log 的归属字段。
        let service = BillingService::new();
        let ctx = RequestContext::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "claude-test",
            Vec::new(),
            false,
            PricingSnapshot::default(),
        );
        // 模拟 executor 已收到 Provider 精确 Usage 后触发结算。
        ctx.set_input_tokens(10);
        ctx.set_output_tokens(20);

        let fallback_account_id = Uuid::new_v4();
        let log = service
            .finalize(&ctx, "anthropic", fallback_account_id, "success")
            .await
            .unwrap();

        assert_eq!(log.provider_name, "anthropic");
        assert_eq!(log.account_id, fallback_account_id);
        assert_eq!(log.status, "success");
        // 精确 usage 已覆盖，来源标记为 ProviderReported 而非估算。
        assert_eq!(log.usage_source, UsageSource::ProviderReported.as_str());
        assert_eq!(log.input_tokens, 10);
        assert_eq!(log.output_tokens, 20);
    }

    #[tokio::test]
    async fn finalize_usage_source_reflects_half_finalized_usage() {
        // 来源标签必须反映“是否含 Provider 精确值”，而不是“两侧都精确”：
        // - 纯估算（无任何 Provider usage 事件）→ GatewayAccumulated
        // - 仅输入精确（Anthropic message_start 后断流，输出仍为估算）→ ProviderReported
        // - 仅输出精确（Usage{input:0, output:N}，输入被跳过保留估算）→ ProviderReported
        let service = BillingService::new();
        let new_ctx = || {
            RequestContext::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                "claude-test",
                Vec::new(),
                false,
                PricingSnapshot::default(),
            )
        };

        let estimate_only = new_ctx();
        estimate_only.set_input_tokens_estimate(5);
        estimate_only.add_output_tokens(3);
        let log = service
            .finalize(&estimate_only, "anthropic", Uuid::new_v4(), "success")
            .await
            .unwrap();
        assert_eq!(log.usage_source, UsageSource::GatewayAccumulated.as_str());

        let input_only = new_ctx();
        input_only.set_input_tokens(10);
        input_only.add_output_tokens(3);
        let log = service
            .finalize(&input_only, "anthropic", Uuid::new_v4(), "success")
            .await
            .unwrap();
        assert_eq!(log.usage_source, UsageSource::ProviderReported.as_str());

        let output_only = new_ctx();
        output_only.add_output_tokens(7);
        output_only.set_output_tokens(20);
        let log = service
            .finalize(&output_only, "anthropic", Uuid::new_v4(), "success")
            .await
            .unwrap();
        assert_eq!(log.usage_source, UsageSource::ProviderReported.as_str());
    }

    #[test]
    fn test_new_usage_log_builder() {
        let request_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let api_key_id = Uuid::new_v4();
        let account_id = Uuid::new_v4();
        let started_at = Utc::now();
        let finished_at = Utc::now();

        let log = NewUsageLog::builder(request_id)
            .tenant_id(tenant_id)
            .user_id(user_id)
            .produce_ai_key_id(api_key_id)
            .model_name("gpt-4o")
            .provider_name("openai")
            .account_id(account_id)
            .tokens(1000, 500)
            .pricing(Decimal::from(1), Decimal::from(2), "CNY")
            .status("success")
            .timing(started_at, finished_at)
            .build()
            .unwrap();

        assert_eq!(log.request_id, request_id);
        assert_eq!(log.tenant_id, tenant_id);
        assert_eq!(log.input_tokens, 1000);
        assert_eq!(log.output_tokens, 500);
        assert_eq!(log.total_tokens, 1500);
        assert_eq!(log.currency, "CNY");
        assert_eq!(log.status, "success");
    }

    #[test]
    fn test_new_usage_log_builder_auto_calculate() {
        let request_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let api_key_id = Uuid::new_v4();
        let account_id = Uuid::new_v4();

        let log = NewUsageLog::builder(request_id)
            .tenant_id(tenant_id)
            .user_id(user_id)
            .produce_ai_key_id(api_key_id)
            .model_name("gpt-4o")
            .provider_name("openai")
            .account_id(account_id)
            .tokens(1000, 500)
            .pricing(Decimal::from(1), Decimal::from(2), "CNY")
            .build()
            .unwrap();

        // 1000/1000*1 + 500/1000*2 = 1 + 1 = 2
        assert_eq!(log.user_amount, Decimal::from(2));
    }

    /// 验证 trigger_distribution 中默认分成比例的 string-bridge 转换：
    /// f64 -> to_string() -> parse::<Decimal>() 必须得到精确的 0.03 / 0.02，
    /// 不能出现 from_f64_retain 那样的浮点噪声（如 0.0299999…）。
    #[test]
    fn test_default_ratio_string_bridge_precision() {
        let dist_config = keycompute_config::DistributionConfig::default();

        let l1 = dist_config
            .level1_ratio()
            .to_string()
            .parse::<Decimal>()
            .unwrap_or_else(|_| Decimal::new(3, 2));
        let l2 = dist_config
            .level2_ratio()
            .to_string()
            .parse::<Decimal>()
            .unwrap_or_else(|_| Decimal::new(2, 2));

        // 精确等于 0.03 / 0.02（string-bridge 无浮点噪声）
        assert_eq!(l1, Decimal::new(3, 2));
        assert_eq!(l2, Decimal::new(2, 2));
        // 序列化后不应出现拖尾噪声位
        assert_eq!(l1.to_string(), "0.03");
        assert_eq!(l2.to_string(), "0.02");
    }

    /// 覆盖自定义比例（含典型浮点截断场景 0.07）经 string-bridge 仍然精确。
    #[test]
    fn test_custom_ratio_string_bridge_precision() {
        let dist_config = keycompute_config::DistributionConfig::with_ratios(0.07, 0.15);
        let l1 = dist_config
            .level1_ratio()
            .to_string()
            .parse::<Decimal>()
            .unwrap_or_else(|_| Decimal::new(7, 2));
        let l2 = dist_config
            .level2_ratio()
            .to_string()
            .parse::<Decimal>()
            .unwrap_or_else(|_| Decimal::new(15, 2));
        assert_eq!(l1, Decimal::new(7, 2)); // 0.07
        assert_eq!(l2, Decimal::new(15, 2)); // 0.15
    }
}
