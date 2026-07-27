//! 支付模块
//!
//! 处理支付订单创建、查询和余额获取

use crate::client::ApiClient;
use crate::error::Result;
use serde::{Deserialize, Serialize};

use super::common::encode_query_value;

/// 支付 API 客户端
#[derive(Debug, Clone)]
pub struct PaymentApi {
    client: ApiClient,
}

impl PaymentApi {
    /// 创建新的支付 API 客户端
    pub fn new(client: &ApiClient) -> Self {
        Self {
            client: client.clone(),
        }
    }

    /// 创建支付订单
    pub async fn create_payment_order(
        &self,
        req: &CreatePaymentOrderRequest,
        token: &str,
    ) -> Result<CreatePaymentOrderResponse> {
        self.client
            .post_json("/api/v1/payments/orders", req, Some(token))
            .await
    }

    /// 获取我的支付订单列表
    pub async fn list_my_payment_orders(
        &self,
        params: Option<&PaymentQueryParams>,
        token: &str,
    ) -> Result<Vec<PaymentOrderSummary>> {
        let path = if let Some(p) = params {
            format!("/api/v1/payments/orders?{}", p.to_query_string())
        } else {
            "/api/v1/payments/orders".to_string()
        };
        let resp: PaymentOrderPage = self.client.get_json(&path, Some(token)).await?;
        Ok(resp.orders)
    }

    pub async fn list_my_payment_orders_page(
        &self,
        params: Option<&PaymentQueryParams>,
        token: &str,
    ) -> Result<PaymentOrderPage> {
        let path = params
            .map(|params| format!("/api/v1/payments/orders?{}", params.to_query_string()))
            .unwrap_or_else(|| "/api/v1/payments/orders".to_string());
        self.client.get_json(&path, Some(token)).await
    }

    /// 获取订单详情
    pub async fn get_payment_order(&self, id: &str, token: &str) -> Result<PaymentOrderResponse> {
        self.client
            .get_json(&format!("/api/v1/payments/orders/{}", id), Some(token))
            .await
    }

    /// 同步订单状态
    pub async fn sync_payment_order(
        &self,
        order_id: &str,
        token: &str,
    ) -> Result<SyncPaymentOrderResponse> {
        self.client
            .post_json(
                &format!("/api/v1/payments/orders/{order_id}/sync"),
                &serde_json::json!({}),
                Some(token),
            )
            .await
    }

    /// 获取我的余额
    pub async fn get_my_balance(&self, token: &str) -> Result<UserBalanceResponse> {
        self.client
            .get_json("/api/v1/payments/balance", Some(token))
            .await
    }

    /// 获取当前可接受新订单的支付渠道。
    pub async fn get_payment_methods(&self, token: &str) -> Result<PaymentMethodsResponse> {
        self.client
            .get_json("/api/v1/payments/methods", Some(token))
            .await
    }
}

/// 创建支付订单请求
#[derive(Debug, Clone, Serialize)]
pub struct CreatePaymentOrderRequest {
    pub amount: String,
    pub subject: String,
    pub body: Option<String>,
    #[serde(skip_serializing_if = "is_default_alipay")]
    pub payment_method: String,
    pub payment_type: String,
}

fn is_default_alipay(value: &String) -> bool {
    value == "alipay"
}

impl CreatePaymentOrderRequest {
    #[deprecated(note = "f64 无法精确表达两位小数金额，请改用 new_for_method 传入字符串金额")]
    pub fn new(amount: f64, subject: impl Into<String>, payment_type: impl Into<String>) -> Self {
        Self {
            amount: format_amount(amount),
            subject: subject.into(),
            body: None,
            payment_method: "alipay".to_string(),
            payment_type: payment_type.into(),
        }
    }

    pub fn new_for_method(
        amount: impl Into<String>,
        payment_method: impl Into<String>,
        payment_scene: impl Into<String>,
    ) -> Self {
        Self {
            amount: amount.into(),
            subject: String::new(),
            body: None,
            payment_method: payment_method.into(),
            payment_type: payment_scene.into(),
        }
    }

    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }
}

