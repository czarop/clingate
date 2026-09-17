//! Asserting tests for the gate-movement maths.
//!
//! The pre-existing `flow_tests` modules in this directory are exploratory: they
//! print their results and pass unconditionally, including when the call under
//! test returns `Err`. They are useful for eyeballing a scenario by hand, but
//! they cannot fail, so they cannot catch a regression. Everything here asserts.
//!
//! cargo test gate_move -- --nocapture

#![cfg(test)]

use crate::gate_move::density_grid::{DensityGrid, GateRules, apply_constraints, cross_correlate};
use crate::gate_move::kde::{kde_1d, kde_peak, silverman_bandwidth, std_dev};
use crate::gate_move::kde_shift::{
    DriftType, GateBoundary, analyse_population_shift, compute_smear_score,
};
use polars::prelude::*;
use rand::prelude::*;
use rand_distr::Normal;

// ─── Fixtures ─────────────────────────────────────────────────────────────────

/// A reproducible 2D gaussian blob.
fn blob(cx: f64, cy: f64, sx: f64, sy: f64, n: usize, rng: &mut StdRng) -> (Vec<f64>, Vec<f64>) {
    let dx = Normal::new(cx, sx).unwrap();
    let dy = Normal::new(cy, sy).unwrap();
    (0..n).map(|_| (dx.sample(rng), dy.sample(rng))).unzip()
}

fn join(a: (Vec<f64>, Vec<f64>), b: (Vec<f64>, Vec<f64>)) -> (Vec<f64>, Vec<f64>) {
    let (mut xs, mut ys) = a;
    xs.extend(b.0);
    ys.extend(b.1);
    (xs, ys)
}

fn frame(points: (Vec<f64>, Vec<f64>)) -> DataFrame {
    df!["x" => points.0, "y" => points.1].unwrap()
}

fn cols(df: &DataFrame) -> (&Column, &Column) {
    (df.column("x").unwrap(), df.column("y").unwrap())
}

/// A negative population at (0.4, 0.4) and a positive at (2.5, 2.5), with the
/// whole sample optionally translated - the shape a QC/test pair takes.
fn sample(seed: u64, shift_x: f64, shift_y: f64) -> DataFrame {
    let mut rng = StdRng::seed_from_u64(seed);
    let neg = blob(0.4 + shift_x, 0.4 + shift_y, 0.12, 0.12, 3000, &mut rng);
    let pos = blob(2.5 + shift_x, 2.5 + shift_y, 0.20, 0.20, 800, &mut rng);
    frame(join(neg, pos))
}

const AXIS: (f64, f64) = (-1.0, 4.5);

fn gate() -> GateBoundary {
    GateBoundary {
        x_lower: 1.2,
        y_lower: 1.2,
    }
}

fn analyse(
    qc: &DataFrame,
    test: &DataFrame,
) -> Result<crate::gate_move::kde_shift::PopulationShiftResult, String> {
    analyse_population_shift(
        cols(qc),
        cols(test),
        AXIS,
        AXIS,
        &gate(),
        0.1, // negative_margin
        512, // n_kde_points
        50,  // min_events
        0.1, // significant_shift
        1.5, // significant_width_ratio
    )
}

// ─── kde_1d ───────────────────────────────────────────────────────────────────

#[test]
fn kde_1d_returns_a_grid_spanning_the_requested_range() {
    let (xs, density) = kde_1d(&[0.0, 1.0], (-2.0, 3.0), 64, 0.3);

    assert_eq!(xs.len(), 64);
    assert_eq!(density.len(), 64);
    assert!(
        (xs[0] - -2.0).abs() < 1e-12,
        "grid must start at the range start"
    );
    assert!(
        (xs[63] - 3.0).abs() < 1e-12,
        "grid must end at the range end"
    );
}

#[test]
fn kde_1d_recovers_the_centre_of_a_single_cluster() {
    let mut rng = StdRng::seed_from_u64(1);
    let d = Normal::new(1.75, 0.15).unwrap();
    let values: Vec<f64> = (0..4000).map(|_| d.sample(&mut rng)).collect();

    let bw = silverman_bandwidth(&values);
    let (xs, density) = kde_1d(&values, AXIS, 512, bw);

    assert!(
        (kde_peak(&xs, &density) - 1.75).abs() < 0.05,
        "peak should land on the generating mean"
    );
}

