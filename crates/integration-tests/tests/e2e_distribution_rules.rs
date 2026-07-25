//! 分销规则匹配端到端测试
//!
//! 验证全局规则回退、per-user override、优先级约定和 upsert 幂等性

use bigdecimal::BigDecimal;
use integration_tests::common::VerificationChain;
use keycompute_distribution::{DistributionLevel, calculate_shares};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

/// 模拟 TenantDistributionRule 的核心字段（用于测试规则匹配逻辑）
#[derive(Debug, Clone)]
struct MockRule {
    id: Uuid,
    beneficiary_id: Uuid,
    commission_rate: BigDecimal,
    priority: i32,
    is_active: bool,
}

impl MockRule {
    fn new(beneficiary_id: Uuid, rate: &str, priority: i32) -> Self {
        Self {
            id: Uuid::new_v4(),
            beneficiary_id,
            commission_rate: BigDecimal::from_str(rate).unwrap(),
            priority,
            is_active: true,
        }
    }

    fn global(rate: &str, priority: i32) -> Self {
        Self::new(Uuid::nil(), rate, priority)
    }

    /// 已禁用的全局规则（is_active = false），用于验证 find_all 与 find_by_tenant 的差异
    fn global_inactive(rate: &str, priority: i32) -> Self {
        let mut r = Self::new(Uuid::nil(), rate, priority);
        r.is_active = false;
        r
    }
}

/// 模拟 bigdecimal_to_decimal 转换（string-bridge pattern）
fn bigdecimal_to_decimal(bd: &BigDecimal) -> Result<Decimal, String> {
    Decimal::from_str(&bd.to_string()).map_err(|e| e.to_string())
}

/// 模拟 usage_log.rs 中的规则匹配算法
///
/// 复制自 BillingService::trigger_distribution 中的核心规则匹配逻辑：
/// - L1: find(per-user) → or(highest-priority global) → unwrap_or(default)
/// - L2: find(per-user) → or_else(lowest-priority global excluding L1's) → unwrap_or(default)
fn match_distribution_ratios(
    rules: &[MockRule],
    l1_id: Uuid,
    l2_id: Option<Uuid>,
    default_l1: Decimal,
    default_l2: Decimal,
) -> (Decimal, Decimal) {
    let nil_id = Uuid::nil();

    // 规则已按 priority DESC 排序（模拟 SQL ORDER BY priority DESC）
    let l1_global_rule = rules.iter().find(|r| r.beneficiary_id == nil_id);

    let level1_ratio = rules
        .iter()
        .find(|r| r.beneficiary_id == l1_id)
        .or(l1_global_rule)
        .and_then(|r| bigdecimal_to_decimal(&r.commission_rate).ok())
        .unwrap_or(default_l1);

    let level2_ratio = l2_id
        .and_then(|l2| {
            rules
                .iter()
                .find(|r| r.beneficiary_id == l2)
                .and_then(|r| bigdecimal_to_decimal(&r.commission_rate).ok())
        })
        .or_else(|| {
            // L2 回退：最低优先级全局规则，排除 L1 命中的
            let l1_rule_id = l1_global_rule.map(|r| r.id);
            rules
                .iter()
                .rfind(|r| r.beneficiary_id == nil_id && Some(r.id) != l1_rule_id)
                .and_then(|r| bigdecimal_to_decimal(&r.commission_rate).ok())
        })
        .unwrap_or(default_l2);

    (level1_ratio, level2_ratio)
}

/// 测试 1：全局规则回退 — 无 per-user 规则时使用全局规则
///
/// 场景：系统初始化后有两条全局规则（priority 10 和 5），
/// 用户没有专属规则，应回退到全局规则
#[test]
fn test_global_rule_fallback_basic() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4(); // L1 受益人
    let l2_id = Uuid::new_v4(); // L2 受益人
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    // 两条全局规则（按 priority DESC 排序）
    let rules = vec![
        MockRule::global("0.05", 10), // L1 全局规则（较高优先级）
        MockRule::global("0.03", 5),  // L2 全局规则（较低优先级）
    ];

    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "global_fallback_l1",
        format!("L1 ratio: {} (expected 0.05)", l1_ratio),
        l1_ratio == Decimal::from_str("0.05").unwrap(),
    );
    chain.add_step(
        "keycompute-billing",
        "global_fallback_l2",
        format!("L2 ratio: {} (expected 0.03)", l2_ratio),
        l2_ratio == Decimal::from_str("0.03").unwrap(),
    );

    chain.print_report();
    assert!(chain.all_passed(), "Global rule fallback test failed");
}

