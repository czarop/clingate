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

// ─── Above the negative ──────────────────────────────────────────────────────

use crate::gate_rules::rule::{AboveTheNegativeRule, NegativeFinder};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};

fn gaussian(seed: u64, n: usize, mean: f64, sd: f64) -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let d = Normal::new(mean, sd).unwrap();
    (0..n).map(|_| d.sample(&mut rng)).collect()
}

/// The shadow a full-height gate whose lower edge sits at `gate_at` casts over
/// these events: each one paired with how far it sits from that edge. A
/// rectangle spanning the whole plot has one boundary at every height, so the
/// offset is just the distance along the axis - which is what these tests want
/// to reason in.
fn shadow_at(values: &[f64], gate_at: f64) -> Vec<(f64, f64)> {
    values.iter().map(|v| (*v, v - gate_at)).collect()
}

#[test]
fn the_gate_follows_a_negative_that_has_drifted() {
    // The point of the rule. The reference's gate sits three widths above its
    // negative; a sample whose negative has shifted up by a whole unit should
    // have its gate shifted with it, not left behind.
    let rule = AboveTheNegativeRule::default();
    let reference = gaussian(1, 4000, 1.0, 0.2);
    let widths = rule
        .calibrate(
            &reference,
            &shadow_at(&reference, 1.0 + 3.0 * 0.2),
            1.0 + 3.0 * 0.2,
        )
        .expect("calibrates")
        .widths;
    assert!((widths - 3.0).abs() < 0.4, "read {widths} widths");

    // Drifted by two widths, and starting from where the reference's gate sits
    // - which is what a sample inherits before it is corrected.
    let drifted = gaussian(2, 4000, 1.4, 0.2);
    let placed = rule
        .place(
            &drifted,
            &shadow_at(&drifted, 1.0 + 3.0 * 0.2),
            widths,
            1.0 + 3.0 * 0.2,
        )
        .expect("places")
        .at;
    assert!(
        (placed - (1.4 + 3.0 * 0.2)).abs() < 0.1,
        "placed at {placed}, expected about {}",
        1.4 + 3.0 * 0.2
    );
}

#[test]
fn a_negative_that_has_broadened_widens_the_gap_too() {
    // Measured in widths, not units - so a sample whose negative has spread out
    // gets its gate further out in absolute terms, which is the behaviour that
    // difficult unmixing needs.
    let rule = AboveTheNegativeRule::default();
    let reference = gaussian(3, 4000, 1.0, 0.2);
    let widths = rule
        .calibrate(
            &reference,
            &shadow_at(&reference, 1.0 + 3.0 * 0.2),
            1.0 + 3.0 * 0.2,
        )
        .unwrap()
        .widths;

    let broad = gaussian(4, 4000, 1.0, 0.4);
    let placed = rule
        .place(
            &broad,
            &shadow_at(&broad, 1.0 + 3.0 * 0.2),
            widths,
            1.0 + 3.0 * 0.2,
        )
        .expect("places")
        .at;
    assert!(
        placed > 1.0 + 3.0 * 0.3,
        "a twice-as-wide negative should push the gate further out, got {placed}"
    );
}

#[test]
fn the_scale_and_nudge_adjust_the_result() {
    let reference = gaussian(5, 4000, 1.0, 0.2);
    let plain = AboveTheNegativeRule::default();
    let gate_at = 1.0 + 3.0 * 0.2;
    let shadow = shadow_at(&reference, gate_at);
    let widths = plain
        .calibrate(&reference, &shadow, gate_at)
        .unwrap()
        .widths;
    let base = plain
        .place(&reference, &shadow, widths, gate_at)
        .unwrap()
        .at;

    let scaled = AboveTheNegativeRule {
        scale: 1.5,
        ..AboveTheNegativeRule::default()
    };
    assert!(
        scaled
            .place(&reference, &shadow, widths, gate_at)
            .unwrap()
            .at
            > base,
        "a larger scale should sit further above the negative"
    );

    // Exact for the single-pass finder. The refining one re-cuts at the nudged
    // position, so the nudge moves it and the move changes what it then sees -
    // it lands near, not exactly on, base + nudge.
    let single_pass = AboveTheNegativeRule {
        find: NegativeFinder::NegativePeak,
        ..AboveTheNegativeRule::default()
    };
    let plain_base = single_pass
        .place(&reference, &shadow, widths, gate_at)
        .unwrap()
        .at;
    let nudged = AboveTheNegativeRule {
        nudge: 0.25,
        find: NegativeFinder::NegativePeak,
        ..AboveTheNegativeRule::default()
    };
    assert!(
        (nudged
            .place(&reference, &shadow, widths, gate_at)
            .unwrap()
            .at
            - plain_base
            - 0.25)
            .abs()
            < 1e-9,
        "the nudge is added in the axis's own units"
    );
}

