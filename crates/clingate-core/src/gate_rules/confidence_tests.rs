//! The score exists to rank gates for review, so what is tested is the ranking
//! and the reasons - not the constants, which are not earned yet.

#![cfg(test)]

use super::confidence::*;
use super::threshold::*;

fn assess(t: &Threshold) -> Confidence {
    CountAndSeparation::default().assess(t)
}

fn part(c: &Confidence, name: &str) -> f64 {
    c.get(name)
        .unwrap_or_else(|| panic!("{name} should be scored"))
        .score
}

/// A clean case: plenty of events, a clear gap, rule satisfied.
fn clean() -> Vec<f64> {
    let mut values = vec![0.0; 9_000];
    // A negative with real width, so the interquartile spread is not zero.
    for (i, v) in values.iter_mut().enumerate() {
        *v = (i % 100) as f64 / 100.0;
    }
    // A well separated positive population.
    values.extend(std::iter::repeat_n(8.0, 1_000));
    values
}

#[test]
fn a_clean_placement_scores_well() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let c = assess(&t);

    assert_eq!(t.status, Status::InBand);
    assert!(c.score > 0.7, "score was {} ({:?})", c.score, c);
}

#[test]
fn a_thin_parent_population_drags_the_score_down() {
    // The same shape, a hundredth of the events.
    let mut values = vec![0.0; 90];
    for (i, v) in values.iter_mut().enumerate() {
        *v = (i % 100) as f64 / 100.0;
    }
    values.extend(std::iter::repeat_n(8.0, 10));

    let t = tail_fraction(&values, (0.09, 0.11)).unwrap();
    let c = assess(&t);

    assert!(
        part(&c, EVENTS) < 0.5,
        "events component was {}",
        part(&c, EVENTS)
    );
    assert!(c.score < 0.5);
}

/// The component the parent count misses: a comfortable parent can still give
/// an answer that rests on a handful of cells.
#[test]
fn a_handful_of_events_in_the_gate_is_flagged_despite_a_large_parent() {
    let mut values: Vec<f64> = (0..10_000).map(|i| (i % 100) as f64 / 100.0).collect();
    values.extend([8.0, 8.1, 8.2, 8.3]);

    let t = tail_fraction(&values, (0.0002, 0.0005)).unwrap();
    let c = assess(&t);

    assert_eq!(part(&c, EVENTS), 1.0, "the parent population is ample");
    assert!(
        part(&c, ADMITTED) < 0.6,
        "but only {} events are in the gate, giving {}",
        t.events_admitted,
        part(&c, ADMITTED)
    );
    assert_eq!(c.weakest().unwrap().name, ADMITTED);
}

#[test]
fn an_edge_buried_in_the_population_scores_worse_than_one_in_a_gap() {
    // No positive population at all: the rule has to put the edge inside the
    // negative, where a small nudge changes what the gate holds.
    let buried: Vec<f64> = (0..10_000).map(|i| (i % 1000) as f64 / 1000.0).collect();

    let in_gap = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let in_cloud = tail_fraction(&buried, (0.09, 0.11)).unwrap();

    let limits = ConfidenceLimits::default();
    let gap_score = CountAndSeparation {
        limits: limits.clone(),
    }
    .assess(&in_gap);
    let cloud_score = CountAndSeparation {
        limits: limits.clone(),
    }
    .assess(&in_cloud);

    assert!(
        part(&cloud_score, STABILITY) < part(&gap_score, STABILITY),
        "buried {} should score below separated {}",
        part(&cloud_score, STABILITY),
        part(&gap_score, STABILITY)
    );
}

/// The measure this replaced. In a continuum the gap between the two events an
/// edge separates is about the spread over the event count, so it can never
/// approach 1 - on 50,000 real events it came out at 0.0003 to 0.006 of the
/// interquartile width on every fluorescence channel, ranking nothing.
#[test]
fn stability_survives_the_event_counts_a_real_gate_sees() {
    let dense: Vec<f64> = (0..10_000).map(|i| i as f64 / 10_000.0).collect();
    let t = tail_fraction(&dense, (0.002, 0.005)).unwrap();

    let gap = 1.0 / 10_000.0;
    assert!(
        gap / t.parent_spread < 0.001,
        "the gap measure would have scored ~{:.5}",
        gap / t.parent_spread
    );
    let stability = part(&assess(&t), STABILITY);
    assert!(
        (0.01..0.99).contains(&stability),
        "stability has to land somewhere rankable, got {stability}"
    );
}

#[test]
fn a_rule_that_could_not_be_satisfied_scores_below_one() {
    // 100 events, 0.2%-0.5%: no whole number of events lands in the band.
    let values: Vec<f64> = (0..100).map(|i| i as f64).collect();

    let t = tail_fraction(&values, (0.002, 0.005)).unwrap();
    let c = assess(&t);

    assert_eq!(
        t.status,
        Status::OutOfBand {
            band: (0.002, 0.005)
        }
    );
    assert!(
        part(&c, BAND) < 1.0,
        "band component was {}",
        part(&c, BAND)
    );
}