/// 测试 2：per-user override — 特定用户规则覆盖全局规则
///
/// 场景：L1 受益人有专属规则（0.10），应使用专属规则而非全局规则（0.05）
#[test]
fn test_per_user_override() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    // 规则列表（priority DESC 排序）
    let rules = vec![
        MockRule::global("0.05", 10),    // 全局 L1
        MockRule::global("0.03", 5),     // 全局 L2
        MockRule::new(l1_id, "0.10", 0), // per-user L1 override
    ];

    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "per_user_override_l1",
        format!("L1 ratio: {} (expected 0.10, per-user override)", l1_ratio),
        l1_ratio == Decimal::from_str("0.10").unwrap(),
    );
    chain.add_step(
        "keycompute-billing",
        "per_user_override_l2_fallback",
        format!("L2 ratio: {} (expected 0.03, global fallback)", l2_ratio),
        l2_ratio == Decimal::from_str("0.03").unwrap(),
    );

    // 验证 L1 override 不影响 L2 的全局回退
    chain.add_step(
        "keycompute-billing",
        "l2_independent_of_l1_override",
        "L2 correctly uses lowest-priority global rule",
        l2_ratio != l1_ratio,
    );

    chain.print_report();
    assert!(chain.all_passed(), "Per-user override test failed");
}

/// 测试 3：Admin API 覆盖 — priority 100 规则覆盖初始化默认规则
///
/// 场景：Admin 通过 API 创建了 priority=100 的全局规则，应覆盖初始化的 priority=10 规则
#[test]
fn test_admin_api_priority_override() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    // Admin 创建了 priority=100 的规则 + 初始化的 priority=10/5 规则
    let rules = vec![
        MockRule::global("0.08", 100), // Admin API 创建的覆盖规则
        MockRule::global("0.05", 10),  // 初始化 L1 规则
        MockRule::global("0.03", 5),   // 初始化 L2 规则
    ];

    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "admin_override_l1",
        format!("L1 ratio: {} (expected 0.08, admin override)", l1_ratio),
        l1_ratio == Decimal::from_str("0.08").unwrap(),
    );
    chain.add_step(
        "keycompute-billing",
        "admin_override_l2_uses_lowest",
        format!(
            "L2 ratio: {} (expected 0.03, lowest-priority global excl. L1)",
            l2_ratio
        ),
        l2_ratio == Decimal::from_str("0.03").unwrap(),
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Admin API priority override test failed"
    );
}

/// 测试 4：单一全局规则 — 只有一条全局规则时 L2 回退到默认值
///
/// 场景：租户只有一条全局规则，L1 使用该规则，L2 因排除 L1 后无可用规则，回退到默认
#[test]
fn test_single_global_rule_l2_fallback_to_default() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    // 仅一条全局规则
    let rules = vec![MockRule::global("0.07", 100)];

    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "single_rule_l1",
        format!(
            "L1 ratio: {} (expected 0.07, the only global rule)",
            l1_ratio
        ),
        l1_ratio == Decimal::from_str("0.07").unwrap(),
    );
    chain.add_step(
        "keycompute-billing",
        "single_rule_l2_default",
        format!(
            "L2 ratio: {} (expected 0.02, default because single rule excluded)",
            l2_ratio
        ),
        l2_ratio == default_l2,
    );

    // 关键验证：L1 和 L2 使用不同的佣金率（避免只有一条规则时两级相同）
    chain.add_step(
        "keycompute-billing",
        "no_duplicate_ratio",
        "L1 and L2 ratios are different (single rule correctly excluded)",
        l1_ratio != l2_ratio,
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Single global rule L2 fallback test failed"
    );
}

