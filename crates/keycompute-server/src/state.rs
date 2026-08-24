//! 应用状态
//!
//! AppState 定义（DB Pool, Redis, 各模块 Handle）

use keycompute_auth::{
    AuthService, EmailService, JwtValidator, ProduceAiKeyValidator, UserService,
};
use keycompute_billing::BillingService;
use keycompute_cache::CacheService;
use keycompute_db::DbRouter;
use keycompute_emailserver::EmailConfig;
use keycompute_routing::{AccountStateStore, ProviderHealthStore, RoutingEngine};
use keycompute_runtime::set_global_crypto;
use keycompute_types::{NoopRequestLifecycleRecorder, RequestLifecycleRecorder};
use llm_gateway::{GatewayBuilder, GatewayExecutor, HttpProxy, ProxyConfig as HttpProxyConfig};
use llm_protocol_provider::ProviderAdapter;
use node_gateway::{NodeGatewayService, PostgresNodeIndex};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// 限流后端配置
#[derive(Debug, Clone, Default)]
pub enum RateLimitBackendConfig {
    /// 内存后端
    #[default]
    Memory,
    /// Redis 后端
    Redis {
        url: String,
        pool_size: usize,
        connect_timeout: Duration,
    },
}

/// JWT 配置
#[derive(Debug, Clone)]
pub struct JwtConfig {
    /// JWT 密钥
    pub secret: String,
    /// JWT 签发者
    pub issuer: String,
    /// JWT 过期时间（秒）
    pub expiry_secs: i64,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret: "change-me-in-production".to_string(),
            issuer: "keycompute".to_string(),
            expiry_secs: 3600,
        }
    }
}

/// 应用状态配置
#[derive(Debug, Clone, Default)]
pub struct AppStateConfig {
    /// 对外公开的前端应用基础 URL（可选）
    pub app_base_url: Option<String>,
    /// 限流后端配置
    pub rate_limit: RateLimitBackendConfig,
    /// JWT 配置
    pub jwt: JwtConfig,
    /// Gateway 配置
    pub gateway: keycompute_config::GatewayConfig,
    /// 邮件服务配置
    pub email: EmailConfig,
    /// 节点网关配置（可选）
    pub node_gateway: Option<keycompute_config::NodeGatewayConfig>,
}

impl AppStateConfig {
    /// 从 keycompute_config::AppConfig 创建
    pub fn from_config(config: &keycompute_config::AppConfig) -> Self {
        Self {
            app_base_url: Some(config.resolved_app_base_url()),
            rate_limit: if let Some(redis) = &config.redis {
                RateLimitBackendConfig::Redis {
                    url: redis.url.clone(),
                    pool_size: redis.pool_size as usize,
                    connect_timeout: Duration::from_secs(redis.connect_timeout_secs),
                }
            } else {
                RateLimitBackendConfig::Memory
            },
            jwt: JwtConfig {
                secret: config.auth.jwt_secret.clone(),
                issuer: config.auth.jwt_issuer.clone(),
                expiry_secs: config.auth.jwt_expiry_secs as i64,
            },
            gateway: config.gateway.clone(),
            email: config.email.clone(),
            node_gateway: config.node_gateway.clone(),
        }
    }
}

/// 初始化全局加密密钥
///
/// 从配置中读取加密密钥并设置全局加密器。
/// 应在应用启动时调用一次。
///
/// # 参数
/// - `config`: 应用配置
/// - `is_production`: 是否为非开发环境；用于调整缺失密钥时的告警等级
///
/// # 返回
/// - `Ok(())`: 成功初始化，或未配置密钥并回退到明文存储
/// - `Err(...)`: 已提供密钥但格式错误
///
/// # 示例
/// ```rust,ignore
/// let config = AppConfig::load()?;
/// init_global_crypto(&config, true)?;
/// let state = AppState::with_config(AppStateConfig::from_config(&config));
/// ```
pub fn init_global_crypto(
    config: &keycompute_config::AppConfig,
    is_production: bool,
) -> crate::error::Result<()> {
    let key = config
        .crypto
        .as_ref()
        .filter(|crypto| crypto.has_key())
        .and_then(|crypto| crypto.secret_key());

    let Some(key) = key else {
        if is_production {
            return Err(crate::error::ApiError::Config(
                "KC__CRYPTO__SECRET_KEY is required in production; refusing plaintext Provider API key storage".to_string(),
            ));
        } else {
            tracing::warn!("未配置 KC__CRYPTO__SECRET_KEY，Provider API Key 将以明文存储");
        }
        return Ok(());
    };

    set_global_crypto(key).map_err(|e| {
        crate::error::ApiError::Config(format!("Failed to set global crypto key: {}", e))
    })?;
    tracing::info!("Global crypto key initialized from config");
    Ok(())
}

