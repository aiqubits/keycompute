//! KeyCompute 后端服务主入口
//!
//! 这是整个 KeyCompute 系统的可执行入口，负责：
//! 1. 按构建方式加载配置（debug: config.toml；release: 环境变量）
//! 2. 初始化可观测性（日志、指标、追踪）
//! 3. 建立数据库连接并初始化新库结构
//! 4. 初始化所有业务模块（Auth、RateLimit、Pricing、Routing、Gateway、Billing 等）
//! 5. 初始化默认系统管理员（如果配置）
//! 6. 启动 HTTP 服务器

use futures::{StreamExt, stream};
use keycompute_auth::PasswordHasher;
use keycompute_config::{AppConfig, DEFAULT_ADMIN_EMAIL, DEFAULT_ADMIN_PASSWORD};
use keycompute_db::{
    CreateDistributionRuleRequest, CreateTenantRequest, CreateUserCredentialRequest,
    CreateUserRequest, Database, DatabaseConfig as DbConfig, DbRouter, SystemSetting, Tenant,
    TenantDistributionRule, User,
};
use keycompute_observability::{init_dev_observability, init_observability};
use keycompute_server::{AppState, AppStateConfig, init_global_crypto, run};
use keycompute_types::UserRole;
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
    sqlx::{Connection as SqlxConnection, PgConnection},
};
use std::time::Duration;
use tracing::{error, info, warn};

