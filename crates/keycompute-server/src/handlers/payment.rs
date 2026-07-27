//! 支付处理器
//!
//! 处理支付相关HTTP请求

use crate::{
    error::{ApiError, Result},
    extractors::AuthExtractor,
    middleware::PaymentNotifyClientIp,
    state::AppState,
};
use axum::{
    Json,
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use keycompute_auth::Permission;
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

fn payment_internal_error(context: &'static str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(%error, context, "Payment operation failed");
    ApiError::Internal("支付服务内部错误".to_string())
}

fn require_own_billing_permission(auth: &AuthExtractor) -> Result<()> {
    if auth.has_permission(&Permission::ManageOwnBilling) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "无权访问用户支付与账单功能".to_string(),
        ))
    }
}

/// 纵深防御：除路由层的 admin_auth_middleware 外，handler 内再次校验
/// 账单管理权限，防止未来路由重排时静默丢失防护。
fn require_billing_admin_permission(auth: &AuthExtractor) -> Result<()> {
    if auth.has_permission(&Permission::ManageBilling) {
        Ok(())
    } else {
        Err(ApiError::Forbidden("无权访问支付管理功能".to_string()))
    }
}

async fn load_payment_amount_limits(pool: &keycompute_db::DbRouter) -> Result<(Decimal, Decimal)> {
    use keycompute_db::models::system_setting::setting_keys;

    #[derive(FromQueryResult)]
    struct LimitRow {
        key: String,
        value: String,
    }

    // Fetch the pair in one writer statement so a concurrent atomic settings
    // update cannot produce a mixed old/new policy snapshot.
    let rows = LimitRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT key, value FROM system_settings WHERE key IN ($1, $2)",
        [
            setting_keys::MIN_RECHARGE_AMOUNT.into(),
            setting_keys::MAX_RECHARGE_AMOUNT.into(),
        ],
    ))
    .all(pool.write_conn())
    .await
    .map_err(|error| payment_internal_error("load_payment_limits", error))?;
    let load = |key: &str, default: Decimal| -> Result<Decimal> {
        match rows.iter().find(|row| row.key == key) {
            Some(row) => row.value.parse::<Decimal>().map_err(|error| {
                payment_internal_error("load_payment_limits", format!("invalid {key}: {error}"))
            }),
            None => Ok(default),
        }
    };
    let min = load(setting_keys::MIN_RECHARGE_AMOUNT, Decimal::ONE)?;
    let max = load(setting_keys::MAX_RECHARGE_AMOUNT, Decimal::new(100_000, 0))?;
    if min <= Decimal::ZERO || min > max {
        return Err(payment_internal_error(
            "load_payment_limits",
            "stored payment limits are inconsistent",
        ));
    }
    Ok((min, max))
}

fn validate_payment_amount(amount: Decimal, min: Decimal, max: Decimal) -> Result<()> {
    if amount.normalize().scale() > 2 {
        return Err(ApiError::BadRequest("支付金额最多保留两位小数".to_string()));
    }
    if amount < min {
        return Err(ApiError::BadRequest(format!("支付金额不能低于{}元", min)));
    }
    if amount > max {
        return Err(ApiError::BadRequest(format!(
            "单笔支付金额不能超过{}元",
            max
        )));
    }
    Ok(())
}

// ==================== 请求/响应结构体 ====================

/// 支付类型
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PaymentType {
    /// 跳转支付（PC 网页）
    Page,
    /// 跳转支付（手机 H5）
    Wap,
    /// 扫码支付（当面付）
    #[default]
    Qr,
}

/// 创建支付订单请求
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePaymentOrderRequest {
    /// 支付金额（元）
    pub amount: Decimal,
    /// 支付渠道
    #[serde(default = "default_payment_method")]
    pub payment_method: String,
    /// 支付场景；兼容旧字段 payment_type
    #[serde(default = "default_payment_scene", alias = "payment_type")]
    pub payment_scene: String,
}

fn default_payment_method() -> String {
    "alipay".to_string()
}

fn default_payment_scene() -> String {
    "page".to_string()
}

/// 创建支付订单响应
#[derive(Debug, Serialize)]
pub struct CreatePaymentOrderResponse {
    /// 订单ID
    pub order_id: Uuid,
    /// 商户订单号
    pub out_trade_no: String,
    pub payment_method: String,
    /// 支付场景
    pub payment_scene: String,
    /// 兼容旧客户端；与 payment_scene 相同。
    pub payment_type: String,
    /// 支付URL（跳转支付）
    pub pay_url: Option<String>,
    /// 二维码内容（扫码支付）
    pub qr_code: Option<String>,
    /// 二维码图片URL（扫码支付）
    pub qr_code_image_url: Option<String>,
    /// 过期时间
    pub expired_at: String,
}

