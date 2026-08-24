use once_cell::sync::Lazy;
use prometheus::{
    Counter, CounterVec, Histogram, HistogramVec, IntCounter, IntGauge, IntGaugeVec, Registry,
    histogram_opts, opts,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 指标初始化状态标志
static METRICS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 检查指标是否已初始化
///
/// 注意：此函数只检查本模块是否已调用过 `init_metrics()`，
/// 不代表指标系统完全可用。如果外部代码直接访问 Lazy 静态变量，
/// 指标可能在本函数返回 false 时已经部分初始化。
pub fn is_metrics_initialized() -> bool {
    METRICS_INITIALIZED.load(Ordering::SeqCst)
}

/// 全局指标注册表
pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

// ==================== 请求指标 ====================

/// 总请求数
pub static REQUEST_TOTAL: Lazy<Counter> = Lazy::new(|| {
    let counter = Counter::with_opts(opts!(
        "keycompute_request_total",
        "Total number of requests"
    ))
    .expect("failed to create request_total counter");
    REGISTRY
        .register(Box::new(counter.clone()))
        .expect("failed to register request_total");
    counter
});

/// 请求延迟分布（秒）
pub static REQUEST_LATENCY: Lazy<Histogram> = Lazy::new(|| {
    let histogram = Histogram::with_opts(histogram_opts!(
        "keycompute_request_latency_seconds",
        "Request latency in seconds",
        vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0
        ]
    ))
    .expect("failed to create request_latency histogram");
    REGISTRY
        .register(Box::new(histogram.clone()))
        .expect("failed to register request_latency");
    histogram
});

/// 活跃请求数
pub static ACTIVE_REQUESTS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::with_opts(opts!(
        "keycompute_active_requests",
        "Number of active requests"
    ))
    .expect("failed to create active_requests gauge");
    REGISTRY
        .register(Box::new(gauge.clone()))
        .expect("failed to register active_requests");
    gauge
});

// ==================== Token 指标 ====================

/// 总处理 token 数
pub static TOKENS_TOTAL: Lazy<Counter> = Lazy::new(|| {
    let counter = Counter::with_opts(opts!(
        "keycompute_tokens_total",
        "Total number of tokens processed"
    ))
    .expect("failed to create tokens_total counter");
    REGISTRY
        .register(Box::new(counter.clone()))
        .expect("failed to register tokens_total");
    counter
});

/// 输入 token 数
pub static INPUT_TOKENS_TOTAL: Lazy<Counter> = Lazy::new(|| {
    let counter = Counter::with_opts(opts!(
        "keycompute_input_tokens_total",
        "Total number of input tokens"
    ))
    .expect("failed to create input_tokens_total counter");
    REGISTRY
        .register(Box::new(counter.clone()))
        .expect("failed to register input_tokens_total");
    counter
});

/// 输出 token 数
pub static OUTPUT_TOKENS_TOTAL: Lazy<Counter> = Lazy::new(|| {
    let counter = Counter::with_opts(opts!(
        "keycompute_output_tokens_total",
        "Total number of output tokens"
    ))
    .expect("failed to create output_tokens_total counter");
    REGISTRY
        .register(Box::new(counter.clone()))
        .expect("failed to register output_tokens_total");
    counter
});

// ==================== Provider 指标 ====================

/// Provider 请求数
pub static PROVIDER_REQUEST_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    let vec = CounterVec::new(
        opts!(
            "keycompute_provider_request_total",
            "Total requests by provider"
        ),
        &["provider", "model"],
    )
    .expect("failed to create provider_request_total counter");
    REGISTRY
        .register(Box::new(vec.clone()))
        .expect("failed to register provider_request_total");
    vec
});

/// Provider 错误数
pub static PROVIDER_ERROR_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    let vec = CounterVec::new(
        opts!(
            "keycompute_provider_error_total",
            "Total errors by provider"
        ),
        &["provider", "error_type"],
    )
    .expect("failed to create provider_error_total counter");
    REGISTRY
        .register(Box::new(vec.clone()))
        .expect("failed to register provider_error_total");
    vec
});

