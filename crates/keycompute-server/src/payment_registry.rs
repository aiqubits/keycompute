//! Runtime registry for payment providers.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use keycompute_db::{DbRouter, PaymentMethod, PaymentOrder};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

// Terminal credential/configuration failures are fail-closed. Routine requests may move between
// available and degraded, but cannot clear unavailable: only mark_verified or a config fingerprint
// change is allowed to do that. This also prevents an older in-flight success from reopening the
// provider after a newer terminal failure.
const RECORD_PROVIDER_RESULT_SQL: &str = r#"UPDATE payment_provider_states
SET circuit_state=$1, last_error_code=$2, last_error_message=$3, updated_at=NOW()
WHERE payment_method=$4
  AND (circuit_state <> 'unavailable' OR $1 = 'unavailable')"#;

/// 管理员渠道验证订单的固定 subject，用于对账时识别 0.01 元验证订单。
pub const PROVIDER_VERIFICATION_SUBJECT: &str = "支付渠道验证";

#[derive(Debug, Clone, Serialize)]
pub struct AvailablePaymentMethod {
    pub code: &'static str,
    pub display_name: &'static str,
    pub scenes: &'static [&'static str],
    pub recommended_scene: &'static str,
    pub sort_order: u16,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct PaymentCreateRequest {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub method: PaymentMethod,
    pub scene: String,
    pub amount: Decimal,
    pub subject: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaymentCreateResult {
    pub order_id: Uuid,
    pub out_trade_no: String,
    pub payment_method: String,
    pub payment_scene: String,
    pub pay_url: Option<String>,
    pub qr_code: Option<String>,
    pub expired_at: DateTime<Utc>,
}

pub struct PaymentRegistry {
    alipay: Option<Arc<keycompute_alipay::PaymentService>>,
    wechatpay: Option<Arc<keycompute_wechatpay::WechatPayClient>>,
    pool: Arc<DbRouter>,
}

#[derive(Debug, Clone)]
pub struct ProviderRuntimeStatus {
    pub configured: bool,
    pub verified: bool,
    pub circuit_state: String,
    pub state_error: bool,
}

impl ProviderRuntimeStatus {
    pub fn accepts_new_orders(&self) -> bool {
        self.configured
            && self.verified
            && matches!(self.circuit_state.as_str(), "available" | "degraded")
    }

    pub fn has_valid_circuit_state(&self) -> bool {
        matches!(
            self.circuit_state.as_str(),
            "available" | "degraded" | "unavailable"
        )
    }
}

impl std::fmt::Debug for PaymentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaymentRegistry")
            .field("alipay", &self.alipay.is_some())
            .field("wechatpay", &self.wechatpay.is_some())
            .finish()
    }
}

impl PaymentRegistry {
    pub fn from_env(pool: Arc<DbRouter>) -> Self {
        let alipay = keycompute_alipay::AlipayConfig::from_env()
            .and_then(|config| {
                keycompute_alipay::PaymentService::new(config, Arc::clone(&pool)).map_err(|_| {
                    keycompute_alipay::ConfigError::InvalidConfig("client init failed")
                })
            })
            .map(Arc::new)
            .map_err(|error| tracing::info!(%error, "Alipay provider unavailable"))
            .ok();

        let wechatpay = keycompute_wechatpay::WechatPayConfig::from_env()
            .and_then(|config| {
                keycompute_wechatpay::WechatPayClient::new(config).map_err(|error| {
                    keycompute_wechatpay::WechatPayConfigError::Invalid(error.to_string())
                })
            })
            .map(Arc::new)
            .map_err(|error| tracing::info!(%error, "WeChat Pay provider unavailable"))
            .ok();

        Self {
            alipay,
            wechatpay,
            pool,
        }
    }

    pub fn alipay(&self) -> Option<&Arc<keycompute_alipay::PaymentService>> {
        self.alipay.as_ref()
    }

    pub fn wechatpay(&self) -> Option<&Arc<keycompute_wechatpay::WechatPayClient>> {
        self.wechatpay.as_ref()
    }

