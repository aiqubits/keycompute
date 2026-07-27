//! 支付服务核心逻辑
//
//! 整合支付宝客户端和数据库操作，提供完整的支付流程

use crate::client::{AlipayClient, QueryResponse};
use crate::config::AlipayConfig;
use chrono::{DateTime, Utc};
use keycompute_db::DbRouter;
use rust_decimal::Decimal;
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

mod urlencoding {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

    pub fn encode(s: &str) -> String {
        utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
    }
}

/// 支付服务
pub struct PaymentService {
    client: AlipayClient,
    pool: Arc<DbRouter>,
}

impl PaymentService {
    /// 创建新的支付服务
    pub fn new(config: AlipayConfig, pool: Arc<DbRouter>) -> Result<Self, PaymentError> {
        let client = AlipayClient::new(config)?;
        Ok(Self { client, pool })
    }

    /// 从环境变量创建支付服务
    pub async fn from_env(pool: Arc<DbRouter>) -> Result<Self, PaymentError> {
        let config = AlipayConfig::from_env()?;
        Self::new(config, pool)
    }

    /// 创建支付订单
    ///
    /// 返回支付URL，用于前端跳转到支付宝支付页面
    pub async fn create_order(
        &self,
        req: CreateOrderRequest,
    ) -> Result<CreateOrderResult, PaymentError> {
        // 生成商户订单号
        let out_trade_no = generate_out_trade_no();
        let expired_at =
            Utc::now() + chrono::Duration::minutes(self.client.config().timeout_minutes as i64);

        // 格式化金额
        let amount_str = format!("{:.2}", req.amount);

        // 生成支付URL
        let pay_url = self.client.page_pay_url(
            &out_trade_no,
            &amount_str,
            &req.subject,
            req.body.as_deref(),
            expired_at,
        )?;

        // 创建数据库订单记录
        let db_req = keycompute_db::CreatePaymentOrderRequest {
            tenant_id: req.tenant_id,
            user_id: req.user_id,
            amount: req.amount,
            subject: req.subject.clone(),
            body: req.body.clone(),
            payment_method: keycompute_db::PaymentMethod::Alipay,
            payment_scene: "page".to_string(),
            expired_at,
        };

        let order = keycompute_db::PaymentOrder::create(
            self.pool.as_ref(),
            &db_req,
            &out_trade_no,
            &pay_url,
        )
        .await
        .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

        Ok(CreateOrderResult {
            order_id: order.id,
            out_trade_no: order.out_trade_no,
            pay_url: order.pay_url.unwrap_or_default(),
            expired_at: order.expired_at,
        })
    }

    /// 创建手机网站支付订单
    pub async fn create_wap_order(
        &self,
        req: CreateOrderRequest,
    ) -> Result<CreateOrderResult, PaymentError> {
        let out_trade_no = generate_out_trade_no();
        let expired_at =
            Utc::now() + chrono::Duration::minutes(self.client.config().timeout_minutes as i64);
        let amount_str = format!("{:.2}", req.amount);

        let pay_url = self.client.wap_pay_url(
            &out_trade_no,
            &amount_str,
            &req.subject,
            req.body.as_deref(),
            expired_at,
        )?;

        let db_req = keycompute_db::CreatePaymentOrderRequest {
            tenant_id: req.tenant_id,
            user_id: req.user_id,
            amount: req.amount,
            subject: req.subject.clone(),
            body: req.body.clone(),
            payment_method: keycompute_db::PaymentMethod::Alipay,
            payment_scene: "wap".to_string(),
            expired_at,
        };

        let order = keycompute_db::PaymentOrder::create(
            self.pool.as_ref(),
            &db_req,
            &out_trade_no,
            &pay_url,
        )
        .await
        .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

        Ok(CreateOrderResult {
            order_id: order.id,
            out_trade_no: order.out_trade_no,
            pay_url: order.pay_url.unwrap_or_default(),
            expired_at: order.expired_at,
        })
    }

