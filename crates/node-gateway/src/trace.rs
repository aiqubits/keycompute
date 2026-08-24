use chrono::{DateTime, Utc};
use keycompute_types::{
    AttemptStatus, AttemptTraceFinish, BillingStatus, ErrorOrigin, NodeTaskCompleteAction,
    RequestExecutionFailure, RequestStatus, StreamEndReason, TraceErrorCategory, TraceErrorInfo,
};
use uuid::Uuid;

pub(crate) fn node_trace_error(code: &str) -> TraceErrorInfo {
    TraceErrorInfo {
        origin: ErrorOrigin::Node,
        category: if matches!(code, "node_expired" | "node_wait_timeout") {
            TraceErrorCategory::NodeExpired
        } else {
            TraceErrorCategory::NodeFailed
        },
        code: code.to_string(),
        summary: None,
        retryable: Some(!matches!(code, "node_expired" | "node_wait_timeout")),
    }
}

pub(crate) fn node_wait_timeout_finish(
    request_id: Uuid,
    attempt_id: Uuid,
    finished_at: DateTime<Utc>,
) -> AttemptTraceFinish {
    AttemptTraceFinish {
        attempt_id,
        request_id,
        attempt_status: AttemptStatus::Expired,
        request_status: RequestStatus::Running,
        is_final: true,
        stream_end_reason: Some(StreamEndReason::Timeout),
        stream_error_count: Some(1),
        error: Some(node_trace_error("node_wait_timeout")),
        billing_status: BillingStatus::Pending,
        finished_at,
    }
}

pub(crate) fn node_wait_timeout_failure() -> RequestExecutionFailure {
    RequestExecutionFailure {
        status: RequestStatus::TimedOut,
        error: node_trace_error("node_wait_timeout"),
        billing_status: BillingStatus::NotApplicable,
    }
}

pub(crate) fn invalid_node_result_failure() -> RequestExecutionFailure {
    RequestExecutionFailure {
        status: RequestStatus::Failed,
        error: TraceErrorInfo {
            origin: ErrorOrigin::Node,
            category: TraceErrorCategory::Protocol,
            code: "node_result_invalid".to_string(),
            summary: None,
            retryable: Some(false),
        },
        billing_status: BillingStatus::NotApplicable,
    }
}

pub(crate) fn node_completion_failure(
    action: &NodeTaskCompleteAction,
) -> Option<RequestExecutionFailure> {
    let (status, error) = match action {
        NodeTaskCompleteAction::Failed => (RequestStatus::Failed, node_trace_error("node_failed")),
        NodeTaskCompleteAction::Expired => {
            (RequestStatus::TimedOut, node_trace_error("node_expired"))
        }
        NodeTaskCompleteAction::Succeeded | NodeTaskCompleteAction::Requeued => return None,
    };
    Some(RequestExecutionFailure {
        status,
        error,
        billing_status: BillingStatus::NotApplicable,
    })
}

/// Map a Node task completion to its execution-attempt trace.
///
/// Every terminal task closes its Node attempt, but the request remains open
/// until the protocol handler completes the client-facing response.
pub(crate) fn node_completion_finish(
    action: &NodeTaskCompleteAction,
    attempt_id: Uuid,
    request_id: Uuid,
    finished_at: DateTime<Utc>,
) -> AttemptTraceFinish {
    let (attempt_status, request_status, is_final, billing_status, end_reason, error) = match action
    {
        NodeTaskCompleteAction::Succeeded => (
            AttemptStatus::Succeeded,
            RequestStatus::Running,
            true,
            BillingStatus::Pending,
            StreamEndReason::Completed,
            None,
        ),
        NodeTaskCompleteAction::Requeued => (
            AttemptStatus::Failed,
            RequestStatus::Queued,
            false,
            BillingStatus::Pending,
            StreamEndReason::UpstreamError,
            Some(node_trace_error("node_requeued")),
        ),
        NodeTaskCompleteAction::Failed => (
            AttemptStatus::Failed,
            RequestStatus::Running,
            true,
            BillingStatus::Pending,
            StreamEndReason::UpstreamError,
            Some(node_trace_error("node_failed")),
        ),
        NodeTaskCompleteAction::Expired => (
            AttemptStatus::Expired,
            RequestStatus::Running,
            true,
            BillingStatus::Pending,
            StreamEndReason::Timeout,
            Some(node_trace_error("node_expired")),
        ),
    };
    AttemptTraceFinish {
        attempt_id,
        request_id,
        attempt_status,
        request_status,
        is_final,
        stream_end_reason: Some(end_reason),
        stream_error_count: Some(if error.is_some() { 1 } else { 0 }),
        error,
        billing_status,
        finished_at,
    }
}