#[test]
fn kde_1d_integrates_to_approximately_one() {
    let mut rng = StdRng::seed_from_u64(2);
    let d = Normal::new(0.0, 0.4).unwrap();
    let values: Vec<f64> = (0..5000).map(|_| d.sample(&mut rng)).collect();

    let (xs, density) = kde_1d(&values, (-4.0, 4.0), 1024, silverman_bandwidth(&values));
    let step = xs[1] - xs[0];
    let mass: f64 = density.iter().sum::<f64>() * step;

    assert!(
        (mass - 1.0).abs() < 0.02,
        "density integrated to {mass}, expected ~1"
    );
}

#[test]
fn kde_1d_density_is_never_negative() {
    let (_, density) = kde_1d(&[0.0, 0.5, 1.0], (-1.0, 2.0), 128, 0.2);
    assert!(density.iter().all(|d| *d >= 0.0));
}

// Regression: an empty population divided by `points.len()` and filled the grid
// with NaN, which propagated silently into every downstream peak and shift.
#[test]
fn kde_1d_returns_zeros_for_an_empty_population() {
    let (xs, density) = kde_1d(&[], (0.0, 1.0), 32, 0.1);

    assert_eq!(xs.len(), 32);
    assert!(
        density.iter().all(|d| *d == 0.0),
        "expected zeros, got {density:?}"
    );
}

// Regression: a constant population yields a zero bandwidth, which divided by
// zero inside the kernel.
#[test]
fn kde_1d_survives_a_degenerate_bandwidth() {
    for bw in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let (_, density) = kde_1d(&[1.0, 2.0], (0.0, 3.0), 16, bw);
        assert!(
            density.iter().all(|d| d.is_finite()),
            "bandwidth {bw} produced non-finite density"
        );
    }
}

// Regression: `n_points - 1` underflowed at 0 and made the step infinite at 1.
#[test]
fn kde_1d_survives_a_degenerate_grid_size() {
    for n in [0usize, 1, 2] {
        let (xs, density) = kde_1d(&[0.5], (0.0, 1.0), n, 0.1);
        assert_eq!(xs.len(), density.len());
        assert!(xs.len() >= 2);
        assert!(xs.iter().all(|x| x.is_finite()));
    }
}

// ─── kde_peak ─────────────────────────────────────────────────────────────────

#[test]
fn kde_peak_picks_the_position_of_the_highest_density() {
    let xs = vec![0.0, 1.0, 2.0, 3.0];
    let density = vec![0.1, 0.9, 0.4, 0.2];
    assert_eq!(kde_peak(&xs, &density), 1.0);
}

// Regression: `unwrap_or` evaluates its argument eagerly, so the midpoint
// fallback indexed an empty slice and panicked.
#[test]
fn kde_peak_of_an_empty_grid_is_nan_not_a_panic() {
    assert!(kde_peak(&[], &[]).is_nan());
}

// Regression: `partial_cmp().unwrap()` panicked the moment any density was NaN.
#[test]
fn kde_peak_ignores_non_finite_densities() {
    let xs = vec![0.0, 1.0, 2.0];
    let density = vec![f64::NAN, 0.5, f64::INFINITY];
    assert_eq!(kde_peak(&xs, &density), 1.0);
}

#[test]
fn kde_peak_falls_back_to_the_midpoint_when_nothing_is_finite() {
    let xs = vec![0.0, 1.0, 2.0];
    let density = vec![f64::NAN, f64::NAN, f64::NAN];
    assert_eq!(kde_peak(&xs, &density), 1.0);
}

// ─── silverman_bandwidth / std_dev ────────────────────────────────────────────

#[test]
fn silverman_bandwidth_is_positive_and_finite_for_normal_data() {
    let mut rng = StdRng::seed_from_u64(3);
    let d = Normal::new(0.0, 1.0).unwrap();
    let values: Vec<f64> = (0..1000).map(|_| d.sample(&mut rng)).collect();

    let bw = silverman_bandwidth(&values);
    assert!(bw > 0.0 && bw.is_finite(), "bandwidth was {bw}");
}