    /// 创建扫码支付订单（当面付）
    ///
    /// 生成支付二维码，用户使用支付宝扫码完成支付
    /// 返回二维码内容和订单信息
    ///
    /// # 执行流程
    /// 1. 先创建数据库订单记录（状态为 pending）
    /// 2. 再调用支付宝 precreate API 获取二维码
    /// 3. 更新数据库订单的 pay_url 字段
    ///
    /// 这样可以避免：支付宝 precreate 成功但数据库失败导致的不一致
    pub async fn create_qr_order(
        &self,
        req: CreateOrderRequest,
    ) -> Result<CreateQrOrderResult, PaymentError> {
        // 生成商户订单号
        let out_trade_no = generate_out_trade_no();
        let expired_at =
            Utc::now() + chrono::Duration::minutes(self.client.config().timeout_minutes as i64);

        // 格式化金额
        let amount_str = format!("{:.2}", req.amount);

        // 先创建数据库订单记录（状态为 pending）
        let db_req = keycompute_db::CreatePaymentOrderRequest {
            tenant_id: req.tenant_id,
            user_id: req.user_id,
            amount: req.amount,
            subject: req.subject.clone(),
            body: req.body.clone(),
            payment_method: keycompute_db::PaymentMethod::Alipay,
            payment_scene: "qr".to_string(),
            expired_at,
        };

        // 临时使用空字符串作为 pay_url，后续更新
        let order =
            keycompute_db::PaymentOrder::create(self.pool.as_ref(), &db_req, &out_trade_no, "")
                .await
                .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

        // 调用支付宝 precreate 接口
        let precreate_result = self
            .client
            .precreate(
                &out_trade_no,
                &amount_str,
                &req.subject,
                req.body.as_deref(),
                self.client.config().timeout_minutes,
            )
            .await;

        let precreate_response = match precreate_result {
            Ok(r) => r,
            Err(e) => {
                // precreate 调用失败，标记订单为失败状态
                if let Err(mark_err) =
                    keycompute_db::PaymentOrder::mark_as_failed(self.pool.as_ref(), order.id).await
                {
                    tracing::error!("Failed to mark order {} as failed: {}", order.id, mark_err);
                }
                return Err(e.into());
            }
        };

        if !precreate_response.is_success() {
            // precreate 返回失败，标记订单为失败状态
            if let Err(mark_err) =
                keycompute_db::PaymentOrder::mark_as_failed(self.pool.as_ref(), order.id).await
            {
                tracing::error!("Failed to mark order {} as failed: {}", order.id, mark_err);
            }
            return Err(PaymentError::ProviderRejected {
                code: precreate_response.sub_code,
                message: precreate_response.sub_msg.unwrap_or(precreate_response.msg),
            });
        }

        let Some(qr_code) = precreate_response.qr_code.clone() else {
            if let Err(mark_err) =
                keycompute_db::PaymentOrder::mark_as_failed(self.pool.as_ref(), order.id).await
            {
                tracing::error!("Failed to mark order {} as failed: {}", order.id, mark_err);
            }
            return Err(PaymentError::ProviderVerification(
                "successful precreate response has no qr_code".to_string(),
            ));
        };

        // 更新数据库订单的 pay_url 字段
        let stmt = sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r#"UPDATE payment_orders SET pay_url = $1 WHERE id = $2"#,
            [qr_code.as_str().into(), order.id.into()],
        );
        if let Err(database_error) = self.pool.execute(stmt).await {
            // The provider order already exists but the client cannot safely receive
            // its QR code. Close it remotely before transitioning the local order.
            match self.client.close_order(&out_trade_no).await {
                Ok(response) if response.is_success() => {
                    if let Err(close_error) =
                        keycompute_db::PaymentOrder::close(self.pool.as_ref(), order.id).await
                    {
                        tracing::error!(%close_error, order_id = %order.id, "Failed to close compensated Alipay order locally");
                    }
                }
                Ok(response) => {
                    tracing::error!(
                        order_id = %order.id,
                        sub_code = ?response.sub_code,
                        "Alipay rejected compensating order close"
                    );
                }
                Err(close_error) => {
                    tracing::error!(%close_error, order_id = %order.id, "Failed to compensate Alipay order after QR persistence failure");
                }
            }
            return Err(PaymentError::DatabaseError(database_error.to_string()));
        }