/// 测试 5：per-user L1 override + 单一全局规则 — L2 排除全局后回退默认值
///
/// 场景：L1 受益人有专属规则，租户另有一条全局规则。L1 使用专属规则；
/// L2 回退时仍排除 l1_global_rule 指向的全局规则（即使 L1 实际未使用它），
/// 因此回退到 default_level2_ratio —— 这是有意设计：单条全局规则语义上是
/// "L1 级别的覆盖规则"，不应被 L2 复用为相同佣金率。
#[test]
fn test_per_user_l1_override_with_single_global_rule() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    // per-user L1 专属规则 + 单一全局规则（priority DESC 排序）
    let rules = vec![
        MockRule::global("0.07", 100),   // 唯一全局规则
        MockRule::new(l1_id, "0.10", 0), // per-user L1 override
    ];

    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "per_user_l1_wins_over_global",
        format!("L1 ratio: {} (expected 0.10, per-user override)", l1_ratio),
        l1_ratio == Decimal::from_str("0.10").unwrap(),
    );
    chain.add_step(
        "keycompute-billing",
        "l2_excludes_single_global_falls_back_default",
        format!(
            "L2 ratio: {} (expected 0.02 default, single global excluded even though L1 used per-user rule)",
            l2_ratio
        ),
        l2_ratio == default_l2,
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Per-user L1 override with single global rule test failed"
    );
}

/// 测试 6：无规则 — 全部回退到配置默认值
#[test]
fn test_no_rules_fallback_to_config_default() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    let rules: Vec<MockRule> = vec![];

    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "no_rules_l1_default",
        format!("L1 ratio: {} (expected 0.03, config default)", l1_ratio),
        l1_ratio == default_l1,
    );
    chain.add_step(
        "keycompute-billing",
        "no_rules_l2_default",
        format!("L2 ratio: {} (expected 0.02, config default)", l2_ratio),
        l2_ratio == default_l2,
    );

    chain.print_report();
    assert!(chain.all_passed(), "No rules fallback test failed");
}

/// 测试 7：无 L2 受益人 — level2_beneficiary 为 None 时的行为
///
/// 验证当用户无二级推荐人时，规则匹配不会产生错误的 L2 分成
#[test]
fn test_no_l2_beneficiary() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    let rules = vec![MockRule::global("0.05", 10), MockRule::global("0.03", 5)];

    // 无 L2 受益人
    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, None, default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "no_l2_beneficiary_l1",
        format!("L1 ratio: {} (expected 0.05)", l1_ratio),
        l1_ratio == Decimal::from_str("0.05").unwrap(),
    );

    // 即使有 L2 ratio，calculate_shares 不会创建 L2 share
    let user_amount = Decimal::from(100);
    let shares = calculate_shares(user_amount, l1_ratio, l2_ratio, l1_id, None);

    chain.add_step(
        "keycompute-distribution",
        "no_l2_share_created",
        format!("Shares count: {} (expected 1, L1 only)", shares.len()),
        shares.len() == 1,
    );
    chain.add_step(
        "keycompute-distribution",
        "l1_share_amount",
        format!("L1 amount: {} (expected 5.0)", shares[0].share_amount),
        shares[0].share_amount == Decimal::from(5),
    );

    chain.print_report();
    assert!(chain.all_passed(), "No L2 beneficiary test failed");
}

/// 测试 8：Upsert 幂等性 — 模拟 handler 的 upsert 查找逻辑
///
/// 验证重复创建全局规则时，handler 能正确识别已有规则并更新
#[test]
fn test_upsert_idempotency_logic() {
    let mut chain = VerificationChain::new();

    // 模拟 handler 的 upsert 查找逻辑：
    // find(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100)

    // 场景 1：初始状态 — 仅有初始化规则（priority 10/5），应走 create 分支
    let initial_rules = [MockRule::global("0.05", 10), MockRule::global("0.03", 5)];

    let existing_p100 = initial_rules
        .iter()
        .find(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100);

    chain.add_step(
        "keycompute-server",
        "upsert_first_call_creates",
        "No existing priority=100 rule → should create",
        existing_p100.is_none(),
    );

    // 场景 2：Admin 已创建 priority=100 规则后，应走 update 分支
    let after_admin_rules = [
        MockRule::global("0.08", 100), // Admin 已创建
        MockRule::global("0.05", 10),
        MockRule::global("0.03", 5),
    ];

    let existing_p100 = after_admin_rules
        .iter()
        .find(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100);

    chain.add_step(
        "keycompute-server",
        "upsert_second_call_updates",
        "Existing priority=100 rule found → should update",
        existing_p100.is_some(),
    );

    // 场景 3：验证 priority=100 匹配不会误命中 per-user 规则
    let per_user_id = Uuid::new_v4();
    let mixed_rules = [
        MockRule::new(per_user_id, "0.10", 100), // per-user at priority 100
        MockRule::global("0.05", 10),
        MockRule::global("0.03", 5),
    ];

    let existing_global_p100 = mixed_rules
        .iter()
        .find(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100);

    chain.add_step(
        "keycompute-server",
        "upsert_skips_per_user_p100",
        "Per-user priority=100 rule NOT matched by global upsert",
        existing_global_p100.is_none(),
    );

    // 场景 4：已禁用的 priority=100 全局规则 — 验证 upsert 使用 find_all_by_tenant（包含禁用）
    // 而非 find_by_tenant（仅激活），以避免重复创建 priority=100 全局规则。
    let rules_with_inactive = [
        MockRule::global_inactive("0.08", 100), // 已被管理员禁用
        MockRule::global("0.05", 10),
        MockRule::global("0.03", 5),
    ];

    // find_all 语义：不过滤 is_active → 能命中禁用规则，走 update（并重新激活）
    let found_via_find_all = rules_with_inactive
        .iter()
        .find(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100);
    chain.add_step(
        "keycompute-server",
        "upsert_find_all_matches_inactive",
        "find_all_by_tenant matches the inactive priority=100 rule → update (reactivate)",
        found_via_find_all.is_some(),
    );

    // 对照：find_by_tenant 语义（仅 is_active=TRUE）会遗漏→ 造成重复创建
    let found_via_active_only = rules_with_inactive
        .iter()
        .filter(|r| r.is_active)
        .find(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100);
    chain.add_step(
        "keycompute-server",
        "upsert_active_only_would_duplicate",
        "active-only lookup misses the disabled rule (regression this fix prevents)",
        found_via_active_only.is_none(),
    );

    chain.print_report();
    assert!(chain.all_passed(), "Upsert idempotency logic test failed");
}

