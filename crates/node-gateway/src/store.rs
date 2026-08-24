//! Node Gateway Store 模块
//!
//! 数据库操作层，封装所有节点相关的数据库操作。

use crate::{config::NodeGatewayAppConfig, trace::node_completion_finish};
use chrono::{DateTime, Utc};
use keycompute_db::DbError;
use keycompute_db::DbRouter;
use keycompute_db::models::{
    node::*, node_session::*, node_task::*, node_task_submission::*, user_node_gateway_token::*,
};
use keycompute_types::node::*;
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde_json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

const ACTIVE_NODE_SESSION_FOR_UPDATE_SQL: &str = "SELECT * FROM node_sessions WHERE id = $1 AND node_id = $2 \
     AND revoked_at IS NULL AND expires_at > NOW() FOR UPDATE";

/// Node Gateway Store
#[derive(Clone)]
pub struct NodeGatewayStore {
    pool: Arc<DbRouter>,
    config: NodeGatewayAppConfig,
}

pub(crate) struct NodeTaskCompletionOutcome {
    pub(crate) response: NodeTaskCompleteResponse,
    pub(crate) model: String,
    pub(crate) is_new_task_transition: bool,
    pub(crate) attempt_started_at: Option<DateTime<Utc>>,
}

impl NodeGatewayStore {
    /// 创建新的 Store 实例
    pub fn new(pool: Arc<DbRouter>, config: NodeGatewayAppConfig) -> Self {
        Self { pool, config }
    }

    /// 获取 pool 引用
    pub fn pool(&self) -> &DbRouter {
        self.pool.as_ref()
    }

    pub(crate) fn pool_arc(&self) -> Arc<DbRouter> {
        Arc::clone(&self.pool)
    }

    async fn rollback_trace_savepoint(
        tx: &DatabaseTransaction,
        savepoint: &str,
        request_id: Uuid,
        phase: &str,
    ) -> bool {
        let rollback = format!("ROLLBACK TO SAVEPOINT {savepoint}");
        if let Err(error) = tx.execute_unprepared(&rollback).await {
            tracing::error!(%request_id, phase, %error, "failed to roll back monitoring savepoint");
            return false;
        }
        let release = format!("RELEASE SAVEPOINT {savepoint}");
        if let Err(error) = tx.execute_unprepared(&release).await {
            tracing::error!(%request_id, phase, %error, "failed to release rolled-back monitoring savepoint");
            return false;
        }
        true
    }

