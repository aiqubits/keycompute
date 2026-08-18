use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Token 累积器：streaming 过程中原子更新
///
/// 设计说明：
/// - 使用 AtomicU32 实现原子更新，支持并发访问
/// - 使用 `output_finalized` 标志位防止 set_output 和 add_output 的竞态条件
/// - 当 set_output 被调用后，output_finalized 设为 true，后续 add_output 会被忽略
/// - 同样的保护也适用于 input：使用 `input_finalized` 标志
#[derive(Debug, Default)]
pub struct UsageAccumulator {
    input_tokens: AtomicU32,
    output_tokens: AtomicU32,
    /// 标记 output usage 是否已被精确值覆盖
    /// 设置为 true 后，后续的 add_output 调用会被忽略
    output_finalized: AtomicBool,
    /// 标记 input usage 是否已被精确值覆盖
    /// 设置为 true 后，后续的 add_input 调用会被忽略
    input_finalized: AtomicBool,
}

impl UsageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加输出 token（流式响应中累积）
    ///
    /// 注意：如果之前调用过 set_output()，此方法会被忽略
    /// 这是为了防止 Usage 事件覆盖后，后续的 Delta 事件继续累积
    pub fn add_output(&self, tokens: u32) {
        // 如果 usage 已经被精确值覆盖，忽略估算值
        if self.output_finalized.load(Ordering::Relaxed) {
            return;
        }
        self.output_tokens.fetch_add(tokens, Ordering::Relaxed);
    }

    /// 添加输入 token（流式开始时估算）
    ///
    /// 注意：如果之前调用过 set_input()，此方法会被忽略
    pub fn add_input(&self, tokens: u32) {
        if self.input_finalized.load(Ordering::Relaxed) {
            return;
        }
        self.input_tokens.fetch_add(tokens, Ordering::Relaxed);
    }

    /// 设置输出 token（用于覆盖估算值）
    ///
    /// 当 Provider 返回精确的 usage 信息时，使用此方法直接设置输出 token 数
    /// 而非累积，确保与 Provider 的计费完全一致
    ///
    /// 注意：调用此方法后，后续的 add_output 调用会被忽略
    pub fn set_output(&self, tokens: u32) {
        self.output_tokens.store(tokens, Ordering::Relaxed);
        self.output_finalized.store(true, Ordering::Relaxed);
    }

    /// 设置输入 token（用于覆盖估算值）
    ///
    /// 当 Provider 返回精确的 usage 信息时，使用此方法直接设置输入 token 数
    ///
    /// 注意：调用此方法后，后续的 add_input 调用会被忽略
    pub fn set_input(&self, tokens: u32) {
        self.input_tokens.store(tokens, Ordering::Relaxed);
        self.input_finalized.store(true, Ordering::Relaxed);
    }

    /// 设置输入 token 的估算值（流开始时）。
    ///
    /// 与 `set_input` 的区别：不标记 `input_finalized`，因为这是 tiktoken
    /// 估算而非 Provider 精确值；收到 `InputUsage`/`Usage` 时仍可用
    /// `set_input` 覆盖并锁定，`is_input_finalized` 因此保持“是否精确”语义。
    /// 同时清除既有的 finalized 标志：fallback 的新尝试不应继承上一次
    /// 尝试已锁定的精确值状态。
    pub fn set_input_estimate(&self, tokens: u32) {
        self.input_tokens.store(tokens, Ordering::Relaxed);
        self.input_finalized.store(false, Ordering::Relaxed);
    }

    /// 设置输出 token 的估算值（流开始时）。
    ///
    /// 与 `set_output` 的区别：不标记 `output_finalized`。用于 fallback
    /// 场景：新的一次上游尝试不应继承上一次失败尝试的精确 output 值，
    /// 否则 fallback 若无最终 Usage 事件，计费会错误沿用残留值。
    pub fn set_output_estimate(&self, tokens: u32) {
        self.output_tokens.store(tokens, Ordering::Relaxed);
        self.output_finalized.store(false, Ordering::Relaxed);
    }

    /// 获取当前用量快照
    pub fn snapshot(&self) -> (u32, u32) {
        (
            self.input_tokens.load(Ordering::Relaxed),
            self.output_tokens.load(Ordering::Relaxed),
        )
    }

    /// 获取总 token 数
    pub fn total_tokens(&self) -> u32 {
        let (input, output) = self.snapshot();
        input + output
    }

    /// 检查 output usage 是否已被精确值覆盖
    #[allow(dead_code)]
    pub fn is_output_finalized(&self) -> bool {
        self.output_finalized.load(Ordering::Relaxed)
    }

    /// 检查 input usage 是否已被精确值覆盖
    #[allow(dead_code)]
    pub fn is_input_finalized(&self) -> bool {
        self.input_finalized.load(Ordering::Relaxed)
    }
}

/// 从 snapshot 创建新的 UsageAccumulator
///
/// 注意：这会创建独立的 Accumulator，不会共享状态。
/// 如需共享状态，请使用 Arc<UsageAccumulator>。
///
/// 注意：使用此方法创建时，input_finalized 和 output_finalized 保持为 false
impl From<(u32, u32)> for UsageAccumulator {
    fn from((input, output): (u32, u32)) -> Self {
        let acc = Self::new();
        // 直接存储值，不设置 finalized 标志
        // 因为这是从 snapshot 初始化的数据，后续仍可能被更新
        acc.input_tokens.store(input, Ordering::Relaxed);
        acc.output_tokens.store(output, Ordering::Relaxed);
        acc
    }
}