/// 支付订单查询参数
#[derive(Debug, Clone, Serialize, Default)]
pub struct PaymentQueryParams {
    pub status: Option<String>,
    pub limit: Option<i32>,
    pub offset: Option<i32>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

impl PaymentQueryParams {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_limit(mut self, limit: i32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn with_offset(mut self, offset: i32) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn with_page(mut self, page: u32) -> Self {
        self.page = Some(page);
        self
    }

    pub fn with_page_size(mut self, page_size: u32) -> Self {
        self.page_size = Some(page_size);
        self
    }

    pub fn to_query_string(&self) -> String {
        let mut params = Vec::new();
        if let Some(ref status) = self.status {
            params.push(format!("status={}", encode_query_value(status)));
        }
        if let Some(limit) = self.limit {
            params.push(format!("limit={}", limit));
        }
        if let Some(offset) = self.offset {
            params.push(format!("offset={}", offset));
        }
        if let Some(page) = self.page {
            params.push(format!("page={page}"));
        }
        if let Some(page_size) = self.page_size {
            params.push(format!("page_size={page_size}"));
        }
        params.join("&")
    }
}

fn format_amount(amount: f64) -> String {
    // Preserve the caller's value so the server can reject unsupported precision instead of
    // silently creating an order for a rounded amount. Callers that require exact decimal input
    // should prefer `new_for_method`, which accepts a string.
    amount.to_string()
}

/// 创建支付订单响应
#[derive(Debug, Clone)]
pub struct CreatePaymentOrderResponse {
    pub order_id: String,
    pub out_trade_no: String,
    pub payment_method: String,
    pub payment_type: String,
    pub pay_url: Option<String>,
    pub qr_code: Option<String>,
    pub qr_code_image_url: Option<String>,
    pub expired_at: String,
}

impl<'de> Deserialize<'de> for CreatePaymentOrderResponse {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireResponse {
            order_id: String,
            out_trade_no: String,
            #[serde(default = "default_alipay_method")]
            payment_method: String,
            #[serde(default)]
            payment_scene: Option<String>,
            #[serde(default)]
            payment_type: Option<String>,
            pay_url: Option<String>,
            qr_code: Option<String>,
            qr_code_image_url: Option<String>,
            expired_at: String,
        }

        let wire = WireResponse::deserialize(deserializer)?;
        let payment_type = match (wire.payment_scene, wire.payment_type) {
            (Some(scene), Some(legacy)) if scene != legacy => {
                return Err(serde::de::Error::custom(
                    "payment_scene and payment_type disagree",
                ));
            }
            (Some(scene), _) => scene,
            (_, Some(legacy)) => legacy,
            (None, None) => return Err(serde::de::Error::missing_field("payment_scene")),
        };
        Ok(Self {
            order_id: wire.order_id,
            out_trade_no: wire.out_trade_no,
            payment_method: wire.payment_method,
            payment_type,
            pay_url: wire.pay_url,
            qr_code: wire.qr_code,
            qr_code_image_url: wire.qr_code_image_url,
            expired_at: wire.expired_at,
        })
    }
}

fn default_alipay_method() -> String {
    "alipay".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentMethodsResponse {
    pub methods: Vec<PaymentMethodInfo>,
    pub min_amount: String,
    pub max_amount: String,
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PaymentMethodInfo {
    pub code: String,
    pub display_name: String,
    pub scenes: Vec<String>,
    pub recommended_scene: String,
    pub sort_order: u16,
    pub is_default: bool,
}

#[cfg(test)]
mod tests {
    use super::{CreatePaymentOrderRequest, CreatePaymentOrderResponse};

    #[test]
    #[allow(deprecated)]
    fn legacy_float_constructor_does_not_round_the_requested_amount() {
        assert_eq!(
            CreatePaymentOrderRequest::new(1.005, "recharge", "page").amount,
            "1.005"
        );
        assert_eq!(
            CreatePaymentOrderRequest::new(0.009, "recharge", "page").amount,
            "0.009"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn legacy_float_constructor_leaves_invalid_values_for_server_validation() {
        assert_eq!(
            CreatePaymentOrderRequest::new(f64::NAN, "recharge", "page").amount,
            "NaN"
        );
        assert_eq!(
            CreatePaymentOrderRequest::new(f64::INFINITY, "recharge", "page").amount,
            "inf"
        );
    }

    #[test]
    fn conflicting_scene_compatibility_fields_are_rejected() {
        let error = serde_json::from_value::<CreatePaymentOrderResponse>(serde_json::json!({
            "order_id": "order-conflict",
            "out_trade_no": "PAY_CONFLICT",
            "payment_method": "wechatpay",
            "payment_scene": "native",
            "payment_type": "page",
            "pay_url": null,
            "qr_code": null,
            "qr_code_image_url": null,
            "expired_at": "2026-07-26T11:00:00Z"
        }))
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("payment_scene and payment_type disagree")
        );
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PaymentOrderPage {
    pub orders: Vec<PaymentOrderSummary>,
    #[serde(default)]
    pub total: u64,
}

/// 支付订单响应
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentOrderResponse {
    pub id: String,
    pub out_trade_no: String,
    pub amount: String,
    pub status: String,
    pub subject: String,
    pub body: Option<String>,
    pub payment_method: String,
    pub pay_url: Option<String>,
    pub expired_at: String,
    pub paid_at: Option<String>,
    pub created_at: String,
}

/// 同步订单状态响应
#[derive(Debug, Clone, Deserialize)]
pub struct SyncPaymentOrderResponse {
    pub order_id: String,
    pub out_trade_no: String,
    pub status: String,
    pub changed: bool,
}

/// 支付订单摘要
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentOrderSummary {
    pub id: String,
    pub out_trade_no: String,
    pub amount: String,
    pub status: String,
    pub subject: String,
    pub created_at: String,
    pub expired_at: String,
}

/// 用户余额响应（用户查询自己余额时返回）
#[derive(Debug, Clone, Deserialize)]
pub struct UserBalanceResponse {
    pub user_id: String,
    pub available_balance: String,
    pub frozen_balance: String,
    pub total_balance: String,
    pub total_recharged: String,
    pub total_consumed: String,
}
