//! Tests for describing a population by its phenotype and finding it again.

#![cfg(test)]

use super::phenotype::*;
use std::sync::Arc;

fn markers(names: &[&str]) -> Vec<Arc<str>> {
    names.iter().map(|n| Arc::from(*n)).collect()
}

/// A population scattered round a centre, deterministic so a failure is
/// reproducible. A plain linear congruential generator: the distribution only
/// has to be a cloud, not a good normal.
struct Cloud(u64);

impl Cloud {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        // Two uniforms averaged, centred: enough of a bell for these tests.
        let a = ((self.0 >> 32) as u32) as f64 / u32::MAX as f64;
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let b = ((self.0 >> 32) as u32) as f64 / u32::MAX as f64;
        a + b - 1.0
    }

    fn around(&mut self, centre: &[f64], spread: f64, n: usize) -> Vec<Vec<f64>> {
        (0..n)
            .map(|_| centre.iter().map(|c| c + self.next() * spread).collect())
            .collect()
    }
}

// ── the arithmetic ───────────────────────────────────────────────────────

#[test]
fn a_robust_baseline_ignores_the_bright_tail() {
    // The parent contains the very population being described. A mean and a
    // standard deviation would be dragged by it; that is the whole reason for
    // median and MAD.
    let mut values: Vec<f64> = (0..95).map(|i| i as f64 / 95.0).collect();
    values.extend((0..5).map(|_| 1000.0));
    let base = Baseline::of(&values);
    assert!(
        base.median < 1.0,
        "the median sits in the bulk, got {}",
        base.median
    );
    assert!(
        base.spread < 2.0,
        "the spread is the bulk's, got {}",
        base.spread
    );
}

#[test]
fn a_marker_with_no_spread_does_not_make_every_cell_infinite() {
    // An empty or saturated detector has a MAD of zero. Dividing by it would
    // put every cell infinitely far from the middle.
    let base = Baseline::of(&[3.0; 200]);
    assert!(base.spread > 0.0);
    assert!(base.z(3.0).is_finite());
    assert!(base.z(4.0).is_finite());
}

#[test]
fn inverting_a_covariance_returns_its_inverse() {
    // A 2x2 with a known inverse, checked by multiplying back to the identity.
    let m = vec![4.0, 1.0, 1.0, 3.0];
    let inv = invert_spd(&m, 2, 0.0).expect("positive definite");
    let mut product = vec![0.0; 4];
    for i in 0..2 {
        for j in 0..2 {
            for k in 0..2 {
                product[i * 2 + j] += m[i * 2 + k] * inv[k * 2 + j];
            }
        }
    }
    for (at, expected) in [1.0, 0.0, 0.0, 1.0].iter().enumerate() {
        assert!(
            (product[at] - expected).abs() < 1e-9,
            "entry {at} was {}",
            product[at]
        );
    }
}

#[test]
fn two_markers_that_move_together_do_not_blow_up_the_inverse() {
    // Panels are full of near-duplicate markers, which make the covariance
    // singular. Without the ridge this has no inverse at all.
    let singular = vec![1.0, 1.0, 1.0, 1.0];
    assert!(
        invert_spd(&singular, 2, 0.0).is_none(),
        "singular without a ridge"
    );
    let inv = invert_spd(&singular, 2, RIDGE).expect("the ridge makes it invertible");
    assert!(inv.iter().all(|v| v.is_finite()));
}

#[test]
fn distance_is_measured_in_the_populations_own_shape() {
    // A population wide in the first marker and narrow in the second. The same
    // step should count for much less along the wide direction.
    let signature = Signature {
        markers: markers(&["wide", "narrow"]),
        centre: vec![0.0, 0.0],
        precision: invert_spd(&[100.0, 0.0, 0.0, 1.0], 2, 0.0).expect("invertible"),
        cut: f64::INFINITY,
        members: 100,
    };
    let along_wide = distance_squared(&signature, &[1.0, 0.0]);
    let along_narrow = distance_squared(&signature, &[0.0, 1.0]);
    assert!(
        along_narrow > along_wide * 50.0,
        "wide {along_wide}, narrow {along_narrow}"
    );
}

