//! KeyCompute 配置管理模块
//!
//! 运行模式与配置来源由可执行文件的构建方式固定：
//! 1. debug 构建仅读取项目根目录 `config.toml`
//! 2. release 构建仅读取 `KC__*` 与顶层 `APP_BASE_URL` 环境变量
//! 3. 两种来源都使用代码默认值补全未配置项
//!
//! 配置中不存在运行模式开关，两种来源也不会相互覆盖。

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::net::IpAddr;
use std::path::Path;
use url::Url;
use uuid::Uuid;

pub mod auth;
pub mod crypto;
pub mod database;
pub mod distribution;
pub mod email;
pub mod gateway;
pub mod node_gateway;
pub mod redis;
pub mod server;

pub use auth::AuthConfig;
pub use auth::DEFAULT_JWT_SECRET;
pub use crypto::CryptoConfig;
pub use database::{DatabaseConfig, DatabaseReadConfig, DatabaseRoutingConfig};
pub use distribution::DistributionConfig;
pub use email::EmailConfig;
pub use gateway::{GatewayConfig, ProxyConfig};
pub use node_gateway::{DEFAULT_REGISTRATION_TOKEN_SECRET, NodeGatewayConfig};
pub use redis::RedisConfig;
pub use server::ServerConfig;

/// 首次启动时创建的示例管理员邮箱。
pub const DEFAULT_ADMIN_EMAIL: &str = "admin@keycompute.local";
/// 首次启动时创建的示例管理员密码；生产环境首次创建 system 管理员时会拒绝该值。
pub const DEFAULT_ADMIN_PASSWORD: &str = "change-me-admin-password";

/// 全局应用配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AppConfig {
    /// 对外公开的前端应用基础 URL（可选）
    pub app_base_url: Option<String>,
    /// 服务器配置
    pub server: ServerConfig,
    /// 数据库配置
    pub database: DatabaseConfig,
    /// 读库连接 URL 列表（空 = 无读写分离）
    #[serde(default)]
    pub database_read_urls: Vec<String>,
    /// 读写分离路由配置
    #[serde(default)]
    pub database_routing: DatabaseRoutingConfig,
    /// 读库连接池配置
    #[serde(default)]
    pub database_read: DatabaseReadConfig,
    /// Redis 配置（可选）
    pub redis: Option<RedisConfig>,
    /// 认证配置
    pub auth: AuthConfig,
    /// Gateway 配置
    pub gateway: GatewayConfig,
    /// 加密配置（可选）
    pub crypto: Option<CryptoConfig>,
    /// 邮件服务配置
    #[serde(default)]
    pub email: EmailConfig,
    /// 节点网关配置（可选）
    pub node_gateway: Option<NodeGatewayConfig>,
}

/// 配置加载错误
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    #[error("配置解析失败: {0}")]
    ParseError(#[from] ConfigError),
    #[error("配置文件不存在: {0}")]
    FileNotFound(String),
    #[error("环境变量格式错误: {0}")]
    EnvFormatError(String),
    #[error("配置验证失败: {0}")]
    ValidationError(String),
}

impl AppConfig {
    pub fn resolved_app_base_url(&self) -> Option<String> {
        Self::normalize_app_base_url(self.app_base_url.clone())
    }

    fn apply_production_env_overrides(mut app_config: AppConfig) -> AppConfig {
        if let Ok(url) = std::env::var("APP_BASE_URL") {
            app_config.app_base_url = Some(url);
        }

        Self::normalize(app_config)
    }

    fn normalize(mut app_config: AppConfig) -> AppConfig {
        app_config.app_base_url = Self::normalize_app_base_url(app_config.app_base_url);
        app_config
    }