#[derive(Debug, Serialize)]
pub struct PaymentMethodsResponse {
    pub methods: Vec<crate::payment_registry::AvailablePaymentMethod>,
    pub min_amount: String,
    pub max_amount: String,
    pub currency: &'static str,
}

#[derive(Debug, Serialize)]
pub struct AdminPaymentProviderStatus {
    pub code: &'static str,
    pub display_name: &'static str,
    pub enabled: bool,
    pub configured: bool,
    pub available: bool,
    pub status: &'static str,
    pub scenes: &'static [&'static str],
    pub message: Option<&'static str>,
}

/// 支付订单列表响应
#[derive(Debug, Serialize)]
pub struct PaymentOrderListResponse {
    pub orders: Vec<PaymentOrderItem>,
    pub total: i64,
}

/// 支付订单项
#[derive(Debug, Serialize)]
pub struct PaymentOrderItem {
    pub id: Uuid,
    pub out_trade_no: String,
    pub amount: String,
    pub status: String,
    pub subject: String,
    pub created_at: String,
    pub expired_at: String,
}

/// 用户余额响应
#[derive(Debug, Serialize)]
pub struct UserBalanceResponse {
    pub user_id: Uuid,
    pub available_balance: String,
    pub frozen_balance: String,
    pub total_balance: String,
    pub total_recharged: String,
    pub total_consumed: String,
}

/// 支付订单查询参数
#[derive(Debug, Deserialize)]
pub struct PaymentOrderQueryParams {
    /// 状态过滤
    pub status: Option<String>,
    /// 管理员订单列表可按用户过滤；普通用户接口忽略该字段。
    pub user_id: Option<Uuid>,
    /// 页码
    pub page: Option<i64>,
    /// 每页数量
    pub page_size: Option<i64>,
    /// 旧版分页参数，保留用于 API 兼容。
    pub limit: Option<i64>,
    /// 旧版分页参数，保留用于 API 兼容。
    pub offset: Option<i64>,
}

fn normalize_pagination(params: &PaymentOrderQueryParams) -> (i64, i64, i64) {
    let page_size = params
        .page_size
        .or(params.limit)
        .unwrap_or(20)
        .clamp(1, 100);
    let page = params
        .page
        .unwrap_or_else(|| params.offset.unwrap_or(0).max(0) / page_size + 1)
        .clamp(1, 1_000_000);
    (page, page_size, (page - 1) * page_size)
}

/// 同步订单状态响应
#[derive(Debug, Serialize)]
pub struct SyncOrderResponse {
    pub order_id: Uuid,
    pub out_trade_no: String,
    pub status: String,
    pub changed: bool,
}

// ==================== Handler函数 ====================

/// 创建支付订单
///
/// POST /api/v1/payments/orders
///
/// 支持三种支付方式：
/// - page: PC网页跳转支付
/// - wap: 手机H5跳转支付
/// - qr: 扫码支付（默认）
pub async fn create_payment_order(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Json(req): Json<CreatePaymentOrderRequest>,
) -> Result<Json<CreatePaymentOrderResponse>> {
    require_own_billing_permission(&auth)?;

    // 验证金额
    if req.amount <= Decimal::ZERO {
        return Err(ApiError::BadRequest("支付金额必须大于0".to_string()));
    }

    // 获取数据库连接池
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| payment_internal_error("create_order", "database unavailable"))?;

    // Monetary policy is read from the writer and parsed directly as Decimal.
    let (min_amount, max_amount) = load_payment_amount_limits(pool).await?;

    validate_payment_amount(req.amount, min_amount, max_amount)?;

    let registry = state
        .payment
        .as_ref()
        .ok_or_else(|| payment_internal_error("create_order", "payment registry unavailable"))?;

    let payment_method = keycompute_db::PaymentMethod::parse(&req.payment_method)
        .ok_or_else(|| ApiError::BadRequest("不支持的支付方式".to_string()))?;
    let site_name = keycompute_db::SystemSetting::find_by_key(
        pool,
        keycompute_db::models::system_setting::setting_keys::SITE_NAME,
    )
    .await
    .ok()
    .flatten()
    .map(|setting| setting.value)
    .unwrap_or_else(|| "KeyCompute".to_string());
    let create_req = crate::payment_registry::PaymentCreateRequest {
        tenant_id: auth.tenant_id,
        user_id: auth.user_id,
        method: payment_method,
        scene: req.payment_scene,
        amount: req.amount,
        subject: "账户充值".to_string(),
        body: Some(format!("{site_name} 账户充值 {} 元", req.amount)),
    };
    let created = registry
        .create_order(create_req)
        .await
        .map_err(|error| match error {
            crate::payment_registry::RegistryError::Unavailable(_)
            | crate::payment_registry::RegistryError::UnsupportedScene(_) => {
                ApiError::BadRequest(error.to_string())
            }
            _ => payment_internal_error("create_order", error),
        })?;
    let qr_code_image_url = created.qr_code.as_deref().and_then(qr_code_data_url);
    let result = CreatePaymentOrderResponse {
        order_id: created.order_id,
        out_trade_no: created.out_trade_no,
        payment_method: created.payment_method,
        payment_type: created.payment_scene.clone(),
        payment_scene: created.payment_scene,
        pay_url: created.pay_url,
        qr_code: created.qr_code,
        qr_code_image_url,
        expired_at: created.expired_at.to_rfc3339(),
    };

    Ok(Json(result))
}

