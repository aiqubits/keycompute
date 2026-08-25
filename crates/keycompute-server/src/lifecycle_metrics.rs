//! Low-cardinality Prometheus instrumentation for request lifecycle tracing.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use keycompute_observability::metrics::{
    BILLING_WRITE_FAILURE_TOTAL, FALLBACK_TOTAL, MONITORING_ACTIVE_REQUESTS,
    MONITORING_ATTEMPT_LATENCY, MONITORING_ATTEMPT_TOTAL, MONITORING_NODE_TASK_TOTAL,
    MONITORING_REQUEST_LATENCY, MONITORING_REQUEST_TOTAL, TRACE_WRITE_FAILURE_TOTAL,
};
use keycompute_types::{
    AttemptKind, AttemptRef, AttemptResponseMeta, AttemptTraceFinish, AttemptTraceStart,
    RequestLifecycleRecorder, RequestStatus, RequestTraceFinish, RequestTraceStart, RouteType,
    TraceWriteError,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone)]
struct RequestMetricState {
    protocol: String,
    route: &'static str,
    status: RequestStatus,
    received_at: DateTime<Utc>,
}

#[derive(Clone, Copy)]
struct AttemptMetricState {
    route: RouteType,
    started_at: DateTime<Utc>,
    trace_disabled: bool,
}

fn is_new_node_task_transition(
    previous_route: &'static str,
    previous_status: RequestStatus,
    route: RouteType,
    status: RequestStatus,
) -> bool {
    route == RouteType::Node
        && matches!(status, RequestStatus::Queued | RequestStatus::Running)
        && (previous_route != route.as_str() || previous_status != status)
}

fn uses_local_attempt_metrics(route: RouteType) -> bool {
    // Node claim and completion are independent HTTP requests and may be served
    // by different replicas. node-gateway records those metrics from the
    // committed task transition instead of correlating them in process memory.
    route != RouteType::Node
}

pub(crate) struct MetricsRequestLifecycleRecorder {
    inner: Arc<dyn RequestLifecycleRecorder>,
    requests: Mutex<HashMap<Uuid, RequestMetricState>>,
    attempts: Mutex<HashMap<Uuid, AttemptMetricState>>,
    disabled_requests: Mutex<HashSet<Uuid>>,
}

impl MetricsRequestLifecycleRecorder {
    pub(crate) fn new(inner: Arc<dyn RequestLifecycleRecorder>) -> Self {
        Self {
            inner,
            requests: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            disabled_requests: Mutex::new(HashSet::new()),
        }
    }

    fn write_failed(phase: &'static str, result: &Result<(), TraceWriteError>) {
        if let Err(error) = result {
            if error.0.contains("intermediate queue full") {
                return;
            }
            TRACE_WRITE_FAILURE_TOTAL.with_label_values(&[phase]).inc();
        }
    }

    fn finish_request_metrics(&self, request_id: Uuid, status: RequestStatus, at: DateTime<Utc>) {
        let state = self
            .requests
            .lock()
            .expect("request metrics state poisoned")
            .remove(&request_id);
        let Some(state) = state else { return };
        MONITORING_ACTIVE_REQUESTS
            .with_label_values(&[state.protocol.as_str(), state.route])
            .dec();
        MONITORING_REQUEST_TOTAL
            .with_label_values(&[state.protocol.as_str(), state.route, status.as_str()])
            .inc();
        MONITORING_REQUEST_LATENCY
            .with_label_values(&[state.protocol.as_str(), state.route, status.as_str()])
            .observe(
                at.signed_duration_since(state.received_at)
                    .num_milliseconds()
                    .max(0) as f64
                    / 1000.0,
            );
    }
}

#[async_trait]
impl RequestLifecycleRecorder for MetricsRequestLifecycleRecorder {
    async fn start_request(&self, request: RequestTraceStart) -> Result<(), TraceWriteError> {
        let request_id = request.request_id;
        let state = RequestMetricState {
            protocol: request.protocol.clone(),
            route: "unassigned",
            status: RequestStatus::Received,
            received_at: request.received_at,
        };
        let result = self.inner.start_request(request).await;
        Self::write_failed("start", &result);
        if result.is_err() {
            self.disabled_requests
                .lock()
                .expect("disabled request state poisoned")
                .insert(request_id);
        }
        self.requests
            .lock()
            .expect("request metrics state poisoned")
            .insert(request_id, state.clone());
        MONITORING_ACTIVE_REQUESTS
            .with_label_values(&[state.protocol.as_str(), state.route])
            .inc();
        if let Err(error) = result {
            tracing::warn!(%request_id, %error, "request tracing disabled after start failure");
        }
        // This decorator is the per-request fault isolator: later calls remain
        // available for metrics but skip the database through disabled_requests.
        Ok(())
    }