/// 运行模式由可执行文件的构建方式固定，不接受配置或环境变量修改。
const IS_DEVELOPMENT_BUILD: bool = cfg!(debug_assertions);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ==================== 阶段 1: 加载配置 ====================
    info!("KeyCompute 启动中...");

    let config_result = if IS_DEVELOPMENT_BUILD {
        AppConfig::load_development()
    } else {
        AppConfig::load_production()
    };
    let config = match config_result {
        Ok(cfg) => {
            info!("配置加载成功");
            cfg
        }
        Err(e) => {
            eprintln!("配置加载失败: {}", e);
            std::process::exit(1);
        }
    };

    let is_development = IS_DEVELOPMENT_BUILD;
    let is_production = !is_development;

    // 验证配置
    let config_validation = if is_production {
        config.validate_for_production()
    } else {
        config.validate()
    };
    if let Err(e) = config_validation {
        eprintln!("配置验证失败: {}", e);
        std::process::exit(1);
    }
    // ==================== 阶段 2: 初始化可观测性 ====================
    // 根据环境选择日志格式
    if is_development {
        init_dev_observability();
        info!("开发环境可观测性已初始化");
    } else {
        init_observability();
        info!("生产环境可观测性已初始化");
    }

    // ==================== 阶段 3: 初始化全局加密 ====================
    if let Err(e) = init_global_crypto(&config, is_production) {
        error!("全局加密初始化失败: {}", e);
        std::process::exit(1);
    }

    // ==================== 阶段 4: 建立数据库连接 ====================
    info!("正在连接数据库...");

    // 转换配置类型
    let db_config = DbConfig {
        url: config.database.url.clone(),
        max_connections: config.database.max_connections,
        min_connections: config.database.min_connections,
        connect_timeout: config.database.connect_timeout_secs,
        idle_timeout: config.database.idle_timeout_secs,
        max_lifetime: config.database.max_lifetime_secs,
    };

    let db_manager = match Database::new(
        &db_config,
        &config.database_read_urls,
        &config.database_read,
        &config.database_routing,
    )
    .await
    {
        Ok(manager) => {
            info!("数据库连接成功");
            manager
        }
        Err(e) => {
            error!("数据库连接失败: {}", e);
            std::process::exit(1);
        }
    };

    // 测试数据库连接
    if let Err(e) = db_manager.test_connection().await {
        error!("数据库连接测试失败: {}", e);
        std::process::exit(1);
    }

    // 系统只支持空库部署：结构变化时应重建数据库，不执行增量升级或数据迁移。
    info!("正在初始化数据库结构...");
    if let Err(e) = db_manager.initialize_schema().await {
        error!("数据库结构初始化失败: {}", e);
        std::process::exit(1);
    }
    info!("数据库结构初始化完成");

    let pool = db_manager.into_router();

    // 启动读库健康检查（如配置了健康检查间隔）
    if config.database_routing.health_check_interval_secs > 0 {
        let health_interval =
            Duration::from_secs(config.database_routing.health_check_interval_secs);
        pool.clone().start_health_check(health_interval);
        info!(
            "Read replica health check started (interval: {}s)",
            config.database_routing.health_check_interval_secs
        );
    }

    // ==================== 阶段 5: 初始化默认系统管理员 ====================
    let default_admin_email = env_var_or_default("KC__DEFAULT_ADMIN_EMAIL", DEFAULT_ADMIN_EMAIL);
    // 空字符串按未配置处理；这使 Compose 能在首次引导完成后安全移除该变量。
    let default_admin_password = std::env::var("KC__DEFAULT_ADMIN_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());
    let _system_tenant = match initialize_default_admin(
        pool.as_ref(),
        is_production,
        &default_admin_email,
        default_admin_password,
    )
    .await
    {
        Ok(tenant) => {
            info!(tenant_id = %tenant.id, "系统租户初始化成功");
            tenant
        }
        Err(e) => {
            error!("默认管理员初始化失败，服务拒绝继续启动: {}", e);
            return Err(e);
        }
    };

    // ==================== 阶段 5.5: 初始化系统默认设置 ====================
    info!("正在初始化系统默认设置...");
    match SystemSetting::init_default_settings(pool.as_ref()).await {
        Ok(_) => info!("系统默认设置初始化完成"),
        Err(e) => warn!("系统默认设置初始化失败（非致命错误）: {}", e),
    }

    // 分销邀请链接必须使用当前部署显式配置的公开 URL。旧基线默认启用分销，
    // 因此在路由开放前原子地收敛不兼容状态，避免请求期才返回配置错误。
    match SystemSetting::reconcile_distribution_public_url(
        pool.write_conn(),
        config.resolved_app_base_url().is_some(),
    )
    .await
    {
        Ok(true) => warn!("未配置 APP_BASE_URL，已禁用分销系统；配置公开 URL 后可由管理员重新启用"),
        Ok(false) => {}
        Err(e) => {
            error!("分销系统公开 URL 状态校验失败，服务拒绝继续启动: {}", e);
            return Err(e.into());
        }
    }

    // ==================== 阶段 5.6: 初始化系统默认定价 ====================
    info!("正在初始化系统默认定价...");
    match keycompute_db::models::pricing_model::PricingModel::init_default_pricing(pool.as_ref())
        .await
    {
        Ok(_) => info!("系统默认定价初始化完成"),
        Err(e) => warn!("系统默认定价初始化失败（非致命错误）: {}", e),
    }

    // ==================== 阶段 6: 初始化应用状态 ====================
    info!("正在初始化应用状态...");

    let state_config = AppStateConfig::from_config(&config);
    let app_state = AppState::with_pool_and_config(pool, state_config);

    // 验证生产环境配置
    if is_production && let Err(e) = app_state.validate_for_production() {
        error!("生产环境验证失败：{}", e);
        std::process::exit(1);
    }

    info!("应用状态初始化完成");

    if app_state.payment.is_some() {
        // 支付回调限流依赖可信代理覆盖的 X-Real-IP，未经代理直连时回调会被
        // fail-closed 拒绝，启动时提前提醒部署前置条件。
        info!(
            "支付回调已启用：/api/v1/payments/notify/* 必须经过会覆盖 X-Real-IP 的可信反向代理，否则回调将被拒绝（503）"
        );
    }
    // 不绑定支付是否启用：即使后续禁用支付，历史安全事件也应按期清理。
    if let Some(pool) = app_state.pool.clone() {
        spawn_payment_security_event_retention(pool);
    }
    if let Some(pool) = app_state.pool.clone() {
        spawn_stale_trace_reconciler(pool, config.gateway.timeout_secs as i64);
    }
    if let Some(node_gateway) = app_state.node_gateway.as_ref() {
        spawn_node_gateway_sweeper(
            node_gateway.sweeper(),
            node_gateway.config.sweeper_repush_interval_secs,
        );
    }
    if config.gateway.account_probe_interval_secs > 0 {
        spawn_account_probe_alerts(
            app_state.clone(),
            config.database.url.clone(),
            config.gateway.account_probe_interval_secs,
            config.gateway.account_probe_concurrency,
        );
    } else {
        info!(
            "Provider Account 自动真实推理探测已禁用；可在监控页手动探测或配置 gateway.account_probe_interval_secs 启用"
        );
    }

    // ==================== 阶段 7: 启动服务器 ====================
    info!("准备启动服务器...");

    let server_config = config.server.clone();

    // 优雅关闭处理
    let shutdown = setup_shutdown_handler();

    info!(
        "KeyCompute 服务器即将启动于 {}:{}",
        server_config.bind_addr, server_config.port
    );

    // 启动服务器（带优雅关闭支持）
    tokio::select! {
        result = run(server_config, app_state) => {
            if let Err(e) = result {
                error!("服务器运行错误: {}", e);
                std::process::exit(1);
            }
        }
        _ = shutdown => {
            info!("收到关闭信号，正在优雅关闭...");
        }
    }

    info!("KeyCompute 服务器已停止");
    Ok(())
}

