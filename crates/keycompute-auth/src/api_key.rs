//! Produce AI Key 验证
//!
//! 处理 Produce AI Key（用户访问系统的 API Key）的验证和解析。

use keycompute_db::{DbRouter, ProduceAiKey, User};
use keycompute_types::{KeyComputeError, Result};
use sea_orm::ConnectionTrait;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::AuthContext;
use crate::permission::{AuthType, build_permissions};

/// Produce AI Key 验证器
#[derive(Clone)]
pub struct ProduceAiKeyValidator {
    /// 数据库连接池（可选）
    pool: Option<Arc<DbRouter>>,
}

impl std::fmt::Debug for ProduceAiKeyValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProduceAiKeyValidator")
            .field("pool", &self.pool.as_ref().map(|_| "DatabaseConnection"))
            .finish()
    }
}

impl ProduceAiKeyValidator {
    /// 创建新的 Produce AI Key 验证器（无数据库连接）
    pub fn new() -> Self {
        Self { pool: None }
    }

    /// 创建带数据库连接的验证器
    pub fn with_pool(pool: Arc<DbRouter>) -> Self {
        Self { pool: Some(pool) }
    }

    /// 验证 Produce AI Key
    ///
    /// Produce AI Key 格式: `sk-` + 48 个字符（与 OpenAI API Key 一致）
    ///
    /// 验证流程：
    /// 1. 检查格式
    /// 2. 从数据库查询 key hash
    /// 3. 验证 key 有效性（未撤销、未过期）
    /// 4. 加载用户和租户信息
    /// 5. 验证租户状态
    /// 6. 更新最后使用时间
    pub async fn validate(&self, key: &str) -> Result<AuthContext> {
        // 检查格式
        if !Self::is_valid_format(key) {
            return Err(KeyComputeError::AuthError("Invalid API key format".into()));
        }

        // 计算 key 的 hash
        let key_hash = Self::hash_key(key);

        // 从数据库验证
        match &self.pool {
            Some(pool) => self.validate_from_database(pool.as_ref(), &key_hash).await,
            None => {
                // 无数据库连接时返回错误，不使用不安全的 fallback
                tracing::error!(
                    "API key validation attempted without database connection. \
                     This indicates a misconfiguration. Use ProduceAiKeyValidator::with_pool() for production."
                );
                Err(KeyComputeError::AuthError(
                    "Authentication service not properly configured".into(),
                ))
            }
        }
    }

    /// 检查 API Key 格式是否有效
    //
    /// 格式要求：
    /// - 以 `sk-` 开头
    /// - 标准格式：sk- + 48 字符 = 51 字符（全字母数字）
    /// - 带前缀格式：sk-{prefix}-{random}，前缀 1-27 字符，随机部分至少 20 字符
    pub fn is_valid_format(key: &str) -> bool {
        if !key.starts_with("sk-") {
            return false;
        }

        let after_sk = &key[3..];

        // 检查是否有前缀格式（包含连字符）
        if let Some(dash_pos) = after_sk.find('-') {
            let prefix = &after_sk[..dash_pos];
            let rest = &after_sk[dash_pos + 1..];
            // 前缀长度 1-27，剩余部分至少 20 字符
            !prefix.is_empty()
                && prefix.len() <= 27
                && prefix.chars().all(|c| c.is_ascii_alphanumeric())
                && rest.len() >= 20
                && rest.chars().all(|c| c.is_ascii_alphanumeric())
        } else {
            // 标准格式：sk- 后面全是字母数字，总长度 51
            key.len() == 51 && after_sk.chars().all(|c| c.is_ascii_alphanumeric())
        }
    }