/// 应用状态
#[derive(Clone)]
pub struct AppState {
    /// 对外公开的前端应用基础 URL（可选）
    pub app_base_url: Option<String>,
    /// 数据库连接池（可选）
    pub pool: Option<Arc<DbRouter>>,
    /// 认证服务
    pub auth: Arc<AuthService>,
    /// 限流服务
    pub rate_limiter: Arc<keycompute_ratelimit::RateLimitService>,
    /// 定价服务
    pub pricing: Arc<keycompute_pricing::PricingService>,
    /// 运行时状态存储（账号状态）
    pub account_states: Arc<AccountStateStore>,
    /// Provider 健康状态存储
    pub provider_health: Arc<ProviderHealthStore>,
    /// 路由引擎
    pub routing: Arc<RoutingEngine>,
    /// Gateway 执行器（唯一执行层）
    pub gateway: Arc<GatewayExecutor>,
    /// Internal HTTP Proxy（统一上游连接管理）
    pub http_proxy: Arc<HttpProxy>,
    /// 计费服务
    pub billing: Arc<BillingService>,
    /// 邮件服务
    pub email_service: Arc<EmailService>,
    /// 公共注册 cookie 签名密钥
    pub public_auth_cookie_secret: Arc<String>,
    /// 统一支付渠道注册表（可选）
    pub payment: Option<Arc<crate::payment_registry::PaymentRegistry>>,
    /// 节点网关服务（可选）
    pub node_gateway: Option<Arc<NodeGatewayService>>,
    /// 统一缓存服务（Redis 不可用时自动降级为 no-op）
    pub cache: Arc<CacheService>,
    /// Gateway 配置
    pub gateway_config: keycompute_config::GatewayConfig,
    /// Best-effort lifecycle tracing sink.
    pub lifecycle: Arc<dyn RequestLifecycleRecorder>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("app_base_url", &self.app_base_url)
            .field("pool", &self.pool.as_ref().map(|_| "DbRouter"))
            .field("auth", &"<AuthService>")
            .field("rate_limiter", &"<RateLimitService>")
            .field("pricing", &"<PricingService>")
            .field("account_states", &self.account_states)
            .field("provider_health", &"<ProviderHealthStore>")
            .field("routing", &"<RoutingEngine>")
            .field("gateway", &"<GatewayExecutor>")
            .field("http_proxy", &"<HttpProxy>")
            .field("billing", &"<BillingService>")
            .field("email_service", &"<EmailService>")
            .field("public_auth_cookie_secret", &"<secret>")
            .field(
                "payment",
                &self.payment.as_ref().map(|_| "<PaymentService>"),
            )
            .field(
                "node_gateway",
                &self.node_gateway.as_ref().map(|_| "<NodeGatewayService>"),
            )
            .field("cache", &"<CacheService>")
            .field("gateway_config", &self.gateway_config)
            .field("lifecycle", &"<RequestLifecycleRecorder>")
            .finish()
    }
}

impl AppState {
    /// 创建新的应用状态（无数据库连接，使用默认配置）
    pub fn new() -> Self {
        Self::with_config(AppStateConfig::default())
    }