/// Provider 延迟
pub static PROVIDER_LATENCY: Lazy<HistogramVec> = Lazy::new(|| {
    let vec = HistogramVec::new(
        histogram_opts!(
            "keycompute_provider_latency_seconds",
            "Provider request latency",
            vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0
            ]
        ),
        &["provider", "model"],
    )
    .expect("failed to create provider_latency histogram");
    REGISTRY
        .register(Box::new(vec.clone()))
        .expect("failed to register provider_latency");
    vec
});

// ==================== 计费指标 ====================

/// 计费金额总计
pub static BILLING_AMOUNT_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    let vec = CounterVec::new(
        opts!("keycompute_billing_amount_total", "Total billing amount"),
        &["currency"],
    )
    .expect("failed to create billing_amount_total counter");
    REGISTRY
        .register(Box::new(vec.clone()))
        .expect("failed to register billing_amount_total");
    vec
});

/// Fallback 次数
pub static FALLBACK_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let counter = IntCounter::with_opts(opts!(
        "keycompute_fallback_total",
        "Total number of fallback operations"
    ))
    .expect("failed to create fallback_total counter");
    REGISTRY
        .register(Box::new(counter.clone()))
        .expect("failed to register fallback_total");
    counter
});

// ==================== Gateway lifecycle tracing metrics ====================

pub static MONITORING_REQUEST_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec(
        "keycompute_monitoring_request_total",
        "Terminal gateway requests recorded by lifecycle tracing",
        &["protocol", "route_type", "status"],
    )
});

pub static MONITORING_ACTIVE_REQUESTS: Lazy<IntGaugeVec> = Lazy::new(|| {
    let metric = IntGaugeVec::new(
        opts!(
            "keycompute_monitoring_active_requests",
            "Gateway requests currently active in lifecycle tracing"
        ),
        &["protocol", "route_type"],
    )
    .expect("failed to create monitoring_active_requests");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("failed to register monitoring_active_requests");
    metric
});

pub static MONITORING_REQUEST_LATENCY: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec(
        "keycompute_monitoring_request_latency_seconds",
        "End-to-end gateway request latency",
        &["protocol", "route_type", "status"],
    )
});

pub static MONITORING_ATTEMPT_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec(
        "keycompute_monitoring_attempt_total",
        "Terminal execution attempts recorded by lifecycle tracing",
        &["route_type", "status", "error_origin", "error_category"],
    )
});

pub static MONITORING_ATTEMPT_LATENCY: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec(
        "keycompute_monitoring_attempt_latency_seconds",
        "Execution attempt latency",
        &["route_type", "status"],
    )
});

pub static MONITORING_NODE_TASK_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec(
        "keycompute_monitoring_node_task_total",
        "Node task lifecycle outcomes",
        &["status"],
    )
});

pub static TRACE_WRITE_FAILURE_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec(
        "keycompute_trace_write_failure_total",
        "Lifecycle trace write failures",
        &["phase"],
    )
});

pub static TRACE_INTERMEDIATE_QUEUE_DROPS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let metric = IntCounter::with_opts(opts!(
        "keycompute_trace_intermediate_queue_drops_total",
        "Dropped best-effort intermediate lifecycle events"
    ))
    .expect("failed to create trace queue drop counter");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("failed to register trace queue drop counter");
    metric
});

pub static CLIENT_REQUEST_ID_REJECTED_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let metric = IntCounter::with_opts(opts!(
        "keycompute_client_request_id_rejected_total",
        "Invalid untrusted client request IDs ignored by the gateway"
    ))
    .expect("failed to create client request ID rejection counter");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("failed to register client request ID rejection counter");
    metric
});

pub static STALE_REQUEST_RECONCILED_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let metric = IntCounter::with_opts(opts!(
        "keycompute_stale_request_reconciled_total",
        "Abandoned request traces reconciled"
    ))
    .expect("failed to create stale reconciled counter");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("failed to register stale reconciled counter");
    metric
});

pub static BILLING_WRITE_FAILURE_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let metric = IntCounter::with_opts(opts!(
        "keycompute_billing_write_failure_total",
        "Billing finalization write failures"
    ))
    .expect("failed to create billing write failure counter");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("failed to register billing write failure counter");
    metric
});

pub static ACCOUNT_PROBE_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec(
        "keycompute_account_probe_total",
        "Provider account probe outcomes",
        &["status"],
    )
});

pub static ACCOUNT_PROBE_LATENCY: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec(
        "keycompute_account_probe_latency_seconds",
        "Provider account probe latency",
        &["status"],
    )
});

