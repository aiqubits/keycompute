//! KeyCompute 数据库访问层
//!
//! 提供 PostgreSQL 数据库连接池、ORM 模型和新库结构初始化

pub mod db_router;
pub mod models;
pub mod schema;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database as SeaDatabase, DatabaseConnection,
    DatabaseTransaction, DbBackend, Statement, TransactionTrait,
};
use std::sync::Arc;
use std::time::Duration;

pub use db_router::DbRouter;
pub use models::*;
pub use schema::*;

/// 当前版本的完整数据库结构；只描述最终状态，不承载升级步骤。
const DATABASE_SCHEMA: &str = include_str!("schema.sql");

// ============================================================================
// 错误类型定义
// ============================================================================

/// 数据库错误类型
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// 连接错误
    #[error("database connection failed: {0}")]
    ConnectionError(String),

    /// 新库结构初始化错误
    #[error("database schema initialization failed: {0}")]
    SchemaInitializationError(String),

    /// 实体未找到
    #[error("{entity} not found: {id}")]
    NotFound { entity: String, id: String },

    /// 余额不足
    #[error("insufficient balance: required {required}, available {available}")]
    InsufficientBalance { required: String, available: String },

    /// 唯一约束冲突
    #[error("duplicate key: {entity} with {field}={value} already exists")]
    DuplicateKey {
        entity: String,
        field: String,
        value: String,
    },

    /// 订单状态无效
    #[error("invalid order status: expected {expected}, actual {actual}")]
    InvalidOrderStatus { expected: String, actual: String },

    /// 数据库原生错误
    #[error("database error: {0}")]
    DatabaseError(#[from] sea_orm::DbErr),

    /// 其他错误
    #[error("{0}")]
    Other(String),
}

impl DbError {
    /// 创建 NotFound 错误
    pub fn not_found(entity: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity: entity.into(),
            id: id.into(),
        }
    }

    /// 创建 InsufficientBalance 错误
    pub fn insufficient_balance(required: impl Into<String>, available: impl Into<String>) -> Self {
        Self::InsufficientBalance {
            required: required.into(),
            available: available.into(),
        }
    }

    /// 创建 DuplicateKey 错误
    pub fn duplicate_key(
        entity: impl Into<String>,
        field: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::DuplicateKey {
            entity: entity.into(),
            field: field.into(),
            value: value.into(),
        }
    }

    /// 检查是否为未找到错误
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    /// 检查是否为余额不足错误
    pub fn is_insufficient_balance(&self) -> bool {
        matches!(self, Self::InsufficientBalance { .. })
    }

    /// 检查是否为唯一约束冲突
    pub fn is_duplicate(&self) -> bool {
        matches!(self, Self::DuplicateKey { .. })
            || matches!(self, Self::DatabaseError(sea_orm::DbErr::Query(e))
                if e.to_string().contains("duplicate key") || e.to_string().contains("unique constraint"))
    }

    /// 从 sea_orm::DbErr 转换，保留语义
    pub fn from_db_err(err: sea_orm::DbErr, entity: &str, id: &str) -> Self {
        match &err {
            sea_orm::DbErr::RecordNotFound(_) => Self::NotFound {
                entity: entity.to_string(),
                id: id.to_string(),
            },
            sea_orm::DbErr::Query(e)
                if e.to_string().contains("duplicate key")
                    || e.to_string().contains("unique constraint") =>
            {
                Self::DuplicateKey {
                    entity: entity.to_string(),
                    field: "constraint".to_string(),
                    value: id.to_string(),
                }
            }
            _ => Self::DatabaseError(err),
        }
    }
}

// ============================================================================
// 数据库配置
// ============================================================================

/// 数据库配置
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// 数据库连接 URL
    pub url: String,
    /// 最大连接数
    pub max_connections: u32,
    /// 最小连接数
    pub min_connections: u32,
    /// 连接超时时间（秒）
    pub connect_timeout: u64,
    /// 连接空闲超时时间（秒）
    pub idle_timeout: u64,
    /// 连接最大生命周期（秒）
    pub max_lifetime: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://localhost/keycompute".to_string()),
            max_connections: 10,
            min_connections: 2,
            connect_timeout: 30,
            idle_timeout: 600,
            max_lifetime: 1800,
        }
    }
}

impl From<&DatabaseConfig> for db_router::DatabaseConfig {
    fn from(c: &DatabaseConfig) -> Self {
        Self {
            max_connections: c.max_connections,
            min_connections: c.min_connections,
            connect_timeout_secs: c.connect_timeout,
            idle_timeout_secs: c.idle_timeout,
            max_lifetime_secs: c.max_lifetime,
        }
    }
}