    pub async fn provider_status(&self, method: PaymentMethod) -> ProviderRuntimeStatus {
        let configured = match method {
            PaymentMethod::Alipay => self.alipay.is_some(),
            PaymentMethod::WechatPay => self.wechatpay.is_some(),
        };
        let (state, state_error) = if configured {
            match self.ensure_config_state(method).await {
                Ok(state) => (Some(state), false),
                Err(error) => {
                    tracing::error!(%error, payment_method = method.as_str(), "Failed to load payment provider runtime state");
                    (None, true)
                }
            }
        } else {
            (None, false)
        };
        ProviderRuntimeStatus {
            configured,
            verified: state.as_ref().is_some_and(|state| state.verified),
            circuit_state: state
                .map(|state| state.circuit_state)
                .unwrap_or_else(|| "unavailable".to_string()),
            state_error,
        }
    }

    pub async fn available_methods(&self) -> Result<Vec<AvailablePaymentMethod>, RegistryError> {
        use keycompute_db::models::system_setting::setting_keys;

        // This is the final order-creation gate. Read it from the writer so an
        // emergency provider disable takes effect immediately despite replica lag.
        #[derive(FromQueryResult)]
        struct SwitchRow {
            key: String,
            value: String,
        }
        let switches = SwitchRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT key, value FROM system_settings WHERE key IN ($1, $2)",
            [
                setting_keys::ALIPAY_ENABLED.into(),
                setting_keys::WECHATPAY_ENABLED.into(),
            ],
        ))
        .all(self.pool.write_conn())
        .await?;
        let enabled = |key: &str| {
            switches
                .iter()
                .find(|row| row.key == key)
                .is_some_and(|row| row.value.eq_ignore_ascii_case("true") || row.value == "1")
        };
        let alipay_enabled = enabled(setting_keys::ALIPAY_ENABLED);
        let wechat_enabled = enabled(setting_keys::WECHATPAY_ENABLED);

        let mut methods = Vec::with_capacity(2);
        let alipay_status = self.provider_status(PaymentMethod::Alipay).await;
        let wechat_status = self.provider_status(PaymentMethod::WechatPay).await;
        if alipay_enabled && alipay_status.accepts_new_orders() {
            methods.push(AvailablePaymentMethod {
                code: "alipay",
                display_name: "支付宝",
                scenes: &["page", "wap", "qr"],
                recommended_scene: "page",
                sort_order: 10,
                is_default: true,
            });
        }
        if wechat_enabled && wechat_status.accepts_new_orders() {
            methods.push(AvailablePaymentMethod {
                code: "wechatpay",
                display_name: "微信支付",
                scenes: &["native"],
                recommended_scene: "native",
                sort_order: 20,
                is_default: methods.is_empty(),
            });
        }
        Ok(methods)
    }

    /// Create and close a real provider order, then bind verification to the current config.
    ///
    /// 补偿窗口说明：若远端 close_order 失败，该 0.01 元订单会保持 pending
    /// 直至 timeout_minutes 过期；期间若被实际支付，将经回调向发起验证的
    /// 管理员账户入账 0.01 元。此类订单可通过 [`PROVIDER_VERIFICATION_SUBJECT`]
    /// 在对账时识别。
    pub async fn verify_provider(
        &self,
        method: PaymentMethod,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), RegistryError> {
        let result = self.verify_provider_inner(method, tenant_id, user_id).await;
        self.record_provider_result(method, &result).await;
        result
    }

    async fn verify_provider_inner(
        &self,
        method: PaymentMethod,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), RegistryError> {
        let request = PaymentCreateRequest {
            tenant_id,
            user_id,
            method,
            scene: match method {
                PaymentMethod::Alipay => "qr",
                PaymentMethod::WechatPay => "native",
            }
            .to_string(),
            amount: Decimal::new(1, 2),
            subject: PROVIDER_VERIFICATION_SUBJECT.to_string(),
            body: Some("管理员发起的支付能力验证订单".to_string()),
        };
        let created = match method {
            PaymentMethod::Alipay => self.create_alipay_order(request).await?,
            PaymentMethod::WechatPay => self.create_wechat_order(request).await?,
        };
        match method {
            PaymentMethod::Alipay => {
                self.alipay
                    .as_ref()
                    .ok_or_else(|| RegistryError::Unavailable("alipay".to_string()))?
                    .close_order(created.order_id, &created.out_trade_no)
                    .await?;
            }
            PaymentMethod::WechatPay => {
                self.wechatpay
                    .as_ref()
                    .ok_or_else(|| RegistryError::Unavailable("wechatpay".to_string()))?
                    .close_order(&created.out_trade_no)
                    .await?;
                keycompute_db::PaymentOrder::close(self.pool.as_ref(), created.order_id).await?;
            }
        }
        self.mark_verified(method).await
    }

    async fn ensure_config_state(
        &self,
        method: PaymentMethod,
    ) -> Result<ProviderStateSnapshot, RegistryError> {
        #[derive(FromQueryResult)]
        struct VerificationRow {
            verified_config_fingerprint: Option<String>,
            circuit_state: String,
            config_fingerprint: String,
        }
        let fingerprint = self
            .config_fingerprint(method)
            .ok_or_else(|| RegistryError::Unavailable(method.as_str().to_string()))?;
        let existing = VerificationRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT verified_config_fingerprint, circuit_state, config_fingerprint FROM payment_provider_states WHERE payment_method=$1",
            [method.as_str().into()],
        ))
        .one(self.pool.write_conn())
        .await?;
        if let Some(row) = existing
            && row.config_fingerprint == fingerprint
        {
            return Ok(ProviderStateSnapshot {
                verified: row.verified_config_fingerprint.as_deref() == Some(fingerprint.as_str()),
                circuit_state: row.circuit_state,
            });
        }
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO payment_provider_states(payment_method, config_fingerprint)
               VALUES ($1, $2)
               ON CONFLICT(payment_method) DO UPDATE SET
                 config_version = payment_provider_states.config_version + 1,
                 verified_config_fingerprint = NULL,
                 verified_at = NULL,
                 circuit_state = 'available',
                 last_error_code = NULL,
                 last_error_message = NULL,
                 config_fingerprint = EXCLUDED.config_fingerprint,
                 updated_at = NOW()
               RETURNING verified_config_fingerprint, circuit_state, config_fingerprint"#,
            [method.as_str().into(), fingerprint.as_str().into()],
        );
        let row = VerificationRow::find_by_statement(statement)
            .one(self.pool.write_conn())
            .await?
            .ok_or_else(|| RegistryError::Provider("provider state update failed".to_string()))?;
        Ok(ProviderStateSnapshot {
            verified: row.verified_config_fingerprint.as_deref() == Some(fingerprint.as_str()),
            circuit_state: row.circuit_state,
        })
    }

    async fn mark_verified(&self, method: PaymentMethod) -> Result<(), RegistryError> {
        let fingerprint = self
            .config_fingerprint(method)
            .ok_or_else(|| RegistryError::Unavailable(method.as_str().to_string()))?;
        self.pool
            .write_conn()
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE payment_provider_states
                   SET config_fingerprint=$1, verified_config_fingerprint=$1, verified_at=NOW(), circuit_state='available', last_error_code=NULL, last_error_message=NULL, updated_at=NOW()
                   WHERE payment_method=$2"#,
                [fingerprint.into(), method.as_str().into()],
            ))
            .await?;
        Ok(())
    }

    fn config_fingerprint(&self, method: PaymentMethod) -> Option<String> {
        let material = match method {
            PaymentMethod::Alipay => {
                let config = self.alipay.as_ref()?.client().config();
                alipay_config_fingerprint_material(config)
            }
            PaymentMethod::WechatPay => {
                let config = self.wechatpay.as_ref()?.config();
                wechat_config_fingerprint_material(config)
            }
        };
        Some(sha256_hex(material.as_bytes()))
    }

    pub async fn create_order(
        &self,
        request: PaymentCreateRequest,
    ) -> Result<PaymentCreateResult, RegistryError> {
        let allowed = self.available_methods().await?.iter().any(|item| {
            item.code == request.method.as_str() && item.scenes.contains(&request.scene.as_str())
        });
        if !allowed {
            return Err(RegistryError::Unavailable(
                request.method.as_str().to_string(),
            ));
        }

        let method = request.method;
        let scene = request.scene.clone();
        let result = match method {
            PaymentMethod::Alipay => self.create_alipay_order(request).await,
            PaymentMethod::WechatPay => self.create_wechat_order(request).await,
        };
        // Page/WAP 只在本地生成签名 URL，成功不能证明支付宝远端已恢复。
        // 本地签名失败仍应记录；QR 和微信创建都会真实访问渠道。
        if result.is_err() || creation_observes_provider(method, &scene) {
            self.record_provider_result(method, &result).await;
        }
        result
    }

    async fn create_alipay_order(
        &self,
        request: PaymentCreateRequest,
    ) -> Result<PaymentCreateResult, RegistryError> {
        let service = self
            .alipay
            .as_ref()
            .ok_or_else(|| RegistryError::Unavailable("alipay".to_string()))?;
        let provider_request = keycompute_alipay::CreateOrderRequest {
            tenant_id: request.tenant_id,
            user_id: request.user_id,
            amount: request.amount,
            subject: request.subject,
            body: request.body,
        };
        match request.scene.as_str() {
            "page" => {
                let result = service.create_order(provider_request).await?;
                Ok(PaymentCreateResult {
                    order_id: result.order_id,
                    out_trade_no: result.out_trade_no,
                    payment_method: "alipay".to_string(),
                    payment_scene: "page".to_string(),
                    pay_url: Some(result.pay_url),
                    qr_code: None,
                    expired_at: result.expired_at,
                })
            }
            "wap" => {
                let result = service.create_wap_order(provider_request).await?;
                Ok(PaymentCreateResult {
                    order_id: result.order_id,
                    out_trade_no: result.out_trade_no,
                    payment_method: "alipay".to_string(),
                    payment_scene: "wap".to_string(),
                    pay_url: Some(result.pay_url),
                    qr_code: None,
                    expired_at: result.expired_at,
                })
            }
            "qr" => {
                let result = service.create_qr_order(provider_request).await?;
                Ok(PaymentCreateResult {
                    order_id: result.order_id,
                    out_trade_no: result.out_trade_no,
                    payment_method: "alipay".to_string(),
                    payment_scene: "qr".to_string(),
                    pay_url: None,
                    qr_code: Some(result.qr_code),
                    expired_at: result.expired_at,
                })
            }
            scene => Err(RegistryError::UnsupportedScene(scene.to_string())),
        }
    }

    async fn create_wechat_order(
        &self,
        request: PaymentCreateRequest,
    ) -> Result<PaymentCreateResult, RegistryError> {
        if request.scene != "native" {
            return Err(RegistryError::UnsupportedScene(request.scene));
        }
        let client = self
            .wechatpay
            .as_ref()
            .ok_or_else(|| RegistryError::Unavailable("wechatpay".to_string()))?;
        let cents = request.amount * Decimal::new(100, 0);
        if !cents.fract().is_zero() {
            return Err(RegistryError::InvalidAmount);
        }
        let cents = cents.to_i64().ok_or(RegistryError::InvalidAmount)?;
        let out_trade_no = generate_out_trade_no();
        let expires_at =
            Utc::now() + chrono::Duration::minutes(client.config().timeout_minutes as i64);
        let db_request = keycompute_db::CreatePaymentOrderRequest {
            tenant_id: request.tenant_id,
            user_id: request.user_id,
            amount: request.amount,
            subject: request.subject.clone(),
            body: request.body,
            payment_method: PaymentMethod::WechatPay,
            payment_scene: "native".to_string(),
            expired_at: expires_at,
        };
        let order =
            PaymentOrder::create(self.pool.as_ref(), &db_request, &out_trade_no, "").await?;
        let provider_request = keycompute_wechatpay::NativeOrderRequest::new(
            request.subject,
            out_trade_no.clone(),
            expires_at,
            cents,
        );
        match client.create_native_order(provider_request).await {
            Ok(result) => {
                let payload = serde_json::json!({"type": "qr_code", "content": result.code_url});
                let stmt = Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE payment_orders SET pay_url = $1, provider_payload = $2, updated_at = NOW() WHERE id = $3",
                    [
                        result.code_url.as_str().into(),
                        payload.into(),
                        order.id.into(),
                    ],
                );
                if let Err(database_error) = self.pool.execute(stmt).await {
                    // The remote order is payable but its QR code could not be
                    // persisted for the client. Close it before changing local state.
                    match client.close_order(&out_trade_no).await {
                        Ok(()) => {
                            if let Err(close_error) =
                                keycompute_db::PaymentOrder::close(self.pool.as_ref(), order.id)
                                    .await
                            {
                                tracing::error!(%close_error, order_id = %order.id, "Failed to close compensated WeChat order locally");
                            }
                        }
                        Err(close_error) => {
                            tracing::error!(%close_error, order_id = %order.id, "Failed to compensate WeChat order after QR persistence failure");
                        }
                    }
                    return Err(database_error.into());
                }
                Ok(PaymentCreateResult {
                    order_id: order.id,
                    out_trade_no,
                    payment_method: "wechatpay".to_string(),
                    payment_scene: "native".to_string(),
                    pay_url: None,
                    qr_code: Some(result.code_url),
                    expired_at: order.expired_at,
                })
            }
            Err(error) => {
                // 保留渠道原始错误用于熔断判断。若清理本地订单失败，记录日志但
                // 不得用数据库错误覆盖认证/签名错误，否则渠道可能继续接收新订单。
                if let Err(database_error) =
                    keycompute_db::PaymentOrder::mark_as_failed(self.pool.as_ref(), order.id).await
                {
                    tracing::error!(%database_error, order_id = %order.id, "Failed to mark rejected WeChat order as failed");
                }
                Err(error.into())
            }
        }
    }

    pub async fn sync_order(&self, order: &PaymentOrder) -> Result<(String, bool), RegistryError> {
        let method = PaymentMethod::parse(&order.payment_method)
            .ok_or_else(|| RegistryError::Unavailable(order.payment_method.clone()))?;
        let result = match method {
            PaymentMethod::Alipay => match self.alipay.as_ref() {
                Some(service) => service
                    .sync_order_status(&order.out_trade_no)
                    .await
                    .map(|result| (result.status, result.changed))
                    .map_err(RegistryError::from),
                None => Err(RegistryError::Unavailable("alipay".to_string())),
            },
            PaymentMethod::WechatPay => self.sync_wechat_order(order).await,
        };
        // A non-pending order is resolved entirely from local state. It does not prove that the
        // remote provider has recovered, so it must not close an open provider circuit.
        if order.status == "pending" {
            self.record_provider_result(method, &result).await;
        }
        result
    }

    async fn record_provider_result<T>(
        &self,
        method: PaymentMethod,
        result: &Result<T, RegistryError>,
    ) {
        let (state, code, message) = match result {
            Ok(_) => ("available", None, None),
            Err(
                RegistryError::Database(_)
                | RegistryError::SeaOrm(_)
                | RegistryError::UnsupportedScene(_)
                | RegistryError::InvalidAmount
                | RegistryError::Unavailable(_)
                | RegistryError::AmountMismatch
                | RegistryError::ProviderIdentityMismatch
                | RegistryError::Alipay(
                    keycompute_alipay::PaymentError::OrderIdentityMismatch
                    | keycompute_alipay::PaymentError::AmountMismatch { .. },
                ),
            ) => return,
            Err(error) if is_terminal_provider_error(error) => (
                "unavailable",
                Some("provider_rejected"),
                Some(error.to_string()),
            ),
            Err(error) => ("degraded", Some("provider_error"), Some(error.to_string())),
        };
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            RECORD_PROVIDER_RESULT_SQL,
            [
                state.into(),
                code.map(str::to_owned).into(),
                message.into(),
                method.as_str().into(),
            ],
        );
        if let Err(error) = self.pool.write_conn().execute(statement).await {
            tracing::warn!(%error, payment_method = method.as_str(), "Failed to update payment provider health");
        }
    }

    async fn sync_wechat_order(
        &self,
        order: &PaymentOrder,
    ) -> Result<(String, bool), RegistryError> {
        use keycompute_wechatpay::TradeState;
        if order.status != "pending" {
            return Ok((order.status.clone(), false));
        }
        let client = self
            .wechatpay
            .as_ref()
            .ok_or_else(|| RegistryError::Unavailable("wechatpay".to_string()))?;
        let trade = client.query_order(&order.out_trade_no).await?;
        if trade.appid != client.config().appid
            || trade.mchid != client.config().mchid
            || trade.out_trade_no != order.out_trade_no
        {
            return Err(RegistryError::ProviderIdentityMismatch);
        }
        match trade.trade_state {
            TradeState::Success => {
                let transaction_id = trade.transaction_id.clone().ok_or_else(|| {
                    RegistryError::Provider(
                        "successful WeChat trade has no transaction_id".to_string(),
                    )
                })?;
                let amount = decimal_from_cents(trade.amount.total)?;
                if amount != order.amount || trade.amount.currency != order.currency {
                    return Err(RegistryError::AmountMismatch);
                }
                credit_paid_order(
                    self.pool.as_ref(),
                    order,
                    &transaction_id,
                    &format!("sync:wechatpay:{transaction_id}"),
                    serde_json::to_value(&trade).unwrap_or_default(),
                )
                .await?;
                Ok(("paid".to_string(), true))
            }
            TradeState::Closed | TradeState::Revoked => {
                keycompute_db::PaymentOrder::close(self.pool.as_ref(), order.id).await?;
                Ok(("closed".to_string(), true))
            }
            TradeState::Payerror => {
                keycompute_db::PaymentOrder::mark_as_failed(self.pool.as_ref(), order.id).await?;
                Ok(("failed".to_string(), true))
            }
            _ => Ok(("pending".to_string(), false)),
        }
    }
}

