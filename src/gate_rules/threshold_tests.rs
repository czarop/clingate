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

/// Both solvers count the way `filter_events_to_mask` does - strictly greater
/// than the lower edge. If that ever diverges, every reported fraction becomes
/// a small lie, so it is pinned here rather than left as a comment.
#[test]
fn an_event_exactly_on_the_edge_is_outside_the_gate() {
    let t = percentile_offset(&[1.0, 2.0, 3.0], 100.0, 0.0).unwrap();

    assert_eq!(t.x, 3.0);
    assert_eq!(t.events_admitted, 0, "3.0 is not strictly greater than 3.0");
}
