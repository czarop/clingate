//! The decisions these make are the whole of the rules approach, so they are
//! tested against hand-worked numbers rather than against the implementation's
//! own idea of what it does.

#![cfg(test)]

use super::threshold::*;

/// `n` events spread evenly from 0 to `n - 1`, largest last.
fn ramp(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64).collect()
}

// ─── tail_fraction: the ordinary case ─────────────────────────────────────────

#[test]
fn a_band_admits_the_count_its_midpoint_asks_for() {
    // 1000 events, band 0.2%-0.5%, midpoint 0.35% -> 3.5 -> 4 events.
    let t = tail_fraction(&ramp(1000), (0.002, 0.005)).unwrap();

    assert_eq!(t.events_admitted, 4);
    assert_eq!(t.fraction_admitted, 0.004);
    assert_eq!(t.status, Status::InBand);
}

#[test]
fn the_edge_sits_midway_between_the_events_it_separates() {
    // The four largest are 999, 998, 997, 996; the first excluded is 995.
    let t = tail_fraction(&ramp(1000), (0.002, 0.005)).unwrap();

    assert_eq!(
        t.x, 995.5,
        "midway between the last admitted and the first not"
    );
}

#[test]
fn the_edge_goes_in_the_middle_of_a_wide_gap() {
    // A clean negative cloud and three far-off positives: the edge belongs in
    // the empty space between them, not hard against either.
    let mut values = vec![0.0; 997];
    values.extend([50.0, 60.0, 70.0]);

    let t = tail_fraction(&values, (0.002, 0.005)).unwrap();

    assert_eq!(t.events_admitted, 3);
    assert_eq!(t.x, 25.0, "midway across the gap from 0 to 50");
}

#[test]
fn a_wider_band_still_aims_at_its_middle() {
    // 1000 events, 1%-5%, midpoint 3% -> 30 events.
    let t = tail_fraction(&ramp(1000), (0.01, 0.05)).unwrap();

    assert_eq!(t.events_admitted, 30);
    assert_eq!(t.status, Status::InBand);
}

// ─── tail_fraction: when the rule cannot be met ───────────────────────────────

#[test]
fn too_few_events_for_the_band_is_flagged_not_fudged() {
    // 100 events and a 0.2%-0.5% band: that is 0.2 to 0.5 of an event, so no
    // whole number of events lands inside it. The closest is none at all.
    let t = tail_fraction(&ramp(100), (0.002, 0.005)).unwrap();

    assert_eq!(t.events_admitted, 0);
    assert_eq!(
        t.status,
        Status::OutOfBand {
            band: (0.002, 0.005)
        },
        "the low-count case has to reach the report, not pass silently"
    );
}

#[test]
fn a_band_that_allows_a_whole_number_of_events_is_satisfied() {
    // 300 events, 0.2%-0.5% -> 0.6 to 1.5 events, so exactly one is allowed.
    // The midpoint asks for 1.05, which rounds to 1 and is already inside.
    let t = tail_fraction(&ramp(300), (0.002, 0.005)).unwrap();

    assert_eq!(t.events_admitted, 1);
    assert_eq!(t.status, Status::InBand);
}

#[test]
fn a_band_allowing_exactly_one_count_lands_on_it() {
    // 1000 events, 0.55%-0.65% -> 5.5 to 6.5 events, so 6 is the only whole
    // number inside.
    let t = tail_fraction(&ramp(1000), (0.0055, 0.0065)).unwrap();

    assert_eq!(t.events_admitted, 6);
    assert_eq!(t.status, Status::InBand);
}

/// A band can be wide in percentage terms and still contain no achievable
/// fraction: 0.61% to 0.69% of a thousand events is 6.1 to 6.9 of them, and a
/// gate admits whole cells. Six is below the band and seven above it.
#[test]
fn a_band_between_two_whole_events_cannot_be_satisfied() {
    let t = tail_fraction(&ramp(1000), (0.0061, 0.0069)).unwrap();

    assert_eq!(t.events_admitted, 7, "the nearest achievable count");
    assert_eq!(
        t.status,
        Status::OutOfBand {
            band: (0.0061, 0.0069)
        }
    );
}

