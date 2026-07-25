//! Distribution 分销管理处理器
//!
//! 完整的二级分销实现：
//! - 查看分销记录（从数据库）
//! - 分销统计（从数据库聚合）
//! - 分销规则管理 (Admin)
//! - 用户分销收益查询（从数据库）
//! - 推荐关系查询（从数据库）

use crate::{
    error::{ApiError, Result},
    extractors::AuthExtractor,
    handlers::configured_public_base_url,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use keycompute_db::models::system_setting::setting_keys;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use sea_orm::ConnectionTrait;

type BigDecimal = bigdecimal::BigDecimal;

// ==================== 数据结构 ====================

/// 分销记录查询参数
#[derive(Debug, Deserialize)]
pub struct DistributionQuery {
    /// 分页偏移
    #[serde(default)]
    pub offset: Option<i64>,
    /// 分页限制
    #[serde(default = "default_limit")]
    pub limit: Option<i64>,
    /// 按状态筛选
    pub status: Option<String>,
    /// 按层级筛选
    pub level: Option<String>,
    /// 按受益人筛选 (Admin 使用)
    pub beneficiary_id: Option<Uuid>,
}

fn default_limit() -> Option<i64> {
    Some(20)
}

/// 分销记录响应
#[derive(Debug, Serialize)]
pub struct DistributionRecordResponse {
    /// 记录 ID
    pub id: String,
    /// 推荐人（受益人）ID
    pub referrer_id: String,
    /// 被推荐用户 ID
    pub referred_id: String,
    /// 被推荐用户消费金额
    pub amount: String,
    /// 分销佣金
    pub commission: String,
    /// 状态: pending, settled, cancelled
    pub status: String,
    /// 创建时间
    pub created_at: String,
}

/// 分销统计响应
#[derive(Debug, Serialize)]
pub struct DistributionStatsResponse {
    /// 总收益
    pub total_earnings: String,
    /// 待结算金额
    pub pending_amount: String,
    /// 已结算金额
    pub settled_amount: String,
    /// 货币
    pub currency: String,
    /// 一级分销收益
    pub level1_earnings: String,
    /// 二级分销收益
    pub level2_earnings: String,
    /// 推荐人数
    pub referral_count: i64,
}

/// 分销规则响应
#[derive(Debug, Serialize)]
pub struct DistributionRuleResponse {
    /// 规则 ID
    pub id: String,
    /// 规则名称
    pub name: String,
    /// 佣金比例 (0.0 - 1.0)
    pub commission_rate: f64,
    /// 最小购买金额（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_purchase_amount: Option<f64>,
    /// 最大佣金金额（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_commission_amount: Option<f64>,
    /// 是否启用
    pub is_active: bool,
    /// 创建时间
    pub created_at: String,
}

/// 创建分销规则请求
#[derive(Debug, Deserialize)]
pub struct CreateDistributionRuleRequest {
    /// 规则名称
    pub name: String,
    /// 佣金比例 (0.0 - 1.0, 例如 0.03 表示 3%)
    pub commission_rate: f64,
    /// 最小购买金额（可选）
    pub min_purchase_amount: Option<f64>,
    /// 最大佣金金额（可选）
    pub max_commission_amount: Option<f64>,
}

/// 更新分销规则请求
#[derive(Debug, Deserialize)]
pub struct UpdateDistributionRuleRequest {
    /// 规则名称
    pub name: Option<String>,
    /// 佣金比例
    pub commission_rate: Option<f64>,
    /// 最小购买金额（可选）
    pub min_purchase_amount: Option<f64>,
    /// 最大佣金金额（可选）
    pub max_commission_amount: Option<f64>,
    /// 是否启用
    pub is_active: Option<bool>,
}

/// 用户分销收益查询响应
#[derive(Debug, Serialize)]
pub struct UserDistributionEarningsResponse {
    /// 用户 ID
    pub user_id: String,
    /// 总收益
    pub total_earnings: String,
    /// 待结算
    pub pending_amount: String,
    /// 已结算
    pub settled_amount: String,
    /// 货币
    pub currency: String,
    /// 一级推荐人数
    pub level1_referrals: i64,
    /// 二级推荐人数
    pub level2_referrals: i64,
}

/// 推荐码响应
#[derive(Debug, Serialize)]
pub struct ReferralCodeResponse {
    /// 用户 ID（作为推荐码）
    pub referral_code: String,
    /// 推荐链接
    pub invite_link: String,
    /// 一级推荐人数
    pub level1_count: i64,
    /// 二级推荐人数
    pub level2_count: i64,
}

/// 生成邀请链接请求
#[derive(Debug, Deserialize)]
pub struct GenerateInviteLinkRequest {
    /// 自定义来源标识（可选，用于追踪不同渠道）
    pub source: Option<String>,
}

/// 邀请链接响应
#[derive(Debug, Serialize)]
pub struct InviteLinkResponse {
    /// 完整邀请链接
    pub invite_link: String,
    /// 推荐码
    pub referral_code: String,
    /// 短链接（可选）
    pub short_link: Option<String>,
    /// 过期时间（可选）
    pub expires_at: Option<String>,
}

fn build_invite_link(base_url: &str, referral_code: &str, source: Option<&str>) -> Result<String> {
    let mut parsed = Url::parse(base_url)
        .map_err(|e| ApiError::Config(format!("Invalid APP_BASE_URL: {}", e)))?;
    let current_path = parsed.path().trim_end_matches('/');
    let next_path = if current_path.is_empty() || current_path == "/" {
        "/auth/register".to_string()
    } else {
        format!("{}/auth/register", current_path)
    };
    parsed.set_path(&next_path);

    {
        let mut query = parsed.query_pairs_mut();
        query.append_pair("ref", referral_code);
        if let Some(source) = source.map(str::trim).filter(|value| !value.is_empty()) {
            query.append_pair("source", source);
        }
    }

    Ok(parsed.into())
}

/// 获取我的推荐码和邀请链接
///
/// GET /api/v1/me/referral/code
pub async fn get_my_referral_code(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<ReferralCodeResponse>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 检查分销系统是否启用
    check_distribution_enabled(pool).await?;

    // 获取推荐统计
    let referral_stats = keycompute_db::UserReferral::get_stats_by_referrer(pool, auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    let base_url = configured_public_base_url(state.app_base_url.as_deref()).ok_or_else(|| {
        ApiError::Config("APP_BASE_URL is required to generate public invite links".to_string())
    })?;
    let referral_code = auth.user_id.to_string();
    let invite_link = build_invite_link(&base_url, &referral_code, None)?;

    Ok(Json(ReferralCodeResponse {
        referral_code,
        invite_link,
        level1_count: referral_stats.level1_count,
        level2_count: referral_stats.level2_count,
    }))
}

/// 生成邀请链接（支持自定义来源）
///
/// POST /api/v1/me/referral/invite-link
pub async fn generate_invite_link(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Json(req): Json<GenerateInviteLinkRequest>,
) -> Result<Json<InviteLinkResponse>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 检查分销系统是否启用
    check_distribution_enabled(pool).await?;

    let base_url = configured_public_base_url(state.app_base_url.as_deref()).ok_or_else(|| {
        ApiError::Config("APP_BASE_URL is required to generate public invite links".to_string())
    })?;
    let referral_code = auth.user_id.to_string();
    let invite_link = build_invite_link(&base_url, &referral_code, req.source.as_deref())?;

    Ok(Json(InviteLinkResponse {
        invite_link,
        referral_code,
        short_link: None, // 可以集成短链接服务
        expires_at: None, // 可以添加过期时间
    }))
}

/// 推荐人信息
#[derive(Debug, Serialize)]
pub struct ReferralInfo {
    /// 被推荐用户 ID
    pub id: String,
    /// 被推荐用户邮箱
    pub email: String,
    /// 被推荐用户昵称
    pub name: Option<String>,
    /// 注册时间
    pub created_at: String,
    /// 被推荐用户累计消费
    pub total_consumption: String,
    /// 当前用户从该推荐用户获得的收益
    pub earnings: String,
}

// ==================== 辅助函数 ====================

/// 将 BigDecimal 转换为字符串
fn bigdecimal_to_string(value: &BigDecimal) -> String {
    value.to_string()
}

/// 将字符串解析为 BigDecimal
fn string_to_bigdecimal(value: &str) -> Result<BigDecimal> {
    value
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("Invalid decimal: {}", e)))
}