    /// 创建带配置的应用状态（无数据库连接）
    pub fn with_config(config: AppStateConfig) -> Self {
        // 创建 API Key 验证器
        let api_key_validator = ProduceAiKeyValidator::new();
        // 创建 JWT 验证器
        let jwt_validator = JwtValidator::new(&config.jwt.secret, &config.jwt.issuer)
            .with_expiration(config.jwt.expiry_secs);
        // 创建 AuthService，同时支持 API Key 和 JWT 认证
        let auth_service = AuthService::new(api_key_validator).with_jwt(jwt_validator);

        // 创建定价服务
        let pricing_service = keycompute_pricing::PricingService::new();

        // 创建运行时状态存储
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        // 获取 Provider 名称列表（与 Gateway 使用一致的列表）
        let provider_names = crate::providers::get_provider_names();

        // 创建路由引擎（集成 ProviderHealthStore 和 AccountStateStore）
        let routing_engine = Arc::new(RoutingEngine::new(
            Arc::clone(&account_states),
            Arc::clone(&provider_health),
            provider_names,
        ));

        // 创建 Internal HTTP Proxy（统一上游连接管理，支持配置）
        let http_proxy = Arc::new(Self::create_http_proxy(
            config.gateway.proxy.as_ref(),
            &config.gateway,
        ));

        // 创建 Gateway 执行器，使用 providers 模块统一的 Provider 列表
        let mut gateway_builder = GatewayBuilder::new()
            .with_config(Self::gateway_executor_config(&config.gateway))
            .with_http_proxy(Arc::clone(&http_proxy));
        for (name, adapter) in crate::providers::get_provider_adapters() {
            gateway_builder = gateway_builder.add_provider(name, adapter);
        }
        let gateway = Arc::new(gateway_builder.build());

        // 创建计费服务
        let billing = Arc::new(BillingService::new());

        // 根据配置创建限流服务
        let rate_limiter = Self::create_rate_limiter(&config.rate_limit);

        // 创建缓存服务（降级为 no-op，因为无 Redis 连接池）
        let cache = Self::create_disabled_cache();

        // 将 PricingService 接入分布式缓存（确保代码路径覆盖）
        let pricing_service = pricing_service.with_dist_cache(Arc::clone(&cache));

        // 创建邮件服务
        let email_service = Arc::new(EmailService::new(config.email));
        let public_auth_cookie_secret =
            Arc::new(format!("{}:public-auth-cookie", config.jwt.secret));

        Self {
            app_base_url: config.app_base_url,
            pool: None,
            auth: Arc::new(auth_service),
            rate_limiter: Arc::new(rate_limiter),
            pricing: Arc::new(pricing_service),
            account_states: Arc::clone(&account_states),
            provider_health,
            routing: routing_engine,
            gateway,
            http_proxy,
            billing,
            email_service,
            public_auth_cookie_secret,
            payment: None,      // 支付服务需要数据库连接
            node_gateway: None, // 节点网关需要数据库连接和 Redis
            cache,
            gateway_config: config.gateway,
            lifecycle: Arc::new(NoopRequestLifecycleRecorder),
        }
    }

    /// 创建 disabled 缓存服务（总是 no-op 降级，无需 Redis 连接池）
    fn create_disabled_cache() -> Arc<CacheService> {
        tracing::info!("Cache service disabled");
        Arc::new(CacheService::disabled())
    }

    /// 创建缓存服务（Redis pool available 时使用共享连接池，否则 no-op 降级）
    #[cfg(feature = "redis")]
    fn create_cache_service(pool: Option<deadpool_redis::Pool>) -> Arc<CacheService> {
        match pool {
            Some(pool) => {
                tracing::info!("Cache service using shared Redis backend");
                Arc::new(CacheService::with_pool(pool))
            }
            None => {
                tracing::info!("Cache service disabled (Redis pool unavailable)");
                Arc::new(CacheService::disabled())
            }
        }
    }

    /// 根据配置创建限流服务
    ///
    /// 如果 Redis 后端创建失败，会优雅降级到内存后端，
    /// 确保应用不会因为 Redis 不可用而无法启动。
    /// Redis 后端内部使用 `deadpool_redis` 连接池管理连接。
    fn create_rate_limiter(
        config: &RateLimitBackendConfig,
    ) -> keycompute_ratelimit::RateLimitService {
        match config {
            RateLimitBackendConfig::Memory => {
                keycompute_ratelimit::RateLimitService::default_memory()
            }
            #[cfg(feature = "redis")]
            RateLimitBackendConfig::Redis {
                url,
                pool_size,
                connect_timeout,
            } => {
                match keycompute_runtime::redis_store::RedisRuntimeStore::create_pool_with_options(
                    url,
                    *pool_size,
                    *connect_timeout,
                ) {
                    Ok(pool) => {
                        tracing::info!(redis_url = %url, "Redis rate limiter initialized successfully");
                        keycompute_ratelimit::RateLimitService::with_redis_pool(pool)
                    }
                    Err(e) => {
                        tracing::warn!(
                            redis_url = %url,
                            error = %e,
                            "Failed to create Redis rate limiter, falling back to memory backend"
                        );
                        keycompute_ratelimit::RateLimitService::default_memory()
                    }
                }
            }
            #[cfg(not(feature = "redis"))]
            RateLimitBackendConfig::Redis { .. } => {
                tracing::warn!(
                    "Redis backend requested but redis feature is not enabled, falling back to memory backend"
                );
                keycompute_ratelimit::RateLimitService::default_memory()
            }
        }
    }