#[test]
fn an_unreadable_negative_refuses_rather_than_guessing() {
    // A gate placed off a peak that is not there is worse than one left alone.
    let rule = AboveTheNegativeRule::default();
    assert!(rule.calibrate(&[], &[], 1.0).is_none());
    let flat = [2.0; 500];
    assert!(
        rule.place(&flat, &shadow_at(&flat, 2.0), 3.0, 2.0)
            .is_none()
    );
}

#[test]
fn calibration_round_trips_on_the_sample_it_came_from() {
    // Applied back to its own reference, the rule reproduces the gate it read.
    let rule = AboveTheNegativeRule::default();
    let reference = gaussian(6, 4000, 1.4, 0.25);
    let gate_at = 1.4 + 2.5 * 0.25;
    let shadow = shadow_at(&reference, gate_at);
    let widths = rule.calibrate(&reference, &shadow, gate_at).unwrap().widths;
    let back = rule.place(&reference, &shadow, widths, gate_at).unwrap().at;
    assert!(
        (back - gate_at).abs() < 1e-9,
        "expected {gate_at}, got {back}"
    );
}

#[test]
fn the_two_finders_have_different_strengths() {
    // Measured, not assumed, and the reason both are offered.
    //
    // Refining from the gate is far the more accurate while the negative has
    // not moved more than the calibrated distance - about three widths - and
    // sticks below the truth beyond that, because a cut sitting under the
    // negative's centre sees a narrow slice, reads a narrow width from it, and
    // places the gate right back at the cut. A self-consistent answer that is
    // nonetheless wrong.
    //
    // The density finder has no such limit - it tracks a drift of any size -
    // but disagrees between two draws of the same population by rather more.
    let sd = 0.2;
    let reference = gaussian(1, 4000, 1.0, sd);
    let gate_at = 1.0 + 3.0 * sd;

    let refine = AboveTheNegativeRule::default();
    let density = AboveTheNegativeRule {
        find: NegativeFinder::NegativePeak,
        ..AboveTheNegativeRule::default()
    };
    let shadow = shadow_at(&reference, gate_at);
    let k_refine = refine
        .calibrate(&reference, &shadow, gate_at)
        .unwrap()
        .widths;
    let k_density = density
        .calibrate(&reference, &shadow, gate_at)
        .unwrap()
        .widths;

    let err = |rule: &AboveTheNegativeRule, k: f64, drift: f64| {
        let centre = 1.0 + drift * sd;
        let sample = gaussian(9, 4000, centre, sd);
        rule.place(&sample, &shadow_at(&sample, gate_at), k, gate_at)
            .unwrap()
            .at
            - (centre + 3.0 * sd)
    };

    // Within range, refining is the sharper of the two.
    for drift in [0.0, 1.0, 2.0] {
        assert!(
            err(&refine, k_refine, drift).abs() < err(&density, k_density, drift).abs(),
            "refining should be closer at a drift of {drift} widths"
        );
        assert!(err(&refine, k_refine, drift).abs() < 0.25 * sd);
    }

    // Past the calibrated distance it falls short, and says so here rather than
    // in someone's data.
    assert!(
        err(&refine, k_refine, 5.0) < -sd,
        "a drift beyond the calibrated distance is known to stick low"
    );
    // The density finder holds its accuracy however far the negative has moved.
    assert!(err(&density, k_density, 5.0).abs() < sd);
}

// ─── the reading the report shows ────────────────────────────────────────────

#[test]
fn the_reported_reading_reproduces_the_placement_it_made() {
    // The whole point of returning the reading rather than recomputing one for
    // display: the figures on screen have to explain the gate on the plot. If
    // these two could drift, a person checking the report would be reading a
    // number that had nothing to do with where the gate went.
    let rule = AboveTheNegativeRule {
        scale: 1.2,
        nudge: 0.05,
        ..AboveTheNegativeRule::default()
    };
    let reference = gaussian(11, 4000, 1.0, 0.2);
    let gate_at = 1.0 + 3.0 * 0.2;
    let cal = rule
        .calibrate(&reference, &shadow_at(&reference, gate_at), gate_at)
        .unwrap();

    let sample = gaussian(12, 4000, 1.25, 0.15);
    let read = rule
        .place(&sample, &shadow_at(&sample, gate_at), cal.widths, gate_at)
        .unwrap();

    let rebuilt = read.centre + read.widths * rule.scale * read.spread + rule.nudge;
    assert!(
        (rebuilt - read.at).abs() < 1e-9,
        "the reported centre, width and widths give {rebuilt}, but the gate went to {}",
        read.at
    );
}

#[test]
fn a_calibration_reports_the_position_a_person_drew() {
    // The reference row's "placed at" is the hand placement by definition -
    // nothing was solved for it.
    let rule = AboveTheNegativeRule::default();
    let reference = gaussian(13, 4000, 0.8, 0.3);
    let gate_at = 1.9;
    let cal = rule
        .calibrate(&reference, &shadow_at(&reference, gate_at), gate_at)
        .unwrap();
    assert_eq!(cal.at, gate_at);
    assert!((cal.centre + cal.widths * cal.spread - gate_at).abs() < 1e-9);
}