fn qr_code_data_url(content: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let code = qrcode::QrCode::new(content.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .dark_color(qrcode::render::svg::Color("#111827"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Some(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg)
    ))
}

/// 返回当前真正可接受新订单的支付渠道。
pub async fn list_payment_methods(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<PaymentMethodsResponse>> {
    require_own_billing_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| payment_internal_error("list_methods", "database unavailable"))?;
    let registry = state
        .payment
        .as_ref()
        .ok_or_else(|| payment_internal_error("list_methods", "payment registry unavailable"))?;
    let (min, max) = load_payment_amount_limits(pool).await?;
    let methods = registry
        .available_methods()
        .await
        .map_err(|error| payment_internal_error("list_methods", error))?;
    Ok(Json(PaymentMethodsResponse {
        methods,
        min_amount: min.to_string(),
        max_amount: max.to_string(),
        currency: "CNY",
    }))
}

/// 获取我的支付订单列表
///
/// GET /api/v1/payments/orders
pub async fn list_my_payment_orders(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Query(params): Query<PaymentOrderQueryParams>,
) -> Result<Json<PaymentOrderListResponse>> {
    require_own_billing_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| payment_internal_error("list_orders", "database unavailable"))?;

    let (_page, page_size, offset) = normalize_pagination(&params);
    let status_value = params
        .status
        .as_deref()
        .map(|status| status.into())
        .unwrap_or(sea_orm::Value::String(None));
    let orders = keycompute_db::PaymentOrder::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM payment_orders WHERE user_id=$1 AND tenant_id=$2 AND ($3::text IS NULL OR status=$3) ORDER BY created_at DESC LIMIT $4 OFFSET $5",
        [
            auth.user_id.into(),
            auth.tenant_id.into(),
            status_value.clone(),
            page_size.into(),
            offset.into(),
        ],
    ))
    .all(pool)
    .await
    .map_err(|e| payment_internal_error("list_orders", e))?;
    #[derive(FromQueryResult)]
    struct CountRow {
        total: i64,
    }
    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS total FROM payment_orders WHERE user_id=$1 AND tenant_id=$2 AND ($3::text IS NULL OR status=$3)",
        [auth.user_id.into(), auth.tenant_id.into(), status_value],
    ))
    .one(pool)
    .await
    .map_err(|e| payment_internal_error("count_orders", e))?
    .map(|row| row.total)
    .unwrap_or(0);

    let items: Vec<PaymentOrderItem> = orders
        .into_iter()
        .map(|o| PaymentOrderItem {
            id: o.id,
            out_trade_no: o.out_trade_no,
            amount: o.amount.to_string(),
            status: o.status,
            subject: o.subject,
            created_at: o.created_at.to_rfc3339(),
            expired_at: o.expired_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(PaymentOrderListResponse {
        total,
        orders: items,
    }))
}

/// 获取支付订单详情
///
/// GET /api/v1/payments/orders/{id}
pub async fn get_payment_order(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Path(order_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    require_own_billing_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| payment_internal_error("get_order", "database unavailable"))?;

    let order = keycompute_db::PaymentOrder::find_by_id(pool, order_id)
        .await
        .map_err(|e| payment_internal_error("get_order", e))?
        .ok_or(ApiError::NotFound("订单不存在".to_string()))?;

    // 验证权限
    if !auth.has_permission(&Permission::SystemAdmin)
        && (order.user_id != auth.user_id || order.tenant_id != auth.tenant_id)
    {
        return Err(ApiError::Forbidden("无权访问此订单".to_string()));
    }

    Ok(Json(serde_json::json!({
        "id": order.id,
        "out_trade_no": order.out_trade_no,
        "trade_no": order.trade_no,
        "amount": order.amount.to_string(),
        "status": order.status,
        "subject": order.subject,
        "body": order.body,
        "payment_method": order.payment_method,
        "pay_url": order.pay_url,
        "expired_at": order.expired_at.to_rfc3339(),
        "paid_at": order.paid_at.map(|t| t.to_rfc3339()),
        "created_at": order.created_at.to_rfc3339(),
    })))
}