#[test]
fn a_satisfied_rule_is_not_penalised_for_its_band() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let c = assess(&t);

    assert_eq!(part(&c, BAND), 1.0);
}

// ─── the combination rule ─────────────────────────────────────────────────────

/// A gate that is merely middling on several counts must not be scored as
/// though it were bad. A thousand events and twenty-five in the gate is an
/// ordinary, usable placement; multiplying the components would drag it below
/// either concern taken on its own.
#[test]
fn the_score_is_the_weakest_component_not_the_product() {
    let mut values: Vec<f64> = (0..975).map(|i| (i % 100) as f64 / 100.0).collect();
    values.extend(std::iter::repeat_n(8.0, 25));

    let t = tail_fraction(&values, (0.02, 0.03)).unwrap();
    let c = assess(&t);

    let weakest = part(&c, EVENTS)
        .min(part(&c, ADMITTED))
        .min(part(&c, STABILITY))
        .min(part(&c, BAND));
    assert_eq!(c.score, weakest);

    let product = part(&c, EVENTS) * part(&c, ADMITTED) * part(&c, STABILITY) * part(&c, BAND);
    assert!(
        c.score > product,
        "weakest {} should beat the product {product} ({c:?})",
        c.score
    );
    assert!(
        part(&c, EVENTS) < 1.0 && part(&c, ADMITTED) < 1.0,
        "the case has to have two middling components to be worth anything"
    );
}

#[test]
fn one_bad_component_is_not_rescued_by_the_others() {
    let mut values: Vec<f64> = (0..10_000).map(|i| (i % 100) as f64 / 100.0).collect();
    values.push(8.0);

    // Ample parent, clean gap, satisfiable band - but a single event in the gate.
    let t = tail_fraction(&values, (0.00005, 0.00015)).unwrap();
    let c = assess(&t);

    assert_eq!(t.events_admitted, 1);
    assert_eq!(c.score, 0.0, "one event cannot support a placement: {c:?}");
}

// ─── which side of the edge carries the noise ────────────────────────────────

/// A threshold holding `admitted` of `parent`, otherwise unremarkable.
fn holding(admitted: usize, parent: usize) -> Threshold {
    Threshold {
        x: 1.0,
        events_admitted: admitted,
        fraction_admitted: admitted as f64 / parent as f64,
        parent_events: parent,
        count_swing: 0.05,
        parent_spread: 2.0,
        status: Status::InBand,
    }
}

#[test]
fn a_gate_keeping_almost_everything_is_scored_on_what_it_excludes() {
    // A negative gate asked to hold 99.7% of its parent admits nearly all of
    // it. Scoring on that count called the placement certain, when where the
    // edge sits is decided entirely by the hundred-odd events it cuts.
    use crate::gate_rules::confidence::ADMITTED;
    let model = CountAndSeparation::default();

    let keeps_most = model.assess(&holding(39_880, 40_000));
    let keeps_few = model.assess(&holding(120, 40_000));

    let a = keeps_most.get(ADMITTED).unwrap().score;
    let b = keeps_few.get(ADMITTED).unwrap().score;
    assert!(
        (a - b).abs() < 1e-9,
        "an edge is equally uncertain whichever side is scarce: {a} against {b}"
    );
    assert!(a < 0.92, "120 events is not a certain answer: {a}");
}

#[test]
fn the_detail_names_both_sides() {
    use crate::gate_rules::confidence::ADMITTED;
    let model = CountAndSeparation::default();
    let detail = model
        .assess(&holding(39_880, 40_000))
        .get(ADMITTED)
        .unwrap()
        .detail
        .clone();
    assert!(detail.contains("39880"), "{detail}");
    assert!(detail.contains("120 excluded"), "{detail}");
}

#[test]
fn a_gate_that_excludes_nothing_is_not_a_placement() {
    // Its edge sits off the end of the data - nothing constrains where.
    use crate::gate_rules::confidence::ADMITTED;
    let model = CountAndSeparation::default();
    let all = model.assess(&holding(40_000, 40_000));
    assert_eq!(all.get(ADMITTED).unwrap().score, 0.0);
}

#[test]
fn a_comfortable_split_still_scores_well() {
    use crate::gate_rules::confidence::ADMITTED;
    let model = CountAndSeparation::default();
    let even = model.assess(&holding(20_000, 40_000));
    assert!(even.get(ADMITTED).unwrap().score > 0.99);
}

// ─── An unmeasurable component ───────────────────────────────────────────────