        Ok(CreateQrOrderResult {
            order_id: order.id,
            out_trade_no: order.out_trade_no,
            qr_code,
            expired_at: order.expired_at,
        })
    }

    /// 查询订单状态
    pub async fn query_order(&self, out_trade_no: &str) -> Result<QueryResponse, PaymentError> {
        Ok(self.client.query_order(out_trade_no).await?)
    }

    /// 处理支付成功回调
    ///
    /// 验签成功后更新订单状态并充值用户余额
    pub async fn handle_notify(
        &self,
        params: HashMap<String, String>,
    ) -> Result<NotifyResult, PaymentError> {
        // 转换为参数列表
        let params_vec: Vec<(String, String)> = params.clone().into_iter().collect();

        // 验签
        if !self.client.verify_notify(&params_vec)? {
            return Err(PaymentError::InvalidSignature);
        }

        // 解析通知参数
        let out_trade_no = params
            .get("out_trade_no")
            .ok_or(PaymentError::MissingParam("out_trade_no"))?
            .clone();
        let trade_no = params
            .get("trade_no")
            .ok_or(PaymentError::MissingParam("trade_no"))?
            .clone();
        let notify_id = params
            .get("notify_id")
            .filter(|value| value.chars().count() <= 128)
            .ok_or(PaymentError::MissingParam("notify_id"))?
            .clone();
        let trade_status = params
            .get("trade_status")
            .ok_or(PaymentError::MissingParam("trade_status"))?
            .clone();
        let total_amount: Decimal = params
            .get("total_amount")
            .ok_or(PaymentError::MissingParam("total_amount"))?
            .parse()
            .map_err(|_| PaymentError::InvalidAmount)?;
        let notify_app_id = params
            .get("app_id")
            .ok_or(PaymentError::MissingParam("app_id"))?;
        if notify_app_id != &self.client.config().app_id {
            return Err(PaymentError::InvalidMerchant);
        }

        // 查询订单
        let order = keycompute_db::PaymentOrder::find_by_out_trade_no(
            self.pool.write_conn(),
            &out_trade_no,
        )
        .await
        .map_err(|e| PaymentError::DatabaseError(e.to_string()))?
        .ok_or(PaymentError::OrderNotFound)?;
        if order.payment_method != "alipay" {
            return Err(PaymentError::InvalidOrderMethod);
        }

        // 验证金额一致性（安全检查）
        if total_amount != order.amount {
            tracing::error!(
                "Amount mismatch: order={}, notify={}",
                order.amount,
                total_amount
            );
            return Err(PaymentError::AmountMismatch {
                expected: order.amount,
                actual: total_amount,
            });
        }
        let notify_data = serde_json::to_value(&params)
            .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

        // 重复通知仍需验证渠道交易号和状态组合，不能因本地订单已完成就
        // 确认一个与本地终态矛盾的通知。
        if order.status != "pending" {
            validate_terminal_notification(
                &order.status,
                order
                    .provider_trade_no
                    .as_deref()
                    .or(order.trade_no.as_deref()),
                &trade_no,
                &trade_status,
            )?;
            if order.status == "paid" {
                let description = format!("支付宝充值 - 订单号: {}", out_trade_no);
                keycompute_db::PaymentOrder::credit_paid(
                    self.pool.as_ref(),
                    order.id,
                    &trade_no,
                    &notify_id,
                    notify_data,
                    &description,
                )
                .await
                .map_err(map_credit_paid_error)?;
            }
            return Ok(NotifyResult {
                order_id: order.id,
                status: order.status.clone(),
                amount: order.amount,
                trade_no: order.provider_trade_no.unwrap_or(trade_no),
            });
        }

        // 检查交易状态
        if trade_status == "TRADE_SUCCESS" || trade_status == "TRADE_FINISHED" {
            // 交易成功，继续处理
        } else if trade_status == "TRADE_CLOSED" {
            // 交易关闭，使用 close 方法设置 closed_at
            keycompute_db::PaymentOrder::close(self.pool.as_ref(), order.id)
                .await
                .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

            return Ok(NotifyResult {
                order_id: order.id,
                status: "closed".to_string(),
                amount: order.amount,
                trade_no,
            });
        } else {
            // 其他状态（如 WAIT_BUYER_PAY），不应该出现在回调中
            // 记录警告日志，但不修改订单状态，返回错误让支付宝重试
            tracing::warn!(
                "Unexpected trade_status '{}' in notify for order {}, ignoring",
                trade_status,
                order.id
            );
            return Err(PaymentError::InvalidTradeStatus(trade_status));
        }

        let description = format!("支付宝充值 - 订单号: {}", out_trade_no);
        keycompute_db::PaymentOrder::credit_paid(
            self.pool.as_ref(),
            order.id,
            &trade_no,
            &notify_id,
            notify_data,
            &description,
        )
        .await
        .map_err(map_credit_paid_error)?;

        Ok(NotifyResult {
            order_id: order.id,
            status: "paid".to_string(),
            amount: order.amount,
            trade_no,
        })
    }

    /// 主动同步订单状态
    ///
    /// 从支付宝查询订单状态并更新本地订单
    pub async fn sync_order_status(&self, out_trade_no: &str) -> Result<SyncResult, PaymentError> {
        // 查询本地订单
        let order =
            keycompute_db::PaymentOrder::find_by_out_trade_no(self.pool.write_conn(), out_trade_no)
                .await
                .map_err(|e| PaymentError::DatabaseError(e.to_string()))?
                .ok_or(PaymentError::OrderNotFound)?;
        if order.payment_method != "alipay" {
            return Err(PaymentError::InvalidOrderMethod);
        }

        // 如果订单已处理，直接返回
        if order.status != "pending" {
            return Ok(SyncResult {
                order_id: order.id,
                status: order.status.clone(),
                changed: false,
            });
        }

        // 从支付宝查询订单状态
        let query_result = self.client.query_order(out_trade_no).await?;

        if !query_result.is_success() {
            return Err(PaymentError::ProviderRejected {
                code: query_result.sub_code,
                message: query_result.sub_msg.unwrap_or(query_result.msg),
            });
        }
        if query_result.out_trade_no.as_deref() != Some(out_trade_no) {
            return Err(PaymentError::OrderIdentityMismatch);
        }

        // 检查交易状态
        let trade_status = query_result.trade_status.as_deref().unwrap_or("");

        if trade_status == "TRADE_SUCCESS" || trade_status == "TRADE_FINISHED" {
            let queried_amount: Decimal = query_result
                .total_amount
                .as_deref()
                .ok_or(PaymentError::InvalidAmount)?
                .parse()
                .map_err(|_| PaymentError::InvalidAmount)?;
            if queried_amount != order.amount {
                return Err(PaymentError::AmountMismatch {
                    expected: order.amount,
                    actual: queried_amount,
                });
            }
            // 交易成功，更新订单并充值
            let trade_no = query_result.trade_no.clone().ok_or_else(|| {
                PaymentError::ProviderVerification(
                    "successful query response has no trade_no".to_string(),
                )
            })?;
            let notify_data = serde_json::to_value(&query_result)
                .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

            let description = format!("支付宝充值(同步) - 订单号: {}", out_trade_no);
            let changed = keycompute_db::PaymentOrder::credit_paid(
                self.pool.as_ref(),
                order.id,
                &trade_no,
                &format!("sync:alipay:{trade_no}"),
                notify_data,
                &description,
            )
            .await
            .map_err(map_credit_paid_error)?;

            Ok(SyncResult {
                order_id: order.id,
                status: "paid".to_string(),
                changed,
            })
        } else if trade_status == "TRADE_CLOSED" {
            // 交易关闭，使用 close 方法设置 closed_at
            keycompute_db::PaymentOrder::close(self.pool.as_ref(), order.id)
                .await
                .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

            Ok(SyncResult {
                order_id: order.id,
                status: "closed".to_string(),
                changed: true,
            })
        } else {
            // 等待付款或其他状态
            Ok(SyncResult {
                order_id: order.id,
                status: "pending".to_string(),
                changed: false,
            })
        }
    }

    /// 关闭订单
    ///
    /// # 注意
    /// 此方法会先调用支付宝关闭订单，然后更新本地状态。
    /// 如果支付宝关闭成功但本地更新失败，本地状态可能不一致。
    pub async fn close_order(
        &self,
        order_id: Uuid,
        out_trade_no: &str,
    ) -> Result<(), PaymentError> {
        // 必须从 writer 校验刚创建的订单。使用普通 DbRouter 查询会被路由到
        // 只读副本，创建后立即验证时可能因复制延迟误报 OrderNotFound。
        let order = keycompute_db::PaymentOrder::find_by_id(self.pool.write_conn(), order_id)
            .await
            .map_err(|e| PaymentError::DatabaseError(e.to_string()))?
            .ok_or(PaymentError::OrderNotFound)?;
        if order.out_trade_no != out_trade_no {
            return Err(PaymentError::OrderIdentityMismatch);
        }
        if order.status != "pending" {
            return Err(PaymentError::InvalidOrderStatus);
        }

        // 调用支付宝关闭订单接口
        let result = self.client.close_order(out_trade_no).await?;

        if !result.is_success() {
            return Err(PaymentError::ProviderRejected {
                code: result.sub_code,
                message: result.sub_msg.unwrap_or(result.msg),
            });
        }

        // 直接按已校验的 ID 条件更新，保证远端与本地关闭的是同一订单。
        keycompute_db::PaymentOrder::close(self.pool.as_ref(), order_id)
            .await
            .map_err(|e| PaymentError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// 获取用户余额
    pub async fn get_user_balance(&self, user_id: Uuid) -> Result<UserBalanceInfo, PaymentError> {
        let balance = keycompute_db::UserBalance::find_by_user(self.pool.as_ref(), user_id)
            .await
            .map_err(|e| PaymentError::DatabaseError(e.to_string()))?
            .unwrap_or_else(|| keycompute_db::UserBalance {
                id: Uuid::nil(),
                tenant_id: Uuid::nil(),
                user_id,
                available_balance: Decimal::ZERO,
                frozen_balance: Decimal::ZERO,
                total_recharged: Decimal::ZERO,
                total_consumed: Decimal::ZERO,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });

        Ok(UserBalanceInfo {
            user_id: balance.user_id,
            available_balance: balance.available_balance,
            frozen_balance: balance.frozen_balance,
            total_balance: balance.total_balance(),
            total_recharged: balance.total_recharged,
            total_consumed: balance.total_consumed,
        })
    }

    /// 获取支付客户端（用于需要直接调用支付宝API的场景）
    pub fn client(&self) -> &AlipayClient {
        &self.client
    }
}

fn validate_terminal_notification(
    local_status: &str,
    recorded_trade_no: Option<&str>,
    incoming_trade_no: &str,
    trade_status: &str,
) -> Result<(), PaymentError> {
    match local_status {
        "paid" => {
            if !matches!(trade_status, "TRADE_SUCCESS" | "TRADE_FINISHED") {
                return Err(PaymentError::InvalidOrderStatus);
            }
            if recorded_trade_no != Some(incoming_trade_no) {
                return Err(PaymentError::OrderIdentityMismatch);
            }
            Ok(())
        }
        "closed" if trade_status == "TRADE_CLOSED" => Ok(()),
        _ => Err(PaymentError::InvalidOrderStatus),
    }
}

/// 生成商户订单号
///
/// 格式: KC + 14位时间戳 + 8位UUID随机后缀
/// 冲突概率极低，支持高并发场景
fn generate_out_trade_no() -> String {
    let timestamp = Utc::now().format("%Y%m%d%H%M%S");
    // 使用 UUID 前8位作为随机后缀，提供约 4.3 billion 种组合
    let uuid_suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    format!("KC{}{}", timestamp, uuid_suffix)
}

/// 创建订单请求
#[derive(Debug, Clone, Deserialize)]
pub struct CreateOrderRequest {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub amount: Decimal,
    pub subject: String,
    pub body: Option<String>,
}

/// 创建订单结果
#[derive(Debug, Clone, Serialize)]
pub struct CreateOrderResult {
    /// 订单ID
    pub order_id: Uuid,
    /// 商户订单号
    pub out_trade_no: String,
    /// 支付URL
    pub pay_url: String,
    /// 过期时间
    pub expired_at: DateTime<Utc>,
}

/// 回调处理结果
#[derive(Debug, Clone, Serialize)]
pub struct NotifyResult {
    pub order_id: Uuid,
    pub status: String,
    pub amount: Decimal,
    pub trade_no: String,
}

/// 同步订单结果
#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub order_id: Uuid,
    pub status: String,
    pub changed: bool,
}