#[test]
fn a_population_that_is_all_one_value_reports_what_it_really_admits() {
    // Every event identical: there is no gap to put an edge in, so whatever is
    // chosen admits all of them or none. It must not claim otherwise.
    let t = tail_fraction(&vec![7.0; 500], (0.002, 0.005)).unwrap();

    assert_eq!(t.events_admitted, 0, "nothing is strictly above the value");
    assert_eq!(
        t.status,
        Status::OutOfBand {
            band: (0.002, 0.005)
        }
    );
}

#[test]
fn a_target_inside_a_run_of_ties_falls_back_to_the_nearest_count_that_works() {
    // Ten events all at exactly 5.0, and a band asking for four of them. No
    // edge can separate four from ten identical values, so the achievable
    // counts here are none of them or all ten. Four is nearer to none, so that
    // is what it takes - and says so, rather than reporting a count it does
    // not actually admit.
    let mut values = vec![0.0; 990];
    values.extend([5.0; 10]);

    let t = tail_fraction(&values, (0.002, 0.005)).unwrap();

    assert_eq!(t.events_admitted, 0);
    assert_eq!(
        t.x, 5.0,
        "on the tied value, which is not strictly above it"
    );
    assert_eq!(
        t.status,
        Status::OutOfBand {
            band: (0.002, 0.005)
        }
    );
}

/// The same run of ties, but with a band that reaches far enough for all ten to
/// be the nearer answer. The fallback searches both ways, not just down.
#[test]
fn a_run_of_ties_is_admitted_whole_when_that_is_the_nearer_count() {
    let mut values = vec![0.0; 990];
    values.extend([5.0; 10]);

    let t = tail_fraction(&values, (0.006, 0.009)).unwrap();

    assert_eq!(t.events_admitted, 10, "seven and a half rounds toward ten");
    assert_eq!(t.x, 2.5, "midway between the tied run and the negative");
}

// ─── tail_fraction: the extremes ──────────────────────────────────────────────

#[test]
fn a_band_of_zero_admits_nothing() {
    let t = tail_fraction(&ramp(1000), (0.0, 0.0)).unwrap();

    assert_eq!(t.events_admitted, 0);
    assert_eq!(t.fraction_admitted, 0.0);
    assert_eq!(t.status, Status::InBand);
}

#[test]
fn a_band_of_everything_admits_everything() {
    let t = tail_fraction(&ramp(1000), (1.0, 1.0)).unwrap();

    assert_eq!(t.events_admitted, 1000);
    assert!(t.x < 0.0, "the edge has to clear the smallest event");
    assert_eq!(t.status, Status::InBand);
}

#[test]
fn a_single_event_is_not_a_crash() {
    let t = tail_fraction(&[42.0], (0.002, 0.005)).unwrap();
    assert_eq!(t.events_admitted, 0);
}

#[test]
fn an_empty_population_is_an_error_not_a_guess() {
    assert_eq!(
        tail_fraction(&[], (0.002, 0.005)),
        Err(SolveError::NoEvents)
    );
}

#[test]
fn a_population_of_nothing_but_nan_is_an_error() {
    assert_eq!(
        tail_fraction(&[f64::NAN, f64::INFINITY], (0.002, 0.005)),
        Err(SolveError::NoFiniteValues)
    );
}

#[test]
fn non_finite_values_do_not_disturb_the_count() {
    let mut values = ramp(1000);
    values.push(f64::NAN);

    let t = tail_fraction(&values, (0.002, 0.005)).unwrap();

    assert_eq!(
        t.events_admitted, 4,
        "the NaN is dropped, not counted or sorted"
    );
    assert_eq!(t.x, 995.5);
}

#[test]
fn a_band_outside_zero_to_one_is_refused() {
    assert!(matches!(
        tail_fraction(&ramp(10), (0.5, 1.5)),
        Err(SolveError::BadBand { .. })
    ));
    assert!(matches!(
        tail_fraction(&ramp(10), (0.5, 0.2)),
        Err(SolveError::BadBand { .. })
    ));
}

// ─── percentile_offset ────────────────────────────────────────────────────────

