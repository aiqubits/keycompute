//! 余额冻结/解冻测试

use integration_tests::common::generate_test_id;
use integration_tests::db::{
    cleanup_test_data, create_test_pool, create_test_tenant, create_test_user,
};
use keycompute_billing::balance::BalanceService;
use keycompute_db::DbRouter;
use rust_decimal::Decimal;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Barrier;
    use tokio::task::JoinSet;

    /// 测试余额冻结：冻结后可用余额减少、冻结余额增加
    #[tokio::test]
    async fn test_balance_freeze_success() {
        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("balance freeze cleanup should succeed");

        let tenant = create_test_tenant(&pool, "bf-suc", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "bf-suc", &test_id).await;

        let balance_service = BalanceService::new(DbRouter::single(pool.clone()));

        // 确保余额记录存在，然后充值
        let _ = balance_service
            .get_or_create(tenant.id, user.id)
            .await
            .expect("get_or_create should succeed");

        let (initial, _) = balance_service
            .recharge(
                user.id,
                tenant.id,
                Decimal::from(100),
                None,
                Some("initial recharge for freeze test"),
            )
            .await
            .expect("recharge should succeed");
        assert_eq!(initial.available_balance, Decimal::from(100));
        assert_eq!(initial.frozen_balance, Decimal::ZERO);

        // 冻结 30 元
        let (after_freeze, tx) = balance_service
            .freeze(user.id, Decimal::from(30), Some("test freeze half"))
            .await
            .expect("freeze should succeed");

        assert_eq!(
            after_freeze.available_balance,
            Decimal::from(70),
            "available should be 70 after freezing 30"
        );
        assert_eq!(
            after_freeze.frozen_balance,
            Decimal::from(30),
            "frozen should be 30"
        );
        // freeze 交易记录金额为负数（从用户视角可用余额减少）
        assert_eq!(tx.amount, Decimal::from(-30));
        assert_eq!(tx.transaction_type, "freeze");
        assert_eq!(tx.description.as_deref(), Some("test freeze half"));
    }

    /// 测试余额冻结合并冻结：第二次冻结累加到 frozen_balance
    #[tokio::test]
    async fn test_balance_freeze_cumulative() {
        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("cumulative freeze cleanup should succeed");

        let tenant = create_test_tenant(&pool, "bf-cum", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "bf-cum", &test_id).await;

        let balance_service = BalanceService::new(DbRouter::single(pool.clone()));
        let _ = balance_service
            .get_or_create(tenant.id, user.id)
            .await
            .expect("get_or_create should succeed");

        let _ = balance_service
            .recharge(
                user.id,
                tenant.id,
                Decimal::from(100),
                None,
                Some("recharge for cumulative test"),
            )
            .await
            .expect("recharge should succeed");

        // 第一次冻结 20
        let (b1, _) = balance_service
            .freeze(user.id, Decimal::from(20), None)
            .await
            .expect("first freeze should succeed");
        assert_eq!(b1.available_balance, Decimal::from(80));
        assert_eq!(b1.frozen_balance, Decimal::from(20));

        // 第二次冻结 40（累计冻结 60）
        let (b2, _) = balance_service
            .freeze(user.id, Decimal::from(40), None)
            .await
            .expect("second freeze should succeed");
        assert_eq!(b2.available_balance, Decimal::from(40));
        assert_eq!(b2.frozen_balance, Decimal::from(60));
    }

    /// 测试余额冻结不足时返回错误
    #[tokio::test]
    async fn test_balance_freeze_insufficient() {
        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("insufficient freeze cleanup should succeed");

        let tenant = create_test_tenant(&pool, "bf-insuf", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "bf-insuf", &test_id).await;

        let balance_service = BalanceService::new(DbRouter::single(pool.clone()));
        let _ = balance_service
            .get_or_create(tenant.id, user.id)
            .await
            .expect("get_or_create should succeed");

        // 只充 10 元，尝试冻结 100 元应失败
        let _ = balance_service
            .recharge(user.id, tenant.id, Decimal::from(10), None, None)
            .await
            .expect("recharge should succeed");

        let err = balance_service
            .freeze(user.id, Decimal::from(100), None)
            .await
            .expect_err("freeze with insufficient balance should fail");

        let err_msg = err.to_string().to_lowercase();
        assert!(
            err_msg.contains("insufficient") || err_msg.contains("not enough"),
            "error should indicate insufficient balance, got: {}",
            err_msg
        );
    }

    /// 测试冻结余额为 0 时冻结应成功（边界情况）
    #[tokio::test]
    async fn test_balance_freeze_zero_amount() {
        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("zero amount freeze cleanup should succeed");

        let tenant = create_test_tenant(&pool, "bf-zero", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "bf-zero", &test_id).await;

        let balance_service = BalanceService::new(DbRouter::single(pool.clone()));
        let _ = balance_service
            .get_or_create(tenant.id, user.id)
            .await
            .expect("get_or_create should succeed");

        // 冻结 0 元：Decimal::ZERO，应被 DB 层拒绝（amount > 0 检查由调用方负责）
        // BalanceService::freeze 调用 UserBalance::freeze，后者检查 available_balance < amount
        // 0 < 0 为 false，所以不会报 insufficient，但 UPDATE 会执行 frozen_balance += 0
        // 这是允许的操作（幂等冻结 0 元）
        let (after, tx) = balance_service
            .freeze(user.id, Decimal::ZERO, Some("zero freeze"))
            .await
            .expect("freeze zero amount should succeed (no-op)");

        assert_eq!(after.available_balance, Decimal::ZERO);
        assert_eq!(after.frozen_balance, Decimal::ZERO);
        assert_eq!(tx.amount, Decimal::ZERO);
        assert_eq!(tx.transaction_type, "freeze");
    }

    /// 测试余额解冻：冻结部分后解冻，恢复可用余额
    #[tokio::test]
    async fn test_balance_unfreeze_success() {
        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("unfreeze cleanup should succeed");

        let tenant = create_test_tenant(&pool, "uf-suc", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "uf-suc", &test_id).await;

        let balance_service = BalanceService::new(DbRouter::single(pool.clone()));
        let _ = balance_service
            .get_or_create(tenant.id, user.id)
            .await
            .expect("get_or_create should succeed");

        let _ = balance_service
            .recharge(user.id, tenant.id, Decimal::from(100), None, None)
            .await
            .expect("recharge should succeed");

        // 冻结 50
        let (frozen, _) = balance_service
            .freeze(user.id, Decimal::from(50), None)
            .await
            .expect("freeze should succeed");
        assert_eq!(frozen.available_balance, Decimal::from(50));
        assert_eq!(frozen.frozen_balance, Decimal::from(50));

        // 解冻 20
        let (unfrozen, tx) = balance_service
            .unfreeze(user.id, Decimal::from(20), Some("partial unfreeze"))
            .await
            .expect("unfreeze should succeed");

        assert_eq!(
            unfrozen.available_balance,
            Decimal::from(70),
            "available should be 70 after unfreezing 20"
        );
        assert_eq!(
            unfrozen.frozen_balance,
            Decimal::from(30),
            "frozen should be 30 after unfreezing 20"
        );
        // unfreeze 交易记录金额为正数（可用余额增加）
        assert_eq!(tx.amount, Decimal::from(20));
        assert_eq!(tx.transaction_type, "unfreeze");
        assert_eq!(tx.description.as_deref(), Some("partial unfreeze"));
    }

    /// 测试解冻金额超过冻结余额时返回错误
    #[tokio::test]
    async fn test_balance_unfreeze_insufficient() {
        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("insufficient unfreeze cleanup should succeed");

        let tenant = create_test_tenant(&pool, "uf-insuf", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "uf-insuf", &test_id).await;

        let balance_service = BalanceService::new(DbRouter::single(pool.clone()));
        let _ = balance_service
            .get_or_create(tenant.id, user.id)
            .await
            .expect("get_or_create should succeed");

        let _ = balance_service
            .recharge(user.id, tenant.id, Decimal::from(50), None, None)
            .await
            .expect("recharge should succeed");

        // 冻结 10
        let _ = balance_service
            .freeze(user.id, Decimal::from(10), None)
            .await
            .expect("freeze should succeed");

        // 尝试解冻 100（超过已冻结的 10）
        let err = balance_service
            .unfreeze(user.id, Decimal::from(100), None)
            .await
            .expect_err("unfreeze with insufficient frozen balance should fail");

        let err_msg = err.to_string().to_lowercase();
        assert!(
            err_msg.contains("insufficient") || err_msg.contains("not enough"),
            "error should indicate insufficient frozen balance, got: {}",
            err_msg
        );
    }

    /// 测试 freeze/unfreeze 完整往返：冻结后解冻回原状态
    #[tokio::test]
    async fn test_balance_freeze_unfreeze_roundtrip() {
        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("roundtrip cleanup should succeed");

        let tenant = create_test_tenant(&pool, "bf-rt", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "bf-rt", &test_id).await;

        let balance_service = BalanceService::new(DbRouter::single(pool.clone()));
        let _ = balance_service
            .get_or_create(tenant.id, user.id)
            .await
            .expect("get_or_create should succeed");

        let (initial, _) = balance_service
            .recharge(user.id, tenant.id, Decimal::from(100), None, None)
            .await
            .expect("recharge should succeed");

        // 冻结 100
        let (frozen, _) = balance_service
            .freeze(user.id, Decimal::from(100), None)
            .await
            .expect("freeze all should succeed");
        assert_eq!(frozen.available_balance, Decimal::ZERO);
        assert_eq!(frozen.frozen_balance, Decimal::from(100));

        // 解冻 100
        let (unfrozen, _) = balance_service
            .unfreeze(user.id, Decimal::from(100), None)
            .await
            .expect("unfreeze all should succeed");

        // 解冻后状态应和初始状态一致
        assert_eq!(
            unfrozen.available_balance, initial.available_balance,
            "available balance should be restored after full unfreeze"
        );
        assert_eq!(
            unfrozen.frozen_balance,
            Decimal::ZERO,
            "frozen balance should be zero after full unfreeze"
        );
    }

    /// 并发首充必须串行化余额流水的 before/after，不能只保证最终余额。
    #[tokio::test]
    async fn concurrent_first_recharges_preserve_a_contiguous_audit_chain() {
        const RECHARGE_COUNT: usize = 8;

        let pool = create_test_pool().await;
        let test_id = generate_test_id();
        cleanup_test_data(&pool, &test_id)
            .await
            .expect("concurrent recharge cleanup should succeed");
        let tenant = create_test_tenant(&pool, "recharge-race", &test_id).await;
        let user = create_test_user(&pool, tenant.id, "recharge-race", &test_id).await;
        let barrier = Arc::new(Barrier::new(RECHARGE_COUNT));
        let mut tasks = JoinSet::new();

        for _ in 0..RECHARGE_COUNT {
            let service = BalanceService::new(DbRouter::single(pool.clone()));
            let barrier = Arc::clone(&barrier);
            tasks.spawn(async move {
                barrier.wait().await;
                service
                    .recharge(
                        user.id,
                        tenant.id,
                        Decimal::ONE,
                        None,
                        Some("concurrent first recharge"),
                    )
                    .await
                    .expect("concurrent first recharge should succeed")
            });
        }

        let mut transactions = Vec::with_capacity(RECHARGE_COUNT);
        while let Some(result) = tasks.join_next().await {
            let (_, transaction) = result.expect("recharge task should not panic");
            transactions.push(transaction);
        }
        transactions.sort_by_key(|transaction| transaction.balance_before);

        for (index, transaction) in transactions.iter().enumerate() {
            let expected_before = Decimal::from(index);
            assert_eq!(transaction.balance_before, expected_before);
            assert_eq!(transaction.balance_after, expected_before + Decimal::ONE);
        }
        let final_balance = keycompute_db::UserBalance::find_by_user(&pool, user.id)
            .await
            .expect("final balance query should succeed")
            .expect("final balance should exist");
        assert_eq!(
            final_balance.available_balance,
            Decimal::from(RECHARGE_COUNT)
        );
    }
}
