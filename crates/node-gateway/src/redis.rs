//! Node Gateway Redis 模块
//!
//! 负责任务队列管理和结果通知

use deadpool_redis::redis::AsyncCommands;
use keycompute_runtime::redis_store::RedisRuntimeStore;
use std::sync::Arc;
use tracing;
use uuid::Uuid;

const RESULT_NOTIFICATION_TTL_SECS: u64 = 5 * 60;

// Keep the existing Redis List representation for deployment compatibility,
// but make recovery publication idempotent. The script is atomic with BRPOP:
// it publishes one fresh occurrence after removing every stale queued copy
// (a blocked consumer may take that occurrence immediately).
const REPLACE_MODEL_QUEUE_ENTRY_SCRIPT: &str = r#"
local removed = redis.call('LREM', KEYS[1], 0, ARGV[1])
redis.call('LPUSH', KEYS[1], ARGV[1])
return removed
"#;

const PUSH_RESULT_NOTIFICATION_SCRIPT: &str = r#"
redis.call('DEL', KEYS[1])
redis.call('LPUSH', KEYS[1], ARGV[1])
redis.call('EXPIRE', KEYS[1], ARGV[2])
return 1
"#;

/// Node Gateway Redis 管理器
#[derive(Clone)]
pub struct NodeGatewayRedis {
    redis: Arc<RedisRuntimeStore>,
}

impl NodeGatewayRedis {
    /// 创建新的 Redis 管理器
    pub fn new(redis: Arc<RedisRuntimeStore>) -> Self {
        Self { redis }
    }

    fn model_queue_key(model: &str) -> String {
        format!("queue:node:model:{model}")
    }

    /// 推送任务到模型队列
    pub async fn push_to_model_queue(
        &self,
        model: &str,
        task_id: Uuid,
    ) -> Result<(), anyhow::Error> {
        let queue_key = Self::model_queue_key(model);
        let mut conn = self.redis.pool().get().await?;
        let _: () = conn.lpush(&queue_key, &[task_id.to_string()]).await?;
        tracing::debug!("Pushed task {} to queue {}", task_id, queue_key);
        Ok(())
    }

    /// 从模型队列中阻塞弹出任务
    pub async fn pop_from_model_queue(
        &self,
        model: &str,
        timeout_secs: u64,
    ) -> Result<Option<Uuid>, anyhow::Error> {
        let queue_key = Self::model_queue_key(model);

        let mut conn = self.redis.pool().get().await?;
        let result: Option<(String, String)> =
            conn.brpop(&[queue_key], timeout_secs as f64).await?;

        match result {
            Some((_, task_id_str)) => {
                let task_id = Uuid::parse_str(&task_id_str)?;
                Ok(Some(task_id))
            }
            None => Ok(None),
        }
    }

    /// 推送任务结果通知
    pub async fn push_result_notification(
        &self,
        task_id: Uuid,
        status: &str,
    ) -> Result<(), anyhow::Error> {
        let result_key = format!("task:result:{}", task_id);
        let mut conn = self.redis.pool().get().await?;
        let _: i64 = deadpool_redis::redis::cmd("EVAL")
            .arg(PUSH_RESULT_NOTIFICATION_SCRIPT)
            .arg(1)
            .arg(&result_key)
            .arg(status)
            .arg(RESULT_NOTIFICATION_TTL_SECS)
            .query_async(&mut conn)
            .await?;
        tracing::debug!(
            "Pushed result notification for task {}: {}",
            task_id,
            status
        );
        Ok(())
    }

    /// 等待任务结果通知
    pub async fn wait_for_result(
        &self,
        task_id: Uuid,
        timeout_secs: u64,
    ) -> Result<Option<String>, anyhow::Error> {
        let result_key = format!("task:result:{}", task_id);

        let mut conn = self.redis.pool().get().await?;
        let result: Option<(String, String)> =
            conn.brpop(&[result_key], timeout_secs as f64).await?;

        match result {
            Some((_, status)) => Ok(Some(status)),
            None => Ok(None),
        }
    }

    /// 补推 queued 任务到模型队列
    pub async fn repush_queued_task(
        &self,
        model: &str,
        task_id: Uuid,
    ) -> Result<(), anyhow::Error> {
        let queue_key = Self::model_queue_key(model);
        let mut conn = self.redis.pool().get().await?;
        let removed: i64 = deadpool_redis::redis::cmd("EVAL")
            .arg(REPLACE_MODEL_QUEUE_ENTRY_SCRIPT)
            .arg(1)
            .arg(&queue_key)
            .arg(task_id.to_string())
            .query_async(&mut conn)
            .await?;
        tracing::debug!(
            task_id = %task_id,
            queue = %queue_key,
            removed_duplicates = removed,
            "Republished task after removing stale queue entries"
        );
        Ok(())
    }

    /// Remove every queued copy of a task after it reaches a terminal state.
    pub async fn remove_from_model_queue(
        &self,
        model: &str,
        task_id: Uuid,
    ) -> Result<u64, anyhow::Error> {
        let queue_key = Self::model_queue_key(model);
        let mut conn = self.redis.pool().get().await?;
        let removed: u64 = conn.lrem(&queue_key, 0, task_id.to_string()).await?;
        if removed > 0 {
            tracing::debug!(
                task_id = %task_id,
                queue = %queue_key,
                removed,
                "Removed terminal task from model queue"
            );
        }
        Ok(removed)
    }
}
