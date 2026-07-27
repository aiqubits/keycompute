//! 数据库模型模块
//
//! 包含所有表的 ORM 模型和 CRUD 操作

pub mod account;
pub mod api_key;
pub mod distribution_record;
pub mod node;
pub mod node_session;
pub mod node_task;
pub mod node_task_submission;
pub mod node_tip;
pub mod node_tip_withdrawal;
pub mod password_reset;
pub mod payment_order;
pub mod pending_registration;
pub mod pricing_model;
pub mod system_setting;
pub mod tenant;
pub mod tenant_distribution_rule;
pub mod usage_log;
pub mod user;
pub mod user_balance;
pub mod user_credential;
pub mod user_node_gateway_token;
pub mod user_referral;

// 重新导出常用模型
pub use account::{Account, CreateAccountRequest, UpdateAccountRequest};
pub use api_key::{CreateProduceAiKeyRequest, ProduceAiKey, ProduceAiKeyResponse};
pub use distribution_record::{
    CreateDistributionRecordRequest, DistributionLevelStats, DistributionRecord, DistributionStats,
};
pub use node::{
    CreateNodeRequest, NODE_STATUS_EXCLUDED, NODE_STATUS_OFFLINE, NODE_STATUS_ONLINE, Node,
};
pub use node_session::{CreateNodeSessionRequest, NodeSession};
pub use node_task::{
    CreateNodeTaskRequest, NodeTask, TASK_STATUS_EXPIRED, TASK_STATUS_FAILED, TASK_STATUS_LEASED,
    TASK_STATUS_QUEUED, TASK_STATUS_SUCCEEDED,
};
pub use node_task_submission::{CreateNodeTaskSubmissionRequest, NodeTaskSubmission};
pub use node_tip::{NodeTip, NodeTipSummary};
pub use node_tip_withdrawal::{
    ApproveWithdrawalRequest, CreateTipWithdrawalRequest, NodeTipWithdrawal,
    NodeTipWithdrawalWithUser, WITHDRAWAL_STATUS_APPROVED, WITHDRAWAL_STATUS_COMPLETED,
    WITHDRAWAL_STATUS_PENDING, WITHDRAWAL_STATUS_REJECTED, WITHDRAWAL_TYPE_ALIPAY,
    WITHDRAWAL_TYPE_BALANCE,
};
pub use password_reset::{CreatePasswordResetRequest, PasswordReset};
pub use payment_order::{
    CreatePaymentOrderRequest, CreditPaidOrderError, PaymentMethod, PaymentOrder,
    PaymentOrderStats, PaymentOrderStatus,
};
pub use pending_registration::{PendingRegistration, UpsertPendingRegistrationRequest};
pub use pricing_model::{CreatePricingRequest, PricingModel, UpdatePricingRequest};
pub use system_setting::{
    BatchUpdateSettingsRequest, PublicSettings, SettingValueType, SystemSetting,
    SystemSettingResponse, UpdateSystemSettingRequest,
};
pub use tenant::{CreateTenantRequest, Tenant, UpdateTenantRequest};
pub use tenant_distribution_rule::{
    CreateDistributionRuleRequest, TenantDistributionRule, UpdateDistributionRuleRequest,
};
pub use usage_log::{CreateUsageLogRequest, UsageLog, UsageStats, UserUsageStats};
pub use user::{CreateUserRequest, UpdateUserRequest, User};
pub use user_balance::{BalanceTransaction, TransactionType, UserBalance};
pub use user_credential::{
    CreateUserCredentialRequest, UpdateUserCredentialRequest, UserCredential,
};
pub use user_node_gateway_token::{
    PendingTokenWithUser, UserNodeGatewayToken, UserNodeGatewayTokenResponse,
};
pub use user_referral::{CreateUserReferralRequest, ReferralStats, UserReferral};