/// 获取我的余额
///
/// GET /api/v1/payments/balance
pub async fn get_my_balance(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<UserBalanceResponse>> {
    require_own_billing_permission(&auth)?;

    let balance_service = state
        .billing
        .balance_service()
        .ok_or_else(|| payment_internal_error("get_balance", "balance service unavailable"))?;

    let balance = balance_service
        .get_or_create(auth.tenant_id, auth.user_id)
        .await
        .map_err(|e| payment_internal_error("get_balance", e))?;

    Ok(Json(UserBalanceResponse {
        user_id: balance.user_id,
        available_balance: balance.available_balance.to_string(),
        frozen_balance: balance.frozen_balance.to_string(),
        total_balance: balance.total_balance().to_string(),
        total_recharged: balance.total_recharged.to_string(),
        total_consumed: balance.total_consumed.to_string(),
    }))
}

/// 同步订单状态
///
/// POST /api/v1/payments/sync/{out_trade_no}
pub async fn sync_payment_order(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Path(order_ref): Path<String>,
) -> Result<Json<SyncOrderResponse>> {
    require_own_billing_permission(&auth)?;

    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| payment_internal_error("sync_order", "database unavailable"))?;
    let order = if let Ok(order_id) = Uuid::parse_str(&order_ref) {
        keycompute_db::PaymentOrder::find_by_id(pool.write_conn(), order_id).await
    } else {
        // 兼容一个发布周期的旧商户订单号路由。
        keycompute_db::PaymentOrder::find_by_out_trade_no(pool.write_conn(), &order_ref).await
    }
    .map_err(|e| payment_internal_error("find_order_for_sync", e))?
    .ok_or(ApiError::NotFound("订单不存在".to_string()))?;
    if order.user_id != auth.user_id || order.tenant_id != auth.tenant_id {
        return Err(ApiError::NotFound("订单不存在".to_string()));
    }

    let registry = state
        .payment
        .as_ref()
        .ok_or_else(|| payment_internal_error("sync_order", "payment registry unavailable"))?;
    let (status, changed) = registry
        .sync_order(&order)
        .await
        .map_err(|e| payment_internal_error("sync_order", e))?;

    Ok(Json(SyncOrderResponse {
        order_id: order.id,
        out_trade_no: order.out_trade_no,
        status,
        changed,
    }))
}

/// 支付宝异步通知
///
/// POST /api/v1/payments/notify/alipay
///
/// 注意：此接口不需要认证，由支付宝服务器调用
pub async fn alipay_notify(
    State(state): State<AppState>,
    Extension(source_ip): Extension<PaymentNotifyClientIp>,
    // 支付宝通知使用 form-data 格式
    form: String,
) -> Result<String> {
    let registry = state
        .payment
        .as_ref()
        .ok_or_else(|| payment_internal_error("alipay_notify", "payment registry unavailable"))?;
    let payment_service = registry
        .alipay()
        .ok_or_else(|| payment_internal_error("alipay_notify", "provider unavailable"))?;

    // 解析 form 数据
    let params = parse_alipay_form(&form);
    let notify_id = bounded_alipay_notify_id(&params).map(str::to_owned);

    // 处理通知
    match payment_service.handle_notify(params).await {
        Ok(_) => Ok("success".to_string()),
        Err(e) => {
            if !e.is_retryable()
                && let Some(pool) = state.pool.as_deref()
            {
                record_security_event(
                    pool,
                    "alipay",
                    "rejected_notification",
                    notify_id.as_deref(),
                    &crate::payment_registry::sha256_hex(form.as_bytes()),
                    &e.to_string(),
                    &source_ip.0,
                )
                .await;
            }
            if e.is_retryable() {
                tracing::error!("处理支付宝通知失败（将重试）: {}", e);
            } else {
                // 对未能验证或未能入账的通知不得返回 success。恶意请求的
                // 响应不会触发支付宝重试，真实平台通知则可在配置修复后重放。
                tracing::error!("处理支付宝通知失败（未确认）: {}", e);
            }
            Ok(alipay_notify_failure_response().to_string())
        }
    }
}