/// 启动支付安全事件的后台保留清理任务
///
/// 回调端点暴露于公网，payment_security_events 按拒绝事件持续追加；
/// 每 24 小时清理一次 90 天前的事件，避免表无界增长。
fn spawn_payment_security_event_retention(pool: std::sync::Arc<keycompute_db::DbRouter>) {
    const RETENTION_DAYS: i64 = 90;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match keycompute_db::purge_expired_payment_security_events(
                pool.write_conn(),
                RETENTION_DAYS,
            )
            .await
            {
                Ok(removed) if removed > 0 => {
                    info!(removed, "已清理过期的支付安全事件");
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(%error, "支付安全事件保留清理失败，将在下个周期重试");
                }
            }
        }
    });
}

const STALE_TRACE_FINALIZATION_GRACE_SECS: i64 = 60;

fn stale_trace_threshold_secs(gateway_timeout_secs: i64) -> i64 {
    // Gateway success finalization deliberately runs outside the execution
    // timeout. Leave one full reconciliation interval of grace so that a
    // request finishing at the timeout boundary cannot be overwritten as
    // stale while its terminal trace write is acquiring the row lock.
    gateway_timeout_secs
        .max(60)
        .saturating_add(STALE_TRACE_FINALIZATION_GRACE_SECS)
}

fn spawn_stale_trace_reconciler(
    pool: std::sync::Arc<keycompute_db::DbRouter>,
    gateway_timeout_secs: i64,
) {
    let stale_after_secs = stale_trace_threshold_secs(gateway_timeout_secs);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match keycompute_db::reconcile_stale_requests(pool.as_ref(), stale_after_secs, 200)
                .await
            {
                Ok(count) if count > 0 => {
                    keycompute_observability::metrics::STALE_REQUEST_RECONCILED_TOTAL.inc_by(count);
                    info!(count, "已修复 stale request traces")
                }
                Ok(_) => {}
                Err(error) => warn!(%error,"stale request trace 修复失败，将在下一周期重试"),
            }
        }
    });
}

fn spawn_node_gateway_sweeper(sweeper: node_gateway::NodeGatewaySweeper, interval_secs: u64) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs.max(1)));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(error) = sweeper.run_once().await {
                warn!(%error, "Node Gateway sweeper 执行失败，将在下一周期重试");
            }
        }
    });
}

// Stable, application-owned PostgreSQL advisory lock key for the automatic
// Provider Account probe. Keep it distinct from every other background job.
const ACCOUNT_PROBE_ADVISORY_LOCK_ID: i64 = 4_708_607_967_117_743_156;

async fn try_acquire_background_job_lock(
    database_url: &str,
    lock_id: i64,
) -> anyhow::Result<Option<PgConnection>> {
    // Use a dedicated session instead of borrowing a pooled business
    // connection or keeping a transaction open across external HTTP calls.
    // PostgreSQL releases the session advisory lock automatically on drop.
    let mut connection = PgConnection::connect(database_url).await?;
    let acquired = sea_orm::sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
        .bind(lock_id)
        .fetch_one(&mut connection)
        .await?;
    if acquired {
        Ok(Some(connection))
    } else {
        Ok(None)
    }
}