/// 测试 9：分销禁用 — 系统设置关闭分销时应跳过所有计算
///
/// 验证 distribution_enabled = false 时的行为（模拟 BillingService 中的早期返回）
#[test]
fn test_distribution_disabled_skips_calculation() {
    let mut chain = VerificationChain::new();

    // 模拟 SystemSetting::get_bool 返回 false 的场景
    let distribution_enabled = false;

    chain.add_step(
        "keycompute-billing",
        "distribution_disabled_check",
        "distribution_enabled = false, should skip",
        !distribution_enabled,
    );

    // 验证禁用时不会产生分销记录
    let should_skip = !distribution_enabled;
    let shares_created = if should_skip {
        vec![] // 早期返回，无 shares
    } else {
        let user_amount = Decimal::from(100);
        let l1_ratio = Decimal::from_str("0.05").unwrap();
        let l2_ratio = Decimal::from_str("0.03").unwrap();
        calculate_shares(
            user_amount,
            l1_ratio,
            l2_ratio,
            Uuid::new_v4(),
            Some(Uuid::new_v4()),
        )
    };

    chain.add_step(
        "keycompute-billing",
        "no_shares_when_disabled",
        format!("Shares: {} (expected 0)", shares_created.len()),
        shares_created.is_empty(),
    );

    // 对照：启用时应产生分销记录
    let distribution_enabled = true;
    let shares_when_enabled = if !distribution_enabled {
        vec![]
    } else {
        let user_amount = Decimal::from(100);
        let l1_ratio = Decimal::from_str("0.05").unwrap();
        let l2_ratio = Decimal::from_str("0.03").unwrap();
        calculate_shares(
            user_amount,
            l1_ratio,
            l2_ratio,
            Uuid::new_v4(),
            Some(Uuid::new_v4()),
        )
    };

    chain.add_step(
        "keycompute-billing",
        "shares_created_when_enabled",
        format!(
            "Shares when enabled: {} (expected 2)",
            shares_when_enabled.len()
        ),
        shares_when_enabled.len() == 2,
    );

    chain.print_report();
    assert!(chain.all_passed(), "Distribution disabled skip test failed");
}

