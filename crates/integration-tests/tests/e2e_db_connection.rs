//! 数据库连接测试

use integration_tests::common::VerificationChain;
use integration_tests::db::create_test_pool;
use keycompute_types::{
    AttemptStatus, AttemptTraceFinish, BillingStatus, ErrorOrigin, RequestLifecycleRecorder,
    RequestStatus, RequestTraceFinish, RequestTraceStart, RouteType, StreamEndReason,
    TraceErrorCategory, TraceErrorInfo,
};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
    TransactionTrait,
};
use uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_database_url() -> String {
        std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
        })
    }

    async fn create_isolated_schema() -> (DatabaseConnection, String) {
        let admin = Database::connect(test_database_url())
            .await
            .expect("migration test admin connection should succeed");
        let schema = format!("migration_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!(r#"CREATE SCHEMA "{schema}""#))
            .await
            .expect("isolated migration test schema should be created");
        (admin, schema)
    }

    async fn connect_to_schema(schema: &str) -> DatabaseConnection {
        let mut options = ConnectOptions::new(test_database_url());
        options
            .max_connections(1)
            .min_connections(1)
            .set_schema_search_path(schema);
        Database::connect(options)
            .await
            .expect("isolated migration test connection should succeed")
    }

    async fn drop_isolated_schema(admin: &DatabaseConnection, schema: &str) {
        admin
            .execute_unprepared(&format!(r#"DROP SCHEMA "{schema}" CASCADE"#))
            .await
            .expect("isolated migration test schema should be removed");
    }

    async fn insert_terminal_pending_trace(
        pool: &DatabaseConnection,
        request_id: Uuid,
        received_at: chrono::DateTime<chrono::Utc>,
        finished_at: chrono::DateTime<chrono::Utc>,
    ) {
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO gateway_requests (
                request_id,tenant_id,user_id,produce_ai_key_id,protocol,request_path,
                requested_model,is_stream,route_type,status,received_at,finished_at,
                billing_status,trace_quality
            ) VALUES ($1,$2,$3,$4,'openai','/v1/chat/completions','test-model',FALSE,
                      'provider_account','succeeded',$5,$6,'pending','actual')"#,
            [
                request_id.into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                received_at.into(),
                finished_at.into(),
            ],
        ))
        .await
        .expect("terminal pending trace should be inserted");
    }

    async fn insert_unfinished_node_trace(
        pool: &DatabaseConnection,
        request_id: Uuid,
        attempt_id: Uuid,
        task_id: Uuid,
        received_at: chrono::DateTime<chrono::Utc>,
    ) {
        let node_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let lease_id = Uuid::new_v4();
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO gateway_requests (
                request_id,tenant_id,user_id,produce_ai_key_id,protocol,request_path,
                requested_model,is_stream,route_type,status,received_at,billing_status,trace_quality
            ) VALUES ($1,$2,$3,$4,'openai','/v1/chat/completions','test-model',FALSE,
                      'node','running',$5,'pending','actual')"#,
            [
                request_id.into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                received_at.into(),
            ],
        ))
        .await
        .expect("unfinished node trace should be inserted");
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO gateway_request_attempts (
                id,request_id,attempt_no,attempt_kind,route_type,model,status,is_final,
                node_task_id,node_id,session_id,lease_id,started_at
            ) VALUES ($1,$2,1,'primary','node','test-model','running',FALSE,$3,$4,$5,$6,$7)"#,
            [
                attempt_id.into(),
                request_id.into(),
                task_id.into(),
                node_id.into(),
                session_id.into(),
                lease_id.into(),
                received_at.into(),
            ],
        ))
        .await
        .expect("running node attempt should be inserted");
    }

    async fn delete_gateway_trace(pool: &DatabaseConnection, request_id: Uuid) {
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM usage_logs WHERE request_id=$1",
            [request_id.into()],
        ))
        .await
        .expect("test usage should be removed");
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM gateway_requests WHERE request_id=$1",
            [request_id.into()],
        ))
        .await
        .expect("test trace should be removed");
    }

    #[tokio::test]
    async fn stale_account_probe_snapshot_does_not_overwrite_new_configuration() {
        let pool = create_test_pool().await;
        let account = keycompute_db::Account::create(
            &pool,
            &keycompute_db::CreateAccountRequest {
                tenant_id: Uuid::new_v4(),
                provider: "openai".to_string(),
                name: format!("probe-race-{}", Uuid::new_v4()),
                endpoint: "https://old.example/v1".to_string(),
                upstream_api_key_encrypted: "test-encrypted-key".to_string(),
                upstream_api_key_preview: "test****".to_string(),
                rpm_limit: Some(60),
                tpm_limit: Some(100_000),
                priority: Some(0),
                models_supported: vec!["test-model".to_string()],
                visibility: Some("tenant".to_string()),
            },
        )
        .await
        .expect("probe race account should be created");
        let new_config_version = account.updated_at + chrono::Duration::seconds(1);
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE accounts SET endpoint=$1,updated_at=$2 WHERE id=$3",
            [
                "https://new.example/v1".into(),
                new_config_version.into(),
                account.id.into(),
            ],
        ))
        .await
        .expect("account configuration should change while probe is in flight");

        let persisted = keycompute_db::Account::record_probe_snapshot_if_config_current(
            &pool,
            account.id,
            account.updated_at,
            chrono::Utc::now(),
            42,
            "failed",
            Some("upstream_http_401"),
        )
        .await
        .expect("stale probe write should be evaluated");
        assert!(!persisted, "stale probe result must be discarded");
        let current = keycompute_db::Account::find_by_id(&pool, account.id)
            .await
            .expect("account reload should succeed")
            .expect("account should still exist");
        assert_eq!(current.endpoint, "https://new.example/v1");
        assert_eq!(current.updated_at, new_config_version);
        assert!(current.last_probe_at.is_none());

        assert!(
            keycompute_db::Account::record_probe_snapshot_if_config_current(
                &pool,
                account.id,
                new_config_version,
                chrono::Utc::now(),
                7,
                "succeeded",
                None,
            )
            .await
            .expect("current probe write should succeed")
        );
        let current = keycompute_db::Account::find_by_id(&pool, account.id)
            .await
            .expect("account reload should succeed")
            .expect("account should still exist");
        assert_eq!(current.last_probe_status.as_deref(), Some("succeeded"));
        assert_eq!(current.updated_at, new_config_version);

        current
            .delete(&pool)
            .await
            .expect("probe race account should be removed");
    }

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

    /// 全新数据库只会应用统一基线。
    #[tokio::test]
    async fn test_consolidated_baseline_migration_record() {
        let pool = create_test_pool().await;
        let row = pool
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT version, name, COUNT(*) OVER () AS migration_count \
                 FROM schema_migrations LIMIT 1"
                    .to_string(),
            ))
            .await
            .expect("migration history query should succeed")
            .expect("migration history should exist");

        assert_eq!(row.try_get::<i64>("", "version").unwrap(), 1);
        assert_eq!(row.try_get::<String>("", "name").unwrap(), "baseline");
        assert_eq!(row.try_get::<i64>("", "migration_count").unwrap(), 1);
    }

    /// Two replicas starting against the same fresh database must serialize
    /// the baseline and converge on one migration-history row.
    #[tokio::test]
    async fn concurrent_migration_startup_applies_baseline_once() {
        let (admin, schema) = create_isolated_schema().await;
        let first = connect_to_schema(&schema).await;
        let second = connect_to_schema(&schema).await;

        let (first_result, second_result) = tokio::join!(
            keycompute_db::migrations::run_migrations(&first),
            keycompute_db::migrations::run_migrations(&second),
        );
        let history = first
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT COUNT(*) AS migration_count, MIN(version) AS version FROM schema_migrations"
                    .to_string(),
            ))
            .await;

        drop(first);
        drop(second);
        drop_isolated_schema(&admin, &schema).await;

        first_result.expect("first migration runner should succeed");
        second_result.expect("concurrent migration runner should succeed");
        let history = history
            .expect("migration history query should succeed")
            .expect("migration history should exist");
        assert_eq!(history.try_get::<i64>("", "migration_count").unwrap(), 1);
        assert_eq!(history.try_get::<i64>("", "version").unwrap(), 1);
    }

    /// Migration history is an integrity boundary: editing an already-applied
    /// migration must fail closed instead of silently accepting schema drift.
    #[tokio::test]
    async fn migration_checksum_mismatch_is_rejected() {
        let (admin, schema) = create_isolated_schema().await;
        let pool = connect_to_schema(&schema).await;
        keycompute_db::migrations::run_migrations(&pool)
            .await
            .expect("baseline migration should succeed before tampering");
        pool.execute_unprepared("UPDATE schema_migrations SET checksum='tampered'")
            .await
            .expect("migration checksum should be tampered for the test");

        let result = keycompute_db::migrations::run_migrations(&pool).await;

        drop(pool);
        drop_isolated_schema(&admin, &schema).await;

        let error = result.expect_err("a checksum mismatch must reject startup");
        assert!(
            error.to_string().contains("checksum mismatch"),
            "unexpected migration error: {error}"
        );
    }

    /// The consolidated baseline only supports fresh deployments. A legacy
    /// application table without migration history must remain untouched and
    /// must not receive a misleading schema_migrations table.
    #[tokio::test]
    async fn nonempty_database_without_history_is_rejected_atomically() {
        let (admin, schema) = create_isolated_schema().await;
        let pool = connect_to_schema(&schema).await;
        pool.execute_unprepared("CREATE TABLE legacy_application_data (id BIGINT PRIMARY KEY)")
            .await
            .expect("legacy application table should be created");

        let result = keycompute_db::migrations::run_migrations(&pool).await;
        let history_table = pool
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT 1 AS present FROM information_schema.tables \
                 WHERE table_schema=current_schema() AND table_name='schema_migrations'"
                    .to_string(),
            ))
            .await;
        let legacy_table = pool
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT 1 AS present FROM information_schema.tables \
                 WHERE table_schema=current_schema() AND table_name='legacy_application_data'"
                    .to_string(),
            ))
            .await;

        drop(pool);
        drop_isolated_schema(&admin, &schema).await;

        let error = result.expect_err("a non-empty database without history must be rejected");
        assert!(
            error
                .to_string()
                .contains("non-empty but has no migration history"),
            "unexpected migration error: {error}"
        );
        assert!(
            history_table
                .expect("migration-history existence query should succeed")
                .is_none(),
            "failed initialization must roll back schema_migrations creation"
        );
        assert!(
            legacy_table
                .expect("legacy-table existence query should succeed")
                .is_some(),
            "failed initialization must not modify existing application tables"
        );
    }

    /// 哨兵列校验：完整 schema 上应通过，结构缺失时应拒绝启动。
    #[tokio::test]
    async fn test_schema_sentinel_verification() {
        let pool = create_test_pool().await;

        // 当前测试库已应用完整 schema，哨兵校验必须通过
        keycompute_db::verify_schema_sentinels(&pool)
            .await
            .expect("sentinel verification must pass on an up-to-date schema");

        // 验证一个不存在的列必须失败并指名缺失项。
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

    #[tokio::test]
    #[serial_test::serial]
    async fn stale_terminal_billing_reconciliation_resolves_pending_status() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let failed_request_id = Uuid::new_v4();
        let succeeded_request_id = Uuid::new_v4();
        let finished_at = chrono::Utc::now() - chrono::Duration::hours(2);
        let received_at = finished_at - chrono::Duration::seconds(1);

        insert_terminal_pending_trace(&pool, failed_request_id, received_at, finished_at).await;
        insert_terminal_pending_trace(&pool, succeeded_request_id, received_at, finished_at).await;
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO usage_logs (
                request_id,tenant_id,user_id,produce_ai_key_id,model_name,provider_name,
                account_id,input_tokens,output_tokens,total_tokens,input_unit_price_snapshot,
                output_unit_price_snapshot,user_amount,currency,usage_source,status,
                started_at,finished_at
            ) VALUES ($1,$2,$3,$4,'test-model','openai',$5,1,1,2,0,0,0,'CNY',
                      'provider_reported','success',$6,$7)"#,
            [
                succeeded_request_id.into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                received_at.into(),
                finished_at.into(),
            ],
        ))
        .await
        .expect("committed usage should be inserted");

        keycompute_db::reconcile_stale_requests(router.as_ref(), 60, 200)
            .await
            .expect("stale billing reconciliation should succeed");

        for (request_id, expected) in [
            (failed_request_id, "failed"),
            (succeeded_request_id, "succeeded"),
        ] {
            let row = pool
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT billing_status FROM gateway_requests WHERE request_id=$1",
                    [request_id.into()],
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                row.try_get::<String>("", "billing_status").unwrap(),
                expected
            );
        }

        delete_gateway_trace(&pool, failed_request_id).await;
        delete_gateway_trace(&pool, succeeded_request_id).await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn stale_request_reconciliation_uses_last_lifecycle_activity() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let request_id = Uuid::new_v4();
        let now = chrono::Utc::now();
        let received_at = now - chrono::Duration::minutes(2);

        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO gateway_requests (
                request_id,tenant_id,user_id,produce_ai_key_id,protocol,request_path,
                requested_model,is_stream,route_type,status,received_at,updated_at,
                billing_status,trace_quality
            ) VALUES ($1,$2,$3,$4,'openai','/v1/chat/completions','test-model',FALSE,
                      'provider_account','running',$5,$6,'pending','actual')"#,
            [
                request_id.into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                received_at.into(),
                now.into(),
            ],
        ))
        .await
        .expect("active old request trace should be inserted");

        keycompute_db::reconcile_stale_requests(router.as_ref(), 60, 200)
            .await
            .expect("fresh lifecycle activity should be evaluated");
        let active = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.try_get::<String>("", "status").unwrap(), "running");
        assert!(
            active
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_none()
        );

        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE gateway_requests SET updated_at=$1 WHERE request_id=$2",
            [received_at.into(), request_id.into()],
        ))
        .await
        .expect("request trace should be made stale");
        keycompute_db::reconcile_stale_requests(router.as_ref(), 60, 200)
            .await
            .expect("inactive request should reconcile");
        let stale = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale.try_get::<String>("", "status").unwrap(), "timed_out");
        assert!(
            stale
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    async fn repeated_terminal_completion_is_idempotent() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let recorder = keycompute_db::PostgresRequestLifecycleRecorder::new(router);
        let request_id = Uuid::new_v4();
        let received_at = chrono::Utc::now();
        recorder
            .start_request(RequestTraceStart {
                request_id,
                client_request_id: None,
                tenant_id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
                produce_ai_key_id: Uuid::new_v4(),
                protocol: "openai".to_string(),
                request_path: "/v1/chat/completions".to_string(),
                requested_model: "test-model".to_string(),
                is_stream: false,
                received_at,
            })
            .await
            .expect("request trace should start");
        recorder
            .set_route(
                request_id,
                RouteType::ProviderAccount,
                RequestStatus::Routing,
            )
            .await
            .expect("request route should be recorded");
        let finish = RequestTraceFinish {
            request_id,
            status: RequestStatus::Succeeded,
            error: None,
            billing_status: BillingStatus::Pending,
            finished_at: chrono::Utc::now(),
        };

        recorder
            .finish_request_without_attempt(finish.clone())
            .await
            .expect("the first terminal completion should succeed");
        recorder
            .finish_request_without_attempt(finish)
            .await
            .expect("the repeated terminal completion should be idempotent");

        let request = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            request.try_get::<String>("", "status").unwrap(),
            "succeeded"
        );
        assert!(
            request
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    async fn non_final_attempt_completion_restores_the_request_status() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let request_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let received_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        insert_unfinished_node_trace(&pool, request_id, attempt_id, task_id, received_at).await;
        let recorder = keycompute_db::PostgresRequestLifecycleRecorder::new(router);

        recorder
            .finish_attempt_and_request(AttemptTraceFinish {
                attempt_id,
                request_id,
                attempt_status: AttemptStatus::Failed,
                request_status: RequestStatus::Queued,
                is_final: false,
                stream_end_reason: Some(StreamEndReason::UpstreamError),
                stream_error_count: Some(1),
                error: Some(TraceErrorInfo {
                    origin: ErrorOrigin::Node,
                    category: TraceErrorCategory::NodeFailed,
                    code: "node_requeued".to_string(),
                    summary: None,
                    retryable: Some(true),
                }),
                billing_status: BillingStatus::Pending,
                finished_at: chrono::Utc::now(),
            })
            .await
            .expect("the lifecycle fallback should complete the attempt");

        let request = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.try_get::<String>("", "status").unwrap(), "queued");
        assert!(
            request
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_none()
        );

        let attempt = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,is_final,finished_at FROM gateway_request_attempts WHERE id=$1",
                [attempt_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(attempt.try_get::<String>("", "status").unwrap(), "failed");
        assert!(!attempt.try_get::<bool>("", "is_final").unwrap());
        assert!(
            attempt
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    async fn final_attempt_can_complete_before_client_response_terminalizes_request() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let request_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let received_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        insert_unfinished_node_trace(&pool, request_id, attempt_id, task_id, received_at).await;
        let recorder = keycompute_db::PostgresRequestLifecycleRecorder::new(router);

        recorder
            .finish_attempt_and_request(AttemptTraceFinish {
                attempt_id,
                request_id,
                attempt_status: AttemptStatus::Succeeded,
                request_status: RequestStatus::Running,
                is_final: true,
                stream_end_reason: Some(StreamEndReason::Completed),
                stream_error_count: Some(0),
                error: None,
                billing_status: BillingStatus::Pending,
                finished_at: chrono::Utc::now(),
            })
            .await
            .expect("the final attempt should complete before the client response");

        let attempt = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,is_final,finished_at FROM gateway_request_attempts WHERE id=$1",
                [attempt_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            attempt.try_get::<String>("", "status").unwrap(),
            "succeeded"
        );
        assert!(attempt.try_get::<bool>("", "is_final").unwrap());
        assert!(
            attempt
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        let request = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.try_get::<String>("", "status").unwrap(), "running");
        assert!(
            request
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_none()
        );

        recorder
            .finish_request_without_attempt(RequestTraceFinish {
                request_id,
                status: RequestStatus::Succeeded,
                error: None,
                billing_status: BillingStatus::Pending,
                finished_at: chrono::Utc::now(),
            })
            .await
            .expect("the client response should terminalize the request independently");

        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    async fn failed_attempt_can_complete_before_client_disconnect_terminalizes_request() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let request_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let received_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        insert_unfinished_node_trace(&pool, request_id, attempt_id, task_id, received_at).await;
        let recorder = keycompute_db::PostgresRequestLifecycleRecorder::new(router);

        recorder
            .finish_attempt_and_request(AttemptTraceFinish {
                attempt_id,
                request_id,
                attempt_status: AttemptStatus::Failed,
                request_status: RequestStatus::Running,
                is_final: true,
                stream_end_reason: Some(StreamEndReason::UpstreamError),
                stream_error_count: Some(1),
                error: Some(TraceErrorInfo {
                    origin: ErrorOrigin::Node,
                    category: TraceErrorCategory::NodeFailed,
                    code: "node_failed".to_string(),
                    summary: None,
                    retryable: Some(true),
                }),
                billing_status: BillingStatus::Pending,
                finished_at: chrono::Utc::now(),
            })
            .await
            .expect("the failed Node attempt should close independently");

        let unfinished = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            unfinished.try_get::<String>("", "status").unwrap(),
            "running"
        );
        assert!(
            unfinished
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_none()
        );

        recorder
            .finish_request_without_attempt(keycompute_types::client_response_trace_finish(
                request_id,
                keycompute_types::ClientResponseOutcome::ClientDisconnected,
            ))
            .await
            .expect("the handler should persist the client disconnect");

        let request = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,error_category,error_code,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            request.try_get::<String>("", "status").unwrap(),
            "cancelled"
        );
        assert_eq!(
            request.try_get::<String>("", "error_category").unwrap(),
            "client_disconnect"
        );
        assert_eq!(
            request.try_get::<String>("", "error_code").unwrap(),
            "client_disconnected"
        );
        assert!(
            request
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        let attempt = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status FROM gateway_request_attempts WHERE id=$1",
                [attempt_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(attempt.try_get::<String>("", "status").unwrap(), "failed");

        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    async fn client_response_can_terminalize_before_final_attempt_is_persisted() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let request_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let received_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        insert_unfinished_node_trace(&pool, request_id, attempt_id, task_id, received_at).await;
        let recorder = keycompute_db::PostgresRequestLifecycleRecorder::new(router);

        recorder
            .finish_request_without_attempt(RequestTraceFinish {
                request_id,
                status: RequestStatus::Succeeded,
                error: None,
                billing_status: BillingStatus::Pending,
                finished_at: chrono::Utc::now(),
            })
            .await
            .expect("the handler should terminalize the delivered response");
        recorder
            .finish_attempt_and_request(AttemptTraceFinish {
                attempt_id,
                request_id,
                attempt_status: AttemptStatus::Succeeded,
                request_status: RequestStatus::Running,
                is_final: true,
                stream_end_reason: Some(StreamEndReason::Completed),
                stream_error_count: Some(0),
                error: None,
                billing_status: BillingStatus::Pending,
                finished_at: chrono::Utc::now(),
            })
            .await
            .expect("a late attempt write must preserve the handler-owned request outcome");

        let request = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            request.try_get::<String>("", "status").unwrap(),
            "succeeded"
        );
        assert!(
            request
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        let attempt = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,is_final,finished_at FROM gateway_request_attempts WHERE id=$1",
                [attempt_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            attempt.try_get::<String>("", "status").unwrap(),
            "succeeded"
        );
        assert!(attempt.try_get::<bool>("", "is_final").unwrap());
        assert!(
            attempt
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn stale_image_success_without_client_response_is_reconciled_as_failed() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let request_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let now = chrono::Utc::now();
        insert_unfinished_node_trace(
            &pool,
            request_id,
            attempt_id,
            task_id,
            now - chrono::Duration::minutes(2),
        )
        .await;
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE gateway_requests SET updated_at=$1 WHERE request_id=$2",
            [
                (now - chrono::Duration::minutes(2)).into(),
                request_id.into(),
            ],
        ))
        .await
        .expect("unfinished image trace should be made stale");
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO node_tasks (
                id,request_id,user_id,model,payload_json,status,result_json,queued_at,
                finished_at,deadline_at,complete_grace_until
            ) VALUES ($1,$2,$3,'test-model','{}','image_succeeded','{}',$4,$5,$6,$7)"#,
            [
                task_id.into(),
                request_id.into(),
                Uuid::new_v4().into(),
                (now - chrono::Duration::minutes(2)).into(),
                (now - chrono::Duration::minutes(1)).into(),
                (now + chrono::Duration::minutes(1)).into(),
                (now + chrono::Duration::minutes(2)).into(),
            ],
        ))
        .await
        .expect("completed image task should be inserted");

        let reconciled = keycompute_db::reconcile_stale_requests(router.as_ref(), 60, 200)
            .await
            .expect("stale image completion should reconcile");
        assert!(reconciled >= 1);

        let request = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,trace_quality,billing_status,error_origin,error_category,error_code,finished_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.try_get::<String>("", "status").unwrap(), "failed");
        assert_eq!(
            request.try_get::<String>("", "trace_quality").unwrap(),
            "partial"
        );
        assert_eq!(
            request.try_get::<String>("", "billing_status").unwrap(),
            "not_applicable"
        );
        assert_eq!(
            request.try_get::<String>("", "error_origin").unwrap(),
            "gateway"
        );
        assert_eq!(
            request.try_get::<String>("", "error_category").unwrap(),
            "internal"
        );
        assert_eq!(
            request.try_get::<String>("", "error_code").unwrap(),
            "node_client_response_missing"
        );
        assert!(
            request
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "finished_at")
                .unwrap()
                .is_some()
        );

        let attempt = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,is_final,stream_end_reason FROM gateway_request_attempts WHERE id=$1",
                [attempt_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            attempt.try_get::<String>("", "status").unwrap(),
            "succeeded"
        );
        assert!(attempt.try_get::<bool>("", "is_final").unwrap());
        assert_eq!(
            attempt.try_get::<String>("", "stream_end_reason").unwrap(),
            "completed"
        );

        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM node_tasks WHERE id=$1",
            [task_id.into()],
        ))
        .await
        .expect("image task should be removed");
        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    async fn final_intermediate_flush_marks_partial_and_clears_failure_state() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let request_id = Uuid::new_v4();
        let received_at = chrono::Utc::now();
        insert_terminal_pending_trace(&pool, request_id, received_at, received_at).await;
        let recorder = keycompute_db::PostgresRequestLifecycleRecorder::new(router);

        // An impossible client timestamp deterministically violates the schema
        // check and exercises the worker failure path after terminalization.
        recorder
            .record_client_first_content(request_id, received_at - chrono::Duration::seconds(1))
            .await
            .expect("the asynchronous update should enqueue");
        recorder
            .flush_intermediate_updates(request_id)
            .await
            .expect_err("the failed intermediate write must reach the barrier");

        let row = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT trace_quality,client_first_content_at FROM gateway_requests WHERE request_id=$1",
                [request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.try_get::<String>("", "trace_quality").unwrap(),
            "partial"
        );
        assert!(
            row.try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "client_first_content_at")
                .unwrap()
                .is_none()
        );

        // The first barrier consumes the per-request failure marker. A second
        // empty barrier must therefore complete successfully rather than
        // reporting the old failure forever.
        recorder
            .flush_intermediate_updates(request_id)
            .await
            .expect("failure state should be cleared after the first barrier");

        delete_gateway_trace(&pool, request_id).await;
    }

    #[tokio::test]
    async fn unrelated_intermediate_writes_do_not_block_request_flush() {
        let pool = create_test_pool().await;
        let router = keycompute_db::DbRouter::single(pool.clone());
        let recorder = keycompute_db::PostgresRequestLifecycleRecorder::new(router);
        let received_at = chrono::Utc::now();
        let blocked_request_ids = (0..4).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
        let healthy_request_id = Uuid::new_v4();

        for request_id in blocked_request_ids
            .iter()
            .copied()
            .chain(std::iter::once(healthy_request_id))
        {
            recorder
                .start_request(RequestTraceStart {
                    request_id,
                    client_request_id: None,
                    tenant_id: Uuid::new_v4(),
                    user_id: Uuid::new_v4(),
                    produce_ai_key_id: Uuid::new_v4(),
                    protocol: "openai".to_string(),
                    request_path: "/v1/chat/completions".to_string(),
                    requested_model: "test-model".to_string(),
                    is_stream: true,
                    received_at,
                })
                .await
                .expect("request trace should start");
        }

        // Hold unrelated request rows long enough that a single global worker
        // would spend four 250 ms write timeouts ahead of the healthy barrier.
        let lock_tx = pool
            .begin()
            .await
            .expect("row-lock transaction should start");
        for request_id in &blocked_request_ids {
            lock_tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT request_id FROM gateway_requests WHERE request_id=$1 FOR UPDATE",
                    [(*request_id).into()],
                ))
                .await
                .expect("blocked request row should lock")
                .expect("blocked request trace should exist");
            recorder
                .record_client_first_content(*request_id, chrono::Utc::now())
                .await
                .expect("blocked intermediate update should enqueue");
        }

        let healthy_first_content_at = chrono::Utc::now();
        recorder
            .record_client_first_content(healthy_request_id, healthy_first_content_at)
            .await
            .expect("healthy intermediate update should enqueue");
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            recorder.flush_intermediate_updates(healthy_request_id),
        )
        .await
        .expect("unrelated row locks must not delay the healthy request barrier")
        .expect("healthy intermediate update should flush");

        let healthy = pool
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT trace_quality,client_first_content_at FROM gateway_requests WHERE request_id=$1",
                [healthy_request_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            healthy.try_get::<String>("", "trace_quality").unwrap(),
            "actual"
        );
        assert!(
            healthy
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "client_first_content_at")
                .unwrap()
                .is_some()
        );

        lock_tx
            .rollback()
            .await
            .expect("row-lock transaction should roll back");
        for request_id in &blocked_request_ids {
            let _ = recorder.flush_intermediate_updates(*request_id).await;
        }
        for request_id in blocked_request_ids
            .into_iter()
            .chain(std::iter::once(healthy_request_id))
        {
            delete_gateway_trace(&pool, request_id).await;
        }
    }
}