    async fn mark_node_trace_partial_savepoint(
        tx: &DatabaseTransaction,
        request_id: Uuid,
        phase: &str,
    ) {
        const SAVEPOINT: &str = "monitoring_trace_partial";
        if let Err(error) = tx
            .execute_unprepared("SAVEPOINT monitoring_trace_partial")
            .await
        {
            tracing::warn!(%request_id, phase, %error, "failed to create degraded trace savepoint");
            return;
        }
        let result = async {
            tx.execute_unprepared("SET LOCAL statement_timeout = '250ms'")
                .await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE gateway_requests SET trace_quality='partial',updated_at=NOW() WHERE request_id=$1",
                [request_id.into()],
            ))
            .await?;
            tx.execute_unprepared("SET LOCAL statement_timeout = DEFAULT")
                .await?;
            Ok::<(), sea_orm::DbErr>(())
        }
        .await;
        match result {
            Ok(()) => {
                if let Err(error) = tx
                    .execute_unprepared("RELEASE SAVEPOINT monitoring_trace_partial")
                    .await
                {
                    tracing::warn!(%request_id, phase, %error, "failed to release degraded trace savepoint");
                    let _ = Self::rollback_trace_savepoint(tx, SAVEPOINT, request_id, phase).await;
                }
            }
            Err(error) => {
                tracing::warn!(%request_id, phase, %error, "failed to mark node trace partial");
                let _ = Self::rollback_trace_savepoint(tx, SAVEPOINT, request_id, phase).await;
            }
        }
    }

    /// 计算 request_hash (canonical JSON hash)
    /// request_hash 只覆盖 task_id + lease_id + result
    fn compute_request_hash(
        task_id: Uuid,
        lease_id: Uuid,
        result: &NodeTaskResult,
    ) -> Result<String, DbError> {
        let hash_input = serde_json::json!({
            "task_id": task_id,
            "lease_id": lease_id,
            "result": result,
        });

        let canonical_json = serde_json::to_string(&hash_input)
            .map_err(|e| DbError::Other(format!("Failed to serialize hash input: {}", e)))?;

        let mut hasher = Sha256::new();
        hasher.update(canonical_json.as_bytes());
        let hash_bytes = hasher.finalize();

        // 转换为 hex 字符串
        Ok(format!("{:x}", hash_bytes))
    }

    /// 注册节点
    ///
    /// 认证策略：
    /// 1. HMAC 签名验证 → 解析 token_id
    /// 2. 查 DB 确认 token 状态为 `approved`
    /// 3. 在事务中原子消费 token（一次性）+ 创建节点
    ///
    /// 不再支持全局 fallback token。
    pub async fn register_node(
        &self,
        req: &NodeRegisterRequest,
    ) -> Result<NodeRegisterResponse, DbError> {
        // 0. HMAC 签名验证（O(1) 内存操作，零 DB 查询）
        let token_id = UserNodeGatewayToken::validate_hmac_token(
            &req.registration_token,
            self.config.registration_token_secret.as_bytes(),
        )
        .map_err(|e| DbError::Other(format!("Invalid registration token: {}", e)))?;

        let now = Utc::now();

        // 1. 开始事务（通过 FOR UPDATE 行级锁防止 TOCTOU，使用默认 READ COMMITTED 隔离级别）
        let tx = self.pool.begin().await?;

        // 2. 在事务内查询 token 并检查是否可消费（消除 TOCTOU 窗口）
        //    使用 FOR UPDATE 锁定行，防止并发修改
        let token = UserNodeGatewayToken::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT * FROM user_node_gateway_tokens
            WHERE id = $1
            FOR UPDATE
            "#,
            [token_id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::Other("Registration token not found".to_string()))?;

        if !token.is_consumable() {
            let status_msg = match token.status.as_str() {
                "consumed" =>
                    "Token has already been consumed by another node registration. Each token can only be used once."
                        .to_string(),
                "rejected" =>
                    "Token was rejected by admin. Please re-apply.".to_string(),
                _ => format!(
                    "Token is not approved (current status: {}). Please wait for admin approval.",
                    token.status
                ),
            };
            return Err(DbError::Other(status_msg));
        }

        let owner_user_id = token.user_id;

        // 3. 查找或创建节点(在事务中)
        let existing_node = Node::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT * FROM nodes
            WHERE owner_user_id = $1 AND client_instance_id = $2
            "#,
            [owner_user_id.into(), req.client_instance_id.as_str().into()],
        ))
        .one(&tx)
        .await?;

        let node = match existing_node {
            Some(existing_node) => {
                // 如果节点被排除，拒绝注册
                if existing_node.is_excluded() {
                    return Err(DbError::Other(
                        "Node is excluded, cannot re-register".to_string(),
                    ));
                }
                existing_node
            }
            None => {
                // 创建新节点(在事务中)
                let capabilities_json = serde_json::to_value(&req.capabilities)
                    .map_err(|e| DbError::Other(e.to_string()))?;

                Node::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"
                    INSERT INTO nodes (owner_user_id, client_instance_id, display_name, status, capabilities_json)
                    VALUES ($1, $2, $3, $4, $5)
                    RETURNING *
                    "#,
                    [
                        owner_user_id.into(),
                        req.client_instance_id.as_str().into(),
                        req.display_name.as_str().into(),
                        NODE_STATUS_OFFLINE.into(),
                        capabilities_json.clone().into(),
                    ],
                ))
                .one(&tx)
                .await?
                .ok_or_else(|| DbError::Other("Failed to create node".to_string()))?
            }
        };

        // 4. 在同一事务中：消费 token（一次性使用）+ 创建 session + 更新节点状态
        let consume_stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE user_node_gateway_tokens SET status = 'consumed', consumed_at = NOW(), consumed_node_id = $1, updated_at = NOW() WHERE id = $2 AND status = 'approved'"#,
            [node.id.into(), token_id.into()],
        );
        let consume_result = tx.execute(consume_stmt).await?;
        let consumed = consume_result.rows_affected() > 0;
        if !consumed {
            // Token 可能在事务外检查通过后、事务内 consume 之前被 admin reject 或并发消费
            return Err(DbError::Other(
                "Token is no longer valid (may have been rejected or consumed by another request). Please re-apply for a new token.".to_string(),
            ));
        }

        let session_token = Uuid::new_v4().to_string();
        let session_token_hash = UserNodeGatewayToken::hash_token(&session_token);
        let expires_at = now + self.config.session_ttl();

        // 提取注册能力中的模型名
        let accepted_models: Vec<String> = req
            .capabilities
            .models
            .iter()
            .map(|m| m.model.clone())
            .collect();

        let create_session_req = CreateNodeSessionRequest {
            node_id: node.id,
            session_token_hash,
            expires_at,
            accepted_models_json: serde_json::to_value(&accepted_models)
                .map_err(|e| DbError::Other(e.to_string()))?,
        };

        // 4.1 创建 session (在事务中)
        let session = NodeSession::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_sessions (node_id, session_token_hash, expires_at, accepted_models_json)
            VALUES ($1, $2, $3, $4)
            RETURNING *
            "#,
            [
                create_session_req.node_id.into(),
                create_session_req.session_token_hash.as_str().into(),
                create_session_req.expires_at.into(),
                create_session_req.accepted_models_json.clone().into(),
            ],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::Other("Failed to create session".to_string()))?;

        // 4.2 更新节点状态为 online (如果原来是 offline,在事务中)
        if node.status == NODE_STATUS_OFFLINE {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE nodes
                SET status = 'online', updated_at = NOW()
                WHERE id = $1
                "#,
                [node.id.into()],
            ))
            .await?;
        }

        // 4.3 更新节点心跳时间 (在事务中)
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            UPDATE nodes
            SET last_heartbeat_at = NOW(), updated_at = NOW()
            WHERE id = $1
            "#,
            [node.id.into()],
        ))
        .await?;

        // 提交事务
        tx.commit().await?;

        Ok(NodeRegisterResponse {
            protocol_version: "node.v1".to_string(),
            node_id: node.id,
            session_id: session.id,
            session_token,
            heartbeat_interval_secs: self.config.heartbeat_interval_secs,
            poll_timeout_secs: self.config.poll_timeout_secs,
        })
    }

    /// 认证 session token
    pub async fn authenticate_session(
        &self,
        session_token: &str,
    ) -> Result<(Node, NodeSession), DbError> {
        let token_hash = UserNodeGatewayToken::hash_token(session_token);

        // Authentication and exclusion checks must not observe stale replicas.
        let session = NodeSession::find_by_token_hash(self.pool.write_conn(), &token_hash).await?;

        match session {
            Some(s) => {
                // Internal callers do not go through the HTTP extractor, so
                // enforce the same expiry/revocation boundary here as well.
                if !s.is_valid() {
                    return Err(DbError::Other("Session expired or revoked".to_string()));
                }

                let node = Node::find_by_id(self.pool.write_conn(), s.node_id)
                    .await?
                    .ok_or_else(|| DbError::not_found("Node", s.node_id.to_string()))?;

                Ok((node, s))
            }
            None => Err(DbError::not_found("Session", "token")),
        }
    }

    /// Admin 把 excluded 节点恢复为 online
    /// 同时清零 consecutive_failure_count, 节点可重新接收任务。
    pub async fn recover_node(&self, node_id: Uuid) -> Result<Node, DbError> {
        Node::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            UPDATE nodes
            SET status = 'online',
                consecutive_failure_count = 0,
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            [node_id.into()],
        ))
        .one(self.pool.as_ref())
        .await?
        .ok_or_else(|| DbError::not_found("Node", node_id.to_string()))
    }

    pub async fn heartbeat(
        &self,
        node_id: Uuid,
        session_id: Uuid,
        accepted_models: Vec<String>,
    ) -> Result<NodeHeartbeatResponse, DbError> {
        let tx = self.pool.begin().await?;
        let now = Utc::now();
        let expires_at = now + self.config.session_ttl();

        // 1. 获取节点和会话(FOR UPDATE)
        let node = Node::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM nodes WHERE id = $1 FOR UPDATE",
            [node_id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::not_found("Node", node_id.to_string()))?;

        let session = NodeSession::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            ACTIVE_NODE_SESSION_FOR_UPDATE_SQL,
            [session_id.into(), node_id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::not_found("active session", session_id.to_string()))?;

        // 2. 校验请求体与认证结果一致
        if session.node_id != node_id {
            return Err(DbError::Other("Session node_id mismatch".to_string()));
        }

        // 3. 根据节点状态分支处理
        if node.is_excluded() {
            // excluded 节点:只更新会话可见性,不改变节点状态
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE node_sessions
                SET last_seen_at = NOW(), expires_at = $1
                WHERE id = $2
                "#,
                [expires_at.into(), session_id.into()],
            ))
            .await?;
        } else {
            // 非 excluded 节点:校验并持久化 accepted_models
            let capabilities: NodeCapabilities =
                serde_json::from_value(node.capabilities_json.clone())
                    .map_err(|e| DbError::Other(format!("Invalid capabilities_json: {}", e)))?;

            let registered_models: Vec<String> = capabilities
                .models
                .iter()
                .map(|m| m.model.clone())
                .collect();

            // 校验 accepted_models 是 registered_models 的子集
            for model in &accepted_models {
                if !registered_models.contains(model) {
                    return Err(DbError::Other(format!(
                        "Model {} is not in registered capabilities",
                        model
                    )));
                }
            }

            // 在同一事务中:
            // 1) 更新会话的 accepted_models 和可见性
            let accepted_models_value = serde_json::to_value(&accepted_models)
                .map_err(|e| DbError::Other(e.to_string()))?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE node_sessions
                SET accepted_models_json = $1, last_seen_at = NOW(), expires_at = $2
                WHERE id = $3
                "#,
                [
                    accepted_models_value.into(),
                    expires_at.into(),
                    session_id.into(),
                ],
            ))
            .await?;

            // 2) 更新节点状态为 online(如果原来是 offline)
            if node.status != NODE_STATUS_ONLINE {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"
                    UPDATE nodes
                    SET status = 'online', updated_at = NOW()
                    WHERE id = $1
                    "#,
                    [node_id.into()],
                ))
                .await?;
            }

            // 3) 更新节点心跳时间
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE nodes
                SET last_heartbeat_at = NOW(), updated_at = NOW()
                WHERE id = $1
                "#,
                [node_id.into()],
            ))
            .await?;
        }

        // 提交前确定最终节点状态——避免事务提交后通过读库查询可能因复制延迟读到过期数据
        let (final_status, final_failure_count) = if node.is_excluded() {
            // excluded 分支：不改变节点状态和失败计数
            (node.status.clone(), node.consecutive_failure_count)
        } else {
            // 非 excluded 分支：节点状态被设为 online（覆盖原值），失败计数不变
            (
                NODE_STATUS_ONLINE.to_string(),
                node.consecutive_failure_count,
            )
        };

        // 提交事务
        tx.commit().await?;

        Ok(NodeHeartbeatResponse {
            protocol_version: "node.v1".to_string(),
            accepted: true,
            node_status: final_status,
            server_failure_count: final_failure_count as u32,
            failure_threshold: node.failure_threshold as u32,
        })
    }

    /// 创建任务并推入队列
    pub async fn create_and_enqueue_task(
        &self,
        user_id: Uuid,
        model: String,
        payload: NodeTaskPayload,
    ) -> Result<NodeTask, DbError> {
        let now = Utc::now();
        let deadline_at = now + self.config.task_deadline();
        let complete_grace_until = deadline_at + self.config.complete_grace();

        let create_req = CreateNodeTaskRequest {
            request_id: payload.request_id,
            user_id,
            model: model.clone(),
            payload_json: serde_json::to_value(&payload)
                .map_err(|e| DbError::Other(e.to_string()))?,
            deadline_at,
            complete_grace_until,
        };

        let task = NodeTask::create(self.pool.as_ref(), &create_req).await?;

        // 注意：Redis 推送由上层调用方负责
        Ok(task)
    }

    /// 原子领取任务（claim）
    pub async fn claim_task(
        &self,
        task_id: Uuid,
        node_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<(NodeTask, NodeTaskEnvelope)>, DbError> {
        let lease_id = Uuid::new_v4();

        let tx = self.pool.begin().await?;
        let task = NodeTask::claim(&tx, task_id, node_id, session_id, lease_id).await?;

        if let Some(task) = task.as_ref() {
            Self::record_node_claim_savepoint(&tx, task).await;
        }
        tx.commit().await?;

        match task {
            Some(t) => {
                let payload: NodeTaskPayload = serde_json::from_value(t.payload_json.clone())
                    .map_err(|e| DbError::Other(format!("Invalid payload: {}", e)))?;

                let envelope = NodeTaskEnvelope {
                    task_id: t.id,
                    lease_id,
                    model: t.model.clone(),
                    deadline_unix_ms: t.deadline_at.timestamp_millis(),
                    complete_grace_until_unix_ms: t.complete_grace_until.timestamp_millis(),
                    payload,
                };

                Ok(Some((t, envelope)))
            }
            None => Ok(None),
        }
    }

    /// Monitoring writes share the claim transaction but are isolated behind
    /// a savepoint, so trace failures never roll back the Node state machine.
    async fn record_node_claim_savepoint(tx: &DatabaseTransaction, task: &NodeTask) {
        if let Err(error) = tx.execute_unprepared("SAVEPOINT monitoring_trace").await {
            tracing::warn!(request_id=%task.request_id,task_id=%task.id,%error,"failed to create node claim trace savepoint");
            return;
        }
        let trace_result = async {
            tx.execute_unprepared("SET LOCAL statement_timeout = '250ms'")
                .await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO gateway_request_attempts(
                       request_id,attempt_no,attempt_kind,route_type,model,status,
                       node_task_id,node_id,session_id,lease_id,started_at)
                   SELECT gr.request_id,
                          COALESCE((SELECT MAX(a.attempt_no) FROM gateway_request_attempts a WHERE a.request_id=gr.request_id),0)+1,
                          CASE WHEN $2>0 THEN 'reclaim' ELSE 'primary' END,
                          'node',$3,'running',$4,$5,$6,$7,$8
                   FROM gateway_requests gr
                   WHERE gr.request_id=$1 AND gr.finished_at IS NULL
                     AND NOT EXISTS(SELECT 1 FROM gateway_request_attempts a WHERE a.node_task_id=$4 AND a.lease_id=$7)"#,
                [
                    task.request_id.into(),task.failure_count.into(),task.model.clone().into(),task.id.into(),
                    task.assigned_node_id.into(),task.assigned_session_id.into(),task.lease_id.into(),
                    task.claimed_at.unwrap_or_else(Utc::now).into(),
                ],
            )).await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE gateway_requests SET route_type='node',status='running',updated_at=NOW() WHERE request_id=$1 AND finished_at IS NULL",
                [task.request_id.into()],
            )).await?;
            tx.execute_unprepared("SET LOCAL statement_timeout = DEFAULT")
                .await?;
            Ok::<(), sea_orm::DbErr>(())
        }.await;
        match trace_result {
            Ok(()) => {
                if let Err(error) = tx
                    .execute_unprepared("RELEASE SAVEPOINT monitoring_trace")
                    .await
                {
                    tracing::warn!(request_id=%task.request_id,task_id=%task.id,%error,"failed to release node claim trace savepoint");
                    if Self::rollback_trace_savepoint(
                        tx,
                        "monitoring_trace",
                        task.request_id,
                        "claim",
                    )
                    .await
                    {
                        Self::mark_node_trace_partial_savepoint(tx, task.request_id, "claim").await;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(request_id=%task.request_id,task_id=%task.id,%error,"node claim trace savepoint rolled back");
                if Self::rollback_trace_savepoint(tx, "monitoring_trace", task.request_id, "claim")
                    .await
                {
                    Self::mark_node_trace_partial_savepoint(tx, task.request_id, "claim").await;
                }
            }
        }
    }

    pub(crate) async fn finish_node_trace_savepoint(
        tx: &DatabaseTransaction,
        task: &NodeTask,
        lease_id: Option<Uuid>,
        action: NodeTaskCompleteAction,
    ) {
        if let Err(error) = tx.execute_unprepared("SAVEPOINT monitoring_trace").await {
            tracing::warn!(request_id=%task.request_id,task_id=%task.id,%error,"failed to create node completion trace savepoint");
            return;
        }
        let finish = node_completion_finish(
            &action,
            Uuid::nil(),
            task.request_id,
            task.finished_at.unwrap_or_else(Utc::now),
        );
        let attempt_status = finish.attempt_status.as_str();
        let request_status = finish.request_status.as_str();
        let is_final = finish.is_final;
        let end_reason = finish
            .stream_end_reason
            .expect("Node completion always has an end reason")
            .as_str();
        let error_category = finish.error.as_ref().map(|error| error.category.as_str());
        let error_code = finish.error.as_ref().map(|error| error.code.as_str());
        let trace_result = async {
            tx.execute_unprepared("SET LOCAL statement_timeout = '250ms'")
                .await?;
            let mut missing_attempt = false;
            if let Some(lease_id)=lease_id {
                let updated = tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"UPDATE gateway_request_attempts SET status=$1,is_final=$2,stream_end_reason=$3,
                         stream_error_count=CASE WHEN $1='succeeded' THEN 0 ELSE 1 END,
                         error_origin=CASE WHEN $1='succeeded' THEN NULL ELSE 'node' END,
                         error_category=$4,error_code=$5,finished_at=NOW(),updated_at=NOW()
                       WHERE node_task_id=$6 AND lease_id=$7 AND finished_at IS NULL"#,
                    [attempt_status.into(),is_final.into(),end_reason.into(),error_category.into(),error_code.into(),task.id.into(),lease_id.into()],
                )).await?;
                if updated.rows_affected() != 1 {
                    // The request-side wait timeout can close this attempt
                    // before the sweeper transitions the business task to
                    // expired. That is an expected idempotent replay, not a
                    // missing trace. Only degrade when the lease attempt is
                    // absent or disagrees with the terminal state.
                    let existing = tx.query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT status,is_final FROM gateway_request_attempts WHERE node_task_id=$1 AND lease_id=$2",
                        [task.id.into(), lease_id.into()],
                    )).await?;
                    let idempotent = existing
                        .and_then(|row| {
                            Some((
                                row.try_get::<String>("", "status").ok()?,
                                row.try_get::<bool>("", "is_final").ok()?,
                            ))
                        })
                        .is_some_and(|(status, existing_is_final)| {
                            status == attempt_status && existing_is_final == is_final
                        });
                    missing_attempt = !idempotent;
                }
            }
            if missing_attempt {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE gateway_requests SET trace_quality='partial',updated_at=NOW() WHERE request_id=$1",
                    [task.request_id.into()],
                )).await?;
            }
            // Node completion owns the attempt only. The client-facing handler
            // writes the terminal request outcome after delivery (or disconnect).
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE gateway_requests SET status=$1,updated_at=NOW() WHERE request_id=$2 AND finished_at IS NULL",
                [request_status.into(),task.request_id.into()],
            )).await?;
            tx.execute_unprepared("SET LOCAL statement_timeout = DEFAULT")
                .await?;
            Ok::<(),sea_orm::DbErr>(())
        }.await;
        match trace_result {
            Ok(()) => {
                if let Err(error) = tx
                    .execute_unprepared("RELEASE SAVEPOINT monitoring_trace")
                    .await
                {
                    tracing::warn!(request_id=%task.request_id,task_id=%task.id,%error,"failed to release node completion trace savepoint");
                    if Self::rollback_trace_savepoint(
                        tx,
                        "monitoring_trace",
                        task.request_id,
                        "completion",
                    )
                    .await
                    {
                        Self::mark_node_trace_partial_savepoint(tx, task.request_id, "completion")
                            .await;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(request_id=%task.request_id,task_id=%task.id,%error,"node completion trace savepoint rolled back");
                if Self::rollback_trace_savepoint(
                    tx,
                    "monitoring_trace",
                    task.request_id,
                    "completion",
                )
                .await
                {
                    Self::mark_node_trace_partial_savepoint(tx, task.request_id, "completion")
                        .await;
                }
            }
        }
    }

    /// 完成任务提交（复杂的事务逻辑）
    pub async fn complete_task(
        &self,
        task_id: Uuid,
        lease_id: Uuid,
        authenticated_node_id: Uuid,
        authenticated_session_id: Uuid,
        result: NodeTaskResult,
    ) -> Result<NodeTaskCompleteResponse, DbError> {
        Ok(self
            .complete_task_with_outcome(
                task_id,
                lease_id,
                authenticated_node_id,
                authenticated_session_id,
                result,
            )
            .await?
            .response)
    }

    /// 与 `complete_task` 相同，但保留本次调用是否推进了任务状态，供指标去重。
    pub(crate) async fn complete_task_with_outcome(
        &self,
        task_id: Uuid,
        lease_id: Uuid,
        authenticated_node_id: Uuid,
        authenticated_session_id: Uuid,
        result: NodeTaskResult,
    ) -> Result<NodeTaskCompletionOutcome, DbError> {
        let tx = self.pool.begin().await?;

        // 1. 查询任务（FOR UPDATE）
        let task = NodeTask::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM node_tasks WHERE id = $1 FOR UPDATE",
            [task_id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::not_found("NodeTask", task_id.to_string()))?;

        // 2. 查询已有 submission
        let existing_submission =
            NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT * FROM node_task_submissions WHERE task_id = $1 AND lease_id = $2",
                [task_id.into(), lease_id.into()],
            ))
            .one(&tx)
            .await?;

        // 3. 如果已有 submission,处理幂等逻辑
        if let Some(submission) = existing_submission {
            // 先检查 session 是否被撤销
            let session = NodeSession::find_by_id(&tx, authenticated_session_id).await?;
            if session.map(|s| s.is_revoked()).unwrap_or(true) {
                return Err(DbError::Other("Session has been revoked".to_string()));
            }

            // 校验 session 身份
            if submission.node_id != authenticated_node_id
                || submission.session_id != authenticated_session_id
            {
                return Err(DbError::Other(
                    "duplicate_submission_session_mismatch".to_string(),
                ));
            }

            // 检查 submission 是否未归档（24 小时内且任务未终态）
            let is_not_archived =
                NodeTaskSubmission::is_not_archived(&tx, task_id, lease_id).await?;

            if is_not_archived {
                // 未归档，检查 request_hash
                let current_request_hash = Self::compute_request_hash(task_id, lease_id, &result)?;

                if submission.request_hash == current_request_hash {
                    // request_hash 相同,直接返回已保存的 ACK
                    let action = parse_action(&submission.action)?;

                    let node = Node::find_by_statement(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT * FROM nodes WHERE id = $1",
                        [authenticated_node_id.into()],
                    ))
                    .one(&tx)
                    .await?
                    .ok_or_else(|| DbError::not_found("Node", authenticated_node_id.to_string()))?;

                    return Ok(NodeTaskCompletionOutcome {
                        response: NodeTaskCompleteResponse {
                            action,
                            task_status: submission.action.clone(),
                            node_status: node.status,
                            server_failure_count: node.consecutive_failure_count as u32,
                            failure_threshold: node.failure_threshold as u32,
                        },
                        model: task.model.clone(),
                        is_new_task_transition: false,
                        attempt_started_at: task.claimed_at,
                    });
                } else {
                    // request_hash 不同，冲突
                    return Err(DbError::Other("duplicate_submission_conflict".to_string()));
                }
            } else {
                // 已归档,仍然返回已保存的 ACK (幂等)
                let action = parse_action(&submission.action)?;

                let node = Node::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT * FROM nodes WHERE id = $1",
                    [authenticated_node_id.into()],
                ))
                .one(&tx)
                .await?
                .ok_or_else(|| DbError::not_found("Node", authenticated_node_id.to_string()))?;

                return Ok(NodeTaskCompletionOutcome {
                    response: NodeTaskCompleteResponse {
                        action,
                        task_status: submission.action.clone(),
                        node_status: node.status,
                        server_failure_count: node.consecutive_failure_count as u32,
                        failure_threshold: node.failure_threshold as u32,
                    },
                    model: task.model.clone(),
                    is_new_task_transition: false,
                    attempt_started_at: task.claimed_at,
                });
            }
        }

        // 4. 无 submission，检查 session 状态
        let session = NodeSession::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM node_sessions WHERE id = $1",
            [authenticated_session_id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::not_found("Session", authenticated_session_id.to_string()))?;

        if session.is_revoked() {
            return Err(DbError::Other("Session revoked".to_string()));
        }

        let node = Node::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM nodes WHERE id = $1",
            [authenticated_node_id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::not_found("Node", authenticated_node_id.to_string()))?;

        let session_expired = session.is_expired();
        let task_expired = task.is_expired();
        let now = Utc::now();

        // 5. 决策树：优先级 1 - Late Expired 判定
        if task_expired
            || (task.status == TASK_STATUS_LEASED && task.deadline_at < now)
            || (session_expired && !node.is_excluded())
        {
            // 条件 (a) 或 (b)：任务已过期
            if task.status == TASK_STATUS_EXPIRED
                || (task.status == TASK_STATUS_LEASED && task.deadline_at < now)
            {
                // Expiry changes the accepted action, not the caller's lease
                // authority. An unrelated authenticated node must not be able
                // to create an audit submission for another node's task.
                if task.assigned_node_id != Some(authenticated_node_id)
                    || task.assigned_session_id != Some(authenticated_session_id)
                    || task.lease_id != Some(lease_id)
                {
                    return Err(DbError::Other("lease_mismatch".to_string()));
                }
                if now > task.complete_grace_until {
                    return Err(DbError::Other("grace_period_expired".to_string()));
                }

                // 在宽限期内，写入 expired submission
                // Hash the actual submitted payload. The stored action is the
                // server's Expired decision, but idempotency is defined over
                // the client's original request; otherwise an exact retry
                // conflicts with the synthetic expired payload.
                let request_hash = Self::compute_request_hash(task_id, lease_id, &result)?;

                let submission_req = CreateNodeTaskSubmissionRequest {
                    task_id,
                    lease_id,
                    node_id: authenticated_node_id,
                    session_id: authenticated_session_id,
                    result_kind: "expired".to_string(),
                    request_hash,
                    action: "expired".to_string(),
                };

                NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"
                    INSERT INTO node_task_submissions (task_id, lease_id, node_id, session_id, result_kind, request_hash, action)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    RETURNING *
                    "#,
                    [
                        submission_req.task_id.into(),
                        submission_req.lease_id.into(),
                        submission_req.node_id.into(),
                        submission_req.session_id.into(),
                        submission_req.result_kind.as_str().into(),
                        submission_req.request_hash.as_str().into(),
                        submission_req.action.as_str().into(),
                    ],
                ))
                .one(&tx)
                .await?
                .ok_or_else(|| DbError::Other("Failed to insert expired submission".to_string()))?;

                // 如果任务不是终态，标记为 expired。若 sweeper 已先完成
                // 该迁移，本次仅保存幂等 submission，不再重复发出指标。
                let is_new_task_transition = !task.is_terminal();
                if is_new_task_transition {
                    NodeTask::find_by_statement(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        r#"
                        UPDATE node_tasks
                        SET status = $1,
                            finished_at = NOW(),
                            updated_at = NOW()
                        WHERE id = $2
                        RETURNING *
                        "#,
                        [TASK_STATUS_EXPIRED.into(), task_id.into()],
                    ))
                    .one(&tx)
                    .await?;
                }

                Self::finish_node_trace_savepoint(
                    &tx,
                    &task,
                    Some(lease_id),
                    NodeTaskCompleteAction::Expired,
                )
                .await;
                tx.commit().await?;

                return Ok(NodeTaskCompletionOutcome {
                    response: NodeTaskCompleteResponse {
                        action: NodeTaskCompleteAction::Expired,
                        task_status: TASK_STATUS_EXPIRED.to_string(),
                        node_status: node.status,
                        server_failure_count: node.consecutive_failure_count as u32,
                        failure_threshold: node.failure_threshold as u32,
                    },
                    model: task.model.clone(),
                    is_new_task_transition,
                    attempt_started_at: task.claimed_at,
                });
            }
            // 条件 (c)：任务未过期但 session 过期，继续到优先级 2-4
        }

        // 6. 决策树：优先级 2 - 任务状态校验
        if task.status != TASK_STATUS_LEASED {
            return Err(DbError::Other("invalid_task_state".to_string()));
        }

        // 7. 决策树：优先级 3 - Lease 校验
        if task.assigned_node_id != Some(authenticated_node_id)
            || task.assigned_session_id != Some(authenticated_session_id)
            || task.lease_id != Some(lease_id)
        {
            return Err(DbError::Other("lease_mismatch".to_string()));
        }

        let payload: NodeTaskPayload = serde_json::from_value(task.payload_json.clone())
            .map_err(|error| DbError::Other(format!("Invalid task payload: {error}")))?;
        payload
            .validate()
            .map_err(|error| DbError::Other(format!("Invalid task payload: {error}")))?;
        if !result_matches_payload(&payload, &result) {
            return Err(DbError::Other("node_result_type_mismatch".to_string()));
        }

        // 8. 决策树：优先级 4 - 正常成功/失败流程
        let response = match result {
            NodeTaskResult::Succeeded { response } => {
                self.handle_success_submission(
                    &tx,
                    &task,
                    &node,
                    authenticated_node_id,
                    authenticated_session_id,
                    lease_id,
                    response,
                )
                .await
            }
            NodeTaskResult::ImageSucceeded { image_response } => {
                self.handle_image_success_submission(
                    &tx,
                    &task,
                    &node,
                    authenticated_node_id,
                    authenticated_session_id,
                    lease_id,
                    image_response,
                )
                .await
            }
            NodeTaskResult::Failed {
                code,
                message,
                is_client_error,
            } => {
                self.handle_failed_submission(
                    &tx,
                    &task,
                    &node,
                    authenticated_node_id,
                    authenticated_session_id,
                    lease_id,
                    code,
                    message,
                    is_client_error,
                )
                .await
            }
        }?;

        // 9. 在同一事务的 SAVEPOINT 中关闭 trace，再提交核心状态。
        Self::finish_node_trace_savepoint(&tx, &task, Some(lease_id), response.action.clone())
            .await;
        tx.commit().await?;

        Ok(NodeTaskCompletionOutcome {
            response,
            model: task.model,
            is_new_task_transition: true,
            attempt_started_at: task.claimed_at,
        })
    }

    /// 处理成功提交（Chat 完成）
    #[allow(clippy::too_many_arguments)]
    async fn handle_success_submission(
        &self,
        tx: &DatabaseTransaction,
        task: &NodeTask,
        node: &Node,
        node_id: Uuid,
        session_id: Uuid,
        lease_id: Uuid,
        response: keycompute_types::ChatCompletionResponse,
    ) -> Result<NodeTaskCompleteResponse, DbError> {
        let response_json =
            serde_json::to_value(&response).map_err(|e| DbError::Other(e.to_string()))?;
        let result_for_hash = NodeTaskResult::Succeeded { response };
        self.handle_success_submission_inner(
            tx,
            task,
            node,
            node_id,
            session_id,
            lease_id,
            response_json,
            "succeeded",
            result_for_hash,
        )
        .await
    }

    /// 处理图片成功提交
    ///
    /// 注意：`ImageGenerationResponse` 中的 `b64_json` 字段可能携带大量 base64 图片数据
    ///（单张可达数 MB），直接存入 `node_tasks.result_json` JSONB 列存在存储膨胀风险。
    /// TODO: 后续考虑将图片数据上传至对象存储（S3/MinIO），DB 仅保留 URL 引用。
    #[allow(clippy::too_many_arguments)]
    async fn handle_image_success_submission(
        &self,
        tx: &DatabaseTransaction,
        task: &NodeTask,
        node: &Node,
        node_id: Uuid,
        session_id: Uuid,
        lease_id: Uuid,
        image_response: keycompute_types::node::ImageGenerationResponse,
    ) -> Result<NodeTaskCompleteResponse, DbError> {
        // 对大体积图片响应记录告警日志
        let b64_total_chars: usize = image_response
            .data
            .iter()
            .filter_map(|d| d.b64_json.as_ref())
            .map(|s| s.len())
            .sum();
        if b64_total_chars > 512 * 1024 {
            tracing::warn!(
                task_id = %task.id,
                b64_json_chars = b64_total_chars,
                image_count = image_response.data.len(),
                "Image response b64_json exceeds 512KB (base64-encoded), ~384KB raw; may cause DB storage bloat"
            );
        }

        let response_json = serde_json::to_value(&image_response)
            .map_err(|e| DbError::Other(format!("Failed to serialize image response: {}", e)))?;
        let result_for_hash = NodeTaskResult::ImageSucceeded { image_response };
        self.handle_success_submission_inner(
            tx,
            task,
            node,
            node_id,
            session_id,
            lease_id,
            response_json,
            "image_succeeded",
            result_for_hash,
        )
        .await
    }

    /// 成功提交的公共逻辑：更新任务状态、清零失败计数、写入 submission ACK
    #[allow(clippy::too_many_arguments)]
    async fn handle_success_submission_inner(
        &self,
        tx: &DatabaseTransaction,
        task: &NodeTask,
        node: &Node,
        node_id: Uuid,
        session_id: Uuid,
        lease_id: Uuid,
        response_json: serde_json::Value,
        result_kind: &str,
        result_for_hash: NodeTaskResult,
    ) -> Result<NodeTaskCompleteResponse, DbError> {
        let updated_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            UPDATE node_tasks
            SET status = $1,
                result_json = $2,
                finished_at = NOW(),
                updated_at = NOW()
            WHERE id = $3
              AND assigned_node_id = $4
              AND assigned_session_id = $5
              AND lease_id = $6
              AND status = 'leased'
              AND deadline_at >= NOW()
            RETURNING *
            "#,
            [
                TASK_STATUS_SUCCEEDED.into(),
                response_json.clone().into(),
                task.id.into(),
                node_id.into(),
                session_id.into(),
                lease_id.into(),
            ],
        ))
        .one(tx)
        .await?;

        let updated_task = match updated_task {
            Some(t) => t,
            None => {
                let current_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT * FROM node_tasks WHERE id = $1",
                    [task.id.into()],
                ))
                .one(tx)
                .await?
                .ok_or_else(|| DbError::not_found("Task", task.id.to_string()))?;

                if current_task.status == TASK_STATUS_EXPIRED
                    || current_task.deadline_at < Utc::now()
                {
                    return Err(DbError::Other("task_expired_during_complete".to_string()));
                } else {
                    return Err(DbError::Other("concurrent_task_update_failed".to_string()));
                }
            }
        };

        // 清零节点连续失败计数（仅非 excluded 节点）
        if !node.is_excluded() {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE nodes
                SET consecutive_failure_count = 0, updated_at = NOW()
                WHERE id = $1
                "#,
                [node_id.into()],
            ))
            .await?;
        }

        // 写入 submission ACK
        let request_hash = Self::compute_request_hash(task.id, lease_id, &result_for_hash)?;

        let submission_req = CreateNodeTaskSubmissionRequest {
            task_id: task.id,
            lease_id,
            node_id,
            session_id,
            result_kind: result_kind.to_string(),
            request_hash,
            action: "succeeded".to_string(),
        };

        NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_task_submissions (task_id, lease_id, node_id, session_id, result_kind, request_hash, action)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
            [
                submission_req.task_id.into(),
                submission_req.lease_id.into(),
                submission_req.node_id.into(),
                submission_req.session_id.into(),
                submission_req.result_kind.as_str().into(),
                submission_req.request_hash.as_str().into(),
                submission_req.action.as_str().into(),
            ],
        ))
        .one(tx)
        .await?
        .ok_or_else(|| DbError::Other("Failed to insert submission".to_string()))?;

        let updated_node = Node::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM nodes WHERE id = $1",
            [node_id.into()],
        ))
        .one(tx)
        .await?
        .ok_or_else(|| DbError::not_found("Node", node_id.to_string()))?;

        Ok(NodeTaskCompleteResponse {
            action: NodeTaskCompleteAction::Succeeded,
            task_status: updated_task.status,
            node_status: updated_node.status,
            server_failure_count: updated_node.consecutive_failure_count as u32,
            failure_threshold: updated_node.failure_threshold as u32,
        })
    }

    /// 处理失败提交
    #[allow(clippy::too_many_arguments)]
    async fn handle_failed_submission(
        &self,
        tx: &DatabaseTransaction,
        task: &NodeTask,
        _node: &Node,
        node_id: Uuid,
        session_id: Uuid,
        lease_id: Uuid,
        code: String,
        message: String,
        is_client_error: bool,
    ) -> Result<NodeTaskCompleteResponse, DbError> {
        let error_json = serde_json::json!({
            "code": code,
            "message": message,
            "is_client_error": is_client_error,
        });

        let updated_task = if is_client_error {
            NodeTask::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE node_tasks
                SET status = 'failed',
                    failure_count = failure_count + 1,
                    error_json = $1,
                    updated_at = NOW()
                WHERE id = $2
                  AND assigned_node_id = $3
                  AND assigned_session_id = $4
                  AND lease_id = $5
                  AND status = 'leased'
                  AND deadline_at >= NOW()
                RETURNING *
                "#,
                [
                    error_json.clone().into(),
                    task.id.into(),
                    node_id.into(),
                    session_id.into(),
                    lease_id.into(),
                ],
            ))
            .one(tx)
            .await?
        } else {
            NodeTask::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE node_tasks
                SET status = CASE
                    WHEN failure_count + 1 < failure_threshold THEN 'queued'
                    ELSE 'failed'
                  END,
                  failure_count = failure_count + 1,
                  assigned_node_id = CASE
                    WHEN failure_count + 1 < failure_threshold THEN NULL
                    ELSE assigned_node_id
                  END,
                  assigned_session_id = CASE
                    WHEN failure_count + 1 < failure_threshold THEN NULL
                    ELSE assigned_session_id
                  END,
                  lease_id = CASE
                    WHEN failure_count + 1 < failure_threshold THEN NULL
                    ELSE lease_id
                  END,
                  claimed_at = CASE
                    WHEN failure_count + 1 < failure_threshold THEN NULL
                    ELSE claimed_at
                  END,
                  error_json = CASE
                    WHEN failure_count + 1 >= failure_threshold THEN $1
                    ELSE error_json
                  END,
                  updated_at = NOW()
                WHERE id = $2
                  AND assigned_node_id = $3
                  AND assigned_session_id = $4
                  AND lease_id = $5
                  AND status = 'leased'
                  AND deadline_at >= NOW()
                RETURNING *
                "#,
                [
                    error_json.clone().into(),
                    task.id.into(),
                    node_id.into(),
                    session_id.into(),
                    lease_id.into(),
                ],
            ))
            .one(tx)
            .await?
        };

        let updated_task = match updated_task {
            Some(t) => t,
            None => {
                let current_task = NodeTask::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT * FROM node_tasks WHERE id = $1",
                    [task.id.into()],
                ))
                .one(tx)
                .await?
                .ok_or_else(|| DbError::not_found("Task", task.id.to_string()))?;

                if current_task.status == TASK_STATUS_EXPIRED
                    || current_task.deadline_at < Utc::now()
                {
                    return Err(DbError::Other("task_expired_during_complete".to_string()));
                } else {
                    return Err(DbError::Other("concurrent_task_update_failed".to_string()));
                }
            }
        };

        // 增加节点连续失败计数并检查排除
        if !is_client_error {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                UPDATE nodes
                SET consecutive_failure_count = consecutive_failure_count + 1,
                    status = CASE
                        WHEN consecutive_failure_count + 1 >= failure_threshold THEN 'excluded'
                        ELSE status
                    END,
                    updated_at = NOW()
                WHERE id = $1
                "#,
                [node_id.into()],
            ))
            .await?;
        }

        let action = if updated_task.status == TASK_STATUS_QUEUED {
            "requeued"
        } else {
            "failed"
        };

        let result_for_hash = NodeTaskResult::Failed {
            code: code.clone(),
            message: message.clone(),
            is_client_error,
        };
        let request_hash = Self::compute_request_hash(task.id, lease_id, &result_for_hash)?;

        let submission_req = CreateNodeTaskSubmissionRequest {
            task_id: task.id,
            lease_id,
            node_id,
            session_id,
            result_kind: "failed".to_string(),
            request_hash,
            action: action.to_string(),
        };

        NodeTaskSubmission::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO node_task_submissions (task_id, lease_id, node_id, session_id, result_kind, request_hash, action)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
            [
                submission_req.task_id.into(),
                submission_req.lease_id.into(),
                submission_req.node_id.into(),
                submission_req.session_id.into(),
                submission_req.result_kind.as_str().into(),
                submission_req.request_hash.as_str().into(),
                submission_req.action.as_str().into(),
            ],
        ))
        .one(tx)
        .await?
        .ok_or_else(|| DbError::Other("Failed to insert submission".to_string()))?;

        let updated_node = Node::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM nodes WHERE id = $1",
            [node_id.into()],
        ))
        .one(tx)
        .await?
        .ok_or_else(|| DbError::not_found("Node", node_id.to_string()))?;

        let complete_action = if updated_task.status == TASK_STATUS_QUEUED {
            NodeTaskCompleteAction::Requeued
        } else {
            NodeTaskCompleteAction::Failed
        };

        Ok(NodeTaskCompleteResponse {
            action: complete_action,
            task_status: updated_task.status,
            node_status: updated_node.status,
            server_failure_count: updated_node.consecutive_failure_count as u32,
            failure_threshold: updated_node.failure_threshold as u32,
        })
    }
}

