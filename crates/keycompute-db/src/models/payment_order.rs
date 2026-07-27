//! 支付订单模型

use crate::DbError;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 订单状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentOrderStatus {
    /// 待支付
    Pending,
    /// 已支付
    Paid,
    /// 支付失败
    Failed,
    /// 已关闭
    Closed,
}

impl PaymentOrderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PaymentOrderStatus::Pending => "pending",
            PaymentOrderStatus::Paid => "paid",
            PaymentOrderStatus::Failed => "failed",
            PaymentOrderStatus::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(PaymentOrderStatus::Pending),
            "paid" => Some(PaymentOrderStatus::Paid),
            "failed" => Some(PaymentOrderStatus::Failed),
            "closed" => Some(PaymentOrderStatus::Closed),
            _ => None,
        }
    }
}

impl std::fmt::Display for PaymentOrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 支付方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentMethod {
    /// 支付宝
    Alipay,
    /// 微信支付
    WechatPay,
}

impl PaymentMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            PaymentMethod::Alipay => "alipay",
            PaymentMethod::WechatPay => "wechatpay",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "alipay" => Some(PaymentMethod::Alipay),
            "wechatpay" => Some(PaymentMethod::WechatPay),
            _ => None,
        }
    }
}

/// 支付订单模型
#[derive(Debug, Clone, FromQueryResult, Serialize, Deserialize)]
pub struct PaymentOrder {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    /// 商户订单号（外部订单号）
    pub out_trade_no: String,
    /// 通用渠道交易号展示字段
    pub trade_no: Option<String>,
    /// 支付渠道交易号
    pub provider_trade_no: Option<String>,
    /// 订单金额（单位：元）
    pub amount: Decimal,
    /// 币种
    pub currency: String,
    /// 订单状态
    pub status: String,
    /// 支付方式
    pub payment_method: String,
    /// 支付场景
    pub payment_scene: String,
    /// 商品标题
    pub subject: String,
    /// 商品描述
    pub body: Option<String>,
    /// 支付时间
    pub paid_at: Option<DateTime<Utc>>,
    /// 关闭时间
    pub closed_at: Option<DateTime<Utc>>,
    /// 过期时间
    pub expired_at: DateTime<Utc>,
    /// 支付URL
    pub pay_url: Option<String>,
    /// 回调通知原始数据
    pub notify_data: Option<serde_json::Value>,
    /// 支付渠道返回的非敏感展示数据
    pub provider_payload: Option<serde_json::Value>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    /// 备注信息
    pub remarks: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreditPaidOrderError {
    #[error(transparent)]
    Database(#[from] DbError),
    #[error("payment order disappeared")]
    OrderNotFound,
    #[error("provider event id was already used by a different notification")]
    NotificationConflict,
    #[error("provider transaction does not match the paid order")]
    ProviderIdentityMismatch,
    #[error("cannot pay order in state {0}")]
    InvalidOrderStatus(String),
    #[error("concurrent order transition")]
    ConcurrentTransition,
}

#[derive(FromQueryResult)]
struct PaymentNotificationIdentity {
    order_id: Option<Uuid>,
    payload_digest: String,
}

impl PaymentOrder {
    /// 获取订单状态枚举
    pub fn get_status(&self) -> Option<PaymentOrderStatus> {
        PaymentOrderStatus::parse(&self.status)
    }

    /// 检查订单是否可支付
    pub fn is_payable(&self) -> bool {
        self.get_status() == Some(PaymentOrderStatus::Pending) && self.expired_at > Utc::now()
    }

    /// 检查订单是否已过期
    pub fn is_expired(&self) -> bool {
        self.expired_at <= Utc::now()
    }
}

/// 创建支付订单请求
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePaymentOrderRequest {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    /// 订单金额（单位：元）
    pub amount: Decimal,
    /// 商品标题
    pub subject: String,
    /// 商品描述
    pub body: Option<String>,
    /// 支付方式
    pub payment_method: PaymentMethod,
    /// 支付场景（page/wap/qr/native）
    pub payment_scene: String,
    /// 渠道订单与本地订单共享的绝对过期时间
    pub expired_at: DateTime<Utc>,
}

impl PaymentOrder {
    /// 创建新订单
    pub async fn create(
        db: &impl ConnectionTrait,
        req: &CreatePaymentOrderRequest,
        out_trade_no: &str,
        pay_url: &str,
    ) -> Result<PaymentOrder, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO payment_orders (
                tenant_id, user_id, out_trade_no, amount,
                currency, status, payment_method, payment_scene, subject, body,
                expired_at, pay_url
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING *
            "#,
            [
                req.tenant_id.into(),
                req.user_id.into(),
                out_trade_no.into(),
                req.amount.into(),
                "CNY".into(),
                PaymentOrderStatus::Pending.as_str().into(),
                req.payment_method.as_str().into(),
                req.payment_scene.as_str().into(),
                req.subject.as_str().into(),
                req.body.clone().into(),
                req.expired_at.into(),
                pay_url.into(),
            ],
        );
        let order = PaymentOrder::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("create failed to return row".to_string()))?;

        Ok(order)
    }

