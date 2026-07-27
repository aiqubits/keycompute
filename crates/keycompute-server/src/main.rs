//! KeyCompute 后端服务主入口
//!
//! 这是整个 KeyCompute 系统的可执行入口，负责：
//! 1. 加载配置（环境变量 + 配置文件）
//! 2. 初始化可观测性（日志、指标、追踪）
//! 3. 建立数据库连接并初始化新库结构
//! 4. 初始化所有业务模块（Auth、RateLimit、Pricing、Routing、Gateway、Billing 等）
//! 5. 初始化默认系统管理员（如果配置）
//! 6. 启动 HTTP 服务器

use keycompute_auth::PasswordHasher;
use keycompute_config::AppConfig;
use keycompute_db::{
    CreateDistributionRuleRequest, CreateTenantRequest, CreateUserCredentialRequest,
    CreateUserRequest, Database, DatabaseConfig as DbConfig, SystemSetting, Tenant,
    TenantDistributionRule, User, models::system_setting::setting_keys,
};
use keycompute_observability::{init_dev_observability, init_observability};
use keycompute_server::{AppState, AppStateConfig, init_global_crypto, run};
use keycompute_types::UserRole;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use std::time::Duration;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ==================== 阶段 1: 加载配置 ====================
    info!("KeyCompute 启动中...");

    let config = match AppConfig::load() {
        Ok(cfg) => {
            info!("配置加载成功");
            cfg
        }
        Err(e) => {
            eprintln!("配置加载失败: {}", e);
            std::process::exit(1);
        }
    };

    // 验证配置
    if let Err(e) = config.validate() {
        eprintln!("配置验证失败: {}", e);
        std::process::exit(1);
    }

    // ==================== 阶段 2: 初始化可观测性 ====================
    // 根据环境选择日志格式
    let env = std::env::var("KC__ENV").unwrap_or_else(|_| "production".to_string());
    if env == "development" || env == "dev" {
        init_dev_observability();
        info!("开发环境可观测性已初始化");
    } else {
        init_observability();
        info!("生产环境可观测性已初始化");
    }

    // ==================== 阶段 3: 初始化全局加密 ====================
    if let Err(e) = init_global_crypto(&config) {
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
    let _system_tenant = match initialize_default_admin(pool.as_ref()).await {
        Ok(tenant) => {
            info!(tenant_id = %tenant.id, "系统租户初始化成功");
            Some(tenant)
        }
        Err(e) => {
            warn!("默认管理员初始化失败: {}", e);
            None
        }
    };

    // ==================== 阶段 5.5: 初始化系统默认设置 ====================
    info!("正在初始化系统默认设置...");
    match SystemSetting::init_default_settings(pool.as_ref()).await {
        Ok(_) => info!("系统默认设置初始化完成"),
        Err(e) => warn!("系统默认设置初始化失败（非致命错误）: {}", e),
    }

    // ==================== 阶段 5.6: 初始化系统默认定价 ====================
    info!("正在初始化系统默认定价...");
    match keycompute_db::models::pricing_model::PricingModel::init_default_pricing(pool.as_ref())
        .await
    {
        Ok(_) => info!("系统默认定价初始化完成"),
        Err(e) => warn!("系统默认定价初始化失败（非致命错误）: {}", e),
    }

    if let Err(e) = validate_distribution_public_base_url(pool.as_ref(), &config).await {
        warn!("运行时配置校验警告: {}", e);
    }

    // ==================== 阶段 6: 初始化应用状态 ====================
    info!("正在初始化应用状态...");

    let state_config = AppStateConfig::from_config(&config);
    let app_state = AppState::with_pool_and_config(pool, state_config);

    // 验证生产环境配置
    if env != "development"
        && env != "dev"
        && let Err(e) = app_state.validate_for_production()
    {
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

/// 初始化默认系统管理员
///
/// 从环境变量读取默认管理员邮箱和密码，如果管理员不存在则创建。
/// 环境变量：
/// - KC__DEFAULT_ADMIN_EMAIL: 管理员邮箱
/// - KC__DEFAULT_ADMIN_PASSWORD: 管理员密码
fn non_empty_or_default(value: Option<String>, default: &str) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_var_or_default(key: &str, default: &str) -> String {
    non_empty_or_default(std::env::var(key).ok(), default)
}

/// 内置的弱默认管理员密码，仅允许在开发环境使用。
const WEAK_DEFAULT_ADMIN_PASSWORD: &str = "12345";

/// 当前是否为非开发（生产）环境。
fn is_production_env() -> bool {
    !matches!(
        std::env::var("KC__ENV").unwrap_or_default().as_str(),
        "development" | "dev"
    )
}

/// 解析用于创建默认管理员的密码。
///
/// 生产环境必须显式配置 `KC__DEFAULT_ADMIN_PASSWORD` 且不能等于内置弱默认值；
/// 开发环境允许回退到弱默认值以方便本地调试。
fn resolve_default_admin_password(configured: Option<String>) -> anyhow::Result<String> {
    resolve_default_admin_password_for_env(configured, is_production_env())
}

fn resolve_default_admin_password_for_env(
    configured: Option<String>,
    is_production: bool,
) -> anyhow::Result<String> {
    if !is_production {
        return Ok(configured.unwrap_or_else(|| WEAK_DEFAULT_ADMIN_PASSWORD.to_string()));
    }

    match configured {
        Some(password) if password != WEAK_DEFAULT_ADMIN_PASSWORD => Ok(password),
        Some(_) => anyhow::bail!(
            "refusing to create the default admin with the weak built-in password in production; \
             set KC__DEFAULT_ADMIN_PASSWORD to a strong, unique value"
        ),
        None => anyhow::bail!(
            "KC__DEFAULT_ADMIN_PASSWORD must be set to create the default admin in production"
        ),
    }
}

async fn initialize_default_admin(
    pool: &(impl ConnectionTrait + TransactionTrait),
) -> anyhow::Result<Tenant> {
    // 从环境变量读取配置
    let admin_email = env_var_or_default("KC__DEFAULT_ADMIN_EMAIL", "admin@keycompute.local");
    // 显式提供的密码（区分“未配置”与“配置为空”）
    let configured_password = std::env::var("KC__DEFAULT_ADMIN_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());

    info!(email = %admin_email, "检查默认管理员账户");

    // 只要已经存在 system 用户，就视为默认系统管理员已完成初始化。
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM users WHERE role = 'system' ORDER BY created_at ASC LIMIT 1",
        [],
    );
    let existing_system_user = User::find_by_statement(stmt).one(pool).await?;

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
        let tenant = Tenant::find_by_slug(pool, "system")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to find system tenant: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("system 租户不存在但 system 用户已存在，数据不一致"))?;
        return Ok(tenant);
    }

    // 没有 system 用户时，配置邮箱也不能被普通账号占用。
    if let Some(existing_user) = User::find_by_email(pool, &admin_email).await? {
        anyhow::bail!(
            "cannot initialize system admin: email {} is already used by non-system user {}",
            admin_email,
            existing_user.id
        );
    }

    info!(email = %admin_email, "创建默认系统管理员");

    // 安全守卫：生产环境禁止使用缺省/弱默认管理员密码创建新管理员。
    // 仅在“确实需要创建新管理员”时校验，避免影响已初始化的部署。
    let admin_password = resolve_default_admin_password(configured_password)?;

    // 复用或创建默认 system 租户
    let tenant = if let Some(existing_tenant) = Tenant::find_by_slug(pool, "system").await? {
        info!(tenant_id = %existing_tenant.id, "复用已有 system 租户");
        existing_tenant
    } else {
        let tenant = Tenant::create(
            pool,
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
        pool,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: admin_email.clone(),
            name: Some("System Administrator".to_string()),
            role: Some(UserRole::System),
        },
    )
    .await?;

    info!(user_id = %user.id, "管理员用户创建成功");

    // 哈希密码
    let hasher = PasswordHasher::new();
    let password_hash = hasher.hash(&admin_password)?;

    // 创建用户凭证
    let credential = keycompute_db::UserCredential::create(
        pool,
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
            pool,
            &UpdateUserCredentialRequest {
                email_verified: Some(true),
                email_verified_at: Some(chrono::Utc::now()),
                ..Default::default()
            },
        )
        .await?;

    // 初始化默认管理员余额（创建余额记录，初始余额为 0）
    initialize_admin_balance(pool, tenant.id, user.id).await?;

    // 创建默认分销规则（基于系统设置中的比例）
    initialize_default_distribution_rules(pool, tenant.id, user.id).await?;

    info!(
        user_id = %user.id,
        email = %admin_email,
        tenant_id = %tenant.id,
        "默认系统管理员初始化成功"
    );

    Ok(tenant)
}

