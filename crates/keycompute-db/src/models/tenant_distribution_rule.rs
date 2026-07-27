use crate::DbError;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 租户分销规则模型
#[derive(Debug, Clone, FromQueryResult, Serialize, Deserialize)]
pub struct TenantDistributionRule {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub beneficiary_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub commission_rate: BigDecimal,
    pub priority: i32,
    pub is_active: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 创建分销规则请求
#[derive(Debug, Clone, Deserialize)]
pub struct CreateDistributionRuleRequest {
    pub tenant_id: Uuid,
    pub beneficiary_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub commission_rate: BigDecimal,
    pub priority: Option<i32>,
    pub effective_from: Option<DateTime<Utc>>,
    pub effective_until: Option<DateTime<Utc>>,
}

/// 更新分销规则请求
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateDistributionRuleRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub commission_rate: Option<BigDecimal>,
    pub priority: Option<i32>,
    pub is_active: Option<bool>,
    pub effective_until: Option<DateTime<Utc>>,
}

impl TenantDistributionRule {
    /// Admin API 创建的全局覆盖规则的优先级约定
    ///
    /// 优先级约定（Priority Convention）：100 = Admin 覆盖，10 = L1 初始化，5 = L2 初始化，0 = per-user 默认
    pub const GLOBAL_OVERRIDE_PRIORITY: i32 = 100;

