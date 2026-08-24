//! 管理功能模块
//!
//! 拆分为子模块：user / account / pricing / payment

mod account;
mod monitoring;
mod node_gateway;
mod payment;
mod pricing;
mod user;

pub use super::common::MessageResponse;
use crate::client::ApiClient;
use crate::error::Result;

// Re-export 各子模块的公共类型
pub use account::{
    AccountInfo, AccountQueryParams, AccountRefreshResponse, AccountTestResponse,
    CreateAccountRequest, UpdateAccountRequest,
};
pub use monitoring::{
    MonitoringAttemptDetail, MonitoringNodeHealth, MonitoringOverviewResponse,
    MonitoringProbeRequest, MonitoringQuery, MonitoringRequestDetail, MonitoringRequestItem,
    MonitoringRequestPage, MonitoringSummary, MonitoringSummaryResponse,
    MonitoringTargetHealthResponse, MonitoringTraceEntry, MonitoringTraceSummary,
};
pub use node_gateway::{
    ApproveTokenRequest, DeleteNodeResponse, ExcludeNodeResponse, NodeGatewayNodeInfo,
    NodeGatewayNodeStats, NodeGatewayOverviewResponse, NodeGatewayTaskInfo, NodeGatewayTaskStats,
    PendingTokenWithUser, RecoverNodeResponse, RevokeNodeRequest, RevokeNodeTokenResponse,
};
pub use payment::{PaymentOrderInfo, PaymentOrderPage, PaymentProviderStatus};
pub use pricing::{
    CalculateCostRequest, CostCalculationResponse, CreatePricingRequest, CreatePricingResponse,
    MakeDefaultPricingResponse, PricingInfo, SetDefaultPricingRequest, UpdatePricingRequest,
    UpdatePricingResponse,
};
pub use user::{
    ApiKeyInfo, UpdateBalanceRequest, UpdateBalanceResponse, UpdateUserRequest, UpdateUserResponse,
    UserDetail, UserListResponse, UserQueryParams,
};

// Re-export admin payment's PaymentQueryParams distinctly
pub use payment::PaymentQueryParams;

/// 管理 API 客户端
#[derive(Debug, Clone)]
pub struct AdminApi {
    client: ApiClient,
}

impl AdminApi {
    /// 创建新的管理 API 客户端
    pub fn new(client: &ApiClient) -> Self {
        Self {
            client: client.clone(),
        }
    }

    // ==================== 用户管理 ====================

    /// 获取所有用户列表
    pub async fn list_all_users(
        &self,
        params: Option<&UserQueryParams>,
        token: &str,
    ) -> Result<UserListResponse> {
        let path = if let Some(p) = params {
            format!("/api/v1/users?{}", p.to_query_string())
        } else {
            "/api/v1/users".to_string()
        };
        self.client.get_json(&path, Some(token)).await
    }

    /// 获取指定用户详情
    pub async fn get_user_by_id(&self, id: &str, token: &str) -> Result<UserDetail> {
        self.client
            .get_json(&format!("/api/v1/users/{}", id), Some(token))
            .await
    }

    /// 更新用户信息
    pub async fn update_user(
        &self,
        id: &str,
        req: &UpdateUserRequest,
        token: &str,
    ) -> Result<UpdateUserResponse> {
        self.client
            .put_json(&format!("/api/v1/users/{}", id), req, Some(token))
            .await
    }

    /// 删除用户
    pub async fn delete_user(&self, id: &str, token: &str) -> Result<MessageResponse> {
        self.client
            .delete_json(&format!("/api/v1/users/{}", id), Some(token))
            .await
    }

    /// 更新用户余额（充值/扣除）
    pub async fn update_user_balance(
        &self,
        id: &str,
        req: &UpdateBalanceRequest,
        token: &str,
    ) -> Result<UpdateBalanceResponse> {
        self.client
            .post_json(&format!("/api/v1/users/{}/balance", id), req, Some(token))
            .await
    }

    /// 冻结用户余额
    pub async fn freeze_user_balance(
        &self,
        id: &str,
        req: &UpdateBalanceRequest,
        token: &str,
    ) -> Result<UpdateBalanceResponse> {
        self.client
            .post_json(
                &format!("/api/v1/users/{}/balance/freeze", id),
                req,
                Some(token),
            )
            .await
    }

    /// 解冻用户余额
    pub async fn unfreeze_user_balance(
        &self,
        id: &str,
        req: &UpdateBalanceRequest,
        token: &str,
    ) -> Result<UpdateBalanceResponse> {
        self.client
            .post_json(
                &format!("/api/v1/users/{}/balance/unfreeze", id),
                req,
                Some(token),
            )
            .await
    }

    /// 获取用户的 API Keys
    pub async fn list_user_api_keys(&self, id: &str, token: &str) -> Result<Vec<ApiKeyInfo>> {
        self.client
            .get_json(&format!("/api/v1/users/{}/api-keys", id), Some(token))
            .await
    }

    // ==================== 账号/渠道管理 ====================

