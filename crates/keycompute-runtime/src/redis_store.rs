//! Redis 运行时状态存储实现
//!
//! 提供基于 Redis 的运行时状态存储后端，支持：
//! - 分布式状态共享
//! - 自动过期清理
//! - 高可用性
//! - 连接池管理

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::{Config, Pool, Runtime, Timeouts};
use std::time::Duration;

/// Redis 存储错误
#[derive(Debug, thiserror::Error)]
pub enum RedisStoreError {
    /// 连接池错误
    #[error("Redis pool error: {0}")]
    PoolError(#[from] deadpool_redis::PoolError),
    /// Redis 错误
    #[error("Redis error: {0}")]
    RedisError(#[from] deadpool_redis::redis::RedisError),
    /// 连接错误
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    /// 创建连接池错误
    #[error("Failed to create pool: {0}")]
    CreatePoolError(String),
}

/// Redis 运行时存储
#[derive(Debug, Clone)]
pub struct RedisRuntimeStore {
    pool: Pool,
    key_prefix: String,
    default_ttl: Duration,
}

impl RedisRuntimeStore {
    /// 创建新的 Redis 运行时存储
    ///
    /// # 参数
    /// - `redis_url`: Redis 连接 URL
    pub fn new(redis_url: &str) -> Result<Self, RedisStoreError> {
        let cfg = Config::from_url(redis_url);
        let pool = cfg
            .create_pool(Some(Runtime::Tokio1))
            .map_err(|e| RedisStoreError::CreatePoolError(e.to_string()))?;

        Ok(Self {
            pool,
            key_prefix: "keycompute:runtime".to_string(),
            default_ttl: Duration::from_secs(300),
        })
    }

    /// 创建带自定义前缀的存储
    pub fn with_prefix(
        redis_url: &str,
        prefix: impl Into<String>,
    ) -> Result<Self, RedisStoreError> {
        let cfg = Config::from_url(redis_url);
        let pool = cfg
            .create_pool(Some(Runtime::Tokio1))
            .map_err(|e| RedisStoreError::CreatePoolError(e.to_string()))?;

        Ok(Self {
            pool,
            key_prefix: prefix.into(),
            default_ttl: Duration::from_secs(300),
        })
    }

    /// 从配置创建存储
    pub fn from_config(config: &RedisPoolConfig) -> Result<Self, RedisStoreError> {
        let pool =
            Self::create_pool_with_options(&config.url, config.pool_size, config.connect_timeout)?;

        Ok(Self {
            pool,
            key_prefix: config.key_prefix.clone(),
            default_ttl: config.default_ttl,
        })
    }

    /// 设置默认 TTL
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = ttl;
        self
    }

    /// 构建完整的 Redis Key
    fn build_key(&self, key: &str) -> String {
        format!("{}:{}", self.key_prefix, key)
    }

    /// 获取 Redis 连接
    async fn get_conn(&self) -> Result<deadpool_redis::Connection, RedisStoreError> {
        self.pool.get().await.map_err(Into::into)
    }

    /// 健康检查
    pub async fn health_check(&self) -> Result<(), RedisStoreError> {
        let mut conn = self.get_conn().await?;
        let _: () = deadpool_redis::redis::cmd("PING")
            .query_async(&mut conn)
            .await?;
        Ok(())
    }

    /// 从 URL 创建共享连接池（静态工厂）
    ///
    /// 供 `state.rs` 等调用方获取 Pool 后传递给多个消费者，
    /// 避免外部模块直接依赖 `deadpool_redis::Config`。
    pub fn create_pool(redis_url: &str) -> Result<Pool, RedisStoreError> {
        let cfg = Config::from_url(redis_url);
        cfg.create_pool(Some(Runtime::Tokio1))
            .map_err(|e| RedisStoreError::CreatePoolError(e.to_string()))
    }

    /// 从 URL 和连接池参数创建共享连接池。
    pub fn create_pool_with_options(
        redis_url: &str,
        pool_size: usize,
        connect_timeout: Duration,
    ) -> Result<Pool, RedisStoreError> {
        let mut cfg = Config::from_url(redis_url);
        cfg.pool = Some(deadpool_redis::PoolConfig {
            max_size: pool_size,
            timeouts: Timeouts {
                create: Some(connect_timeout),
                ..Timeouts::default()
            },
            ..Default::default()
        });
        cfg.create_pool(Some(Runtime::Tokio1))
            .map_err(|e| RedisStoreError::CreatePoolError(e.to_string()))
    }

    /// 使用已有连接池创建存储
    pub fn with_pool(pool: Pool) -> Self {
        Self {
            pool,
            key_prefix: "keycompute:runtime".to_string(),
            default_ttl: Duration::from_secs(300),
        }
    }

    /// 使用已有连接池 + 自定义前缀
    pub fn with_pool_and_prefix(pool: Pool, prefix: impl Into<String>) -> Self {
        Self {
            pool,
            key_prefix: prefix.into(),
            default_ttl: Duration::from_secs(300),
        }
    }

    /// 获取连接池状态
    pub fn pool_status(&self) -> deadpool_redis::Status {
        self.pool.status()
    }

    /// 获取 Redis 连接池引用
    pub fn pool(&self) -> &Pool {
        &self.pool
    }
}

impl RedisRuntimeStore {
    /// 批量获取值
    pub async fn mget(&self, keys: &[&str]) -> Vec<Option<String>> {
        let full_keys: Vec<String> = keys.iter().map(|k| self.build_key(k)).collect();

        match self.get_conn().await {
            Ok(mut conn) => conn.mget(&full_keys).await.unwrap_or_else(|_| vec![]),
            Err(e) => {
                tracing::warn!("Failed to get Redis connection: {}", e);
                vec![None; keys.len()]
            }
        }
    }

