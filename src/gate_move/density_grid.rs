use polars::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};

// this module is used to determine shift between 2D plots - perhaps useful
// for QC between runs, or FMO/FMX effect of FMO on reportables
// don't use the negative / peak isolation fns they are overkill

pub struct DensityGrid {
    pub counts: Vec<f32>, // row-major, shape [n_bins x n_bins]
    pub n_bins: usize,
    pub x_range: (f64, f64),
    pub y_range: (f64, f64),
}

impl DensityGrid {
    pub fn from_column(
        xs: &Column,
        ys: &Column,
        n_bins: usize,
        x_range: (f64, f64),
        y_range: (f64, f64),
    ) -> Self {
        let xs = xs.f64().unwrap();
        let ys = ys.f64().unwrap();

        let mut counts = vec![0.0f32; n_bins * n_bins];

        let x_scale = n_bins as f64 / (x_range.1 - x_range.0);
        let y_scale = n_bins as f64 / (y_range.1 - y_range.0);

        for (x, y) in xs.into_iter().zip(ys.into_iter()) {
            let (Some(x), Some(y)) = (x, y) else { continue };

            let col = ((x - x_range.0) * x_scale) as isize;
            let row = ((y - y_range.0) * y_scale) as isize;

            if col >= 0 && col < n_bins as isize && row >= 0 && row < n_bins as isize {
                counts[row as usize * n_bins + col as usize] += 1.0;
            }
        }

        Self {
            counts,
            n_bins,
            x_range,
            y_range,
        }
    }

    /// Bin width in data-space units — used to convert peak offset back to data coords
    pub fn bin_width_x(&self) -> f64 {
        (self.x_range.1 - self.x_range.0) / self.n_bins as f64
    }

    pub fn bin_width_y(&self) -> f64 {
        (self.y_range.1 - self.y_range.0) / self.n_bins as f64
    }

    /// Finds the maximum density bin restricted to the lower-left quadrant.
    /// This ensures we lock onto the double-negative background, even if
    /// a positive population has more total events.
    pub fn find_lower_quadrant_peak(&self) -> (usize, usize) {
        let n = self.n_bins;
        let half_n = n / 2; // Restrict to the bottom-left 25% of the plot area

        let mut max_val = -1.0f32;
        let mut max_row = 0;
        let mut max_col = 0;

        for row in 0..half_n {
            for col in 0..half_n {
                let idx = row * n + col;
                let val = self.counts[idx];

                if val > max_val {
                    max_val = val;
                    max_row = row;
                    max_col = col;
                }
            }
        }

        (max_row, max_col)
    }

    /// Applies a Gaussian window centered on a specific bin, fading out distant populations
    pub fn isolate_peak(&mut self, center_row: usize, center_col: usize, radius_bins: f32) {
        let n = self.n_bins;
        for row in 0..n {
            for col in 0..n {
                let dr = row as f32 - center_row as f32;
                let dc = col as f32 - center_col as f32;
                let dist_sq = dr * dr + dc * dc;

                let weight = (-dist_sq / (2.0 * radius_bins * radius_bins)).exp();
                self.counts[row * n + col] *= weight;
            }
        }
    }

    pub fn isolate_peak_elliptical(
        &mut self,
        center_row: usize,
        center_col: usize,
        sigma_row: f32, // Y-axis spread
        sigma_col: f32, // X-axis spread
    ) {
        let n = self.n_bins;
        for row in 0..n {
            let dr = row as f32 - center_row as f32;
            let row_weight = -(dr * dr) / (2.0 * sigma_row * sigma_row);

            for col in 0..n {
                let dc = col as f32 - center_col as f32;
                let col_weight = -(dc * dc) / (2.0 * sigma_col * sigma_col);

                let weight = (row_weight + col_weight).exp();
                self.counts[row * n + col] *= weight;
            }
        }
    }
}

/// Simple separable Gaussian blur applied in-place
pub fn gaussian_blur(grid: &mut DensityGrid, sigma: f32) {
    let n = grid.n_bins;
    let kernel = make_gaussian_kernel(sigma);
    let k = kernel.len();
    let half = k / 2;
    let mut tmp = vec![0.0f32; n * n];

    // Horizontal pass
    for row in 0..n {
        for col in 0..n {
            let mut val = 0.0f32;
            for (ki, &kw) in kernel.iter().enumerate() {
                let src_col = col as isize + ki as isize - half as isize;
                if src_col >= 0 && src_col < n as isize {
                    val += grid.counts[row * n + src_col as usize] * kw;
                }
            }
            tmp[row * n + col] = val;
        }
    }

    // Vertical pass
    for row in 0..n {
        for col in 0..n {
            let mut val = 0.0f32;
            for (ki, &kw) in kernel.iter().enumerate() {
                let src_row = row as isize + ki as isize - half as isize;
                if src_row >= 0 && src_row < n as isize {
                    val += tmp[src_row as usize * n + col] * kw;
                }
            }
            grid.counts[row * n + col] = val;
        }
    }
}