struct ProviderStateSnapshot {
    verified: bool,
    circuit_state: String,
}

fn is_terminal_provider_error(error: &RegistryError) -> bool {
    match error {
        RegistryError::WechatPay(keycompute_wechatpay::WechatPayError::Api { status, .. }) => {
            matches!(status, 401 | 403)
        }
        RegistryError::WechatPay(
            keycompute_wechatpay::WechatPayError::InvalidSignature(_)
            | keycompute_wechatpay::WechatPayError::Crypto(_),
        ) => true,
        RegistryError::Alipay(keycompute_alipay::PaymentError::ProviderVerification(_)) => true,
        RegistryError::Alipay(keycompute_alipay::PaymentError::ProviderRejected {
            code: Some(code),
            ..
        }) => is_terminal_alipay_code(code),
        _ => false,
    }
}

fn is_terminal_alipay_code(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    code.contains("invalid-signature")
        || code.contains("missing-signature")
        || code.contains("invalid-app-id")
        || code.contains("permission")
        || code.contains("unauthorized")
}

fn creation_observes_provider(method: PaymentMethod, scene: &str) -> bool {
    method == PaymentMethod::WechatPay || (method == PaymentMethod::Alipay && scene == "qr")
}

fn alipay_config_fingerprint_material(config: &keycompute_alipay::AlipayConfig) -> String {
    format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        config.app_id,
        config.private_key,
        config.alipay_public_key,
        config.gateway_url(),
        config.notify_url,
        config.return_url.as_deref().unwrap_or_default(),
        config.sign_type,
        config.charset,
        config.version,
        config.timeout_minutes
    )
}