    fn normalize_app_base_url(value: Option<String>) -> Option<String> {
        value.and_then(|url| {
            let normalized = url.trim().trim_end_matches('/').to_string();
            if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            }
        })
    }

    fn is_local_development_host(url: &Url) -> bool {
        match url.host_str() {
            Some("localhost") => true,
            Some(host) => host
                .parse::<IpAddr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false),
            None => false,
        }
    }

    fn validate_public_app_base_url(base_url: &str) -> Result<(), String> {
        let parsed = Url::parse(base_url)
            .map_err(|e| format!("APP_BASE_URL 必须是合法的绝对 URL: {}", e))?;

        if parsed.host_str().is_none() {
            return Err("APP_BASE_URL 必须包含主机名".to_string());
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("APP_BASE_URL 不能包含用户名或密码".to_string());
        }

        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err("APP_BASE_URL 不能包含查询参数或片段".to_string());
        }

        match parsed.scheme() {
            "https" => Ok(()),
            "http" if Self::is_local_development_host(&parsed) => Ok(()),
            "http" => Err("APP_BASE_URL 在非本地环境必须使用 https".to_string()),
            scheme => Err(format!(
                "APP_BASE_URL 仅支持 http/https 协议，当前为 {}",
                scheme
            )),
        }
    }

    /// 加载 debug 构建的开发配置。
    ///
    /// 固定读取当前工作目录下的 `config.toml`；文件不存在时
    /// fail closed，且不读取 `KC__*` 或 `APP_BASE_URL` 环境变量。
    pub fn load_development() -> Result<Self, ConfigLoadError> {
        Self::from_file("config.toml")
    }

    /// 加载 release 构建的生产配置。
    ///
    /// 固定读取 `KC__*` 与顶层 `APP_BASE_URL` 环境变量，不探测或
    /// 读取 `config.toml`。Docker Compose 通过 `.env` 向容器注入这些变量。
    pub fn load_production() -> Result<Self, ConfigLoadError> {
        Self::from_env()
    }

    /// 仅从生产环境变量加载配置。
    fn from_env() -> Result<Self, ConfigLoadError> {
        // 设置默认值
        let mut builder = Self::create_default_builder()?;

        // 仅从环境变量加载
        builder = builder.add_source(
            Environment::with_prefix("KC")
                .separator("__")
                .try_parsing(true)
                .ignore_empty(true)
                .list_separator(",")
                .with_list_parse_key("database_read_urls")
                .with_list_parse_key("database_routing.read_weights"),
        );

        let config = builder.build()?;
        let app_config: AppConfig = Self::apply_production_env_overrides(config.try_deserialize()?);

        Ok(app_config)
    }

    /// 仅从开发配置文件加载配置，不接受环境变量覆盖。
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigLoadError> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(ConfigLoadError::FileNotFound(
                path.to_string_lossy().to_string(),
            ));
        }

        // 设置默认值
        let mut builder = Self::create_default_builder()?;

        // 从指定文件加载
        builder = builder.add_source(File::from(path).required(true));

        let config = builder.build()?;
        let app_config: AppConfig = Self::normalize(config.try_deserialize()?);

        Ok(app_config)
    }

    /// 创建带默认值的配置构建器
    fn create_default_builder()
    -> Result<config::ConfigBuilder<config::builder::DefaultState>, ConfigError> {
        let builder = Config::builder()
            // 服务器默认值
            .set_default("server.bind_addr", "0.0.0.0")?
            .set_default("server.port", 3000)?
            // 数据库默认值
            .set_default("database.url", "postgres://localhost/keycompute")?
            .set_default("database.max_connections", 10)?
            .set_default("database.min_connections", 2)?
            .set_default("database.connect_timeout_secs", 30)?
            .set_default("database.idle_timeout_secs", 600)?
            .set_default("database.max_lifetime_secs", 1800)?
            // 认证默认值
            .set_default("auth.jwt_secret", DEFAULT_JWT_SECRET)?
            .set_default("auth.jwt_issuer", "keycompute")?
            .set_default("auth.jwt_expiry_secs", 3600)?
            // Gateway 默认值
            .set_default("gateway.max_retries", 3)?
            .set_default("gateway.timeout_secs", 120)?
            .set_default("gateway.enable_fallback", true)?
            .set_default("gateway.request_timeout_secs", 120)?
            .set_default("gateway.stream_timeout_secs", 600)?
            .set_default("gateway.account_probe_interval_secs", 0)?
            .set_default("gateway.account_probe_concurrency", 4)?;

        Ok(builder)
    }

    /// 验证配置有效性
    ///
    /// 验证项包括：
    /// - 服务器绑定地址有效性
    /// - 服务器端口有效性
    /// - 数据库连接 URL 有效性
    /// - 数据库连接池配置合理性（max > 0, max >= min）
    /// - 数据库超时配置有效性
    /// - Email 配置完整性建议（不完整时禁用邮件能力）
    /// - JWT 密钥安全性建议
    /// - JWT 密钥长度警告
    /// - JWT 过期时间有效性
    /// - JWT 签发者有效性
    /// - 加密密钥配置提醒
    /// - Redis 配置验证（如果已配置）
    /// - Gateway 超时配置警告
    /// - Gateway 最大重试次数警告
    pub fn validate(&self) -> Result<(), ConfigLoadError> {
        // 验证服务器配置
        if self.server.bind_addr.is_empty() {
            return Err(ConfigLoadError::ValidationError(
                "服务器绑定地址不能为空".to_string(),
            ));
        }

        // 验证服务器端口（有效范围 1-65535）
        if self.server.port == 0 {
            return Err(ConfigLoadError::ValidationError(
                "服务器端口不能为 0".to_string(),
            ));
        }
        // 注意：u16 类型自动保证端口 <= 65535，无需额外检查

        // 验证数据库 URL
        if self.database.url.is_empty() {
            return Err(ConfigLoadError::ValidationError(
                "数据库 URL 不能为空".to_string(),
            ));
        }

        // 验证数据库连接池配置
        if self.database.max_connections == 0 {
            return Err(ConfigLoadError::ValidationError(
                "数据库最大连接数不能为 0".to_string(),
            ));
        }

        if self.database.max_connections < self.database.min_connections {
            return Err(ConfigLoadError::ValidationError(
                "数据库最大连接数不能小于最小连接数".to_string(),
            ));
        }

        // 数据库超时配置检查
        if self.database.connect_timeout_secs == 0 {
            return Err(ConfigLoadError::ValidationError(
                "数据库连接超时不能为 0".to_string(),
            ));
        }

        if self.database.idle_timeout_secs == 0 {
            tracing::warn!("⚠️  数据库空闲超时设置为 0，连接将永不过期");
        }

        if self.database.max_lifetime_secs == 0 {
            tracing::warn!("⚠️  数据库连接最大生命周期设置为 0，连接将永不过期");
        }

        // 读库配置验证（如有配置读库）
        if !self.database_read_urls.is_empty() {
            // 验证每个读库 URL 格式
            for (i, url) in self.database_read_urls.iter().enumerate() {
                if Url::parse(url).is_err() {
                    return Err(ConfigLoadError::ValidationError(format!(
                        "读库 URL #{} 格式无效: '{}'",
                        i + 1,
                        url
                    )));
                }
            }

            // 验证路由策略
            match self.database_routing.strategy.to_lowercase().as_str() {
                "round_robin" | "random" | "weighted" => {}
                _ => {
                    return Err(ConfigLoadError::ValidationError(format!(
                        "读写分离路由策略无效: '{}'，可选值: round_robin, random, weighted",
                        self.database_routing.strategy
                    )));
                }
            }

            // 验证 weights 长度匹配
            if !self.database_routing.read_weights.is_empty()
                && self.database_routing.read_weights.len() != self.database_read_urls.len()
            {
                return Err(ConfigLoadError::ValidationError(format!(
                    "读库权重数量 ({}) 与读库 URL 数量 ({}) 不匹配",
                    self.database_routing.read_weights.len(),
                    self.database_read_urls.len(),
                )));
            }

            // 验证熔断时间
            if self.database_routing.circuit_break_ms == 0 {
                return Err(ConfigLoadError::ValidationError(
                    "读库熔断时间不能为 0".to_string(),
                ));
            }

            // 读库连接池配置检查
            if self.database_read.max_connections == 0 {
                return Err(ConfigLoadError::ValidationError(
                    "读库最大连接数不能为 0".to_string(),
                ));
            }

            if self.database_read.connect_timeout_secs == 0 {
                return Err(ConfigLoadError::ValidationError(
                    "读库连接超时不能为 0".to_string(),
                ));
            }
        }

        // 开发环境保留可运行、可覆盖的示例凭据，并在这里给出安全告警。
        // 生产入口会继续调用 validate_for_production，以 fail-closed 方式拒绝
        // 公开占位密钥和缺失的 Provider API Key 加密密钥。
        // JWT 密钥安全检查
        if self.auth.jwt_secret == DEFAULT_JWT_SECRET {
            tracing::warn!(
                "⚠️  安全警告: JWT 密钥使用开发示例值；生产启动会拒绝该值，请设置 KC__AUTH__JWT_SECRET"
            );
        }

        // JWT 密钥长度检查（排除默认密钥，避免重复警告）
        if self.auth.jwt_secret != DEFAULT_JWT_SECRET && self.auth.jwt_secret.len() < 32 {
            tracing::warn!("⚠️  安全警告: JWT 密钥长度不足 32 字节，建议使用更长的密钥");
        }

        // JWT 过期时间验证
        if self.auth.jwt_expiry_secs == 0 {
            return Err(ConfigLoadError::ValidationError(
                "JWT 过期时间不能为 0".to_string(),
            ));
        }

        if self.auth.jwt_expiry_secs > 86400 * 30 {
            // 超过 30 天
            tracing::warn!(
                "⚠️  JWT 过期时间设置为 {} 秒（超过 30 天），请确认是否符合安全策略",
                self.auth.jwt_expiry_secs
            );
        }

        // JWT 签发者验证
        if self.auth.jwt_issuer.is_empty() {
            return Err(ConfigLoadError::ValidationError(
                "JWT 签发者不能为空".to_string(),
            ));
        }

        // 数据库连接检查
        if self.database.url.contains("localhost") || self.database.url.contains("127.0.0.1") {
            tracing::debug!("数据库连接到本地地址，请确认生产环境配置正确");
        }

        let email_is_configured = self.email.is_configured();
        let email_is_partially_configured = self.email.is_partially_configured();

        // Email 配置检查。部分配置不会阻止主服务启动，邮件能力会保持禁用。
        if email_is_partially_configured {
            tracing::warn!(
                "⚠️  Email 配置不完整，邮件发送将被禁用；强烈建议完整配置 SMTP 主机、用户名、密码和发件人地址"
            );
        }

        if email_is_configured {
            // 简单的邮箱格式验证
            if !self.email.from_address.contains('@') {
                tracing::warn!(
                    "⚠️  Email 发件人地址 '{}' 格式可能不正确，缺少 @ 符号",
                    self.email.from_address
                );
            }

            if self.email.timeout_secs == 0 {
                tracing::warn!("⚠️  Email 发送超时设置为 0，将禁用 SMTP 超时");
            }
        }

        let resolved_app_base_url = self.resolved_app_base_url();
        if let Some(base_url) = resolved_app_base_url.as_deref() {
            Self::validate_public_app_base_url(base_url)
                .map_err(ConfigLoadError::ValidationError)?;
        } else if email_is_configured {
            return Err(ConfigLoadError::ValidationError(
                "启用 Email 服务时必须显式配置 APP_BASE_URL；禁止回退到其他部署的公开地址"
                    .to_string(),
            ));
        } else {
            tracing::info!("💡 提示: 未配置 APP_BASE_URL，密码重置和公开邀请链接功能不可用");
        }

        // 加密配置提醒
        let has_crypto_key = self.crypto.as_ref().map(|c| c.has_key()).unwrap_or(false);
        if !has_crypto_key {
            tracing::info!(
                "💡 提示: 未配置加密密钥，开发环境会明文存储 Provider API Key；生产启动会拒绝该配置"
            );
        }

        // Redis 配置检查
        if let Some(ref redis_config) = self.redis {
            // Redis 已配置，验证配置有效性
            if redis_config.url.is_empty() {
                return Err(ConfigLoadError::ValidationError(
                    "Redis URL 不能为空".to_string(),
                ));
            }
            if redis_config.pool_size == 0 {
                return Err(ConfigLoadError::ValidationError(
                    "Redis 连接池大小不能为 0".to_string(),
                ));
            }
            if redis_config.connect_timeout_secs == 0 {
                return Err(ConfigLoadError::ValidationError(
                    "Redis 连接超时不能为 0".to_string(),
                ));
            }
        } else {
            tracing::info!("💡 提示: 未配置 Redis，分布式限流功能将不可用");
        }

        // Gateway 的三层超时都会直接转换为 Duration。0 会让
        // tokio/reqwest 立即超时，因此必须在启动前拒绝这类配置。
        for (field, value) in [
            ("timeout_secs", self.gateway.timeout_secs),
            ("request_timeout_secs", self.gateway.request_timeout_secs),
            ("stream_timeout_secs", self.gateway.stream_timeout_secs),
        ] {
            if value == 0 {
                return Err(ConfigLoadError::ValidationError(format!(
                    "Gateway {field} 不能为 0"
                )));
            }
        }

        if self.gateway.max_retries == 0 {
            tracing::warn!("⚠️  Gateway 最大重试次数设置为 0，请求失败时将不会重试");
        }

        if self.gateway.max_retries > 10 {
            tracing::warn!(
                "⚠️  Gateway 最大重试次数设置为 {}，可能导致请求延迟过高",
                self.gateway.max_retries
            );
        }

        if !(1..=24).contains(&self.gateway.monitoring_raw_max_hours) {
            return Err(ConfigLoadError::ValidationError(
                "Gateway monitoring_raw_max_hours 必须在 1 到 24 之间".to_string(),
            ));
        }

        if self.gateway.account_probe_interval_secs != 0
            && self.gateway.account_probe_interval_secs < 60
        {
            return Err(ConfigLoadError::ValidationError(
                "Gateway account_probe_interval_secs 启用时不能小于 60".to_string(),
            ));
        }

        // Redis 可用时 server 会同时启用 Node Gateway。节点等待超时需要在
        // 持有 task 行锁的同时通过 lifecycle recorder 关闭 trace，因此必须
        // 给这两个短事务各保留一个写连接。
        if self.redis.is_some() && self.database.max_connections < 2 {
            return Err(ConfigLoadError::ValidationError(
                "启用 Redis-backed Node Gateway 时数据库最大连接数不能小于 2".to_string(),
            ));
        }

        if !(1..=32).contains(&self.gateway.account_probe_concurrency) {
            return Err(ConfigLoadError::ValidationError(
                "Gateway account_probe_concurrency 必须在 1 到 32 之间".to_string(),
            ));
        }

        if let Some(proxy) = &self.gateway.proxy {
            let validate_proxy_url = |rule: &str, value: &str| {
                let parsed = Url::parse(value).map_err(|_| {
                    ConfigLoadError::ValidationError(format!(
                        "Gateway proxy 规则 '{rule}' 的 URL 无效"
                    ))
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(ConfigLoadError::ValidationError(format!(
                        "Gateway proxy 规则 '{rule}' 仅支持 http/https URL"
                    )));
                }
                Ok(())
            };
            for (provider, url) in &proxy.providers {
                if provider.is_empty() {
                    return Err(ConfigLoadError::ValidationError(
                        "Gateway provider proxy 名称不能为空".to_string(),
                    ));
                }
                validate_proxy_url(provider, url)?;
            }
            if let Some(patterns) = &proxy.patterns {
                for (pattern, url) in patterns {
                    if pattern.is_empty() {
                        return Err(ConfigLoadError::ValidationError(
                            "Gateway pattern proxy 规则不能为空".to_string(),
                        ));
                    }
                    validate_proxy_url(pattern, url)?;
                }
            }
            if let Some(accounts) = &proxy.accounts {
                for (key, url) in accounts {
                    let Some((provider, account_id)) = key.rsplit_once(':') else {
                        return Err(ConfigLoadError::ValidationError(format!(
                            "Gateway account proxy 键 '{key}' 必须使用 provider:account_uuid 格式"
                        )));
                    };
                    if provider.is_empty() || Uuid::parse_str(account_id).is_err() {
                        return Err(ConfigLoadError::ValidationError(format!(
                            "Gateway account proxy 键 '{key}' 必须使用 provider:account_uuid 格式"
                        )));
                    }
                    validate_proxy_url(key, url)?;
                }
            }
        }

        // Node Gateway 配置检查
        if let Some(ref node_gateway_config) = self.node_gateway {
            // 检查 registration_token_secret (HMAC 签名密钥)
            if let Some(ref secret) = node_gateway_config.registration_token_secret {
                if secret.len() < 16 {
                    tracing::warn!(
                        "⚠️  安全警告: Node Gateway registration_token_secret 长度不足 16 字节，建议使用更长的密钥"
                    );
                }
                if secret == "change-me-in-production"
                    || secret == "change-me-node-registration-token-secret"
                {
                    tracing::warn!(
                        "⚠️  安全警告: Node Gateway registration_token_secret 使用开发占位符；生产环境配置 Redis 时会拒绝该值"
                    );
                }
            } else {
                tracing::warn!(
                    "⚠️  未设置 Node Gateway registration_token_secret，将使用开发示例密钥；生产环境配置 Redis 时会拒绝该配置"
                );
            }

            // 检查超时配置合理性
            if let Some(session_ttl) = node_gateway_config.session_ttl_secs
                && session_ttl == 0
            {
                tracing::warn!("⚠️  Node Gateway 会话 TTL 设置为 0，会话将立即过期");
            }

            if let Some(poll_timeout) = node_gateway_config.poll_timeout_secs
                && poll_timeout == 0
            {
                tracing::warn!("⚠️  Node Gateway 轮询超时设置为 0，轮询将立即失败");
            }

            if let Some(task_deadline) = node_gateway_config.task_deadline_secs
                && task_deadline == 0
            {
                tracing::warn!("⚠️  Node Gateway 任务 deadline 设置为 0，任务将立即过期");
            }

            // 检查失败阈值
            if let Some(threshold) = node_gateway_config.node_failure_threshold
                && threshold == 0
            {
                tracing::warn!("⚠️  Node Gateway 节点失败阈值设置为 0，节点将永远不会被排除");
            }

            if let Some(threshold) = node_gateway_config.task_failure_threshold
                && threshold == 0
            {
                tracing::warn!("⚠️  Node Gateway 任务失败阈值设置为 0，任务失败后将不会重试");
            }

            tracing::info!("Node Gateway 配置已加载");
        } else {
            tracing::warn!(
                "未显式配置 Node Gateway，将使用开发示例密钥和默认参数；若生产环境配置 Redis，启动检查会要求独立随机密钥"
            );
        }

        tracing::info!("配置验证通过");
        Ok(())
    }

    /// Validate secrets required to run this configuration in production.
    /// Development keeps the runnable examples, but production always requires
    /// a non-blank JWT secret of at least 32 bytes and a Provider API-key
    /// encryption key. A non-default node HMAC secret of at least 16 bytes is
    /// additionally required when Redis enables Node Gateway. SMTP settings
    /// must be either complete or entirely disabled.
    pub fn validate_for_production(&self) -> Result<(), ConfigLoadError> {
        self.validate()?;

        let mut issues = Vec::new();
        if self.auth.jwt_secret.trim().is_empty()
            || self.auth.jwt_secret == DEFAULT_JWT_SECRET
            || self.auth.jwt_secret.len() < 32
        {
            issues.push(
                "KC__AUTH__JWT_SECRET must be a non-default value of at least 32 bytes".to_string(),
            );
        }

        if !self.crypto.as_ref().is_some_and(CryptoConfig::has_key) {
            issues.push("KC__CRYPTO__SECRET_KEY must be configured so Provider API keys are not stored in plaintext".to_string());
        }

        if self.email.is_partially_configured() {
            issues.push(
                "KC__EMAIL__SMTP_HOST, KC__EMAIL__SMTP_USERNAME, KC__EMAIL__SMTP_PASSWORD, and KC__EMAIL__FROM_ADDRESS must either all be configured or all remain blank"
                    .to_string(),
            );
        }

        // Node Gateway 只有在 Redis 后端存在时才会由 AppState 初始化；纯
        // Provider 部署不应被一个不会使用的节点注册密钥阻断。
        if self.redis.is_some() {
            let node_secret = self
                .node_gateway
                .as_ref()
                .and_then(|config| config.registration_token_secret.as_deref())
                .map(str::trim);
            if node_secret.is_none_or(|secret| {
                secret.is_empty()
                    || secret == DEFAULT_REGISTRATION_TOKEN_SECRET
                    || secret == "change-me-in-production"
                    || secret.len() < 16
            }) {
                issues.push("KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET must be a non-default value of at least 16 bytes when Redis enables Node Gateway".to_string());
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(ConfigLoadError::ValidationError(issues.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvVarGuard {
        original: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvVarGuard {
        fn set(values: &[(&'static str, &str)]) -> Self {
            let original = values
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect();

            unsafe {
                for (key, value) in values {
                    std::env::set_var(key, value);
                }
            }

            Self { original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                for (key, value) in &self.original {
                    if let Some(value) = value {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
        }
    }

    fn env_example_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
        let prefix = format!("{key}=");
        let mut commented = None;

        for line in contents.lines() {
            let trimmed = line.trim();
            if let Some(value) = trimmed.strip_prefix(&prefix) {
                return Some(value.trim());
            }
            if let Some(value) = trimmed
                .strip_prefix('#')
                .map(str::trim_start)
                .and_then(|line| line.strip_prefix(&prefix))
            {
                commented.get_or_insert(value.trim());
            }
        }

        commented
    }

    fn active_env_example_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
        let prefix = format!("{key}=");
        contents
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
    }

    fn readme_config_row<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
        let marker = format!("| `{key}` |");
        contents.lines().find(|line| line.starts_with(&marker))
    }

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.bind_addr, "0.0.0.0");
        assert!(config.app_base_url.is_none());
    }

    #[test]
    #[serial]
    fn test_config_example_matches_shared_fallbacks() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = root.join("config.example.toml");
        let config = AppConfig::from_file(path).expect("config.example.toml 应与配置结构保持一致");
        let defaults = AppConfig::default();
        let env_example =
            std::fs::read_to_string(root.join(".env.example")).expect("应该读取 .env.example");

        assert_eq!(
            config.app_base_url.as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(config.server.bind_addr, defaults.server.bind_addr);
        assert_eq!(config.server.port, defaults.server.port);
        assert_eq!(
            config.database.url,
            "postgres://keycompute:change-me-strong-password@127.0.0.1:5432/keycompute"
        );
        let postgres_url = format!(
            "postgres://{}:{}@127.0.0.1:{}/{}",
            active_env_example_value(&env_example, "POSTGRES_USER").unwrap(),
            active_env_example_value(&env_example, "POSTGRES_PASSWORD").unwrap(),
            active_env_example_value(&env_example, "POSTGRES_PORT").unwrap(),
            active_env_example_value(&env_example, "POSTGRES_DB").unwrap(),
        );
        assert_eq!(config.database.url, postgres_url);
        assert_eq!(
            config.database.max_connections,
            defaults.database.max_connections
        );
        assert_eq!(
            config.database.min_connections,
            defaults.database.min_connections
        );
        assert_eq!(
            config.database.connect_timeout_secs,
            defaults.database.connect_timeout_secs
        );
        assert_eq!(
            config.database.idle_timeout_secs,
            defaults.database.idle_timeout_secs
        );
        assert_eq!(
            config.database.max_lifetime_secs,
            defaults.database.max_lifetime_secs
        );
        assert_eq!(config.auth.jwt_secret, DEFAULT_JWT_SECRET);
        assert_eq!(config.auth.jwt_issuer, defaults.auth.jwt_issuer);
        assert_eq!(config.auth.jwt_expiry_secs, defaults.auth.jwt_expiry_secs);
        assert!(config.database_read_urls.is_empty());
        assert_eq!(
            config.database_routing.strategy,
            defaults.database_routing.strategy
        );
        assert!(config.database_routing.read_weights.is_empty());
        assert_eq!(
            config.database_routing.retry_attempts,
            defaults.database_routing.retry_attempts
        );
        assert_eq!(
            config.database_routing.circuit_break_ms,
            defaults.database_routing.circuit_break_ms
        );
        assert_eq!(
            config.database_routing.fallback_to_write,
            defaults.database_routing.fallback_to_write
        );
        assert_eq!(
            config.database_routing.health_check_interval_secs,
            defaults.database_routing.health_check_interval_secs
        );
        assert_eq!(
            config.database_read.max_connections,
            defaults.database_read.max_connections
        );
        assert_eq!(
            config.database_read.min_connections,
            defaults.database_read.min_connections
        );
        assert_eq!(
            config.database_read.connect_timeout_secs,
            defaults.database_read.connect_timeout_secs
        );
        assert_eq!(
            config.database_read.idle_timeout_secs,
            defaults.database_read.idle_timeout_secs
        );
        assert_eq!(
            config.database_read.acquire_timeout_secs,
            defaults.database_read.acquire_timeout_secs
        );
        assert_eq!(
            config.database_read.max_lifetime_secs,
            defaults.database_read.max_lifetime_secs
        );
        assert_eq!(
            config.gateway.monitoring_raw_max_hours,
            defaults.gateway.monitoring_raw_max_hours
        );
        assert_eq!(
            config.gateway.account_probe_interval_secs,
            defaults.gateway.account_probe_interval_secs
        );
        assert_eq!(
            config.gateway.account_probe_concurrency,
            defaults.gateway.account_probe_concurrency
        );
        assert_eq!(config.gateway.max_retries, defaults.gateway.max_retries);
        assert_eq!(config.gateway.timeout_secs, defaults.gateway.timeout_secs);
        assert_eq!(
            config.gateway.enable_fallback,
            defaults.gateway.enable_fallback
        );
        assert_eq!(
            config.gateway.request_timeout_secs,
            defaults.gateway.request_timeout_secs
        );
        assert_eq!(
            config.gateway.stream_timeout_secs,
            defaults.gateway.stream_timeout_secs
        );
        assert_eq!(
            config.redis.as_ref().expect("示例包含 Redis").pool_size,
            RedisConfig::default().pool_size
        );
        assert_eq!(
            config.redis.as_ref().expect("示例包含 Redis").url,
            format!(
                "redis://:{}@127.0.0.1:{}",
                active_env_example_value(&env_example, "REDIS_PASSWORD").unwrap(),
                active_env_example_value(&env_example, "REDIS_PORT").unwrap(),
            )
        );
        assert_eq!(
            config
                .redis
                .as_ref()
                .expect("示例包含 Redis")
                .connect_timeout_secs,
            RedisConfig::default().connect_timeout_secs
        );
        assert_eq!(config.email.smtp_port, defaults.email.smtp_port);
        assert_eq!(config.email.from_name, defaults.email.from_name);
        assert_eq!(config.email.use_tls, defaults.email.use_tls);
        assert_eq!(config.email.timeout_secs, defaults.email.timeout_secs);
        assert_eq!(config.email.requirement_recipient, None);
        assert_eq!(config.email, defaults.email);
        assert!(config.crypto.is_none());

        let node = config.node_gateway.expect("示例包含 Node Gateway");
        let node_defaults = NodeGatewayConfig::default();
        assert_eq!(
            node.registration_token_secret,
            node_defaults.registration_token_secret
        );
        assert_eq!(node.session_ttl_secs, node_defaults.session_ttl_secs);
        assert_eq!(
            node.heartbeat_interval_secs,
            node_defaults.heartbeat_interval_secs
        );
        assert_eq!(node.poll_timeout_secs, node_defaults.poll_timeout_secs);
        assert_eq!(node.task_deadline_secs, node_defaults.task_deadline_secs);
        assert_eq!(node.complete_grace_secs, node_defaults.complete_grace_secs);
        assert_eq!(
            node.node_failure_threshold,
            node_defaults.node_failure_threshold
        );
        assert_eq!(
            node.task_failure_threshold,
            node_defaults.task_failure_threshold
        );
        assert_eq!(
            node.sweeper_heartbeat_ttl_secs,
            node_defaults.sweeper_heartbeat_ttl_secs
        );
        assert_eq!(
            node.sweeper_repush_interval_secs,
            node_defaults.sweeper_repush_interval_secs
        );
    }

    #[test]
    fn test_env_example_matches_shared_fallbacks() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let contents =
            std::fs::read_to_string(root.join(".env.example")).expect("应该读取 .env.example");

        let expected = [
            ("APP_BASE_URL", ""),
            ("KC__SERVER__BIND_ADDR", "0.0.0.0"),
            ("KC__SERVER__PORT", "3000"),
            ("KC__DATABASE__MAX_CONNECTIONS", "10"),
            ("KC__DATABASE__MIN_CONNECTIONS", "2"),
            ("KC__DATABASE__CONNECT_TIMEOUT_SECS", "30"),
            ("KC__DATABASE__IDLE_TIMEOUT_SECS", "600"),
            ("KC__DATABASE__MAX_LIFETIME_SECS", "1800"),
            ("KC__REDIS__POOL_SIZE", "10"),
            ("KC__REDIS__CONNECT_TIMEOUT_SECS", "5"),
            ("KC__AUTH__JWT_SECRET", DEFAULT_JWT_SECRET),
            ("KC__AUTH__JWT_ISSUER", "keycompute"),
            ("KC__AUTH__JWT_EXPIRY_SECS", "3600"),
            ("KC__EMAIL__SMTP_PORT", "465"),
            ("KC__EMAIL__FROM_NAME", "KeyCompute"),
            ("KC__EMAIL__TIMEOUT_SECS", "30"),
            ("KC__EMAIL__USE_TLS", "true"),
            ("KC__GATEWAY__TIMEOUT_SECS", "120"),
            ("KC__GATEWAY__REQUEST_TIMEOUT_SECS", "120"),
            ("KC__GATEWAY__STREAM_TIMEOUT_SECS", "600"),
            ("KC__GATEWAY__MAX_RETRIES", "3"),
            ("KC__GATEWAY__ENABLE_FALLBACK", "true"),
            ("KC__GATEWAY__MONITORING_RAW_MAX_HOURS", "24"),
            ("KC__GATEWAY__ACCOUNT_PROBE_INTERVAL_SECS", "0"),
            ("KC__GATEWAY__ACCOUNT_PROBE_CONCURRENCY", "4"),
            ("KC__DATABASE_ROUTING__STRATEGY", "round_robin"),
            ("KC__DATABASE_READ__MAX_CONNECTIONS", "10"),
            ("KC__DATABASE_READ__MIN_CONNECTIONS", "1"),
            ("KC__DATABASE_READ__CONNECT_TIMEOUT_SECS", "5"),
            ("KC__DATABASE_READ__IDLE_TIMEOUT_SECS", "600"),
            ("KC__DATABASE_READ__ACQUIRE_TIMEOUT_SECS", "10"),
            ("KC__DATABASE_READ__MAX_LIFETIME_SECS", "1800"),
            ("KC__DATABASE_ROUTING__RETRY_ATTEMPTS", "2"),
            ("KC__DATABASE_ROUTING__CIRCUIT_BREAK_MS", "30000"),
            ("KC__DATABASE_ROUTING__FALLBACK_TO_WRITE", "true"),
            ("KC__DATABASE_ROUTING__HEALTH_CHECK_INTERVAL_SECS", "15"),
            ("KC__DEFAULT_ADMIN_EMAIL", DEFAULT_ADMIN_EMAIL),
            ("KC__DEFAULT_ADMIN_PASSWORD", DEFAULT_ADMIN_PASSWORD),
            (
                "KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET",
                DEFAULT_REGISTRATION_TOKEN_SECRET,
            ),
            ("KC__NODE_GATEWAY__SESSION_TTL_SECS", "300"),
            ("KC__NODE_GATEWAY__HEARTBEAT_INTERVAL_SECS", "30"),
            ("KC__NODE_GATEWAY__POLL_TIMEOUT_SECS", "30"),
            ("KC__NODE_GATEWAY__TASK_DEADLINE_SECS", "120"),
            ("KC__NODE_GATEWAY__COMPLETE_GRACE_SECS", "60"),
            ("KC__NODE_GATEWAY__NODE_FAILURE_THRESHOLD", "3"),
            ("KC__NODE_GATEWAY__TASK_FAILURE_THRESHOLD", "3"),
            ("KC__NODE_GATEWAY__SWEEPER_HEARTBEAT_TTL_SECS", "600"),
            ("KC__NODE_GATEWAY__SWEEPER_REPUSH_INTERVAL_SECS", "10"),
        ];

        for (key, value) in expected {
            assert_eq!(env_example_value(&contents, key), Some(value), "{key}");
        }

        assert_eq!(
            active_env_example_value(&contents, "KC__CRYPTO__SECRET_KEY"),
            None,
            "Crypto 示例不应注入无效占位密钥"
        );
    }

    #[test]
    fn test_compose_security_examples_match_the_production_contract() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for file in ["docker-compose.yml", "docker-compose.replicas.yml"] {
            let contents = std::fs::read_to_string(root.join(file)).expect("应该读取 Compose 文件");
            assert!(
                contents.contains(
                    "${POSTGRES_USER:-keycompute}:${POSTGRES_PASSWORD:-change-me-strong-password}@"
                ),
                "{file} 中服务端数据库 URL 必须与 PostgreSQL 容器使用相同的密码回退值"
            );
            assert!(contents.contains(&format!("${{KC__AUTH__JWT_SECRET:-{DEFAULT_JWT_SECRET}}}")));
            assert!(contents.contains(&format!(
                "${{KC__DEFAULT_ADMIN_EMAIL:-{DEFAULT_ADMIN_EMAIL}}}"
            )));
            assert!(
                contents.contains("KC__DEFAULT_ADMIN_PASSWORD: ${KC__DEFAULT_ADMIN_PASSWORD:-}")
            );
            assert!(contents.contains(&format!(
                "${{KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET:-{DEFAULT_REGISTRATION_TOKEN_SECRET}}}"
            )));
            assert!(contents.contains("${KC__CRYPTO__SECRET_KEY:-}"));
            assert!(contents.contains("KC__EMAIL__SMTP_HOST: ${KC__EMAIL__SMTP_HOST:-}"));
            assert!(contents.contains("APP_BASE_URL: ${APP_BASE_URL:-}"));
            assert!(
                contents.contains("本编排启用")
                    && contents.contains("Redis")
                    && contents.contains("全新数据库"),
                "{file} 必须说明节点密钥和管理员引导密码的条件"
            );
            assert!(
                contents.contains("不会被应用统一拦截"),
                "{file} 不得暗示所有 change-me 凭据都会被应用拒绝"
            );
        }
    }

    #[test]
    fn security_templates_document_the_fail_closed_policy() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let env_example = std::fs::read_to_string(root.join(".env.example")).unwrap();
        let config_example = std::fs::read_to_string(root.join("config.example.toml")).unwrap();
        let contributing = std::fs::read_to_string(root.join("CONTRIBUTING.md")).unwrap();

        for contents in [&env_example, &config_example] {
            assert!(contents.contains("配置 Redis"));
            assert!(contents.contains("首次创建 system 管理员"));
            assert!(contents.contains("不会") && contents.contains("统一拦截"));
        }
        assert!(contributing.contains("When Redis enables Node Gateway"));
        assert!(contributing.contains("Only the first\n  `system` administrator bootstrap"));
        assert!(contributing.contains("are not covered by\n  the application placeholder checks"));
        assert!(contributing.contains(
            "docker compose --env-file .env.example -f docker-compose.yml -f docker-compose.dev.yml"
        ));
    }

    #[test]
    fn documentation_contract_changes_trigger_ci() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let workflow =
            std::fs::read_to_string(root.join(".github/workflows/keycompute.yml")).unwrap();

        assert_eq!(workflow.matches("- \"README*.md\"").count(), 2);
        assert_eq!(workflow.matches("- \"CONTRIBUTING.md\"").count(), 2);
    }

    #[test]
    fn localized_readmes_document_the_production_security_contract() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let obsolete_default_login_guidance = [
            "Default account: `admin@keycompute.local`, password:",
            "初始账号：`admin@keycompute.local`，密码：",
            "預設帳號：`admin@keycompute.local`，密碼：",
            "Cuenta predeterminada: `admin@keycompute.local`, contraseña:",
            "الحساب الافتراضي: `admin@keycompute.local`، كلمة المرور:",
        ];

        for file in [
            "README.md",
            "README.zh-CN.md",
            "README.zh-TW.md",
            "README.es.md",
            "README.ar.md",
        ] {
            let contents = std::fs::read_to_string(root.join(file)).unwrap();
            assert!(contents.contains("config.example.toml"), "{file}");
            assert!(contents.contains("docker-compose.dev.yml"), "{file}");
            assert!(
                contents.contains(
                    "docker compose --env-file .env.example -f docker-compose.yml -f docker-compose.dev.yml"
                ),
                "{file}: 本地依赖必须显式使用示例环境，不能隐式读取生产 .env"
            );
            assert!(!contents.contains("set -a && source .env"), "{file}");
            assert!(
                obsolete_default_login_guidance
                    .iter()
                    .all(|obsolete| !contents.contains(obsolete)),
                "{file} 仍在指导生产环境使用默认管理员密码登录"
            );

            let jwt = readme_config_row(&contents, "KC__AUTH__JWT_SECRET").unwrap();
            let crypto = readme_config_row(&contents, "KC__CRYPTO__SECRET_KEY").unwrap();
            let node = readme_config_row(&contents, "KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET")
                .unwrap();
            let app_base_url = readme_config_row(&contents, "APP_BASE_URL").unwrap();
            let admin = readme_config_row(&contents, "KC__DEFAULT_ADMIN_PASSWORD").unwrap();
            assert!(
                contents.contains("cargo run -p keycompute-server --release"),
                "{file} 未说明 release 生产启动方式"
            );
            assert!(jwt.contains("32"), "{file}: JWT 门槛未记录");
            assert!(crypto.contains("Base64") && crypto.contains("32"), "{file}");
            assert!(node.contains("Redis") && node.contains("16"), "{file}");
            assert!(admin.contains("system") && admin.contains("12"), "{file}");
            assert!(
                !node.ends_with("✅ |"),
                "{file}: 节点密钥不应标成无条件必填"
            );
            assert!(
                !admin.ends_with("⚪ |"),
                "{file}: 管理员首启密码不应标成无条件可选"
            );
            assert!(
                !app_base_url.ends_with("⚪ |"),
                "{file}: 启用邮件或公开邀请时 APP_BASE_URL 是条件必填"
            );
        }
    }

    #[test]
    fn development_validation_keeps_examples_runnable() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    #[serial]
    fn test_config_from_env() {
        let _env = EnvVarGuard::set(&[
            ("KC__SERVER__PORT", "8080"),
            ("APP_BASE_URL", "http://localhost"),
            ("KC__EMAIL__SMTP_HOST", "localhost"),
            ("KC__EMAIL__SMTP_USERNAME", "test"),
            ("KC__EMAIL__SMTP_PASSWORD", "test"),
            ("KC__EMAIL__FROM_ADDRESS", "test@localhost"),
            (
                "KC__DATABASE_READ_URLS",
                "postgres://reader-1/db,postgres://reader-2/db",
            ),
            ("KC__DATABASE_ROUTING__READ_WEIGHTS", "1,2"),
            ("KC__REDIS__URL", "redis://redis.internal:6379"),
            ("KC__CRYPTO__SECRET_KEY", ""),
        ]);

        let config = AppConfig::from_env().expect("应该从环境变量加载配置");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.app_base_url.as_deref(), Some("http://localhost"));
        assert_eq!(config.database_read_urls.len(), 2);
        assert_eq!(config.database_routing.read_weights, vec![1, 2]);
        let redis = config
            .redis
            .expect("Redis URL should enable Redis configuration");
        assert_eq!(redis.pool_size, 10);
        assert_eq!(redis.connect_timeout_secs, 5);
        assert!(config.crypto.is_none(), "空 Crypto 环境变量应按未配置处理");

        unsafe {
            std::env::set_var("KC__DATABASE_ROUTING__READ_WEIGHTS", "");
        }
        let config = AppConfig::from_env().expect("空列表环境变量应按未设置处理");
        assert!(config.database_routing.read_weights.is_empty());
    }

    #[test]
    #[serial]
    fn development_file_loader_ignores_production_environment() {
        let _env = EnvVarGuard::set(&[
            ("KC__SERVER__PORT", "8080"),
            ("APP_BASE_URL", "https://env.example.com"),
        ]);

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.toml");
        let config = AppConfig::from_file(path).expect("开发配置文件应可加载");
        assert_eq!(config.server.port, 3000);
        assert_eq!(
            config.app_base_url.as_deref(),
            Some("http://localhost:8080")
        );
    }

    #[test]
    #[serial]
    fn production_loader_treats_blank_compose_email_values_as_disabled() {
        let compose_email_env = [
            (
                "KC__AUTH__JWT_SECRET",
                "compose-production-jwt-secret-at-least-32-bytes",
            ),
            (
                "KC__CRYPTO__SECRET_KEY",
                "dGVzdC1rZXktZm9yLXByb2R1Y3Rpb24tMzItYnl0ZXM=",
            ),
            ("KC__REDIS__URL", "redis://:secret@redis:6379"),
            (
                "KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET",
                "compose-production-node-secret",
            ),
            ("KC__EMAIL__SMTP_HOST", ""),
            ("KC__EMAIL__SMTP_PORT", "465"),
            ("KC__EMAIL__SMTP_USERNAME", ""),
            ("KC__EMAIL__SMTP_PASSWORD", ""),
            ("KC__EMAIL__FROM_ADDRESS", ""),
            ("KC__EMAIL__FROM_NAME", "KeyCompute"),
            ("KC__EMAIL__TIMEOUT_SECS", "30"),
            ("KC__EMAIL__USE_TLS", "true"),
            ("KC__EMAIL__REQUIREMENT_RECIPIENT", ""),
            ("APP_BASE_URL", ""),
        ];

        let _env = EnvVarGuard::set(&compose_email_env);

        let config =
            AppConfig::load_production().expect("Compose 空 SMTP 变量应加载为禁用邮件的生产配置");
        assert_eq!(config.email, EmailConfig::default());
        assert!(!config.email.is_configured());
        assert!(config.app_base_url.is_none());
        config
            .validate_for_production()
            .expect("其他生产密钥有效时，空 SMTP 和空 APP_BASE_URL 不应阻止生产启动");
    }

    #[test]
    #[serial]
    fn production_loader_keeps_email_disabled_when_any_required_value_is_blank() {
        const SMTP_HOST: &str = "KC__EMAIL__SMTP_HOST";
        const SMTP_USERNAME: &str = "KC__EMAIL__SMTP_USERNAME";
        const SMTP_PASSWORD: &str = "KC__EMAIL__SMTP_PASSWORD";
        const FROM_ADDRESS: &str = "KC__EMAIL__FROM_ADDRESS";

        for blank_key in [SMTP_HOST, SMTP_USERNAME, SMTP_PASSWORD, FROM_ADDRESS] {
            let values = [
                (
                    "KC__AUTH__JWT_SECRET",
                    "email-test-jwt-secret-at-least-32-bytes",
                ),
                (
                    "KC__CRYPTO__SECRET_KEY",
                    "dGVzdC1rZXktZm9yLXByb2R1Y3Rpb24tMzItYnl0ZXM=",
                ),
                (
                    SMTP_HOST,
                    if blank_key == SMTP_HOST {
                        ""
                    } else {
                        "smtp.example.com"
                    },
                ),
                (
                    SMTP_USERNAME,
                    if blank_key == SMTP_USERNAME {
                        ""
                    } else {
                        "mailer"
                    },
                ),
                (
                    SMTP_PASSWORD,
                    if blank_key == SMTP_PASSWORD {
                        ""
                    } else {
                        "secret"
                    },
                ),
                (
                    FROM_ADDRESS,
                    if blank_key == FROM_ADDRESS {
                        ""
                    } else {
                        "noreply@example.com"
                    },
                ),
                ("APP_BASE_URL", ""),
            ];
            let _env = EnvVarGuard::set(&values);

            let config = AppConfig::load_production()
                .expect("留空任一 SMTP 必填变量都应得到可加载的禁用配置");
            assert!(
                !config.email.is_configured(),
                "{blank_key} 留空时不应启用邮件"
            );
            assert!(matches!(
                config.validate_for_production(),
                Err(ConfigLoadError::ValidationError(message))
                    if message.contains("KC__EMAIL__SMTP_HOST")
            ));
        }
    }

    #[test]
    #[serial]
    fn test_crypto_config_from_env() {
        let _env = EnvVarGuard::set(&[
            ("KC__CRYPTO__SECRET_KEY", "dGVzdC1rZXktZnJvbS1lbnY="),
            ("APP_BASE_URL", "http://localhost"),
            ("KC__EMAIL__SMTP_HOST", "localhost"),
            ("KC__EMAIL__SMTP_USERNAME", "test"),
            ("KC__EMAIL__SMTP_PASSWORD", "test"),
            ("KC__EMAIL__FROM_ADDRESS", "test@localhost"),
        ]);

        let config = AppConfig::from_env().expect("应该从环境变量加载配置");

        // 验证 crypto 配置被正确加载
        assert!(config.crypto.is_some(), "crypto 配置应该存在");
        let crypto = config.crypto.unwrap();
        assert!(crypto.has_key(), "crypto 应该有密钥");
        assert_eq!(crypto.secret_key(), Some("dGVzdC1rZXktZnJvbS1lbnY="));
    }

    #[test]
    fn test_validate_port_zero() {
        let mut config = AppConfig::default();
        config.server.port = 0;
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("端口"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_empty_database_url() {
        let mut config = AppConfig::default();
        config.database.url = "".to_string();
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("数据库 URL"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_database_pool_config() {
        let mut config = AppConfig::default();
        config.database.max_connections = 1;
        config.database.min_connections = 5;
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("最大连接数"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_database_connect_timeout_zero() {
        // 数据库连接超时为 0 应该报错
        let mut config = AppConfig::default();
        config.database.connect_timeout_secs = 0;
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("连接超时"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_gateway_zero_timeouts() {
        let cases = [
            (
                "timeout_secs",
                GatewayConfig {
                    timeout_secs: 0,
                    ..GatewayConfig::default()
                },
            ),
            (
                "request_timeout_secs",
                GatewayConfig {
                    request_timeout_secs: 0,
                    ..GatewayConfig::default()
                },
            ),
            (
                "stream_timeout_secs",
                GatewayConfig {
                    stream_timeout_secs: 0,
                    ..GatewayConfig::default()
                },
            ),
        ];

        for (field, gateway) in cases {
            let config = AppConfig {
                gateway,
                ..AppConfig::default()
            };
            assert!(
                matches!(
                    config.validate(),
                    Err(ConfigLoadError::ValidationError(message)) if message.contains(field)
                ),
                "expected {field}=0 to be rejected"
            );
        }
    }

    #[test]
    fn test_validate_monitoring_raw_max_hours_bounds() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.gateway.monitoring_raw_max_hours = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("monitoring_raw_max_hours")
        ));

        config.gateway.monitoring_raw_max_hours = 25;
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("monitoring_raw_max_hours")
        ));
    }

    #[test]
    fn test_validate_account_probe_settings() {
        let mut config = AppConfig::default();
        config.gateway.account_probe_interval_secs = 59;
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("account_probe_interval_secs")
        ));

        config.gateway.account_probe_interval_secs = 60;
        config.gateway.account_probe_concurrency = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("account_probe_concurrency")
        ));

        config.gateway.account_probe_concurrency = 32;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_gateway_proxy_rules() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.gateway.proxy = Some(ProxyConfig {
            providers: std::collections::HashMap::from([(
                "openai".to_string(),
                "not-a-proxy-url".to_string(),
            )]),
            accounts: None,
            patterns: None,
        });
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message)) if message.contains("proxy")
        ));

        config.gateway.proxy = Some(ProxyConfig {
            providers: std::collections::HashMap::new(),
            accounts: Some(std::collections::HashMap::from([(
                "openai:not-a-uuid".to_string(),
                "http://proxy.example:8080".to_string(),
            )])),
            patterns: None,
        });
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("provider:account_uuid")
        ));
    }

    #[test]
    fn test_validate_jwt_short_key() {
        // 短 JWT 密钥应该触发警告（但不报错）
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "short-key".to_string(); // 9 字符，< 32
        let result = config.validate();
        // 应该通过验证，但会有警告日志
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_valid_config() {
        let mut config = AppConfig::default();
        // 设置非默认的 JWT 密钥避免警告
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn production_validation_rejects_runnable_security_examples() {
        let config = AppConfig::default();
        assert!(matches!(
            config.validate_for_production(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("JWT_SECRET")
                    && message.contains("CRYPTO__SECRET_KEY")
                    && !message.contains("REGISTRATION_TOKEN_SECRET")
        ));
    }

    #[test]
    fn production_validation_rejects_blank_jwt_secret() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "                                ".to_string();
        config.crypto = Some(CryptoConfig {
            secret_key: Some("dGVzdC1rZXktZm9yLXByb2R1Y3Rpb24tMzItYnl0ZXM=".to_string()),
        });

        assert!(matches!(
            config.validate_for_production(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("JWT_SECRET") && message.contains("32 bytes")
        ));
    }

    #[test]
    fn production_validation_accepts_explicit_secrets() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-production".to_string();
        config.crypto = Some(CryptoConfig {
            secret_key: Some("dGVzdC1rZXktZm9yLXByb2R1Y3Rpb24tMzItYnl0ZXM=".to_string()),
        });
        config.node_gateway = Some(NodeGatewayConfig {
            registration_token_secret: Some("independent-node-registration-secret".to_string()),
            ..NodeGatewayConfig::default()
        });
        config.redis = Some(RedisConfig::default());

        assert!(config.validate_for_production().is_ok());
    }

    #[test]
    fn production_validation_allows_provider_only_without_node_secret() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-production".to_string();
        config.crypto = Some(CryptoConfig {
            secret_key: Some("dGVzdC1rZXktZm9yLXByb2R1Y3Rpb24tMzItYnl0ZXM=".to_string()),
        });

        assert!(config.redis.is_none());
        assert!(config.node_gateway.is_none());
        assert!(config.validate_for_production().is_ok());
    }

    #[test]
    fn production_validation_requires_node_secret_when_redis_enables_gateway() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-production".to_string();
        config.crypto = Some(CryptoConfig {
            secret_key: Some("dGVzdC1rZXktZm9yLXByb2R1Y3Rpb24tMzItYnl0ZXM=".to_string()),
        });
        config.redis = Some(RedisConfig::default());

        assert!(matches!(
            config.validate_for_production(),
            Err(ConfigLoadError::ValidationError(message))
                if message.contains("REGISTRATION_TOKEN_SECRET")
        ));
    }

    #[test]
    fn test_validate_app_base_url_is_required_when_email_enabled() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.app_base_url = None;
        config.email.smtp_host = "localhost".to_string();
        config.email.smtp_username = "mailer".to_string();
        config.email.smtp_password = "secret".to_string();
        config.email.from_address = "noreply@example.com".to_string();

        let result = config.validate();
        assert!(matches!(
            result,
            Err(ConfigLoadError::ValidationError(message)) if message.contains("APP_BASE_URL")
        ));
        assert_eq!(config.resolved_app_base_url(), None);
    }

    #[test]
    fn test_resolved_app_base_url_remains_unconfigured_when_missing() {
        let config = AppConfig {
            app_base_url: None,
            server: ServerConfig {
                port: 8088,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(config.resolved_app_base_url(), None);
    }

    #[test]
    fn test_validate_app_base_url_requires_supported_scheme() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.app_base_url = Some("ftp://example.com".to_string());

        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("http/https"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_app_base_url_requires_https_for_non_local_hosts() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.app_base_url = Some("http://example.com".to_string());

        let result = config.validate();
        assert!(matches!(
            result,
            Err(ConfigLoadError::ValidationError(message)) if message.contains("https")
        ));
    }

    #[test]
    fn test_validate_app_base_url_accepts_local_http() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.app_base_url = Some("http://localhost:3000/base/".to_string());

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_app_base_url_rejects_embedded_credentials() {
        let config = AppConfig {
            app_base_url: Some("https://user:secret@example.com".to_string()),
            ..AppConfig::default()
        };

        let result = config.validate();
        assert!(matches!(
            result,
            Err(ConfigLoadError::ValidationError(message)) if message.contains("用户名或密码")
        ));
    }

    #[test]
    fn test_validate_database_max_connections_zero() {
        // 数据库最大连接数为 0 应该报错
        let mut config = AppConfig::default();
        config.database.max_connections = 0;
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("最大连接数"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_smtp_port_zero_is_advisory() {
        let mut config = AppConfig::default();
        config.email.smtp_port = 0;
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_redis_url_empty() {
        // Redis URL 为空应该报错
        let config = AppConfig {
            redis: Some(RedisConfig {
                url: "".to_string(),
                pool_size: 10,
                connect_timeout_secs: 5,
            }),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("Redis URL"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_redis_pool_settings() {
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.redis = Some(RedisConfig::default());

        config.redis.as_mut().unwrap().pool_size = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message)) if message.contains("连接池大小")
        ));

        let redis = config.redis.as_mut().unwrap();
        redis.pool_size = 10;
        redis.connect_timeout_secs = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message)) if message.contains("连接超时")
        ));
    }

    #[test]
    fn test_account_probe_uses_a_dedicated_lock_connection() {
        let mut config = AppConfig::default();
        config.gateway.account_probe_interval_secs = 60;
        config.database.max_connections = 1;
        config.database.min_connections = 1;

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_redis_backed_node_gateway_requires_two_write_connections() {
        let mut config = AppConfig {
            redis: Some(RedisConfig::default()),
            database: DatabaseConfig {
                max_connections: 2,
                min_connections: 1,
                ..DatabaseConfig::default()
            },
            ..AppConfig::default()
        };
        assert!(config.validate().is_ok());

        config.database.max_connections = 1;
        assert!(matches!(
            config.validate(),
            Err(ConfigLoadError::ValidationError(message)) if message.contains("Node Gateway")
        ));
    }

    #[test]
    fn test_validate_jwt_expiry_zero() {
        // JWT 过期时间为 0 应该报错
        let mut config = AppConfig::default();
        config.auth.jwt_expiry_secs = 0;
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("JWT 过期时间"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_gateway_max_retries_zero() {
        // 最大重试次数为 0 应该警告但不报错
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.gateway.max_retries = 0;
        let result = config.validate();
        // 应该通过验证，但会有警告日志
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_bind_addr_empty() {
        // 服务器绑定地址为空应该报错
        let mut config = AppConfig::default();
        config.server.bind_addr = "".to_string();
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("绑定地址"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_email_from_address_empty_is_advisory() {
        let mut config = AppConfig::default();
        config.email.smtp_host = "smtp.example.com".to_string();
        config.email.smtp_username = "mailer".to_string();
        config.email.smtp_password = "secret".to_string();
        config.email.from_address = "".to_string();
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_email_from_address_invalid() {
        // Email 发件人地址缺少 @ 符号应该警告但不报错
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "a-very-secure-jwt-secret-key-for-testing".to_string();
        config.email.smtp_host = "smtp.example.com".to_string();
        config.email.smtp_username = "mailer".to_string();
        config.email.smtp_password = "secret".to_string();
        config.email.from_address = "invalid-email".to_string();
        config.app_base_url = Some("https://app.example.com".to_string());
        let result = config.validate();
        // 应该通过验证，但会有警告日志
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_jwt_issuer_empty() {
        // JWT 签发者为空应该报错
        let mut config = AppConfig::default();
        config.auth.jwt_issuer = "".to_string();
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(ConfigLoadError::ValidationError(msg)) => {
                assert!(msg.contains("签发者"));
            }
            _ => panic!("期望 ValidationError"),
        }
    }

    #[test]
    fn test_validate_smtp_host_empty_is_advisory() {
        let mut config = AppConfig::default();
        config.email.smtp_host = "".to_string();
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_email_whitespace_username_is_advisory() {
        let mut config = AppConfig::default();
        config.email.smtp_host = "smtp.example.com".to_string();
        config.email.smtp_username = "   ".to_string();
        config.email.smtp_password = "secret".to_string();
        config.email.from_address = "noreply@example.com".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }
}
