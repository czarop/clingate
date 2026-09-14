//! The score exists to rank gates for review, so what is tested is the ranking
//! and the reasons - not the constants, which are not earned yet.

#![cfg(test)]

use super::confidence::*;
use super::threshold::*;

fn assess(t: &Threshold, reference: Option<f64>) -> Confidence {
    CountAndSeparation::default().assess(t, reference)
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
    let c = assess(&t, None);

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
    let c = assess(&t, None);

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
    let c = assess(&t, None);

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
    // negative, where it separates nothing.
    let buried: Vec<f64> = (0..10_000).map(|i| (i % 1000) as f64 / 1000.0).collect();

    let in_gap = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let in_cloud = tail_fraction(&buried, (0.09, 0.11)).unwrap();

    let limits = ConfidenceLimits::default();
    let gap_score = CountAndSeparation {
        limits: limits.clone(),
    }
    .assess(&in_gap, None);
    let cloud_score = CountAndSeparation {
        limits: limits.clone(),
    }
    .assess(&in_cloud, None);

    assert!(
        part(&cloud_score, SEPARATION) < part(&gap_score, SEPARATION),
        "buried {} should score below separated {}",
        part(&cloud_score, SEPARATION),
        part(&gap_score, SEPARATION)
    );
}

#[test]
fn a_rule_that_could_not_be_satisfied_scores_below_one() {
    // 100 events, 0.2%-0.5%: no whole number of events lands in the band.
    let values: Vec<f64> = (0..100).map(|i| i as f64).collect();

    let t = tail_fraction(&values, (0.002, 0.005)).unwrap();
    let c = assess(&t, None);

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
    let c = assess(&t, None);

    assert_eq!(part(&c, BAND), 1.0);
}

// ─── displacement ─────────────────────────────────────────────────────────────

#[test]
fn a_gate_that_barely_moved_keeps_its_confidence() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let limits = ConfidenceLimits::default();

    let c = CountAndSeparation {
        limits: limits.clone(),
    }
    .assess(&t, Some(t.x));

    assert_eq!(
        c.get(DISPLACEMENT).map(|d| d.score),
        Some(1.0),
        "no movement at all"
    );
}

#[test]
fn a_gate_dragged_a_long_way_from_its_reference_is_flagged() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let limits = ConfidenceLimits::default();

    // Three interquartile widths away, past the limit of two.
    let far = t.x + 3.0 * t.parent_spread;
    let c = CountAndSeparation {
        limits: limits.clone(),
    }
    .assess(&t, Some(far));

    assert_eq!(c.get(DISPLACEMENT).map(|d| d.score), Some(0.0));
    assert_eq!(c.score, 0.0);
    assert_eq!(c.weakest().unwrap().name, DISPLACEMENT);
}

#[test]
fn displacement_is_read_against_the_spread_not_in_raw_units() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let limits = ConfidenceLimits::default();

    // One interquartile width is half the limit, so half the component.
    let moved = t.x + t.parent_spread;
    let c = CountAndSeparation {
        limits: limits.clone(),
    }
    .assess(&t, Some(moved));

    let displacement = part(&c, DISPLACEMENT);
    assert!(
        (displacement - 0.5).abs() < 1e-9,
        "expected half, got {displacement}"
    );
}

#[test]
fn no_reference_means_no_displacement_component() {
    let t = tail_fraction(&clean(), (0.09, 0.11)).unwrap();
    let c = assess(&t, None);

    assert_eq!(c.get(DISPLACEMENT), None);
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
    let c = assess(&t, Some(t.x));

    let weakest = part(&c, EVENTS)
        .min(part(&c, ADMITTED))
        .min(part(&c, SEPARATION))
        .min(part(&c, BAND))
        .min(part(&c, DISPLACEMENT));
    assert_eq!(c.score, weakest);

    let product = part(&c, EVENTS) * part(&c, ADMITTED) * part(&c, SEPARATION) * part(&c, BAND);
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
    let c = assess(&t, None);

    assert_eq!(t.events_admitted, 1);
    assert_eq!(c.score, 0.0, "one event cannot support a placement: {c:?}");
}