    /// 从数据库验证 Produce AI Key
    async fn validate_from_database(
        &self,
        pool: &impl ConnectionTrait,
        key_hash: &str,
    ) -> Result<AuthContext> {
        // 查询 Produce AI Key
        let produce_ai_key = ProduceAiKey::find_by_hash(pool, key_hash)
            .await
            .map_err(|e| {
                KeyComputeError::DatabaseError(format!("Failed to query API key: {}", e))
            })?;

        let Some(produce_ai_key) = produce_ai_key else {
            tracing::warn!(key_hash = %key_hash, "Produce AI key not found");
            return Err(KeyComputeError::AuthError("Invalid API key".into()));
        };

        // 检查是否有效
        if !produce_ai_key.is_valid() {
            tracing::warn!(
                produce_ai_key_id = %produce_ai_key.id,
                revoked = produce_ai_key.revoked,
                "Produce AI key is not valid"
            );
            return Err(KeyComputeError::AuthError(
                "API key is revoked or expired".into(),
            ));
        }

        // 查询用户信息
        let user = User::find_by_id(pool, produce_ai_key.user_id)
            .await
            .map_err(|e| KeyComputeError::DatabaseError(format!("Failed to query user: {}", e)))?;

        // 用户已被删除但 key 记录残留（孤儿 key），对外统一返回通用错误，避免泄露内部状态
        let Some(user) = user else {
            tracing::warn!(
                produce_ai_key_id = %produce_ai_key.id,
                user_id = %produce_ai_key.user_id,
                "User not found for produce AI key (orphaned key)"
            );
            return Err(KeyComputeError::AuthError("Invalid API key".into()));
        };

        // 验证用户租户 ID 与 Produce AI Key 租户 ID 一致
        if user.tenant_id != produce_ai_key.tenant_id {
            tracing::warn!(
                user_id = %user.id,
                user_tenant_id = %user.tenant_id,
                produce_ai_key_tenant_id = %produce_ai_key.tenant_id,
                "User tenant does not match Produce AI key tenant"
            );
            return Err(KeyComputeError::AuthError("Invalid API key".into()));
        }

        // 查询租户信息并验证状态
        use keycompute_db::Tenant;
        let tenant = Tenant::find_by_id(pool, user.tenant_id)
            .await
            .map_err(|e| {
                KeyComputeError::DatabaseError(format!("Failed to query tenant: {}", e))
            })?;

        let Some(tenant) = tenant else {
            tracing::warn!(tenant_id = %user.tenant_id, "Tenant not found for produce AI key");
            return Err(KeyComputeError::AuthError("Invalid API key".into()));
        };

        // 检查租户状态：具体状态值属于内部信息只进日志，不对外暴露
        if !tenant.is_active() {
            tracing::warn!(
                tenant_id = %tenant.id,
                status = %tenant.status,
                "Tenant is not active"
            );
            return Err(KeyComputeError::AuthError("Tenant is not active".into()));
        }

        // 更新最后使用时间
        let _ = produce_ai_key.update_last_used(pool).await;

        tracing::info!(
            user_id = %user.id,
            tenant_id = %user.tenant_id,
            produce_ai_key_id = %produce_ai_key.id,
            role = %user.role,
            "Produce AI key validated successfully"
        );

        // 构建权限列表
        // API Key 认证仅有 UseApi 权限，用于转发请求到上游 LLM Provider
        // 不包含任何系统管理权限（用户管理、计量计费、模块管理、系统设置等）
        let permissions = build_permissions(AuthType::ApiKey, &user.role);

        Ok(AuthContext {
            user_id: user.id,
            tenant_id: user.tenant_id,
            produce_ai_key_id: produce_ai_key.id,
            role: user.role,
            permissions,
            token_version: 0,
            user_info: None,
            tenant_info: None,
        })
    }

    /// 检查是否已配置数据库连接
    ///
    /// 用于启动时验证配置
    pub fn has_pool(&self) -> bool {
        self.pool.is_some()
    }