fn spawn_account_probe_alerts(
    state: AppState,
    database_url: String,
    interval_secs: u64,
    concurrency: usize,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
            let Some(pool) = state.pool.as_deref() else {
                return;
            };
            let leader_connection = match try_acquire_background_job_lock(
                &database_url,
                ACCOUNT_PROBE_ADVISORY_LOCK_ID,
            )
            .await
            {
                Ok(Some(connection)) => connection,
                Ok(None) => {
                    tracing::debug!("跳过非 leader 副本的 Provider Account 自动探测");
                    continue;
                }
                Err(error) => {
                    warn!(%error, "Provider Account 自动探测无法获取 leader 锁");
                    continue;
                }
            };
            let accounts = match keycompute_db::Account::find_enabled_all(pool.write_conn()).await {
                Ok(accounts) => accounts,
                Err(error) => {
                    warn!(%error,"定时账号探测无法读取账号");
                    drop(leader_connection);
                    continue;
                }
            };
            stream::iter(accounts.into_iter().map(|account| {
                let state = state.clone();
                async move {
                    let id = account.id;
                    let outcome =
                        keycompute_server::handlers::probe_enabled_account_for_monitoring(
                            &state, id,
                        )
                        .await;
                    (id, outcome)
                }
            }))
            .buffer_unordered(concurrency)
            .for_each(|(account_id, outcome)| async move {
                match outcome {
                    Ok(Some(value))
                        if value.get("success").and_then(|value| value.as_bool()) == Some(true) => {
                    }
                    Ok(Some(_)) => warn!(%account_id,"Provider Account 定时探测失败"),
                    Ok(None) => {
                        tracing::debug!(%account_id,"跳过已禁用或已删除的 Provider Account");
                    }
                    Err(error) => warn!(%account_id,%error,"Provider Account 定时探测异常"),
                }
            })
            .await;
            // Dropping the dedicated connection releases the session lock.
            drop(leader_connection);
        }
    });
}

/// 设置优雅关闭信号处理器
///
/// 监听 SIGINT (Ctrl+C) 和 SIGTERM 信号
fn setup_shutdown_handler() -> tokio::sync::oneshot::Receiver<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("Failed to create SIGINT handler");
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to create SIGTERM handler");

        tokio::select! {
            _ = sigint.recv() => {
                info!("收到 SIGINT 信号");
            }
            _ = sigterm.recv() => {
                info!("收到 SIGTERM 信号");
            }
        }

        let _ = tx.send(());
    });

    rx
}

/// Resolve an optional environment value, treating an empty string as absent.
fn non_empty_or_default(value: Option<String>, default: &str) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_var_or_default(key: &str, default: &str) -> String {
    non_empty_or_default(std::env::var(key).ok(), default)
}

/// 解析用于创建默认管理员的密码。
///
/// 未配置时使用公开示例值以便开发环境开箱运行；生产环境首次创建
/// system 管理员时必须显式覆盖。已有 system 管理员后不再需要该引导密码。
fn resolve_default_admin_password(configured: Option<String>) -> String {
    non_empty_or_default(configured, DEFAULT_ADMIN_PASSWORD)
}

#[derive(Debug)]
struct InvalidDefaultAdminBootstrapPassword;

impl std::fmt::Display for InvalidDefaultAdminBootstrapPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "KC__DEFAULT_ADMIN_PASSWORD must be a non-default password of at least 12 characters",
        )
    }
}

impl std::error::Error for InvalidDefaultAdminBootstrapPassword {}

fn validate_default_admin_password_for_production(
    configured: Option<String>,
) -> anyhow::Result<()> {
    let password = resolve_default_admin_password(configured);
    if password.trim().is_empty()
        || password == DEFAULT_ADMIN_PASSWORD
        || password.chars().count() < 12
    {
        return Err(InvalidDefaultAdminBootstrapPassword.into());
    }
    Ok(())
}

fn validate_default_admin_bootstrap_password(
    system_admin_exists: bool,
    is_production: bool,
    configured: Option<String>,
) -> anyhow::Result<()> {
    if is_production && !system_admin_exists {
        validate_default_admin_password_for_production(configured)?;
    }
    Ok(())
}

fn default_admin_bootstrap_connection(pool: &DbRouter) -> &sea_orm::DatabaseConnection {
    // Bootstrap is a read-modify-write sequence. Every decision must observe
    // the writer or replica lag can make an initialized production deployment
    // look empty and incorrectly require the one-time bootstrap password.
    pool.write_conn()
}

// Serialize the one-time bootstrap across replicas. The transaction-scoped
// lock is released automatically on commit or rollback.
const DEFAULT_ADMIN_BOOTSTRAP_LOCK_ID: i64 = 5_421_647_644_090_913_945;