fn alipay_notify_failure_response() -> &'static str {
    "fail"
}

fn parse_alipay_form(form: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(form.as_bytes())
        .into_owned()
        .collect()
}

fn bounded_alipay_notify_id(params: &HashMap<String, String>) -> Option<&str> {
    params
        .get("notify_id")
        .filter(|value| value.chars().count() <= 128)
        .map(String::as_str)
}

/// 微信支付 API v3 异步通知。
pub async fn wechatpay_notify(
    State(state): State<AppState>,
    Extension(source_ip): Extension<PaymentNotifyClientIp>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(registry) = state.payment.as_ref() else {
        return wechat_error_response(StatusCode::SERVICE_UNAVAILABLE, "provider unavailable");
    };
    let Some(client) = registry.wechatpay() else {
        return wechat_error_response(StatusCode::SERVICE_UNAVAILABLE, "provider unavailable");
    };
    let Some(pool) = state.pool.as_deref() else {
        return wechat_error_response(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    let get = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let notify_headers = match (
        get("Wechatpay-Timestamp"),
        get("Wechatpay-Nonce"),
        get("Wechatpay-Signature"),
        get("Wechatpay-Serial"),
    ) {
        (Some(timestamp), Some(nonce), Some(signature), Some(serial)) => {
            keycompute_wechatpay::NotifyHeaders {
                timestamp,
                nonce,
                signature,
                serial,
            }
        }
        _ => return wechat_error_response(StatusCode::BAD_REQUEST, "missing signature headers"),
    };

    let digest = crate::payment_registry::sha256_hex(&body);
    let (envelope, notify) = match client.verify_and_decode_notify(&notify_headers, &body) {
        Ok(value) => value,
        Err(error) => {
            record_security_event(
                pool,
                "wechatpay",
                "signature_or_decrypt",
                None,
                &digest,
                &error.to_string(),
                &source_ip.0,
            )
            .await;
            return wechat_error_response(StatusCode::BAD_REQUEST, "invalid notification");
        }
    };
    if notify.appid != client.config().appid || notify.mchid != client.config().mchid {
        record_security_event(
            pool,
            "wechatpay",
            "merchant_mismatch",
            Some(&envelope.id),
            &digest,
            "appid or mchid mismatch",
            &source_ip.0,
        )
        .await;
        return wechat_error_response(StatusCode::BAD_REQUEST, "merchant mismatch");
    }
    if envelope.event_type != "TRANSACTION.SUCCESS" || notify.trade_state != "SUCCESS" {
        return StatusCode::NO_CONTENT.into_response();
    }
    let order = match keycompute_db::PaymentOrder::find_by_out_trade_no(
        pool.write_conn(),
        &notify.out_trade_no,
    )
    .await
    {
        Ok(Some(order)) if order.payment_method == "wechatpay" => order,
        Ok(_) => return wechat_error_response(StatusCode::NOT_FOUND, "order not found"),
        Err(error) => {
            // 瞬态数据库故障与确定性 not-found 分离：500 会触发微信重试，
            // 且监控上可区分故障类型。
            tracing::error!(%error, out_trade_no = %notify.out_trade_no, "WeChat Pay notification order lookup failed");
            return wechat_error_response(StatusCode::INTERNAL_SERVER_ERROR, "processing failed");
        }
    };
    let actual = match crate::payment_registry::decimal_from_cents(notify.amount.total) {
        Ok(amount) => amount,
        Err(_) => return wechat_error_response(StatusCode::BAD_REQUEST, "invalid amount"),
    };
    if actual != order.amount || notify.amount.currency != order.currency {
        record_security_event(
            pool,
            "wechatpay",
            "amount_mismatch",
            Some(&envelope.id),
            &digest,
            "amount or currency mismatch",
            &source_ip.0,
        )
        .await;
        return wechat_error_response(StatusCode::BAD_REQUEST, "amount mismatch");
    }
    let payload = serde_json::to_value(&notify).unwrap_or_else(|error| {
        // 审计 payload 降级为 null 时必须留痕，避免静默丢失对账依据。
        tracing::warn!(%error, out_trade_no = %notify.out_trade_no, "Failed to serialize WeChat Pay notify payload for audit");
        serde_json::Value::Null
    });
    match crate::payment_registry::credit_paid_order(
        pool,
        &order,
        &notify.transaction_id,
        &envelope.id,
        payload,
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(%error, order_id = %order.id, "WeChat Pay notification failed");
            wechat_error_response(StatusCode::INTERNAL_SERVER_ERROR, "processing failed")
        }
    }
}

fn wechat_error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"code": "FAIL", "message": message})),
    )
        .into_response()
}