fn wechat_config_fingerprint_material(config: &keycompute_wechatpay::WechatPayConfig) -> String {
    format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        config.appid,
        config.mchid,
        config.merchant_serial_no,
        config.merchant_private_key,
        config.api_v3_key,
        config.wechatpay_public_key_id,
        config.wechatpay_public_key,
        config.notify_url
    )
}

pub async fn credit_paid_order(
    pool: &DbRouter,
    order: &PaymentOrder,
    provider_trade_no: &str,
    event_id: &str,
    payload: serde_json::Value,
) -> Result<bool, RegistryError> {
    let description = format!(
        "{}充值 - 订单号: {}",
        order.payment_method, order.out_trade_no
    );
    PaymentOrder::credit_paid(
        pool,
        order.id,
        provider_trade_no,
        event_id,
        payload,
        &description,
    )
    .await
    .map_err(|error| match error {
        keycompute_db::CreditPaidOrderError::Database(error) => RegistryError::Database(error),
        keycompute_db::CreditPaidOrderError::ProviderIdentityMismatch => {
            RegistryError::ProviderIdentityMismatch
        }
        other => RegistryError::Provider(other.to_string()),
    })
}

pub fn decimal_from_cents(cents: i64) -> Result<Decimal, RegistryError> {
    if cents <= 0 {
        return Err(RegistryError::InvalidAmount);
    }
    Ok(Decimal::new(cents, 2))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn generate_out_trade_no() -> String {
    format!(
        "KC{}{}",
        Utc::now().format("%Y%m%d%H%M%S"),
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("payment method is unavailable: {0}")]
    Unavailable(String),
    #[error("unsupported payment scene: {0}")]
    UnsupportedScene(String),
    #[error("invalid payment amount")]
    InvalidAmount,
    #[error("payment amount or currency does not match the order")]
    AmountMismatch,
    #[error("provider response identity does not match the local order")]
    ProviderIdentityMismatch,
    #[error("provider error: {0}")]
    Provider(String),
    #[error(transparent)]
    Database(#[from] keycompute_db::DbError),
    #[error(transparent)]
    SeaOrm(#[from] sea_orm::DbErr),
    #[error(transparent)]
    Alipay(#[from] keycompute_alipay::PaymentError),
    #[error(transparent)]
    WechatPay(#[from] keycompute_wechatpay::WechatPayError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_authentication_failures_open_the_circuit() {
        let wechat = RegistryError::WechatPay(keycompute_wechatpay::WechatPayError::Api {
            status: 401,
            code: Some("SIGN_ERROR".to_string()),
            message: "invalid merchant signature".to_string(),
        });
        let alipay = RegistryError::Alipay(keycompute_alipay::PaymentError::ProviderVerification(
            "invalid response signature".to_string(),
        ));
        let alipay_rejected =
            RegistryError::Alipay(keycompute_alipay::PaymentError::ProviderRejected {
                code: Some("isv.invalid-signature".to_string()),
                message: "invalid merchant signature".to_string(),
            });

        assert!(is_terminal_provider_error(&wechat));
        assert!(is_terminal_provider_error(&alipay));
        assert!(is_terminal_provider_error(&alipay_rejected));
        assert!(!is_terminal_provider_error(
            &RegistryError::ProviderIdentityMismatch
        ));
    }

    #[test]
    fn alipay_business_rejections_only_open_the_circuit_for_credentials() {
        assert!(is_terminal_alipay_code("isv.invalid-app-id"));
        assert!(is_terminal_alipay_code("isv.permission-api-not-allowed"));
        assert!(!is_terminal_alipay_code("isp.unknown-error"));
        assert!(!is_terminal_alipay_code("ACQ.SYSTEM_ERROR"));
    }

    #[test]
    fn transient_provider_failures_only_degrade_the_circuit() {
        let transient = RegistryError::WechatPay(keycompute_wechatpay::WechatPayError::Http(
            "timeout".to_string(),
        ));

        assert!(!is_terminal_provider_error(&transient));
    }

    #[test]
    fn routine_observations_cannot_reopen_an_unavailable_provider() {
        assert!(
            RECORD_PROVIDER_RESULT_SQL
                .contains("circuit_state <> 'unavailable' OR $1 = 'unavailable'")
        );
    }

    #[test]
    fn unknown_provider_states_fail_closed() {
        let status = |state: &str| ProviderRuntimeStatus {
            configured: true,
            verified: true,
            circuit_state: state.to_string(),
            state_error: false,
        };

        assert!(status("available").accepts_new_orders());
        assert!(status("degraded").accepts_new_orders());
        assert!(!status("unavailable").accepts_new_orders());
        assert!(!status("unexpected").accepts_new_orders());
        assert!(!status("unexpected").has_valid_circuit_state());
    }

    #[test]
    fn local_alipay_redirect_creation_does_not_claim_provider_health() {
        assert!(!creation_observes_provider(PaymentMethod::Alipay, "page"));
        assert!(!creation_observes_provider(PaymentMethod::Alipay, "wap"));
        assert!(creation_observes_provider(PaymentMethod::Alipay, "qr"));
        assert!(creation_observes_provider(
            PaymentMethod::WechatPay,
            "native"
        ));
    }

    #[test]
    fn active_wechat_trust_material_changes_the_config_fingerprint() {
        let mut config = keycompute_wechatpay::WechatPayConfig {
            appid: "appid".to_string(),
            mchid: "mchid".to_string(),
            merchant_serial_no: "merchant-serial".to_string(),
            merchant_private_key: "merchant-key".to_string(),
            api_v3_key: "0123456789abcdef0123456789abcdef".to_string(),
            wechatpay_public_key_id: "PUB_KEY_ID".to_string(),
            wechatpay_public_key: "public-key-a".to_string(),
            previous_callback_keys: Vec::new(),
            notify_url: "https://example.com/notify".to_string(),
            timeout_minutes: 15,
        };
        let before = sha256_hex(wechat_config_fingerprint_material(&config).as_bytes());
        config.wechatpay_public_key = "public-key-b".to_string();
        let with_rotated_platform_key =
            sha256_hex(wechat_config_fingerprint_material(&config).as_bytes());
        assert_ne!(before, with_rotated_platform_key);

        config.wechatpay_public_key = "public-key-a".to_string();
        config.wechatpay_public_key_id = "PUB_KEY_ID_2".to_string();
        let with_rotated_platform_key_id =
            sha256_hex(wechat_config_fingerprint_material(&config).as_bytes());
        assert_ne!(before, with_rotated_platform_key_id);

        config.wechatpay_public_key_id = "PUB_KEY_ID".to_string();
        config.api_v3_key = "abcdef0123456789abcdef0123456789".to_string();
        let with_rotated_api_v3_key =
            sha256_hex(wechat_config_fingerprint_material(&config).as_bytes());
        assert_ne!(before, with_rotated_api_v3_key);

        // Historical callback keys only extend the overlap window. They do not
        // replace active response-verification or decryption credentials and
        // therefore should not force a new outbound provider verification.
        config.api_v3_key = "0123456789abcdef0123456789abcdef".to_string();
        config.previous_callback_keys = vec![keycompute_wechatpay::WechatPayCallbackKey {
            public_key_id: "OLD_KEY_ID".to_string(),
            public_key: "old-public-key".to_string(),
            api_v3_key: "0123456789abcdef0123456789abcdef".to_string(),
        }];
        let with_retained_key = sha256_hex(wechat_config_fingerprint_material(&config).as_bytes());
        assert_eq!(before, with_retained_key);

        config.merchant_private_key = "rotated-merchant-key".to_string();
        let with_rotated_outbound_key =
            sha256_hex(wechat_config_fingerprint_material(&config).as_bytes());
        assert_ne!(before, with_rotated_outbound_key);
    }

    #[test]
    fn alipay_environment_changes_the_config_fingerprint() {
        let mut config = keycompute_alipay::AlipayConfig {
            app_id: "appid".to_string(),
            private_key: "merchant-key".to_string(),
            alipay_public_key: "alipay-key".to_string(),
            previous_alipay_public_keys: Vec::new(),
            env: keycompute_alipay::AlipayEnv::Sandbox,
            notify_url: "https://example.com/notify".to_string(),
            return_url: Some("https://example.com/return".to_string()),
            sign_type: "RSA2".to_string(),
            charset: "utf-8".to_string(),
            version: "1.0".to_string(),
            timeout_minutes: 30,
        };
        let before = sha256_hex(alipay_config_fingerprint_material(&config).as_bytes());
        config.env = keycompute_alipay::AlipayEnv::Production;
        let after = sha256_hex(alipay_config_fingerprint_material(&config).as_bytes());

        assert_ne!(before, after);

        config.previous_alipay_public_keys = vec!["old-alipay-public-key".to_string()];
        let with_retained_key = sha256_hex(alipay_config_fingerprint_material(&config).as_bytes());
        assert_eq!(after, with_retained_key);
    }
}