fn register_counter_vec(name: &str, help: &str, labels: &[&str]) -> CounterVec {
    let metric = CounterVec::new(opts!(name, help), labels).expect("failed to create counter vec");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("failed to register counter vec");
    metric
}

fn register_histogram_vec(name: &str, help: &str, labels: &[&str]) -> HistogramVec {
    let metric = HistogramVec::new(
        histogram_opts!(
            name,
            help,
            vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 120.0
            ]
        ),
        labels,
    )
    .expect("failed to create histogram vec");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("failed to register histogram vec");
    metric
}

// ==================== 自定义指标类型包装 ====================

/// 指标收集器
#[derive(Clone)]
pub struct MetricsCollector {
    registry: Arc<Registry>,
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self {
            registry: Arc::new(REGISTRY.clone()),
        }
    }
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录请求开始
    pub fn request_started(&self) {
        REQUEST_TOTAL.inc();
        ACTIVE_REQUESTS.inc();
    }

    /// 记录请求完成
    pub fn request_completed(&self, duration_secs: f64) {
        ACTIVE_REQUESTS.dec();
        REQUEST_LATENCY.observe(duration_secs);
    }

    /// 记录 token 使用量
    pub fn record_tokens(&self, input_tokens: u64, output_tokens: u64) {
        INPUT_TOKENS_TOTAL.inc_by(input_tokens as f64);
        OUTPUT_TOKENS_TOTAL.inc_by(output_tokens as f64);
        TOKENS_TOTAL.inc_by((input_tokens + output_tokens) as f64);
    }

    /// 记录 fallback
    pub fn record_fallback(&self) {
        FALLBACK_TOTAL.inc();
    }

    /// 获取 Prometheus 格式的指标输出
    pub fn gather(&self) -> Vec<prometheus::proto::MetricFamily> {
        self.registry.gather()
    }

    /// 将指标编码为文本格式
    pub fn encode_text(&self) -> Result<String, prometheus::Error> {
        let encoder = prometheus::TextEncoder::new();
        let metric_families = self.registry.gather();
        encoder.encode_to_string(&metric_families)
    }
}

/// 初始化所有指标（确保在程序启动时调用）
///
/// 此函数是线程安全的，可以安全地多次调用。如果指标已经初始化，
/// 后续调用会跳过重复初始化。使用 `is_metrics_initialized()` 可以检查初始化状态。
pub fn init_metrics() {
    // 使用 compare_exchange 实现原子性的检查和设置，避免竞态条件
    if METRICS_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // 已经初始化过了，跳过
        return;
    }

    // 触发所有 Lazy 静态变量的初始化
    let _ = &*REQUEST_TOTAL;
    let _ = &*REQUEST_LATENCY;
    let _ = &*ACTIVE_REQUESTS;
    let _ = &*TOKENS_TOTAL;
    let _ = &*INPUT_TOKENS_TOTAL;
    let _ = &*OUTPUT_TOKENS_TOTAL;
    let _ = &*FALLBACK_TOTAL;
    let _ = &*PROVIDER_REQUEST_TOTAL;
    let _ = &*PROVIDER_ERROR_TOTAL;
    let _ = &*PROVIDER_LATENCY;
    let _ = &*BILLING_AMOUNT_TOTAL;
    let _ = &*MONITORING_REQUEST_TOTAL;
    let _ = &*MONITORING_ACTIVE_REQUESTS;
    let _ = &*MONITORING_REQUEST_LATENCY;
    let _ = &*MONITORING_ATTEMPT_TOTAL;
    let _ = &*MONITORING_ATTEMPT_LATENCY;
    let _ = &*MONITORING_NODE_TASK_TOTAL;
    let _ = &*TRACE_WRITE_FAILURE_TOTAL;
    let _ = &*TRACE_INTERMEDIATE_QUEUE_DROPS_TOTAL;
    let _ = &*CLIENT_REQUEST_ID_REJECTED_TOTAL;
    let _ = &*STALE_REQUEST_RECONCILED_TOTAL;
    let _ = &*BILLING_WRITE_FAILURE_TOTAL;
    let _ = &*ACCOUNT_PROBE_TOTAL;
    let _ = &*ACCOUNT_PROBE_LATENCY;
}