async fn initialize_default_admin(
    pool: &DbRouter,
    is_production: bool,
    admin_email: &str,
    configured_password: Option<String>,
) -> anyhow::Result<Tenant> {
    let writer = default_admin_bootstrap_connection(pool);
    let tx = writer.begin().await?;
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        [DEFAULT_ADMIN_BOOTSTRAP_LOCK_ID.into()],
    ))
    .await?;

    info!(email = %admin_email, "检查默认管理员账户");

    // 已存在 system 用户时仍会校验租户归属和登录凭证，拒绝历史半初始化状态。
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM users WHERE role = 'system' ORDER BY created_at ASC LIMIT 1",
        [],
    );
    let existing_system_user = User::find_by_statement(stmt).one(&tx).await?;

    // The bootstrap password is needed only when this startup will actually
    // create the first system administrator. Initialized deployments should be
    // able to remove this one-time secret and continue restarting safely.
    validate_default_admin_bootstrap_password(
        existing_system_user.is_some(),
        is_production,
        configured_password.clone(),
    )?;

    if let Some(user) = existing_system_user {
        if user.email == admin_email {
            info!(email = %admin_email, user_id = %user.id, "默认系统管理员已存在，跳过初始化");
        } else {
            warn!(
                configured_email = %admin_email,
                existing_email = %user.email,
                user_id = %user.id,
                "已存在 system 用户，跳过默认管理员初始化"
            );
        }
        // 获取 system 租户（复用已有租户）
        let tenant = Tenant::find_by_slug(&tx, "system")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to find system tenant: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("system 租户不存在但 system 用户已存在，数据不一致"))?;

        if tenant.id != user.tenant_id {
            anyhow::bail!("system 用户 {} 不属于 system 租户，数据不一致", user.id);
        }

        let credential = keycompute_db::UserCredential::find_by_user_id(&tx, user.id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "system 用户 {} 缺少登录凭证，拒绝以不完整引导状态启动",
                    user.id
                )
            })?;
        if credential.password_hash.trim().is_empty() {
            anyhow::bail!("system 用户 {} 的密码哈希为空，数据不一致", user.id);
        }

        tx.commit().await?;
        return Ok(tenant);
    }

    // 没有 system 用户时，配置邮箱也不能被普通账号占用。
    if let Some(existing_user) = User::find_by_email(&tx, admin_email).await? {
        anyhow::bail!(
            "cannot initialize system admin: email {} is already used by non-system user {}",
            admin_email,
            existing_user.id
        );
    }

    info!(email = %admin_email, "创建默认系统管理员");

    let admin_password = resolve_default_admin_password(configured_password);
    if !is_production
        && (admin_password.trim().is_empty()
            || admin_password == DEFAULT_ADMIN_PASSWORD
            || admin_password.chars().count() < 12)
    {
        warn!("默认管理员正在使用示例或较弱密码");
    }

    // 在写入任何引导数据前完成密码哈希；之后所有写入均属于同一事务。
    let hasher = PasswordHasher::new();
    let password_hash = hasher.hash(&admin_password)?;

    // 复用或创建默认 system 租户
    let tenant = if let Some(existing_tenant) = Tenant::find_by_slug(&tx, "system").await? {
        info!(tenant_id = %existing_tenant.id, "复用已有 system 租户");
        existing_tenant
    } else {
        let tenant = Tenant::create(
            &tx,
            &CreateTenantRequest {
                name: "System".to_string(),
                slug: "system".to_string(),
                description: Some("System default tenant".to_string()),
                default_rpm_limit: None,
                default_tpm_limit: None,
            },
        )
        .await?;

        info!(tenant_id = %tenant.id, "默认租户创建成功");
        tenant
    };

    // 创建管理员用户（role="system" 表示系统管理员）
    let user = User::create(
        &tx,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: admin_email.to_string(),
            name: Some("System Administrator".to_string()),
            role: Some(UserRole::System),
        },
    )
    .await?;

    info!(user_id = %user.id, "管理员用户创建成功");

    // 创建用户凭证
    let credential = keycompute_db::UserCredential::create(
        &tx,
        &CreateUserCredentialRequest {
            user_id: user.id,
            password_hash,
        },
    )
    .await?;

    // 标记邮箱已验证
    use keycompute_db::UpdateUserCredentialRequest;
    credential
        .update(
            &tx,
            &UpdateUserCredentialRequest {
                email_verified: Some(true),
                email_verified_at: Some(chrono::Utc::now()),
                ..Default::default()
            },
        )
        .await?;

    // 初始化默认管理员余额（创建余额记录并充值 100 元）
    initialize_admin_balance(&tx, tenant.id, user.id).await?;

    // 创建默认分销规则（基于系统设置中的比例）
    initialize_default_distribution_rules(&tx, tenant.id, user.id).await?;

    tx.commit().await?;

    info!(
        user_id = %user.id,
        email = %admin_email,
        tenant_id = %tenant.id,
        "默认系统管理员初始化成功"
    );

    Ok(tenant)
}