#[test]
fn a_percentile_is_interpolated_between_order_statistics() {
    // 0..=100 inclusive: the 99th percentile of 101 points is exactly 99.
    let sorted: Vec<f64> = (0..=100).rev().map(|i| i as f64).collect();

    assert_eq!(percentile_of_descending(&sorted, 99.0), 99.0);
    assert_eq!(percentile_of_descending(&sorted, 50.0), 50.0);
    assert_eq!(percentile_of_descending(&sorted, 0.0), 0.0);
    assert_eq!(percentile_of_descending(&sorted, 100.0), 100.0);
}

#[test]
fn a_percentile_between_two_points_is_weighted() {
    // Four points, so the 50th percentile sits midway between the middle two.
    let sorted = vec![4.0, 3.0, 2.0, 1.0];
    assert_eq!(percentile_of_descending(&sorted, 50.0), 2.5);
}

#[test]
fn the_edge_lands_the_offset_above_the_percentile() {
    let values: Vec<f64> = (0..=100).map(|i| i as f64).collect();

    let t = percentile_offset(&values, 99.0, 5.0).unwrap();

    assert_eq!(t.x, 104.0, "the 99th percentile is 99, plus an offset of 5");
    assert_eq!(t.status, Status::NoBand);
}

#[test]
fn a_percentile_rule_reports_what_it_captures() {
    let values: Vec<f64> = (0..1000).map(|i| i as f64).collect();

    // The 99th percentile of 0..999 is 989.01; an offset of 0 leaves the events
    // above it admitted.
    let t = percentile_offset(&values, 99.0, 0.0).unwrap();

    assert_eq!(t.events_admitted, 10);
    assert_eq!(t.fraction_admitted, 0.01);
}

#[test]
fn an_offset_far_above_the_data_admits_nothing() {
    let t = percentile_offset(&ramp(1000), 99.0, 10_000.0).unwrap();

    assert_eq!(t.events_admitted, 0);
    assert_eq!(t.fraction_admitted, 0.0);
}

#[test]
fn a_negative_offset_reaches_back_into_the_negative() {
    let values: Vec<f64> = (0..=100).map(|i| i as f64).collect();

    let t = percentile_offset(&values, 99.0, -9.0).unwrap();

    assert_eq!(t.x, 90.0);
    assert_eq!(t.events_admitted, 10, "91 through 100");
}

#[test]
fn a_percentile_outside_its_range_is_refused() {
    assert_eq!(
        percentile_offset(&ramp(10), 101.0, 0.0),
        Err(SolveError::BadPercentile(101.0))
    );
}

#[test]
fn a_percentile_rule_on_an_empty_population_is_an_error() {
    assert_eq!(percentile_offset(&[], 99.0, 1.0), Err(SolveError::NoEvents));
}

// ─── the convention that ties the two together ────────────────────────────────

/// Both solvers count strictly greater than the edge - their model, pinned so
/// it changes only on purpose. A placed gate is measured on the gate itself
/// (`autogate::admitted_by`), which holds an event on its lower edge; see the
/// module notes in `threshold`.
#[test]
fn an_event_exactly_on_the_edge_is_outside_the_gate() {
    let t = percentile_offset(&[1.0, 2.0, 3.0], 100.0, 0.0).unwrap();

    assert_eq!(t.x, 3.0);
    assert_eq!(t.events_admitted, 0, "3.0 is not strictly greater than 3.0");
}

// ─── Finding the negative ────────────────────────────────────────────────────

use crate::gate_rules::threshold::negative_peak;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};

/// `n` events around `mean` with width `sd`, reproducibly.
fn cluster(seed: u64, n: usize, mean: f64, sd: f64) -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let d = Normal::new(mean, sd).unwrap();
    (0..n).map(|_| d.sample(&mut rng)).collect()
}

#[test]
fn the_negative_peak_is_found_where_the_population_is() {
    let found = negative_peak(&cluster(1, 4000, 1.75, 0.15)).expect("one clean population");
    assert!(
        (found.centre - 1.75).abs() < 0.06,
        "centre landed at {}",
        found.centre
    );
    assert!(
        (found.spread - 0.15).abs() < 0.05,
        "spread read as {}",
        found.spread
    );
}

