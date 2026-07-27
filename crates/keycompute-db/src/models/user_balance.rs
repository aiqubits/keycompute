//! 用户余额模型

use crate::DbError;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 交易类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionType {
    /// 充值
    Recharge,
    /// 消费
    Consume,
    /// 冻结
    Freeze,
    /// 解冻
    Unfreeze,
    /// 小费入账（tips 转为可用余额）
    TipCredit,
}

impl TransactionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionType::Recharge => "recharge",
            TransactionType::Consume => "consume",
            TransactionType::Freeze => "freeze",
            TransactionType::Unfreeze => "unfreeze",
            TransactionType::TipCredit => "tip_credit",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recharge" => Some(TransactionType::Recharge),
            "consume" => Some(TransactionType::Consume),
            "freeze" => Some(TransactionType::Freeze),
            "unfreeze" => Some(TransactionType::Unfreeze),
            "tip_credit" => Some(TransactionType::TipCredit),
            _ => None,
        }
    }
}

/// 用户余额模型
#[derive(Debug, Clone, FromQueryResult, Serialize, Deserialize)]
pub struct UserBalance {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    /// 可用余额
    pub available_balance: Decimal,
    /// 冻结余额
    pub frozen_balance: Decimal,
    /// 累计充值金额
    pub total_recharged: Decimal,
    /// 累计消费金额
    pub total_consumed: Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UserBalance {
    /// 总余额（可用 + 冻结）
    pub fn total_balance(&self) -> Decimal {
        self.available_balance + self.frozen_balance
    }

    /// 检查可用余额是否足够
    pub fn can_deduct(&self, amount: Decimal) -> bool {
        self.available_balance >= amount
    }
}

impl UserBalance {
    /// 获取或创建用户余额记录
    pub async fn get_or_create(
        db: &impl ConnectionTrait,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<UserBalance, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO user_balances (tenant_id, user_id) VALUES ($1, $2) ON CONFLICT (user_id) DO UPDATE SET updated_at = NOW() RETURNING *"#,
            [tenant_id.into(), user_id.into()],
        );
        let balance = UserBalance::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("get_or_create failed".to_string()))?;