    /// 批量设置值
    pub async fn mset(&self, kvs: &[(&str, &str)], ttl: Option<Duration>) {
        let ttl = ttl.unwrap_or(self.default_ttl);

        match self.get_conn().await {
            Ok(mut conn) => {
                for (key, value) in kvs {
                    let full_key = self.build_key(key);
                    if let Err(e) = conn
                        .set_ex::<&str, &str, ()>(&full_key, *value, ttl.as_secs())
                        .await
                    {
                        tracing::warn!("Redis mset error: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get Redis connection: {}", e);
            }
        }
    }

    /// 检查键是否存在
    pub async fn exists(&self, key: &str) -> bool {
        match self.get_conn().await {
            Ok(mut conn) => conn.exists(self.build_key(key)).await.unwrap_or(false),
            Err(e) => {
                tracing::warn!("Failed to get Redis connection: {}", e);
                false
            }
        }
    }

    /// 获取剩余过期时间（秒）
    pub async fn ttl(&self, key: &str) -> i64 {
        match self.get_conn().await {
            Ok(mut conn) => conn.ttl(self.build_key(key)).await.unwrap_or(-2),
            Err(e) => {
                tracing::warn!("Failed to get Redis connection: {}", e);
                -2
            }
        }
    }

    /// 清理所有以当前前缀开头的键
    pub async fn flush_prefix(&self) -> Result<(), RedisStoreError> {
        let pattern = format!("{}:*", self.key_prefix);

        // 收集所有匹配的 key
        let mut keys = Vec::new();
        {
            let mut conn = self.get_conn().await?;
            let mut iter: deadpool_redis::redis::AsyncIter<String> = conn
                .scan_match(&pattern)
                .await
                .map_err(RedisStoreError::RedisError)?;
            while let Some(key) = iter.next_item().await {
                keys.push(key);
            }
        }

        // 批量删除 key
        if !keys.is_empty() {
            let mut conn = self.get_conn().await?;
            let _: () = conn.del(&keys).await.map_err(RedisStoreError::RedisError)?;
        }

        Ok(())
    }

    /// 获取 Key 前缀
    pub fn key_prefix(&self) -> &str {
        &self.key_prefix
    }
}

/// Redis 连接池配置
#[derive(Debug, Clone)]
pub struct RedisPoolConfig {
    /// Redis URL
    pub url: String,
    /// 连接池大小
    pub pool_size: usize,
    /// 连接超时
    pub connect_timeout: Duration,
    /// 默认 TTL
    pub default_ttl: Duration,
    /// Key 前缀
    pub key_prefix: String,
}

impl Default for RedisPoolConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            pool_size: 10,
            connect_timeout: Duration::from_secs(5),
            default_ttl: Duration::from_secs(300),
            key_prefix: "keycompute:runtime".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_pool_uses_requested_size() {
        let pool = RedisRuntimeStore::create_pool_with_options(
            "redis://127.0.0.1:6379",
            17,
            Duration::from_secs(3),
        )
        .expect("pool construction should not require a live Redis server");

        assert_eq!(pool.status().max_size, 17);
    }
}