#[test]
fn silverman_bandwidth_shrinks_as_the_population_tightens() {
    let mut rng = StdRng::seed_from_u64(4);
    let wide: Vec<f64> = (0..1000)
        .map(|_| Normal::new(0.0, 1.0).unwrap().sample(&mut rng))
        .collect();
    let tight: Vec<f64> = (0..1000)
        .map(|_| Normal::new(0.0, 0.1).unwrap().sample(&mut rng))
        .collect();

    assert!(silverman_bandwidth(&tight) < silverman_bandwidth(&wide));
}

// Regression: an identical-valued population gave an IQR of zero and so a
// bandwidth of zero, which made kde_1d degenerate.
#[test]
fn silverman_bandwidth_stays_positive_for_a_constant_population() {
    let bw = silverman_bandwidth(&[2.0; 500]);
    assert!(bw > 0.0 && bw.is_finite(), "bandwidth was {bw}");
}

// A population whose middle 50% is identical but whose tails are not: the IQR
// estimate collapses while the standard deviation does not.
#[test]
fn silverman_bandwidth_stays_positive_when_only_the_iqr_collapses() {
    let mut values = vec![5.0; 100];
    values[0] = 0.0;
    values[99] = 10.0;

    let bw = silverman_bandwidth(&values);
    assert!(bw > 0.0 && bw.is_finite(), "bandwidth was {bw}");
}

#[test]
fn silverman_bandwidth_of_a_degenerate_sample_is_one() {
    assert_eq!(silverman_bandwidth(&[]), 1.0);
    assert_eq!(silverman_bandwidth(&[1.0]), 1.0);
}

#[test]
fn std_dev_matches_the_sample_standard_deviation() {
    // Sample sd of 2,4,4,4,5,5,7,9 is 2.138... (n-1 denominator).
    let sd = std_dev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]);
    assert!((sd - 2.1380899).abs() < 1e-6, "got {sd}");
}

#[test]
fn std_dev_of_a_degenerate_sample_is_zero() {
    assert_eq!(std_dev(&[]), 0.0);
    assert_eq!(std_dev(&[3.0]), 0.0);
    assert_eq!(std_dev(&[3.0; 10]), 0.0);
}

// ─── analyse_population_shift ─────────────────────────────────────────────────

#[test]
fn identical_samples_report_no_shift_and_read_as_clean() {
    let qc = sample(10, 0.0, 0.0);
    let result = analyse(&qc, &qc).expect("identical samples should analyse");

    assert!(
        result.negative_dx.abs() < 0.05,
        "dx was {}",
        result.negative_dx
    );
    assert!(
        result.negative_dy.abs() < 0.05,
        "dy was {}",
        result.negative_dy
    );
    assert!((result.width_ratio_x - 1.0).abs() < 0.01);
    assert!(matches!(result.drift_type, DriftType::Clean));
}

#[test]
fn a_uniform_translation_is_recovered_on_both_axes() {
    let qc = sample(11, 0.0, 0.0);
    let test = sample(12, 0.30, 0.20);
    let result = analyse(&qc, &test).expect("shifted samples should analyse");

    assert!(
        (result.negative_dx - 0.30).abs() < 0.08,
        "expected dx ~0.30, got {}",
        result.negative_dx
    );
    assert!(
        (result.negative_dy - 0.20).abs() < 0.08,
        "expected dy ~0.20, got {}",
        result.negative_dy
    );
}

#[test]
fn the_shift_sign_follows_the_direction_of_travel() {
    let qc = sample(13, 0.0, 0.0);
    let right = analyse(&qc, &sample(14, 0.25, 0.0)).unwrap();
    let left = analyse(&qc, &sample(15, -0.25, 0.0)).unwrap();

    assert!(
        right.negative_dx > 0.1,
        "rightward shift should be positive"
    );
    assert!(left.negative_dx < -0.1, "leftward shift should be negative");
}

