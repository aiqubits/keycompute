#![allow(dead_code)]

use client_api::error::Result;
use client_api::{
    AdminApi,
    api::admin::{
        AccountInfo, AccountQueryParams, AccountRefreshResponse, AccountTestResponse,
        CreateAccountRequest, MessageResponse, UpdateAccountRequest,
    },
};

use super::api_client::get_client;

pub async fn list(params: Option<AccountQueryParams>, token: &str) -> Result<Vec<AccountInfo>> {
    let client = get_client();
    AdminApi::new(&client)
        .list_accounts(params.as_ref(), token)
        .await
}

pub async fn create(req: CreateAccountRequest, token: &str) -> Result<AccountInfo> {
    let client = get_client();
    AdminApi::new(&client).create_account(&req, token).await
}

pub async fn update(id: &str, req: UpdateAccountRequest, token: &str) -> Result<AccountInfo> {
    let client = get_client();
    AdminApi::new(&client).update_account(id, &req, token).await
}

pub async fn delete(id: &str, token: &str) -> Result<MessageResponse> {
    let client = get_client();
    AdminApi::new(&client).delete_account(id, token).await
}

pub async fn test(id: &str, token: &str) -> Result<AccountTestResponse> {
    let client = get_client();
    AdminApi::new(&client).test_account(id, token).await
}

pub async fn refresh(id: &str, token: &str) -> Result<AccountRefreshResponse> {
    let client = get_client();
    AdminApi::new(&client).refresh_account(id, token).await
}