// ── describing and finding a population ──────────────────────────────────

#[test]
fn a_population_finds_itself() {
    let mut rng = Cloud(1);
    let mut parent = rng.around(&[0.0, 0.0, 0.0], 1.0, 800);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 120);
    parent.extend(population.clone());

    let signature = Signature::describe(markers(&["a", "b", "c"]), &population, &parent)
        .expect("a signature can be described");
    let found = signature.find_in(&parent);

    // Every member should be found, and few others.
    assert!(
        found.members.len() >= 110,
        "found only {} of 120",
        found.members.len()
    );
    assert!(
        found.members.len() < 160,
        "found {} - it is taking in the background",
        found.members.len()
    );
}

#[test]
fn a_population_is_found_where_the_whole_panel_moved() {
    // The donor case: the same cells, every marker shifted, and the spread
    // changed too. Because each sample is baselined against its own parent,
    // none of that should matter.
    let mut rng = Cloud(2);
    let mut reference = rng.around(&[0.0, 0.0, 0.0], 1.0, 800);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 120);
    reference.extend(population.clone());
    let signature =
        Signature::describe(markers(&["a", "b", "c"]), &population, &reference).expect("described");

    // The same structure, shifted and stretched as a different donor's would be.
    let shift = [3.0, -2.0, 1.5];
    let stretch = 1.6;
    let moved = |rows: &[Vec<f64>]| -> Vec<Vec<f64>> {
        rows.iter()
            .map(|row| {
                row.iter()
                    .zip(shift.iter())
                    .map(|(v, s)| v * stretch + s)
                    .collect()
            })
            .collect()
    };
    let sample = moved(&reference);
    let expected: Vec<Vec<f64>> = moved(&population);

    let found = signature.find_in(&sample);
    assert!(
        found.members.len() >= 100,
        "found only {} of 120 after the panel moved",
        found.members.len()
    );
    // And they are the right cells: the members sit where the moved population
    // does, not where the background does.
    let centre = expected[0][0];
    let matched_first: Vec<f64> = found.members.iter().map(|at| sample[*at][0]).collect();
    let mean = matched_first.iter().sum::<f64>() / matched_first.len() as f64;
    assert!(
        (mean - centre).abs() < 3.0,
        "matched cells centre on {mean}, the population is near {centre}"
    );
}

#[test]
fn a_rarer_population_is_still_found() {
    // 7% in the reference against 1% in the sample: the question that defeats
    // anything matching on density. A phenotype does not care how many there
    // are.
    let mut rng = Cloud(3);
    let mut reference = rng.around(&[0.0, 0.0, 0.0], 1.0, 930);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 70);
    reference.extend(population.clone());
    let signature =
        Signature::describe(markers(&["a", "b", "c"]), &population, &reference).expect("described");

    let mut sample = rng.around(&[0.0, 0.0, 0.0], 1.0, 990);
    let rare = rng.around(&[6.0, 0.0, -4.0], 0.4, 10);
    sample.extend(rare);

    let found = signature.find_in(&sample);
    assert!(
        found.members.len() >= 8,
        "found only {} of 10",
        found.members.len()
    );
    assert!(
        found.fraction() < 0.05,
        "it took in {:.1}% of the parent",
        found.fraction() * 100.0
    );
}

#[test]
fn a_population_that_is_not_there_matches_almost_nothing() {
    // The failure that has to be visible. When the population is absent the
    // nearest cells are still *something*, so the count is what says so - and
    // it must not quietly come back full.
    let mut rng = Cloud(4);
    let mut reference = rng.around(&[0.0, 0.0, 0.0], 1.0, 800);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 120);
    reference.extend(population.clone());
    let signature =
        Signature::describe(markers(&["a", "b", "c"]), &population, &reference).expect("described");

    // Background only.
    let sample = rng.around(&[0.0, 0.0, 0.0], 1.0, 900);
    let found = signature.find_in(&sample);
    assert!(
        found.fraction() < 0.02,
        "matched {:.1}% of a sample with no such population",
        found.fraction() * 100.0
    );
}