/// 初始化默认分销规则
///
/// 基于 system_settings 中的配置创建一级和二级分销规则
async fn initialize_default_distribution_rules(
    pool: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    _admin_user_id: uuid::Uuid,
) -> anyhow::Result<()> {
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    // 检查是否已存在分销规则
    let existing_rules = TenantDistributionRule::find_all_by_tenant(pool, tenant_id).await?;
    if !existing_rules.is_empty() {
        info!(tenant_id = %tenant_id, "分销规则已存在，跳过初始化");
        return Ok(());
    }

    // 从系统设置获取默认分销比例（与 RuleEngine 硬编码保持一致：3% 和 2%）
    let level1_ratio_str =
        SystemSetting::get_string(pool, "distribution_level1_default_ratio", "0.03").await;

    let level2_ratio_str =
        SystemSetting::get_string(pool, "distribution_level2_default_ratio", "0.02").await;

    let level1_ratio = BigDecimal::from_str(&level1_ratio_str)
        .unwrap_or_else(|_| BigDecimal::from_str("0.03").unwrap());
    let level2_ratio = BigDecimal::from_str(&level2_ratio_str)
        .unwrap_or_else(|_| BigDecimal::from_str("0.02").unwrap());

    info!(
        tenant_id = %tenant_id,
        level1_ratio = %level1_ratio,
        level2_ratio = %level2_ratio,
        "正在创建默认分销规则"
    );

    // 创建一级分销规则（全局规则，对所有用户生效）
    let level1_rule = CreateDistributionRuleRequest {
        tenant_id,
        beneficiary_id: uuid::Uuid::nil(), // 全局规则，对所有用户生效
        name: "一级分销规则".to_string(),
        description: Some("默认一级分销规则，推荐人可获得指定比例的分销佣金".to_string()),
        commission_rate: level1_ratio,
        priority: Some(10),
        effective_from: Some(chrono::Utc::now()),
        effective_until: None,
    };

    let rule = TenantDistributionRule::create(pool, &level1_rule).await?;
    info!(rule_id = %rule.id, "一级分销规则创建成功");

    // 创建二级分销规则（全局规则，对所有用户生效）
    let level2_rule = CreateDistributionRuleRequest {
        tenant_id,
        beneficiary_id: uuid::Uuid::nil(), // 全局规则，对所有用户生效
        name: "二级分销规则".to_string(),
        description: Some("默认二级分销规则，间接推荐人可获得指定比例的分销佣金".to_string()),
        commission_rate: level2_ratio,
        priority: Some(5),
        effective_from: Some(chrono::Utc::now()),
        effective_until: None,
    };

    let rule = TenantDistributionRule::create(pool, &level2_rule).await?;
    info!(rule_id = %rule.id, "二级分销规则创建成功");

    info!(tenant_id = %tenant_id, "默认分销规则初始化完成");
    Ok(())
}