#[test]
fn a_positive_tail_does_not_widen_the_negative() {
    // The whole point of measuring the left flank. A standard deviation taken
    // across the peak grows with however many positives the sample has, which
    // is exactly the variation this rule exists to see past.
    let clean = negative_peak(&cluster(2, 4000, 1.0, 0.2)).unwrap();

    let mut with_tail = cluster(2, 4000, 1.0, 0.2);
    with_tail.extend(cluster(3, 800, 3.0, 0.4));
    let contaminated = negative_peak(&with_tail).unwrap();

    assert!(
        (contaminated.spread - clean.spread).abs() < 0.05,
        "the tail moved the spread from {} to {}",
        clean.spread,
        contaminated.spread
    );
    assert!(
        (contaminated.centre - clean.centre).abs() < 0.06,
        "the tail moved the centre from {} to {}",
        clean.centre,
        contaminated.centre
    );
}

#[test]
fn the_negative_is_taken_even_when_the_positives_outnumber_it() {
    // The tallest peak is not always the negative. Taking it would put the gate
    // above the population it is meant to separate.
    let mut values = cluster(4, 1000, 0.5, 0.15);
    values.extend(cluster(5, 4000, 2.5, 0.2));
    let found = negative_peak(&values).expect("two populations");

    assert!(
        (found.centre - 0.5).abs() < 0.1,
        "took the wrong peak: {}",
        found.centre
    );
}

#[test]
fn a_population_too_small_to_read_is_declined() {
    assert!(negative_peak(&[]).is_none());
    assert!(negative_peak(&[1.0]).is_none());
    // Every event identical: no width to measure, so no spread to scale by.
    assert!(negative_peak(&[2.0; 500]).is_none());
}

// ─── finding the valley ──────────────────────────────────────────────────────

use crate::gate_rules::threshold::{first_valley, valley_in};

/// A density built by hand, so the right answer is known rather than estimated.
fn density(heights: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let xs: Vec<f64> = (0..heights.len()).map(|i| i as f64).collect();
    (xs, heights.to_vec())
}

#[test]
fn the_dip_between_two_peaks_is_the_boundary() {
    let (xs, d) = density(&[1.0, 5.0, 10.0, 4.0, 1.0, 4.0, 9.0, 3.0, 1.0]);
    let found = valley_in(&xs, &d).expect("two peaks with a dip between them");
    assert_eq!(found.peak, 2.0, "the negative is the leftmost peak");
    assert_eq!(found.bottom, 4.0, "the gate belongs at the lowest point");
    // The dip falls to 1 from a flanking height of 9, so it is 8/9 deep.
    assert!(
        (found.depth - 8.0 / 9.0).abs() < 1e-9,
        "depth {}",
        found.depth
    );
}

#[test]
fn a_shallow_dip_is_still_a_boundary() {
    // The case this rule exists for: the two populations have blurred together
    // but there is still a lowest point, and it is still where the gate goes.
    let (xs, d) = density(&[1.0, 5.0, 10.0, 9.0, 8.5, 9.0, 9.5, 4.0, 1.0]);
    let found = valley_in(&xs, &d).expect("a shallow dip is a dip");
    assert_eq!(found.bottom, 4.0);
    assert!(
        found.depth > 0.02 && found.depth < 0.2,
        "shallow but real: {}",
        found.depth
    );
}

#[test]
fn a_single_population_has_no_boundary_to_find() {
    // The EOMES failure. Two populations merged into one hump - so there is no
    // dip, and the honest answer is to refuse rather than place something.
    let (xs, d) = density(&[1.0, 4.0, 9.0, 10.0, 9.0, 6.0, 3.0, 1.0]);
    let why = valley_in(&xs, &d).unwrap_err();
    assert!(matches!(why, NoValley::OnlyOnePeak { .. }), "{why:?}");
}

#[test]
fn a_smear_off_the_negative_has_no_boundary_either() {
    // Monotone decline from the negative into a tail. No second population, so
    // nothing to sit between - this is what above-the-negative is for.
    let (xs, d) = density(&[1.0, 6.0, 10.0, 7.0, 5.0, 3.5, 2.0, 1.0, 0.5]);
    let why = valley_in(&xs, &d).unwrap_err();
    assert!(
        matches!(why, NoValley::OnlyOnePeak { peak, .. } if peak == 2.0),
        "{why:?}"
    );
}

