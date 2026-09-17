use polars::prelude::*;

// ─── 1D KDE ──────────────────────────────────────────────────────────────────
// this module examines the negative peak to determine shifts and or widening/narrowing

/// Evaluates a 1D KDE over `points` at `n_points` evenly spaced positions
/// across `range`, using a Gaussian kernel with the given bandwidth.
/// Returns (grid of x positions, grid of density values).
pub fn kde_1d(
    points: &[f64],
    range: (f64, f64),
    n_points: usize,
    bandwidth: f64,
) -> (Vec<f64>, Vec<f64>) {
    // n_points of 0 would underflow the step divisor, and 1 would make it infinite.
    let n_points = n_points.max(2);
    let step = (range.1 - range.0) / (n_points - 1) as f64;
    let xs: Vec<f64> = (0..n_points).map(|i| range.0 + i as f64 * step).collect();

    // A degenerate bandwidth - a constant population, or a caller-supplied zero -
    // divides by zero and fills the grid with NaN, which then propagates silently
    // into every peak, shift and drift classification downstream. An empty input
    // does the same via the sum/len at the end.
    if points.is_empty() || !bandwidth.is_finite() || bandwidth <= 0.0 {
        return (xs, vec![0.0; n_points]);
    }

    let norm = 1.0 / (bandwidth * (2.0 * std::f64::consts::PI).sqrt());

    let density: Vec<f64> = xs
        .iter()
        .map(|&x| {
            let sum: f64 = points
                .iter()
                .map(|&p| {
                    let z = (x - p) / bandwidth;
                    norm * (-0.5 * z * z).exp()
                })
                .sum();
            sum / points.len() as f64
        })
        .collect();

    (xs, density)
}

/// Finds the x position of the highest density peak in a KDE output.
///
/// Returns `NaN` for an empty grid. Non-finite densities are ignored rather than
/// compared - `partial_cmp().unwrap()` panics on NaN, and the midpoint fallback
/// indexed an empty slice because `unwrap_or` evaluates its argument eagerly.
pub fn kde_peak(xs: &[f64], density: &[f64]) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }

    density
        .iter()
        .take(xs.len())
        .enumerate()
        .filter(|(_, d)| d.is_finite())
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(i, _)| xs[i])
        .unwrap_or_else(|| xs[xs.len() / 2])
}

/// Silverman's rule of thumb for bandwidth selection.
/// A reasonable automatic choice for unimodal roughly-normal distributions.
pub fn silverman_bandwidth(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    if n < 2.0 {
        return 1.0;
    }

    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std = variance.sqrt();

    // IQR-based robust std estimate (avoids inflation from outliers/tail)
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let q1 = sorted[(n * 0.25) as usize];
    let q3 = sorted[(n * 0.75) as usize];
    let iqr_std = (q3 - q1) / 1.34;

    let s = std.min(iqr_std);
    let bandwidth = 0.9 * s * n.powf(-0.2);
    if bandwidth.is_finite() && bandwidth > 0.0 {
        return bandwidth;
    }

    // A population whose middle 50% is identical gives an IQR of 0 and so a
    // bandwidth of 0, which would make kde_1d degenerate. Fall back to the
    // non-robust estimate, then to a small positive width.
    let unrobust = 0.9 * std * n.powf(-0.2);
    if unrobust.is_finite() && unrobust > 0.0 {
        return unrobust;
    }

    f64::EPSILON.sqrt()
}

// ─── Result types ─────────────────────────────────────────────────────────────

pub struct NegativePopulationShift {
    /// How far the negative peak moved on each axis (data-space units).
    pub dx: f64,
    pub dy: f64,
    /// Width (std dev) of the negative population in the QC, in data-space units.
    pub qc_width_x: f64,
    pub qc_width_y: f64,
    /// Width in the test sample — compare against qc_width to detect spread changes.
    pub test_width_x: f64,
    pub test_width_y: f64,
    /// Ratio of test/qc width. >1.5 on either axis is worth flagging to the user.
    pub width_ratio_x: f64,
    pub width_ratio_y: f64,
}

impl NegativePopulationShift {
    pub fn width_warning_x(&self, threshold: f64) -> bool {
        self.width_ratio_x > threshold
    }
    pub fn width_warning_y(&self, threshold: f64) -> bool {
        self.width_ratio_y > threshold
    }
}