    /// 计算 Produce AI Key 的 SHA256 hash
    pub fn hash_key(key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// 生成新的 Produce AI Key
    ///
    /// 格式与 OpenAI API Key 一致：`sk-` + 48 个字符
    /// 示例: sk-proj-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
    pub fn generate_key() -> String {
        // 生成 48 字符的随机字符串
        // 使用两个 UUID 来生成足够的随机字符
        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();
        let combined = format!(
            "{}{}",
            uuid1.to_string().replace("-", ""),
            uuid2.to_string().replace("-", "")
        );
        // 取前 48 个字符
        format!("sk-{}", &combined[..48])
    }

    /// 生成带前缀的 Produce AI Key
    //
    /// 格式: `sk-{prefix}-` + 随机字符，确保随机部分至少 20 字符
    /// 示例: sk-proj-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
    //
    /// 注意：前缀长度不能超过 27 字符（因为随机部分至少需要 20 字符）
    pub fn generate_key_with_prefix(prefix: &str) -> String {
        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();
        let combined = format!(
            "{}{}",
            uuid1.to_string().replace("-", ""),
            uuid2.to_string().replace("-", "")
        );

        // 前缀最长 27 字符（因为 48 - 1(连字符) - 27 = 20 最小随机部分）
        let max_prefix_len = 27;
        let prefix = if prefix.len() > max_prefix_len {
            &prefix[..max_prefix_len]
        } else {
            prefix
        };

        // 计算格式：sk-{prefix}-{random}
        // 随机部分长度 = 48 - prefix.len() - 1(连字符)
        let random_len = 48usize.saturating_sub(prefix.len() + 1);
        let random_part = &combined[..random_len.min(combined.len())];

        format!("sk-{}-{}", prefix, random_part)
    }
}

impl Default for ProduceAiKeyValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Produce AI Key 认证 trait
#[async_trait::async_trait]
pub trait ProduceAiKeyAuth: Send + Sync {
    /// 验证 Produce AI Key
    async fn authenticate(&self, key: &str) -> Result<AuthContext>;
}

#[async_trait::async_trait]
impl ProduceAiKeyAuth for ProduceAiKeyValidator {
    async fn authenticate(&self, key: &str) -> Result<AuthContext> {
        self.validate(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_key() {
        let key = ProduceAiKeyValidator::generate_key();
        assert!(key.starts_with("sk-"));
        // 新格式：sk- + 48 字符 = 51 字符
        assert_eq!(key.len(), 51, "Key should be 51 characters (sk- + 48)");
        // 验证格式
        assert!(ProduceAiKeyValidator::is_valid_format(&key));
    }

    #[test]
    fn test_generate_key_with_prefix() {
        let key = ProduceAiKeyValidator::generate_key_with_prefix("proj");
        assert!(key.starts_with("sk-proj-"));
        assert!(ProduceAiKeyValidator::is_valid_format(&key));

        let key = ProduceAiKeyValidator::generate_key_with_prefix("test");
        assert!(key.starts_with("sk-test-"));
        assert!(ProduceAiKeyValidator::is_valid_format(&key));
    }

    #[test]
    fn test_is_valid_format() {
        // 标准格式 - 有效
        let key = ProduceAiKeyValidator::generate_key();
        assert!(ProduceAiKeyValidator::is_valid_format(&key));

        // 带前缀格式 - 有效
        let key_with_prefix = ProduceAiKeyValidator::generate_key_with_prefix("proj");
        assert!(ProduceAiKeyValidator::is_valid_format(&key_with_prefix));

        // 无效格式 - 不以 sk- 开头
        assert!(!ProduceAiKeyValidator::is_valid_format("invalid-key"));

        // 无效格式 - 太短
        assert!(!ProduceAiKeyValidator::is_valid_format("sk-short"));

        // 无效格式 - 包含特殊字符
        assert!(!ProduceAiKeyValidator::is_valid_format(
            "sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx!"
        ));

        // OpenAI 格式兼容测试
        // OpenAI key: sk-proj-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
        let openai_style = "sk-proj-abcdefgh12345678abcdefgh12345678abcdefgh12";
        assert!(ProduceAiKeyValidator::is_valid_format(openai_style));
    }

    #[test]
    fn test_hash_key() {
        let key = "sk-test1234567890123456789012345678901234567890";
        let hash1 = ProduceAiKeyValidator::hash_key(key);
        let hash2 = ProduceAiKeyValidator::hash_key(key);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA256 hex 长度
    }

    #[tokio::test]
    async fn test_validate_invalid_format() {
        let validator = ProduceAiKeyValidator::new();
        let result = validator.validate("invalid-key").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_without_pool() {
        // 无数据库连接时验证应该失败
        let validator = ProduceAiKeyValidator::new();
        let key = ProduceAiKeyValidator::generate_key();
        let result = validator.validate(&key).await;
        assert!(
            result.is_err(),
            "Validation should fail without database connection"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("not properly configured"),
            "Error should indicate misconfiguration"
        );
    }

    #[test]
    fn test_has_pool() {
        let validator_without_pool = ProduceAiKeyValidator::new();
        assert!(!validator_without_pool.has_pool());
    }
}