async fn record_security_event(
    pool: &keycompute_db::DbRouter,
    payment_method: &str,
    event_type: &str,
    request_id: Option<&str>,
    digest: &str,
    detail: &str,
    source_ip: &str,
) {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO payment_security_events(payment_method, event_type, request_id, payload_digest, detail, source_ip) VALUES ($1, $2, $3, $4, $5, $6)",
        [
            payment_method.into(),
            event_type.into(),
            request_id.map(str::to_owned).into(),
            digest.into(),
            detail.into(),
            source_ip.into(),
        ],
    );
    if let Err(error) = pool.execute(statement).await {
        tracing::warn!(%error, "Failed to record payment security event");
    }
}

// ==================== 管理员接口 ====================

/// 管理员获取所有支付订单
///
/// GET /api/v1/admin/payments/orders
pub async fn admin_list_payment_orders(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Query(params): Query<PaymentOrderQueryParams>,
) -> Result<Json<serde_json::Value>> {
    require_billing_admin_permission(&auth)?;
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| payment_internal_error("admin_list_orders", "database unavailable"))?;

    // 管理员可以查看所有订单
    let (page, page_size, offset) = normalize_pagination(&params);

    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT * FROM payment_orders
        WHERE ($1::text IS NULL OR status = $1)
          AND ($2::uuid IS NULL OR user_id = $2)
        ORDER BY created_at DESC
        LIMIT $3 OFFSET $4
        "#,
        [
            params
                .status
                .as_deref()
                .map(|s| s.into())
                .unwrap_or(sea_orm::Value::String(None)),
            params
                .user_id
                .map(Into::into)
                .unwrap_or(sea_orm::Value::Uuid(None)),
            page_size.into(),
            offset.into(),
        ],
    );
    let orders = keycompute_db::PaymentOrder::find_by_statement(stmt)
        .all(pool)
        .await
        .map_err(|e| payment_internal_error("admin_list_orders", e))?;
    #[derive(FromQueryResult)]
    struct CountRow {
        total: i64,
    }
    let count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS total FROM payment_orders WHERE ($1::text IS NULL OR status=$1) AND ($2::uuid IS NULL OR user_id=$2)",
        [
            params
                .status
                .as_deref()
                .map(|status| status.into())
                .unwrap_or(sea_orm::Value::String(None)),
            params
                .user_id
                .map(Into::into)
                .unwrap_or(sea_orm::Value::Uuid(None)),
        ],
    ))
    .one(pool)
    .await
    .map_err(|e| payment_internal_error("admin_count_orders", e))?
    .map(|row| row.total)
    .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "orders": orders.iter().map(|o| serde_json::json!({
            "id": o.id,
            "tenant_id": o.tenant_id,
            "user_id": o.user_id,
            "out_trade_no": o.out_trade_no,
            "trade_no": o.trade_no,
            "provider_trade_no": o.provider_trade_no,
            "payment_method": o.payment_method,
            "amount": o.amount.to_string(),
            "status": o.status,
            "subject": o.subject,
            "created_at": o.created_at.to_rfc3339(),
        })).collect::<Vec<_>>(),
        "total": count,
        "page": page,
        "page_size": page_size,
        "total_pages": (count + page_size - 1) / page_size,
    })))
}