#[test]
fn a_wobble_on_the_shoulder_is_not_a_boundary() {
    // A dip of a few percent is noise on a shoulder. Taking it as a boundary
    // would put the gate wherever the estimate happened to wobble.
    let (xs, d) = density(&[1.0, 5.0, 10.0, 7.0, 6.95, 6.98, 4.0, 1.0]);
    let why = valley_in(&xs, &d).unwrap_err();
    // The wobble was seen and judged too shallow - not missed.
    let NoValley::NothingDeepEnough {
        best_at,
        best_depth,
        ..
    } = why
    else {
        panic!("{why:?}");
    };
    assert_eq!(best_at, 4.0);
    assert!(best_depth < 0.02, "{best_depth}");
}

#[test]
fn the_first_dip_is_taken_when_there_are_several() {
    // Three populations. The boundary that matters is the one next to the
    // negative, not the deepest one further out.
    let (xs, d) = density(&[1.0, 8.0, 10.0, 3.0, 1.0, 7.0, 9.0, 0.5, 0.2, 6.0, 8.0]);
    let found = valley_in(&xs, &d).expect("several dips");
    assert_eq!(found.bottom, 4.0, "the first one, next to the negative");
}

#[test]
fn a_valley_is_found_in_real_looking_data() {
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rand_distr::{Distribution, Normal};

    let mut rng = StdRng::seed_from_u64(4);
    let neg = Normal::new(0.0, 0.4).unwrap();
    let pos = Normal::new(3.0, 0.5).unwrap();
    let mut v: Vec<f64> = (0..20_000).map(|_| neg.sample(&mut rng)).collect();
    v.extend((0..8_000).map(|_| pos.sample(&mut rng)));

    let found = first_valley(&v, 1.0).expect("a clean two-peak population");
    assert!(found.peak.abs() < 0.2, "negative peak at {}", found.peak);
    assert!(
        found.bottom > 1.0 && found.bottom < 2.2,
        "the dip should sit between them, got {}",
        found.bottom
    );
    assert!(found.depth > 0.5, "a clean separation: {}", found.depth);
}

#[test]
fn smoothing_trades_shallow_dips_against_noise() {
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rand_distr::{Distribution, Normal};

    // Two populations close enough that the dip between them is shallow.
    let mut rng = StdRng::seed_from_u64(5);
    let neg = Normal::new(0.0, 0.5).unwrap();
    let pos = Normal::new(1.6, 0.5).unwrap();
    let mut v: Vec<f64> = (0..20_000).map(|_| neg.sample(&mut rng)).collect();
    v.extend((0..10_000).map(|_| pos.sample(&mut rng)));

    let fine = first_valley(&v, 0.6);
    let coarse = first_valley(&v, 2.5);
    assert!(
        fine.is_ok(),
        "less smoothing should still find it: {fine:?}"
    );
    // Heavy smoothing merges them into one hump, which is the trade the
    // parameter exists to expose rather than decide.
    if let (Ok(a), Ok(b)) = (fine, coarse) {
        assert!(a.depth >= b.depth, "{:?} vs {:?}", a, b);
    }
}

#[test]
fn a_ripple_in_the_tail_is_not_a_valley() {
    // Depth is read against the lower of the two flanking peaks, and out in a
    // sparse tail that height is nearly zero - so a ripple of no consequence
    // reads as a deep dip and puts the gate far out in empty data. That is the
    // failure this whole rule exists to avoid, arriving by another route.
    //
    // One tall population, then a negligible wobble in its tail.
    let (xs, d) = density(&[
        1.0, 20.0, 100.0, 60.0, 20.0, 5.0, 1.0, 0.4, 0.9, 0.5, 0.2, 0.1,
    ]);
    let why = valley_in(&xs, &d).unwrap_err();
    assert!(
        matches!(why, NoValley::NothingDeepEnough { best_at, .. } if best_at == 7.0),
        "the far side must be a population, not a ripple: {why:?}"
    );
}

#[test]
fn a_small_but_real_second_population_is_still_a_valley() {
    // The other side of that bar: a positive a tenth the height of the negative
    // is a population, and the dip before it is a boundary.
    let (xs, d) = density(&[1.0, 20.0, 100.0, 40.0, 5.0, 2.0, 8.0, 12.0, 6.0, 1.0]);
    let found = valley_in(&xs, &d).expect("a small population is still a population");
    assert_eq!(found.bottom, 5.0);
}

