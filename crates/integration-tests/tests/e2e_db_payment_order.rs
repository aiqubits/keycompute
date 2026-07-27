//! Payment order state-machine and database constraint integration tests.

use chrono::{Duration, Utc};
use integration_tests::common::generate_test_id;
use integration_tests::db::{
    cleanup_test_data, create_test_pool, create_test_tenant, create_test_user,
};
use keycompute_db::{
    CreatePaymentOrderRequest, CreditPaidOrderError, DbError, PaymentMethod, PaymentOrder,
    UserBalance, purge_expired_payment_security_events,
};
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};

#[derive(FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(FromQueryResult)]
struct NotificationStatusRow {
    processing_status: String,
}

#[tokio::test]
async fn failed_transition_rejects_an_existing_paid_order() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("payment order cleanup should succeed");
    let tenant = create_test_tenant(&pool, "payment-state", &test_id).await;
    let user = create_test_user(&pool, tenant.id, "payment-state", &test_id).await;
    let expired_at = Utc::now() + Duration::minutes(30);
    let out_trade_no = format!("TESTPAY{}", test_id.replace('-', ""));
    let order = PaymentOrder::create(
        &pool,
        &CreatePaymentOrderRequest {
            tenant_id: tenant.id,
            user_id: user.id,
            amount: Decimal::ONE,
            subject: "state transition test".to_string(),
            body: None,
            payment_method: PaymentMethod::WechatPay,
            payment_scene: "native".to_string(),
            expired_at,
        },
        &out_trade_no,
        "",
    )
    .await
    .expect("payment order should be created");
    assert!(
        (order.expired_at - expired_at)
            .num_microseconds()
            .is_some_and(|difference| difference.abs() <= 1),
        "database expiration should preserve the provider deadline within PostgreSQL precision"
    );

    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE payment_orders SET status='paid', provider_trade_no=$1, paid_at=NOW() WHERE id=$2",
        ["provider-trade-paid".into(), order.id.into()],
    ))
    .await
    .expect("test should transition the order to paid");

    let error = PaymentOrder::mark_as_failed(&pool, order.id)
        .await
        .expect_err("a paid order must not be reported as newly failed");
    assert!(matches!(
        error,
        DbError::InvalidOrderStatus { expected, actual }
            if expected == "pending" && actual == "paid"
    ));

    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM payment_orders WHERE id=$1",
        [order.id.into()],
    ))
    .await
    .expect("test payment order should be removed");
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("payment state test cleanup should succeed");
}

#[tokio::test]
async fn provider_circuit_constraint_rejects_unknown_states() {
    let pool = create_test_pool().await;
    let transaction = pool.begin().await.expect("transaction should start");
    let error = transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE payment_provider_states SET circuit_state=$1 WHERE payment_method='alipay'",
            ["unexpected".into()],
        ))
        .await
        .expect_err("unknown provider state should violate the database constraint");
    assert!(
        error
            .to_string()
            .contains("chk_payment_provider_states_circuit")
    );
    transaction
        .rollback()
        .await
        .expect("failed constraint transaction should roll back");
}

#[tokio::test]
async fn paid_order_credit_is_atomic_audited_and_idempotent() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("payment credit cleanup should succeed");
    let tenant = create_test_tenant(&pool, "payment-credit", &test_id).await;
    let user = create_test_user(&pool, tenant.id, "payment-credit", &test_id).await;
    let out_trade_no = format!("TESTCREDIT{}", test_id.replace('-', ""));
    let provider_trade_no = format!("TRADE{}", test_id.replace('-', ""));
    let event_id = format!("EVENT{}", test_id.replace('-', ""));
    let order = PaymentOrder::create(
        &pool,
        &CreatePaymentOrderRequest {
            tenant_id: tenant.id,
            user_id: user.id,
            amount: Decimal::new(1234, 2),
            subject: "credit transaction test".to_string(),
            body: None,
            payment_method: PaymentMethod::Alipay,
            payment_scene: "page".to_string(),
            expired_at: Utc::now() + Duration::minutes(30),
        },
        &out_trade_no,
        "",
    )
    .await
    .expect("payment order should be created");
    let payload = serde_json::json!({"trade_no": provider_trade_no, "status": "success"});

    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let first = PaymentOrder::credit_paid(
        &pool_a,
        order.id,
        &provider_trade_no,
        &event_id,
        payload.clone(),
        "test recharge",
    );
    let second = PaymentOrder::credit_paid(
        &pool_b,
        order.id,
        &provider_trade_no,
        &event_id,
        payload,
        "test recharge",
    );
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first concurrent provider event should succeed");
    let second = second.expect("second concurrent provider event should be idempotent");
    assert_ne!(first, second, "exactly one concurrent call should credit");

    let balance = UserBalance::find_by_user(&pool, user.id)
        .await
        .expect("balance query should succeed")
        .expect("credited balance should exist");
    assert_eq!(balance.available_balance, Decimal::new(1234, 2));
    let transaction_count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::BIGINT AS count FROM balance_transactions WHERE order_id=$1",
        [order.id.into()],
    ))
    .one(&pool)
    .await
    .expect("transaction count query should succeed")
    .expect("transaction count should return a row");
    assert_eq!(transaction_count.count, 1);
    let notification = NotificationStatusRow::find_by_statement(
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT processing_status FROM payment_notifications WHERE payment_method='alipay' AND provider_event_id=$1",
            [event_id.as_str().into()],
        ),
    )
    .one(&pool)
    .await
    .expect("notification query should succeed")
    .expect("notification should be recorded");
    assert_eq!(notification.processing_status, "processed");

    let conflict = PaymentOrder::credit_paid(
        &pool,
        order.id,
        &provider_trade_no,
        &event_id,
        serde_json::json!({"trade_no": provider_trade_no, "status": "tampered"}),
        "test recharge",
    )
    .await
    .expect_err("an event id must not be reusable with a different payload");
    assert!(matches!(
        conflict,
        CreditPaidOrderError::NotificationConflict
    ));

    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM payment_notifications WHERE provider_event_id=$1",
        [event_id.into()],
    ))
    .await
    .expect("test notification should be removed");
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("payment credit test cleanup should succeed");
}