    fn gateway_executor_config(
        gateway_config: &keycompute_config::GatewayConfig,
    ) -> llm_gateway::GatewayConfig {
        llm_gateway::GatewayConfig {
            max_retries: gateway_config.max_retries,
            timeout_secs: gateway_config.timeout_secs,
            enable_fallback: gateway_config.enable_fallback,
        }
    }

    /// 创建 HTTP Proxy（支持从配置读取代理设置）
    fn create_http_proxy(
        proxy_config: Option<&keycompute_config::ProxyConfig>,
        gateway_config: &keycompute_config::GatewayConfig,
    ) -> HttpProxy {
        // 创建 HTTP Proxy 配置
        let http_proxy_config = HttpProxyConfig::default()
            .with_request_timeout(Duration::from_secs(gateway_config.request_timeout_secs))
            .with_stream_timeout(Duration::from_secs(gateway_config.stream_timeout_secs));

        if let Some(proxy) = proxy_config {
            // Build each rule into its native selector tier. Encoding account
            // and pattern rules into the provider map leaves them unreachable.
            let mut http_proxy = HttpProxy::new(http_proxy_config);
            for (provider, url) in &proxy.providers {
                http_proxy.add_proxy(provider.clone(), url.clone());
            }

            if let Some(patterns) = &proxy.patterns {
                for (pattern, url) in patterns {
                    http_proxy.add_pattern(pattern.clone(), url.clone());
                }
            }

            if let Some(accounts) = &proxy.accounts {
                for (key, url) in accounts {
                    let Some((provider, account_id)) = key.rsplit_once(':') else {
                        tracing::warn!(proxy_account_key=%key, "ignoring invalid account proxy key");
                        continue;
                    };
                    let Ok(account_id) = uuid::Uuid::parse_str(account_id) else {
                        tracing::warn!(proxy_account_key=%key, "ignoring invalid account proxy UUID");
                        continue;
                    };
                    http_proxy.add_account_proxy(provider.to_string(), account_id, url.clone());
                }
            }
            http_proxy
        } else {
            // 无代理配置，使用默认 HttpProxy
            HttpProxy::new(http_proxy_config)
        }
    }

    /// 创建 Node Gateway 服务
    ///
    /// 使用已有 Redis 连接池创建，与限流服务共享同一连接池。
    /// 仅在启用 `redis` feature 时可用。
    #[cfg(feature = "redis")]
    fn create_node_gateway_with_pool(
        router: &Arc<DbRouter>,
        redis_pool: deadpool_redis::Pool,
        node_config: Option<keycompute_config::NodeGatewayConfig>,
    ) -> Result<NodeGatewayService, anyhow::Error> {
        use keycompute_runtime::redis_store::RedisRuntimeStore;
        use node_gateway::{NodeGatewayAppConfig, NodeGatewayRedis, NodeGatewayStore};

        // 使用已有连接池创建 Redis 存储（与限流服务共享连接池）
        let redis_store = Arc::new(RedisRuntimeStore::with_pool(redis_pool));

        // 创建节点网关配置。缺失的安全密钥使用可运行的示例值并记录安全建议。
        let config = NodeGatewayAppConfig::from_config(&node_config.unwrap_or_default());

        // 创建 Store 和 Redis 实例
        let store = NodeGatewayStore::new(Arc::clone(router), config.clone());
        let redis = NodeGatewayRedis::new(redis_store);

        // 创建 NodeGatewayService
        Ok(NodeGatewayService::new(store, redis, config))
    }

    /// 创建带数据库连接的应用状态（使用默认配置）
    pub fn with_pool(pool: Arc<DbRouter>) -> Self {
        Self::with_pool_and_config(pool, AppStateConfig::default())
    }

