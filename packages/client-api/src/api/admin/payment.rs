//! 支付订单管理相关类型（Admin 视角）

use serde::{Deserialize, Serialize};

use crate::api::common::encode_query_value;

/// 支付订单查询参数（Admin）
#[derive(Debug, Clone, Serialize, Default)]
pub struct PaymentQueryParams {
    pub status: Option<String>,
    pub user_id: Option<String>,
    /// 旧版分页参数，保留源码和线上 API 兼容。
    pub limit: Option<i32>,
    /// 旧版分页参数，保留源码和线上 API 兼容。
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

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
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
        if let Some(ref user_id) = self.user_id {
            params.push(format!("user_id={}", encode_query_value(user_id)));
        }
        if let Some(limit) = self.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = self.offset {
            params.push(format!("offset={offset}"));
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

/// 支付订单信息（Admin 视角，含 user_id）
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentOrderInfo {
    pub id: String,
    pub tenant_id: Option<String>,
    pub user_id: String,
    pub out_trade_no: String,
    pub trade_no: Option<String>,
    pub provider_trade_no: Option<String>,
    #[serde(default = "default_payment_method")]
    pub payment_method: String,
    /// 金额（字符串格式，如 "100.00"）
    pub amount: String,
    pub status: String,
    pub subject: Option<String>,
    pub created_at: String,
}

fn default_payment_method() -> String {
    "alipay".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentProviderStatus {
    pub code: String,
    pub display_name: String,
    pub enabled: bool,
    pub configured: bool,
    pub available: bool,
    pub status: String,
    pub scenes: Vec<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PaymentOrderPage {
    pub orders: Vec<PaymentOrderInfo>,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    #[serde(default)]
    pub total_pages: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_modern_pagination_builders_remain_available() {
        let legacy = PaymentQueryParams::new()
            .with_limit(20)
            .with_offset(40)
            .to_query_string();
        let modern = PaymentQueryParams::new()
            .with_page(3)
            .with_page_size(20)
            .to_query_string();

        assert_eq!(legacy, "limit=20&offset=40");
        assert_eq!(modern, "page=3&page_size=20");
    }
}