async fn validate_distribution_public_base_url(
    pool: &impl ConnectionTrait,
    config: &AppConfig,
) -> anyhow::Result<()> {
    let distribution_enabled = SystemSetting::find_by_key(pool, setting_keys::DISTRIBUTION_ENABLED)
        .await?
        .map(|setting| setting.parse_bool())
        .unwrap_or(true);

    if distribution_enabled && config.app_base_url.is_none() {
        anyhow::bail!("APP_BASE_URL must be configured when distribution is enabled");
    }

    Ok(())
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

    match TenantDistributionRule::create(pool, &level1_rule).await {
        Ok(rule) => info!(rule_id = %rule.id, "一级分销规则创建成功"),
        Err(e) => warn!("一级分销规则创建失败: {}", e),
    }

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

    match TenantDistributionRule::create(pool, &level2_rule).await {
        Ok(rule) => info!(rule_id = %rule.id, "二级分销规则创建成功"),
        Err(e) => warn!("二级分销规则创建失败: {}", e),
    }

    info!(tenant_id = %tenant_id, "默认分销规则初始化完成");
    Ok(())
}

/// 初始化管理员余额
///
/// 为默认系统管理员充值 100 元初始余额
/// 系统管理员不需要审计，直接设置余额
async fn initialize_admin_balance(
    pool: &(impl ConnectionTrait + TransactionTrait),
    tenant_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> anyhow::Result<()> {
    use keycompute_db::UserBalance;
    use rust_decimal::Decimal;

    // 检查是否已存在余额记录
    if let Some(existing_balance) = UserBalance::find_by_user(pool, user_id).await? {
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

    // recharge 内部已管理事务
    let initial_amount = Decimal::new(100, 0); // 100 元
    let (updated_balance, transaction) = UserBalance::recharge(
        pool,
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
    use super::{non_empty_or_default, resolve_default_admin_password_for_env};

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
    fn test_resolve_admin_password_dev_allows_weak_default() {
        // 开发环境未配置时回退到弱默认值
        let resolved = resolve_default_admin_password_for_env(None, false).unwrap();
        assert_eq!(resolved, "12345");
    }

    #[test]
    fn test_resolve_admin_password_dev_uses_configured() {
        let resolved =
            resolve_default_admin_password_for_env(Some("custom-dev-pass".to_string()), false)
                .unwrap();
        assert_eq!(resolved, "custom-dev-pass");
    }

    #[test]
    fn test_resolve_admin_password_production_requires_configured() {
        // 生产环境未配置密码——必须拒绝
        let err = resolve_default_admin_password_for_env(None, true).unwrap_err();
        assert!(
            err.to_string()
                .contains("KC__DEFAULT_ADMIN_PASSWORD must be set")
        );
    }

    #[test]
    fn test_resolve_admin_password_production_rejects_weak_default() {
        // 生产环境显式使用弱默认值——必须拒绝
        let err =
            resolve_default_admin_password_for_env(Some("12345".to_string()), true).unwrap_err();
        assert!(err.to_string().contains("weak built-in password"));
    }

    #[test]
    fn test_resolve_admin_password_production_accepts_strong() {
        let resolved =
            resolve_default_admin_password_for_env(Some("S7r0ng-Pass!word".to_string()), true)
                .unwrap();
        assert_eq!(resolved, "S7r0ng-Pass!word");
    }
}
