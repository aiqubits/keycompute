//! 分销规则
//!
//! 读取 tenant_distribution_rules 分账规则

use crate::DistributionLevel;
use rust_decimal::Decimal;
use uuid::Uuid;

/// 分销规则
#[derive(Debug, Clone)]
pub struct DistributionRule {
    /// 规则 ID
    pub id: Uuid,
    /// 租户 ID
    pub tenant_id: Uuid,
    /// 受益人 ID
    pub beneficiary_id: Uuid,
    /// 分成比例
    pub share_ratio: Decimal,
    /// 分销层级
    pub level: DistributionLevel,
    /// 是否启用
    pub enabled: bool,
}

impl DistributionRule {
    /// 创建新的分销规则
    pub fn new(
        tenant_id: Uuid,
        beneficiary_id: Uuid,
        share_ratio: Decimal,
        level: DistributionLevel,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            tenant_id,
            beneficiary_id,
            share_ratio,
            level,
            enabled: true,
        }
    }

    /// 禁用规则
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// 启用规则
    pub fn enable(&mut self) {
        self.enabled = true;
    }
}

/// 规则引擎
#[derive(Debug, Clone, Default)]
pub struct RuleEngine {
    /// 默认第一级分成比例
    default_level1_ratio: Decimal,
    /// 默认第二级分成比例
    default_level2_ratio: Decimal,
}

impl RuleEngine {
    /// 创建新的规则引擎
    ///
    /// 使用配置中的默认比例（3% / 2%）
    pub fn new() -> Self {
        let config = keycompute_config::DistributionConfig::default();
        Self::from_config(&config)
    }

    /// 从配置创建规则引擎
    pub fn from_config(config: &keycompute_config::DistributionConfig) -> Self {
        // 使用 round() 避免浮点数截断误差（例如 0.07*100=6.9999... 截断为 6）
        let level1_ratio =
            Decimal::from_i128_with_scale((config.level1_ratio() * 100.0).round() as i128, 2);
        let level2_ratio =
            Decimal::from_i128_with_scale((config.level2_ratio() * 100.0).round() as i128, 2);
        Self {
            default_level1_ratio: level1_ratio,
            default_level2_ratio: level2_ratio,
        }
    }

    /// 创建带默认比例的规则引擎
    pub fn with_defaults(level1_ratio: Decimal, level2_ratio: Decimal) -> Self {
        Self {
            default_level1_ratio: level1_ratio,
            default_level2_ratio: level2_ratio,
        }
    }

    /// 获取默认比例
    pub fn default_ratios(&self) -> (Decimal, Decimal) {
        (self.default_level1_ratio, self.default_level2_ratio)
    }

    /// 计算有效规则
    ///
    /// 从规则列表中筛选出启用的规则，并计算实际分成比例
    pub fn compute_effective_rules(&self, rules: &[DistributionRule]) -> Vec<DistributionRule> {
        rules.iter().filter(|r| r.enabled).cloned().collect()
    }

    /// 验证规则总和
    ///
    /// 确保同一租户的所有规则分成比例总和不超过上限
    pub fn validate_total_ratio(
        &self,
        rules: &[DistributionRule],
        max_total_ratio: Decimal,
    ) -> bool {
        let total: Decimal = rules
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.share_ratio)
            .sum();
        total <= max_total_ratio
    }
}

/// 规则构建器
#[derive(Debug)]
pub struct DistributionRuleBuilder {
    tenant_id: Option<Uuid>,
    beneficiary_id: Option<Uuid>,
    share_ratio: Option<Decimal>,
    level: Option<DistributionLevel>,
}

impl DistributionRuleBuilder {
    /// 创建新的规则构建器
    pub fn new() -> Self {
        Self {
            tenant_id: None,
            beneficiary_id: None,
            share_ratio: None,
            level: None,
        }
    }

    /// 设置租户 ID
    pub fn tenant_id(mut self, id: Uuid) -> Self {
        self.tenant_id = Some(id);
        self
    }

