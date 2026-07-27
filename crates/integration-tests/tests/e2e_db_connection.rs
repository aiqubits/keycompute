//! 数据库连接测试

use integration_tests::common::VerificationChain;
use integration_tests::db::create_test_pool;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试数据库连接
    #[tokio::test]
    async fn test_database_connection() {
        let mut chain = VerificationChain::new();

        // 1. 连接数据库
        let pool = create_test_pool().await;
        chain.add_step(
            "keycompute-db",
            "create_test_pool",
            "Database connection established",
            true,
        );

        // 2. 测试简单查询
        let result = pool
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT 1".to_string(),
            ))
            .await;
        let passed = result.is_ok();
        chain.add_step("keycompute-db", "SELECT 1", "Simple query executed", passed);

        // 3. 验证表存在（实际检查 COUNT(*) 值）
        let result = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'tenants'",
                [],
            ))
            .await;
        let table_exists = result
            .ok()
            .flatten()
            .and_then(|r| r.try_get_by_index::<i64>(0).ok())
            .map(|count| count > 0)
            .unwrap_or(false);
        chain.add_step(
            "keycompute-db",
            "check_tenants_table",
            "Tenants table exists",
            table_exists,
        );

        chain.print_report();
        assert!(chain.all_passed(), "Database connection tests failed");
    }

    /// 测试数据库管理器
    #[tokio::test]
    async fn test_database_manager() {
        let mut chain = VerificationChain::new();

        // 直接使用 sea_orm ConnectOptions 创建连接池
        use sea_orm::ConnectOptions;
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
        });
        let mut opt = ConnectOptions::new(&database_url);
        opt.max_connections(5);
        let pool = Database::connect(opt).await;

        chain.add_step(
            "keycompute-db",
            "ConnectOptions::connect",
            "Database pool created",
            pool.is_ok(),
        );

        let pool = pool.expect("Failed to create database pool");

        // 测试连接
        let test_result = pool
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT 1".to_string(),
            ))
            .await;
        chain.add_step(
            "keycompute-db",
            "test_connection",
            "Connection test passed",
            test_result.is_ok(),
        );

        chain.print_report();
        assert!(chain.all_passed());
    }

    /// 哨兵列校验：完整 schema 上应通过，缺列（模拟存量旧库）应拒绝启动
    #[tokio::test]
    async fn test_schema_sentinel_verification() {
        let pool = create_test_pool().await;

        // 当前测试库已应用完整 schema，哨兵校验必须通过
        keycompute_db::verify_schema_sentinels(&pool)
            .await
            .expect("sentinel verification must pass on an up-to-date schema");

        // 模拟旧库缺列：验证一个不存在的列必须失败并指名缺失项
        let error = keycompute_db::verify_required_columns(
            &pool,
            &[
                ("payment_orders", "payment_scene"),
                ("payment_orders", "column_only_in_future_schema"),
            ],
        )
        .await
        .expect_err("a missing sentinel column must fail verification");
        let message = error.to_string();
        assert!(
            message.contains("payment_orders.column_only_in_future_schema"),
            "error should name the missing column, got: {message}"
        );
        assert!(
            !message.contains("payment_orders.payment_scene,"),
            "columns that exist must not be reported as missing, got: {message}"
        );
    }
}