#[test]
fn a_shift_on_one_axis_leaves_the_other_axis_alone() {
    let qc = sample(16, 0.0, 0.0);
    let result = analyse(&qc, &sample(17, 0.30, 0.0)).unwrap();

    assert!((result.negative_dx - 0.30).abs() < 0.08);
    assert!(
        result.negative_dy.abs() < 0.08,
        "y should not move, got {}",
        result.negative_dy
    );
}

#[test]
fn a_widened_negative_is_flagged_by_the_width_ratio() {
    let mut rng = StdRng::seed_from_u64(18);
    let qc = frame(join(
        blob(0.4, 0.4, 0.12, 0.12, 3000, &mut rng),
        blob(2.5, 2.5, 0.20, 0.20, 800, &mut rng),
    ));
    let test = frame(join(
        blob(0.4, 0.4, 0.35, 0.12, 3000, &mut rng), // three times wider on x
        blob(2.5, 2.5, 0.20, 0.20, 800, &mut rng),
    ));

    let result = analyse(&qc, &test).unwrap();

    assert!(
        result.width_ratio_x > 1.5,
        "x width ratio was {}",
        result.width_ratio_x
    );
    assert!(result.width_warning_x(1.5));
    assert!(!result.width_warning_y(1.5), "y should not be flagged");
}

#[test]
fn too_few_negative_events_is_an_error_not_a_guess() {
    let mut rng = StdRng::seed_from_u64(19);
    // Everything sits above the gate boundary, so the negative region is empty.
    let sparse = frame(blob(3.0, 3.0, 0.1, 0.1, 500, &mut rng));
    let qc = sample(20, 0.0, 0.0);

    assert!(
        analyse(&qc, &sparse).is_err(),
        "an empty test negative must error"
    );
    assert!(
        analyse(&sparse, &qc).is_err(),
        "an empty QC negative must error"
    );
}

#[test]
fn a_missing_positive_population_degrades_gracefully() {
    let mut rng = StdRng::seed_from_u64(21);
    // Negative only - nothing above the gate boundary in either sample.
    let neg_only = frame(blob(0.4, 0.4, 0.12, 0.12, 3000, &mut rng));
    let result = analyse(&neg_only, &neg_only).expect("negative-only should still analyse");

    assert!(
        result.positive_dx.is_none(),
        "no positive events to measure"
    );
    assert!(result.positive_dy.is_none());
    // The negative is still measurable.
    assert!(result.negative_dx.is_finite());
}

#[test]
fn a_positive_appearing_only_in_the_test_reads_as_biological() {
    let mut rng = StdRng::seed_from_u64(22);
    let qc = frame(blob(0.4, 0.4, 0.12, 0.12, 3000, &mut rng));
    let test = frame(join(
        blob(0.4, 0.4, 0.12, 0.12, 3000, &mut rng),
        blob(2.5, 2.5, 0.20, 0.20, 800, &mut rng),
    ));

    let result = analyse(&qc, &test).unwrap();
    assert!(matches!(result.drift_type, DriftType::Biological));
}

#[test]
fn every_reported_figure_is_finite() {
    let result = analyse(&sample(23, 0.0, 0.0), &sample(24, 0.15, -0.1)).unwrap();

    for (label, v) in [
        ("negative_dx", result.negative_dx),
        ("negative_dy", result.negative_dy),
        ("qc_negative_width_x", result.qc_negative_width_x),
        ("test_negative_width_x", result.test_negative_width_x),
        ("width_ratio_x", result.width_ratio_x),
        ("width_ratio_y", result.width_ratio_y),
    ] {
        assert!(v.is_finite(), "{label} was {v}");
    }
}

// ─── compute_smear_score ──────────────────────────────────────────────────────

#[test]
fn a_tight_cluster_scores_lower_than_a_uniform_smear() {
    let mut rng = StdRng::seed_from_u64(25);
    let tight: Vec<f64> = (0..2000)
        .map(|_| Normal::new(2.5, 0.08).unwrap().sample(&mut rng))
        .collect();
    let smeared: Vec<f64> = (0..2000).map(|_| rng.random_range(1.2..4.5)).collect();

    let (_, tight_d) = kde_1d(&tight, (1.2, 4.5), 256, silverman_bandwidth(&tight));
    let (_, smear_d) = kde_1d(&smeared, (1.2, 4.5), 256, silverman_bandwidth(&smeared));

    let tight_score = compute_smear_score(&tight, &tight_d, 3.3);
    let smear_score = compute_smear_score(&smeared, &smear_d, 3.3);

    assert!(
        smear_score > tight_score,
        "smear {smear_score} should exceed tight {tight_score}"
    );
}

