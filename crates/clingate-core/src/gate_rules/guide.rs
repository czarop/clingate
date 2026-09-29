//! A guide to each rule, for a person or Claude choosing one for a gate or
//! working out why one misplaced a gate: what it is for, what data it suits,
//! how it works step by step, every setting, its traps, and what its
//! confidence says.
//!
//! The text lives in `docs/rules`, compiled in so that what is read is what
//! was built, and tested against the code below: every setting a rule has is
//! named in its guide, and the numbers the guides quote are the code's.

/// How to choose between the rules, and the settings they share.
pub const CHOOSING: &str = include_str!("../../../../docs/rules/choosing.md");

/// One rule's guide.
pub struct Guide {
    /// The rule's kind, as the rules file writes it.
    pub key: &'static str,
    /// Its name in the Gate Rules tab.
    pub name: &'static str,
    /// In a line.
    pub summary: &'static str,
    pub text: &'static str,
}

pub const GUIDES: &[Guide] = &[
    Guide {
        key: "TailFraction",
        name: "Tail fraction",
        summary: "slide the gate until it holds a percentage of the file it reads, within a band \
                  - usually the FMX, for smears with no dip",
        text: include_str!("../../../../docs/rules/tail-fraction.md"),
    },
    Guide {
        key: "PercentileOffset",
        name: "Percentile offset",
        summary: "a fixed step above (or below) a percentile of the file it reads - the top of \
                  a negative",
        text: include_str!("../../../../docs/rules/percentile-offset.md"),
    },
    Guide {
        key: "AboveTheNegative",
        name: "Above the negative",
        summary: "as many negative-widths above each sample's negative as on a hand-gated \
                  reference - smears with no FMX",
        text: include_str!("../../../../docs/rules/above-the-negative.md"),
    },
    Guide {
        key: "InTheValley",
        name: "In the valley",
        summary: "in the dip between negative and positive, as far from its bottom as on the \
                  reference - separate populations",
        text: include_str!("../../../../docs/rules/in-the-valley.md"),
    },
    Guide {
        key: "MatchThePhenotype",
        name: "Match the phenotype",
        summary: "find the cells that look like the reference gate's across chosen markers and \
                  fit the gate to them - populations no line separates",
        text: include_str!("../../../../docs/rules/match-the-phenotype.md"),
    },
    Guide {
        key: "FromAnotherGate",
        name: "From another gate",
        summary: "take another gate's shape, or set an edge against another gate's edge, on the \
                  same sample - gates a guide places by other gates",
        text: include_str!("../../../../docs/rules/from-another-gate.md"),
    },
];

