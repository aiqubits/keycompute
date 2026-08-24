//! Node Gateway Service 模块
//!
//! 业务逻辑层，提供 enqueue_and_wait 等核心接口

use crate::config::NodeGatewayAppConfig;
use crate::metrics::{record_node_task_completion, record_node_task_running};
use crate::redis::NodeGatewayRedis;
use crate::store::NodeGatewayStore;
use crate::sweeper::NodeGatewaySweeper;
use crate::trace::{
    invalid_node_result_failure, node_completion_failure, node_completion_finish,
    node_wait_timeout_failure, node_wait_timeout_finish,
};
use keycompute_db::DbError;
use keycompute_db::models::node_task::*;
use keycompute_types::ChatCompletionResponse;
use keycompute_types::node::*;
use keycompute_types::{
    AttemptKind, AttemptTraceStart, BillingStatus, ClientResponseOutcome, ErrorOrigin,
    NoopRequestLifecycleRecorder, RequestExecutionFailure, RequestLifecycleRecorder, RequestStatus,
    RouteType, TraceErrorCategory, TraceErrorInfo,
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing;
use uuid::Uuid;

/// 节点任务执行失败的归因 — 让 HTTP handler 能映射到合适的 status code (B2 修复)
#[derive(Debug, Error)]
pub enum NodeExecutionError {
    /// 请求本身有问题 (node 上报 is_client_error=true) → HTTP 4xx
    #[error("{code}: {message}")]
    ClientError { code: String, message: String },
    /// 其他失败 (节点错、超时、过期、内部错) → HTTP 5xx
    #[error("{source}")]
    Other {
        #[source]
        source: anyhow::Error,
        failure: RequestExecutionFailure,
    },
}

impl NodeExecutionError {
    fn other(source: anyhow::Error, failure: RequestExecutionFailure) -> Self {
        Self::Other { source, failure }
    }

    fn gateway_internal(source: anyhow::Error, code: &str) -> Self {
        Self::other(
            source,
            RequestExecutionFailure {
                status: RequestStatus::Failed,
                error: TraceErrorInfo {
                    origin: ErrorOrigin::Gateway,
                    category: TraceErrorCategory::Internal,
                    code: code.to_string(),
                    summary: None,
                    retryable: Some(true),
                },
                billing_status: BillingStatus::NotApplicable,
            },
        )
    }

    pub fn request_failure(&self) -> RequestExecutionFailure {
        match self {
            Self::ClientError { .. } => RequestExecutionFailure {
                status: RequestStatus::Failed,
                error: TraceErrorInfo {
                    origin: ErrorOrigin::Node,
                    category: TraceErrorCategory::NodeFailed,
                    code: "node_client_error".to_string(),
                    summary: None,
                    retryable: Some(false),
                },
                billing_status: BillingStatus::NotApplicable,
            },
            Self::Other { failure, .. } => failure.clone(),
        }
    }

    pub fn client_response_outcome(&self) -> ClientResponseOutcome {
        if self.request_failure().status == RequestStatus::TimedOut {
            ClientResponseOutcome::TimedOut
        } else {
            ClientResponseOutcome::ResponseFailed
        }
    }
}

/// Node Gateway Service
#[derive(Clone)]
pub struct NodeGatewayService {
    pub store: NodeGatewayStore,
    redis: NodeGatewayRedis,
    /// 节点网关应用配置（包含 registration_token_secret）
    pub config: NodeGatewayAppConfig,
    lifecycle: Arc<dyn RequestLifecycleRecorder>,
}

impl NodeGatewayService {
    /// 创建新的 Service 实例
    pub fn new(
        store: NodeGatewayStore,
        redis: NodeGatewayRedis,
        config: NodeGatewayAppConfig,
    ) -> Self {
        Self {
            store,
            redis,
            config,
            lifecycle: Arc::new(NoopRequestLifecycleRecorder),
        }
    }

    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn RequestLifecycleRecorder>) -> Self {
        self.lifecycle = lifecycle;
        self
    }

    /// 使用与服务相同的数据库、Redis 和配置创建后台维护器。
    pub fn sweeper(&self) -> NodeGatewaySweeper {
        NodeGatewaySweeper::new(
            self.store.pool_arc(),
            self.redis.clone(),
            self.config.clone(),
        )
    }

    /// 入队并等待任务完成（核心接口）
    pub async fn enqueue_and_wait(
        &self,
        user_id: Uuid,
        model: String,
        payload: NodeTaskPayload,
    ) -> Result<ChatCompletionResponse, NodeExecutionError> {
        let deadline_secs = self.config.task_deadline_secs;
        // 1. 创建任务并入队
        let task = match self
            .store
            .create_and_enqueue_task(user_id, model.clone(), payload)
            .await
        {
            Ok(task) => task,
            Err(error) => {
                return Err(NodeExecutionError::gateway_internal(
                    anyhow::Error::from(error),
                    "node_task_create_failed",
                ));
            }
        };
        if let Err(error) = self
            .lifecycle
            .set_route(task.request_id, RouteType::Node, RequestStatus::Queued)
            .await
        {
            tracing::warn!(request_id=%task.request_id, %error, "failed to record node enqueue");
        }

        // 2. 推送到 Redis 队列
        if let Err(e) = self.redis.push_to_model_queue(&model, task.id).await {
            tracing::warn!("Failed to push task {} to Redis queue: {}", task.id, e);
            // Redis 失败不影响，sweeper 会补推
        }

        // 3. 等待结果（使用 Redis 通知 + Postgres 轮询兜底）
        let wait_timeout = Duration::from_secs(deadline_secs);
        let result = tokio::time::timeout(wait_timeout, async {
            loop {
                // 3.1 尝试从 Redis 获取结果通知
                if let Ok(Some(_status)) = self.redis.wait_for_result(task.id, 1).await {
                    // Redis notification is only a wake-up hint. Reload the
                    // authoritative terminal row from the writer below.
                    return self.query_task_result(task.id).await;
                }

                // 3.3 直接查询 Postgres（兜底）
                if let Ok(Some(task)) = NodeTask::find_by_id(self.store.pool(), task.id).await
                    && task.is_terminal()
                {
                    return self.query_task_result(task.id).await;
                }

                // 短暂休眠后继续轮询
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                self.finish_wait_timeout_trace(&task).await;
                Err(NodeExecutionError::other(
                    anyhow::anyhow!("Task {} timed out after {} seconds", task.id, deadline_secs),
                    node_wait_timeout_failure(),
                ))
            }
        }
    }

    /// 在发起请求的进程内关闭超时 trace，确保本地 active gauge 和生命周期
    /// HashMap 不依赖 sweeper 所在副本才能清理。
    async fn finish_wait_timeout_trace(&self, task: &NodeTask) {
        // Serialize the timeout decision with node completion, which locks this
        // same task row before changing its business state and trace. Without
        // this lock, a successful completion at the deadline can commit between
        // our stale status check and the timeout trace write.
        let tx = match self.store.pool().begin().await {
            Ok(tx) => tx,
            Err(error) => {
                tracing::warn!(request_id=%task.request_id, task_id=%task.id, %error, "failed to start node wait-timeout transaction");
                return;
            }
        };
        let current_task = match NodeTask::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM node_tasks WHERE id=$1 FOR UPDATE",
            [task.id.into()],
        ))
        .one(&tx)
        .await
        {
            Ok(Some(task)) => task,
            Ok(None) => {
                let _ = tx.rollback().await;
                tracing::warn!(request_id=%task.request_id, task_id=%task.id, "node task disappeared before wait-timeout finalization");
                return;
            }
            Err(error) => {
                let _ = tx.rollback().await;
                tracing::warn!(request_id=%task.request_id, task_id=%task.id, %error, "failed to lock node task for wait-timeout finalization");
                return;
            }
        };

        if current_task.is_terminal() {
            if let Err(error) = tx.commit().await {
                tracing::warn!(request_id=%task.request_id, task_id=%task.id, %error, "failed to release terminal node task lock");
                return;
            }
            self.synchronize_terminal_trace(&current_task).await;
            return;
        }

        let attempt = match tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM gateway_request_attempts WHERE node_task_id=$1 AND finished_at IS NULL ORDER BY attempt_no DESC LIMIT 1",
                [current_task.id.into()],
            ))
            .await
        {
            Ok(row) => row.and_then(|row| row.try_get::<Uuid>("", "id").ok()),
            Err(error) => {
                let _ = tx.rollback().await;
                tracing::warn!(request_id=%current_task.request_id, task_id=%current_task.id, %error, "failed to load node attempt for wait-timeout finalization");
                return;
            }
        };

        let result = if let Some(attempt_id) = attempt {
            Some(
                self.lifecycle
                    .finish_attempt_and_request(node_wait_timeout_finish(
                        current_task.request_id,
                        attempt_id,
                        chrono::Utc::now(),
                    ))
                    .await,
            )
        } else {
            None
        };
        if let Err(error) = tx.commit().await {
            tracing::warn!(request_id=%current_task.request_id, task_id=%current_task.id, %error, "failed to release node wait-timeout task lock");
        }
        if let Some(Err(error)) = result {
            tracing::warn!(request_id=%current_task.request_id, task_id=%current_task.id, %error, "failed to finish timed-out node trace");
        }
        if attempt.is_none() {
            mark_node_request_missing_attempt(&self.lifecycle, &current_task).await;
        }
    }

    /// 查询任务结果
    ///
    /// 失败任务读 error_json.is_client_error, 分别映射到 ClientError / Other,
    /// 让 HTTP handler 能区分 4xx vs 5xx (B2 修复)
    async fn query_task_result(
        &self,
        task_id: Uuid,
    ) -> Result<ChatCompletionResponse, NodeExecutionError> {
        // A terminal Redis notification is published after the writer commits,
        // but a configured read replica may still expose the leased row. Use a
        // writer-fresh task for result decoding and local lifecycle cleanup.
        let task = NodeTask::find_by_id(self.store.pool().write_conn(), task_id)
            .await
            .map_err(|error| {
                NodeExecutionError::gateway_internal(
                    anyhow::Error::from(error),
                    "node_task_result_query_failed",
                )
            })?
            .ok_or_else(|| {
                NodeExecutionError::gateway_internal(
                    anyhow::anyhow!("Task not found"),
                    "node_task_result_missing",
                )
            })?;

        let result = decode_chat_task_result(&task);
        self.synchronize_terminal_trace(&task).await;
        result
    }

    async fn synchronize_terminal_trace(&self, task: &NodeTask) {
        let action = match task.status.as_str() {
            "succeeded" | "image_succeeded" => NodeTaskCompleteAction::Succeeded,
            "failed" => NodeTaskCompleteAction::Failed,
            "expired" => NodeTaskCompleteAction::Expired,
            _ => return,
        };
        // A miss marks the trace partial for leased tasks, so this lookup must
        // observe the writer rather than a potentially lagging read replica.
        let attempt = if let Some(lease_id) = task.lease_id {
            self.store
                .pool()
                .write_conn()
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM gateway_request_attempts WHERE node_task_id=$1 AND lease_id=$2",
                    [task.id.into(), lease_id.into()],
                ))
                .await
                .ok()
                .flatten()
                .and_then(|row| row.try_get::<Uuid>("", "id").ok())
        } else {
            None
        };
        let finish = node_completion_finish(
            &action,
            attempt.unwrap_or_else(Uuid::nil),
            task.request_id,
            task.finished_at.unwrap_or_else(chrono::Utc::now),
        );
        if attempt.is_some() {
            let _ = self.lifecycle.finish_attempt_and_request(finish).await;
        } else {
            mark_node_request_missing_attempt(&self.lifecycle, task).await;
        }
    }

    /// 注册节点
    ///
    /// owner_user_id 通过 registration_token 自动解析，不再需要调用方传入
    pub async fn register_node(
        &self,
        req: &NodeRegisterRequest,
    ) -> Result<NodeRegisterResponse, DbError> {
        self.store.register_node(req).await
    }

    /// 心跳
    pub async fn heartbeat(
        &self,
        node_id: Uuid,
        session_id: Uuid,
        accepted_models: Vec<String>,
    ) -> Result<NodeHeartbeatResponse, DbError> {
        self.store
            .heartbeat(node_id, session_id, accepted_models)
            .await
    }

    /// 领取任务(长轮询)
    pub async fn poll_task(
        &self,
        node_id: Uuid,
        session_id: Uuid,
        accepted_models: Vec<String>,
    ) -> Result<NodePollResponse, anyhow::Error> {
        // 1. 检查节点状态
        // Poll admission is an authorization decision: writer-fresh state
        // prevents a recently excluded node from claiming more work.
        let node =
            keycompute_db::models::node::Node::find_by_id(self.store.pool().write_conn(), node_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Node not found"))?;

        // 如果节点状态不是 online,直接返回空 task (不参与 poll)
        if node.status != "online" {
            return Ok(NodePollResponse {
                protocol_version: "node.v1".to_string(),
                task: None,
                retry_after_ms: Some(5000),
            });
        }

        // 2. 检查 session 是否过期或撤销 (ready predicate)
        let session = keycompute_db::models::node_session::NodeSession::find_by_id(
            self.store.pool().write_conn(),
            session_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let now = chrono::Utc::now();
        if session.is_revoked() || session.expires_at < now {
            // session 已撤销或过期,不允许 poll
            return Ok(NodePollResponse {
                protocol_version: "node.v1".to_string(),
                task: None,
                retry_after_ms: Some(5000),
            });
        }

        // 3. 对每个 accepted_model 尝试 poll（所有模型共享同一个 poll_timeout）
        // 设计意图：防止多个模型队列依次等待导致总超时时间过长
        let poll_deadline = tokio::time::Instant::now() + self.config.poll_timeout();

        // 随机打乱模型顺序，避免固定顺序导致的队列饥饿问题
        let mut shuffled_models = accepted_models;
        fastrand::shuffle(&mut shuffled_models);

        for model in shuffled_models {
            // 检查是否已超过 poll 总超时时间
            let remaining = poll_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break; // 超时，不再尝试更多模型
            }

            // 当前模型的等待时间：取剩余时间和单个模型最小等待时间（2秒）的较大值
            let model_timeout = remaining.as_secs().max(2);

            match self.redis.pop_from_model_queue(&model, model_timeout).await {
                Ok(Some(task_id)) => {
                    // 3. 原子 claim 任务
                    match self.store.claim_task(task_id, node_id, session_id).await? {
                        Some((task, envelope)) => {
                            record_node_task_running();
                            if let Err(error) = self
                                .lifecycle
                                .start_attempt(AttemptTraceStart {
                                    request_id: task.request_id,
                                    attempt_kind: classify_node_attempt_kind(task.failure_count),
                                    route_type: RouteType::Node,
                                    model: task.model.clone(),
                                    provider_name: None,
                                    account_id: None,
                                    node_task_id: Some(task.id),
                                    node_id: task.assigned_node_id,
                                    session_id: task.assigned_session_id,
                                    lease_id: task.lease_id,
                                    started_at: task.claimed_at.unwrap_or_else(chrono::Utc::now),
                                })
                                .await
                            {
                                tracing::warn!(request_id=%task.request_id, task_id=%task.id, %error, "failed to record node claim");
                                if let Err(partial_error) =
                                    self.lifecycle.mark_trace_partial(task.request_id).await
                                {
                                    tracing::warn!(request_id=%task.request_id, task_id=%task.id, %partial_error, "failed to mark node trace partial after claim trace failure");
                                }
                            }
                            return Ok(NodePollResponse {
                                protocol_version: "node.v1".to_string(),
                                task: Some(envelope),
                                retry_after_ms: None,
                            });
                        }
                        None => {
                            // claim 失败,任务已过期或被其他节点领取
                            continue;
                        }
                    }
                }
                Ok(None) => {
                    // 超时,尝试下一个模型
                    continue;
                }
                Err(e) => {
                    tracing::warn!("Failed to pop from queue for model {}: {}", model, e);
                    continue;
                }
            }
        }

        // 没有任务
        Ok(NodePollResponse {
            protocol_version: "node.v1".to_string(),
            task: None,
            retry_after_ms: Some(1000), // 建议 1 秒后重试
        })
    }

    /// 完成任务提交
    pub async fn complete_task(
        &self,
        task_id: Uuid,
        lease_id: Uuid,
        node_id: Uuid,
        session_id: Uuid,
        result: NodeTaskResult,
    ) -> Result<NodeTaskCompleteResponse, DbError> {
        // 在 result 被 move 进 store 之前，记录是否为图片任务，
        // 以便后续推送正确的 Redis 通知 key（防止 query_task_result 反序列化错类型）。
        let is_image = matches!(&result, NodeTaskResult::ImageSucceeded { .. });

        let completion = self
            .store
            .complete_task_with_outcome(task_id, lease_id, node_id, session_id, result)
            .await?;
        let response = completion.response;
        let task_model = completion.model;
        record_node_task_completion(
            &response.action,
            completion.is_new_task_transition,
            completion.attempt_started_at,
            chrono::Utc::now(),
        );

        // The claim endpoint and completion endpoint are separate requests, so recover the
        // attempt by the immutable (task, lease) identity. Read from the writer because the
        // completion transaction committed immediately before this recovery step.
        if let Ok(Some(row)) = self
            .store
            .pool()
            .write_conn()
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, request_id FROM gateway_request_attempts WHERE node_task_id=$1 AND lease_id=$2",
                [task_id.into(), lease_id.into()],
            ))
            .await
        {
            let attempt_id: Result<Uuid, _> = row.try_get("", "id");
            let request_id: Result<Uuid, _> = row.try_get("", "request_id");
            if let (Ok(attempt_id), Ok(request_id)) = (attempt_id, request_id) {
                let finish = node_completion_finish(
                    &response.action,
                    attempt_id,
                    request_id,
                    chrono::Utc::now(),
                );
                let _ = self
                    .lifecycle
                    .finish_attempt_and_request(finish)
                    .await
                    .map_err(|error| tracing::warn!(request_id=%request_id, task_id=%task_id, %error, "failed to finish node trace"));
            }
        }

        // A recovery sweep can race with claim and leave a stale List element
        // after the successful BRPOP. Terminal completion removes every copy;
        // requeue replaces any stale copies with one fresh queue entry below.
        if !matches!(&response.action, NodeTaskCompleteAction::Requeued)
            && let Err(error) = self
                .redis
                .remove_from_model_queue(&task_model, task_id)
                .await
        {
            tracing::warn!(
                task_id = %task_id,
                model = %task_model,
                %error,
                "Failed to remove terminal task from Redis queue"
            );
        }

        // 推送结果通知到 Redis(best-effort)
        match response.action {
            NodeTaskCompleteAction::Succeeded => {
                let notification_status = if is_image {
                    "image_succeeded"
                } else {
                    "succeeded"
                };
                if let Err(e) = self
                    .redis
                    .push_result_notification(task_id, notification_status)
                    .await
                {
                    tracing::warn!(
                        "Failed to push succeeded notification for task {}: {}",
                        task_id,
                        e
                    );
                }
            }
            NodeTaskCompleteAction::Requeued => {
                // 使用完成事务返回的 writer-fresh model，原子收敛为一个队列条目。
                if let Err(e) = self.redis.repush_queued_task(&task_model, task_id).await {
                    tracing::warn!("Failed to repush requeued task {} to queue: {}", task_id, e);
                }
            }
            NodeTaskCompleteAction::Failed => {
                if let Err(e) = self.redis.push_result_notification(task_id, "failed").await {
                    tracing::warn!(
                        "Failed to push failed notification for task {}: {}",
                        task_id,
                        e
                    );
                }
            }
            NodeTaskCompleteAction::Expired => {
                if let Err(e) = self
                    .redis
                    .push_result_notification(task_id, "expired")
                    .await
                {
                    tracing::warn!(
                        "Failed to push expired notification for task {}: {}",
                        task_id,
                        e
                    );
                }
            }
        }

        Ok(response)
    }
}