/// 用户余额信息
#[derive(Debug, Clone, Serialize)]
pub struct UserBalanceInfo {
    pub user_id: Uuid,
    pub available_balance: Decimal,
    pub frozen_balance: Decimal,
    pub total_balance: Decimal,
    pub total_recharged: Decimal,
    pub total_consumed: Decimal,
}

/// 创建扫码支付订单结果
#[derive(Debug, Clone, Serialize)]
pub struct CreateQrOrderResult {
    /// 订单ID
    pub order_id: Uuid,
    /// 商户订单号
    pub out_trade_no: String,
    /// 支付二维码内容（可用于生成二维码图片）
    pub qr_code: String,
    /// 过期时间
    pub expired_at: DateTime<Utc>,
}

impl CreateQrOrderResult {
    /// 获取二维码图片URL（使用第三方二维码生成服务）
    ///
    /// 示例：返回一个可以展示二维码图片的URL
    pub fn qr_code_image_url(&self) -> String {
        // 使用公共二维码生成API
        format!(
            "https://api.qrserver.com/v1/create-qr-code/?size=300x300&data={}",
            urlencoding::encode(&self.qr_code)
        )
    }
}

/// 支付错误
#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("配置错误: {0}")]
    ConfigError(String),
    #[error("API错误: {0}")]
    ApiError(String),
    #[error("支付宝 API 拒绝 ({code:?}): {message}")]
    ProviderRejected {
        code: Option<String>,
        message: String,
    },
    #[error("支付宝身份或响应签名验证失败: {0}")]
    ProviderVerification(String),
    #[error("数据库错误: {0}")]
    DatabaseError(String),
    #[error("签名验证失败")]
    InvalidSignature,
    #[error("回调商户身份不匹配")]
    InvalidMerchant,
    #[error("缺少参数: {0}")]
    MissingParam(&'static str),
    #[error("订单不存在")]
    OrderNotFound,
    #[error("金额无效")]
    InvalidAmount,
    #[error("订单状态无效")]
    InvalidOrderStatus,
    #[error("支付渠道与本地订单不匹配")]
    InvalidOrderMethod,
    #[error("渠道响应中的商户订单号与本地订单不匹配")]
    OrderIdentityMismatch,
    #[error("金额不匹配: 订单 {expected}, 回调 {actual}")]
    AmountMismatch { expected: Decimal, actual: Decimal },
    #[error("无效的交易状态: {0}")]
    InvalidTradeStatus(String),
    #[error("通知事件标识与已有记录冲突")]
    NotificationConflict,
}

