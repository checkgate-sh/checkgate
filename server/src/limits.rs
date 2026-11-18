//! Size limits on flag and segment configuration.
//!
//! The 64 KiB request-body cap bounds what one write stores, but not what the
//! server and every SDK *derive* from it:
//!
//! - **Segment expansion** copies a segment's rules into every flag rule that
//!   references it (`segments::expand_flag_with_segments`). Without a bound, a
//!   flag that references one 60 KB segment 3 000 times expands to ~180 MB —
//!   rebuilt for each publish, each SSE bootstrap and each `/flags/snapshot`.
//! - **Prerequisites** are evaluated recursively by every SDK. Duplicates and
//!   a large fan-out multiply the work per level (see `MAX_PREREQUISITE_EVALUATIONS`
//!   in `checkgate-core`, which bounds the damage of configurations saved
//!   before these limits existed).
//!
//! The limits are far above any configuration we have seen in practice; they
//! exist to turn "one editor can exhaust memory on every instance" into a 422.

use checkgate_core::evaluator::{Flag, TargetingRule};
use std::collections::HashSet;

/// Rules on one flag, counting segment references (before expansion).
pub(crate) const MAX_RULES_PER_FLAG: usize = 500;
/// Distinct segments one flag may reference. With each segment's own body
/// capped at 64 KiB, this bounds an expanded flag at a few MB.
pub(crate) const MAX_SEGMENT_REFERENCES_PER_FLAG: usize = 50;
/// Rules in one segment.
pub(crate) const MAX_RULES_PER_SEGMENT: usize = 500;
/// Prerequisites on one flag.
pub(crate) const MAX_PREREQUISITES_PER_FLAG: usize = 50;
/// Weighted variants on one flag.
pub(crate) const MAX_VARIANTS_PER_FLAG: usize = 100;

/// Checks a flag as it will be stored. Returns a human-readable reason on
/// rejection (logged; the API answers 422).
pub(crate) fn check_flag(flag: &Flag) -> Result<(), String> {
    if flag.rules.len() > MAX_RULES_PER_FLAG {
        return Err(format!(
            "{} rules (limit {MAX_RULES_PER_FLAG})",
            flag.rules.len()
        ));
    }

    let mut segments = HashSet::new();
    for key in flag.rules.iter().filter_map(|r| r.segment_key.as_deref()) {
        if !segments.insert(key) {
            return Err(format!("segment {key:?} is referenced more than once"));
        }
    }
    if segments.len() > MAX_SEGMENT_REFERENCES_PER_FLAG {
        return Err(format!(
            "{} segment references (limit {MAX_SEGMENT_REFERENCES_PER_FLAG})",
            segments.len()
        ));
    }

    if flag.prerequisites.len() > MAX_PREREQUISITES_PER_FLAG {
        return Err(format!(
            "{} prerequisites (limit {MAX_PREREQUISITES_PER_FLAG})",
            flag.prerequisites.len()
        ));
    }
    let mut prerequisites = HashSet::new();
    for p in &flag.prerequisites {
        if p.flag_key == flag.key {
            return Err("a flag cannot be its own prerequisite".into());
        }
        if !prerequisites.insert(p.flag_key.as_str()) {
            return Err(format!(
                "prerequisite {:?} is listed more than once",
                p.flag_key
            ));
        }
    }

    if flag.variants.len() > MAX_VARIANTS_PER_FLAG {
        return Err(format!(
            "{} variants (limit {MAX_VARIANTS_PER_FLAG})",
            flag.variants.len()
        ));
    }
    Ok(())
}

/// Checks a segment's rule list as it will be stored.
pub(crate) fn check_segment_rules(rules: &[TargetingRule]) -> Result<(), String> {
    if rules.len() > MAX_RULES_PER_SEGMENT {
        return Err(format!(
            "{} rules (limit {MAX_RULES_PER_SEGMENT})",
            rules.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use checkgate_core::evaluator::{FlagType, Operator, Prerequisite, WeightedVariant};

    fn flag() -> Flag {
        Flag {
            key: "f".into(),
            is_enabled: true,
            rollout_percentage: None,
            description: None,
            rules: vec![],
            flag_type: FlagType::Boolean,
            default_value: None,
            disabled_value: None,
            variants: vec![],
            prerequisites: vec![],
        }
    }

    fn rule(attribute: &str) -> TargetingRule {
        TargetingRule {
            attribute: attribute.into(),
            operator: Operator::Equals,
            values: vec!["x".into()],
            segment_key: None,
            variant: None,
        }
    }

    fn segment_ref(key: &str) -> TargetingRule {
        TargetingRule {
            segment_key: Some(key.into()),
            ..rule("")
        }
    }

    fn prereq(key: &str) -> Prerequisite {
        Prerequisite {
            flag_key: key.into(),
            required_value: None,
        }
    }

    #[test]
    fn ordinary_flags_pass() {
        let mut f = flag();
        f.rules = vec![rule("plan"), segment_ref("beta"), segment_ref("staff")];
        f.prerequisites = vec![prereq("a"), prereq("b")];
        assert_eq!(check_flag(&f), Ok(()));
    }

    #[test]
    fn rule_count_is_capped_at_the_limit() {
        let mut f = flag();
        f.rules = (0..MAX_RULES_PER_FLAG).map(|_| rule("plan")).collect();
        assert_eq!(check_flag(&f), Ok(()));
        f.rules.push(rule("plan"));
        assert!(check_flag(&f).is_err());
    }

    #[test]
    fn duplicate_segment_references_are_rejected() {
        // The amplification vector: one segment copied once per reference.
        let mut f = flag();
        f.rules = vec![segment_ref("s"), rule("plan"), segment_ref("s")];
        assert!(check_flag(&f).unwrap_err().contains("more than once"));
    }

    #[test]
    fn distinct_segment_references_are_capped() {
        let mut f = flag();
        f.rules = (0..MAX_SEGMENT_REFERENCES_PER_FLAG)
            .map(|i| segment_ref(&format!("s{i}")))
            .collect();
        assert_eq!(check_flag(&f), Ok(()));
        f.rules.push(segment_ref("one-more"));
        assert!(check_flag(&f).is_err());
    }

    #[test]
    fn prerequisites_reject_self_duplicates_and_excess() {
        let mut f = flag();
        f.prerequisites = vec![prereq("f")];
        assert!(check_flag(&f).unwrap_err().contains("own prerequisite"));

        f.prerequisites = vec![prereq("a"), prereq("a")];
        assert!(check_flag(&f).unwrap_err().contains("more than once"));

        f.prerequisites = (0..=MAX_PREREQUISITES_PER_FLAG)
            .map(|i| prereq(&format!("p{i}")))
            .collect();
        assert!(check_flag(&f).is_err());
    }

    #[test]
    fn variants_are_capped() {
        let mut f = flag();
        f.variants = (0..=MAX_VARIANTS_PER_FLAG)
            .map(|_| WeightedVariant {
                weight: 1,
                value: checkgate_core::evaluator::FlagValue::Bool(true),
            })
            .collect();
        assert!(check_flag(&f).is_err());
    }

    #[test]
    fn segment_rules_are_capped() {
        let rules: Vec<_> = (0..MAX_RULES_PER_SEGMENT).map(|_| rule("plan")).collect();
        assert_eq!(check_segment_rules(&rules), Ok(()));
        let mut over = rules;
        over.push(rule("plan"));
        assert!(check_segment_rules(&over).is_err());
    }
}
