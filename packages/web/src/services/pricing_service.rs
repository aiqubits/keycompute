#![allow(dead_code)]

use client_api::error::Result;
use client_api::{
    AdminApi,
    api::admin::{
        CreatePricingRequest, CreatePricingResponse, MakeDefaultPricingResponse, MessageResponse,
        PricingInfo, SetDefaultPricingRequest, UpdatePricingRequest, UpdatePricingResponse,
    },
};

use super::api_client::get_client;

/// 全局默认定价使用的 nil UUID。
pub const GLOBAL_DEFAULT_TENANT_ID: &str = "00000000-0000-0000-0000-000000000000";

/// 判断价格行是否属于所有租户共享的全局默认定价。
pub fn is_global_default(tenant_id: &Option<String>) -> bool {
    tenant_id
        .as_deref()
        .is_some_and(|id| id == GLOBAL_DEFAULT_TENANT_ID)
}

pub async fn list(token: &str) -> Result<Vec<PricingInfo>> {
    let client = get_client();
    AdminApi::new(&client).list_pricing(token).await
}

pub async fn create(req: CreatePricingRequest, token: &str) -> Result<CreatePricingResponse> {
    let client = get_client();
    AdminApi::new(&client).create_pricing(&req, token).await
}

pub async fn update(
    id: &str,
    req: UpdatePricingRequest,
    token: &str,
) -> Result<UpdatePricingResponse> {
    let client = get_client();
    AdminApi::new(&client).update_pricing(id, &req, token).await
}

pub async fn delete(id: &str, token: &str) -> Result<MessageResponse> {
    let client = get_client();
    AdminApi::new(&client).delete_pricing(id, token).await
}

pub async fn make_default(id: &str, token: &str) -> Result<MakeDefaultPricingResponse> {
    let client = get_client();
    AdminApi::new(&client).make_pricing_default(id, token).await
}

pub async fn set_defaults(req: SetDefaultPricingRequest, token: &str) -> Result<MessageResponse> {
    let client = get_client();
    AdminApi::new(&client)
        .set_default_pricing(&req, token)
        .await
}

#[cfg(test)]
mod tests {
    use super::{GLOBAL_DEFAULT_TENANT_ID, is_global_default};

    #[test]
    fn only_nil_tenant_is_the_global_default_scope() {
        assert!(is_global_default(&Some(
            GLOBAL_DEFAULT_TENANT_ID.to_string()
        )));
        assert!(!is_global_default(&None));
        assert!(!is_global_default(&Some(
            "11111111-1111-1111-1111-111111111111".to_string()
        )));
    }
}