/// A rule's guide by its kind or its name, however written: "TailFraction",
/// "tail fraction", "tail-fraction".
pub fn find(asked: &str) -> Option<&'static Guide> {
    let squash = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let wanted = squash(asked);
    GUIDES
        .iter()
        .find(|g| squash(g.key) == wanted || squash(g.name) == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate_rules::rule::{
        AboveTheNegativeRule, BandAim, EdgeFrom, FromGateRule, NegativeFinder,
        PercentileOffsetRule, PhenotypeRule, Rule, ShapeFit, Side, TailFractionRule, ValleyRule,
    };

    fn every_rule() -> Vec<Rule> {
        vec![
            Rule::TailFraction(TailFractionRule::new((0.002, 0.005))),
            Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.3)),
            Rule::AboveTheNegative(AboveTheNegativeRule::default()),
            Rule::InTheValley(ValleyRule::default()),
            Rule::MatchThePhenotype(PhenotypeRule::default()),
            Rule::FromAnotherGate(FromGateRule {
                same_shape_as: Some(crate::gate_rules::rule_store::RuleTarget::named("CD4+")),
                edges: vec![EdgeFrom {
                    anchor: crate::gate_rules::rule_store::RuleTarget::named("CD19+"),
                    parameter: "CD19".into(),
                    side: Side::Upper,
                    anchor_side: Side::Lower,
                    gap: 0.0,
                }],
            }),
        ]
    }

    #[test]
    fn every_rule_has_a_guide_under_the_name_the_rules_file_gives_it() {
        let rules = every_rule();
        assert_eq!(rules.len(), GUIDES.len());
        for rule in rules {
            let json = serde_json::to_value(&rule).unwrap();
            let kind = json["kind"].as_str().unwrap();
            let guide = find(kind).unwrap_or_else(|| panic!("no guide for {kind}"));
            assert_eq!(guide.key, kind);
            assert_eq!(guide.name, rule.kind(), "the tab's name for it");
            assert!(guide.text.starts_with(&format!("# {}", guide.name)));
            // Named in the guide to choosing, too.
            assert!(
                CHOOSING.contains(&format!("**{}**", guide.name)),
                "{}",
                guide.name
            );
        }
    }

    #[test]
    fn every_setting_a_rule_has_is_explained_in_its_guide() {
        for rule in every_rule() {
            let json = serde_json::to_value(&rule).unwrap();
            let guide = find(json["kind"].as_str().unwrap()).unwrap();
            for field in json.as_object().unwrap().keys() {
                if field == "kind" {
                    continue;
                }
                assert!(
                    guide.text.contains(&format!("`{field}`")),
                    "{}: the setting `{field}` is not explained",
                    guide.name
                );
            }
        }
        // And every choice a setting offers.
        let text = |kind| find(kind).unwrap().text;
        for aim in BandAim::ALL {
            assert!(
                text("TailFraction").contains(&format!("`{}`", aim.key())),
                "{}",
                aim.key()
            );
        }
        for finder in NegativeFinder::ALL {
            assert!(text("AboveTheNegative").contains(&format!("`{}`", finder.key())));
        }
        for fit in ShapeFit::ALL {
            assert!(text("MatchThePhenotype").contains(&format!("`{}`", fit.key())));
        }
        // An edge's own settings, and both sides, which the check on a rule's
        // top-level settings does not reach.
        let edge = serde_json::to_value(EdgeFrom {
            anchor: crate::gate_rules::rule_store::RuleTarget::named("CD19+"),
            parameter: "CD19".into(),
            side: Side::Upper,
            anchor_side: Side::Lower,
            gap: 0.0,
        })
        .unwrap();
        for field in edge.as_object().unwrap().keys() {
            assert!(
                text("FromAnotherGate").contains(&format!("`{field}`")),
                "the edge setting `{field}` is not explained"
            );
        }
        for side in ["Lower", "Upper"] {
            assert!(text("FromAnotherGate").contains(&format!("`{side}`")));
        }
        // The settings every rule shares.
        for shared in [
            "`parameter`",
            "`bound`",
            "`measured_on`",
            "`confidence.limits`",
        ] {
            assert!(CHOOSING.contains(shared), "{shared}");
        }
    }

    #[test]
    fn the_numbers_the_guides_quote_are_the_code_s() {
        use crate::gate_rules::confidence::ConfidenceLimits;
        let limits = ConfidenceLimits::default();
        assert_eq!(
            (
                limits.events_full,
                limits.events_floor,
                limits.swing_half,
                limits.displacement_limit
            ),
            (10_000.0, 100.0, 1.0, 0.5)
        );
        assert!(CHOOSING.contains("`events_full` (10,000"));
        assert!(CHOOSING.contains("`events_floor` (100"));
        assert!(CHOOSING.contains("`swing_half` (1"));
        assert!(CHOOSING.contains("`displacement_limit` (0.5"));

        let text = |kind| find(kind).unwrap().text;
        // The right-side check reads down to a quarter of the peak.
        assert_eq!(crate::gate_rules::threshold::SIDE_HEIGHT, 0.25);
        assert!(text("AboveTheNegative").contains("down to a quarter of its height"));
        // The phenotype rule's defaults and limits.
        let phenotype = PhenotypeRule::default();
        assert_eq!((phenotype.keep, phenotype.vertices), (0.95, 24));
        assert!(text("MatchThePhenotype").contains("(0.95 by\n  default)"));
        assert!(text("MatchThePhenotype").contains("(24 by default)"));
        assert_eq!(crate::gate_rules::phenotype::TRIM, 4.0);
        assert!(text("MatchThePhenotype").contains("more than 4\n   spreads out"));
        assert_eq!(crate::gate_rules::shape_fit::MAX_STRETCH, 4.0);
        assert!(text("MatchThePhenotype").contains("at most 4 times"));
        assert_eq!(crate::gate_rules::phenotype::KEEP, 0.95);
        // Above the negative's defaults.
        let above = AboveTheNegativeRule::default();
        assert_eq!(
            (above.scale, above.nudge, above.find),
            (1.0, 0.0, NegativeFinder::BelowTheGate)
        );
        assert!(text("AboveTheNegative").contains("`BelowTheGate` (the default)"));
        assert_eq!(ValleyRule::default().smoothing, 1.0);
        assert_eq!(
            TailFractionRule::new((0.1, 0.2)).aim,
            BandAim::AnywhereInBand
        );
        assert!(text("TailFraction").contains("`AnywhereInBand` (the default)"));
    }

    #[test]
    fn a_guide_is_found_however_its_name_is_written() {
        for asked in [
            "TailFraction",
            "tail fraction",
            "Tail-Fraction",
            " tail_fraction ",
        ] {
            assert_eq!(find(asked).map(|g| g.key), Some("TailFraction"), "{asked}");
        }
        assert_eq!(
            find("above the negative").map(|g| g.key),
            Some("AboveTheNegative")
        );
        assert!(find("wobble").is_none());
        assert!(find("").is_none());
    }
}