#[test]
fn the_smear_score_stays_within_its_documented_bounds() {
    let mut rng = StdRng::seed_from_u64(26);
    for spread in [0.05, 0.5, 2.0] {
        let values: Vec<f64> = (0..500)
            .map(|_| Normal::new(2.0, spread).unwrap().sample(&mut rng))
            .collect();
        let (_, density) = kde_1d(&values, (-1.0, 5.0), 128, silverman_bandwidth(&values));
        let score = compute_smear_score(&values, &density, 6.0);

        assert!((0.0..=1.0).contains(&score), "score {score} out of range");
    }
}

#[test]
fn the_smear_score_of_a_degenerate_input_is_zero() {
    assert_eq!(compute_smear_score(&[], &[], 1.0), 0.0);
    assert_eq!(compute_smear_score(&[1.0, 2.0], &[0.5, 0.5], 1.0), 0.0);
    assert_eq!(compute_smear_score(&[1.0, 2.0, 3.0, 4.0], &[], 1.0), 0.0);
}

// ─── DensityGrid ──────────────────────────────────────────────────────────────

#[test]
fn density_grid_bins_every_in_range_event_exactly_once() {
    let df = frame((vec![0.5, 1.5, 2.5], vec![0.5, 1.5, 2.5]));
    let (xs, ys) = cols(&df);
    let grid = DensityGrid::from_column(xs, ys, 4, (0.0, 4.0), (0.0, 4.0));

    assert_eq!(grid.counts.len(), 16);
    assert_eq!(grid.counts.iter().sum::<f32>(), 3.0);
}

#[test]
fn density_grid_discards_events_outside_the_range() {
    let df = frame((vec![-5.0, 0.5, 99.0], vec![0.5, 0.5, 0.5]));
    let (xs, ys) = cols(&df);
    let grid = DensityGrid::from_column(xs, ys, 4, (0.0, 4.0), (0.0, 4.0));

    assert_eq!(
        grid.counts.iter().sum::<f32>(),
        1.0,
        "only the in-range event counts"
    );
}

#[test]
fn density_grid_reports_its_bin_widths() {
    let df = frame((vec![0.5], vec![0.5]));
    let (xs, ys) = cols(&df);
    let grid = DensityGrid::from_column(xs, ys, 10, (0.0, 5.0), (-2.0, 3.0));

    assert!((grid.bin_width_x() - 0.5).abs() < 1e-12);
    assert!((grid.bin_width_y() - 0.5).abs() < 1e-12);
}

#[test]
fn density_grid_places_an_event_in_the_expected_cell() {
    // 4 bins over 0..4, so an event at (2.5, 0.5) lands at col 2, row 0.
    let df = frame((vec![2.5], vec![0.5]));
    let (xs, ys) = cols(&df);
    let grid = DensityGrid::from_column(xs, ys, 4, (0.0, 4.0), (0.0, 4.0));

    assert_eq!(grid.counts[0 * 4 + 2], 1.0);
}

// ─── cross_correlate ──────────────────────────────────────────────────────────

#[test]
fn cross_correlation_of_a_grid_with_itself_is_zero_translation() {
    let mut rng = StdRng::seed_from_u64(27);
    let df = frame(blob(0.0, 0.0, 0.5, 0.5, 4000, &mut rng));
    let (xs, ys) = cols(&df);
    let grid = DensityGrid::from_column(xs, ys, 64, (-4.0, 4.0), (-4.0, 4.0));

    let t = cross_correlate(&grid, &grid);

    assert_eq!(t.dx_bins, 0);
    assert_eq!(t.dy_bins, 0);
}