/// 解析 action 字符串
fn parse_action(action: &str) -> Result<NodeTaskCompleteAction, DbError> {
    match action {
        "succeeded" => Ok(NodeTaskCompleteAction::Succeeded),
        "requeued" => Ok(NodeTaskCompleteAction::Requeued),
        "failed" => Ok(NodeTaskCompleteAction::Failed),
        "expired" => Ok(NodeTaskCompleteAction::Expired),
        _ => Err(DbError::Other(format!("Unknown action: {}", action))),
    }
}

fn result_matches_payload(payload: &NodeTaskPayload, result: &NodeTaskResult) -> bool {
    match result {
        NodeTaskResult::Succeeded { .. } => payload.is_chat(),
        NodeTaskResult::ImageSucceeded { .. } => {
            payload.is_image_generation() || payload.is_image_edit()
        }
        NodeTaskResult::Failed { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTIVE_NODE_SESSION_FOR_UPDATE_SQL, result_matches_payload};
    use keycompute_types::{ChatCompletionRequest, node::*};
    use uuid::Uuid;

    #[test]
    fn heartbeat_locks_only_the_authenticated_active_session() {
        assert!(ACTIVE_NODE_SESSION_FOR_UPDATE_SQL.contains("node_id = $2"));
        assert!(ACTIVE_NODE_SESSION_FOR_UPDATE_SQL.contains("revoked_at IS NULL"));
        assert!(ACTIVE_NODE_SESSION_FOR_UPDATE_SQL.contains("expires_at > NOW()"));
        assert!(ACTIVE_NODE_SESSION_FOR_UPDATE_SQL.ends_with("FOR UPDATE"));
    }

    #[test]
    fn successful_result_type_must_match_the_task_payload() {
        let chat_payload = NodeTaskPayload {
            request_id: Uuid::new_v4(),
            chat: Some(ChatCompletionRequest::new("test-model", Vec::new())),
            image_generation: None,
            image_edit: None,
        };
        let image_payload = NodeTaskPayload {
            request_id: Uuid::new_v4(),
            chat: None,
            image_generation: Some(ImageGenerationRequest {
                prompt: "test".to_string(),
                n: None,
                size: None,
            }),
            image_edit: None,
        };
        let chat_result: NodeTaskResult = serde_json::from_value(serde_json::json!({
            "status": "succeeded",
            "response": {
                "id": "response-id",
                "object": "chat.completion",
                "created": 0,
                "model": "test-model",
                "choices": [],
                "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
            }
        }))
        .expect("chat result should deserialize");
        let image_result = NodeTaskResult::ImageSucceeded {
            image_response: ImageGenerationResponse {
                created: 0,
                data: Vec::new(),
            },
        };
        let failure = NodeTaskResult::Failed {
            code: "execution_failed".to_string(),
            message: "failed".to_string(),
            is_client_error: false,
        };

        assert!(result_matches_payload(&chat_payload, &chat_result));
        assert!(!result_matches_payload(&chat_payload, &image_result));
        assert!(result_matches_payload(&image_payload, &image_result));
        assert!(!result_matches_payload(&image_payload, &chat_result));
        assert!(result_matches_payload(&chat_payload, &failure));
        assert!(result_matches_payload(&image_payload, &failure));
    }
}
