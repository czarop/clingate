//! What the trait and the enum are for: a rule solves and scores itself, and
//! the stored form round-trips.

#![cfg(test)]

use super::confidence::*;
use super::rule::*;
use super::threshold::{SolveError, Status};

/// A negative with real width and a well separated positive population.
fn population(negatives: usize, positives: usize) -> Vec<f64> {
    let mut values: Vec<f64> = (0..negatives).map(|i| (i % 100) as f64 / 100.0).collect();
    values.extend(std::iter::repeat_n(8.0, positives));
    values
}

#[test]
fn a_rule_solves_and_scores_in_one_step() {
    let rule = TailFractionRule::new((0.09, 0.11));

    let solved = rule.apply(&population(9_000, 1_000), None).unwrap();

    assert_eq!(solved.threshold.status, Status::InBand);
    assert_eq!(solved.threshold.events_admitted, 1_000);
    assert!(solved.confidence.score > 0.7, "{:?}", solved.confidence);
}

#[test]
fn a_rule_passes_its_reference_through_to_the_score() {
    let rule = TailFractionRule::new((0.09, 0.11));
    let values = population(9_000, 1_000);

    let unmoved = rule.apply(&values, None).unwrap();
    let far = unmoved.threshold.x + 3.0 * unmoved.threshold.parent_spread;
    let moved = rule.apply(&values, Some(far)).unwrap();

    assert_eq!(
        unmoved.confidence.get(DISPLACEMENT),
        None,
        "no reference given"
    );
    assert_eq!(moved.confidence.weakest().unwrap().name, DISPLACEMENT);
    assert_eq!(moved.confidence.score, 0.0);
}

#[test]
fn a_rule_carries_its_own_confidence_parameters() {
    // The same data judged by two models: one that expects ten thousand events
    // in a parent, one that is content with a hundred.
    let values = population(900, 100);
    let strict = TailFractionRule::new((0.09, 0.11));
    let lenient = TailFractionRule {
        band: (0.09, 0.11),
        confidence: CountAndSeparation {
            limits: ConfidenceLimits {
                events_full: 500.0,
                ..ConfidenceLimits::default()
            },
        },
    };

    let strict_events = strict
        .apply(&values, None)
        .unwrap()
        .confidence
        .get(EVENTS)
        .unwrap()
        .score;
    let lenient_events = lenient
        .apply(&values, None)
        .unwrap()
        .confidence
        .get(EVENTS)
        .unwrap()
        .score;

    assert!(strict_events < 1.0);
    assert_eq!(
        lenient_events, 1.0,
        "a thousand events clears a limit of 500"
    );
}

#[test]
fn a_percentile_rule_solves_through_the_same_trait() {
    let rule = PercentileOffsetRule::new(99.0, 0.5);
    let values: Vec<f64> = (0..=100).map(|i| i as f64).collect();

    let solved = rule.apply(&values, None).unwrap();

    assert_eq!(solved.threshold.x, 99.5);
    assert_eq!(solved.threshold.status, Status::NoBand);
    assert_eq!(
        solved.confidence.get(BAND).unwrap().score,
        1.0,
        "a rule with no band is not penalised for missing one"
    );
}

#[test]
fn a_rule_reports_an_empty_population_rather_than_scoring_one() {
    let rule = TailFractionRule::new((0.002, 0.005));
    assert_eq!(rule.apply(&[], None), Err(SolveError::NoEvents));
}

// ─── the stored form ──────────────────────────────────────────────────────────

#[test]
fn the_enum_dispatches_to_the_variant() {
    let values = population(9_000, 1_000);
    let direct = TailFractionRule::new((0.09, 0.11))
        .apply(&values, None)
        .unwrap();
    let through_enum = Rule::TailFraction(TailFractionRule::new((0.09, 0.11)))
        .apply(&values, None)
        .unwrap();

    assert_eq!(direct, through_enum);
}

#[test]
fn a_rule_round_trips_through_the_sidecar() {
    let rule = Rule::TailFraction(TailFractionRule::new((0.002, 0.005)));

    let json = serde_json::to_string(&rule).unwrap();
    let back: Rule = serde_json::from_str(&json).unwrap();

    assert_eq!(rule, back);
}

#[test]
fn a_percentile_rule_round_trips_too() {
    let rule = Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.3));

    let back: Rule = serde_json::from_str(&serde_json::to_string(&rule).unwrap()).unwrap();

    assert_eq!(rule, back);
}

/// The variants are tagged by name, so a stored rule stays readable and a new
/// variant cannot silently deserialise as an old one.
#[test]
fn the_stored_form_names_its_kind() {
    let json =
        serde_json::to_string(&Rule::TailFraction(TailFractionRule::new((0.002, 0.005)))).unwrap();

    assert!(json.contains("\"kind\":\"TailFraction\""), "{json}");
}

/// Confidence limits are stored with the rule, so a tuned rule stays tuned.
#[test]
fn tuned_confidence_limits_survive_the_round_trip() {
    let rule = Rule::TailFraction(TailFractionRule {
        band: (0.002, 0.005),
        confidence: CountAndSeparation {
            limits: ConfidenceLimits {
                events_floor: 42.0,
                ..ConfidenceLimits::default()
            },
        },
    });

    let back: Rule = serde_json::from_str(&serde_json::to_string(&rule).unwrap()).unwrap();

    let Rule::TailFraction(back) = back else {
        panic!("expected a tail fraction rule");
    };
    assert_eq!(back.confidence.limits.events_floor, 42.0);
}

#[test]
fn a_rule_describes_itself_for_the_report() {
    assert_eq!(
        Rule::TailFraction(TailFractionRule::new((0.002, 0.005))).describe(),
        "capture 0.200% to 0.500% of the parent population"
    );
    assert_eq!(
        Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.5)).describe(),
        "sit above the 99th percentile of the negative by 0.5"
    );
    assert_eq!(
        Rule::PercentileOffset(PercentileOffsetRule::new(99.0, -0.5)).describe(),
        "sit below the 99th percentile of the negative by 0.5"
    );
}

#[test]
fn a_rule_names_its_kind_for_the_tab() {
    assert_eq!(
        Rule::TailFraction(TailFractionRule::new((0.0, 1.0))).kind(),
        "Tail fraction"
    );
    assert_eq!(
        Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.0)).kind(),
        "Percentile offset"
    );
}