        Ok(balance)
    }

    /// 根据用户ID查找余额
    pub async fn find_by_user(
        db: &impl ConnectionTrait,
        user_id: Uuid,
    ) -> Result<Option<UserBalance>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM user_balances WHERE user_id = $1",
            [user_id.into()],
        );
        let balance = UserBalance::find_by_statement(stmt).one(db).await?;
        Ok(balance)
    }

    /// 批量根据用户ID查找余额
    pub async fn find_by_users(
        db: &impl ConnectionTrait,
        user_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, UserBalance>, DbError> {
        if user_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM user_balances WHERE user_id = ANY($1)",
            [user_ids.to_vec().into()],
        );
        let balances = UserBalance::find_by_statement(stmt).all(db).await?;
        Ok(balances.into_iter().map(|b| (b.user_id, b)).collect())
    }

    /// 充值（自身创建事务执行）
    pub async fn recharge(
        db: &(impl ConnectionTrait + TransactionTrait),
        user_id: Uuid,
        tenant_id: Uuid,
        amount: Decimal,
        order_id: Option<Uuid>,
        description: Option<&str>,
    ) -> Result<(UserBalance, BalanceTransaction), DbError> {
        let tx = db.begin().await?;

        let result =
            Self::recharge_in_tx(&tx, user_id, tenant_id, amount, order_id, description).await?;

        tx.commit().await?;
        Ok(result)
    }

    /// 充值（在已有事务内执行）
    ///
    /// 与 [`recharge`] 功能相同，但不自行创建事务，接受外部传入的事务引用。
    /// 用于需要将充值操作与其它 DB 操作（如订单更新）放在同一事务中的场景。
    pub async fn recharge_in_tx(
        tx: &DatabaseTransaction,
        user_id: Uuid,
        tenant_id: Uuid,
        amount: Decimal,
        order_id: Option<Uuid>,
        description: Option<&str>,
    ) -> Result<(UserBalance, BalanceTransaction), DbError> {
        // 先物化余额行，再加锁读取。如果两笔“首次充值”并发，
        // 直接 SELECT FOR UPDATE 会让两个事务都读到空集，导致第二笔
        // balance_transactions 的 balance_before/after 与实际余额不一致。
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO user_balances
               (user_id, tenant_id, available_balance, frozen_balance, total_recharged, total_consumed)
               VALUES ($1, $2, 0, 0, 0, 0)
               ON CONFLICT (user_id) DO NOTHING"#,
            [user_id.into(), tenant_id.into()],
        ))
        .await?;

        // 获取当前余额（加锁）
        let lock_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM user_balances WHERE user_id = $1 FOR UPDATE",
            [user_id.into()],
        );
        let balance = UserBalance::find_by_statement(lock_stmt)
            .one(tx)
            .await?
            .ok_or_else(|| DbError::Other("recharge balance row disappeared".to_string()))?;

        let balance_before = balance.available_balance;
        let balance_after = balance_before + amount;

        // 已持有行锁，直接更新即可得到与流水一致的前后余额。
        let update_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE user_balances
               SET available_balance = available_balance + $1,
                   total_recharged = total_recharged + $1,
                   updated_at = NOW()
               WHERE user_id = $2
               RETURNING *"#,
            [amount.into(), user_id.into()],
        );
        let updated_balance = UserBalance::find_by_statement(update_stmt)
            .one(tx)
            .await?
            .ok_or_else(|| DbError::Other("recharge balance update failed".to_string()))?;

        // 记录交易
        let transaction = BalanceTransaction::create_internal(
            tx,
            updated_balance.tenant_id,
            user_id,
            order_id,
            None,
            TransactionType::Recharge,
            amount,
            balance_before,
            balance_after,
            description,
        )
        .await?;

        Ok((updated_balance, transaction))
    }

    /// 消费（事务内执行）
    pub async fn consume(
        db: &(impl ConnectionTrait + TransactionTrait),
        user_id: Uuid,
        amount: Decimal,
        usage_log_id: Option<Uuid>,
        description: Option<&str>,
    ) -> Result<(UserBalance, BalanceTransaction), DbError> {
        let tx = db.begin().await?;

        let lock_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM user_balances WHERE user_id = $1 FOR UPDATE",
            [user_id.into()],
        );
        let balance = UserBalance::find_by_statement(lock_stmt).one(&tx).await?;

        let balance = match balance {
            Some(b) => b,
            None => return Err(DbError::not_found("UserBalance", user_id.to_string())),
        };

        if balance.available_balance < amount {
            return Err(DbError::insufficient_balance(
                amount.to_string(),
                balance.available_balance.to_string(),
            ));
        }

        let balance_before = balance.available_balance;
        let balance_after = balance_before - amount;

        let update_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE user_balances SET available_balance = available_balance - $1, total_consumed = total_consumed + $1, updated_at = NOW() WHERE user_id = $2 RETURNING *"#,
            [amount.into(), user_id.into()],
        );
        let updated_balance = UserBalance::find_by_statement(update_stmt)
            .one(&tx)
            .await?
            .ok_or_else(|| DbError::not_found("UserBalance", user_id.to_string()))?;

        let transaction = BalanceTransaction::create_internal(
            &tx,
            balance.tenant_id,
            user_id,
            None,
            usage_log_id,
            TransactionType::Consume,
            -amount,
            balance_before,
            balance_after,
            description,
        )
        .await?;

        tx.commit().await?;

        Ok((updated_balance, transaction))
    }

    /// 冻结余额
    pub async fn freeze(
        db: &(impl ConnectionTrait + TransactionTrait),
        user_id: Uuid,
        amount: Decimal,
        description: Option<&str>,
    ) -> Result<(UserBalance, BalanceTransaction), DbError> {
        let tx = db.begin().await?;

        let lock_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM user_balances WHERE user_id = $1 FOR UPDATE",
            [user_id.into()],
        );
        let balance = UserBalance::find_by_statement(lock_stmt).one(&tx).await?;

        let balance = match balance {
            Some(b) => b,
            None => return Err(DbError::not_found("UserBalance", user_id.to_string())),
        };

        if balance.available_balance < amount {
            return Err(DbError::insufficient_balance(
                amount.to_string(),
                balance.available_balance.to_string(),
            ));
        }

        let balance_before = balance.available_balance;
        let balance_after = balance_before - amount;

        let update_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE user_balances SET available_balance = available_balance - $1, frozen_balance = frozen_balance + $1, updated_at = NOW() WHERE user_id = $2 RETURNING *"#,
            [amount.into(), user_id.into()],
        );
        let updated_balance = UserBalance::find_by_statement(update_stmt)
            .one(&tx)
            .await?
            .ok_or_else(|| DbError::not_found("UserBalance", user_id.to_string()))?;

        let transaction = BalanceTransaction::create_internal(
            &tx,
            balance.tenant_id,
            user_id,
            None,
            None,
            TransactionType::Freeze,
            -amount,
            balance_before,
            balance_after,
            description,
        )
        .await?;

        tx.commit().await?;

        Ok((updated_balance, transaction))
    }

    /// 小费入账（tips 转为可用余额）
    ///
    /// 注意：调用方**必须**已在外部开启数据库事务（`db.begin()`），
    /// 此方法依赖事务内的 `SELECT ... FOR UPDATE` 行锁保证并发安全。
    /// 当前唯一调用方 node_tips.rs 已满足此前提。
    ///
    /// 签名限定 `&DatabaseTransaction` 而非 `&impl ConnectionTrait`，
    /// 以在编译期强制事务上下文约束。
    pub async fn credit_tips(
        db: &DatabaseTransaction,
        user_id: Uuid,
        tenant_id: Uuid,
        amount: Decimal,
        description: Option<&str>,
    ) -> Result<(UserBalance, BalanceTransaction), DbError> {
        let lock_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM user_balances WHERE user_id = $1 FOR UPDATE",
            [user_id.into()],
        );
        let balance = UserBalance::find_by_statement(lock_stmt).one(db).await?;

        let effective_tenant_id = balance.as_ref().map(|b| b.tenant_id).unwrap_or(tenant_id);

        let upsert_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO user_balances (user_id, tenant_id, available_balance, total_recharged) VALUES ($1, $2, $3, $3) ON CONFLICT (user_id) DO UPDATE SET available_balance = user_balances.available_balance + $3, total_recharged = user_balances.total_recharged + $3, updated_at = NOW() RETURNING *"#,
            [user_id.into(), effective_tenant_id.into(), amount.into()],
        );
        let updated_balance = UserBalance::find_by_statement(upsert_stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("credit_tips upsert failed".to_string()))?;

        let balance_after = updated_balance.available_balance;
        let balance_before = balance_after - amount;

        let transaction = BalanceTransaction::create_internal(
            db,
            updated_balance.tenant_id,
            user_id,
            None,
            None,
            TransactionType::TipCredit,
            amount,
            balance_before,
            balance_after,
            description,
        )
        .await?;

        Ok((updated_balance, transaction))
    }

    /// 解冻余额
    pub async fn unfreeze(
        db: &(impl ConnectionTrait + TransactionTrait),
        user_id: Uuid,
        amount: Decimal,
        description: Option<&str>,
    ) -> Result<(UserBalance, BalanceTransaction), DbError> {
        let tx = db.begin().await?;

        let lock_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM user_balances WHERE user_id = $1 FOR UPDATE",
            [user_id.into()],
        );
        let balance = UserBalance::find_by_statement(lock_stmt).one(&tx).await?;

        let balance = match balance {
            Some(b) => b,
            None => return Err(DbError::not_found("UserBalance", user_id.to_string())),
        };

        if balance.frozen_balance < amount {
            return Err(DbError::insufficient_balance(
                amount.to_string(),
                balance.frozen_balance.to_string(),
            ));
        }

        let balance_before = balance.available_balance;
        let balance_after = balance_before + amount;

        let update_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE user_balances SET available_balance = available_balance + $1, frozen_balance = frozen_balance - $1, updated_at = NOW() WHERE user_id = $2 RETURNING *"#,
            [amount.into(), user_id.into()],
        );
        let updated_balance = UserBalance::find_by_statement(update_stmt)
            .one(&tx)
            .await?
            .ok_or_else(|| DbError::not_found("UserBalance", user_id.to_string()))?;

        let transaction = BalanceTransaction::create_internal(
            &tx,
            balance.tenant_id,
            user_id,
            None,
            None,
            TransactionType::Unfreeze,
            amount,
            balance_before,
            balance_after,
            description,
        )
        .await?;

        tx.commit().await?;

        Ok((updated_balance, transaction))
    }
}