    /// 设置受益人 ID
    pub fn beneficiary_id(mut self, id: Uuid) -> Self {
        self.beneficiary_id = Some(id);
        self
    }

    /// 设置分成比例
    pub fn share_ratio(mut self, ratio: Decimal) -> Self {
        self.share_ratio = Some(ratio);
        self
    }

    /// 设置分销层级
    pub fn level(mut self, level: DistributionLevel) -> Self {
        self.level = Some(level);
        self
    }

    /// 构建规则
    pub fn build(self) -> Option<DistributionRule> {
        Some(DistributionRule::new(
            self.tenant_id?,
            self.beneficiary_id?,
            self.share_ratio?,
            self.level?,
        ))
    }
}

impl Default for DistributionRuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn test_distribution_rule_new() {
        let tenant_id = Uuid::new_v4();
        let beneficiary_id = Uuid::new_v4();
        let ratio = Decimal::from_f64_retain(0.1).unwrap();

        let rule =
            DistributionRule::new(tenant_id, beneficiary_id, ratio, DistributionLevel::Level1);

        assert_eq!(rule.tenant_id, tenant_id);
        assert_eq!(rule.beneficiary_id, beneficiary_id);
        assert_eq!(rule.share_ratio, ratio);
        assert!(rule.enabled);
    }

    #[test]
    fn test_distribution_rule_disable_enable() {
        let tenant_id = Uuid::new_v4();
        let beneficiary_id = Uuid::new_v4();
        let ratio = Decimal::from_f64_retain(0.1).unwrap();

        let mut rule =
            DistributionRule::new(tenant_id, beneficiary_id, ratio, DistributionLevel::Level1);

        rule.disable();
        assert!(!rule.enabled);

        rule.enable();
        assert!(rule.enabled);
    }

    #[test]
    fn test_rule_engine_defaults() {
        let engine = RuleEngine::new();
        let (l1, l2) = engine.default_ratios();

        // 使用配置中的默认比例 3% / 2%
        // 使用相同的构造方式避免精度问题
        assert_eq!(l1, Decimal::from_i128_with_scale(3, 2));
        assert_eq!(l2, Decimal::from_i128_with_scale(2, 2));
    }

    #[test]
    fn test_rule_engine_compute_effective_rules() {
        let engine = RuleEngine::new();
        let tenant_id = Uuid::new_v4();
        let beneficiary_id = Uuid::new_v4();

        let mut disabled_rule = DistributionRule::new(
            tenant_id,
            beneficiary_id,
            Decimal::from_f64_retain(0.1).unwrap(),
            DistributionLevel::Level1,
        );
        disabled_rule.disable();

        let enabled_rule = DistributionRule::new(
            tenant_id,
            beneficiary_id,
            Decimal::from_f64_retain(0.05).unwrap(),
            DistributionLevel::Level2,
        );

        let rules = vec![disabled_rule, enabled_rule];
        let effective = engine.compute_effective_rules(&rules);

        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].level, DistributionLevel::Level2);
    }

    #[test]
    fn test_rule_engine_validate_total_ratio() {
        let engine = RuleEngine::new();
        let tenant_id = Uuid::new_v4();

        let rules = vec![
            DistributionRule::new(
                tenant_id,
                Uuid::new_v4(),
                Decimal::from_f64_retain(0.1).unwrap(),
                DistributionLevel::Level1,
            ),
            DistributionRule::new(
                tenant_id,
                Uuid::new_v4(),
                Decimal::from_f64_retain(0.05).unwrap(),
                DistributionLevel::Level2,
            ),
        ];

        assert!(engine.validate_total_ratio(&rules, Decimal::from_f64_retain(0.20).unwrap()));
        assert!(!engine.validate_total_ratio(&rules, Decimal::from_f64_retain(0.10).unwrap()));
    }

    #[test]
    fn test_distribution_rule_builder() {
        let tenant_id = Uuid::new_v4();
        let beneficiary_id = Uuid::new_v4();
        let ratio = Decimal::from_f64_retain(0.1).unwrap();

        let rule = DistributionRuleBuilder::new()
            .tenant_id(tenant_id)
            .beneficiary_id(beneficiary_id)
            .share_ratio(ratio)
            .level(DistributionLevel::Level1)
            .build();

        assert!(rule.is_some());
        let rule = rule.unwrap();
        assert_eq!(rule.tenant_id, tenant_id);
        assert_eq!(rule.share_ratio, ratio);
    }

    /// 验证 from_config 中的 round() 修复：浮点数截断误差不会导致精度损失
    /// 例如 0.07 * 100.0 = 6.9999... 应该被 round() 到 7，而不是截断为 6
    #[test]
    fn test_from_config_rounding_precision() {
        use keycompute_config::DistributionConfig;

        // 测试 0.07 (7%) —— 典型的浮点截断误差场景
        let config = DistributionConfig::with_ratios(0.07, 0.03);
        let engine = RuleEngine::from_config(&config);
        let (l1, l2) = engine.default_ratios();
        assert_eq!(l1, Decimal::from_i128_with_scale(7, 2)); // 0.07
        assert_eq!(l2, Decimal::from_i128_with_scale(3, 2)); // 0.03

        // 测试 0.15 (15%) / 0.08 (8%)
        let config = DistributionConfig::with_ratios(0.15, 0.08);
        let engine = RuleEngine::from_config(&config);
        let (l1, l2) = engine.default_ratios();
        assert_eq!(l1, Decimal::from_i128_with_scale(15, 2)); // 0.15
        assert_eq!(l2, Decimal::from_i128_with_scale(8, 2)); // 0.08

        // 测试边界值 0.01 (1%) / 0.99 (99%)
        let config = DistributionConfig::with_ratios(0.01, 0.99);
        let engine = RuleEngine::from_config(&config);
        let (l1, l2) = engine.default_ratios();
        assert_eq!(l1, Decimal::from_i128_with_scale(1, 2)); // 0.01
        assert_eq!(l2, Decimal::from_i128_with_scale(99, 2)); // 0.99

        // 测试 0.0 (0%) 和 0.10 (10%) —— 0.10*100=10.0 无截断问题
        let config = DistributionConfig::with_ratios(0.0, 0.10);
        let engine = RuleEngine::from_config(&config);
        let (l1, l2) = engine.default_ratios();
        assert_eq!(l1, Decimal::from_i128_with_scale(0, 2)); // 0.00
        assert_eq!(l2, Decimal::from_i128_with_scale(10, 2)); // 0.10
    }

    /// 记录 from_config 的精度限制：比例以整百分位（scale=2）存储，
    /// 亚百分位（如 0.5%）会被 round() 舍入到最近的整百分位。
    /// 若未来需支持亚百分位比例，需提高 scale 并同步更新本测试。
    #[test]
    fn test_from_config_sub_percent_rounds_to_whole_percent() {
        use keycompute_config::DistributionConfig;

        // 0.005 (0.5%)：0.5 四舍五入（round half away from zero）到 1 → 0.01
        let config = DistributionConfig::with_ratios(0.005, 0.004);
        let engine = RuleEngine::from_config(&config);
        let (l1, l2) = engine.default_ratios();
        assert_eq!(l1, Decimal::from_i128_with_scale(1, 2)); // 0.005 → 0.01
        assert_eq!(l2, Decimal::from_i128_with_scale(0, 2)); // 0.004 → 0.00

        // 0.001 (0.1%)：低于半百分位，舍入为 0 —— 分销比例实质失效
        let config = DistributionConfig::with_ratios(0.001, 0.024);
        let engine = RuleEngine::from_config(&config);
        let (l1, l2) = engine.default_ratios();
        assert_eq!(l1, Decimal::from_i128_with_scale(0, 2)); // 0.001 → 0.00
        assert_eq!(l2, Decimal::from_i128_with_scale(2, 2)); // 0.024 → 0.02
    }
}