fn decode_chat_task_result(task: &NodeTask) -> Result<ChatCompletionResponse, NodeExecutionError> {
    match task.status.as_str() {
        "succeeded" => {
            let response = serde_json::from_value(task.result_json.clone().ok_or_else(|| {
                NodeExecutionError::other(
                    anyhow::anyhow!("Task succeeded but no result_json"),
                    invalid_node_result_failure(),
                )
            })?)
            .map_err(|error| {
                NodeExecutionError::other(anyhow::Error::from(error), invalid_node_result_failure())
            })?;
            Ok(response)
        }
        "image_succeeded" => Err(NodeExecutionError::other(
            anyhow::anyhow!(
                "Image task succeeded but enqueue_and_wait does not support image results yet"
            ),
            invalid_node_result_failure(),
        )),
        "failed" => {
            let error = task.error_json.clone().unwrap_or(serde_json::json!({}));
            let is_client_error = error
                .get("is_client_error")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let code = error
                .get("code")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown")
                .to_string();
            let message = error
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("Task failed")
                .to_string();
            if is_client_error {
                Err(NodeExecutionError::ClientError { code, message })
            } else {
                Err(NodeExecutionError::other(
                    anyhow::anyhow!("Task failed: {code} - {message}"),
                    node_completion_failure(&NodeTaskCompleteAction::Failed)
                        .expect("failed Node tasks have a request failure"),
                ))
            }
        }
        "expired" => Err(NodeExecutionError::other(
            anyhow::anyhow!("Task expired"),
            node_completion_failure(&NodeTaskCompleteAction::Expired)
                .expect("expired Node tasks have a request failure"),
        )),
        status => Err(NodeExecutionError::other(
            anyhow::anyhow!("Unknown task status: {status}"),
            invalid_node_result_failure(),
        )),
    }
}

