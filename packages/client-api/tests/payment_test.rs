//! 支付模块集成测试

use client_api::api::payment::{CreatePaymentOrderRequest, PaymentApi, PaymentQueryParams};
use client_api::error::ClientError;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, ResponseTemplate};

mod common;
use common::{create_test_client, fixtures};

#[tokio::test]
#[allow(deprecated)]
async fn test_create_payment_order_success() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    let expected_body = serde_json::json!({
        "amount": "10",
        "subject": "账户充值",
        "body": null,
        "payment_type": "page"
    });

    Mock::given(method("POST"))
        .and(path("/api/v1/payments/orders"))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "order_id": "order_001",
            "out_trade_no": "PAY202401200001",
            "payment_type": "page",
            "pay_url": "https://alipay.com/pay?order=xxx",
            "qr_code": null,
            "qr_code_image_url": null,
            "expired_at": "2024-01-20T11:00:00Z"
        })))
        .mount(&mock_server)
        .await;

    let req = CreatePaymentOrderRequest::new(10.0, "账户充值", "page");
    let result = payment_api
        .create_payment_order(&req, fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let order = result.unwrap();
    assert_eq!(order.out_trade_no, "PAY202401200001");
    assert_eq!(order.payment_type, "page");
}

#[tokio::test]
#[allow(deprecated)]
async fn test_create_payment_order_does_not_round_subcent_amount() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("POST"))
        .and(path("/api/v1/payments/orders"))
        .and(body_json(serde_json::json!({
            "amount": "1.005",
            "subject": "账户充值",
            "body": null,
            "payment_type": "page"
        })))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "Invalid amount"
        })))
        .mount(&mock_server)
        .await;

    let request = CreatePaymentOrderRequest::new(1.005, "账户充值", "page");
    let result = payment_api
        .create_payment_order(&request, fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_create_payment_order_accepts_dual_scene_compatibility_fields() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("POST"))
        .and(path("/api/v1/payments/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "order_id": "order_dual_fields",
            "out_trade_no": "PAY_DUAL_FIELDS",
            "payment_method": "wechatpay",
            "payment_scene": "native",
            "payment_type": "native",
            "pay_url": null,
            "qr_code": "weixin://wxpay/example",
            "qr_code_image_url": null,
            "expired_at": "2026-07-26T11:00:00Z"
        })))
        .mount(&mock_server)
        .await;

    let req = CreatePaymentOrderRequest::new_for_method("10", "wechatpay", "native");
    let order = payment_api
        .create_payment_order(&req, fixtures::TEST_ACCESS_TOKEN)
        .await
        .unwrap();

    assert_eq!(order.payment_method, "wechatpay");
    assert_eq!(order.payment_type, "native");
}

#[tokio::test]
async fn test_create_payment_order_rejects_conflicting_scene_compatibility_fields() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("POST"))
        .and(path("/api/v1/payments/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "order_id": "order_conflicting_fields",
            "out_trade_no": "PAY_CONFLICTING_FIELDS",
            "payment_method": "wechatpay",
            "payment_scene": "native",
            "payment_type": "page",
            "pay_url": null,
            "qr_code": "weixin://wxpay/example",
            "qr_code_image_url": null,
            "expired_at": "2026-07-26T11:00:00Z"
        })))
        .mount(&mock_server)
        .await;

    let req = CreatePaymentOrderRequest::new_for_method("10", "wechatpay", "native");
    let error = payment_api
        .create_payment_order(&req, fixtures::TEST_ACCESS_TOKEN)
        .await
        .unwrap_err();

    assert!(matches!(error, ClientError::Http(_)));
}

#[tokio::test]
#[allow(deprecated)]
async fn test_create_payment_order_with_description() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    let expected_body = serde_json::json!({
        "amount": "50",
        "subject": "账户充值",
        "body": "Account recharge",
        "payment_type": "qr"
    });

    Mock::given(method("POST"))
        .and(path("/api/v1/payments/orders"))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "order_id": "order_002",
            "out_trade_no": "PAY202401200002",
            "payment_type": "qr",
            "pay_url": null,
            "qr_code": "qr-content",
            "qr_code_image_url": "https://example.com/qr.png",
            "expired_at": "2024-01-20T11:00:00Z"
        })))
        .mount(&mock_server)
        .await;

    let req = CreatePaymentOrderRequest::new(50.0, "账户充值", "qr").with_body("Account recharge");
    let result = payment_api
        .create_payment_order(&req, fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let order = result.unwrap();
    assert_eq!(order.qr_code, Some("qr-content".to_string()));
}

