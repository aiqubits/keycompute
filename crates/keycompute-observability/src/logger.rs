use std::sync::atomic::{AtomicBool, Ordering};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// 日志初始化状态标志
static LOGGER_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 检查日志系统是否已初始化
///
/// 注意：此函数只检查本模块是否已调用过初始化函数，
/// 不代表全局 tracing subscriber 的实际状态。
/// 如果其他代码设置了全局 subscriber，此函数仍可能返回 false。
pub fn is_logger_initialized() -> bool {
    LOGGER_INITIALIZED.load(Ordering::SeqCst)
}

/// 日志格式枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFormat {
    /// JSON 格式（适合日志采集系统）
    Json,
    /// 紧凑格式（适合本地终端快速查看）
    Compact,
    /// Full 格式（含文件路径、行号、目标模块）
    Full,
}

/// 判断应使用的日志格式
///
/// 两级优先级策略：
/// 1. `KC__LOG_FORMAT` 环境变量（显式覆盖，最高优先级）：
///    - `json`（大小写不敏感）→ `LogFormat::Json`
///    - `compact`（大小写不敏感）→ `LogFormat::Compact`
///    - 其他值 → `LogFormat::Full`
/// 2. `KC__ENV` 环境变量（`KC__LOG_FORMAT` 未设置时作为智能回退）：
///    - `production` / `prod` → `LogFormat::Json`
///    - 其他值或未设置 → `LogFormat::Full`（默认）
fn get_log_format() -> LogFormat {
    log_format_from_values(
        std::env::var("KC__LOG_FORMAT").ok().as_deref(),
        std::env::var("KC__ENV").ok().as_deref(),
    )
}

fn log_format_from_values(log_format: Option<&str>, environment: Option<&str>) -> LogFormat {
    if let Some(val) = log_format {
        let trimmed = val.trim();
        if trimmed.eq_ignore_ascii_case("json") {
            return LogFormat::Json;
        }
        if trimmed.eq_ignore_ascii_case("compact") {
            return LogFormat::Compact;
        }
        // 显式设置但值无法识别 → 回退到 Full
        return LogFormat::Full;
    }

    match environment.map(str::trim) {
        Some(value)
            if value.eq_ignore_ascii_case("production") || value.eq_ignore_ascii_case("prod") =>
        {
            LogFormat::Json
        }
        _ => LogFormat::Full,
    }
}

/// 使用指定的 filter 构建并尝试初始化 tracing subscriber
///
/// 根据 `KC__LOG_FORMAT` 环境变量决定日志格式：
/// - `json` → JSON 格式
/// - `compact` → 紧凑格式
/// - 其他值或未设置 → 根据 `KC__ENV` 智能回退（`production` / `prod` → JSON，其他 → Full）
///
/// 将格式分支提取为共享函数，消除各初始化函数之间的代码重复。
///
/// 注意：json() / compact() / with_file() 会改变 Layer 的泛型类型，必须分为多条路径
fn try_init_subscriber_with_filter(
    filter: EnvFilter,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match get_log_format() {
        LogFormat::Json => Ok(tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()?),
        LogFormat::Compact => Ok(tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().compact())
            .try_init()?),
        LogFormat::Full => Ok(tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_file(true)
                    .with_line_number(true)
                    .with_target(true),
            )
            .try_init()?),
    }
}

/// 构建并尝试初始化 tracing subscriber（使用默认 filter）
///
/// 使用 `RUST_LOG` 环境变量或默认级别 `info` 构建 filter，
/// 然后根据 `KC__LOG_FORMAT` 环境变量决定日志格式。
fn try_init_subscriber() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info")
            .add_directive("keycompute=info".parse().unwrap())
            .add_directive("tower_http=info".parse().unwrap())
    });
    try_init_subscriber_with_filter(filter)
}

