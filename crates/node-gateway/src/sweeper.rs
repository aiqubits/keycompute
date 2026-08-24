//! Node Gateway Sweeper 模块
//!
//! 后台维护任务：节点 TTL 过期、任务过期、Redis 补推

use crate::config::NodeGatewayAppConfig;
use crate::redis::NodeGatewayRedis;
use keycompute_db::DbError;
use keycompute_db::DbRouter;
use keycompute_db::models::node_task::*;
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use std::sync::Arc;
use tracing;

// Stable, application-owned PostgreSQL advisory lock key for the Node Gateway
// sweeper. Keep it distinct from every other background job.
const NODE_GATEWAY_SWEEPER_ADVISORY_LOCK_ID: i64 = 7_534_776_966_917_122_895;

/// Node Gateway Sweeper
pub struct NodeGatewaySweeper {
    pool: Arc<DbRouter>,
    redis: NodeGatewayRedis,
    config: NodeGatewayAppConfig,
}

impl NodeGatewaySweeper {
    /// 创建新的 Sweeper
    pub fn new(pool: Arc<DbRouter>, redis: NodeGatewayRedis, config: NodeGatewayAppConfig) -> Self {
        Self {
            pool,
            redis,
            config,
        }
    }

    /// 运行一次 sweeper 周期
    pub async fn run_once(&self) -> Result<(), anyhow::Error> {
        // 多副本只允许一个 sweeper 执行当前周期。所有数据库维护都复用持有
        // transaction-scoped advisory lock 的同一事务：这既保证领导权覆盖完整
        // DB 阶段，也允许 max_connections=1 的合法配置正常运行。
        let leader_tx = self.pool.begin().await?;
        let row = leader_tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT pg_try_advisory_xact_lock($1) AS acquired",
                [NODE_GATEWAY_SWEEPER_ADVISORY_LOCK_ID.into()],
            ))
            .await?
            .ok_or_else(|| anyhow::anyhow!("node sweeper advisory lock returned no row"))?;
        let lock = SweeperLock::from_query_result(&row, "")?;
        if !lock.acquired {
            leader_tx.rollback().await?;
            tracing::debug!("Skipping node gateway sweeper on non-leader replica");
            return Ok(());
        }

        tracing::debug!("Running node gateway sweeper");

        // 1. 将超时的 online 节点标记为 offline
        let offline_nodes = self.expire_offline_nodes(&leader_tx).await?;

        // 2. 将过期任务标记为 expired
        let expired_tasks = self.expire_overdue_tasks(&leader_tx).await?;

        // 3. 在同一快照中查询需要补推的 queued 任务
        let tasks_to_repush = self.find_queued_tasks_to_repush(&leader_tx).await?;

        // 先提交数据库变更，再执行 Redis 副作用，避免提交失败时仍对外发布
        // 尚未生效的过期状态或队列消息。提交同时释放 advisory lock。
        leader_tx.commit().await?;

        if offline_nodes > 0 {
            tracing::info!("Marked {} nodes as offline", offline_nodes);
        }
        if !expired_tasks.is_empty() {
            for task in &expired_tasks {
                crate::metrics::record_node_task_completion(
                    &keycompute_types::node::NodeTaskCompleteAction::Expired,
                    true,
                    task.claimed_at,
                    task.finished_at.unwrap_or_else(chrono::Utc::now),
                );
            }
            tracing::info!("Marked {} tasks as expired", expired_tasks.len());
        }

        // 4. 先清理已经终态的队列条目。历史版本可能留下同一任务的多个
        // List 元素，因此必须删除全部匹配项，而不是只弹出一个。
        self.remove_expired_tasks_from_queues(&expired_tasks).await;

        // 5. 补推 queued 任务到 Redis
        self.repush_queued_tasks(&tasks_to_repush).await;

        // 6. 通知等待方过期任务
        for task in &expired_tasks {
            if let Err(e) = self
                .redis
                .push_result_notification(task.id, "expired")
                .await
            {
                tracing::warn!(
                    "Failed to push expired notification for task {}: {}",
                    task.id,
                    e
                );
            }
        }

        Ok(())
    }

    /// 将超时的 online 节点标记为 offline
    async fn expire_offline_nodes(&self, tx: &DatabaseTransaction) -> Result<u64, DbError> {
        let ttl = self.config.sweeper_heartbeat_ttl_secs as i64;

        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            UPDATE nodes
            SET status = 'offline', updated_at = NOW()
            WHERE status = 'online'
              AND last_heartbeat_at < NOW() - MAKE_INTERVAL(secs => $1)
            "#,
            [ttl.into()],
        );
        let result = tx.execute(stmt).await?;
        Ok(result.rows_affected())
    }

    /// 将过期任务标记为 expired
    async fn expire_overdue_tasks(
        &self,
        tx: &DatabaseTransaction,
    ) -> Result<Vec<NodeTask>, DbError> {
        let expired_tasks = NodeTask::expire_overdue_tasks(tx).await?;
        for task in &expired_tasks {
            crate::store::NodeGatewayStore::finish_node_trace_savepoint(
                tx,
                task,
                task.lease_id,
                keycompute_types::node::NodeTaskCompleteAction::Expired,
            )
            .await;
        }

        Ok(expired_tasks)
    }

    /// 查询需要补推到 Redis 的 queued 任务。
    async fn find_queued_tasks_to_repush(
        &self,
        tx: &DatabaseTransaction,
    ) -> Result<Vec<NodeTask>, DbError> {
        let repush_interval = self.config.sweeper_repush_interval_secs as i64;

        // 查询需要补推的任务
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT * FROM node_tasks
            WHERE status = 'queued'
              AND deadline_at > NOW()
              AND queued_at < NOW() - MAKE_INTERVAL(secs => $1)
            ORDER BY queued_at ASC
            "#,
            [repush_interval.into()],
        );
        Ok(NodeTask::find_by_statement(stmt).all(tx).await?)
    }

    /// 补推 queued 任务到 Redis。数据库事务提交后才执行外部副作用。
    async fn repush_queued_tasks(&self, tasks_to_repush: &[NodeTask]) {
        for task in tasks_to_repush {
            if let Err(e) = self.redis.repush_queued_task(&task.model, task.id).await {
                tracing::warn!("Failed to repush task {} to queue: {}", task.id, e);
            }
        }

        if !tasks_to_repush.is_empty() {
            tracing::info!("Repushed {} queued tasks to Redis", tasks_to_repush.len());
        }
    }

    async fn remove_expired_tasks_from_queues(&self, expired_tasks: &[NodeTask]) {
        for task in expired_tasks {
            if let Err(error) = self
                .redis
                .remove_from_model_queue(&task.model, task.id)
                .await
            {
                tracing::warn!(
                    task_id = %task.id,
                    model = %task.model,
                    %error,
                    "Failed to remove expired task from Redis queue"
                );
            }
        }
    }
}

#[derive(FromQueryResult)]
struct SweeperLock {
    acquired: bool,
}
