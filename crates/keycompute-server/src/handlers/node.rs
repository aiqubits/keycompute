//! 节点网关 HTTP Handler
//!
//! 处理 node-token 客户端的注册、心跳、任务领取和结果提交请求

use crate::{
    error::{ApiError, Result},
    extractors::NodeSessionAuth,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use keycompute_types::node::{
    NodeHeartbeatRequest, NodeHeartbeatResponse, NodePollRequest, NodePollResponse,
    NodeRegisterRequest, NodeRegisterResponse, NodeTaskCompleteRequest, NodeTaskCompleteResponse,
};
use node_gateway::NodeGatewayService;
use std::sync::Arc;
use uuid::Uuid;

/// 节点注册 Handler
/// POST /node/v1/register
///
/// 不需要 session token 认证，使用 HMAC 签名的 registration_token 验证。
/// token 由用户申请 → Admin 审批后下发 → 注册时一次性消费。
pub async fn node_register(
    State(state): State<AppState>,
    Json(request): Json<NodeRegisterRequest>,
) -> Result<Json<NodeRegisterResponse>> {
    let node_gateway = get_node_gateway(&state)?;

    let response = node_gateway
        .register_node(&request)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(response))
}

/// 节点心跳 Handler
/// POST /node/v1/heartbeat
///
/// 需要 session token 认证
pub async fn node_heartbeat(
    State(state): State<AppState>,
    auth: NodeSessionAuth,
    Json(body): Json<NodeHeartbeatRequest>,
) -> Result<Json<NodeHeartbeatResponse>> {
    // 验证请求体中的 node_id 和 session_id 与认证结果一致
    if body.node_id != auth.node_id || body.session_id != auth.session_id {
        return Err(ApiError::NodeIdentityMismatch {
            expected_node_id: auth.node_id,
            expected_session_id: auth.session_id,
            actual_node_id: body.node_id,
            actual_session_id: body.session_id,
        });
    }

    let node_gateway = get_node_gateway(&state)?;

    // 从 body 中提取 accepted_models
    let accepted_models = body.accepted_models.clone();

    let response = node_gateway
        .heartbeat(auth.node_id, auth.session_id, accepted_models)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(response))
}

/// 节点任务轮询 Handler
/// POST /node/v1/tasks/poll
///
/// 需要 session token 认证,长轮询领取任务
pub async fn node_poll(
    State(state): State<AppState>,
    auth: NodeSessionAuth,
    Json(body): Json<NodePollRequest>,
) -> Result<Json<NodePollResponse>> {
    // 验证请求体中的 node_id 和 session_id 与认证结果一致
    if body.node_id != auth.node_id || body.session_id != auth.session_id {
        return Err(ApiError::NodeIdentityMismatch {
            expected_node_id: auth.node_id,
            expected_session_id: auth.session_id,
            actual_node_id: body.node_id,
            actual_session_id: body.session_id,
        });
    }

    let node_gateway = get_node_gateway(&state)?;

    // 从数据库读取 session 的 accepted_models
    let pool = state
        .pool
        .as_deref()
        .ok_or_else(|| ApiError::Internal("Database pool not configured".to_string()))?;

    // Heartbeat persists accepted_models on the writer. Polling must observe
    // that fresh authorization state instead of a potentially lagging replica.
    let session = keycompute_db::models::node_session::NodeSession::find_by_id(
        pool.write_conn(),
        auth.session_id,
    )
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to query session: {}", e)))?
    .ok_or_else(|| ApiError::NotFound(format!("Session {} not found", auth.session_id)))?;

    let accepted_models: Vec<String> =
        serde_json::from_value(session.accepted_models_json).unwrap_or_default();

    let response = node_gateway
        .poll_task(auth.node_id, auth.session_id, accepted_models)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(response))
}

/// 节点任务完成 Handler
/// POST /node/v1/tasks/{task_id}/complete
///
/// 需要 session token 认证，支持幂等重试
pub async fn node_complete(
    State(state): State<AppState>,
    auth: NodeSessionAuth,
    Path(task_id): Path<Uuid>,
    Json(body): Json<NodeTaskCompleteRequest>,
) -> Result<Json<NodeTaskCompleteResponse>> {
    // 验证请求体中的 node_id 和 session_id 与认证结果一致
    if body.node_id != auth.node_id || body.session_id != auth.session_id {
        return Err(ApiError::NodeIdentityMismatch {
            expected_node_id: auth.node_id,
            expected_session_id: auth.session_id,
            actual_node_id: body.node_id,
            actual_session_id: body.session_id,
        });
    }

    // 验证路径中的 task_id 与请求体中的 task_id 一致
    if body.task_id != task_id {
        return Err(ApiError::BadRequest(
            "task_id in path does not match task_id in body".to_string(),
        ));
    }

    let node_gateway = get_node_gateway(&state)?;

    let response = node_gateway
        .complete_task(
            body.task_id,
            body.lease_id,
            auth.node_id,
            auth.session_id,
            body.result,
        )
        .await
        .map_err(ApiError::from)?;

    Ok(Json(response))
}

/// 获取 NodeGatewayService 引用
fn get_node_gateway(state: &AppState) -> Result<Arc<NodeGatewayService>> {
    state
        .node_gateway
        .clone()
        .ok_or_else(|| ApiError::Internal("Node gateway not configured".to_string()))
}