fn map_credit_paid_error(error: keycompute_db::CreditPaidOrderError) -> PaymentError {
    match error {
        keycompute_db::CreditPaidOrderError::Database(error) => {
            PaymentError::DatabaseError(error.to_string())
        }
        keycompute_db::CreditPaidOrderError::ProviderIdentityMismatch => {
            PaymentError::OrderIdentityMismatch
        }
        keycompute_db::CreditPaidOrderError::NotificationConflict => {
            PaymentError::NotificationConflict
        }
        keycompute_db::CreditPaidOrderError::InvalidOrderStatus(_)
        | keycompute_db::CreditPaidOrderError::ConcurrentTransition => {
            PaymentError::InvalidOrderStatus
        }
        keycompute_db::CreditPaidOrderError::OrderNotFound => PaymentError::OrderNotFound,
    }
}

impl PaymentError {
    /// 判断错误是否可重试
    ///
    /// 可重试错误：数据库临时故障、网络超时等，支付宝应该重试通知
    /// 不可重试错误：签名错误、订单不存在、金额不匹配等，支付宝不应重试
    pub fn is_retryable(&self) -> bool {
        match self {
            // 数据库错误通常是可重试的（连接池耗尽、临时网络问题等）
            PaymentError::DatabaseError(_) => true,
            // 以下错误不可重试，重试也无法解决
            PaymentError::ConfigError(_) => false,
            PaymentError::ApiError(_) => false,
            PaymentError::ProviderRejected { .. } => false,
            PaymentError::ProviderVerification(_) => false,
            PaymentError::InvalidSignature => false,
            PaymentError::InvalidMerchant => false,
            PaymentError::MissingParam(_) => false,
            PaymentError::OrderNotFound => false,
            PaymentError::InvalidAmount => false,
            PaymentError::InvalidOrderStatus => false,
            PaymentError::InvalidOrderMethod => false,
            PaymentError::OrderIdentityMismatch => false,
            PaymentError::AmountMismatch { .. } => false,
            PaymentError::InvalidTradeStatus(_) => false,
            PaymentError::NotificationConflict => false,
        }
    }
}