#[test]
fn cross_correlation_recovers_a_known_translation() {
    let mut rng = StdRng::seed_from_u64(28);
    let qc_pts = blob(0.0, 0.0, 0.4, 0.4, 6000, &mut rng);
    // Translate the same cloud by a whole number of bins: 8 bins over 8 units
    // at 64 bins is 1.0 in data space.
    let shifted = (
        qc_pts.0.iter().map(|x| x + 1.0).collect::<Vec<_>>(),
        qc_pts.1.iter().map(|y| y - 0.5).collect::<Vec<_>>(),
    );

    let qc_df = frame(qc_pts);
    let test_df = frame(shifted);
    let (qx, qy) = cols(&qc_df);
    let (tx, ty) = cols(&test_df);

    let qc = DensityGrid::from_column(qx, qy, 64, (-4.0, 4.0), (-4.0, 4.0));
    let test = DensityGrid::from_column(tx, ty, 64, (-4.0, 4.0), (-4.0, 4.0));

    let t = cross_correlate(&qc, &test);

    assert!(
        (t.dx_data - 1.0).abs() < 0.2,
        "expected dx ~1.0, got {}",
        t.dx_data
    );
    assert!(
        (t.dy_data - -0.5).abs() < 0.2,
        "expected dy ~-0.5, got {}",
        t.dy_data
    );
}

#[test]
fn cross_correlation_converts_bins_to_data_units_via_the_bin_width() {
    let mut rng = StdRng::seed_from_u64(29);
    let df = frame(blob(0.0, 0.0, 0.4, 0.4, 2000, &mut rng));
    let (xs, ys) = cols(&df);
    let grid = DensityGrid::from_column(xs, ys, 32, (-4.0, 4.0), (-4.0, 4.0));

    let t = cross_correlate(&grid, &grid);

    assert!((t.dx_data - t.dx_bins as f64 * grid.bin_width_x()).abs() < 1e-12);
    assert!((t.dy_data - t.dy_bins as f64 * grid.bin_width_y()).abs() < 1e-12);
}

// ─── apply_constraints ────────────────────────────────────────────────────────

/// The rules type is the seed of the eventual autogating ruleset, so its
/// clamping behaviour is worth pinning down now.
fn translation(dx: f64, dy: f64) -> crate::gate_move::density_grid::TranslationVector {
    crate::gate_move::density_grid::TranslationVector {
        dx_bins: 10,
        dy_bins: 10,
        dx_data: dx,
        dy_data: dy,
        peak_strength: 1.0,
    }
}

#[test]
fn locking_an_axis_zeroes_that_axis_only() {
    let rules = GateRules {
        lock_x: true,
        lock_y: false,
        max_translation: None,
    };
    let t = apply_constraints(translation(0.8, 0.6), &rules);

    assert_eq!(t.dx_data, 0.0);
    assert_eq!(t.dx_bins, 0);
    assert_eq!(t.dy_data, 0.6);
    assert_eq!(t.dy_bins, 10);
}

#[test]
fn locking_both_axes_pins_the_gate() {
    let rules = GateRules {
        lock_x: true,
        lock_y: true,
        max_translation: None,
    };
    let t = apply_constraints(translation(0.8, 0.6), &rules);

    assert_eq!((t.dx_data, t.dy_data), (0.0, 0.0));
}

#[test]
fn max_translation_scales_an_overlong_move_back_to_the_limit() {
    let rules = GateRules {
        lock_x: false,
        lock_y: false,
        max_translation: Some(0.5),
    };
    // Magnitude of (0.6, 0.8) is exactly 1.0, so it should scale to 0.5.
    let t = apply_constraints(translation(0.6, 0.8), &rules);
    let magnitude = (t.dx_data.powi(2) + t.dy_data.powi(2)).sqrt();

    assert!((magnitude - 0.5).abs() < 1e-9, "magnitude was {magnitude}");
    // Direction must be preserved.
    assert!((t.dx_data / t.dy_data - 0.75).abs() < 1e-9);
}

#[test]
fn max_translation_leaves_a_short_move_untouched() {
    let rules = GateRules {
        lock_x: false,
        lock_y: false,
        max_translation: Some(5.0),
    };
    let t = apply_constraints(translation(0.6, 0.8), &rules);

    assert_eq!((t.dx_data, t.dy_data), (0.6, 0.8));
}