    /// 创建带数据库连接和自定义配置的应用状态
    pub fn with_pool_and_config(pool: Arc<DbRouter>, config: AppStateConfig) -> Self {
        // 创建带数据库连接的 API Key 验证器
        let api_key_validator = ProduceAiKeyValidator::with_pool(Arc::clone(&pool));
        // 创建 JWT 验证器
        let jwt_validator = JwtValidator::new(&config.jwt.secret, &config.jwt.issuer)
            .with_expiration(config.jwt.expiry_secs);
        // 创建 AuthService，同时支持 API Key 和 JWT 认证。
        // 关键：注入带数据库连接的 UserService，使 verify_token 能对 JWT 的
        // token_version 做数据库比对——否则密码重置/登出后的旧 access token
        // 仍能通过认证，token_version 失效机制形同虚设。
        let auth_service = AuthService::new(api_key_validator)
            .with_jwt(jwt_validator)
            .with_user_service(UserService::with_pool(Arc::clone(&pool)));

        // 创建带数据库连接的定价服务
        let pricing_service = keycompute_pricing::PricingService::with_pool(Arc::clone(&pool));

        // 创建运行时状态存储
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        // 获取 Provider 名称列表（与 Gateway 使用一致的列表）
        let provider_names = crate::providers::get_provider_names();

        // 创建 Node 能力索引
        let node_index = Arc::new(PostgresNodeIndex::new(Arc::clone(&pool)));

        // 创建带数据库连接和 Node 能力索引的路由引擎
        let routing_engine = Arc::new(RoutingEngine::with_node_index(
            Arc::clone(&account_states),
            Arc::clone(&provider_health),
            Arc::clone(&pool),
            provider_names,
            node_index,
        ));

        // 创建 Internal HTTP Proxy（统一上游连接管理，支持配置）
        let http_proxy = Arc::new(Self::create_http_proxy(
            config.gateway.proxy.as_ref(),
            &config.gateway,
        ));

        // 创建 Gateway 执行器，使用 providers 模块统一的 Provider 列表
        let mut gateway_builder = GatewayBuilder::new()
            .with_config(Self::gateway_executor_config(&config.gateway))
            .with_http_proxy(Arc::clone(&http_proxy));
        for (name, adapter) in crate::providers::get_provider_adapters() {
            gateway_builder = gateway_builder.add_provider(name, adapter);
        }
        let gateway = Arc::new(gateway_builder.build());

        // 创建带数据库连接的计费服务
        let billing = Arc::new(BillingService::with_pool(Arc::clone(&pool)));

        let database: Arc<dyn RequestLifecycleRecorder> = Arc::new(
            keycompute_db::PostgresRequestLifecycleRecorder::new(Arc::clone(&pool)),
        );
        let lifecycle: Arc<dyn RequestLifecycleRecorder> =
            Arc::new(crate::lifecycle_metrics::MetricsRequestLifecycleRecorder::new(database));

        #[cfg(feature = "redis")]
        let (rate_limiter, node_gateway, cache) = {
            let shared_redis_pool = match &config.rate_limit {
                RateLimitBackendConfig::Redis {
                    url,
                    pool_size,
                    connect_timeout,
                } => {
                    match keycompute_runtime::redis_store::RedisRuntimeStore::create_pool_with_options(
                        url,
                        *pool_size,
                        *connect_timeout,
                    ) {
                        Ok(pool) => {
                            tracing::info!(redis_url = %url, "Shared Redis connection pool created");
                            Some(pool)
                        }
                        Err(e) => {
                            tracing::warn!(
                                redis_url = %url,
                                error = %e,
                                "Failed to create Redis pool, falling back to memory backend"
                            );
                            None
                        }
                    }
                }
                _ => None,
            };

            let rate_limiter = match &shared_redis_pool {
                Some(pool) => {
                    tracing::info!("Rate limiter using shared Redis backend");
                    keycompute_ratelimit::RateLimitService::with_redis_pool(pool.clone())
                }
                None => {
                    tracing::info!("Rate limiter using memory backend (Redis pool unavailable)");
                    keycompute_ratelimit::RateLimitService::default_memory()
                }
            };

            let node_gateway = match &shared_redis_pool {
                Some(redis_pool) => {
                    let node_config = config.node_gateway.clone();
                    match Self::create_node_gateway_with_pool(
                        &pool,
                        redis_pool.clone(),
                        node_config,
                    ) {
                        Ok(service) => {
                            tracing::info!("Node gateway service initialized successfully");
                            Some(Arc::new(service.with_lifecycle(Arc::clone(&lifecycle))))
                        }
                        Err(e) => {
                            tracing::warn!("Failed to initialize node gateway service: {}", e);
                            None
                        }
                    }
                }
                None => {
                    tracing::info!("Node gateway requires Redis backend, skipping initialization");
                    None
                }
            };

            let cache = Self::create_cache_service(shared_redis_pool);

            (rate_limiter, node_gateway, cache)
        };

        #[cfg(not(feature = "redis"))]
        let (rate_limiter, node_gateway, cache) = {
            tracing::info!("Redis feature disabled, using memory rate limiter");
            (
                keycompute_ratelimit::RateLimitService::default_memory(),
                None,
                Self::create_disabled_cache(),
            )
        };

        // 将 PricingService 接入分布式缓存（L2 防击穿）
        let pricing_service = pricing_service.with_dist_cache(Arc::clone(&cache));

        // 创建邮件服务
        let email_service = Arc::new(EmailService::new(config.email));
        let public_auth_cookie_secret =
            Arc::new(format!("{}:public-auth-cookie", config.jwt.secret));

        // Registry 始终存在；每个 provider 独立完成配置校验和初始化。
        let payment = Some(Arc::new(
            crate::payment_registry::PaymentRegistry::from_env(Arc::clone(&pool)),
        ));

        Self {
            app_base_url: config.app_base_url,
            pool: Some(pool),
            auth: Arc::new(auth_service),
            rate_limiter: Arc::new(rate_limiter),
            pricing: Arc::new(pricing_service),
            account_states: Arc::clone(&account_states),
            provider_health,
            routing: routing_engine,
            gateway,
            http_proxy,
            billing,
            email_service,
            public_auth_cookie_secret,
            payment,
            node_gateway,
            cache,
            gateway_config: config.gateway,
            lifecycle,
        }
    }