/// 测试 10：完整分销链路 — 规则匹配 → 金额计算 → 份额验证
///
/// 端到端验证从规则匹配到最终金额的完整流程
#[test]
fn test_full_distribution_chain_with_global_rules() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();
    let user_amount = Decimal::from_str("10.50").unwrap(); // 10.50 CNY

    // Admin 设置了 8% 的覆盖规则，系统初始化了 5%/3% 默认规则
    let rules = vec![
        MockRule::global("0.08", 100), // Admin override
        MockRule::global("0.05", 10),  // Init L1
        MockRule::global("0.03", 5),   // Init L2
    ];

    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    // Step 1: 规则匹配
    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "chain_rule_match",
        format!("L1={}, L2={}", l1_ratio, l2_ratio),
        l1_ratio == Decimal::from_str("0.08").unwrap()
            && l2_ratio == Decimal::from_str("0.03").unwrap(),
    );

    // Step 2: 计算分成
    let shares = calculate_shares(user_amount, l1_ratio, l2_ratio, l1_id, Some(l2_id));

    chain.add_step(
        "keycompute-distribution",
        "chain_shares_count",
        format!("Shares: {}", shares.len()),
        shares.len() == 2,
    );

    // Step 3: 验证金额精度
    let expected_l1 = (user_amount * l1_ratio).round_dp(10);
    let expected_l2 = (user_amount * l2_ratio).round_dp(10);

    chain.add_step(
        "keycompute-distribution",
        "chain_l1_amount",
        format!(
            "L1 amount: {} (expected {})",
            shares[0].share_amount, expected_l1
        ),
        shares[0].share_amount == expected_l1,
    );
    chain.add_step(
        "keycompute-distribution",
        "chain_l2_amount",
        format!(
            "L2 amount: {} (expected {})",
            shares[1].share_amount, expected_l2
        ),
        shares[1].share_amount == expected_l2,
    );

    // Step 4: 验证总分成不超过用户金额
    let total_share: Decimal = shares.iter().map(|s| s.share_amount).sum();
    chain.add_step(
        "keycompute-distribution",
        "chain_total_within_bounds",
        format!(
            "Total share: {} < user_amount: {}",
            total_share, user_amount
        ),
        total_share < user_amount,
    );

    // Step 5: 验证层级标记
    chain.add_step(
        "keycompute-distribution",
        "chain_levels_correct",
        "L1=Level1, L2=Level2",
        shares[0].level == DistributionLevel::Level1
            && shares[1].level == DistributionLevel::Level2,
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Full distribution chain with global rules failed"
    );
}

/// 测试 11：同优先级双全局规则 — find/rfind 在平级时的确定性行为
///
/// 场景：两条同 priority 的全局规则（SQL 排序 `priority DESC, created_at ASC`
/// 在平级时按创建时间升序）：L1 的 `find` 命中最早创建的一条，L2 的 `rfind`
/// 命中最晚创建的一条 —— 行为由向量顺序（即 SQL 排序）决定，固化该边界行为。
/// 同时验证 upsert 自愈保留"最早创建"的策略与 L1 实际命中的规则一致。
#[test]
fn test_equal_priority_global_rules_deterministic_match() {
    let mut chain = VerificationChain::new();

    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();
    let default_l1 = Decimal::from_str("0.03").unwrap();
    let default_l2 = Decimal::from_str("0.02").unwrap();

    // 两条同 priority=100 的全局规则，向量顺序模拟 created_at ASC（先创建的在前）
    let earlier = MockRule::global("0.08", 100); // 最早创建
    let later = MockRule::global("0.06", 100); // 后创建（历史遗留重复）
    let rules = vec![earlier.clone(), later.clone()];

    let (l1_ratio, l2_ratio) =
        match_distribution_ratios(&rules, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-billing",
        "equal_priority_l1_uses_earliest",
        format!("L1 ratio: {} (expected 0.08, earliest created)", l1_ratio),
        l1_ratio == Decimal::from_str("0.08").unwrap(),
    );
    chain.add_step(
        "keycompute-billing",
        "equal_priority_l2_uses_latest",
        format!("L2 ratio: {} (expected 0.06, rfind hits latest)", l2_ratio),
        l2_ratio == Decimal::from_str("0.06").unwrap(),
    );

    // 验证 upsert 自愈的保留策略（最早创建）与 L1 实际命中的规则一致：
    // 自愈停用 later 后，L1 仍命中 earlier，佣金率不变
    let after_heal: Vec<MockRule> = rules
        .iter()
        .filter(|r| r.id == earlier.id || r.beneficiary_id != Uuid::nil())
        .cloned()
        .collect();
    let (healed_l1, healed_l2) =
        match_distribution_ratios(&after_heal, l1_id, Some(l2_id), default_l1, default_l2);

    chain.add_step(
        "keycompute-db",
        "self_heal_keeps_l1_rule",
        format!(
            "L1 after self-heal: {} (unchanged, kept rule == L1's match)",
            healed_l1
        ),
        healed_l1 == l1_ratio,
    );
    chain.add_step(
        "keycompute-db",
        "self_heal_l2_falls_back_default",
        format!(
            "L2 after self-heal: {} (expected 0.02 default, duplicate removed)",
            healed_l2
        ),
        healed_l2 == default_l2,
    );

    chain.print_report();
    assert!(
        chain.all_passed(),
        "Equal-priority global rules deterministic match test failed"
    );
}