/// 管理员查看所有支付渠道的运营开关与实际运行状态。
pub async fn admin_payment_providers(
    auth: AuthExtractor,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminPaymentProviderStatus>>> {
    use keycompute_db::models::system_setting::setting_keys;
    require_billing_admin_permission(&auth)?;
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| payment_internal_error("admin_provider_status", "database unavailable"))?;
    let registry = state.payment.as_ref().ok_or_else(|| {
        payment_internal_error("admin_provider_status", "payment registry unavailable")
    })?;
    let alipay_enabled =
        keycompute_db::SystemSetting::get_bool(pool, setting_keys::ALIPAY_ENABLED, false).await;
    let wechat_enabled =
        keycompute_db::SystemSetting::get_bool(pool, setting_keys::WECHATPAY_ENABLED, false).await;
    let alipay_status = registry
        .provider_status(keycompute_db::PaymentMethod::Alipay)
        .await;
    let wechat_status = registry
        .provider_status(keycompute_db::PaymentMethod::WechatPay)
        .await;
    let build = |code,
                 display_name,
                 enabled,
                 provider: crate::payment_registry::ProviderRuntimeStatus,
                 scenes: &'static [&'static str]| {
        let available = enabled && provider.accepts_new_orders();
        AdminPaymentProviderStatus {
            code,
            display_name,
            enabled,
            configured: provider.configured,
            available,
            status: if !enabled {
                "disabled"
            } else if !provider.configured {
                "misconfigured"
            } else if provider.state_error {
                "state_error"
            } else if !provider.verified {
                "configured_unverified"
            } else if !provider.has_valid_circuit_state() {
                "state_error"
            } else if provider.circuit_state == "unavailable" {
                "unavailable"
            } else if provider.circuit_state == "degraded" {
                "degraded"
            } else {
                "available"
            },
            scenes,
            message: if enabled && !provider.configured {
                Some("运营开关已开启，但密钥或通知地址配置不完整")
            } else if enabled && provider.state_error {
                Some("支付渠道状态存储暂不可用，已停止接受新订单")
            } else if enabled && !provider.verified {
                Some("配置已加载，但尚未通过真实渠道验证；验证成功前不会向用户展示")
            } else if !provider.has_valid_circuit_state() {
                Some("支付渠道状态无效，已停止接受新订单")
            } else if provider.circuit_state == "unavailable" {
                Some("渠道发生确定性认证或验签错误，已停止接受新订单")
            } else if provider.circuit_state == "degraded" {
                Some("渠道近期请求异常，仍允许新订单并持续观察")
            } else {
                None
            },
        }
    };
    Ok(Json(vec![
        build(
            "alipay",
            "支付宝",
            alipay_enabled,
            alipay_status,
            &["page", "wap", "qr"],
        ),
        build(
            "wechatpay",
            "微信支付",
            wechat_enabled,
            wechat_status,
            &["native"],
        ),
    ]))
}