    async fn set_route(
        &self,
        request_id: Uuid,
        route: RouteType,
        status: RequestStatus,
    ) -> Result<(), TraceWriteError> {
        let disabled = self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id);
        let result = if disabled {
            Ok(())
        } else {
            self.inner.set_route(request_id, route, status).await
        };
        Self::write_failed("intermediate", &result);
        let mut record_node_transition = false;
        if let Some(state) = self
            .requests
            .lock()
            .expect("request metrics state poisoned")
            .get_mut(&request_id)
        {
            let route_changed = state.route != route.as_str();
            record_node_transition =
                is_new_node_task_transition(state.route, state.status, route, status);
            if route_changed {
                MONITORING_ACTIVE_REQUESTS
                    .with_label_values(&[state.protocol.as_str(), state.route])
                    .dec();
                state.route = route.as_str();
                MONITORING_ACTIVE_REQUESTS
                    .with_label_values(&[state.protocol.as_str(), state.route])
                    .inc();
            }
            state.status = status;
        }
        if record_node_transition {
            MONITORING_NODE_TASK_TOTAL
                .with_label_values(&[status.as_str()])
                .inc();
        }
        result
    }

    async fn start_attempt(
        &self,
        attempt: AttemptTraceStart,
    ) -> Result<AttemptRef, TraceWriteError> {
        let route = attempt.route_type;
        let started_at = attempt.started_at;
        let kind = attempt.attempt_kind;
        let disabled = self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&attempt.request_id);
        let result = if disabled {
            Ok(AttemptRef {
                id: Uuid::new_v4(),
                attempt_no: 1,
            })
        } else {
            self.inner.start_attempt(attempt).await
        };
        if let Ok(reference) = result.as_ref() {
            if uses_local_attempt_metrics(route) {
                self.attempts
                    .lock()
                    .expect("attempt metrics state poisoned")
                    .insert(
                        reference.id,
                        AttemptMetricState {
                            route,
                            started_at,
                            trace_disabled: disabled,
                        },
                    );
            }
            if kind == AttemptKind::Fallback {
                FALLBACK_TOTAL.inc();
            }
        } else {
            TRACE_WRITE_FAILURE_TOTAL
                .with_label_values(&["start"])
                .inc();
        }
        result
    }

    async fn mark_trace_partial(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        let disabled = self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id);
        let result = if disabled {
            Ok(())
        } else {
            self.inner.mark_trace_partial(request_id).await
        };
        Self::write_failed("intermediate", &result);
        result
    }

    async fn record_attempt_response_meta(
        &self,
        request_id: Uuid,
        attempt_id: Uuid,
        meta: AttemptResponseMeta,
    ) -> Result<(), TraceWriteError> {
        let disabled = self
            .attempts
            .lock()
            .expect("attempt metrics state poisoned")
            .get(&attempt_id)
            .is_some_and(|state| state.trace_disabled);
        let result = if disabled {
            Ok(())
        } else {
            self.inner
                .record_attempt_response_meta(request_id, attempt_id, meta)
                .await
        };
        Self::write_failed("intermediate", &result);
        result
    }

    async fn record_attempt_first_content(
        &self,
        request_id: Uuid,
        attempt_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        let disabled = self
            .attempts
            .lock()
            .expect("attempt metrics state poisoned")
            .get(&attempt_id)
            .is_some_and(|state| state.trace_disabled);
        let result = if disabled {
            Ok(())
        } else {
            self.inner
                .record_attempt_first_content(request_id, attempt_id, at)
                .await
        };
        Self::write_failed("intermediate", &result);
        result
    }

    async fn record_client_first_content(
        &self,
        request_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        let disabled = self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id);
        let result = if disabled {
            Ok(())
        } else {
            self.inner.record_client_first_content(request_id, at).await
        };
        Self::write_failed("intermediate", &result);
        result
    }

    async fn flush_intermediate_updates(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        let disabled = self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id);
        let result = if disabled {
            Ok(())
        } else {
            self.inner.flush_intermediate_updates(request_id).await
        };
        Self::write_failed("intermediate", &result);
        result
    }

    async fn finish_attempt_and_request(
        &self,
        finish: AttemptTraceFinish,
    ) -> Result<(), TraceWriteError> {
        let attempt_id = finish.attempt_id;
        let request_id = finish.request_id;
        let attempt_status = finish.attempt_status;
        let request_status = finish.request_status;
        let finished_at = finish.finished_at;
        let origin = finish
            .error
            .as_ref()
            .map(|e| e.origin.as_str())
            .unwrap_or("none");
        let category = finish
            .error
            .as_ref()
            .map(|e| e.category.as_str())
            .unwrap_or("none");
        let disabled = self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id);
        let result = if disabled {
            Ok(())
        } else {
            self.inner.finish_attempt_and_request(finish).await
        };
        Self::write_failed("final", &result);

        if let Some(attempt) = self
            .attempts
            .lock()
            .expect("attempt metrics state poisoned")
            .remove(&attempt_id)
        {
            MONITORING_ATTEMPT_TOTAL
                .with_label_values(&[
                    attempt.route.as_str(),
                    attempt_status.as_str(),
                    origin,
                    category,
                ])
                .inc();
            MONITORING_ATTEMPT_LATENCY
                .with_label_values(&[attempt.route.as_str(), attempt_status.as_str()])
                .observe(
                    finished_at
                        .signed_duration_since(attempt.started_at)
                        .num_milliseconds()
                        .max(0) as f64
                        / 1000.0,
                );
        }
        // A final provider attempt can precede client-visible completion. Only
        // a terminal request status closes the request metric lifecycle.
        if request_status.is_terminal() {
            self.finish_request_metrics(request_id, request_status, finished_at);
            self.disabled_requests
                .lock()
                .expect("disabled request state poisoned")
                .remove(&request_id);
        }
        result
    }

    async fn finish_request_without_attempt(
        &self,
        finish: RequestTraceFinish,
    ) -> Result<(), TraceWriteError> {
        let request_id = finish.request_id;
        let status = finish.status;
        let finished_at = finish.finished_at;
        let disabled = self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id);
        let result = if disabled {
            Ok(())
        } else {
            self.inner.finish_request_without_attempt(finish).await
        };
        Self::write_failed("final", &result);
        self.finish_request_metrics(request_id, status, finished_at);
        self.disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .remove(&request_id);
        result
    }

    async fn mark_billing_succeeded(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        if self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id)
        {
            return Ok(());
        }
        let result = self.inner.mark_billing_succeeded(request_id).await;
        Self::write_failed("intermediate", &result);
        result
    }

    async fn mark_billing_failed(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        BILLING_WRITE_FAILURE_TOTAL.inc();
        if self
            .disabled_requests
            .lock()
            .expect("disabled request state poisoned")
            .contains(&request_id)
        {
            return Ok(());
        }
        let result = self.inner.mark_billing_failed(request_id).await;
        Self::write_failed("intermediate", &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MetricsRequestLifecycleRecorder, is_new_node_task_transition, uses_local_attempt_metrics,
    };
    use keycompute_types::{
        ClientResponseOutcome, RequestLifecycleRecorder, RequestStatus, RequestTraceStart,
        RouteType, TestRequestLifecycleRecorder, client_response_trace_finish,
    };
    use std::sync::Arc;
    use uuid::Uuid;

    #[test]
    fn node_task_metrics_start_at_the_real_queue_transition() {
        assert!(!is_new_node_task_transition(
            "unassigned",
            RequestStatus::Received,
            RouteType::Node,
            RequestStatus::Routing,
        ));
        assert!(is_new_node_task_transition(
            "node",
            RequestStatus::Routing,
            RouteType::Node,
            RequestStatus::Queued,
        ));
        assert!(!is_new_node_task_transition(
            "node",
            RequestStatus::Queued,
            RouteType::Node,
            RequestStatus::Queued,
        ));
        assert!(is_new_node_task_transition(
            "node",
            RequestStatus::Queued,
            RouteType::Node,
            RequestStatus::Running,
        ));
    }

    #[test]
    fn node_attempts_do_not_depend_on_process_local_correlation() {
        assert!(!uses_local_attempt_metrics(RouteType::Node));
        assert!(uses_local_attempt_metrics(RouteType::ProviderAccount));
    }

    #[tokio::test]
    async fn cancelled_node_request_releases_process_local_metric_state() {
        let request_id = Uuid::new_v4();
        let recorder =
            MetricsRequestLifecycleRecorder::new(Arc::new(TestRequestLifecycleRecorder::default()));
        recorder
            .start_request(RequestTraceStart {
                request_id,
                client_request_id: None,
                tenant_id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
                produce_ai_key_id: Uuid::new_v4(),
                protocol: "openai".to_string(),
                request_path: "/v1/chat/completions".to_string(),
                requested_model: "node:test".to_string(),
                is_stream: false,
                received_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        recorder
            .set_route(request_id, RouteType::Node, RequestStatus::Queued)
            .await
            .unwrap();

        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
        recorder
            .finish_request_without_attempt(client_response_trace_finish(
                request_id,
                ClientResponseOutcome::ClientDisconnected,
            ))
            .await
            .unwrap();

        assert!(recorder.requests.lock().unwrap().is_empty());
        assert!(recorder.disabled_requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pre_execution_cancellation_releases_process_local_metric_state() {
        let request_id = Uuid::new_v4();
        let inner = Arc::new(TestRequestLifecycleRecorder::default());
        let recorder = Arc::new(MetricsRequestLifecycleRecorder::new(inner));
        recorder
            .start_request(RequestTraceStart {
                request_id,
                client_request_id: None,
                tenant_id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
                produce_ai_key_id: Uuid::new_v4(),
                protocol: "openai".to_string(),
                request_path: "/v1/chat/completions".to_string(),
                requested_model: "test-model".to_string(),
                is_stream: false,
                received_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let lifecycle = Arc::clone(&recorder) as Arc<dyn RequestLifecycleRecorder>;
        drop(crate::handlers::PreExecutionTraceGuard::new(
            lifecycle, request_id,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !recorder.requests.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pre-execution guard should release request metrics");

        assert!(recorder.disabled_requests.lock().unwrap().is_empty());
    }
}
