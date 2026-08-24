//! Stable request lifecycle tracing contracts shared across runtime crates.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock, Mutex};
use uuid::Uuid;

use crate::request::ClientResponseOutcome;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $value),+ } }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

string_enum!(RouteType { ProviderAccount => "provider_account", Node => "node" });
string_enum!(RequestStatus {
    Received => "received", Routing => "routing", Queued => "queued", Running => "running",
    Succeeded => "succeeded", Failed => "failed", TimedOut => "timed_out", Cancelled => "cancelled"
});
string_enum!(BillingStatus { Pending => "pending", Succeeded => "succeeded", Failed => "failed", NotApplicable => "not_applicable" });
string_enum!(TraceQuality { Actual => "actual", Derived => "derived", Partial => "partial" });
string_enum!(AttemptKind { Primary => "primary", Fallback => "fallback", Retry => "retry", Reclaim => "reclaim" });
string_enum!(AttemptStatus { Running => "running", Succeeded => "succeeded", Failed => "failed", TimedOut => "timed_out", Cancelled => "cancelled", Expired => "expired" });
string_enum!(ErrorOrigin { Client => "client", Gateway => "gateway", Upstream => "upstream", Node => "node" });
string_enum!(TraceErrorCategory {
    Authorization => "authorization", InvalidRequest => "invalid_request", Balance => "balance",
    RateLimit => "rate_limit", Transport => "transport", Timeout => "timeout",
    Upstream4xx => "upstream_4xx", Upstream5xx => "upstream_5xx", Protocol => "protocol",
    ClientDisconnect => "client_disconnect", NodeExpired => "node_expired", NodeFailed => "node_failed",
    Internal => "internal"
});
string_enum!(StreamEndReason { Completed => "completed", UpstreamError => "upstream_error", ProtocolError => "protocol_error", ClientDisconnect => "client_disconnect", Cancelled => "cancelled", Timeout => "timeout", Truncated => "truncated" });

impl RequestStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::TimedOut | Self::Cancelled
        )
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        if self as u8 == next as u8 {
            return true;
        }
        match self {
            Self::Received => matches!(
                next,
                Self::Routing
                    | Self::Queued
                    | Self::Running
                    | Self::Succeeded
                    | Self::Failed
                    | Self::TimedOut
                    | Self::Cancelled
            ),
            Self::Routing => matches!(
                next,
                Self::Queued
                    | Self::Running
                    | Self::Succeeded
                    | Self::Failed
                    | Self::TimedOut
                    | Self::Cancelled
            ),
            Self::Queued | Self::Running => next.is_terminal(),
            Self::Succeeded | Self::Failed | Self::TimedOut | Self::Cancelled => false,
        }
    }
}

impl AttemptStatus {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(self, Self::Running) || self as u8 == next as u8
    }
}