    /// 原子地 upsert 租户级全局覆盖规则（beneficiary_id = nil, priority = 100）
    ///
    /// 如已存在同租户的 priority=100 全局规则则更新（并重新激活），否则创建。
    /// 若存在历史遗留的多条重复全局规则，保留最早创建的一条（与计费匹配的
    /// `created_at ASC` 排序一致），其余在同一事务内自动停用（自愈），避免
    /// 旧数据导致本端点对该租户永久失败。
    ///
    /// 并发安全：整个 check-then-create/update 在写库事务内执行，并先获取按租户
    /// 划分的事务级 advisory lock（`pg_advisory_xact_lock`，提交/回滚时自动释放），
    /// 串行化同一租户的并发 upsert，消除 TOCTOU 重复创建竞态；提交前再校验
    /// 激活态 priority=100 全局规则的唯一性，异常时回滚并返回错误。
    pub async fn upsert_global_override(
        db: &(impl ConnectionTrait + TransactionTrait),
        tenant_id: Uuid,
        name: &str,
        commission_rate: BigDecimal,
    ) -> Result<TenantDistributionRule, DbError> {
        let txn = db.begin().await?;

        // 事务级 advisory lock：双 key 分别隔离业务命名空间与租户。
        // hashtext 映射到 int4（2^32 空间），不同租户的 key 存在碰撞可能，但碰撞仅导致
        // 跨租户 upsert 被不必要地串行化（性能影响），不影响正确性——下方的查找与
        // 唯一性校验始终按 tenant_id 过滤。
        txn.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtext('kc_dist_rule_upsert'), hashtext($1))",
            [tenant_id.to_string().into()],
        ))
        .await?;

        // 使用 find_all_by_tenant（包含已禁用/已过期规则）查找，避免遗漏后重复创建
        let mut globals: Vec<_> = Self::find_all_by_tenant(&txn, tenant_id)
            .await?
            .into_iter()
            .filter(|r| {
                r.beneficiary_id == Uuid::nil() && r.priority == Self::GLOBAL_OVERRIDE_PRIORITY
            })
            .collect();

        // 确定性保留最早创建的一条（created_at 相同时按 id 决胜），即计费匹配
        // `priority DESC, created_at ASC` 排序下实际命中的那条；其余历史遗留
        // 重复规则在本事务内停用自愈，而非回滚报错
        globals.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        let mut globals = globals.into_iter();
        let existing = globals.next();
        for extra in globals {
            tracing::warn!(
                tenant_id = %tenant_id,
                rule_id = %extra.id,
                "deactivating duplicate global override rule during upsert self-heal"
            );
            extra
                .update(
                    &txn,
                    &UpdateDistributionRuleRequest {
                        name: None,
                        description: None,
                        commission_rate: None,
                        priority: None,
                        is_active: Some(false),
                        effective_until: None,
                    },
                )
                .await?;
        }

        let rule = match existing {
            Some(existing) => {
                // 更新已有的全局规则，并确保其处于激活状态
                existing
                    .update(
                        &txn,
                        &UpdateDistributionRuleRequest {
                            name: Some(name.to_string()),
                            description: None,
                            commission_rate: Some(commission_rate),
                            priority: None,
                            is_active: Some(true),
                            effective_until: None,
                        },
                    )
                    .await?
            }
            None => {
                // 首次创建全局规则
                Self::create(
                    &txn,
                    &CreateDistributionRuleRequest {
                        tenant_id,
                        beneficiary_id: Uuid::nil(),
                        name: name.to_string(),
                        description: None,
                        commission_rate,
                        priority: Some(Self::GLOBAL_OVERRIDE_PRIORITY),
                        effective_from: Some(Utc::now()),
                        effective_until: None,
                    },
                )
                .await?
            }
        };

        // 提交前校验优先级唯一性：同租户只允许一条激活态的 priority=100 全局规则。
        // advisory lock 已阻止本方法自身的并发竞态，历史遗留重复已在上方自愈停用，
        // 此处兜底检测其他写入路径引入的激活态重复数据，避免将重复状态固化。
        let count_row = txn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT COUNT(*) FROM tenant_distribution_rules WHERE tenant_id = $1 AND beneficiary_id = $2 AND priority = $3 AND is_active = TRUE",
                [
                    tenant_id.into(),
                    Uuid::nil().into(),
                    Self::GLOBAL_OVERRIDE_PRIORITY.into(),
                ],
            ))
            .await?
            .ok_or_else(|| DbError::Other("count query failed".to_string()))?;
        let count: i64 = count_row
            .try_get_by_index(0)
            .map_err(DbError::DatabaseError)?;
        if count != 1 {
            // 未 commit 的事务在 Drop 时自动回滚
            return Err(DbError::Other(format!(
                "active global override rule uniqueness violated for tenant {}: expected 1, found {}",
                tenant_id, count
            )));
        }

        txn.commit().await?;
        Ok(rule)
    }

    /// 创建新分销规则
    pub async fn create(
        db: &impl ConnectionTrait,
        req: &CreateDistributionRuleRequest,
    ) -> Result<TenantDistributionRule, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO tenant_distribution_rules (
                tenant_id, beneficiary_id, name, description, commission_rate,
                priority, effective_from, effective_until
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
            [
                req.tenant_id.into(),
                req.beneficiary_id.into(),
                req.name.as_str().into(),
                req.description.clone().into(),
                req.commission_rate.clone().into(),
                req.priority.unwrap_or(0).into(),
                req.effective_from.unwrap_or_else(Utc::now).into(),
                req.effective_until.into(),
            ],
        );
        let rule = TenantDistributionRule::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("create failed to return row".to_string()))?;

        Ok(rule)
    }

    /// 根据 ID 查找规则
    pub async fn find_by_id(
        db: &impl ConnectionTrait,
        id: Uuid,
    ) -> Result<Option<TenantDistributionRule>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM tenant_distribution_rules WHERE id = $1",
            [id.into()],
        );
        let rule = TenantDistributionRule::find_by_statement(stmt)
            .one(db)
            .await?;

        Ok(rule)
    }

    /// 查找租户的所有有效规则
    ///
    /// 排序末位的 id ASC 决胜确保 created_at 相同时顺序确定，
    /// 与 upsert_global_override 自愈时的保留排序（created_at, id）一致
    pub async fn find_by_tenant(
        db: &impl ConnectionTrait,
        tenant_id: Uuid,
    ) -> Result<Vec<TenantDistributionRule>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT * FROM tenant_distribution_rules
            WHERE tenant_id = $1
              AND is_active = TRUE
              AND effective_from <= NOW()
              AND (effective_until IS NULL OR effective_until > NOW())
            ORDER BY priority DESC, created_at ASC, id ASC
            "#,
            [tenant_id.into()],
        );
        let rules = TenantDistributionRule::find_by_statement(stmt)
            .all(db)
            .await?;

        Ok(rules)
    }

    /// 查找租户的所有规则（包括已禁用）
    pub async fn find_all_by_tenant(
        db: &impl ConnectionTrait,
        tenant_id: Uuid,
    ) -> Result<Vec<TenantDistributionRule>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM tenant_distribution_rules WHERE tenant_id = $1 ORDER BY priority DESC",
            [tenant_id.into()],
        );
        let rules = TenantDistributionRule::find_by_statement(stmt)
            .all(db)
            .await?;

        Ok(rules)
    }

    /// 更新规则
    pub async fn update(
        &self,
        db: &impl ConnectionTrait,
        req: &UpdateDistributionRuleRequest,
    ) -> Result<TenantDistributionRule, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            UPDATE tenant_distribution_rules
            SET name = COALESCE($1, name),
                description = COALESCE($2, description),
                commission_rate = COALESCE($3, commission_rate),
                priority = COALESCE($4, priority),
                is_active = COALESCE($5, is_active),
                effective_until = COALESCE($6, effective_until),
                updated_at = NOW()
            WHERE id = $7
            RETURNING *
            "#,
            [
                req.name.clone().into(),
                req.description.clone().into(),
                req.commission_rate.clone().into(),
                req.priority.into(),
                req.is_active.into(),
                req.effective_until.into(),
                self.id.into(),
            ],
        );
        let rule = TenantDistributionRule::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("update failed to return row".to_string()))?;

        Ok(rule)
    }

    /// 删除规则
    pub async fn delete(&self, db: &impl ConnectionTrait) -> Result<(), DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM tenant_distribution_rules WHERE id = $1",
            [self.id.into()],
        );
        db.execute(stmt).await?;

        Ok(())
    }

    /// 检查规则是否有效
    pub fn is_effective(&self) -> bool {
        if !self.is_active {
            return false;
        }

        let now = Utc::now();

        if self.effective_from > now {
            return false;
        }

        if let Some(effective_until) = self.effective_until
            && effective_until <= now
        {
            return false;
        }

        true
    }
}