fn make_gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil() as usize;
    let size = 2 * radius + 1;
    let mut k: Vec<f32> = (0..size)
        .map(|i| {
            let x = i as f32 - radius as f32;
            (-x * x / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

pub struct TranslationVector {
    pub dx_bins: i32, // positive = test is shifted right relative to QC
    pub dy_bins: i32,
    pub dx_data: f64, // in data-space units (e.g. arcsinh-transformed)
    pub dy_data: f64,
    pub peak_strength: f32, // for confidence scoring
}

fn fft2d(data: &mut Vec<Complex<f32>>, n: usize, planner: &mut FftPlanner<f32>, inverse: bool) {
    let fft = if inverse {
        planner.plan_fft_inverse(n)
    } else {
        planner.plan_fft_forward(n)
    };

    // FFT along rows
    for row in 0..n {
        let slice = &mut data[row * n..(row + 1) * n];
        fft.process(slice);
    }

    // FFT along columns — extract, process, write back
    let mut col_buf = vec![Complex::new(0.0f32, 0.0); n];
    for col in 0..n {
        for row in 0..n {
            col_buf[row] = data[row * n + col];
        }
        fft.process(&mut col_buf);
        for row in 0..n {
            data[row * n + col] = col_buf[row];
        }
    }

    if inverse {
        let norm = (n * n) as f32;
        data.iter_mut().for_each(|v| *v /= norm);
    }
}

pub fn cross_correlate(qc: &DensityGrid, test: &DensityGrid) -> TranslationVector {
    let n = qc.n_bins;
    assert_eq!(n, test.n_bins);

    let mut planner = FftPlanner::<f32>::new();

    let mut a: Vec<Complex<f32>> = qc.counts.iter().map(|&v| Complex::new(v, 0.0)).collect();
    let mut b: Vec<Complex<f32>> = test.counts.iter().map(|&v| Complex::new(v, 0.0)).collect();

    fft2d(&mut a, n, &mut planner, false);
    fft2d(&mut b, n, &mut planner, false);

    let mut product: Vec<Complex<f32>> = a
        .iter()
        .zip(b.iter())
        .map(|(ai, bi)| bi * ai.conj())
        .collect();

    fft2d(&mut product, n, &mut planner, true);

    let (peak_idx, peak_val) = product
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.re.partial_cmp(&b.re).unwrap())
        .unwrap();

    let raw_row = (peak_idx / n) as i32;
    let raw_col = (peak_idx % n) as i32;
    let n_i = n as i32;

    let dy_bins = if raw_row > n_i / 2 {
        raw_row - n_i
    } else {
        raw_row
    };
    let dx_bins = if raw_col > n_i / 2 {
        raw_col - n_i
    } else {
        raw_col
    };

    TranslationVector {
        dx_bins,
        dy_bins,
        dx_data: dx_bins as f64 * qc.bin_width_x(),
        dy_data: dy_bins as f64 * qc.bin_width_y(),
        peak_strength: peak_val.re,
    }
}

pub struct GateRules {
    pub lock_x: bool,
    pub lock_y: bool,
    pub max_translation: Option<f64>, // in data-space units
}

pub fn apply_constraints(mut t: TranslationVector, rules: &GateRules) -> TranslationVector {
    if rules.lock_x {
        t.dx_data = 0.0;
        t.dx_bins = 0;
    }
    if rules.lock_y {
        t.dy_data = 0.0;
        t.dy_bins = 0;
    }

    if let Some(max_t) = rules.max_translation {
        let mag = (t.dx_data.powi(2) + t.dy_data.powi(2)).sqrt();
        if mag > max_t {
            let scale = max_t / mag;
            t.dx_data *= scale;
            t.dy_data *= scale;
        }
    }

    t
}

fn calculate_dynamic_radii(
    xs: &Column,
    ys: &Column,
    x_mid: f64,
    y_mid: f64,
    n_bins: usize,
    x_data_range: f64,
    y_data_range: f64,
) -> Option<(f32, f32)> {
    let mask = xs.f64().ok()?.lt(x_mid) & ys.f64().ok()?.lt(y_mid);
    let filtered_x = xs.filter(&mask).ok()?;
    let filtered_y = ys.filter(&mask).ok()?;

    let count = filtered_x.len();

    // RED FLAG 1: Not enough events to define a population
    // 50 is a safe floor for 5 million events; adjust based on your sensitivity needs.
    if count < 50 {
        return None;
    }

    let std_x = filtered_x.f64().ok()?.std(1)?;
    let std_y = filtered_y.f64().ok()?.std(1)?;

    // RED FLAG 2: Noise Check
    // If StdDev is 0.0, the data is a single vertical/horizontal line (error).
    // If StdDev is too high (e.g., > 25% of total range), it's just noise, not a cluster.
    if std_x <= 0.0 || std_y <= 0.0 || std_x > (x_data_range * 0.25) {
        return None;
    }

    let bins_per_unit_x = n_bins as f64 / x_data_range;
    let bins_per_unit_y = n_bins as f64 / y_data_range;

    let sigma_x = ((std_x * bins_per_unit_x) as f32 * 2.5).clamp(4.0, 15.0);
    let sigma_y = ((std_y * bins_per_unit_y) as f32 * 2.5).clamp(4.0, 15.0);

    Some((sigma_x, sigma_y))
}

pub fn compute_negative_shift(
    qc_parent_events: (&Column, &Column),
    test_parent_events: (&Column, &Column),
    axis_x_range: (f64, f64),
    axis_y_range: (f64, f64),
    rules: &GateRules,
    n_bins: usize,
    blur_sigma: f32,
) -> Result<TranslationVector, String> {
    let x_data_len = axis_x_range.1 - axis_x_range.0;
    let y_data_len = axis_y_range.1 - axis_y_range.0;
    let x_mid = axis_x_range.0 + (x_data_len / 2.0);
    let y_mid = axis_y_range.0 + (y_data_len / 2.0);

    // 1. Try to find the spotlight radii. Errors if QC is noise.
    let (sigma_x, sigma_y) = calculate_dynamic_radii(
        qc_parent_events.0,
        qc_parent_events.1,
        x_mid,
        y_mid,
        n_bins,
        x_data_len,
        y_data_len,
    )
    .ok_or("QC sample has no clear negative population in the lower quadrant.")?;

    let mut qc_grid = DensityGrid::from_column(
        qc_parent_events.0,
        qc_parent_events.1,
        n_bins,
        axis_x_range,
        axis_y_range,
    );
    let mut test_grid = DensityGrid::from_column(
        test_parent_events.0,
        test_parent_events.1,
        n_bins,
        axis_x_range,
        axis_y_range,
    );

    gaussian_blur(&mut qc_grid, blur_sigma);
    gaussian_blur(&mut test_grid, blur_sigma);

    // 2. Find peaks. Check if test grid is empty.
    let (qc_row, qc_col) = qc_grid.find_lower_quadrant_peak();
    let (test_row, test_col) = test_grid.find_lower_quadrant_peak();

    if qc_grid.counts[qc_row * n_bins + qc_col] < 1.0 {
        return Err("QC grid is empty".into());
    }

    // 3. Apply elliptical masks
    qc_grid.isolate_peak_elliptical(qc_row, qc_col, sigma_y, sigma_x);
    test_grid.isolate_peak_elliptical(test_row, test_col, sigma_y, sigma_x);

    // 4. Correlate
    let translation = cross_correlate(&qc_grid, &test_grid);

    // RED FLAG 3: Correlation Strength
    // If the peak_strength is very low, the isolated shapes don't match.
    if translation.peak_strength < 0.1 {
        return Err("Samples are too different to align (Low Correlation).".into());
    }

    Ok(apply_constraints(translation, rules))
}

pub fn compute_total_shift(
    qc_parent_events: (&Column, &Column),
    test_parent_events: (&Column, &Column),
    axis_x_range: (f64, f64),
    axis_y_range: (f64, f64),
    rules: &GateRules,
    n_bins: usize,
    blur_sigma: f32,
) -> Result<TranslationVector, String> {
    let mut qc_grid = DensityGrid::from_column(
        qc_parent_events.0,
        qc_parent_events.1,
        n_bins,
        axis_x_range,
        axis_y_range,
    );
    let mut test_grid = DensityGrid::from_column(
        test_parent_events.0,
        test_parent_events.1,
        n_bins,
        axis_x_range,
        axis_y_range,
    );

    gaussian_blur(&mut qc_grid, blur_sigma);
    gaussian_blur(&mut test_grid, blur_sigma);

    // 4. Correlate
    let translation = cross_correlate(&qc_grid, &test_grid);

    // RED FLAG 3: Correlation Strength
    // If the peak_strength is very low, the isolated shapes don't match.
    if translation.peak_strength < 0.1 {
        return Err("Samples are too different to align (Low Correlation).".into());
    }

    Ok(apply_constraints(translation, rules))
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
        let rules = GateRules {
            lock_x: false,
            lock_y: false,
            max_translation: None,
        };
        let axis = ((-1.0, 4.5), (-1.0, 4.5));

        let result = compute_negative_shift(
            (qc.column("x").unwrap(), qc.column("y").unwrap()),
            (test.column("x").unwrap(), test.column("y").unwrap()),
            axis.0,
            axis.1,
            &rules,
            64,
            2.0,
        );

        match result {
            Ok(t) => println!(
                "[{label}]\n  translation: dx={:.4}, dy={:.4}\n  expected:    dx={:.4}, dy={:.4}\n  error:       dx={:.4}, dy={:.4}\n  peak:        {:.2}\n",
                t.dx_data,
                t.dy_data,
                expected_dx,
                expected_dy,
                (t.dx_data - expected_dx).abs(),
                (t.dy_data - expected_dy).abs(),
                t.peak_strength
            ),
            Err(e) => println!("[{label}] returned Err: {e}\n"),
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