// ─── Main function ────────────────────────────────────────────────────────────

/// Extracts events from the negative (lower-left) quadrant and uses 1D KDE
/// peak-finding on each axis independently to measure how far the negative
/// population has shifted between QC and test.
///
/// Separate from `compute_negative_shift` (cross-correlation) — that function
/// remains useful for full 2D gate alignment. This function is specifically
/// for characterising the negative population.
pub fn kde_negative_shift(
    qc_events: (&Column, &Column),
    test_events: (&Column, &Column),
    axis_x_range: (f64, f64),
    axis_y_range: (f64, f64),
    n_kde_points: usize, // resolution of KDE grid — 512 is plenty
    min_events: usize,   // minimum events in negative quadrant to proceed
) -> Result<NegativePopulationShift, String> {
    let x_mid = axis_x_range.0 + (axis_x_range.1 - axis_x_range.0) / 2.0;
    let y_mid = axis_y_range.0 + (axis_y_range.1 - axis_y_range.0) / 2.0;

    let (qc_neg_x, qc_neg_y) = extract_negative_quadrant(qc_events, x_mid, y_mid)?;
    let (test_neg_x, test_neg_y) = extract_negative_quadrant(test_events, x_mid, y_mid)?;

    if qc_neg_x.len() < min_events {
        return Err(format!(
            "QC negative quadrant has only {} events (min {})",
            qc_neg_x.len(),
            min_events
        ));
    }
    if test_neg_x.len() < min_events {
        return Err(format!(
            "Test negative quadrant has only {} events (min {})",
            test_neg_x.len(),
            min_events
        ));
    }

    // ── Bandwidths ────────────────────────────────────────────────────────────
    // Computed from the QC — test uses the same bandwidth so peaks are comparable.
    let bw_x = silverman_bandwidth(&qc_neg_x);
    let bw_y = silverman_bandwidth(&qc_neg_y);

    // ── KDE on X axis ─────────────────────────────────────────────────────────
    let (xs_grid, qc_density_x) = kde_1d(&qc_neg_x, axis_x_range, n_kde_points, bw_x);
    let (_, test_density_x) = kde_1d(&test_neg_x, axis_x_range, n_kde_points, bw_x);

    let qc_peak_x = kde_peak(&xs_grid, &qc_density_x);
    let test_peak_x = kde_peak(&xs_grid, &test_density_x);

    // ── KDE on Y axis ─────────────────────────────────────────────────────────
    let (ys_grid, qc_density_y) = kde_1d(&qc_neg_y, axis_y_range, n_kde_points, bw_y);
    let (_, test_density_y) = kde_1d(&test_neg_y, axis_y_range, n_kde_points, bw_y);

    let qc_peak_y = kde_peak(&ys_grid, &qc_density_y);
    let test_peak_y = kde_peak(&ys_grid, &test_density_y);

    // ── Width (std dev of events in the quadrant) ─────────────────────────────
    let qc_width_x = std_dev(&qc_neg_x);
    let qc_width_y = std_dev(&qc_neg_y);
    let test_width_x = std_dev(&test_neg_x);
    let test_width_y = std_dev(&test_neg_y);

    Ok(NegativePopulationShift {
        dx: test_peak_x - qc_peak_x,
        dy: test_peak_y - qc_peak_y,
        qc_width_x,
        qc_width_y,
        test_width_x,
        test_width_y,
        width_ratio_x: test_width_x / qc_width_x,
        width_ratio_y: test_width_y / qc_width_y,
    })
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn extract_negative_quadrant(
    events: (&Column, &Column),
    x_mid: f64,
    y_mid: f64,
) -> Result<(Vec<f64>, Vec<f64>), String> {
    let xs = events.0.f64().map_err(|e| e.to_string())?;
    let ys = events.1.f64().map_err(|e| e.to_string())?;

    let (neg_x, neg_y): (Vec<f64>, Vec<f64>) = xs
        .into_iter()
        .zip(ys.into_iter())
        .filter_map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) if x < x_mid && y < y_mid => Some((x, y)),
            _ => None,
        })
        .unzip();

    Ok((neg_x, neg_y))
}

pub fn std_dev(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    variance.sqrt()
}