#[test]
fn a_refusal_says_what_the_density_looked_like() {
    use crate::gate_rules::threshold::NoValley;

    // One hump: the message should name the peak rather than just refusing.
    let (xs, d) = density(&[1.0, 4.0, 9.0, 10.0, 9.0, 6.0, 3.0, 1.0]);
    let why = valley_in(&xs, &d).unwrap_err();
    assert!(
        matches!(why, NoValley::OnlyOnePeak { peak, .. } if peak == 3.0),
        "{why:?}"
    );
    assert!(why.to_string().contains("merged"), "{why}");

    // A tail ripple: the message should say the far side was negligible, which
    // is what tells it apart from a real dip.
    let (xs, d) = density(&[
        1.0, 20.0, 100.0, 60.0, 20.0, 5.0, 1.0, 0.4, 0.9, 0.5, 0.2, 0.1,
    ]);
    let why = valley_in(&xs, &d).unwrap_err();
    let NoValley::NothingDeepEnough { far_side, .. } = why else {
        panic!("{why:?}");
    };
    assert!(far_side < 0.05, "a ripple in the tail: {far_side}");
    assert!(why.to_string().contains("far side"), "{why}");
}

/// BUG (docs/test-audit.md, B-THR-1): `OnlyOnePeak` carries how many events
/// the peak was read from, for the report, and every construction of it
/// writes 0 - so the message says "one peak ... over 0 events" whatever the
/// population was.
#[test]
#[ignore = "known bug B-THR-1: OnlyOnePeak always reports 0 events"]
fn a_single_peak_reports_how_many_events_it_was_read_from() {
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rand_distr::{Distribution, Normal};

    let mut rng = StdRng::seed_from_u64(6);
    let one = Normal::new(0.0, 0.4).unwrap();
    let v: Vec<f64> = (0..5_000).map(|_| one.sample(&mut rng)).collect();
    let why = first_valley(&v, 1.0).unwrap_err();
    assert!(
        matches!(why, NoValley::OnlyOnePeak { events: 5_000, .. }),
        "{why:?}"
    );
}

#[test]
fn a_valley_refuses_a_bad_smoothing_or_too_little_data() {
    let v = [0.0, 1.0, 2.0];
    for smoothing in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            first_valley(&v, smoothing),
            Err(NoValley::NoPopulation),
            "{smoothing}"
        );
    }
    assert_eq!(first_valley(&[1.0], 1.0), Err(NoValley::NoPopulation));
    assert_eq!(
        first_valley(&[2.0, 2.0, 2.0], 1.0),
        Err(NoValley::NoPopulation)
    );
    let (xs, d) = density(&[1.0, 2.0]);
    assert_eq!(valley_in(&xs, &d), Err(NoValley::NoPopulation));
    let (xs, d) = density(&[0.0, 0.0, 0.0, 0.0]);
    assert_eq!(valley_in(&xs, &d), Err(NoValley::NoPopulation));
}

// ─── the negative read from below a gate ─────────────────────────────────────

/// Each event paired with its distance from a straight edge at `edge`, the
/// way the rules build a gate's shadow.
fn shadow(values: &[f64], edge: f64) -> Vec<(f64, f64)> {
    values.iter().map(|v| (*v, v - edge)).collect()
}

fn gaussian(centre: f64, sigma: f64, n: usize, seed: u64) -> Vec<f64> {
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rand_distr::{Distribution, Normal};
    let mut rng = StdRng::seed_from_u64(seed);
    let d = Normal::new(centre, sigma).unwrap();
    (0..n).map(|_| d.sample(&mut rng)).collect()
}

#[test]
fn the_negative_below_a_gate_is_the_events_under_it() {
    // A negative at 1.0 (sigma 0.2) and positives at 4.0; the gate's edge at
    // 2.5 separates them, so what is below is the negative alone.
    let mut v = gaussian(1.0, 0.2, 20_000, 11);
    v.extend(gaussian(4.0, 0.3, 5_000, 12));
    let found = negative_below(&shadow(&v, 2.5), 0.0).unwrap();

    assert!((found.centre - 1.0).abs() < 0.02, "centre {}", found.centre);
    assert!((found.spread - 0.2).abs() < 0.02, "spread {}", found.spread);
    assert!(
        (9_000..11_000).contains(&found.flank_events),
        "about half the negative is below its centre: {}",
        found.flank_events
    );
}