#[test]
fn a_tighter_negative_reads_as_a_smaller_width() {
    // The diagnostic that answers "why did this sample's gate come out lower" -
    // the width ratio is the comparison, so it has to track the truth.
    let rule = AboveTheNegativeRule::default();
    let gate_at = 1.6;
    let wide = gaussian(14, 6000, 0.0, 0.40);
    let tight = gaussian(15, 6000, 0.0, 0.25);

    let a = rule
        .calibrate(&wide, &shadow_at(&wide, gate_at), gate_at)
        .unwrap();
    let b = rule
        .calibrate(&tight, &shadow_at(&tight, gate_at), gate_at)
        .unwrap();

    let ratio = b.spread / a.spread;
    assert!(
        (ratio - 0.625).abs() < 0.1,
        "a negative 0.625 as wide should read as about that, got {ratio}"
    );
    // And the narrower one therefore reads as more widths below the same gate.
    assert!(b.widths > a.widths);
}

#[test]
fn the_flank_count_says_how_many_events_the_width_came_from() {
    // A width read off a handful of events is not worth the gate it places, and
    // nothing else in the report would say so.
    let rule = AboveTheNegativeRule::default();
    let values = gaussian(16, 4000, 0.0, 0.3);
    let gate_at = 1.0;
    let cal = rule
        .calibrate(&values, &shadow_at(&values, gate_at), gate_at)
        .unwrap();

    // Below-the-gate takes everything under the line, then the half of that
    // below its median - so about half of whatever the gate's shadow holds.
    let under_gate = values.iter().filter(|v| **v <= gate_at).count();
    assert!(
        cal.flank_events > under_gate / 2 - 20 && cal.flank_events < under_gate / 2 + 20,
        "{} events for a shadow holding {under_gate}",
        cal.flank_events
    );
}

// ── the phenotype rule's shape ───────────────────────────────────────────

#[test]
fn a_phenotype_rule_round_trips_through_a_sidecar() {
    use crate::gate_rules::rule::{PhenotypeRule, ShapeFit};
    let rule = Rule::MatchThePhenotype(PhenotypeRule {
        markers: vec![
            std::sync::Arc::from("TCRVa7.2"),
            std::sync::Arc::from("CD161"),
            std::sync::Arc::from("CD127"),
        ],
        fit: ShapeFit::DrawPolygon,
        keep: 0.9,
        smoothing: 1.2,
        vertices: 16,
        ..Default::default()
    });
    let text = serde_json::to_string(&rule).expect("serialises");
    let back: Rule = serde_json::from_str(&text).expect("deserialises");
    assert_eq!(back, rule);
}

#[test]
fn a_phenotype_rule_defaults_to_every_marker_and_the_drawn_shape() {
    use crate::gate_rules::rule::{PhenotypeRule, ShapeFit};
    let rule = PhenotypeRule::default();
    assert!(rule.markers.is_empty(), "empty means the whole panel");
    assert_eq!(rule.fit, ShapeFit::KeepShape);
}

#[test]
fn a_sidecar_written_before_this_rule_existed_still_loads() {
    // Every field is defaulted, so a rule naming only its markers is valid -
    // which is what a hand-written sidecar will look like.
    use crate::gate_rules::rule::{PhenotypeRule, ShapeFit};
    let rule: PhenotypeRule =
        serde_json::from_str(r#"{"markers":["CD161"]}"#).expect("the rest defaults");
    assert_eq!(rule.markers.len(), 1);
    assert_eq!(rule.fit, ShapeFit::KeepShape);
    assert_eq!(rule.vertices, 24);
}

#[test]
fn the_two_ways_of_fitting_are_offered_and_round_trip_on_their_keys() {
    use crate::gate_rules::rule::ShapeFit;
    for fit in ShapeFit::ALL {
        let text = serde_json::to_string(&fit).expect("serialises");
        assert_eq!(text.trim_matches('"'), fit.key());
        assert!(!fit.label().is_empty());
        assert!(!fit.choice().is_empty());
    }
}

#[test]
fn a_phenotype_rule_says_which_markers_it_uses() {
    use crate::gate_rules::rule::{PhenotypeRule, ShapeFit};
    let named = PhenotypeRule {
        markers: vec![std::sync::Arc::from("CD161")],
        fit: ShapeFit::DrawPolygon,
        ..Default::default()
    };
    assert!(named.describe().contains("CD161"));
    assert!(PhenotypeRule::default().describe().contains("every marker"));
}

#[test]
fn a_phenotype_rule_has_no_band_to_be_already_inside() {
    use crate::gate_rules::rule::PhenotypeRule;
    // A gate is right when it holds the matching cells, which cannot be read
    // off a fraction of the parent - so it is always re-fitted.
    assert!(
        Rule::MatchThePhenotype(PhenotypeRule::default())
            .accepted_band()
            .is_none()
    );
}