/// Was B-CONF-1: `Component::new` clamped its score, and clamping NaN gives
/// NaN; `from_components` then folded with `f64::min`, which returns the
/// other operand when one is NaN. A component that could not be measured was
/// dropped from the overall instead of dragging it down, so a gate was ranked
/// as trustworthy for exactly the reason it should be looked at.
#[test]
fn a_component_that_could_not_be_measured_counts_against_the_gate() {
    let c = Confidence::from_components(vec![
        Component::new("fine", 0.9, ""),
        Component::new("unmeasured", f64::NAN, "0 of 0 events"),
    ]);
    assert_eq!(c.score, 0.0, "overall {}", c.score);
    let weakest = c.weakest().unwrap();
    assert_eq!(weakest.name, "unmeasured");
    assert_eq!(weakest.detail, "could not be measured: 0 of 0 events");
}

/// The fields are public, so a NaN can still be put in by hand; the overall
/// counts it as 0 rather than passing over it.
#[test]
fn a_nan_put_straight_into_a_component_still_counts_against_the_gate() {
    let mut unmeasured = Component::new("unmeasured", 0.5, "");
    unmeasured.score = f64::NAN;
    let c = Confidence::from_components(vec![Component::new("fine", 0.9, ""), unmeasured]);
    assert_eq!(c.score, 0.0);
}

#[test]
fn a_measured_component_keeps_its_own_detail() {
    let c = Component::new("fine", 0.9, "the gate captures a fraction inside the band");
    assert_eq!(c.detail, "the gate captures a fraction inside the band");
    assert_eq!(
        Component::new("empty", f64::NAN, "").detail,
        "could not be measured"
    );
}

#[test]
fn scores_are_clamped_into_zero_to_one() {
    assert_eq!(Component::new("x", 1.7, "").score, 1.0);
    assert_eq!(Component::new("x", -0.3, "").score, 0.0);
}

#[test]
fn nothing_to_judge_is_full_confidence_with_nothing_weakest() {
    let c = Confidence::from_components(Vec::new());
    assert_eq!(c.score, 1.0);
    assert!(c.weakest().is_none());
    assert!(c.get(EVENTS).is_none());
}

#[test]
fn a_zero_swing_scale_falls_back_rather_than_dividing_by_zero() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let model = CountAndSeparation {
        limits: ConfidenceLimits {
            swing_half: 0.0,
            ..ConfidenceLimits::default()
        },
    };
    let s = part(&model.assess(&t), STABILITY);
    assert!(s.is_finite() && s > 0.0, "{s}");
}

#[test]
fn limits_survive_the_rules_file() {
    let limits = ConfidenceLimits {
        events_full: 5_000.0,
        events_floor: 50.0,
        swing_half: 0.5,
    };
    let text = serde_json::to_string(&CountAndSeparation {
        limits: limits.clone(),
    })
    .unwrap();
    let back: CountAndSeparation = serde_json::from_str(&text).unwrap();
    assert_eq!(back.limits, limits);
}

#[test]
fn a_placement_is_not_scored_on_how_far_it_moved() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let names: Vec<&str> = assess(&t).components.iter().map(|c| c.name).collect();
    assert_eq!(names, [EVENTS, ADMITTED, STABILITY, BAND]);
}

// ─── The phenotype model ─────────────────────────────────────────────────────

fn evidence() -> MatchEvidence {
    MatchEvidence {
        matched: 2_000,
        parent: 20_000,
        purity: 0.95,
        caught: 0.9,
        pieces: 1,
        steady: Some(0.97),
    }
}

#[test]
fn a_clean_match_scores_well_on_every_count() {
    let c = assess_match(evidence());
    for name in [MATCHED, PURITY, CAUGHT, ONE_CLOUD, STEADY] {
        assert!(part(&c, name) > 0.8, "{name}: {}", part(&c, name));
    }
    assert!(c.score > 0.8, "{c:?}");
}

/// How common the population is, against the reference, is not scored:
/// donors differ that much in data the rule is right on.
#[test]
fn a_match_is_not_scored_on_how_common_its_population_is() {
    let c = assess_match(evidence());
    let names: Vec<&str> = c.components.iter().map(|c| c.name).collect();
    assert_eq!(names, [MATCHED, PURITY, CAUGHT, ONE_CLOUD, STEADY]);
    // A hundredth as common scores the same, save for the count itself.
    let rare = assess_match(MatchEvidence {
        matched: 1_000,
        parent: 1_000_000,
        ..evidence()
    });
    assert_eq!(rare.score, part(&rare, MATCHED).min(c.score));
}

#[test]
fn edges_that_disagree_between_halves_hold_the_score_down() {
    let c = assess_match(MatchEvidence {
        steady: Some(0.35),
        ..evidence()
    });
    assert_eq!(part(&c, STEADY), 0.35);
    assert_eq!(c.weakest().unwrap().name, STEADY);
    assert_eq!(c.score, 0.35);
}