#[test]
fn a_signature_needs_members() {
    let parent = vec![vec![0.0, 0.0]; 10];
    assert!(Signature::describe(markers(&["a", "b"]), &[], &parent).is_none());
}

#[test]
fn a_signature_remembers_how_many_cells_described_it() {
    let mut rng = Cloud(5);
    let parent = rng.around(&[0.0, 0.0], 1.0, 200);
    let population = rng.around(&[3.0, 3.0], 0.3, 17);
    let signature =
        Signature::describe(markers(&["a", "b"]), &population, &parent).expect("described");
    assert_eq!(signature.members, 17);
}

// ── the numerical helpers ────────────────────────────────────────────────

#[test]
fn the_normal_quantile_matches_the_table() {
    for (p, expected) in [
        (0.5, 0.0),
        (0.95, 1.644_854),
        (0.975, 1.959_964),
        (0.99, 2.326_348),
        (0.025, -1.959_964),
    ] {
        let got = normal_quantile(p);
        assert!(
            (got - expected).abs() < 1e-5,
            "quantile at {p} was {got}, expected {expected}"
        );
    }
}

#[test]
fn the_chi_squared_quantile_matches_the_table() {
    // Wilson-Hilferty is an approximation; it is weakest at one degree of
    // freedom and improves quickly. Within a percent or two is far inside what
    // the covariance it is applied to can claim.
    for (k, expected) in [
        (1, 3.841),
        (2, 5.991),
        (3, 7.815),
        (5, 11.070),
        (10, 18.307),
    ] {
        let got = chi_squared_quantile(k, 0.95);
        let error = (got - expected).abs() / expected;
        assert!(
            error < 0.03,
            "chi2({k}, 0.95) was {got}, expected {expected}"
        );
    }
}

// ── the headline claim ───────────────────────────────────────────────────

/// How common a population is must not change where it is found.
///
/// This is the whole reason the baseline is taken with the tail cut off. The
/// parent contains the population, so a plain robust spread is inflated by it
/// in proportion to how much of it there is - and that is exactly what differs
/// between one donor and the next. Untreated, the same cells read several
/// widths lower in a sample that has more of them.
#[test]
fn how_common_the_population_is_does_not_move_where_it_sits() {
    let centre_at = |fraction: f64| -> f64 {
        let mut rng = Cloud(9);
        let total = 2000;
        let members = (total as f64 * fraction) as usize;
        let mut parent = rng.around(&[0.0], 1.0, total - members);
        parent.extend(rng.around(&[6.0], 0.4, members));
        let base = &baselines(&parent, 1)[0];
        base.z(6.0)
    };

    let rare = centre_at(0.01);
    let common = centre_at(0.20);
    assert!(
        (rare - common).abs() / rare < 0.08,
        "the same cells read {rare:.2} at 1% and {common:.2} at 20%"
    );
}

#[test]
fn an_untrimmed_baseline_would_have_moved_it() {
    // The control for the test above: without cutting the tail off, the same
    // measurement drifts with abundance. If this ever stops being true the
    // trimming has become unnecessary - and the test above would no longer be
    // evidence of anything.
    let untrimmed_z = |fraction: f64| -> f64 {
        let mut rng = Cloud(9);
        let total = 2000;
        let members = (total as f64 * fraction) as usize;
        let mut parent = rng.around(&[0.0], 1.0, total - members);
        parent.extend(rng.around(&[6.0], 0.4, members));
        let mut column: Vec<f64> = parent.iter().map(|r| r[0]).collect();
        column.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = column[column.len() / 2];
        let mut deviations: Vec<f64> = column.iter().map(|v| (v - median).abs()).collect();
        deviations.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let spread = deviations[deviations.len() / 2] * 1.4826;
        (6.0 - median) / spread
    };
    let rare = untrimmed_z(0.01);
    let common = untrimmed_z(0.20);
    assert!(
        (rare - common).abs() / rare > 0.15,
        "untrimmed read {rare:.2} at 1% and {common:.2} at 20% - too close for this to be the reason trimming exists"
    );
}