// ==================== 真实数据库测试（需要 DATABASE_URL） ====================

mod db_tests {
    use super::*;
    use integration_tests::db::{create_test_pool, create_test_tenant};
    use keycompute_db::{TenantDistributionRule, UpdateDistributionRuleRequest};
    use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
    use std::str::FromStr;

    /// 直接调用生产代码的原子 upsert（写库事务 + advisory lock + 唯一性校验），
    /// 与 handler create_distribution_rule 的调用路径完全一致
    async fn upsert_global_rule(
        pool: &DatabaseConnection,
        tenant_id: Uuid,
        rate: &str,
        name: &str,
    ) -> TenantDistributionRule {
        TenantDistributionRule::upsert_global_override(
            pool,
            tenant_id,
            name,
            BigDecimal::from_str(rate).unwrap(),
        )
        .await
        .expect("upsert_global_override should succeed")
    }

    /// 清理测试租户及其分销规则
    async fn cleanup(pool: &DatabaseConnection, tenant_id: Uuid) {
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM tenant_distribution_rules WHERE tenant_id = $1",
            [tenant_id.into()],
        ))
        .await
        .ok();
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM tenants WHERE id = $1",
            [tenant_id.into()],
        ))
        .await
        .ok();
    }

    /// 测试 12：真实 DB upsert 幂等性 — 重复 upsert 后仅存在一条 priority=100 全局规则
    ///
    /// 验证 handler 的 upsert 语义在真实数据库上成立：
    /// 第一次调用创建规则，第二次调用更新同一条规则（而非重复创建）
    #[tokio::test]
    async fn test_upsert_idempotency_against_real_db() {
        let pool = create_test_pool().await;
        let test_id = &Uuid::new_v4().to_string()[..8];
        let tenant = create_test_tenant(&pool, "dist-upsert", test_id).await;

        // 第一次 upsert：应创建
        let first = upsert_global_rule(&pool, tenant.id, "0.08", "Rule V1").await;
        // 第二次 upsert：应更新同一条规则
        let second = upsert_global_rule(&pool, tenant.id, "0.10", "Rule V2").await;

        assert_eq!(first.id, second.id, "upsert should update the same rule");

        let global_p100: Vec<_> = TenantDistributionRule::find_all_by_tenant(&pool, tenant.id)
            .await
            .expect("find_all_by_tenant should succeed")
            .into_iter()
            .filter(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100)
            .collect();

        assert_eq!(
            global_p100.len(),
            1,
            "exactly one priority=100 global rule should exist after repeated upserts"
        );
        assert_eq!(global_p100[0].name, "Rule V2");
        assert_eq!(
            global_p100[0].commission_rate,
            BigDecimal::from_str("0.10").unwrap()
        );
        assert!(global_p100[0].is_active);

        cleanup(&pool, tenant.id).await;
    }

    /// 测试 13：upsert 命中已禁用规则时重新激活（而非重复创建）
    #[tokio::test]
    async fn test_upsert_reactivates_disabled_rule_against_real_db() {
        let pool = create_test_pool().await;
        let test_id = &Uuid::new_v4().to_string()[..8];
        let tenant = create_test_tenant(&pool, "dist-react", test_id).await;

        let rule = upsert_global_rule(&pool, tenant.id, "0.08", "Rule Active").await;

        // 管理员禁用该规则
        rule.update(
            &pool,
            &UpdateDistributionRuleRequest {
                name: None,
                description: None,
                commission_rate: None,
                priority: None,
                is_active: Some(false),
                effective_until: None,
            },
        )
        .await
        .expect("disable should succeed");

        // 再次 upsert：应命中禁用规则并重新激活，而非创建新规则
        let reactivated = upsert_global_rule(&pool, tenant.id, "0.12", "Rule Reactivated").await;

        assert_eq!(rule.id, reactivated.id, "disabled rule should be reused");
        assert!(reactivated.is_active, "rule should be reactivated");

        let global_p100_count = TenantDistributionRule::find_all_by_tenant(&pool, tenant.id)
            .await
            .expect("find_all_by_tenant should succeed")
            .into_iter()
            .filter(|r| r.beneficiary_id == Uuid::nil() && r.priority == 100)
            .count();
        assert_eq!(
            global_p100_count, 1,
            "no duplicate rule should be created for a disabled rule"
        );

        cleanup(&pool, tenant.id).await;
    }

    /// 测试 14：并发 upsert 原子性 — 多个并发请求不会重复创建 priority=100 全局规则
    ///
    /// 验证写库事务 + 租户级 advisory lock 对 check-then-create/update TOCTOU 竞态的修复：
    /// 10 个并发 upsert 全部成功，且最终仅存在一条 priority=100 全局规则
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_upsert_creates_single_rule() {
        let pool = create_test_pool().await;
        let test_id = &Uuid::new_v4().to_string()[..8];
        let tenant = create_test_tenant(&pool, "dist-conc", test_id).await;

        let mut handles = Vec::new();
        for i in 0..10 {
            let pool = pool.clone();
            let tenant_id = tenant.id;
            handles.push(tokio::spawn(async move {
                TenantDistributionRule::upsert_global_override(
                    &pool,
                    tenant_id,
                    &format!("Concurrent Rule {}", i),
                    BigDecimal::from_str("0.08").unwrap(),
                )
                .await
            }));
        }

        let mut rule_ids = Vec::new();
        for handle in handles {
            let rule = handle
                .await
                .expect("task should not panic")
                .expect("concurrent upsert should succeed");
            rule_ids.push(rule.id);
        }

        // 所有并发 upsert 必须收敛到同一条规则
        assert!(
            rule_ids.iter().all(|id| *id == rule_ids[0]),
            "all concurrent upserts should converge to the same rule"
        );

        let global_p100_count = TenantDistributionRule::find_all_by_tenant(&pool, tenant.id)
            .await
            .expect("find_all_by_tenant should succeed")
            .into_iter()
            .filter(|r| {
                r.beneficiary_id == Uuid::nil()
                    && r.priority == TenantDistributionRule::GLOBAL_OVERRIDE_PRIORITY
            })
            .count();
        assert_eq!(
            global_p100_count, 1,
            "exactly one priority=100 global rule should exist after concurrent upserts"
        );

        cleanup(&pool, tenant.id).await;
    }

    /// 测试 15：历史遗留重复全局规则的自愈 — upsert 保留最早一条并停用其余
    ///
    /// 模拟补丁前的旧代码路径留下的两条 priority=100 全局规则：
    /// upsert 应成功（而非回滚报错锁死端点），保留并更新最早创建的一条，
    /// 自动停用其余重复规则，最终仅剩一条激活态全局规则
    #[tokio::test]
    async fn test_upsert_self_heals_legacy_duplicate_rules() {
        use keycompute_db::CreateDistributionRuleRequest;

        let pool = create_test_pool().await;
        let test_id = &Uuid::new_v4().to_string()[..8];
        let tenant = create_test_tenant(&pool, "dist-heal", test_id).await;

        // 直接用底层 create 植入两条重复的 priority=100 全局规则（模拟旧代码路径）。
        // 假设：两次独立 autocommit INSERT 的 created_at（DEFAULT NOW()，微秒精度）
        // 必不相同，因此"保留最早创建"确定性地命中第一条；时间戳碰撞的
        // id 决胜路径由测试 16 显式覆盖
        let mut legacy_ids = Vec::new();
        for (i, rate) in ["0.05", "0.06"].iter().enumerate() {
            let rule = TenantDistributionRule::create(
                &pool,
                &CreateDistributionRuleRequest {
                    tenant_id: tenant.id,
                    beneficiary_id: Uuid::nil(),
                    name: format!("Legacy Dup {}", i),
                    description: None,
                    commission_rate: BigDecimal::from_str(rate).unwrap(),
                    priority: Some(TenantDistributionRule::GLOBAL_OVERRIDE_PRIORITY),
                    effective_from: None,
                    effective_until: None,
                },
            )
            .await
            .expect("legacy create should succeed");
            legacy_ids.push(rule.id);
        }

        // upsert 应成功自愈，而非因唯一性校验失败锁死端点
        let healed = upsert_global_rule(&pool, tenant.id, "0.09", "Healed Rule").await;

        // 保留并更新的是最早创建的那条
        assert_eq!(
            healed.id, legacy_ids[0],
            "self-heal should keep the earliest-created rule"
        );
        assert_eq!(healed.name, "Healed Rule");
        assert!(healed.is_active);

        let all_p100: Vec<_> = TenantDistributionRule::find_all_by_tenant(&pool, tenant.id)
            .await
            .expect("find_all_by_tenant should succeed")
            .into_iter()
            .filter(|r| {
                r.beneficiary_id == Uuid::nil()
                    && r.priority == TenantDistributionRule::GLOBAL_OVERRIDE_PRIORITY
            })
            .collect();

        // 重复规则未被删除（保留审计痕迹），但仅剩一条激活
        assert_eq!(
            all_p100.len(),
            2,
            "duplicate rule is deactivated, not deleted"
        );
        let active: Vec<_> = all_p100.iter().filter(|r| r.is_active).collect();
        assert_eq!(
            active.len(),
            1,
            "exactly one active global rule after self-heal"
        );
        assert_eq!(active[0].id, legacy_ids[0]);

        // 后续 upsert 仍然幂等（自愈后端点持续可用）
        let again = upsert_global_rule(&pool, tenant.id, "0.11", "Healed Rule V2").await;
        assert_eq!(again.id, healed.id);

        cleanup(&pool, tenant.id).await;
    }

    /// 测试 16：created_at 完全相同时的 id 决胜 — SQL 排序与自愈保留一致
    ///
    /// 用单条多行 INSERT（同事务 NOW()）植入两条 created_at 完全相同的重复
    /// 全局规则，验证：
    /// 1. find_by_tenant 的 `ORDER BY ... id ASC` 决胜使计费 L1 确定性命中 id 较小的一条
    /// 2. 自愈的 (created_at, id) 保留排序保留同一条 —— 两侧行为对齐
    #[tokio::test]
    async fn test_id_tiebreak_on_identical_created_at() {
        let pool = create_test_pool().await;
        let test_id = &Uuid::new_v4().to_string()[..8];
        let tenant = create_test_tenant(&pool, "dist-tie", test_id).await;

        // 客户端生成 id 以便断言；单条语句内 NOW() 恒相同 → created_at 必碰撞。
        // effective_from 错开 1 秒以满足 UNIQUE(tenant_id, beneficiary_id, effective_from)
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        pool.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO tenant_distribution_rules
                (id, tenant_id, beneficiary_id, name, commission_rate, priority, effective_from, created_at)
            VALUES
                ($1, $3, $4, 'Tie Dup A', 0.05, 100, NOW() - INTERVAL '1 second', NOW()),
                ($2, $3, $4, 'Tie Dup B', 0.06, 100, NOW(), NOW())
            "#,
            [id_a.into(), id_b.into(), tenant.id.into(), Uuid::nil().into()],
        ))
        .await
        .expect("seeding identical created_at duplicates should succeed");

        let expected_kept = if id_a < id_b { id_a } else { id_b };

        // 自愈前：计费侧 find_by_tenant 的 id ASC 决胜应使首条 p100 全局规则确定
        let first_match = TenantDistributionRule::find_by_tenant(&pool, tenant.id)
            .await
            .expect("find_by_tenant should succeed")
            .into_iter()
            .find(|r| {
                r.beneficiary_id == Uuid::nil()
                    && r.priority == TenantDistributionRule::GLOBAL_OVERRIDE_PRIORITY
            })
            .expect("a global rule should match");
        assert_eq!(
            first_match.id, expected_kept,
            "billing-side find should hit the smaller-id rule on created_at tie"
        );

        // 自愈：保留的必须是同一条（与计费命中对齐）
        let healed = upsert_global_rule(&pool, tenant.id, "0.09", "Tie Healed").await;
        assert_eq!(
            healed.id, expected_kept,
            "self-heal must keep the same rule billing-side matching hits"
        );

        let active: Vec<_> = TenantDistributionRule::find_all_by_tenant(&pool, tenant.id)
            .await
            .expect("find_all_by_tenant should succeed")
            .into_iter()
            .filter(|r| {
                r.beneficiary_id == Uuid::nil()
                    && r.priority == TenantDistributionRule::GLOBAL_OVERRIDE_PRIORITY
                    && r.is_active
            })
            .collect();
        assert_eq!(
            active.len(),
            1,
            "exactly one active global rule after tie-break heal"
        );
        assert_eq!(active[0].id, expected_kept);

        cleanup(&pool, tenant.id).await;
    }
}