#[tokio::test]
#[allow(deprecated)]
async fn test_create_payment_order_invalid_amount() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("POST"))
        .and(path("/api/v1/payments/orders"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "Invalid amount"
        })))
        .mount(&mock_server)
        .await;

    let req = CreatePaymentOrderRequest::new(-5.0, "账户充值", "page");
    let result = payment_api
        .create_payment_order(&req, fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_my_payment_orders_success() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/payments/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "orders": [
                {
                    "id": "order_001",
                    "out_trade_no": "PAY202401200001",
                    "amount": "10.0",
                    "status": "paid",
                    "subject": "Account Recharge",
                    "created_at": "2024-01-20T10:00:00Z",
                    "expired_at": "2024-01-20T11:00:00Z"
                },
                {
                    "id": "order_002",
                    "out_trade_no": "PAY202401190001",
                    "amount": "50.0",
                    "status": "pending",
                    "subject": "Account Recharge",
                    "created_at": "2024-01-19T10:00:00Z",
                    "expired_at": "2024-01-19T11:00:00Z"
                }
            ],
            "total": 2
        })))
        .mount(&mock_server)
        .await;

    let result = payment_api
        .list_my_payment_orders(None, fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let orders = result.unwrap();
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].status, "paid");
    assert_eq!(orders[1].status, "pending");
}

#[tokio::test]
async fn test_list_my_payment_orders_with_status_filter() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/payments/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "orders": [],
            "total": 0
        })))
        .mount(&mock_server)
        .await;

    let params = PaymentQueryParams::new().with_status("paid").with_limit(5);

    let result = payment_api
        .list_my_payment_orders(Some(&params), fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_get_payment_order_success() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/payments/orders/order_001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "order_001",
            "out_trade_no": "PAY202401200001",
            "amount": "10",
            "status": "paid",
            "subject": "账户充值",
            "body": null,
            "payment_method": "alipay",
            "pay_url": null,
            "expired_at": "2024-01-20T11:00:00Z",
            "paid_at": "2024-01-20T10:05:00Z",
            "created_at": "2024-01-20T10:00:00Z"
        })))
        .mount(&mock_server)
        .await;

    let result = payment_api
        .get_payment_order("order_001", fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let order = result.unwrap();
    assert_eq!(order.id, "order_001");
    assert_eq!(order.status, "paid");
}

#[tokio::test]
async fn test_get_payment_order_not_found() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/payments/orders/nonexistent"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "error": "Order not found"
        })))
        .mount(&mock_server)
        .await;

    let result = payment_api
        .get_payment_order("nonexistent", fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(matches!(result.unwrap_err(), ClientError::NotFound(_)));
}

#[tokio::test]
async fn test_sync_payment_order_success() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("POST"))
        .and(path("/api/v1/payments/orders/order_001/sync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "order_id": "order_001",
            "out_trade_no": "PAY202401200001",
            "status": "paid",
            "changed": true
        })))
        .mount(&mock_server)
        .await;

    let result = payment_api
        .sync_payment_order("order_001", fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let order = result.unwrap();
    assert_eq!(order.order_id, "order_001");
    assert_eq!(order.status, "paid");
    assert!(order.changed);
}

#[tokio::test]
async fn test_get_my_balance_success() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/payments/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": fixtures::TEST_USER_ID,
            "available_balance": "49.8766",
            "frozen_balance": "0.0",
            "total_balance": "49.8766",
            "total_recharged": "100.0",
            "total_consumed": "50.1234"
        })))
        .mount(&mock_server)
        .await;

    let result = payment_api
        .get_my_balance(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    let balance = result.unwrap();
    assert_eq!(balance.available_balance, "49.8766");
    assert_eq!(balance.frozen_balance, "0.0");
}

#[tokio::test]
async fn test_get_my_balance_zero() {
    let (client, mock_server) = create_test_client().await;
    let payment_api = PaymentApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/api/v1/payments/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": fixtures::TEST_USER_ID,
            "available_balance": "0.0",
            "frozen_balance": "0.0",
            "total_balance": "0.0",
            "total_recharged": "0.0",
            "total_consumed": "0.0"
        })))
        .mount(&mock_server)
        .await;

    let result = payment_api
        .get_my_balance(fixtures::TEST_ACCESS_TOKEN)
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().available_balance, "0.0");
}
