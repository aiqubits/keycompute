use chrono::{DateTime, Utc};
use keycompute_observability::metrics::{
    MONITORING_ATTEMPT_LATENCY, MONITORING_ATTEMPT_TOTAL, MONITORING_NODE_TASK_TOTAL,
};
use keycompute_types::node::NodeTaskCompleteAction;

#[derive(Debug, Clone, Copy, PartialEq)]
struct NodeCompletionMetric {
    status: &'static str,
    error_origin: &'static str,
    error_category: &'static str,
    attempt_latency_seconds: Option<f64>,
}

fn node_completion_metric(
    action: &NodeTaskCompleteAction,
    is_new_task_transition: bool,
    started_at: Option<DateTime<Utc>>,
    finished_at: DateTime<Utc>,
) -> Option<NodeCompletionMetric> {
    if !is_new_task_transition {
        return None;
    }
    let (status, error_origin, error_category) = match action {
        NodeTaskCompleteAction::Succeeded => ("succeeded", "none", "none"),
        NodeTaskCompleteAction::Requeued | NodeTaskCompleteAction::Failed => {
            ("failed", "node", "node_failed")
        }
        NodeTaskCompleteAction::Expired => ("expired", "node", "node_expired"),
    };
    Some(NodeCompletionMetric {
        status,
        error_origin,
        error_category,
        attempt_latency_seconds: started_at.map(|started_at| {
            finished_at
                .signed_duration_since(started_at)
                .num_milliseconds()
                .max(0) as f64
                / 1000.0
        }),
    })
}

pub(crate) fn record_node_task_running() {
    MONITORING_NODE_TASK_TOTAL
        .with_label_values(&["running"])
        .inc();
}

pub(crate) fn record_node_task_completion(
    action: &NodeTaskCompleteAction,
    is_new_task_transition: bool,
    started_at: Option<DateTime<Utc>>,
    finished_at: DateTime<Utc>,
) {
    let Some(metric) =
        node_completion_metric(action, is_new_task_transition, started_at, finished_at)
    else {
        return;
    };
    MONITORING_NODE_TASK_TOTAL
        .with_label_values(&[metric.status])
        .inc();
    let Some(latency_seconds) = metric.attempt_latency_seconds else {
        return;
    };
    MONITORING_ATTEMPT_TOTAL
        .with_label_values(&[
            "node",
            metric.status,
            metric.error_origin,
            metric.error_category,
        ])
        .inc();
    MONITORING_ATTEMPT_LATENCY
        .with_label_values(&["node", metric.status])
        .observe(latency_seconds);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("valid test timestamp")
    }

    #[test]
    fn completion_metrics_match_node_lifecycle_labels() {
        let cases = [
            (
                NodeTaskCompleteAction::Succeeded,
                ("succeeded", "none", "none"),
            ),
            (
                NodeTaskCompleteAction::Requeued,
                ("failed", "node", "node_failed"),
            ),
            (
                NodeTaskCompleteAction::Failed,
                ("failed", "node", "node_failed"),
            ),
            (
                NodeTaskCompleteAction::Expired,
                ("expired", "node", "node_expired"),
            ),
        ];

        for (action, expected) in cases {
            let metric = node_completion_metric(&action, true, Some(at(10)), at(12)).unwrap();
            assert_eq!(
                (metric.status, metric.error_origin, metric.error_category),
                expected
            );
            assert_eq!(metric.attempt_latency_seconds, Some(2.0));
        }
    }

    #[test]
    fn completion_metrics_ignore_replays_and_unclaimed_attempts() {
        assert!(
            node_completion_metric(
                &NodeTaskCompleteAction::Succeeded,
                false,
                Some(at(10)),
                at(12)
            )
            .is_none()
        );

        let unclaimed =
            node_completion_metric(&NodeTaskCompleteAction::Expired, true, None, at(12)).unwrap();
        assert_eq!(unclaimed.status, "expired");
        assert_eq!(unclaimed.attempt_latency_seconds, None);
    }

    #[test]
    fn completion_latency_is_never_negative() {
        let metric = node_completion_metric(
            &NodeTaskCompleteAction::Succeeded,
            true,
            Some(at(12)),
            at(10),
        )
        .unwrap();
        assert_eq!(metric.attempt_latency_seconds, Some(0.0));
    }
}