    /// 创建用于测试的应用状态，使用自定义 Provider（默认配置）
    pub fn with_providers(providers: HashMap<String, Arc<dyn ProviderAdapter>>) -> Self {
        Self::with_providers_and_config(providers, AppStateConfig::default())
    }

    /// 创建用于测试的应用状态，使用自定义 Provider和配置
    pub fn with_providers_and_config(
        providers: HashMap<String, Arc<dyn ProviderAdapter>>,
        config: AppStateConfig,
    ) -> Self {
        // 创建 API Key 验证器
        let api_key_validator = ProduceAiKeyValidator::new();
        // 创建 JWT 验证器
        let jwt_validator = JwtValidator::new(&config.jwt.secret, &config.jwt.issuer)
            .with_expiration(config.jwt.expiry_secs);
        // 创建 AuthService，同时支持 API Key 和 JWT 认证
        let auth_service = AuthService::new(api_key_validator).with_jwt(jwt_validator);

        // 创建定价服务
        let pricing_service = keycompute_pricing::PricingService::new();

        // 创建运行时状态存储
        let account_states = Arc::new(AccountStateStore::new());
        let provider_health = Arc::new(ProviderHealthStore::new());

        // 从自定义 providers 中提取名称列表
        let provider_names: Vec<String> = providers.keys().cloned().collect();

        // 创建路由引擎（集成 ProviderHealthStore 和 AccountStateStore）
        let routing_engine = Arc::new(RoutingEngine::new(
            Arc::clone(&account_states),
            Arc::clone(&provider_health),
            provider_names,
        ));

        // 创建 Internal HTTP Proxy（支持配置）
        let http_proxy = Arc::new(Self::create_http_proxy(
            config.gateway.proxy.as_ref(),
            &config.gateway,
        ));

        // 创建 Gateway 执行器，使用自定义 Provider
        let mut builder = GatewayBuilder::new()
            .with_config(Self::gateway_executor_config(&config.gateway))
            .with_http_proxy(Arc::clone(&http_proxy));
        for (name, provider) in providers {
            builder = builder.add_provider(name, provider);
        }
        let gateway = Arc::new(builder.build());

        // 创建计费服务
        let billing = Arc::new(BillingService::new());

        // 根据配置创建限流服务
        let rate_limiter = Self::create_rate_limiter(&config.rate_limit);

        // 创建缓存服务（降级为 no-op，因为无 Redis 连接池）
        let cache = Self::create_disabled_cache();

        // 将 PricingService 接入分布式缓存（确保代码路径覆盖）
        let pricing_service = pricing_service.with_dist_cache(Arc::clone(&cache));

        // 创建邮件服务
        let email_service = Arc::new(EmailService::new(config.email));
        let public_auth_cookie_secret =
            Arc::new(format!("{}:public-auth-cookie", config.jwt.secret));

        Self {
            app_base_url: config.app_base_url,
            pool: None,
            auth: Arc::new(auth_service),
            rate_limiter: Arc::new(rate_limiter),
            pricing: Arc::new(pricing_service),
            account_states: Arc::clone(&account_states),
            provider_health,
            routing: routing_engine,
            gateway,
            http_proxy,
            billing,
            email_service,
            public_auth_cookie_secret,
            payment: None,      // 测试环境不需要支付服务
            node_gateway: None, // 测试环境不需要节点网关
            cache,
            gateway_config: config.gateway,
            lifecycle: Arc::new(NoopRequestLifecycleRecorder),
        }
    }