#[test]
fn a_traced_polygon_is_not_scored_on_its_edges() {
    let c = assess_match(MatchEvidence {
        steady: None,
        ..evidence()
    });
    assert!(c.get(STEADY).is_none(), "{c:?}");
}

#[test]
fn purity_and_catch_are_their_own_scores() {
    let c = assess_match(MatchEvidence {
        purity: 0.4,
        caught: 0.6,
        ..evidence()
    });
    assert_eq!(part(&c, PURITY), 0.4);
    assert_eq!(part(&c, CAUGHT), 0.6);
    assert_eq!(c.weakest().unwrap().name, PURITY);
}

#[test]
fn an_unmeasured_purity_or_catch_scores_nothing() {
    let c = assess_match(MatchEvidence {
        purity: f64::NAN,
        caught: f64::INFINITY,
        ..evidence()
    });
    assert_eq!(part(&c, PURITY), 0.0);
    assert_eq!(part(&c, CAUGHT), 0.0);
    assert_eq!(c.score, 0.0);
}

#[test]
fn several_clouds_divide_the_one_cloud_score() {
    let score = |pieces| {
        part(
            &assess_match(MatchEvidence {
                pieces,
                ..evidence()
            }),
            ONE_CLOUD,
        )
    };
    assert_eq!(score(0), 0.0, "nothing matched is no cloud at all");
    assert_eq!(score(1), 1.0);
    assert_eq!(score(2), 0.5);
    assert_eq!(score(4), 0.25);
}

// ─── the negative's right side against the reference ────────────────────────

fn sides(
    peak: f64,
    left: Option<f64>,
    right: Option<f64>,
) -> crate::gate_rules::threshold::PeakSides {
    crate::gate_rules::threshold::PeakSides { peak, left, right }
}

#[test]
fn a_gate_as_far_out_by_the_right_side_as_on_the_reference_scores_full() {
    use crate::gate_rules::confidence::{RIGHT_SIDE, right_side};
    // Reference: gate 3 right-side widths above its peak.
    let reference = sides(0.0, Some(1.0), Some(1.0));
    let same = right_side(3.0, reference, 13.0, sides(10.0, Some(2.0), Some(1.0))).unwrap();
    assert_eq!(same.name, RIGHT_SIDE);
    assert_eq!(same.score, 1.0);
    assert!(same.detail.contains("100% as far out"), "{}", same.detail);
    assert!(
        same.detail.contains("right side 0.50 times the left here"),
        "{}",
        same.detail
    );
    // A quarter further out is still fine.
    assert_eq!(
        right_side(3.0, reference, 3.75, sides(0.0, None, Some(1.0)))
            .unwrap()
            .score,
        1.0
    );
}

#[test]
fn a_gate_further_out_than_the_right_side_warrants_scores_down_to_nothing() {
    use crate::gate_rules::confidence::right_side;
    let reference = sides(0.0, Some(1.0), Some(1.0));
    let at = |k: f64| {
        right_side(3.0, reference, k * 3.0, sides(0.0, Some(1.0), Some(1.0)))
            .unwrap()
            .score
    };
    assert!((at(1.5) - (1.0 - 0.25 / 0.75)).abs() < 1e-12);
    assert_eq!(at(2.0), 0.0);
    assert_eq!(at(5.0), 0.0);
}

#[test]
fn a_gate_closer_in_than_on_the_reference_never_scores_below_a_half() {
    use crate::gate_rules::confidence::right_side;
    let reference = sides(0.0, Some(1.0), Some(1.0));
    let at = |k: f64| {
        right_side(3.0, reference, k * 3.0, sides(0.0, Some(1.0), Some(1.0)))
            .unwrap()
            .score
    };
    assert!((at(0.7) - 1.0).abs() < 1e-9);
    assert!((at(0.55) - 0.75).abs() < 1e-12);
    assert!((at(0.4) - 0.5).abs() < 1e-9);
    assert_eq!(at(0.0), 0.5);
}

#[test]
fn a_right_side_that_never_ends_is_said_and_nothing_is_compared_without_a_reference() {
    use crate::gate_rules::confidence::right_side;
    let reference = sides(0.0, Some(1.0), Some(1.0));
    let merged = right_side(3.0, reference, 3.0, sides(0.0, Some(1.0), None)).unwrap();
    assert_eq!(merged.score, 0.5);
    assert!(merged.detail.contains("never falls"), "{}", merged.detail);
    // The reference's own right side unreadable, or its gate at its peak.
    assert!(right_side(3.0, sides(0.0, Some(1.0), None), 3.0, reference).is_none());
    assert!(right_side(0.0, reference, 3.0, reference).is_none());
    assert!(right_side(-1.0, reference, 3.0, reference).is_none());
}