/// 检查分销系统是否启用
async fn check_distribution_enabled(pool: &impl ConnectionTrait) -> Result<()> {
    let enabled =
        keycompute_db::SystemSetting::find_by_key(pool, setting_keys::DISTRIBUTION_ENABLED)
            .await
            .map_err(|e| {
                ApiError::Internal(format!("Failed to query distribution setting: {}", e))
            })?
            .map(|setting| setting.parse_bool())
            // 与默认初始化保持一致：缺失设置时按启用处理，避免升级环境漏种默认值时误判为禁用。
            .unwrap_or(true);

    if enabled {
        Ok(())
    } else {
        Err(ApiError::Forbidden("Distribution is disabled".to_string()))
    }
}

async fn build_distribution_record_response(
    pool: &impl ConnectionTrait,
    record: keycompute_db::DistributionRecord,
) -> Result<DistributionRecordResponse> {
    let usage_log = keycompute_db::UsageLog::find_by_id(pool, record.usage_log_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    let (referred_id, amount) = usage_log
        .map(|usage_log| {
            (
                usage_log.user_id.to_string(),
                bigdecimal_to_string(&usage_log.user_amount),
            )
        })
        .unwrap_or_else(|| (record.usage_log_id.to_string(), "0".to_string()));

    Ok(DistributionRecordResponse {
        id: record.id.to_string(),
        referrer_id: record.beneficiary_id.to_string(),
        referred_id,
        amount,
        commission: bigdecimal_to_string(&record.share_amount),
        status: record.status,
        created_at: record.created_at.to_rfc3339(),
    })
}

async fn build_referral_info(
    pool: &impl ConnectionTrait,
    beneficiary_id: Uuid,
    referral: keycompute_db::UserReferral,
) -> Result<ReferralInfo> {
    let user = keycompute_db::User::find_by_id(pool, referral.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    let usage_stats = keycompute_db::UsageLog::get_user_stats(pool, referral.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    let earnings = keycompute_db::DistributionRecord::get_earnings_for_referral(
        pool,
        beneficiary_id,
        referral.user_id,
    )
    .await
    .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    let email = user
        .as_ref()
        .map(|user| user.email.clone())
        .unwrap_or_else(|| referral.user_id.to_string());
    let name = user.and_then(|user| user.name);

    Ok(ReferralInfo {
        id: referral.user_id.to_string(),
        email,
        name,
        created_at: referral.created_at.to_rfc3339(),
        total_consumption: bigdecimal_to_string(&usage_stats.total_cost),
        earnings: bigdecimal_to_string(&earnings),
    })
}

// ==================== API Handlers ====================

/// 查看分销记录
///
/// GET /api/v1/distribution/records
/// - Admin: 查看所有记录
/// - 普通用户: 查看自己的记录
pub async fn list_distribution_records(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Query(query): Query<DistributionQuery>,
) -> Result<Json<Vec<DistributionRecordResponse>>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    let limit = query.limit.unwrap_or(20);
    let offset = query.offset.unwrap_or(0);

    let records = if auth.is_admin() {
        // Admin 可以查看所有记录，或按受益人筛选
        if let Some(beneficiary_id) = query.beneficiary_id {
            keycompute_db::DistributionRecord::find_by_beneficiary(
                pool,
                beneficiary_id,
                limit,
                offset,
            )
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        } else {
            keycompute_db::DistributionRecord::find_by_tenant(pool, auth.tenant_id, limit, offset)
                .await
                .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        }
    } else {
        // 普通用户只能查看自己的记录
        keycompute_db::DistributionRecord::find_by_beneficiary(pool, auth.user_id, limit, offset)
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
    };

    let filtered_records = records
        .into_iter()
        .filter(|r| {
            if let Some(ref status) = query.status {
                r.status == *status
            } else {
                true
            }
        })
        .filter(|r| {
            if let Some(ref level) = query.level {
                r.level == *level
            } else {
                true
            }
        })
        .collect::<Vec<_>>();

    let mut responses = Vec::with_capacity(filtered_records.len());
    for record in filtered_records {
        responses.push(build_distribution_record_response(pool, record).await?);
    }

    Ok(Json(responses))
}

/// 获取分销统计
///
/// GET /api/v1/distribution/stats
pub async fn get_distribution_stats(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<DistributionStatsResponse>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 检查分销系统是否启用（普通用户）
    if !auth.is_admin() {
        check_distribution_enabled(pool).await?;
    }

    // 获取当前用户的分销统计
    let stats = keycompute_db::DistributionRecord::get_stats_by_beneficiary(pool, auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    // 获取按层级的统计
    let level_stats =
        keycompute_db::DistributionRecord::get_level_stats_by_beneficiary(pool, auth.user_id)
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    // 获取推荐统计
    let referral_stats = keycompute_db::UserReferral::get_stats_by_referrer(pool, auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    Ok(Json(DistributionStatsResponse {
        total_earnings: bigdecimal_to_string(&stats.total_amount),
        pending_amount: bigdecimal_to_string(&stats.pending_amount),
        settled_amount: bigdecimal_to_string(&stats.settled_amount),
        currency: "CNY".to_string(),
        level1_earnings: bigdecimal_to_string(&level_stats.level1_amount),
        level2_earnings: bigdecimal_to_string(&level_stats.level2_amount),
        referral_count: referral_stats.total_referrals,
    }))
}

/// 查看分销规则列表
///
/// GET /api/v1/distribution/rules
/// 仅 Admin 可访问
pub async fn list_distribution_rules(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<Vec<DistributionRuleResponse>>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 查询租户的所有规则
    let rules = keycompute_db::TenantDistributionRule::find_all_by_tenant(pool, auth.tenant_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    let responses: Vec<DistributionRuleResponse> = rules
        .into_iter()
        .map(|r| DistributionRuleResponse {
            id: r.id.to_string(),
            name: r.name,
            commission_rate: r.commission_rate.to_string().parse().unwrap_or(0.0),
            min_purchase_amount: None,   // 数据库模型中暂无此字段
            max_commission_amount: None, // 数据库模型中暂无此字段
            is_active: r.is_active,
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(responses))
}

/// 创建分销规则
///
/// POST /api/v1/distribution/rules
/// 仅 Admin 可访问
pub async fn create_distribution_rule(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Json(req): Json<CreateDistributionRuleRequest>,
) -> Result<Json<DistributionRuleResponse>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    // 验证参数
    if req.commission_rate < 0.0 || req.commission_rate > 1.0 {
        return Err(ApiError::BadRequest(
            "commission_rate must be between 0.0 and 1.0".to_string(),
        ));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 全局规则使用 Uuid::nil() 表示对系统所有用户生效（租户级全局规则）
    // 采用 upsert 语义：如已存在同租户的 priority=100 全局规则则更新（并重新激活），否则创建。
    //
    // 并发安全：upsert 在写库事务 + 租户级 advisory lock 内原子执行
    //（见 TenantDistributionRule::upsert_global_override），消除并发请求下
    // check-then-create/update 的 TOCTOU 重复创建问题，并在提交前校验
    // priority=100 全局规则的唯一性；事务由 DbRouter 路由到写库，
    // 不受读副本复制延迟影响。
    let new_rate = string_to_bigdecimal(&req.commission_rate.to_string())?;
    let rule = keycompute_db::TenantDistributionRule::upsert_global_override(
        pool,
        auth.tenant_id,
        &req.name,
        new_rate,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, tenant_id = %auth.tenant_id, "Failed to upsert distribution rule");
        ApiError::Internal("Distribution rule operation failed".to_string())
    })?;

    Ok(Json(DistributionRuleResponse {
        id: rule.id.to_string(),
        name: rule.name,
        commission_rate: req.commission_rate,
        min_purchase_amount: req.min_purchase_amount,
        max_commission_amount: req.max_commission_amount,
        is_active: rule.is_active,
        created_at: rule.created_at.to_rfc3339(),
    }))
}

/// 更新分销规则
///
/// PUT /api/v1/distribution/rules/{id}
/// 仅 Admin 可访问
pub async fn update_distribution_rule(
    auth: AuthExtractor,
    Path(rule_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<UpdateDistributionRuleRequest>,
) -> Result<Json<DistributionRuleResponse>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    // 验证参数
    if let Some(rate) = req.commission_rate
        && !(0.0..=1.0).contains(&rate)
    {
        return Err(ApiError::BadRequest(
            "commission_rate must be between 0.0 and 1.0".to_string(),
        ));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 查找规则
    let rule = keycompute_db::TenantDistributionRule::find_by_id(pool, rule_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound("Distribution rule not found".to_string()))?;

    // 更新规则
    let update_req = keycompute_db::UpdateDistributionRuleRequest {
        name: req.name,
        description: None,
        commission_rate: req.commission_rate.and_then(|r| r.to_string().parse().ok()),
        priority: None,
        is_active: req.is_active,
        effective_until: None,
    };

    let updated_rule = rule
        .update(pool, &update_req)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update rule: {}", e)))?;

    Ok(Json(DistributionRuleResponse {
        id: updated_rule.id.to_string(),
        name: updated_rule.name,
        commission_rate: req.commission_rate.unwrap_or_else(|| {
            updated_rule
                .commission_rate
                .to_string()
                .parse()
                .unwrap_or(0.0)
        }),
        min_purchase_amount: req.min_purchase_amount,
        max_commission_amount: req.max_commission_amount,
        is_active: updated_rule.is_active,
        created_at: updated_rule.created_at.to_rfc3339(),
    }))
}

/// 删除分销规则
///
/// DELETE /api/v1/distribution/rules/{id}
/// 仅 Admin 可访问
pub async fn delete_distribution_rule(
    auth: AuthExtractor,
    Path(rule_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(ApiError::Auth("Admin permission required".to_string()));
    }

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 查找并删除规则
    let rule = keycompute_db::TenantDistributionRule::find_by_id(pool, rule_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound("Distribution rule not found".to_string()))?;

    rule.delete(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete rule: {}", e)))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Distribution rule deleted",
        "rule_id": rule_id.to_string(),
        "deleted_by": auth.user_id.to_string(),
    })))
}

/// 获取当前用户的分销收益
///
/// GET /api/v1/me/distribution/earnings
pub async fn get_my_distribution_earnings(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<UserDistributionEarningsResponse>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 检查分销系统是否启用
    check_distribution_enabled(pool).await?;

    // 获取分销统计
    let stats = keycompute_db::DistributionRecord::get_stats_by_beneficiary(pool, auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    // 获取推荐统计
    let referral_stats = keycompute_db::UserReferral::get_stats_by_referrer(pool, auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    Ok(Json(UserDistributionEarningsResponse {
        user_id: auth.user_id.to_string(),
        total_earnings: bigdecimal_to_string(&stats.total_amount),
        pending_amount: bigdecimal_to_string(&stats.pending_amount),
        settled_amount: bigdecimal_to_string(&stats.settled_amount),
        currency: "CNY".to_string(),
        level1_referrals: referral_stats.level1_count,
        level2_referrals: referral_stats.level2_count,
    }))
}

/// 获取当前用户的推荐列表
///
/// GET /api/v1/me/distribution/referrals
pub async fn get_my_referrals(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<Vec<ReferralInfo>>> {
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;

    // 检查分销系统是否启用
    check_distribution_enabled(pool).await?;

    // 获取一级推荐
    let level1_referrals = keycompute_db::UserReferral::find_by_level1_referrer(pool, auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    // 获取二级推荐
    let level2_referrals = keycompute_db::UserReferral::find_by_level2_referrer(pool, auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    // 合并并转换为响应格式，查询真实收益
    let mut referrals: Vec<ReferralInfo> = Vec::new();

    for referral in level1_referrals {
        referrals.push(build_referral_info(pool, auth.user_id, referral).await?);
    }

    for referral in level2_referrals {
        referrals.push(build_referral_info(pool, auth.user_id, referral).await?);
    }

    Ok(Json(referrals))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distribution_query_default_limit() {
        let query: DistributionQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.limit, Some(20));
    }

    #[test]
    fn test_create_distribution_rule_request_deserialize() {
        let json = r#"{
            "name": "默认分销规则",
            "commission_rate": 0.03
        }"#;
        let req: CreateDistributionRuleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.commission_rate, 0.03);
        assert_eq!(req.name, "默认分销规则");
    }

    #[test]
    fn test_distribution_stats_response_serialize() {
        let stats = DistributionStatsResponse {
            total_earnings: "100.00".to_string(),
            pending_amount: "30.00".to_string(),
            settled_amount: "70.00".to_string(),
            currency: "CNY".to_string(),
            level1_earnings: "60.00".to_string(),
            level2_earnings: "40.00".to_string(),
            referral_count: 5,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("100.00"));
        assert!(json.contains("CNY"));
    }

    #[test]
    fn test_build_invite_link_uses_short_ref_param() {
        let link = build_invite_link(
            "https://app.example.com",
            "6aac8ab5-aeec-48b8-a4cc-0a446d952862",
            None,
        )
        .unwrap();

        assert_eq!(
            link,
            "https://app.example.com/auth/register?ref=6aac8ab5-aeec-48b8-a4cc-0a446d952862"
        );
    }

    #[test]
    fn test_build_invite_link_preserves_source_param() {
        let link =
            build_invite_link("https://app.example.com", "abc123", Some("campaign")).unwrap();

        assert_eq!(
            link,
            "https://app.example.com/auth/register?ref=abc123&source=campaign"
        );
    }

    #[test]
    fn test_build_invite_link_preserves_base_path_and_encodes_source() {
        let link = build_invite_link(
            "https://app.example.com/console/",
            "abc123",
            Some("email campaign&fall"),
        )
        .unwrap();

        assert_eq!(
            link,
            "https://app.example.com/console/auth/register?ref=abc123&source=email+campaign%26fall"
        );
    }
}
