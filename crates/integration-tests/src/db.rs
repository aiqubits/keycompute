//! 数据库集成测试公共辅助函数
//!
//! 提供测试数据库连接创建、数据清理等共享函数

use keycompute_db::{
    CreateTenantRequest, CreateUserRequest, PendingRegistration, Tenant,
    UpsertPendingRegistrationRequest, User, initialize_schema,
};
use keycompute_types::UserRole;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use std::time::Duration;
use uuid::Uuid;

// 一个集成测试可并行运行多个 case，但它们共享同一个测试数据库。
// schema 只需在进程内初始化一次，避免多个 case 同时执行 DDL。
// 历史 system 用户的清理也在这里一次性完成：所有 case 都会等待
// 初始化结束后才开始访问数据库，不会观察到清理过程的中间状态。
static SCHEMA_INITIALIZED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

pub async fn initialize_test_schema(db: &DatabaseConnection) -> Result<(), keycompute_db::DbError> {
    SCHEMA_INITIALIZED
        .get_or_try_init(|| async {
            initialize_schema(db).await?;
            // 清理已存在的 system 用户（`uq_users_single_system_role` 全局唯一索引要求）。
            // main.rs 的 initialize_default_admin 或历史测试可能遗留了 system 用户，
            // 导致后续测试无法创建自己的 system 用户。清理失败不致命：
            // 仅依赖 system 用户唯一性的测试会受影响。
            if let Err(error) = remove_leftover_system_users(db).await {
                eprintln!("warning: failed to clean leftover system users: {error}");
            }
            Ok(())
        })
        .await
        .map(|_| ())
}

/// 在单个事务内临时禁用保护触发器并删除遗留的 system 用户。
///
/// 必须在同一事务内完成禁用-删除-启用：PostgreSQL 的 DDL 是事务性的，
/// `ALTER TABLE` 持有的 ACCESS EXCLUSIVE 锁会阻塞其他会话直到提交，
/// 因此并发测试永远不会观察到“触发器被禁用”的窗口。若改用逐条
/// 自动提交的语句，其他并行用例（如角色提升/降级拒绝测试）会在
/// 禁用窗口内穿透保护导致 flaky 失败。
async fn remove_leftover_system_users(db: &DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    let tx = db.begin().await?;
    tx.execute_unprepared("ALTER TABLE users DISABLE TRIGGER trg_prevent_system_user_delete")
        .await?;
    tx.execute_unprepared("ALTER TABLE users DISABLE TRIGGER trg_prevent_system_role_change")
        .await?;
    tx.execute_unprepared("DELETE FROM users WHERE role = 'system'")
        .await?;
    tx.execute_unprepared("ALTER TABLE users ENABLE TRIGGER trg_prevent_system_role_change")
        .await?;
    tx.execute_unprepared("ALTER TABLE users ENABLE TRIGGER trg_prevent_system_user_delete")
        .await?;
    tx.commit().await
}

pub async fn create_test_pool() -> DatabaseConnection {
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://keycompute:change-me-strong-password@localhost:5432/keycompute".to_string()
    });

    use sea_orm::ConnectOptions;
    let mut opt = ConnectOptions::new(&database_url);
    opt.max_connections(20)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .idle_timeout(Duration::from_secs(300))
        .max_lifetime(Duration::from_secs(900));

    let db = Database::connect(opt)
        .await
        .expect("Failed to connect to database. Set DATABASE_URL environment variable.");

    // 测试环境可能复用数据库；schema 本身保持可重复初始化，业务数据由各测试清理。
    initialize_test_schema(&db)
        .await
        .expect("Failed to initialize database schema");

    db
}

/// 清理特定测试运行的数据
pub async fn cleanup_test_data(
    pool: &DatabaseConnection,
    run_id: &str,
) -> Result<(), sea_orm::DbErr> {
    let slug_pattern = format!("test-%-{}", run_id);
    let email_pattern = format!("%{}%", run_id);

    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM pending_registrations WHERE email LIKE $1",
        [email_pattern.into()],
    ))
    .await?;
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM distribution_records WHERE tenant_id IN (SELECT id FROM tenants WHERE slug LIKE $1)",
        [slug_pattern.clone().into()],
    )).await?;
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM usage_logs WHERE tenant_id IN (SELECT id FROM tenants WHERE slug LIKE $1)",
        [slug_pattern.clone().into()],
    ))
    .await?;
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM balance_transactions WHERE user_id IN (SELECT id FROM users WHERE tenant_id IN (SELECT id FROM tenants WHERE slug LIKE $1))",
        [slug_pattern.clone().into()],
    )).await?;
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM user_balances WHERE user_id IN (SELECT id FROM users WHERE tenant_id IN (SELECT id FROM tenants WHERE slug LIKE $1))",
        [slug_pattern.clone().into()],
    )).await?;
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM produce_ai_keys WHERE tenant_id IN (SELECT id FROM tenants WHERE slug LIKE $1)",
        [slug_pattern.clone().into()],
    )).await?;
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM users WHERE tenant_id IN (SELECT id FROM tenants WHERE slug LIKE $1)",
        [slug_pattern.clone().into()],
    ))
    .await?;
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM tenants WHERE slug LIKE $1",
        [slug_pattern.into()],
    ))
    .await?;

    Ok(())
}

/// 创建测试租户
pub async fn create_test_tenant(pool: &DatabaseConnection, suffix: &str, test_id: &str) -> Tenant {
    Tenant::create(
        pool,
        &CreateTenantRequest {
            name: format!("Test Tenant {}", suffix),
            slug: format!("test-tenant-{}-{}", suffix, test_id),
            description: Some(format!("Test tenant for {}", suffix)),
            default_rpm_limit: Some(100),
            default_tpm_limit: Some(50000),
        },
    )
    .await
    .expect("Failed to create test tenant")
}

/// 创建测试用户
pub async fn create_test_user(
    pool: &DatabaseConnection,
    tenant_id: Uuid,
    suffix: &str,
    test_id: &str,
) -> User {
    User::create(
        pool,
        &CreateUserRequest {
            tenant_id,
            email: format!("test-{}-{}@example.com", suffix, test_id),
            name: Some(format!("Test User {}", suffix)),
            role: Some(UserRole::User),
        },
    )
    .await
    .expect("Failed to create test user")
}

/// 创建测试中的待完成注册记录
pub async fn create_test_pending_registration(
    pool: &DatabaseConnection,
    req: UpsertPendingRegistrationRequest,
) -> PendingRegistration {
    let tx = pool.begin().await.expect("transaction should start");
    let pending = PendingRegistration::create_in_tx(&tx, &req)
        .await
        .expect("pending registration should be created");
    tx.commit().await.expect("transaction should commit");
    pending
}

/// 通过邮箱删除用户
pub async fn delete_user_by_email(
    pool: &DatabaseConnection,
    email: &str,
) -> Result<(), sea_orm::DbErr> {
    pool.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM users WHERE email = $1",
        [email.into()],
    ))
    .await?;
    Ok(())
}
