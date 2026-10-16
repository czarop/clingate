//! The check on events built by hand: negatives centred on 0 and 100 units
//! wide, a third of the cells positive for A at about 5000, a known share of
//! A spilled into B, and a known autofluorescence in large granular cells -
//! so every expected number is set here, not read off the check.

use std::sync::Arc;

use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Uniform};

use crate::spillover::{Finding, Measured, Negative, SampleEvents, check, lean, measure};

const CELLS: usize = 30_000;
const WIDTH: f32 = 100.0;

fn channels() -> Vec<Arc<str>> {
    vec![Arc::from("A"), Arc::from("B"), Arc::from("C")]
}

/// One sample: `spill` of A left in B, and `af` units more in C for cells in
/// the top fifth of side scatter.
fn measured(name: &str, seed: u64, spill: f32, af: f32) -> Measured {
    measure(&sample(name, seed, spill, af))
}

fn sample(name: &str, seed: u64, spill: f32, af: f32) -> SampleEvents {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let negative = Normal::new(0.0f32, WIDTH).unwrap();
    let positive = Normal::new(5000.0f32, 1000.0).unwrap();
    let scatter = Uniform::new(0.0f32, 1.0).unwrap();
    let (mut a, mut b, mut c, mut side) = (vec![], vec![], vec![], vec![]);
    for cell in 0..CELLS {
        let ssc = scatter.sample(&mut rng);
        let a_value = if cell % 3 == 0 {
            positive.sample(&mut rng)
        } else {
            negative.sample(&mut rng)
        };
        a.push(a_value);
        b.push(negative.sample(&mut rng) + spill * a_value.max(0.0));
        c.push(negative.sample(&mut rng) + if ssc > 0.8 { af } else { 0.0 });
        side.push(ssc);
    }
    SampleEvents {
        sample: name.into(),
        values: vec![a, b, c],
        side_scatter: side,
    }
}

#[test]
fn a_negative_is_read_off_the_cells_below_the_median_whatever_the_positives() {
    let events = sample("s", 1, 0.0, 0.0);

    let a = Negative::of(&events.values[0]).unwrap();

    // A third of A is positive, so the median sits in the negative's upper
    // half (67); the negative is still read at 0, 100 wide.
    assert!(a.centre.abs() < 15.0, "{a:?}");
    assert!((a.width - 100.0).abs() < 10.0, "{a:?}");
    assert_eq!(a.positive_above(), a.centre + 4.0 * a.width);
}

#[test]
fn a_lean_is_the_share_of_a_left_in_b_and_how_far_it_moves_b() {
    let events = sample("s", 2, 0.1, 0.0);
    let (a, b) = (
        Negative::of(&events.values[0]).unwrap(),
        Negative::of(&events.values[1]).unwrap(),
    );

    let (slope, widths) = lean(&events.values[0], &events.values[1], a, b).unwrap();

    // B = noise + 0.1·A: slope 0.1. Across A's positives from their 10th to
    // their 90th percentile - 5000 ± 1.28·1000, 2563 apart - that is 256
    // units: 2.56 of B's 100-wide negative, its width read within a tenth.
    assert!((slope - 0.1).abs() < 0.003, "{slope}");
    assert!((widths - 2.56).abs() < 0.3, "{widths}");
}

#[test]
fn the_same_lean_on_every_sample_is_the_matrix_with_the_value_to_change() {
    let stained: Vec<Measured> = (0..6)
        .map(|n| measured(&format!("s{n}"), 10 + n, 0.1, 0.0))
        .collect();

    let findings = check(&channels(), &stained, &[]);

    assert_eq!(findings.len(), 1, "{findings:?}");
    let Finding::Spillover {
        from,
        into,
        coefficient,
        samples,
        ..
    } = &findings[0]
    else {
        panic!("{findings:?}")
    };
    assert_eq!((&**from, &**into, *samples), ("A", "B", 6));
    assert!((coefficient - 0.1).abs() < 0.01, "{coefficient}");
    assert!(
        findings[0]
            .says()
            .contains("Change the A → B spillover value by +10."),
        "{}",
        findings[0].says()
    );
}

#[test]
fn a_lean_on_some_samples_only_is_theirs_not_the_matrix() {
    let stained: Vec<Measured> = (0..6)
        .map(|n| measured(&format!("s{n}"), 20 + n, if n < 2 { 0.1 } else { 0.0 }, 0.0))
        .collect();

    let findings = check(&channels(), &stained, &[]);

    assert_eq!(findings.len(), 1, "{findings:?}");
    let Finding::SampleSpecific {
        leaning, samples, ..
    } = &findings[0]
    else {
        panic!("{findings:?}")
    };
    let names: Vec<&str> = leaning.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(names, ["s0", "s1"]);
    assert_eq!(*samples, 6);
}

#[test]
fn large_granular_cells_brighter_in_one_channel_is_autofluorescence_confirmed_by_the_unstained() {
    // 300 units more in C for the top fifth by side scatter: 3 widths.
    let stained: Vec<Measured> = (0..4)
        .map(|n| measured(&format!("s{n}"), 30 + n, 0.0, 300.0))
        .collect();
    let unstained = vec![measured("u", 40, 0.0, 300.0)];
    let quiet = vec![measured("u", 41, 0.0, 0.0)];

    let with = check(&channels(), &stained, &unstained);
    let without = check(&channels(), &stained, &[]);
    let not_in_unstained = check(&channels(), &stained, &quiet);

    let af = |findings: &[Finding]| match findings {
        [
            Finding::Autofluorescence {
                channels,
                in_unstained,
                look_in,
            },
        ] => (channels.clone(), *in_unstained, look_in.clone()),
        other => panic!("{other:?}"),
    };
    let (channels, confirmed, look_in) = af(&with);
    assert_eq!(channels.len(), 1);
    assert_eq!(&*channels[0].0, "C");
    assert!((2.5..3.5).contains(&channels[0].1), "{channels:?}");
    assert_eq!(confirmed, Some(true));
    assert_eq!(&*look_in, "C");
    assert_eq!(af(&without).1, None);
    assert_eq!(af(&not_in_unstained).1, Some(false));
    assert!(with[0].says().contains("not a spillover error"));
}

#[test]
fn clean_data_finds_nothing() {
    let stained: Vec<Measured> = (0..4)
        .map(|n| measured(&format!("s{n}"), 50 + n, 0.0, 0.0))
        .collect();

    assert_eq!(
        check(&channels(), &stained, &[measured("u", 60, 0.0, 0.0)]),
        Vec::new()
    );
}

#[test]
fn u_and_unstained_in_any_case_are_unstained_and_nothing_else_is() {
    for unstained in [
        "U",
        "u",
        " U ",
        "Unstained",
        "UNSTAINED",
        "unstained control",
    ] {
        assert!(
            crate::spillover::is_unstained_type(unstained),
            "{unstained}"
        );
    }
    for stained in ["FS", "FMX", "US", "Ustain", "stained", ""] {
        assert!(!crate::spillover::is_unstained_type(stained), "{stained}");
    }
}