/// 初始化管理员余额
///
/// 为默认系统管理员充值 100 元初始余额
/// 系统管理员不需要审计，直接设置余额
async fn initialize_admin_balance(
    tx: &DatabaseTransaction,
    tenant_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> anyhow::Result<()> {
    use keycompute_db::UserBalance;
    use rust_decimal::Decimal;

    // 检查是否已存在余额记录
    if let Some(existing_balance) = UserBalance::find_by_user(tx, user_id).await? {
        // 如果已有余额且不为 0，说明已经初始化过，跳过
        if existing_balance.available_balance > Decimal::ZERO {
            info!(
                user_id = %user_id,
                balance = %existing_balance.available_balance,
                "管理员余额已初始化，跳过"
            );
            return Ok(());
        }
    }

    let initial_amount = Decimal::new(100, 0); // 100 元
    let (updated_balance, transaction) = UserBalance::recharge_in_tx(
        tx,
        user_id,
        tenant_id,
        initial_amount,
        None, // 无订单 ID
        Some("系统管理员初始余额"),
    )
    .await?;

    info!(
        user_id = %user_id,
        tenant_id = %tenant_id,
        balance_id = %updated_balance.id,
        initial_balance = %updated_balance.available_balance,
        transaction_id = %transaction.id,
        "系统管理员初始余额充值成功"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        default_admin_bootstrap_connection, initialize_default_admin, non_empty_or_default,
        resolve_default_admin_password, stale_trace_threshold_secs,
        validate_default_admin_bootstrap_password, validate_default_admin_password_for_production,
    };
    use keycompute_config::DEFAULT_ADMIN_PASSWORD;
    use keycompute_db::models::system_setting::setting_keys;
    use keycompute_db::{
        DbRouter, SystemSetting, Tenant, TenantDistributionRule, User, UserBalance, UserCredential,
    };
    use rust_decimal::Decimal;
    use sea_orm::{
        ConnectOptions, ConnectionTrait, Database as SeaDatabase, DatabaseConnection, DbBackend,
        Statement,
    };
    use uuid::Uuid;

    /// Create an isolated PostgreSQL schema so bootstrap tests can exercise the
    /// real transaction and advisory lock without touching shared test data.
    async fn isolated_bootstrap_database()
    -> Option<(DatabaseConnection, DatabaseConnection, String)> {
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(value) => value,
            Err(_) => {
                eprintln!("skipping PostgreSQL bootstrap test: DATABASE_URL is not set");
                return None;
            }
        };
        let admin = SeaDatabase::connect(&database_url)
            .await
            .expect("DATABASE_URL should be reachable");
        let schema = format!("bootstrap_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .expect("isolated bootstrap schema should be created");

        let mut options = ConnectOptions::new(database_url);
        options
            .max_connections(8)
            .min_connections(1)
            .set_schema_search_path(&schema);
        let isolated = SeaDatabase::connect(options)
            .await
            .expect("isolated bootstrap database should connect");
        keycompute_db::initialize_schema(&isolated)
            .await
            .expect("isolated bootstrap schema should initialize");

        Some((admin, isolated, schema))
    }

    async fn drop_isolated_bootstrap_database(
        admin: DatabaseConnection,
        isolated: DatabaseConnection,
        schema: String,
    ) {
        isolated
            .close()
            .await
            .expect("isolated connection should close");
        admin
            .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
            .await
            .expect("isolated bootstrap schema should be removed");
        admin.close().await.expect("admin connection should close");
    }

    #[test]
    fn test_non_empty_or_default_uses_non_empty_value() {
        let resolved = non_empty_or_default(
            Some("admin@example.com".to_string()),
            "fallback@example.com",
        );

        assert_eq!(resolved, "admin@example.com");
    }

    #[test]
    fn test_non_empty_or_default_falls_back_for_empty_value() {
        let resolved = non_empty_or_default(Some(String::new()), "fallback");

        assert_eq!(resolved, "fallback");
    }

    #[test]
    fn test_non_empty_or_default_falls_back_for_missing_value() {
        let resolved = non_empty_or_default(None, "fallback");

        assert_eq!(resolved, "fallback");
    }

    #[test]
    fn test_resolve_admin_password_uses_shared_default() {
        let resolved = resolve_default_admin_password(None);
        assert_eq!(resolved, DEFAULT_ADMIN_PASSWORD);
    }

    #[test]
    fn test_resolve_admin_password_uses_configured_value() {
        let resolved = resolve_default_admin_password(Some("custom-dev-pass".to_string()));
        assert_eq!(resolved, "custom-dev-pass");
    }

    #[test]
    fn test_resolve_admin_password_preserves_explicit_value_for_development() {
        let resolved = resolve_default_admin_password(Some("12345".to_string()));
        assert_eq!(resolved, "12345");
    }

    #[test]
    fn production_rejects_default_and_weak_admin_passwords() {
        let error = validate_default_admin_password_for_production(None).unwrap_err();
        assert!(
            error
                .downcast_ref::<super::InvalidDefaultAdminBootstrapPassword>()
                .is_some()
        );
        assert!(validate_default_admin_password_for_production(Some("12345".to_string())).is_err());
        assert!(
            validate_default_admin_password_for_production(Some("            ".to_string()))
                .is_err()
        );
        // UTF-8 字节数超过 12、但字符数不足 12 的密码仍应被拒绝。
        assert!(
            validate_default_admin_password_for_production(Some("管理密码".to_string())).is_err()
        );
        assert!(
            validate_default_admin_password_for_production(Some(
                "安全管理密码字符正好十二".to_string()
            ))
            .is_ok()
        );
        assert!(
            validate_default_admin_password_for_production(Some(
                "independent-admin-password".to_string()
            ))
            .is_ok()
        );
    }

    #[test]
    fn initialized_production_does_not_require_the_bootstrap_password() {
        assert!(validate_default_admin_bootstrap_password(true, true, None).is_ok());
        assert!(validate_default_admin_bootstrap_password(false, true, None).is_err());
        assert!(validate_default_admin_bootstrap_password(false, false, None).is_ok());
    }

    #[test]
    fn default_admin_bootstrap_uses_a_writer_connection() {
        let router = DbRouter::single(DatabaseConnection::Disconnected);
        assert!(matches!(
            default_admin_bootstrap_connection(router.as_ref()),
            DatabaseConnection::Disconnected
        ));
    }

    #[tokio::test]
    async fn default_admin_bootstrap_is_concurrent_and_idempotent_in_postgres() {
        let Some((admin, isolated, schema)) = isolated_bootstrap_database().await else {
            return;
        };
        let router = DbRouter::single(isolated.clone());
        let email = "bootstrap-concurrency@example.com";
        let password = "independent-bootstrap-password";

        let (first, second) = tokio::join!(
            initialize_default_admin(router.as_ref(), true, email, Some(password.to_string())),
            initialize_default_admin(router.as_ref(), true, email, Some(password.to_string()))
        );
        let first = first.expect("first bootstrap should succeed");
        let second = second.expect("concurrent bootstrap should reuse committed state");
        assert_eq!(first.id, second.id);

        let user = User::find_by_email(&isolated, email)
            .await
            .unwrap()
            .expect("system user should exist");
        assert_eq!(user.role, "system");
        assert_eq!(user.tenant_id, first.id);
        let credential = UserCredential::find_by_user_id(&isolated, user.id)
            .await
            .unwrap()
            .expect("credential should exist");
        assert!(credential.email_verified);
        assert!(!credential.password_hash.trim().is_empty());
        let balance = UserBalance::find_by_user(&isolated, user.id)
            .await
            .unwrap()
            .expect("balance should exist");
        assert_eq!(balance.available_balance, Decimal::new(100, 0));
        assert_eq!(
            TenantDistributionRule::find_all_by_tenant(&isolated, first.id)
                .await
                .unwrap()
                .len(),
            2
        );
        let recharge_count = isolated
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT COUNT(*)::BIGINT AS count FROM balance_transactions WHERE user_id = $1 AND transaction_type = 'recharge'",
                [user.id.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "count")
            .unwrap();
        assert_eq!(recharge_count, 1);

        drop(router);
        drop_isolated_bootstrap_database(admin, isolated, schema).await;
    }

    #[tokio::test]
    async fn default_admin_bootstrap_rolls_back_partial_writes_in_postgres() {
        let Some((admin, isolated, schema)) = isolated_bootstrap_database().await else {
            return;
        };
        // DECIMAL(5,4) cannot store this ratio. The failure occurs after the
        // tenant, user, credential, and balance writes, proving the outer
        // bootstrap transaction rolls the entire sequence back.
        SystemSetting::update_value(
            &isolated,
            setting_keys::DISTRIBUTION_LEVEL1_DEFAULT_RATIO,
            "100",
        )
        .await
        .unwrap();
        let router = DbRouter::single(isolated.clone());
        let email = "bootstrap-rollback@example.com";

        assert!(
            initialize_default_admin(
                router.as_ref(),
                true,
                email,
                Some("independent-bootstrap-password".to_string()),
            )
            .await
            .is_err()
        );
        assert!(
            User::find_by_email(&isolated, email)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            Tenant::find_by_slug(&isolated, "system")
                .await
                .unwrap()
                .is_none()
        );

        drop(router);
        drop_isolated_bootstrap_database(admin, isolated, schema).await;
    }

    #[test]
    fn stale_trace_reconciliation_waits_past_gateway_timeout() {
        assert_eq!(stale_trace_threshold_secs(120), 180);
        assert_eq!(stale_trace_threshold_secs(0), 120);
    }
}