impl From<crate::config::ConfigError> for PaymentError {
    fn from(e: crate::config::ConfigError) -> Self {
        PaymentError::ConfigError(e.to_string())
    }
}

impl From<crate::client::ClientError> for PaymentError {
    fn from(e: crate::client::ClientError) -> Self {
        use crate::client::ClientError;

        match e {
            error @ (ClientError::ConfigError(_)
            | ClientError::SignError(_)
            | ClientError::MissingSign
            | ClientError::InvalidResponseSignature) => {
                PaymentError::ProviderVerification(error.to_string())
            }
            error => PaymentError::ApiError(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_out_trade_no() {
        let order_no = generate_out_trade_no();
        assert!(order_no.starts_with("KC"));
        assert_eq!(order_no.len(), 24); // KC + 14位时间戳 + 8位UUID后缀

        // 验证生成的订单号唯一性
        let order_no2 = generate_out_trade_no();
        assert_ne!(order_no, order_no2);

        // 验证UUID后缀只包含十六进制字符
        let uuid_suffix = &order_no[16..]; // KC(2) + 时间戳(14) = 16
        assert!(
            uuid_suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "UUID后缀应只包含十六进制字符: {}",
            uuid_suffix
        );
    }

    #[test]
    fn response_verification_errors_remain_distinguishable_from_network_errors() {
        let verification = PaymentError::from(crate::client::ClientError::InvalidResponseSignature);
        let networkish = PaymentError::from(crate::client::ClientError::ParseError(
            "invalid JSON".to_string(),
        ));

        assert!(matches!(
            verification,
            PaymentError::ProviderVerification(_)
        ));
        assert!(matches!(networkish, PaymentError::ApiError(_)));
    }

    #[test]
    fn duplicate_paid_notification_requires_matching_successful_trade() {
        assert!(
            validate_terminal_notification(
                "paid",
                Some("provider-trade-1"),
                "provider-trade-1",
                "TRADE_SUCCESS",
            )
            .is_ok()
        );
        assert!(matches!(
            validate_terminal_notification(
                "paid",
                Some("provider-trade-1"),
                "provider-trade-2",
                "TRADE_SUCCESS",
            ),
            Err(PaymentError::OrderIdentityMismatch)
        ));
        assert!(matches!(
            validate_terminal_notification(
                "paid",
                Some("provider-trade-1"),
                "provider-trade-1",
                "TRADE_CLOSED",
            ),
            Err(PaymentError::InvalidOrderStatus)
        ));
    }

    #[test]
    fn terminal_notification_cannot_reopen_closed_or_failed_order() {
        assert!(validate_terminal_notification("closed", None, "trade-1", "TRADE_CLOSED").is_ok());
        for local_status in ["closed", "failed"] {
            assert!(matches!(
                validate_terminal_notification(local_status, None, "trade-1", "TRADE_SUCCESS",),
                Err(PaymentError::InvalidOrderStatus)
            ));
        }
    }

    #[test]
    fn notify_amount_comparison_uses_decimal_value_semantics() {
        // handle_notify 以 `total_amount != order.amount` 严格比对金额；
        // 本测试固化 Decimal 的数值相等语义，防止后续改动退回字符串比较
        // 或引入容差比较。
        let order_amount: Decimal = "12.30".parse().unwrap();

        // 支付宝可能回传省略末尾零的 "12.3"，数值相等必须通过
        let notify_trimmed: Decimal = "12.3".parse().unwrap();
        assert_eq!(notify_trimmed, order_amount);

        // 多一位小数的 "12.301" 必须判为不等（金额篡改）
        let notify_tampered: Decimal = "12.301".parse().unwrap();
        assert_ne!(notify_tampered, order_amount);

        // 差一分钱也必须判为不等，不存在 ±0.01 容差
        let notify_off_by_cent: Decimal = "12.31".parse().unwrap();
        assert_ne!(notify_off_by_cent, order_amount);
    }
}