    /// 验证应用状态是否适合生产环境
    ///
    /// 检查必要的数据库连接是否已配置
    ///
    /// # 返回
    /// - `Ok(())`: 所有检查通过
    /// - `Err(...) )`: 缺少必要配置
    pub fn validate_for_production(&self) -> crate::error::Result<()> {
        let mut issues = Vec::new();

        // 检查数据库连接
        if self.pool.is_none() {
            issues.push("Database connection pool is not configured".to_string());
        }

        // 检查 Auth 服务是否配置了数据库
        if !self.auth.has_pool() {
            issues.push("Auth service is not configured with database connection".to_string());
        }

        // 检查 Pricing 服务是否配置了数据库
        if !self.pricing.has_pool() {
            issues.push("Pricing service is not configured with database connection".to_string());
        }

        // 检查 Billing 服务是否配置了数据库
        if !self.billing.has_pool() {
            issues.push("Billing service is not configured with database connection".to_string());
        }

        if issues.is_empty() {
            tracing::info!("Application state validated for production");
            Ok(())
        } else {
            let error_msg = format!(
                "Application not ready for production:\n{}",
                issues
                    .iter()
                    .map(|s| format!("  - {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            tracing::error!("{}", error_msg);
            Err(crate::error::ApiError::Config(error_msg))
        }
    }

    /// 检查是否有数据库连接
    pub fn has_pool(&self) -> bool {
        self.pool.is_some()
    }

    /// 获取节点网关 HMAC 签名密钥
    ///
    /// 单一数据源：始终从 `node_gateway.config.registration_token_secret` 读取，
    /// 避免与独立字段重复存储导致的值不一致问题。
    pub fn node_gateway_secret(&self) -> Option<&str> {
        self.node_gateway.as_ref().and_then(|s| {
            if s.config.registration_token_secret.is_empty() {
                None
            } else {
                Some(s.config.registration_token_secret.as_str())
            }
        })
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_crypto_initialization_rejects_missing_key() {
        assert!(init_global_crypto(&keycompute_config::AppConfig::default(), true).is_err());
    }

    #[test]
    fn development_crypto_initialization_allows_missing_key() {
        init_global_crypto(&keycompute_config::AppConfig::default(), false).unwrap();
    }

    #[test]
    fn app_state_config_preserves_redis_pool_settings() {
        let config = keycompute_config::AppConfig {
            redis: Some(keycompute_config::RedisConfig {
                url: "redis://redis.internal:6379".to_string(),
                pool_size: 23,
                connect_timeout_secs: 7,
            }),
            ..keycompute_config::AppConfig::default()
        };

        let state_config = AppStateConfig::from_config(&config);
        assert!(matches!(
            state_config.rate_limit,
            RateLimitBackendConfig::Redis {
                pool_size: 23,
                connect_timeout,
                ..
            } if connect_timeout == Duration::from_secs(7)
        ));
    }

    #[test]
    fn gateway_executor_config_preserves_runtime_controls() {
        let gateway_config = keycompute_config::GatewayConfig {
            max_retries: 7,
            timeout_secs: 43,
            enable_fallback: false,
            ..keycompute_config::GatewayConfig::default()
        };

        let executor_config = AppState::gateway_executor_config(&gateway_config);
        assert_eq!(executor_config.max_retries, 7);
        assert_eq!(executor_config.timeout_secs, 43);
        assert!(!executor_config.enable_fallback);
    }

    #[test]
    fn test_app_state_new() {
        let state = AppState::new();
        // 基础测试，确保可以创建
        let _ = state;
    }
}