// ============================================================================
// 连接池管理
// ============================================================================

/// 初始化数据库连接
///
/// # Examples
///
/// ```rust,no_run
/// use keycompute_db::{init_pool, DatabaseConfig};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = DatabaseConfig::default();
///     let db = init_pool(&config).await?;
///     Ok(())
/// }
/// ```
pub async fn init_pool(config: &DatabaseConfig) -> Result<DatabaseConnection, DbError> {
    let mut opt = ConnectOptions::new(&config.url);
    opt.max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.connect_timeout))
        .idle_timeout(Duration::from_secs(config.idle_timeout))
        .max_lifetime(Duration::from_secs(config.max_lifetime));

    let db = SeaDatabase::connect(opt)
        .await
        .map_err(|e| DbError::ConnectionError(e.to_string()))?;

    tracing::info!("Database pool initialized successfully");

    Ok(db)
}

/// 初始化一个全新的数据库结构。
///
/// 本项目不提供增量迁移或旧数据升级能力。部署时必须连接空数据库；结构变化后
/// 应重建数据库再启动服务。整个 schema 在单个事务内执行，任一语句失败都会回滚，
/// 防止服务在结构不完整的数据库上继续运行。
///
/// 执行完成后会验证哨兵列：由于 schema 全部使用 `IF NOT EXISTS`，误连接到
/// 旧版结构的存量库时旧表会被静默跳过，哨兵校验把这种部署错误从运行期
/// 故障提前为启动即失败。
pub async fn initialize_schema(db: &DatabaseConnection) -> Result<(), DbError> {
    let transaction = db
        .begin()
        .await
        .map_err(|error| DbError::SchemaInitializationError(error.to_string()))?;

    transaction
        .execute_unprepared(DATABASE_SCHEMA)
        .await
        .map_err(|error| DbError::SchemaInitializationError(error.to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| DbError::SchemaInitializationError(error.to_string()))?;

    verify_schema_sentinels(db).await?;

    tracing::info!("Database schema initialized successfully");
    Ok(())
}

/// 近期结构修订引入的哨兵列。旧版数据库缺少这些列，而 `IF NOT EXISTS`
/// 不会为已存在的表补列，因此可用于识别“误部署到存量旧库”的场景。
const SCHEMA_SENTINEL_COLUMNS: &[(&str, &str)] = &[
    ("payment_orders", "payment_scene"),
    ("payment_orders", "provider_trade_no"),
    ("payment_notifications", "payload_digest"),
    ("payment_provider_states", "circuit_state"),
];

/// 验证当前数据库包含最新 schema 的哨兵列，缺失时拒绝启动。
pub async fn verify_schema_sentinels(db: &impl ConnectionTrait) -> Result<(), DbError> {
    verify_required_columns(db, SCHEMA_SENTINEL_COLUMNS).await
}

/// 验证指定的 (表, 列) 对在当前 schema 中存在。
pub async fn verify_required_columns(
    db: &impl ConnectionTrait,
    required: &[(&str, &str)],
) -> Result<(), DbError> {
    let mut missing = Vec::new();
    for (table, column) in required {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 AS present FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 AND column_name = $2",
            [(*table).into(), (*column).into()],
        );
        let found = db
            .query_one(stmt)
            .await
            .map_err(|error| DbError::SchemaInitializationError(error.to_string()))?;
        if found.is_none() {
            missing.push(format!("{table}.{column}"));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(DbError::SchemaInitializationError(format!(
            "database is missing required columns [{}]; this deployment only supports \
             an empty database; rebuild the database instead of reusing a legacy one",
            missing.join(", ")
        )))
    }
}

/// 清理超过保留期的支付安全事件，返回删除的行数。
///
/// 回调端点暴露于公网，`payment_security_events` 按拒绝事件持续追加；
/// 定期清理避免表无界增长。
pub async fn purge_expired_payment_security_events(
    db: &impl ConnectionTrait,
    retention_days: i64,
) -> Result<u64, DbError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM payment_security_events WHERE created_at < NOW() - make_interval(days => $1::int)",
        [retention_days.into()],
    );
    let result = db.execute(statement).await?;
    Ok(result.rows_affected())
}

// ============================================================================
// 数据库管理器
// ============================================================================