    /// 获取账号列表
    pub async fn list_accounts(
        &self,
        params: Option<&AccountQueryParams>,
        token: &str,
    ) -> Result<Vec<AccountInfo>> {
        let path = if let Some(p) = params {
            format!("/api/v1/accounts?{}", p.to_query_string())
        } else {
            "/api/v1/accounts".to_string()
        };
        self.client.get_json(&path, Some(token)).await
    }

    /// 创建账号
    pub async fn create_account(
        &self,
        req: &CreateAccountRequest,
        token: &str,
    ) -> Result<AccountInfo> {
        self.client
            .post_json("/api/v1/accounts", req, Some(token))
            .await
    }

    /// 更新账号
    pub async fn update_account(
        &self,
        id: &str,
        req: &UpdateAccountRequest,
        token: &str,
    ) -> Result<AccountInfo> {
        self.client
            .put_json(&format!("/api/v1/accounts/{}", id), req, Some(token))
            .await
    }

    /// 删除账号
    pub async fn delete_account(&self, id: &str, token: &str) -> Result<MessageResponse> {
        self.client
            .delete_json(&format!("/api/v1/accounts/{}", id), Some(token))
            .await
    }

    /// 测试账号
    pub async fn test_account(&self, id: &str, token: &str) -> Result<AccountTestResponse> {
        self.client
            .post_json(
                &format!("/api/v1/accounts/{}/test", id),
                &serde_json::json!({}),
                Some(token),
            )
            .await
    }

    /// 刷新账号
    pub async fn refresh_account(&self, id: &str, token: &str) -> Result<AccountRefreshResponse> {
        self.client
            .post_json(
                &format!("/api/v1/accounts/{}/refresh", id),
                &serde_json::json!({}),
                Some(token),
            )
            .await
    }

    // ==================== Node Gateway 管理 ====================

    pub async fn node_gateway_overview(&self, token: &str) -> Result<NodeGatewayOverviewResponse> {
        self.client
            .get_json("/api/v1/admin/node-gateway/overview", Some(token))
            .await
    }

    /// 获取待审批的注册令牌列表
    pub async fn list_pending_tokens(&self, token: &str) -> Result<Vec<PendingTokenWithUser>> {
        self.client
            .get_json("/api/v1/admin/node-gateway/tokens/pending", Some(token))
            .await
    }

    /// 审批/拒绝注册令牌
    pub async fn approve_token(
        &self,
        token_id: &str,
        req: &ApproveTokenRequest,
        auth_token: &str,
    ) -> Result<serde_json::Value> {
        self.client
            .post_json(
                &format!("/api/v1/admin/node-gateway/tokens/{}/approve", token_id),
                req,
                Some(auth_token),
            )
            .await
    }

    /// 排除节点（从节点池中移除）
    pub async fn exclude_node(&self, node_id: &str, token: &str) -> Result<ExcludeNodeResponse> {
        self.client
            .post_json(
                &format!("/api/v1/admin/nodes/{}/exclude", node_id),
                &(),
                Some(token),
            )
            .await
    }

    /// 恢复被排除的节点
    pub async fn recover_node(&self, node_id: &str, token: &str) -> Result<RecoverNodeResponse> {
        self.client
            .post_json(
                &format!("/api/v1/admin/nodes/{}/recover", node_id),
                &(),
                Some(token),
            )
            .await
    }

    /// 吊销节点注册令牌（排除节点 + 将对应 token 标记为 rejected 并记录原因）
    pub async fn revoke_node_token(
        &self,
        node_id: &str,
        reason: &str,
        token: &str,
    ) -> Result<RevokeNodeTokenResponse> {
        let req = RevokeNodeRequest {
            reason: reason.to_string(),
        };
        self.client
            .post_json(
                &format!("/api/v1/admin/nodes/{}/revoke-token", node_id),
                &req,
                Some(token),
            )
            .await
    }

    /// 删除节点（彻底删除节点数据，清除关联 token）
    pub async fn delete_node(&self, node_id: &str, token: &str) -> Result<DeleteNodeResponse> {
        self.client
            .delete_json(&format!("/api/v1/admin/nodes/{}", node_id), Some(token))
            .await
    }

    pub async fn monitoring_overview(&self, token: &str) -> Result<MonitoringOverviewResponse> {
        self.client
            .get_json("/api/v1/admin/monitoring/overview", Some(token))
            .await
    }

    pub async fn monitoring_requests(
        &self,
        params: &MonitoringQuery,
        token: &str,
    ) -> Result<MonitoringRequestPage> {
        let query = params.to_query_string();
        let path = if query.is_empty() {
            "/api/v1/admin/monitoring/requests".to_string()
        } else {
            format!("/api/v1/admin/monitoring/requests?{query}")
        };
        self.client.get_json(&path, Some(token)).await
    }

    pub async fn monitoring_request(
        &self,
        request_id: &str,
        token: &str,
    ) -> Result<MonitoringRequestDetail> {
        self.client
            .get_json(
                &format!("/api/v1/admin/monitoring/requests/{request_id}"),
                Some(token),
            )
            .await
    }