#[derive(Debug, Clone)]
pub struct RequestTraceStart {
    pub request_id: Uuid,
    pub client_request_id: Option<String>,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub produce_ai_key_id: Uuid,
    pub protocol: String,
    pub request_path: String,
    pub requested_model: String,
    pub is_stream: bool,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AttemptTraceStart {
    pub request_id: Uuid,
    pub attempt_kind: AttemptKind,
    pub route_type: RouteType,
    pub model: String,
    pub provider_name: Option<String>,
    pub account_id: Option<Uuid>,
    pub node_task_id: Option<Uuid>,
    pub node_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub lease_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptRef {
    pub id: Uuid,
    pub attempt_no: i32,
}

#[derive(Debug, Clone, Default)]
pub struct AttemptResponseMeta {
    pub http_status: Option<i32>,
    pub headers_received_at: Option<DateTime<Utc>>,
    pub upstream_request_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TraceErrorInfo {
    pub origin: ErrorOrigin,
    pub category: TraceErrorCategory,
    pub code: String,
    pub summary: Option<String>,
    pub retryable: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct AttemptTraceFinish {
    pub attempt_id: Uuid,
    pub request_id: Uuid,
    pub attempt_status: AttemptStatus,
    pub request_status: RequestStatus,
    /// Whether this is the last execution attempt selected for the request.
    /// A final provider attempt may finish while `request_status` remains
    /// running until the protocol handler completes the client response.
    pub is_final: bool,
    pub stream_end_reason: Option<StreamEndReason>,
    pub stream_error_count: Option<i32>,
    pub error: Option<TraceErrorInfo>,
    pub billing_status: BillingStatus,
    pub finished_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RequestTraceFinish {
    pub request_id: Uuid,
    pub status: RequestStatus,
    pub error: Option<TraceErrorInfo>,
    pub billing_status: BillingStatus,
    pub finished_at: DateTime<Utc>,
}

/// Structured terminal execution failure handed from an executor to the
/// client-facing response handler.
///
/// The executor records the attempt immediately, but the handler owns the
/// request terminal state because a queued error can still be superseded by a
/// client disconnect before it is delivered.
#[derive(Debug, Clone)]
pub struct RequestExecutionFailure {
    pub status: RequestStatus,
    pub error: TraceErrorInfo,
    pub billing_status: BillingStatus,
}

impl RequestExecutionFailure {
    pub fn into_request_finish(self, request_id: Uuid) -> RequestTraceFinish {
        RequestTraceFinish {
            request_id,
            status: self.status,
            error: Some(self.error),
            billing_status: self.billing_status,
            finished_at: Utc::now(),
        }
    }
}

/// Convert the handler-visible response outcome into the terminal request trace.
///
/// Upstream or Node completion only closes the execution attempt. The request
/// itself is successful once the handler has handed the complete response to
/// the client-facing response path.
pub fn client_response_trace_finish(
    request_id: Uuid,
    outcome: ClientResponseOutcome,
) -> RequestTraceFinish {
    client_response_trace_finish_with_failure(request_id, outcome, None)
}

/// Convert the handler-visible response outcome into the terminal request
/// trace while preserving an executor-supplied failure when the error reached
/// the response path.
///
/// A disconnect always wins over the pending execution failure. This keeps
/// request state aligned with the first client-visible outcome while retaining
/// upstream/Node attribution for delivered errors.
pub fn client_response_trace_finish_with_failure(
    request_id: Uuid,
    outcome: ClientResponseOutcome,
    execution_failure: Option<RequestExecutionFailure>,
) -> RequestTraceFinish {
    if matches!(
        outcome,
        ClientResponseOutcome::ResponseFailed | ClientResponseOutcome::TimedOut
    ) && let Some(failure) = execution_failure
    {
        return failure.into_request_finish(request_id);
    }
    let (status, error) = match outcome {
        ClientResponseOutcome::Succeeded => (RequestStatus::Succeeded, None),
        ClientResponseOutcome::ClientDisconnected => (
            RequestStatus::Cancelled,
            Some(TraceErrorInfo {
                origin: ErrorOrigin::Gateway,
                category: TraceErrorCategory::ClientDisconnect,
                code: "client_disconnected".to_string(),
                summary: None,
                retryable: Some(false),
            }),
        ),
        ClientResponseOutcome::ResponseFailed => (
            RequestStatus::Failed,
            Some(TraceErrorInfo {
                origin: ErrorOrigin::Gateway,
                category: TraceErrorCategory::Internal,
                code: "client_response_failed".to_string(),
                summary: None,
                retryable: Some(false),
            }),
        ),
        ClientResponseOutcome::TimedOut => (
            RequestStatus::TimedOut,
            Some(TraceErrorInfo {
                origin: ErrorOrigin::Gateway,
                category: TraceErrorCategory::Timeout,
                code: "client_response_timeout".to_string(),
                summary: None,
                retryable: Some(false),
            }),
        ),
    };
    RequestTraceFinish {
        request_id,
        status,
        error,
        billing_status: BillingStatus::Pending,
        finished_at: Utc::now(),
    }
}

#[derive(Debug, thiserror::Error)]
#[error("request lifecycle trace write failed: {0}")]
pub struct TraceWriteError(pub String);

#[async_trait]
pub trait RequestLifecycleRecorder: Send + Sync {
    async fn start_request(&self, request: RequestTraceStart) -> Result<(), TraceWriteError>;
    async fn set_route(
        &self,
        request_id: Uuid,
        route: RouteType,
        status: RequestStatus,
    ) -> Result<(), TraceWriteError>;
    async fn start_attempt(
        &self,
        attempt: AttemptTraceStart,
    ) -> Result<AttemptRef, TraceWriteError>;
    async fn mark_trace_partial(&self, request_id: Uuid) -> Result<(), TraceWriteError>;
    async fn record_attempt_response_meta(
        &self,
        request_id: Uuid,
        attempt_id: Uuid,
        meta: AttemptResponseMeta,
    ) -> Result<(), TraceWriteError>;
    async fn record_attempt_first_content(
        &self,
        request_id: Uuid,
        attempt_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), TraceWriteError>;
    async fn record_client_first_content(
        &self,
        request_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), TraceWriteError>;
    /// Wait until all intermediate updates queued before this call have been
    /// processed. Implementations without an asynchronous intermediate queue
    /// may keep the default no-op behavior.
    async fn flush_intermediate_updates(&self, _: Uuid) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn finish_attempt_and_request(
        &self,
        finish: AttemptTraceFinish,
    ) -> Result<(), TraceWriteError>;
    async fn finish_request_without_attempt(
        &self,
        finish: RequestTraceFinish,
    ) -> Result<(), TraceWriteError>;
    async fn mark_billing_succeeded(&self, request_id: Uuid) -> Result<(), TraceWriteError>;
    async fn mark_billing_failed(&self, request_id: Uuid) -> Result<(), TraceWriteError>;
}

#[derive(Debug, Default)]
pub struct NoopRequestLifecycleRecorder;

#[async_trait]
impl RequestLifecycleRecorder for NoopRequestLifecycleRecorder {
    async fn start_request(&self, _: RequestTraceStart) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn set_route(
        &self,
        _: Uuid,
        _: RouteType,
        _: RequestStatus,
    ) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn start_attempt(&self, _: AttemptTraceStart) -> Result<AttemptRef, TraceWriteError> {
        Ok(AttemptRef {
            id: Uuid::new_v4(),
            attempt_no: 1,
        })
    }
    async fn mark_trace_partial(&self, _: Uuid) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn record_attempt_response_meta(
        &self,
        _: Uuid,
        _: Uuid,
        _: AttemptResponseMeta,
    ) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn record_attempt_first_content(
        &self,
        _: Uuid,
        _: Uuid,
        _: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn record_client_first_content(
        &self,
        _: Uuid,
        _: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn finish_attempt_and_request(
        &self,
        _: AttemptTraceFinish,
    ) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn finish_request_without_attempt(
        &self,
        _: RequestTraceFinish,
    ) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn mark_billing_succeeded(&self, _: Uuid) -> Result<(), TraceWriteError> {
        Ok(())
    }
    async fn mark_billing_failed(&self, _: Uuid) -> Result<(), TraceWriteError> {
        Ok(())
    }
}

/// Deterministic in-memory implementation for tests.
#[derive(Debug, Clone, Default)]
pub struct TestRequestLifecycleRecorder {
    events: Arc<Mutex<Vec<String>>>,
    request_starts: Arc<Mutex<Vec<RequestTraceStart>>>,
    attempt_finishes: Arc<Mutex<Vec<AttemptTraceFinish>>>,
    request_finishes: Arc<Mutex<Vec<RequestTraceFinish>>>,
}
impl TestRequestLifecycleRecorder {
    pub fn events(&self) -> Vec<String> {
        self.events.lock().expect("trace events poisoned").clone()
    }
    pub fn attempt_finishes(&self) -> Vec<AttemptTraceFinish> {
        self.attempt_finishes
            .lock()
            .expect("trace attempt finishes poisoned")
            .clone()
    }
    pub fn request_starts(&self) -> Vec<RequestTraceStart> {
        self.request_starts
            .lock()
            .expect("trace request starts poisoned")
            .clone()
    }
    pub fn request_finishes(&self) -> Vec<RequestTraceFinish> {
        self.request_finishes
            .lock()
            .expect("trace request finishes poisoned")
            .clone()
    }
    fn push(&self, event: impl Into<String>) {
        self.events
            .lock()
            .expect("trace events poisoned")
            .push(event.into());
    }
}

#[async_trait]
impl RequestLifecycleRecorder for TestRequestLifecycleRecorder {
    async fn start_request(&self, v: RequestTraceStart) -> Result<(), TraceWriteError> {
        self.request_starts
            .lock()
            .expect("trace request starts poisoned")
            .push(v.clone());
        self.push(format!("start_request:{}", v.request_id));
        Ok(())
    }
    async fn set_route(
        &self,
        id: Uuid,
        route: RouteType,
        _: RequestStatus,
    ) -> Result<(), TraceWriteError> {
        self.push(format!("set_route:{id}:{route}"));
        Ok(())
    }
    async fn start_attempt(&self, v: AttemptTraceStart) -> Result<AttemptRef, TraceWriteError> {
        let no = self
            .events()
            .iter()
            .filter(|e| e.starts_with("start_attempt:"))
            .count() as i32
            + 1;
        let id = Uuid::new_v4();
        self.push(format!("start_attempt:{}:{no}", v.request_id));
        Ok(AttemptRef { id, attempt_no: no })
    }
    async fn mark_trace_partial(&self, id: Uuid) -> Result<(), TraceWriteError> {
        self.push(format!("trace_partial:{id}"));
        Ok(())
    }
    async fn record_attempt_response_meta(
        &self,
        _: Uuid,
        id: Uuid,
        _: AttemptResponseMeta,
    ) -> Result<(), TraceWriteError> {
        self.push(format!("attempt_meta:{id}"));
        Ok(())
    }
    async fn record_attempt_first_content(
        &self,
        _: Uuid,
        id: Uuid,
        _: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        self.push(format!("attempt_first_content:{id}"));
        Ok(())
    }
    async fn record_client_first_content(
        &self,
        id: Uuid,
        _: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        self.push(format!("client_first_content:{id}"));
        Ok(())
    }
    async fn flush_intermediate_updates(&self, id: Uuid) -> Result<(), TraceWriteError> {
        self.push(format!("flush_intermediate:{id}"));
        Ok(())
    }
    async fn finish_attempt_and_request(
        &self,
        v: AttemptTraceFinish,
    ) -> Result<(), TraceWriteError> {
        self.push(format!("finish_attempt:{}", v.attempt_id));
        self.attempt_finishes
            .lock()
            .expect("trace attempt finishes poisoned")
            .push(v);
        Ok(())
    }
    async fn finish_request_without_attempt(
        &self,
        v: RequestTraceFinish,
    ) -> Result<(), TraceWriteError> {
        self.push(format!("finish_request:{}", v.request_id));
        self.request_finishes
            .lock()
            .expect("trace request finishes poisoned")
            .push(v);
        Ok(())
    }
    async fn mark_billing_succeeded(&self, id: Uuid) -> Result<(), TraceWriteError> {
        self.push(format!("billing_succeeded:{id}"));
        Ok(())
    }
    async fn mark_billing_failed(&self, id: Uuid) -> Result<(), TraceWriteError> {
        self.push(format!("billing_failed:{id}"));
        Ok(())
    }
}

/// Remove common credentials and URL queries, then truncate on a UTF-8 character boundary.
pub fn sanitize_error_summary(value: &str) -> String {
    static BEARER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)\bbearer\s+[^\s,"'}]+"#).expect("valid bearer credential regex")
    });
    static CREDENTIAL_FIELD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)(authorization|proxy[-_]?authorization|x-api-key|api[-_]?key|apikey|access[-_]?token|refresh[-_]?token|client[-_]?secret|secret[-_]?access[-_]?key|private[-_]?key|password|token)(\s*[\"']?\s*[:=]\s*[\"']?)[^\s,\"'}]+"#,
        )
        .expect("valid credential field regex")
    });
    static PROVIDER_KEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bsk(?:-[a-z]+)?[-_][a-z0-9_-]{6,}\b").expect("valid provider key regex")
    });
    static URL_QUERY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(https?://[^\s?]+)\?[^\s]+").expect("valid URL query regex")
    });
    static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(https?://)[^/\s?#]+@").expect("valid URL userinfo credential regex")
    });
    static GOOGLE_API_KEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\bAIza[0-9A-Za-z_-]{20,}\b").expect("valid Google API key regex")
    });

    let redacted = BEARER.replace_all(value, "Bearer [REDACTED]");
    let redacted = CREDENTIAL_FIELD.replace_all(&redacted, "$1$2[REDACTED]");
    let redacted = PROVIDER_KEY.replace_all(&redacted, "[REDACTED]");
    let redacted = GOOGLE_API_KEY.replace_all(&redacted, "[REDACTED]");
    let redacted = URL_USERINFO.replace_all(&redacted, "$1[REDACTED]@");
    URL_QUERY
        .replace_all(&redacted, "$1")
        .chars()
        .take(512)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitizes_secrets_and_truncates_on_char_boundaries() {
        let input = format!(
            "Authorization: Bearer bearer-secret api_key=secret \
             {{\"x-api-key\":\"json-secret\",\"client_secret\":\"oauth-secret\"}} \
             access_token=query-secret password='password-secret' \
             sk-ant-providersecret AIza1234567890abcdefghijklmnop \
             https://url-user:url-password@example.test/a?token=x {}",
            "界".repeat(600)
        );
        let output = sanitize_error_summary(&input);
        assert!(!output.contains("bearer-secret"));
        assert!(!output.contains("api_key=secret"));
        assert!(!output.contains("json-secret"));
        assert!(!output.contains("providersecret"));
        assert!(!output.contains("oauth-secret"));
        assert!(!output.contains("query-secret"));
        assert!(!output.contains("password-secret"));
        assert!(!output.contains("url-user"));
        assert!(!output.contains("url-password"));
        assert!(output.contains("https://[REDACTED]@example.test/a"));
        assert!(!output.contains("AIza1234567890abcdefghijklmnop"));
        assert!(!output.contains("token=x"));
        assert!(output.chars().count() <= 512);
    }

    #[test]
    fn terminal_states_cannot_be_reopened() {
        assert!(RequestStatus::Received.can_transition_to(RequestStatus::Routing));
        assert!(RequestStatus::Routing.can_transition_to(RequestStatus::Running));
        assert!(RequestStatus::Running.can_transition_to(RequestStatus::Succeeded));
        assert!(!RequestStatus::Succeeded.can_transition_to(RequestStatus::Running));
        assert!(AttemptStatus::Running.can_transition_to(AttemptStatus::Expired));
        assert!(!AttemptStatus::Failed.can_transition_to(AttemptStatus::Running));
    }

    #[test]
    fn client_disconnect_is_a_cancelled_request_trace() {
        let request_id = Uuid::new_v4();
        let finish =
            client_response_trace_finish(request_id, ClientResponseOutcome::ClientDisconnected);

        assert_eq!(finish.request_id, request_id);
        assert_eq!(finish.status, RequestStatus::Cancelled);
        assert_eq!(finish.billing_status, BillingStatus::Pending);
        let error = finish.error.expect("disconnect error");
        assert_eq!(error.origin, ErrorOrigin::Gateway);
        assert_eq!(error.category, TraceErrorCategory::ClientDisconnect);
        assert_eq!(error.code, "client_disconnected");
    }

    #[test]
    fn delivered_execution_error_preserves_its_failure_attribution() {
        let request_id = Uuid::new_v4();
        let finish = client_response_trace_finish_with_failure(
            request_id,
            ClientResponseOutcome::ResponseFailed,
            Some(RequestExecutionFailure {
                status: RequestStatus::Failed,
                error: TraceErrorInfo {
                    origin: ErrorOrigin::Upstream,
                    category: TraceErrorCategory::Upstream5xx,
                    code: "upstream_unavailable".to_string(),
                    summary: None,
                    retryable: Some(true),
                },
                billing_status: BillingStatus::NotApplicable,
            }),
        );

        assert_eq!(finish.status, RequestStatus::Failed);
        assert_eq!(finish.billing_status, BillingStatus::NotApplicable);
        let error = finish.error.expect("execution error");
        assert_eq!(error.origin, ErrorOrigin::Upstream);
        assert_eq!(error.code, "upstream_unavailable");
    }

    #[test]
    fn client_disconnect_supersedes_a_pending_execution_failure() {
        let request_id = Uuid::new_v4();
        let finish = client_response_trace_finish_with_failure(
            request_id,
            ClientResponseOutcome::ClientDisconnected,
            Some(RequestExecutionFailure {
                status: RequestStatus::TimedOut,
                error: TraceErrorInfo {
                    origin: ErrorOrigin::Node,
                    category: TraceErrorCategory::NodeExpired,
                    code: "node_expired".to_string(),
                    summary: None,
                    retryable: Some(false),
                },
                billing_status: BillingStatus::NotApplicable,
            }),
        );

        assert_eq!(finish.status, RequestStatus::Cancelled);
        assert_eq!(
            finish.error.expect("disconnect error").code,
            "client_disconnected"
        );
    }
}