/// 最终用量记录
#[derive(Debug, Clone, Copy)]
pub struct UsageRecord {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl UsageRecord {
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

impl From<(u32, u32)> for UsageRecord {
    fn from((input, output): (u32, u32)) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_accumulator_add_then_set() {
        // 测试：先 add_output 后 set_output，set 应该覆盖 add
        let acc = UsageAccumulator::new();

        // 初始状态
        assert_eq!(acc.snapshot(), (0, 0));
        assert!(!acc.is_output_finalized());

        // 添加一些 token
        acc.add_output(100);
        assert_eq!(acc.snapshot().1, 100);

        acc.add_output(50);
        assert_eq!(acc.snapshot().1, 150);

        // set_output 应该覆盖之前的累加值
        acc.set_output(200);
        assert_eq!(acc.snapshot().1, 200);
        assert!(acc.is_output_finalized());

        // 后续的 add_output 会被忽略
        acc.add_output(999);
        assert_eq!(acc.snapshot().1, 200); // 仍然是 200
    }

    #[test]
    fn test_usage_accumulator_set_then_add() {
        // 测试：先 set_output 后 add_output，add 应该被忽略
        let acc = UsageAccumulator::new();

        // 初始状态
        assert!(!acc.is_output_finalized());

        // 直接 set_output
        acc.set_output(200);
        assert_eq!(acc.snapshot().1, 200);
        assert!(acc.is_output_finalized());

        // 后续的 add_output 会被忽略
        acc.add_output(100);
        assert_eq!(acc.snapshot().1, 200); // 仍然是 200，未累加

        acc.add_output(50);
        assert_eq!(acc.snapshot().1, 200); // 仍然是 200
    }

    #[test]
    fn test_input_finalized_protection() {
        // 测试：input 的 finalized 保护
        let acc = UsageAccumulator::new();

        // 初始状态
        assert!(!acc.is_input_finalized());

        // 直接 set_input
        acc.set_input(100);
        assert_eq!(acc.snapshot().0, 100);
        assert!(acc.is_input_finalized());

        // 后续的 add_input 会被忽略
        acc.add_input(999);
        assert_eq!(acc.snapshot().0, 100); // 仍然是 100
    }

    #[test]
    fn test_input_estimate_does_not_finalize() {
        // 估算值可被后续精确值覆盖，且不应把 input 标记为 finalized。
        let acc = UsageAccumulator::new();
        acc.set_input_estimate(50);
        assert_eq!(acc.snapshot().0, 50);
        assert!(!acc.is_input_finalized());

        // 估算后仍可继续累加（估算尚未被“锁定”）。
        acc.add_input(10);
        assert_eq!(acc.snapshot().0, 60);

        // 精确值到达后锁定，后续累加被忽略。
        acc.set_input(100);
        assert!(acc.is_input_finalized());
        acc.add_input(999);
        assert_eq!(acc.snapshot().0, 100);
    }

    #[test]
    fn test_output_estimate_resets_without_finalizing() {
        // fallback 场景：新的上游尝试必须清除上一次尝试的精确 output 值，
        // 且不把 output 标记为 finalized（后续仍可累积估算）。
        let acc = UsageAccumulator::new();
        acc.set_output(222);
        assert!(acc.is_output_finalized());

        acc.set_output_estimate(0);
        assert_eq!(acc.snapshot().1, 0);
        assert!(!acc.is_output_finalized());

        // 重置后可继续累积估算。
        acc.add_output(15);
        assert_eq!(acc.snapshot().1, 15);
    }

    #[test]
    fn test_both_finalized_independently() {
        // 测试：input 和 output 的 finalized 是独立的
        let acc = UsageAccumulator::new();

        // 先 finalized output
        acc.set_output(100);
        assert!(acc.is_output_finalized());
        assert!(!acc.is_input_finalized());

        // 再 finalized input
        acc.set_input(50);
        assert!(acc.is_output_finalized());
        assert!(acc.is_input_finalized());

        // 验证值
        assert_eq!(acc.snapshot(), (50, 100));
    }

    #[test]
    fn test_from_tuple_preserves_mutability() {
        // 测试：从 tuple 创建的 Accumulator 仍然可以被更新
        let acc = UsageAccumulator::from((10, 20));
        assert_eq!(acc.snapshot(), (10, 20));
        assert!(!acc.is_input_finalized());
        assert!(!acc.is_output_finalized());

        // 可以继续 add
        acc.add_output(30);
        assert_eq!(acc.snapshot().1, 50);

        // 可以继续 add_input
        acc.add_input(5);
        assert_eq!(acc.snapshot().0, 15);
    }

    #[test]
    fn test_total_tokens() {
        let acc = UsageAccumulator::new();
        acc.add_input(100);
        acc.add_output(50);
        assert_eq!(acc.total_tokens(), 150);
    }

    #[test]
    fn test_new_default() {
        let acc = UsageAccumulator::new();
        assert_eq!(acc.snapshot(), (0, 0));
        assert!(!acc.is_input_finalized());
        assert!(!acc.is_output_finalized());
    }
}