#[tokio::test]
async fn paid_order_replay_paths_distinguish_provider_identity() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("payment replay cleanup should succeed");
    let tenant = create_test_tenant(&pool, "payment-replay", &test_id).await;
    let user = create_test_user(&pool, tenant.id, "payment-replay", &test_id).await;
    let out_trade_no = format!("TESTREPLAY{}", test_id.replace('-', ""));
    let provider_trade_no = format!("TRADEA{}", test_id.replace('-', ""));
    let first_event = format!("EVENTA{}", test_id.replace('-', ""));
    let second_event = format!("EVENTB{}", test_id.replace('-', ""));
    let third_event = format!("EVENTC{}", test_id.replace('-', ""));
    let order = PaymentOrder::create(
        &pool,
        &CreatePaymentOrderRequest {
            tenant_id: tenant.id,
            user_id: user.id,
            amount: Decimal::new(500, 2),
            subject: "replay identity test".to_string(),
            body: None,
            payment_method: PaymentMethod::WechatPay,
            payment_scene: "native".to_string(),
            expired_at: Utc::now() + Duration::minutes(30),
        },
        &out_trade_no,
        "",
    )
    .await
    .expect("payment order should be created");

    let credited = PaymentOrder::credit_paid(
        &pool,
        order.id,
        &provider_trade_no,
        &first_event,
        serde_json::json!({"trade_no": provider_trade_no, "status": "success"}),
        "replay test recharge",
    )
    .await
    .expect("first provider event should credit the order");
    assert!(credited, "first event must perform the actual credit");

    // 同一渠道交易号、不同事件 ID 的重复成功回调：必须走幂等路径，不得二次入账
    let replay = PaymentOrder::credit_paid(
        &pool,
        order.id,
        &provider_trade_no,
        &second_event,
        serde_json::json!({"trade_no": provider_trade_no, "status": "success", "retry": true}),
        "replay test recharge",
    )
    .await
    .expect("replay with the same provider trade no should be idempotent");
    assert!(!replay, "replay must not credit the balance again");

    // 同一订单携带不同渠道交易号：必须拒为 ProviderIdentityMismatch
    let mismatch = PaymentOrder::credit_paid(
        &pool,
        order.id,
        "TRADEB-different",
        &third_event,
        serde_json::json!({"trade_no": "TRADEB-different", "status": "success"}),
        "replay test recharge",
    )
    .await
    .expect_err("a different provider trade no must not confirm a paid order");
    assert!(matches!(
        mismatch,
        CreditPaidOrderError::ProviderIdentityMismatch
    ));

    let balance = UserBalance::find_by_user(&pool, user.id)
        .await
        .expect("balance query should succeed")
        .expect("credited balance should exist");
    assert_eq!(
        balance.available_balance,
        Decimal::new(500, 2),
        "balance must reflect exactly one credit across all replay paths"
    );
    let transaction_count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::BIGINT AS count FROM balance_transactions WHERE order_id=$1",
        [order.id.into()],
    ))
    .one(&pool)
    .await
    .expect("transaction count query should succeed")
    .expect("transaction count should return a row");
    assert_eq!(transaction_count.count, 1);

    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM payment_notifications WHERE provider_event_id IN ($1, $2, $3)",
        [first_event.into(), second_event.into(), third_event.into()],
    ))
    .await
    .expect("test notifications should be removed");
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("payment replay test cleanup should succeed");
}

#[tokio::test]
async fn security_event_retention_only_removes_events_past_the_window() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    let stale_digest = format!("stale{}", test_id.replace('-', ""));
    let fresh_digest = format!("fresh{}", test_id.replace('-', ""));

    // 插入 91 天前（应被清理）与 89 天前（应保留）的事件各一条
    for (digest, days) in [(&stale_digest, 91), (&fresh_digest, 89)] {
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO payment_security_events(payment_method, event_type, payload_digest, detail, source_ip, created_at)\
             VALUES ('alipay', 'retention_test', $1, 'retention window test', '203.0.113.1', NOW() - make_interval(days => $2::int))",
            [digest.as_str().into(), days.into()],
        ))
        .await
        .expect("test security event should be inserted");
    }

    let removed = purge_expired_payment_security_events(&pool, 90)
        .await
        .expect("retention purge should succeed");
    assert!(removed >= 1, "at least the stale event must be removed");

    let remaining = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::BIGINT AS count FROM payment_security_events WHERE payload_digest IN ($1, $2)",
        [stale_digest.as_str().into(), fresh_digest.as_str().into()],
    ))
    .one(&pool)
    .await
    .expect("remaining events query should succeed")
    .expect("remaining events query should return a row");
    assert_eq!(
        remaining.count, 1,
        "exactly the 89-day event must survive the 90-day retention window"
    );
    let survivor = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::BIGINT AS count FROM payment_security_events WHERE payload_digest = $1",
        [fresh_digest.as_str().into()],
    ))
    .one(&pool)
    .await
    .expect("survivor query should succeed")
    .expect("survivor query should return a row");
    assert_eq!(survivor.count, 1, "the fresh event must be the survivor");

    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM payment_security_events WHERE payload_digest IN ($1, $2)",
        [stale_digest.into(), fresh_digest.into()],
    ))
    .await
    .expect("test security events should be removed");
}