/// 管理员通过创建并关闭一笔真实的 0.01 元订单验证当前渠道配置。
pub async fn admin_verify_payment_provider(
    auth: AuthExtractor,
    State(state): State<AppState>,
    Path(method): Path<String>,
) -> Result<Json<serde_json::Value>> {
    require_billing_admin_permission(&auth)?;
    let method = keycompute_db::PaymentMethod::parse(&method)
        .ok_or_else(|| ApiError::BadRequest("不支持的支付渠道".to_string()))?;
    let registry = state
        .payment
        .as_ref()
        .ok_or_else(|| payment_internal_error("verify_provider", "payment registry unavailable"))?;
    registry
        .verify_provider(method, auth.tenant_id, auth.user_id)
        .await
        .map_err(|error| payment_internal_error("verify_provider", error))?;
    Ok(Json(serde_json::json!({ "message": "支付渠道验证成功" })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keycompute_auth::{AuthType, build_permissions};

    #[test]
    fn test_create_payment_order_request() {
        let req = CreatePaymentOrderRequest {
            amount: Decimal::new(100, 0),
            payment_method: "wechatpay".to_string(),
            payment_scene: "native".to_string(),
        };

        assert_eq!(req.amount, Decimal::new(100, 0));
    }

    #[test]
    fn test_self_service_billing_rejects_api_keys_and_allows_user_jwt_permissions() {
        let api_key_auth =
            AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "user")
                .with_permissions(build_permissions(AuthType::ApiKey, "user"));
        assert!(matches!(
            require_own_billing_permission(&api_key_auth),
            Err(ApiError::Forbidden(_))
        ));

        let jwt_auth = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::nil(), "user")
            .with_permissions(build_permissions(AuthType::Jwt, "user"));
        assert!(require_own_billing_permission(&jwt_auth).is_ok());
    }

    #[test]
    fn test_billing_admin_permission_gates_admin_payment_handlers() {
        // 普通用户 JWT 没有 ManageBilling，必须被拒绝
        let user_jwt = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::nil(), "user")
            .with_permissions(build_permissions(AuthType::Jwt, "user"));
        assert!(matches!(
            require_billing_admin_permission(&user_jwt),
            Err(ApiError::Forbidden(_))
        ));

        // API Key 即使角色是 admin 也只有 UseApi，必须被拒绝
        let admin_api_key =
            AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), "admin")
                .with_permissions(build_permissions(AuthType::ApiKey, "admin"));
        assert!(matches!(
            require_billing_admin_permission(&admin_api_key),
            Err(ApiError::Forbidden(_))
        ));

        // admin / system 角色的 JWT 均持有 ManageBilling
        for role in ["admin", "system"] {
            let jwt = AuthExtractor::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::nil(), role)
                .with_permissions(build_permissions(AuthType::Jwt, role));
            assert!(require_billing_admin_permission(&jwt).is_ok());
        }
    }

    #[test]
    fn test_parse_alipay_form_uses_form_urlencoded_rules() {
        let params = parse_alipay_form("subject=account+recharge&literal=a%2Bb");
        assert_eq!(
            params.get("subject").map(String::as_str),
            Some("account recharge")
        );
        assert_eq!(params.get("literal").map(String::as_str), Some("a+b"));
    }

    #[test]
    fn test_alipay_notify_id_is_decoded_and_bounded_for_audit_storage() {
        let params = parse_alipay_form("notify_id=encoded%2Bid");
        assert_eq!(bounded_alipay_notify_id(&params), Some("encoded+id"));

        let oversized = HashMap::from([("notify_id".to_string(), "x".repeat(129))]);
        assert_eq!(bounded_alipay_notify_id(&oversized), None);
    }

    #[test]
    fn test_create_response_keeps_legacy_payment_type() {
        let response = CreatePaymentOrderResponse {
            order_id: Uuid::nil(),
            out_trade_no: "KC1".to_string(),
            payment_method: "wechatpay".to_string(),
            payment_scene: "native".to_string(),
            payment_type: "native".to_string(),
            pay_url: None,
            qr_code: None,
            qr_code_image_url: None,
            expired_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["payment_scene"], "native");
        assert_eq!(json["payment_type"], "native");
    }

    #[test]
    fn test_payment_pagination_supports_legacy_and_page_parameters() {
        let legacy: PaymentOrderQueryParams =
            serde_json::from_value(serde_json::json!({"limit": 50, "offset": 100})).unwrap();
        let modern: PaymentOrderQueryParams = serde_json::from_value(
            serde_json::json!({"page": 3, "page_size": 25, "limit": 10, "offset": 0}),
        )
        .unwrap();

        assert_eq!(normalize_pagination(&legacy), (3, 50, 100));
        assert_eq!(normalize_pagination(&modern), (3, 25, 50));
    }

    #[test]
    fn test_payment_pagination_clamps_boundary_values() {
        // 负 offset：不得产生负页码或负 offset
        let negative: PaymentOrderQueryParams =
            serde_json::from_value(serde_json::json!({"offset": -100})).unwrap();
        assert_eq!(normalize_pagination(&negative), (1, 20, 0));

        // page_size 超上限：钳制到 100；page_size 为 0：钳制到 1
        let oversized: PaymentOrderQueryParams =
            serde_json::from_value(serde_json::json!({"page_size": 10_000})).unwrap();
        assert_eq!(normalize_pagination(&oversized), (1, 100, 0));
        let zero: PaymentOrderQueryParams =
            serde_json::from_value(serde_json::json!({"page_size": 0})).unwrap();
        assert_eq!(normalize_pagination(&zero), (1, 1, 0));

        // page 超上限：钳制到 1_000_000，offset 不溢出
        let huge_page: PaymentOrderQueryParams =
            serde_json::from_value(serde_json::json!({"page": 9_000_000_000i64, "page_size": 100}))
                .unwrap();
        let (page, page_size, offset) = normalize_pagination(&huge_page);
        assert_eq!((page, page_size), (1_000_000, 100));
        assert_eq!(offset, (1_000_000 - 1) * 100);
    }

    #[test]
    fn payment_amount_limits_compare_exact_decimals() {
        let min: Decimal = "0.10".parse().unwrap();
        let max: Decimal = "9999999999.99".parse().unwrap();
        assert!(validate_payment_amount(min, min, max).is_ok());
        assert!(validate_payment_amount(max, min, max).is_ok());
        assert!(validate_payment_amount("0.09".parse().unwrap(), min, max).is_err());
        assert!(validate_payment_amount("10000000000.00".parse().unwrap(), min, max).is_err());
        assert!(validate_payment_amount("1.2300".parse().unwrap(), min, max).is_ok());
        for invalid in ["1.001", "1.005", "1.999"] {
            assert!(
                validate_payment_amount(invalid.parse().unwrap(), min, max).is_err(),
                "{invalid} should be rejected instead of normalized differently by the provider and database"
            );
        }
    }

    #[test]
    fn test_alipay_failures_are_never_acknowledged_as_success() {
        assert_eq!(alipay_notify_failure_response(), "fail");
    }

    #[test]
    fn test_internal_payment_errors_are_hidden_from_clients() {
        let error = payment_internal_error(
            "test",
            "postgres password authentication failed for secret-user",
        );
        match error {
            ApiError::Internal(message) => {
                assert_eq!(message, "支付服务内部错误");
                assert!(!message.contains("secret-user"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