#[test]
fn sliding_the_gate_changes_which_events_are_below_it() {
    // `at` slides the gate: only events whose offset is at most `at` count.
    let v: Vec<f64> = (0..10).map(f64::from).collect();
    let low = negative_below(&shadow(&v, 5.0), -2.0).unwrap();
    let high = negative_below(&shadow(&v, 5.0), 2.0).unwrap();
    // Below 3: 0..=3, median 2. Below 7: 0..=7, median 4.
    assert_eq!(low.centre, 2.0);
    assert_eq!(high.centre, 4.0);
}

#[test]
fn too_little_below_the_gate_is_nothing() {
    let v = [1.0, 2.0, 3.0];
    assert_eq!(negative_below(&shadow(&v, 1.5), 0.0), None, "one event");
    assert_eq!(negative_below(&shadow(&v, 0.0), 0.0), None, "none");
    // Two identical events have no width to measure.
    assert_eq!(negative_below(&[(1.0, 0.0), (1.0, 0.0)], 0.0), None);
}

#[test]
fn refining_from_a_gate_set_too_high_settles_on_the_negative() {
    // The gate starts well above the negative, so its first look takes in
    // positives and reads the centre high; each pass sees less of them.
    // `place` asks for the gate 3 widths above the centre, the usual rule.
    let mut v = gaussian(1.0, 0.2, 20_000, 13);
    v.extend(gaussian(3.0, 0.3, 4_000, 14));
    let edge = 2.6;
    let found = refine_from(&shadow(&v, edge), 0.0, |p| p.centre + 3.0 * p.spread - edge).unwrap();

    assert!((found.centre - 1.0).abs() < 0.05, "centre {}", found.centre);
    assert!((found.spread - 0.2).abs() < 0.05, "spread {}", found.spread);
}

#[test]
fn refining_does_not_run_off_the_axis_from_a_gate_set_too_low() {
    // A gate starting below most of the negative sees only its bottom and
    // reads it low; unchecked, each pass would walk further down. The step
    // cap and the divergence check stop it: the answer stays within the data.
    let v = gaussian(1.0, 0.2, 20_000, 15);
    let edge = 0.5;
    let found = refine_from(&shadow(&v, edge), 0.0, |p| p.centre - 3.0 * p.spread - edge).unwrap();

    let lowest = v.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(found.centre >= lowest, "{} below every event", found.centre);
}

#[test]
fn refining_a_gate_already_in_place_changes_nothing() {
    let v = gaussian(1.0, 0.2, 20_000, 16);
    let edge = 1.6;
    let once = negative_below(&shadow(&v, edge), 0.0).unwrap();
    // `place` returns exactly where the gate already is.
    let settled = refine_from(&shadow(&v, edge), 0.0, |_| 0.0).unwrap();
    assert_eq!(settled, once);
}

// ─── the spread and the count swing ──────────────────────────────────────────

#[test]
fn the_interquartile_spread_of_a_ramp_is_half_its_range() {
    let mut desc = ramp(101);
    desc.reverse();
    assert_eq!(interquartile_spread(&desc), 50.0);
}

#[test]
fn an_edge_in_empty_space_does_not_swing() {
    // Two clusters far apart; an edge in the gap between them can be nudged
    // by a tenth of the spread without changing what it admits.
    let mut v = vec![0.0; 500];
    v.extend(vec![100.0; 500]);
    let t = tail_fraction(&v, (0.4, 0.6)).unwrap();
    assert_eq!(t.events_admitted, 500);
    assert_eq!(t.count_swing, 0.0);
}

#[test]
fn an_edge_in_a_dense_cloud_swings() {
    // The same fraction taken out of the middle of a continuum: nudging the
    // edge moves events in and out.
    let t = tail_fraction(&ramp(1000), (0.4, 0.6)).unwrap();
    assert!(t.count_swing > 0.1, "{}", t.count_swing);
}