/// 尝试初始化日志系统
///
/// 返回 `true` 表示初始化成功或日志系统已经可用。
/// 返回 `false` 表示初始化失败（极少见）。
///
/// 此函数是线程安全的，可以安全地多次调用。
/// 即使全局 subscriber 已被其他代码设置，此函数也不会 panic。
///
/// 日志格式由 `KC__LOG_FORMAT` 环境变量控制（两级优先级）：
/// - `json`：JSON 格式，适配日志采集系统
/// - `compact`：紧凑格式，适合终端快速查看
/// - 其他值：Full 格式
/// - 未设置时：根据 `KC__ENV` 回退（`production` / `prod` → JSON，其他 → Full）
pub fn try_init_logger() -> bool {
    // 使用 compare_exchange 实现原子性的检查和设置，避免竞态条件
    // 如果已经是 true，直接返回成功
    if LOGGER_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // 本模块已初始化过
        return true;
    }

    // 使用 try_init 避免在全局 subscriber 已存在时 panic
    match try_init_subscriber() {
        Ok(_) => {
            // 初始化成功
            true
        }
        Err(_) => {
            // 全局 subscriber 可能已被其他代码设置
            // tracing 全局 subscriber 一旦设置就无法更改
            // 这种情况下日志系统已经可用，视为成功
            true
        }
    }
}

/// 初始化日志系统
///
/// 使用 tracing-subscriber 配置结构化日志输出。
/// 环境变量 KEYCOMPUTE_LOG 控制日志级别，默认为 info。
///
/// 日志格式由 `KC__LOG_FORMAT` 环境变量控制（两级优先级）：
/// - `json`：JSON 格式，适配日志采集系统
/// - `compact`：紧凑格式，适合终端快速查看
/// - 其他值：Full 格式
/// - 未设置时：根据 `KC__ENV` 回退（`production` / `prod` → JSON，其他 → Full）
///
/// 此函数是线程安全的，可以安全地多次调用。如果日志系统已经初始化
/// （无论是本模块还是其他代码），后续调用会静默跳过。
///
/// # Examples
///
/// ```
/// use keycompute_observability::init_logger;
/// init_logger();
/// ```
pub fn init_logger() {
    // 使用 compare_exchange 实现原子性的检查和设置，避免竞态条件
    if LOGGER_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // 本模块已初始化过，静默跳过
        return;
    }

    // 使用 try_init 避免在全局 subscriber 已存在时 panic
    let _ = try_init_subscriber();
}

/// 初始化开发环境日志（人类可读格式）
///
/// 适用于本地开发，debug 级别输出便于调试。
/// 日志格式同样遵循 `KC__LOG_FORMAT` 环境变量控制：
/// - `json` → JSON 格式（若需要在本地测试 JSON 日志）
/// - `compact` → 紧凑格式
/// - 其他值或未设置 → Full 格式（默认，适合终端调试）
///
/// 此函数是线程安全的，可以安全地多次调用。如果日志系统已经初始化
/// （无论是本模块还是其他代码），后续调用会静默跳过。
pub fn init_dev_logger() {
    // 使用 compare_exchange 实现原子性的检查和设置，避免竞态条件
    if LOGGER_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // 本模块已初始化过，静默跳过
        return;
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("debug").add_directive("keycompute=debug".parse().unwrap())
    });

    // 使用 try_init 避免在全局 subscriber 已存在时 panic
    let _ = try_init_subscriber_with_filter(filter);
}

/// 初始化测试环境日志
///
/// 仅在测试时启用，避免污染测试输出
#[cfg(test)]
pub fn init_test_logger() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("error")
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::{LogFormat, log_format_from_values};

    #[test]
    fn prod_alias_uses_json_log_format() {
        assert_eq!(log_format_from_values(None, Some("prod")), LogFormat::Json);
        assert_eq!(
            log_format_from_values(None, Some("production")),
            LogFormat::Json
        );
    }

    #[test]
    fn explicit_log_format_still_has_priority() {
        assert_eq!(
            log_format_from_values(Some("compact"), Some("prod")),
            LogFormat::Compact
        );
    }
}