async fn mark_node_request_missing_attempt(
    lifecycle: &Arc<dyn RequestLifecycleRecorder>,
    task: &NodeTask,
) {
    if task.lease_id.is_some()
        && let Err(error) = lifecycle.mark_trace_partial(task.request_id).await
    {
        tracing::warn!(request_id=%task.request_id, task_id=%task.id, %error, "failed to mark node trace partial after missing attempt recovery");
    }
}

fn classify_node_attempt_kind(failure_count: i32) -> AttemptKind {
    if failure_count > 0 {
        AttemptKind::Reclaim
    } else {
        AttemptKind::Primary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keycompute_types::{AttemptStatus, StreamEndReason, TraceErrorCategory};

    fn terminal_test_task(lease_id: Option<Uuid>) -> NodeTask {
        let now = chrono::Utc::now();
        NodeTask {
            id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            model: "test-model".to_string(),
            payload_json: serde_json::json!({}),
            status: "succeeded".to_string(),
            assigned_node_id: lease_id.map(|_| Uuid::new_v4()),
            assigned_session_id: lease_id.map(|_| Uuid::new_v4()),
            lease_id,
            failure_count: 0,
            failure_threshold: 3,
            result_json: Some(serde_json::json!({})),
            error_json: None,
            queued_at: now,
            claimed_at: lease_id.map(|_| now),
            finished_at: Some(now),
            deadline_at: now + chrono::Duration::minutes(1),
            complete_grace_until: now + chrono::Duration::minutes(2),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn classifies_first_claim_and_reclaim() {
        assert_eq!(classify_node_attempt_kind(0), AttemptKind::Primary);
        assert_eq!(classify_node_attempt_kind(1), AttemptKind::Reclaim);
        assert_eq!(classify_node_attempt_kind(3), AttemptKind::Reclaim);
    }

    #[test]
    fn node_wait_timeout_closes_attempt_and_preserves_request_failure() {
        let request_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let finish = node_wait_timeout_finish(request_id, attempt_id, chrono::Utc::now());

        assert_eq!(finish.request_id, request_id);
        assert_eq!(finish.attempt_id, attempt_id);
        assert_eq!(finish.attempt_status, AttemptStatus::Expired);
        assert_eq!(finish.request_status, RequestStatus::Running);
        assert!(finish.is_final);
        assert_eq!(finish.billing_status, BillingStatus::Pending);
        assert_eq!(finish.stream_end_reason, Some(StreamEndReason::Timeout));
        assert_eq!(
            finish.error.expect("timeout error").code,
            "node_wait_timeout"
        );

        let failure = node_wait_timeout_failure();
        assert_eq!(failure.status, RequestStatus::TimedOut);
        assert_eq!(failure.billing_status, BillingStatus::NotApplicable);
        assert_eq!(failure.error.code, "node_wait_timeout");
    }

    #[test]
    fn invalid_successful_node_result_is_not_exposed_as_a_chat_response() {
        let mut task = terminal_test_task(Some(Uuid::new_v4()));
        assert!(decode_chat_task_result(&task).is_err());

        task.result_json = Some(serde_json::json!({
            "id": "response-id",
            "object": "chat.completion",
            "created": 0,
            "model": "test-model",
            "choices": [],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
        }));
        let response = decode_chat_task_result(&task).expect("valid chat result should decode");
        assert_eq!(response.id, "response-id");

        task.status = "image_succeeded".to_string();
        assert!(decode_chat_task_result(&task).is_err());
    }

    #[test]
    fn invalid_node_result_fails_the_request_without_billing() {
        let failure = invalid_node_result_failure();

        assert_eq!(failure.status, RequestStatus::Failed);
        assert_eq!(failure.billing_status, BillingStatus::NotApplicable);
        let error = failure.error;
        assert_eq!(error.origin, ErrorOrigin::Node);
        assert_eq!(error.category, TraceErrorCategory::Protocol);
        assert_eq!(error.code, "node_result_invalid");
    }

    #[test]
    fn requeued_completion_returns_the_request_to_the_queue() {
        let finish = node_completion_finish(
            &NodeTaskCompleteAction::Requeued,
            Uuid::new_v4(),
            Uuid::new_v4(),
            chrono::Utc::now(),
        );

        assert_eq!(finish.attempt_status, AttemptStatus::Failed);
        assert_eq!(finish.request_status, RequestStatus::Queued);
        assert!(!finish.is_final);
        assert_eq!(finish.billing_status, BillingStatus::Pending);
        assert_eq!(
            finish.stream_end_reason,
            Some(StreamEndReason::UpstreamError)
        );
        assert_eq!(finish.error.expect("requeue error").code, "node_requeued");
    }

    #[test]
    fn successful_completion_closes_attempt_but_keeps_request_running() {
        let finish = node_completion_finish(
            &NodeTaskCompleteAction::Succeeded,
            Uuid::new_v4(),
            Uuid::new_v4(),
            chrono::Utc::now(),
        );

        assert_eq!(finish.attempt_status, AttemptStatus::Succeeded);
        assert_eq!(finish.request_status, RequestStatus::Running);
        assert!(finish.is_final);
        assert_eq!(finish.billing_status, BillingStatus::Pending);
        assert_eq!(finish.stream_end_reason, Some(StreamEndReason::Completed));
        assert!(finish.error.is_none());
    }

    #[test]
    fn terminal_node_failures_close_attempt_but_leave_request_to_handler() {
        for (action, attempt_status, request_status, code) in [
            (
                NodeTaskCompleteAction::Failed,
                AttemptStatus::Failed,
                RequestStatus::Failed,
                "node_failed",
            ),
            (
                NodeTaskCompleteAction::Expired,
                AttemptStatus::Expired,
                RequestStatus::TimedOut,
                "node_expired",
            ),
        ] {
            let finish =
                node_completion_finish(&action, Uuid::new_v4(), Uuid::new_v4(), chrono::Utc::now());

            assert_eq!(finish.attempt_status, attempt_status);
            assert_eq!(finish.request_status, RequestStatus::Running);
            assert!(finish.is_final);
            assert_eq!(finish.billing_status, BillingStatus::Pending);

            let failure = node_completion_failure(&action)
                .expect("terminal Node failures preserve a handler request failure");
            assert_eq!(failure.status, request_status);
            assert_eq!(failure.billing_status, BillingStatus::NotApplicable);
            assert_eq!(failure.error.code, code);
        }
    }

    #[tokio::test]
    async fn leased_terminal_task_without_attempt_is_marked_partial() {
        let task = terminal_test_task(Some(Uuid::new_v4()));
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let lifecycle = Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>;

        mark_node_request_missing_attempt(&lifecycle, &task).await;

        assert_eq!(
            recorder.events(),
            vec![format!("trace_partial:{}", task.request_id)]
        );
    }

    #[tokio::test]
    async fn never_leased_terminal_task_does_not_invent_a_missing_attempt() {
        let task = terminal_test_task(None);
        let recorder = Arc::new(keycompute_types::TestRequestLifecycleRecorder::default());
        let lifecycle = Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>;

        mark_node_request_missing_attempt(&lifecycle, &task).await;

        assert!(recorder.events().is_empty());
    }
}
