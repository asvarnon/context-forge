use crate::scoring::ScoringConfig;

/// Configuration for progressive injection policy.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InjectionConfig {
    /// Budget escalation factor per compaction level (default: 0.25).
    pub escalation_factor: f64,
    /// Optional absolute cap on scaled budget.
    pub max_budget_cap: Option<usize>,
}

impl Default for InjectionConfig {
    fn default() -> Self {
        Self {
            escalation_factor: 0.25,
            max_budget_cap: None,
        }
    }
}

/// Compute the effective token budget based on compaction depth.
///
/// Formula: `base_budget * (1 + (compaction_count - 1) * escalation_factor)`
/// Clamped to `max_budget_cap` if set.
///
/// `compaction_count` of 0 or `None` is treated as 1 (no scaling).
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "f64 scaling arithmetic is required by the policy formula"
)]
pub fn scale_budget(
    base_budget: usize,
    compaction_count: Option<i64>,
    config: &InjectionConfig,
) -> usize {
    let count = compaction_count.unwrap_or(1);
    if count <= 1 || config.escalation_factor <= 0.0 {
        return base_budget;
    }

    let scale = 1.0 + (count - 1) as f64 * config.escalation_factor;
    let scaled = base_budget as f64 * scale;
    let rounded = scaled.round();

    let mut result = if !rounded.is_finite() || rounded <= 0.0 {
        base_budget
    } else if rounded >= usize::MAX as f64 {
        usize::MAX
    } else {
        rounded as usize
    };

    if let Some(cap) = config.max_budget_cap {
        result = result.min(cap);
    }

    result
}

/// Return a `ScoringConfig` with category weights redistributed
/// according to the compaction-level priority table.
///
/// Weight values (1.5, 1.3, 1.2, 1.0) are reassigned by priority order:
/// - Count 0-1: Corrective > Decisive > Stateful > Reinforcing (default)
/// - Count 2: Reinforcing > Corrective > Stateful > Decisive
/// - Count 3+: Reinforcing > Stateful > Corrective > Decisive
///
/// `uncategorized_weight` and `importance_half_life_secs` are preserved from `base`.
#[must_use]
pub fn adjust_weights(base: &ScoringConfig, compaction_count: Option<i64>) -> ScoringConfig {
    let mut adjusted = base.clone();
    let count = compaction_count.unwrap_or(1);

    if count == 2 {
        adjusted.reinforcing_weight = 1.5;
        adjusted.corrective_weight = 1.3;
        adjusted.stateful_weight = 1.2;
        adjusted.decisive_weight = 1.0;
        return adjusted;
    }

    if count >= 3 {
        adjusted.reinforcing_weight = 1.5;
        adjusted.stateful_weight = 1.3;
        adjusted.corrective_weight = 1.2;
        adjusted.decisive_weight = 1.0;
        return adjusted;
    }

    adjusted.corrective_weight = 1.5;
    adjusted.decisive_weight = 1.3;
    adjusted.stateful_weight = 1.2;
    adjusted.reinforcing_weight = 1.0;
    adjusted
}

#[cfg(test)]
mod tests {
    use super::*;

    // Budget scaling tests
    #[test]
    fn scale_budget_no_scaling_at_count_one() {
        let config = InjectionConfig::default();
        assert_eq!(scale_budget(2048, Some(1), &config), 2048);
    }

    #[test]
    fn scale_budget_no_scaling_at_count_zero() {
        let config = InjectionConfig::default();
        assert_eq!(scale_budget(2048, Some(0), &config), 2048);
    }

    #[test]
    fn scale_budget_no_scaling_at_count_none() {
        let config = InjectionConfig::default();
        assert_eq!(scale_budget(2048, None, &config), 2048);
    }

    #[test]
    fn scale_budget_scales_at_count_two() {
        let config = InjectionConfig::default();
        assert_eq!(scale_budget(2048, Some(2), &config), 2560); // 2048 * 1.25
    }

    #[test]
    fn scale_budget_scales_at_count_three() {
        let config = InjectionConfig::default();
        assert_eq!(scale_budget(2048, Some(3), &config), 3072); // 2048 * 1.5
    }

    #[test]
    fn scale_budget_scales_at_count_five() {
        let config = InjectionConfig::default();
        assert_eq!(scale_budget(2048, Some(5), &config), 4096); // 2048 * 2.0
    }

    #[test]
    fn scale_budget_respects_max_cap() {
        let config = InjectionConfig {
            max_budget_cap: Some(3000),
            ..InjectionConfig::default()
        };
        assert_eq!(scale_budget(2048, Some(5), &config), 3000);
    }

