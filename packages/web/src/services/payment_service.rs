#![allow(dead_code)]

use client_api::error::Result;
use client_api::{
    PaymentApi,
    api::payment::{
        CreatePaymentOrderRequest, CreatePaymentOrderResponse, PaymentMethodsResponse,
        PaymentOrderPage, PaymentOrderResponse, PaymentOrderSummary, PaymentQueryParams,
        SyncPaymentOrderResponse, UserBalanceResponse,
    },
};

use super::api_client::get_client;

pub async fn get_balance(token: &str) -> Result<UserBalanceResponse> {
    let client = get_client();
    PaymentApi::new(&client).get_my_balance(token).await
}

pub async fn list_orders_page(
    params: Option<PaymentQueryParams>,
    token: &str,
) -> Result<PaymentOrderPage> {
    let client = get_client();
    PaymentApi::new(&client)
        .list_my_payment_orders_page(params.as_ref(), token)
        .await
}

pub async fn get_methods(token: &str) -> Result<PaymentMethodsResponse> {
    let client = get_client();
    PaymentApi::new(&client).get_payment_methods(token).await
}

pub async fn list_orders(
    params: Option<PaymentQueryParams>,
    token: &str,
) -> Result<Vec<PaymentOrderSummary>> {
    let client = get_client();
    PaymentApi::new(&client)
        .list_my_payment_orders(params.as_ref(), token)
        .await
}

pub async fn get_order(id: &str, token: &str) -> Result<PaymentOrderResponse> {
    let client = get_client();
    PaymentApi::new(&client).get_payment_order(id, token).await
}

pub async fn create_order(
    req: CreatePaymentOrderRequest,
    token: &str,
) -> Result<CreatePaymentOrderResponse> {
    let client = get_client();
    PaymentApi::new(&client)
        .create_payment_order(&req, token)
        .await
}

pub async fn sync_order(order_id: &str, token: &str) -> Result<SyncPaymentOrderResponse> {
    let client = get_client();
    PaymentApi::new(&client)
        .sync_payment_order(order_id, token)
        .await
}