/// 数据库管理器
///
/// 封装数据库路由器，提供统一的数据库访问入口
#[derive(Clone)]
pub struct Database {
    router: Arc<DbRouter>,
}

impl Database {
    /// 创建新的数据库实例
    ///
    /// # 参数
    /// * `write_config` — 写库连接池配置
    /// * `read_urls` — 读库连接 URL 列表（空列表 = 无读写分离）
    /// * `read_config` — 读库连接池配置
    /// * `routing_config` — 读写分离路由配置
    pub async fn new(
        write_config: &DatabaseConfig,
        read_urls: &[String],
        read_config: &keycompute_config::DatabaseReadConfig,
        routing_config: &keycompute_config::DatabaseRoutingConfig,
    ) -> Result<Self, DbError> {
        use db_router::{
            DatabaseConfig as RouterDbConfig, DatabaseReadConfig as RouterReadConfig,
            DatabaseRoutingConfig as RouterRoutingConfig,
        };
        let router = DbRouter::new(
            &write_config.url,
            read_urls,
            &RouterDbConfig::from(write_config),
            &RouterReadConfig::from(read_config),
            &RouterRoutingConfig::from(routing_config),
        )
        .await
        .map_err(|e| DbError::ConnectionError(e.to_string()))?;
        Ok(Self { router })
    }

    /// 从 DbRouter 创建
    pub fn from_router(router: Arc<DbRouter>) -> Self {
        Self { router }
    }

    /// 从现有连接创建（包装为单库模式）
    pub fn from_connection(db: DatabaseConnection) -> Self {
        Self {
            router: DbRouter::single(db),
        }
    }

    /// 获取连接引用（返回写库连接）
    pub fn connection(&self) -> &DatabaseConnection {
        self.router.write_conn()
    }

    /// 获取路由引用
    pub fn router(&self) -> Arc<DbRouter> {
        Arc::clone(&self.router)
    }

    /// 开始一个事务
    pub async fn begin(&self) -> Result<DatabaseTransaction, sea_orm::DbErr> {
        self.router.write_conn().begin().await
    }

    /// 初始化空数据库结构；不负责升级已有数据库
    pub async fn initialize_schema(&self) -> Result<(), DbError> {
        initialize_schema(self.router.write_conn()).await
    }

    /// 测试连接
    pub async fn test_connection(&self) -> Result<(), DbError> {
        let stmt = Statement::from_string(DbBackend::Postgres, "SELECT 1".to_string());
        self.router.write_conn().execute(stmt).await?;
        Ok(())
    }

    /// 获取写库连接（消费自身）
    #[deprecated(since = "0.3.0", note = "Use `router()` or `connection()` instead")]
    pub fn into_connection(self) -> DatabaseConnection {
        self.router.write_conn().clone()
    }

    /// 获取路由器（消费自身）
    pub fn into_router(self) -> Arc<DbRouter> {
        self.router
    }
}

/// 数据库连接管理器（已弃用，使用 Database）
#[deprecated(since = "0.2.0", note = "Use `Database` instead")]
pub type DatabaseManager = Database;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_config_default() {
        let config = DatabaseConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
    }

    #[test]
    fn test_db_error_helpers() {
        let err = DbError::not_found("User", "123");
        assert!(err.is_not_found());
        assert!(err.to_string().contains("User not found"));

        let err = DbError::insufficient_balance("100", "50");
        assert!(err.is_insufficient_balance());
        assert!(err.to_string().contains("insufficient balance"));

        let err = DbError::duplicate_key("User", "email", "test@example.com");
        assert!(err.is_duplicate());
    }

    #[test]
    fn test_db_error_from_db_err() {
        let err = DbError::from_db_err(
            sea_orm::DbErr::RecordNotFound("not found".to_string()),
            "User",
            "123",
        );
        assert!(err.is_not_found());
    }

    #[test]
    fn database_schema_contains_no_incremental_upgrade_steps() {
        assert!(DATABASE_SCHEMA.contains("token_version INTEGER NOT NULL DEFAULT 0"));
        assert!(DATABASE_SCHEMA.contains("CONSTRAINT uk_distribution_records_unique"));
        assert!(DATABASE_SCHEMA.contains("CONSTRAINT uk_node_tips_usage_log_id UNIQUE"));
        assert!(!DATABASE_SCHEMA.contains("ALTER TABLE"));
        assert!(!DATABASE_SCHEMA.contains("\nUPDATE "));
        assert!(!DATABASE_SCHEMA.contains("\nDELETE FROM "));
    }
}