    #[test]
    fn scale_budget_no_scaling_with_zero_escalation() {
        let config = InjectionConfig {
            escalation_factor: 0.0,
            ..InjectionConfig::default()
        };
        assert_eq!(scale_budget(2048, Some(5), &config), 2048);
    }

    #[test]
    fn scale_budget_no_scaling_with_negative_escalation() {
        let config = InjectionConfig {
            escalation_factor: -0.5,
            ..InjectionConfig::default()
        };
        assert_eq!(scale_budget(2048, Some(5), &config), 2048);
    }

    // Weight adjustment tests
    #[test]
    fn adjust_weights_default_at_count_zero() {
        let base = ScoringConfig::default();
        let adjusted = adjust_weights(&base, Some(0));
        assert!((adjusted.corrective_weight - 1.5).abs() < f64::EPSILON);
        assert!((adjusted.decisive_weight - 1.3).abs() < f64::EPSILON);
        assert!((adjusted.stateful_weight - 1.2).abs() < f64::EPSILON);
        assert!((adjusted.reinforcing_weight - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_weights_default_at_count_one() {
        let base = ScoringConfig::default();
        let adjusted = adjust_weights(&base, Some(1));
        assert!((adjusted.corrective_weight - 1.5).abs() < f64::EPSILON);
        assert!((adjusted.decisive_weight - 1.3).abs() < f64::EPSILON);
        assert!((adjusted.stateful_weight - 1.2).abs() < f64::EPSILON);
        assert!((adjusted.reinforcing_weight - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_weights_drift_at_count_two() {
        let base = ScoringConfig::default();
        let adjusted = adjust_weights(&base, Some(2));
        assert!((adjusted.reinforcing_weight - 1.5).abs() < f64::EPSILON);
        assert!((adjusted.corrective_weight - 1.3).abs() < f64::EPSILON);
        assert!((adjusted.stateful_weight - 1.2).abs() < f64::EPSILON);
        assert!((adjusted.decisive_weight - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_weights_heavy_drift_at_count_three() {
        let base = ScoringConfig::default();
        let adjusted = adjust_weights(&base, Some(3));
        assert!((adjusted.reinforcing_weight - 1.5).abs() < f64::EPSILON);
        assert!((adjusted.stateful_weight - 1.3).abs() < f64::EPSILON);
        assert!((adjusted.corrective_weight - 1.2).abs() < f64::EPSILON);
        assert!((adjusted.decisive_weight - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_weights_heavy_drift_at_count_ten() {
        let base = ScoringConfig::default();
        let adjusted = adjust_weights(&base, Some(10));
        assert!((adjusted.reinforcing_weight - 1.5).abs() < f64::EPSILON);
        assert!((adjusted.stateful_weight - 1.3).abs() < f64::EPSILON);
        assert!((adjusted.corrective_weight - 1.2).abs() < f64::EPSILON);
        assert!((adjusted.decisive_weight - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_weights_preserves_uncategorized() {
        let mut base = ScoringConfig::default();
        base.uncategorized_weight = 0.75;
        let adjusted = adjust_weights(&base, Some(3));
        assert!((adjusted.uncategorized_weight - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_weights_preserves_half_life() {
        let mut base = ScoringConfig::default();
        base.importance_half_life_secs = 123_456.0;
        let adjusted = adjust_weights(&base, Some(3));
        assert!((adjusted.importance_half_life_secs - 123_456.0).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_weights_none_uses_default_order() {
        let base = ScoringConfig::default();
        let adjusted = adjust_weights(&base, None);
        assert!((adjusted.corrective_weight - 1.5).abs() < f64::EPSILON);
        assert!((adjusted.decisive_weight - 1.3).abs() < f64::EPSILON);
        assert!((adjusted.stateful_weight - 1.2).abs() < f64::EPSILON);
        assert!((adjusted.reinforcing_weight - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_budget_no_scaling_at_negative_count() {
        let config = InjectionConfig::default();
        assert_eq!(scale_budget(2048, Some(-1), &config), 2048);
    }

    #[test]
    fn adjust_weights_default_at_negative_count() {
        let base = ScoringConfig::default();
        let adjusted = adjust_weights(&base, Some(-1));
        assert!((adjusted.corrective_weight - 1.5).abs() < f64::EPSILON);
        assert!((adjusted.decisive_weight - 1.3).abs() < f64::EPSILON);
        assert!((adjusted.stateful_weight - 1.2).abs() < f64::EPSILON);
        assert!((adjusted.reinforcing_weight - 1.0).abs() < f64::EPSILON);
    }
}