    /// 根据ID查找订单
    pub async fn find_by_id(
        db: &impl ConnectionTrait,
        id: Uuid,
    ) -> Result<Option<PaymentOrder>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM payment_orders WHERE id = $1",
            [id.into()],
        );
        let order = PaymentOrder::find_by_statement(stmt).one(db).await?;
        Ok(order)
    }

    /// 根据商户订单号查找订单
    pub async fn find_by_out_trade_no(
        db: &impl ConnectionTrait,
        out_trade_no: &str,
    ) -> Result<Option<PaymentOrder>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM payment_orders WHERE out_trade_no = $1",
            [out_trade_no.into()],
        );
        let order = PaymentOrder::find_by_statement(stmt).one(db).await?;
        Ok(order)
    }

    /// 根据支付宝交易号查找订单
    pub async fn find_by_trade_no(
        db: &impl ConnectionTrait,
        trade_no: &str,
    ) -> Result<Option<PaymentOrder>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM payment_orders WHERE trade_no = $1",
            [trade_no.into()],
        );
        let order = PaymentOrder::find_by_statement(stmt).one(db).await?;
        Ok(order)
    }

    /// Atomically record a verified provider event, mark the order paid, and credit its balance.
    pub async fn credit_paid(
        db: &(impl ConnectionTrait + TransactionTrait),
        order_id: Uuid,
        provider_trade_no: &str,
        provider_event_id: &str,
        payload: serde_json::Value,
        description: &str,
    ) -> Result<bool, CreditPaidOrderError> {
        let tx = db.begin().await.map_err(DbError::from)?;
        let locked = Self::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM payment_orders WHERE id = $1 FOR UPDATE",
            [order_id.into()],
        ))
        .one(&tx)
        .await
        .map_err(DbError::from)?
        .ok_or(CreditPaidOrderError::OrderNotFound)?;
        let payload_digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&payload).map_err(|error| DbError::Other(error.to_string()))?,
        ));

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO payment_notifications(payment_method, provider_event_id, order_id, out_trade_no, processing_status, payload_digest)
               VALUES ($1, $2, $3, $4, 'received', $5)
               ON CONFLICT(payment_method, provider_event_id) DO NOTHING"#,
            [
                locked.payment_method.as_str().into(),
                provider_event_id.into(),
                locked.id.into(),
                locked.out_trade_no.as_str().into(),
                payload_digest.as_str().into(),
            ],
        ))
        .await
        .map_err(DbError::from)?;
        let notification = PaymentNotificationIdentity::find_by_statement(
            Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT order_id, payload_digest FROM payment_notifications WHERE payment_method=$1 AND provider_event_id=$2 FOR UPDATE",
                [locked.payment_method.as_str().into(), provider_event_id.into()],
            ),
        )
        .one(&tx)
        .await
        .map_err(DbError::from)?
        .ok_or(CreditPaidOrderError::NotificationConflict)?;
        if notification.order_id != Some(locked.id) || notification.payload_digest != payload_digest
        {
            return Err(CreditPaidOrderError::NotificationConflict);
        }

        if locked.status == PaymentOrderStatus::Paid.as_str() {
            if locked
                .provider_trade_no
                .as_deref()
                .or(locked.trade_no.as_deref())
                != Some(provider_trade_no)
            {
                return Err(CreditPaidOrderError::ProviderIdentityMismatch);
            }
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE payment_notifications SET processing_status='processed', processed_at=COALESCE(processed_at, NOW()) WHERE payment_method=$1 AND provider_event_id=$2",
                [locked.payment_method.as_str().into(), provider_event_id.into()],
            ))
            .await
            .map_err(DbError::from)?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(false);
        }
        if locked.status != PaymentOrderStatus::Pending.as_str() {
            return Err(CreditPaidOrderError::InvalidOrderStatus(locked.status));
        }

        let updated = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE payment_orders SET status='paid', trade_no=$1, provider_trade_no=$1, notify_data=$2, paid_at=NOW(), updated_at=NOW()
                   WHERE id=$3 AND status='pending'"#,
                [provider_trade_no.into(), payload.into(), locked.id.into()],
            ))
            .await
            .map_err(DbError::from)?;
        if updated.rows_affected() != 1 {
            return Err(CreditPaidOrderError::ConcurrentTransition);
        }
        crate::UserBalance::recharge_in_tx(
            &tx,
            locked.user_id,
            locked.tenant_id,
            locked.amount,
            Some(locked.id),
            Some(description),
        )
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE payment_notifications SET processing_status='processed', processed_at=NOW() WHERE payment_method=$1 AND provider_event_id=$2",
            [locked.payment_method.as_str().into(), provider_event_id.into()],
        ))
        .await
        .map_err(DbError::from)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(true)
    }

    /// 查找用户的订单列表
    pub async fn find_by_user(
        db: &impl ConnectionTrait,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PaymentOrder>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM payment_orders WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
            [user_id.into(), limit.into(), offset.into()],
        );
        let orders = PaymentOrder::find_by_statement(stmt).all(db).await?;
        Ok(orders)
    }

    /// 更新订单为已支付
    #[deprecated(
        since = "0.2.0",
        note = "此方法没有并发保护，请使用 PaymentService 中的 handle_notify 或 sync_order_status"
    )]
    pub async fn mark_as_paid(
        db: &impl ConnectionTrait,
        id: Uuid,
        trade_no: &str,
        notify_data: &serde_json::Value,
    ) -> Result<PaymentOrder, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE payment_orders SET status = $1, trade_no = $2, notify_data = $3, paid_at = NOW(), updated_at = NOW() WHERE id = $4 RETURNING *"#,
            [
                PaymentOrderStatus::Paid.as_str().into(),
                trade_no.into(),
                notify_data.clone().into(),
                id.into(),
            ],
        );
        let order = PaymentOrder::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::not_found("PaymentOrder", id.to_string()))?;
        Ok(order)
    }

    /// 更新订单为支付失败
    pub async fn mark_as_failed(
        db: &impl ConnectionTrait,
        id: Uuid,
    ) -> Result<PaymentOrder, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE payment_orders SET status = $1, updated_at = NOW() WHERE id = $2 AND status = $3 RETURNING *"#,
            [
                PaymentOrderStatus::Failed.as_str().into(),
                id.into(),
                PaymentOrderStatus::Pending.as_str().into(),
            ],
        );

        match PaymentOrder::find_by_statement(stmt).one(db).await? {
            Some(o) => Ok(o),
            None => {
                if let Some(existing) = Self::find_by_id(db, id).await? {
                    Err(DbError::InvalidOrderStatus {
                        expected: "pending".to_string(),
                        actual: existing.status,
                    })
                } else {
                    Err(DbError::not_found("PaymentOrder", id.to_string()))
                }
            }
        }
    }

    /// 关闭订单
    pub async fn close(db: &impl ConnectionTrait, id: Uuid) -> Result<PaymentOrder, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE payment_orders SET status = $1, closed_at = NOW(), updated_at = NOW() WHERE id = $2 AND status = $3 RETURNING *"#,
            [
                PaymentOrderStatus::Closed.as_str().into(),
                id.into(),
                PaymentOrderStatus::Pending.as_str().into(),
            ],
        );

        match PaymentOrder::find_by_statement(stmt).one(db).await? {
            Some(order) => Ok(order),
            None => {
                if let Some(existing) = Self::find_by_id(db, id).await? {
                    Err(DbError::InvalidOrderStatus {
                        expected: "pending".to_string(),
                        actual: existing.status,
                    })
                } else {
                    Err(DbError::not_found("PaymentOrder", id.to_string()))
                }
            }
        }
    }

    /// 关闭过期订单
    pub async fn close_expired_orders(db: &impl ConnectionTrait) -> Result<u64, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE payment_orders SET status = $1, closed_at = NOW(), updated_at = NOW() WHERE status = $2 AND expired_at < NOW()"#,
            [
                PaymentOrderStatus::Closed.as_str().into(),
                PaymentOrderStatus::Pending.as_str().into(),
            ],
        );
        let result = db.execute(stmt).await?;
        Ok(result.rows_affected())
    }
}

/// 支付订单统计
#[derive(Debug, Clone, FromQueryResult)]
pub struct PaymentOrderStats {
    pub total_orders: i64,
    pub total_amount: Decimal,
    pub paid_orders: i64,
    pub paid_amount: Decimal,
    pub pending_orders: i64,
    pub pending_amount: Decimal,
}

impl PaymentOrder {
    /// 获取用户订单统计
    pub async fn get_user_stats(
        db: &impl ConnectionTrait,
        user_id: Uuid,
    ) -> Result<PaymentOrderStats, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT
                COUNT(*) as total_orders,
                COALESCE(SUM(amount), 0) as total_amount,
                COUNT(*) FILTER (WHERE status = 'paid') as paid_orders,
                COALESCE(SUM(amount) FILTER (WHERE status = 'paid'), 0) as paid_amount,
                COUNT(*) FILTER (WHERE status = 'pending') as pending_orders,
                COALESCE(SUM(amount) FILTER (WHERE status = 'pending'), 0) as pending_amount
            FROM payment_orders
            WHERE user_id = $1
            "#,
            [user_id.into()],
        );
        let stats = PaymentOrderStats::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("stats query failed".to_string()))?;
        Ok(stats)
    }
}