/// 余额变动记录模型
#[derive(Debug, Clone, FromQueryResult, Serialize, Deserialize)]
pub struct BalanceTransaction {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub order_id: Option<Uuid>,
    pub usage_log_id: Option<Uuid>,
    pub transaction_type: String,
    pub amount: Decimal,
    pub balance_before: Decimal,
    pub balance_after: Decimal,
    pub currency: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl BalanceTransaction {
    /// 内部创建交易记录
    #[allow(clippy::too_many_arguments)]
    async fn create_internal(
        db: &impl sea_orm::ConnectionTrait,
        tenant_id: Uuid,
        user_id: Uuid,
        order_id: Option<Uuid>,
        usage_log_id: Option<Uuid>,
        transaction_type: TransactionType,
        amount: Decimal,
        balance_before: Decimal,
        balance_after: Decimal,
        description: Option<&str>,
    ) -> Result<BalanceTransaction, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO balance_transactions (tenant_id, user_id, order_id, usage_log_id, transaction_type, amount, balance_before, balance_after, description) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING *"#,
            [
                tenant_id.into(),
                user_id.into(),
                order_id.into(),
                usage_log_id.into(),
                transaction_type.as_str().into(),
                amount.into(),
                balance_before.into(),
                balance_after.into(),
                description.map(String::from).into(),
            ],
        );
        let transaction = BalanceTransaction::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("create transaction failed".to_string()))?;

        Ok(transaction)
    }

    /// 查找用户的交易记录
    pub async fn find_by_user(
        db: &impl ConnectionTrait,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<BalanceTransaction>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM balance_transactions WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
            [user_id.into(), limit.into(), offset.into()],
        );
        let transactions = BalanceTransaction::find_by_statement(stmt).all(db).await?;
        Ok(transactions)
    }

    /// 获取交易类型枚举
    pub fn get_transaction_type(&self) -> Option<TransactionType> {
        TransactionType::parse(&self.transaction_type)
    }
}