//cargo test -- --nocapture
// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod flow_tests {
    use super::*;

    use rand::prelude::*;
    use rand_distr::Normal;

    // ─── Core sampler ────────────────────────────────────────────────────────────

    struct Cluster {
        cx: f64,
        cy: f64,
        sx: f64,
        sy: f64,
        n: usize,
    }

    fn sample_clusters(clusters: &[Cluster], rng: &mut StdRng) -> (Vec<f64>, Vec<f64>) {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for c in clusters {
            let dx = Normal::new(c.cx, c.sx).unwrap();
            let dy = Normal::new(c.cy, c.sy).unwrap();
            for _ in 0..c.n {
                xs.push(dx.sample(rng));
                ys.push(dy.sample(rng));
            }
        }
        (xs, ys)
    }

    /// Smeared positive: uniform scatter across a rectangle rather than a tight cluster
    fn sample_smear(
        x_range: (f64, f64),
        y_range: (f64, f64),
        n: usize,
        rng: &mut StdRng,
    ) -> (Vec<f64>, Vec<f64>) {
        let xs: Vec<f64> = (0..n)
            .map(|_| rng.random_range(x_range.0..x_range.1))
            .collect();
        let ys: Vec<f64> = (0..n)
            .map(|_| rng.random_range(y_range.0..y_range.1))
            .collect();
        (xs, ys)
    }

    fn make_df(xs: Vec<f64>, ys: Vec<f64>) -> DataFrame {
        df!["x" => xs, "y" => ys].unwrap()
    }

    fn concat_events(a: (Vec<f64>, Vec<f64>), b: (Vec<f64>, Vec<f64>)) -> (Vec<f64>, Vec<f64>) {
        let mut xs = a.0;
        xs.extend(b.0);
        let mut ys = a.1;
        ys.extend(b.1);
        (xs, ys)
    }

    // // ─── Shared baseline negative ─────────────────────────────────────────────────
    // // Reused by most tests: a clean double-negative at (0.4, 0.4)

    // fn baseline_negative(rng: &mut StdRng) -> (Vec<f64>, Vec<f64>) {
    //     sample_clusters(&[Cluster { cx: 0.4, cy: 0.4, sx: 0.12, sy: 0.12, n: 3000 }], rng)
    // }

    // ─── Test scenarios ───────────────────────────────────────────────────────────

    /// Negative is wider on X axis in the test sample.
    /// Expect: negative peak positions similar, but test spread is larger on x.
    pub fn wider_negative_x(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.35,
                    sy: 0.12,
                    n: 3000,
                }, // wider x
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Negative is wider on Y axis in the test sample.
    pub fn wider_negative_y(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.35,
                    n: 3000,
                }, // wider y
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Negative shifted right (+0.3 on x only).
    /// Expect: dx ≈ +0.3, dy ≈ 0.0
    pub fn negative_shifted_x(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.7,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                }, // +0.3 x
                Cluster {
                    cx: 2.8,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Negative shifted up (+0.3 on y only).
    /// Expect: dx ≈ 0.0, dy ≈ +0.3
    pub fn negative_shifted_y(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.7,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                }, // +0.3 y
                Cluster {
                    cx: 2.5,
                    cy: 2.8,
                    sx: 0.20,
                    sy: 0.20,
                    n: 800,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// QC has a distinct tight positive population; test sample does not (antigen-negative sample).
    /// Expect: alignment still works on the negative; positive simply absent in test.
    pub fn positive_only_in_qc(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.8,
                    cy: 2.8,
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                }, // distinct positive
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                }, // negative only
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Test sample has a distinct positive; QC does not.
    pub fn positive_only_in_test(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.8,
                    cy: 2.8,
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// QC has a smeared positive (dim, diffuse expression); test does not.
    /// Smear sits in the intermediate region — not a tight cluster.
    pub fn smeared_positive_only_in_qc(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let neg = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        let smear = sample_smear((0.8, 2.5), (0.8, 2.5), 600, &mut rng);
        let (qc_x, qc_y) = concat_events(neg, smear);

        let test = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        (make_df(qc_x, qc_y), make_df(test.0, test.1))
    }

    /// Test sample has a smeared positive; QC does not.
    pub fn smeared_positive_only_in_test(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );

        let neg = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        let smear = sample_smear((0.8, 2.5), (0.8, 2.5), 600, &mut rng);
        let (test_x, test_y) = concat_events(neg, smear);

        (make_df(qc.0, qc.1), make_df(test_x, test_y))
    }

    /// Both samples have a distinct positive, but it is shifted in the test (+0.4 on both axes).
    /// The negative is identical — so alignment should be driven by the negative,
    /// and the positive shift should be visible as a residual after alignment.
    pub fn positive_shifted_in_test(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.20,
                    sy: 0.20,
                    n: 900,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.4,
                    cy: 0.4,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 2.9,
                    cy: 2.9,
                    sx: 0.20,
                    sy: 0.20,
                    n: 900,
                }, // positive shifted +0.4
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    fn run(label: &str, qc: &DataFrame, test: &DataFrame, expected_dx: f64, expected_dy: f64) {
        let axis = ((-1.0f64, 4.5f64), (-1.0f64, 4.5f64));

        let result = kde_negative_shift(
            (qc.column("x").unwrap(), qc.column("y").unwrap()),
            (test.column("x").unwrap(), test.column("y").unwrap()),
            axis.0,
            axis.1,
            512, // kde resolution
            50,  // min events
        );

        match result {
            Ok(s) => println!(
                "[{label}]\n  shift:       dx={:.4}, dy={:.4}\n  expected:    dx={:.4}, dy={:.4}\n  error:       dx={:.4}, dy={:.4}\n  qc width:    x={:.4}, y={:.4}\n  test width:  x={:.4}, y={:.4}\n  width ratio: x={:.2}, y={:.2}{}{}\n",
                s.dx,
                s.dy,
                expected_dx,
                expected_dy,
                (s.dx - expected_dx).abs(),
                (s.dy - expected_dy).abs(),
                s.qc_width_x,
                s.qc_width_y,
                s.test_width_x,
                s.test_width_y,
                s.width_ratio_x,
                s.width_ratio_y,
                if s.width_warning_x(1.5) {
                    "  ⚠ x width increased"
                } else {
                    ""
                },
                if s.width_warning_y(1.5) {
                    "  ⚠ y width increased"
                } else {
                    ""
                },
            ),
            Err(e) => println!("[{label}] Err: {e}\n"),
        }
    }

    #[test]
    fn test_wider_negative_x() {
        // Negative is in same position — expect near-zero shift.
        // The wider spread is a diagnostic signal, not a translation.
        let (qc, test) = wider_negative_x(42);
        run("wider_negative_x", &qc, &test, 0.0, 0.0);
    }

    #[test]
    fn test_wider_negative_y() {
        let (qc, test) = wider_negative_y(42);
        run("wider_negative_y", &qc, &test, 0.0, 0.0);
    }

    #[test]
    fn test_negative_shifted_x() {
        let (qc, test) = negative_shifted_x(42);
        run("negative_shifted_x", &qc, &test, 0.3, 0.0);
    }

    #[test]
    fn test_negative_shifted_y() {
        let (qc, test) = negative_shifted_y(42);
        run("negative_shifted_y", &qc, &test, 0.0, 0.3);
    }

    #[test]
    fn test_positive_only_in_qc() {
        // Negative identical — expect near-zero shift, no crash from missing positive.
        let (qc, test) = positive_only_in_qc(42);
        run("positive_only_in_qc", &qc, &test, 0.0, 0.0);
    }

    #[test]
    fn test_positive_only_in_test() {
        let (qc, test) = positive_only_in_test(42);
        run("positive_only_in_test", &qc, &test, 0.0, 0.0);
    }

    #[test]
    fn test_smeared_positive_only_in_qc() {
        let (qc, test) = smeared_positive_only_in_qc(42);
        run("smeared_positive_only_in_qc", &qc, &test, 0.0, 0.0);
    }

    #[test]
    fn test_smeared_positive_only_in_test() {
        let (qc, test) = smeared_positive_only_in_test(42);
        run("smeared_positive_only_in_test", &qc, &test, 0.0, 0.0);
    }

    #[test]
    fn test_positive_shifted_in_test() {
        // Negative identical — alignment should report near-zero shift.
        // Positive shift is biological and should NOT influence the result.
        let (qc, test) = positive_shifted_in_test(42);
        run("positive_shifted_in_test", &qc, &test, 0.0, 0.0);
    }
}