    pub async fn monitoring_summary(
        &self,
        params: &MonitoringQuery,
        token: &str,
    ) -> Result<MonitoringSummaryResponse> {
        let query = params.to_query_string();
        let path = if query.is_empty() {
            "/api/v1/admin/monitoring/summary".to_string()
        } else {
            format!("/api/v1/admin/monitoring/summary?{query}")
        };
        self.client.get_json(&path, Some(token)).await
    }

    pub async fn monitoring_target_health(
        &self,
        params: &MonitoringQuery,
        token: &str,
    ) -> Result<MonitoringTargetHealthResponse> {
        let query = params.to_query_string();
        let path = if query.is_empty() {
            "/api/v1/admin/monitoring/targets/health".to_string()
        } else {
            format!("/api/v1/admin/monitoring/targets/health?{query}")
        };
        self.client.get_json(&path, Some(token)).await
    }

    pub async fn probe_monitoring_targets(
        &self,
        account_ids: Option<Vec<String>>,
        token: &str,
    ) -> Result<serde_json::Value> {
        self.client
            .post_json(
                "/api/v1/admin/monitoring/targets/probe",
                &MonitoringProbeRequest { account_ids },
                Some(token),
            )
            .await
    }

    // ==================== 定价管理 ====================

    /// 获取定价列表
    pub async fn list_pricing(&self, token: &str) -> Result<Vec<PricingInfo>> {
        self.client.get_json("/api/v1/pricing", Some(token)).await
    }

    /// 创建定价
    pub async fn create_pricing(
        &self,
        req: &CreatePricingRequest,
        token: &str,
    ) -> Result<CreatePricingResponse> {
        self.client
            .post_json("/api/v1/pricing", req, Some(token))
            .await
    }

    /// 更新定价
    pub async fn update_pricing(
        &self,
        id: &str,
        req: &UpdatePricingRequest,
        token: &str,
    ) -> Result<UpdatePricingResponse> {
        self.client
            .put_json(&format!("/api/v1/pricing/{}", id), req, Some(token))
            .await
    }

    /// 删除定价
    pub async fn delete_pricing(&self, id: &str, token: &str) -> Result<MessageResponse> {
        self.client
            .delete_json(&format!("/api/v1/pricing/{}", id), Some(token))
            .await
    }

    /// 将定价设为默认
    pub async fn make_pricing_default(
        &self,
        id: &str,
        token: &str,
    ) -> Result<MakeDefaultPricingResponse> {
        self.client
            .post_json(
                &format!("/api/v1/pricing/{}/make-default", id),
                &(),
                Some(token),
            )
            .await
    }

    /// 设置默认定价
    pub async fn set_default_pricing(
        &self,
        req: &SetDefaultPricingRequest,
        token: &str,
    ) -> Result<MessageResponse> {
        self.client
            .post_json("/api/v1/pricing/batch-defaults", req, Some(token))
            .await
    }

    /// 计算费用
    pub async fn calculate_cost(
        &self,
        req: &CalculateCostRequest,
        token: &str,
    ) -> Result<CostCalculationResponse> {
        self.client
            .post_json("/api/v1/pricing/calculate", req, Some(token))
            .await
    }

    // ==================== 支付管理 ====================

    /// 获取所有支付订单（Admin）
    pub async fn list_all_payment_orders(
        &self,
        params: Option<&PaymentQueryParams>,
        token: &str,
    ) -> Result<Vec<PaymentOrderInfo>> {
        let path = if let Some(p) = params {
            format!("/api/v1/admin/payments/orders?{}", p.to_query_string())
        } else {
            "/api/v1/admin/payments/orders".to_string()
        };
        let resp: PaymentOrderPage = self.client.get_json(&path, Some(token)).await?;
        Ok(resp.orders)
    }

    pub async fn list_payment_orders_page(
        &self,
        params: Option<&PaymentQueryParams>,
        token: &str,
    ) -> Result<PaymentOrderPage> {
        let path = if let Some(params) = params {
            format!("/api/v1/admin/payments/orders?{}", params.to_query_string())
        } else {
            "/api/v1/admin/payments/orders".to_string()
        };
        self.client.get_json(&path, Some(token)).await
    }

    /// 获取支付渠道的开关、配置和实际可用状态。
    pub async fn get_payment_providers(&self, token: &str) -> Result<Vec<PaymentProviderStatus>> {
        self.client
            .get_json("/api/v1/admin/payments/providers", Some(token))
            .await
    }

    /// 创建并关闭一笔真实小额订单，验证当前支付渠道配置。
    pub async fn verify_payment_provider(
        &self,
        method: &str,
        token: &str,
    ) -> Result<MessageResponse> {
        self.client
            .post_json(
                &format!("/api/v1/admin/payments/providers/{method}/verify"),
                &serde_json::json!({}),
                Some(token),
            )
            .await
    }
}
