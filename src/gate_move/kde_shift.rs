// ─── Gate-informed population shift analysis ──────────────────────────────────
use crate::gate_move::kde::{kde_1d, kde_peak, silverman_bandwidth, std_dev};
use polars::prelude::*;

/// The lower boundary of the gate on each axis, taken directly from the QC gate.
/// These are in data-space units (arcsinh-transformed).
pub struct GateBoundary {
    pub x_lower: f64,
    pub y_lower: f64,
}

pub enum DriftType {
    /// Negative and positive shift by similar amounts — instrument moved uniformly.
    InstrumentDrift,
    /// Negative stable, positive shifted — biological change between samples.
    Biological,
    /// Negative widened without shifting — compensation or voltage spread issue.
    CompensationIssue,
    /// Nothing moved significantly — samples look equivalent.
    Clean,
    /// Patterns don't fit a simple category — inspect manually.
    Ambiguous,
}

impl std::fmt::Display for DriftType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriftType::InstrumentDrift => write!(f, "Instrument drift"),
            DriftType::Biological => write!(f, "Biological variation"),
            DriftType::CompensationIssue => write!(f, "Compensation / voltage spread"),
            DriftType::Clean => write!(f, "Clean — no significant shift"),
            DriftType::Ambiguous => write!(f, "Ambiguous — inspect manually"),
        }
    }
}

pub struct PopulationShiftResult {
    // ── Negative population ───────────────────────────────────────────────────
    /// How far the negative peak moved — this is your gate adjustment vector.
    pub negative_dx: f64,
    pub negative_dy: f64,
    pub qc_negative_width_x: f64,
    pub qc_negative_width_y: f64,
    pub test_negative_width_x: f64,
    pub test_negative_width_y: f64,
    /// Ratio > 1.5 on either axis is worth flagging.
    pub width_ratio_x: f64,
    pub width_ratio_y: f64,

    // ── Positive population ───────────────────────────────────────────────────
    /// None if either QC or test had insufficient events above the gate boundary.
    pub positive_dx: Option<f64>,
    pub positive_dy: Option<f64>,
    pub qc_positive_width_x: Option<f64>,
    pub qc_positive_width_y: Option<f64>,
    pub test_positive_width_x: Option<f64>,
    pub test_positive_width_y: Option<f64>,

    // ── Interpretation ────────────────────────────────────────────────────────
    pub drift_type: DriftType,
    pub positive_smear_score_x: Option<f64>,
    pub positive_smear_score_y: Option<f64>,
}

impl PopulationShiftResult {
    pub fn width_warning_x(&self, threshold: f64) -> bool {
        self.width_ratio_x > threshold
    }
    pub fn width_warning_y(&self, threshold: f64) -> bool {
        self.width_ratio_y > threshold
    }
}

/// Analyse negative and positive population shift between QC and test,
/// using the QC gate boundary to define the search regions.
///
/// Returns a [`PopulationShiftResult`] describing how far each population
/// moved, whether the negative widened, and a [`DriftType`] classification
/// of what kind of change occurred between samples.
///
/// # Arguments
///
/// * `qc_events` - The (x, y) columns of the **fully gated QC sample** for
///   the parent gate of the gate being analysed. These are the events that
///   passed all ancestor gates — not just the events inside the gate itself.
///   Both columns must be in arcsinh-transformed data-space units.
///
/// * `test_events` - The (x, y) columns of the **test sample** for the same
///   parent gate. Must be on the same two axes and in the same coordinate
///   system as `qc_events`. Biological differences between samples are
///   expected and handled; this function separates instrument drift from
///   biological variation.
///
/// * `axis_x_range` - The fixed instrument-defined axis limits on the x axis,
///   e.g. `(-1.0, 4.5)` in arcsinh space for a typical Aurora channel. Must
///   be identical between QC and test — never derive from the data itself.
///   Used to define the KDE evaluation range and to normalise the smear score.
///
/// * `axis_y_range` - Same as `axis_x_range` but for the y axis. Can differ
///   from `axis_x_range` if the two channels have different dynamic ranges.
///
/// * `gate` - The lower boundary of the gate as drawn on the QC sample, in
///   data-space units. `x_lower` and `y_lower` define where the gate edge
///   sits on each axis. This is used to split events into negative (below
///   the boundary) and positive (above the boundary) search regions, so it
///   should reflect the actual gate position on the QC — not an estimated
///   or assumed boundary.
///
/// * `negative_margin` - How far **below** the gate edge to set the upper
///   limit of the negative search region, in data-space units. For example,
///   with `gate.x_lower = 1.2` and `negative_margin = 0.1`, the negative
///   region extends from `axis_x_range.0` up to `1.1`. The margin excludes
///   the transition zone just below the gate edge where true positive events
///   scatter down and true negative events scatter up — including this zone
///   would pull the negative KDE peak toward the gate edge and bias the shift
///   estimate. A value of roughly 5–10% of the gate height is a reasonable
///   starting point. Setting this too large shrinks the negative search region
///   and risks excluding the bulk of the negative population.
///
/// * `n_kde_points` - Number of evenly spaced evaluation points for the 1D
///   KDE grid on each axis. Higher values give finer peak resolution but
///   increase computation time linearly. 512 is sufficient for typical flow
///   data; 256 is acceptable if performance is a concern. There is no benefit
///   to going above 1024 — the bandwidth (set by Silverman's rule) limits
///   the effective resolution regardless of grid density.
///
/// * `min_events` - Minimum number of events required in a region before KDE
///   is attempted. If either the QC or test sample has fewer than this many
///   events in the negative region, the function returns `Err`. If the
///   positive region falls below this threshold in either sample, the positive
///   analysis is skipped gracefully and `positive_dx`/`positive_dy` return
///   `None`. A value of 50 is a safe floor for typical acquisition counts;
///   lower values risk KDE estimates being driven by noise rather than a
///   real population.
///
/// * `significant_shift` - The minimum shift magnitude, in data-space units,
///   that is considered a real population movement rather than noise. Shifts
///   below this threshold on both axes are treated as zero for the purposes
///   of [`DriftType`] classification. Typical values are 0.05–0.15 depending
///   on how tightly your instrument is controlled and how precisely the QC
///   gate was drawn. Setting this too low causes noisy samples to be
///   misclassified as drifted; too high and small but real instrument drift
///   goes undetected.
///
/// * `significant_width_ratio` - The minimum ratio of test negative width to
///   QC negative width (on either axis) that triggers a width warning and
///   influences [`DriftType`] classification. A ratio of 1.0 means identical
///   width; 1.5 means the test negative is 50% wider than the QC negative.
///   Values around 1.4–1.6 are appropriate for most panels. Setting this too
///   low causes normal run-to-run variation to be flagged; too high and
///   genuine compensation or voltage spread issues are missed.
pub fn analyse_population_shift(
    qc_events: (&Column, &Column),
    test_events: (&Column, &Column),
    axis_x_range: (f64, f64),
    axis_y_range: (f64, f64),
    gate: &GateBoundary,
    negative_margin: f64,
    n_kde_points: usize,
    min_events: usize,
    // Thresholds for drift classification
    significant_shift: f64,       // e.g. 0.1 — shifts below this are noise
    significant_width_ratio: f64, // e.g. 1.5
) -> Result<PopulationShiftResult, String> {
    // ── Define search regions from gate boundary ───────────────────────────────
    //
    // Negative: strictly below the gate edge minus a margin.
    //   The margin excludes the fuzzy transition zone just below the gate edge
    //   where some true positives scatter down and some negatives scatter up.
    //
    // Positive: above the gate edge.
    //   We don't apply a margin here — we want all events the gate would capture.
    //   Patient-to-patient variation means the positive can be anywhere above the
    //   gate edge, so we hand the full region to the KDE and let it find the peak.

    let neg_x_upper = gate.x_lower - negative_margin;
    let neg_y_upper = gate.y_lower - negative_margin;

    // ── Extract events ────────────────────────────────────────────────────────

    let (qc_neg_x, qc_neg_y) = extract_region(
        qc_events,
        axis_x_range.0,
        neg_x_upper,
        axis_y_range.0,
        neg_y_upper,
    )?;
    let (test_neg_x, test_neg_y) = extract_region(
        test_events,
        axis_x_range.0,
        neg_x_upper,
        axis_y_range.0,
        neg_y_upper,
    )?;

    if qc_neg_x.len() < min_events {
        return Err(format!(
            "QC negative region has only {} events (min {})",
            qc_neg_x.len(),
            min_events
        ));
    }
    if test_neg_x.len() < min_events {
        return Err(format!(
            "Test negative region has only {} events (min {})",
            test_neg_x.len(),
            min_events
        ));
    }

    // Positive: everything above the gate edge on both axes.
    let qc_pos = extract_region(
        qc_events,
        gate.x_lower,
        axis_x_range.1,
        gate.y_lower,
        axis_y_range.1,
    )
    .ok();
    let test_pos = extract_region(
        test_events,
        gate.x_lower,
        axis_x_range.1,
        gate.y_lower,
        axis_y_range.1,
    )
    .ok();

    let qc_pos_count = qc_pos.as_ref().map(|(x, _)| x.len()).unwrap_or(0);
    let test_pos_count = test_pos.as_ref().map(|(x, _)| x.len()).unwrap_or(0);

    let qc_has_pos = qc_pos_count >= min_events;
    let test_has_pos = test_pos_count >= min_events;

    let pos_emerged = !qc_has_pos && test_has_pos;
    let pos_disappeared = qc_has_pos && !test_has_pos;

    // ── Negative KDE ──────────────────────────────────────────────────────────
    // Bandwidth from QC, reused for test so peaks are on the same scale.

    let neg_x_range = (axis_x_range.0, neg_x_upper);
    let neg_y_range = (axis_y_range.0, neg_y_upper);

    let bw_neg_x = silverman_bandwidth(&qc_neg_x);
    let bw_neg_y = silverman_bandwidth(&qc_neg_y);

    let (xs_grid, qc_density_neg_x) = kde_1d(&qc_neg_x, neg_x_range, n_kde_points, bw_neg_x);
    let (_, test_density_neg_x) = kde_1d(&test_neg_x, neg_x_range, n_kde_points, bw_neg_x);
    let (ys_grid, qc_density_neg_y) = kde_1d(&qc_neg_y, neg_y_range, n_kde_points, bw_neg_y);
    let (_, test_density_neg_y) = kde_1d(&test_neg_y, neg_y_range, n_kde_points, bw_neg_y);

    let qc_peak_neg_x = kde_peak(&xs_grid, &qc_density_neg_x);
    let test_peak_neg_x = kde_peak(&xs_grid, &test_density_neg_x);
    let qc_peak_neg_y = kde_peak(&ys_grid, &qc_density_neg_y);
    let test_peak_neg_y = kde_peak(&ys_grid, &test_density_neg_y);

    let negative_dx = test_peak_neg_x - qc_peak_neg_x;
    let negative_dy = test_peak_neg_y - qc_peak_neg_y;

    let qc_negative_width_x = std_dev(&qc_neg_x);
    let qc_negative_width_y = std_dev(&qc_neg_y);
    let test_negative_width_x = std_dev(&test_neg_x);
    let test_negative_width_y = std_dev(&test_neg_y);

    let width_ratio_x = test_negative_width_x / qc_negative_width_x;
    let width_ratio_y = test_negative_width_y / qc_negative_width_y;

    // ── Positive KDE (best-effort) ────────────────────────────────────────────

    let positive_result = match (qc_pos, test_pos) {
        (Some((qc_pos_x, qc_pos_y)), Some((test_pos_x, test_pos_y)))
            if qc_pos_x.len() >= min_events && test_pos_x.len() >= min_events =>
        {
            let pos_x_range = (gate.x_lower, axis_x_range.1);
            let pos_y_range = (gate.y_lower, axis_y_range.1);

            let bw_pos_x = silverman_bandwidth(&qc_pos_x);
            let bw_pos_y = silverman_bandwidth(&qc_pos_y);

            let (px_grid, qc_d_px) = kde_1d(&qc_pos_x, pos_x_range, n_kde_points, bw_pos_x);
            let (_, test_d_px) = kde_1d(&test_pos_x, pos_x_range, n_kde_points, bw_pos_x);

            let (py_grid, qc_d_py) = kde_1d(&qc_pos_y, pos_y_range, n_kde_points, bw_pos_y);
            let (_, test_d_py) = kde_1d(&test_pos_y, pos_y_range, n_kde_points, bw_pos_y);

            // ── 1. Peak-based shift (your original method)
            let peak_dx = kde_peak(&px_grid, &test_d_px) - kde_peak(&px_grid, &qc_d_px);
            let peak_dy = kde_peak(&py_grid, &test_d_py) - kde_peak(&py_grid, &qc_d_py);

            // ── 2. Quantile-based shift (new)
            let mut qc_x = qc_pos_x.clone();
            let mut test_x = test_pos_x.clone();
            let mut qc_y = qc_pos_y.clone();
            let mut test_y = test_pos_y.clone();

            let quant_dx = quantile(&mut test_x, 0.5) - quantile(&mut qc_x, 0.5);
            let quant_dy = quantile(&mut test_y, 0.5) - quantile(&mut qc_y, 0.5);

            let x_range = pos_x_range.1 - pos_x_range.0;
            let y_range = pos_y_range.1 - pos_y_range.0;

            // ── 3. Smear score (based on QC shape)
            let smear_score_x = compute_smear_score(&qc_pos_x, &qc_d_px, x_range)
                .max(compute_smear_score(&test_pos_x, &test_d_px, x_range));
            let smear_score_y = compute_smear_score(&qc_pos_y, &qc_d_py, y_range)
                .max(compute_smear_score(&test_pos_y, &test_d_py, y_range));

            // ── 4. Hybrid blend (THIS is the key step)
            let dx = (1.0 - smear_score_x) * peak_dx + smear_score_x * quant_dx;
            let dy = (1.0 - smear_score_y) * peak_dy + smear_score_y * quant_dy;

            Some((
                dx,
                dy,
                std_dev(&qc_pos_x),
                std_dev(&qc_pos_y),
                std_dev(&test_pos_x),
                std_dev(&test_pos_y),
                smear_score_x,
                smear_score_y,
            ))
        }
        _ => None,
    };

    let (
        positive_dx,
        positive_dy,
        qc_positive_width_x,
        qc_positive_width_y,
        test_positive_width_x,
        test_positive_width_y,
        smear_score_x,
        smear_score_y,
    ) = match positive_result {
        Some((dx, dy, qwx, qwy, twx, twy, ssx, ssy)) => (
            Some(dx),
            Some(dy),
            Some(qwx),
            Some(qwy),
            Some(twx),
            Some(twy),
            Some(ssx),
            Some(ssy),
        ),
        None => (None, None, None, None, None, None, None, None),
    };

    // ── Drift classification ──────────────────────────────────────────────────

    let neg_shifted =
        negative_dx.abs() > significant_shift || negative_dy.abs() > significant_shift;
    let neg_widened =
        width_ratio_x > significant_width_ratio || width_ratio_y > significant_width_ratio;

    let pos_shifted = match (positive_dx, positive_dy) {
        (Some(dx), Some(dy)) => dx.abs() > significant_shift || dy.abs() > significant_shift,
        _ => false,
    };

    let drift_type = if pos_emerged {
        DriftType::Biological
    } else if pos_disappeared {
        DriftType::Biological
    } else {
        match (neg_shifted, neg_widened, pos_shifted) {
            (true, false, true) => {
                let pos_dx = positive_dx.unwrap_or(0.0);
                let pos_dy = positive_dy.unwrap_or(0.0);
                let agreement_x = (pos_dx - negative_dx).abs() < significant_shift;
                let agreement_y = (pos_dy - negative_dy).abs() < significant_shift;
                if agreement_x && agreement_y {
                    DriftType::InstrumentDrift
                } else {
                    DriftType::Ambiguous
                }
            }
            (false, false, true) => DriftType::Biological,
            (false, true, false) => DriftType::CompensationIssue,
            (false, false, false) => DriftType::Clean,
            (true, false, false) => DriftType::Ambiguous,
            _ => DriftType::Ambiguous,
        }
    };

    Ok(PopulationShiftResult {
        negative_dx,
        negative_dy,
        qc_negative_width_x,
        qc_negative_width_y,
        test_negative_width_x,
        test_negative_width_y,
        width_ratio_x,
        width_ratio_y,
        positive_dx,
        positive_dy,
        qc_positive_width_x,
        qc_positive_width_y,
        test_positive_width_x,
        test_positive_width_y,
        drift_type,
        positive_smear_score_x: smear_score_x,
        positive_smear_score_y: smear_score_y,
    })
}

// ─── Region extractor ─────────────────────────────────────────────────────────

/// Returns events falling within [x_min, x_max) × [y_min, y_max).
fn extract_region(
    events: (&Column, &Column),
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
) -> Result<(Vec<f64>, Vec<f64>), String> {
    let xs = events.0.f64().map_err(|e| e.to_string())?;
    let ys = events.1.f64().map_err(|e| e.to_string())?;

    let (out_x, out_y) = xs
        .into_iter()
        .zip(ys.into_iter())
        .filter_map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) if x >= x_min && x < x_max && y >= y_min && y < y_max => {
                Some((x, y))
            }
            _ => None,
        })
        .unzip();

    Ok((out_x, out_y))
}

/// Smear score in [0.0, 1.0].
/// 0.0 = tight, well-defined cluster.
/// 1.0 = fully diffuse smear with no discernible peak.
///
/// Three independent signals are combined:
///   - KDE entropy:      flat density = high entropy = smear
///   - IQR spread:       wide middle 50% relative to the axis range = smear
///   - Tail symmetry:    heavy right tail relative to left = smear or mixed population
///
/// All three are normalised via a sigmoid so there is no hard clipping and
/// the contribution of each component degrades gracefully rather than
/// saturating at 1.0 and losing information.
///
/// `axis_range`: the length of the axis (x_max - x_min) for this population.
/// Used to normalise IQR so the score is comparable across different panels
/// and axis scales.
pub fn compute_smear_score(values: &[f64], density: &[f64], axis_range: f64) -> f64 {
    if values.len() < 4 || density.is_empty() {
        return 0.0;
    }

    // ── 1. KDE entropy ────────────────────────────────────────────────────────
    // Normalise the density to a probability distribution, then compute
    // Shannon entropy. A tight cluster concentrates mass at one point (low entropy).
    // A diffuse smear spreads mass uniformly (high entropy approaching log(n)).
    // We normalise by log(n) so the result is in [0, 1].

    let density_sum: f64 = density.iter().sum();
    let entropy = if density_sum > 0.0 {
        let max_entropy = (density.len() as f64).ln();
        if max_entropy > 0.0 {
            let h: f64 = density
                .iter()
                .map(|&d| {
                    let p = d / density_sum;
                    if p > 0.0 { -p * p.ln() } else { 0.0 }
                })
                .sum();
            (h / max_entropy).clamp(0.0, 1.0)
        } else {
            0.0
        }
    } else {
        0.0
    };

    // ── 2. IQR spread ─────────────────────────────────────────────────────────
    // IQR (q75 - q25) normalised by the axis range.
    // A tight cluster occupies a small fraction of the axis range.
    // A smear occupies a large fraction.
    // sigmoid_k controls how steeply the score rises with spread —
    // at k=10 the score reaches ~0.5 when IQR is 10% of the axis range.

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();

    let q10 = sorted[(0.10 * (n - 1) as f64) as usize];
    let q25 = sorted[(0.25 * (n - 1) as f64) as usize];
    let q50 = sorted[(0.50 * (n - 1) as f64) as usize];
    let q75 = sorted[(0.75 * (n - 1) as f64) as usize];
    let q90 = sorted[(0.90 * (n - 1) as f64) as usize];

    let iqr = q75 - q25;
    let iqr_fraction = if axis_range > 0.0 {
        iqr / axis_range
    } else {
        0.0
    };
    // sigmoid centred at 0.15 (15% of axis range) — tune this for your arcsinh scale
    let iqr_score = sigmoid(iqr_fraction, 0.15, 20.0);

    // ── 3. Tail symmetry ──────────────────────────────────────────────────────
    // Compare the upper tail (q90 - q50) to the lower tail (q50 - q10).
    // A symmetric tight cluster has a ratio near 1.0.
    // A smear or mixed population tends to have a heavy right tail:
    // ratio >> 1.0 indicates positive events trailing into a smear.
    // We score the deviation from symmetry.

    let lower_tail = (q50 - q10).max(1e-9);
    let upper_tail = q90 - q50;
    let tail_ratio = upper_tail / lower_tail;
    // sigmoid centred at ratio=2.0 — a ratio below 2 is roughly symmetric
    let tail_score = sigmoid(tail_ratio, 2.0, 1.5);

    // ── Weighted combination ──────────────────────────────────────────────────
    // Entropy is the most reliable signal and gets the most weight.
    // IQR spread is a strong secondary signal.
    // Tail symmetry is informative but can be high for legitimate skewed
    // populations (e.g. a very bright population with a dim shoulder),
    // so it carries the least weight.

    let score = 0.50 * entropy + 0.35 * iqr_score + 0.15 * tail_score;

    score.clamp(0.0, 1.0)
}

/// Logistic sigmoid: maps x to [0, 1], centred at `midpoint`,
/// with steepness controlled by `k`. Higher k = sharper transition.
///
///   f(x) = 1 / (1 + exp(-k * (x - midpoint)))
///
/// At x = midpoint, f = 0.5 exactly.
#[inline]
fn sigmoid(x: f64, midpoint: f64, k: f64) -> f64 {
    1.0 / (1.0 + (-k * (x - midpoint)).exp())
}

fn quantile(values: &mut [f64], q: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    // total_cmp rather than partial_cmp().unwrap(): a single NaN event would
    // otherwise panic the sort comparator.
    values.sort_by(|a, b| a.total_cmp(b));
    let idx = ((q.clamp(0.0, 1.0) * (values.len() - 1) as f64) as usize).min(values.len() - 1);
    values[idx]
}

//cargo test flow_tests_kde_shift -- --nocapture
// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod flow_tests_kde_shift {
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

        let result = crate::gate_move::kde::kde_negative_shift(
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

    fn run_shift(
        label: &str,
        qc: &DataFrame,
        test: &DataFrame,
        gate: &GateBoundary,
        expected_neg_dx: f64,
        expected_neg_dy: f64,
    ) {
        let result = analyse_population_shift(
            (qc.column("x").unwrap(), qc.column("y").unwrap()),
            (test.column("x").unwrap(), test.column("y").unwrap()),
            (-1.0, 4.5),
            (-1.0, 4.5),
            gate,
            0.1, // negative_margin — 0.1 units below gate edge
            512, // kde resolution
            50,  // min events
            0.1, // significant_shift threshold
            1.5, // significant_width_ratio threshold
        );

        match result {
            Ok(r) => println!(
                "[{label}]\n  negative shift: dx={:.4}, dy={:.4}  (expected dx={:.4}, dy={:.4})\n  error:          dx={:.4}, dy={:.4}\n  width ratio:    x={:.2}, y={:.2}{}{}\n  positive shift: {}\n  smear score:    x={}, y={}\n  drift type:     {}\n",
                r.negative_dx,
                r.negative_dy,
                expected_neg_dx,
                expected_neg_dy,
                (r.negative_dx - expected_neg_dx).abs(),
                (r.negative_dy - expected_neg_dy).abs(),
                r.width_ratio_x,
                r.width_ratio_y,
                if r.width_warning_x(1.5) {
                    "  ⚠ x wider"
                } else {
                    ""
                },
                if r.width_warning_y(1.5) {
                    "  ⚠ y wider"
                } else {
                    ""
                },
                match (r.positive_dx, r.positive_dy) {
                    (Some(dx), Some(dy)) => format!("dx={dx:.4}, dy={dy:.4}"),
                    _ => "no positive population".into(),
                },
                match r.positive_smear_score_x {
                    Some(v) => format!("{:.3}", v),
                    None => "-".into(),
                },
                match r.positive_smear_score_y {
                    Some(v) => format!("{:.3}", v),
                    None => "-".into(),
                },
                r.drift_type,
            ),
            Err(e) => println!("[{label}] Err: {e}\n"),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Additional test data generators
    // ─────────────────────────────────────────────────────────────────────────────

    // ── Requested ─────────────────────────────────────────────────────────────────

    /// Distinct positive in both, but shifted +0.4 on both axes in the test.
    /// Negative identical. Expect: neg shift ≈ 0, pos shift ≈ +0.4.
    /// Drift classification should be Biological.
    pub fn distinct_positive_shifted_in_test(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Distinct positive in both, but wider in the test on both axes.
    /// Negative identical. Expect: neg shift ≈ 0, pos width ratio > 1.
    /// Could indicate increased non-specific binding or a broader expression range.
    pub fn distinct_positive_wider_in_test(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
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
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.45,
                    sy: 0.45,
                    n: 900,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Smeared positive in both, but the smear has shifted upward in the test.
    /// Smear centre moves from (1.5, 1.5) to (1.9, 1.9).
    pub fn smear_positive_shifted_in_test(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc_neg = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        let qc_smear = sample_smear((0.9, 2.1), (0.9, 2.1), 600, &mut rng);
        let (qc_x, qc_y) = concat_events(qc_neg, qc_smear);

        let test_neg = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        let test_smear = sample_smear((1.3, 2.5), (1.3, 2.5), 600, &mut rng); // shifted
        let (test_x, test_y) = concat_events(test_neg, test_smear);

        (make_df(qc_x, qc_y), make_df(test_x, test_y))
    }

    /// Smeared positive in both, but the smear is wider in the test.
    pub fn smear_positive_wider_in_test(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc_neg = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        let qc_smear = sample_smear((1.2, 2.0), (1.2, 2.0), 600, &mut rng);
        let (qc_x, qc_y) = concat_events(qc_neg, qc_smear);

        let test_neg = sample_clusters(
            &[Cluster {
                cx: 0.4,
                cy: 0.4,
                sx: 0.12,
                sy: 0.12,
                n: 3000,
            }],
            &mut rng,
        );
        let test_smear = sample_smear((0.8, 2.8), (0.8, 2.8), 600, &mut rng); // wider range
        let (test_x, test_y) = concat_events(test_neg, test_smear);

        (make_df(qc_x, qc_y), make_df(test_x, test_y))
    }

    // ── Suggested additional cases ─────────────────────────────────────────────────

    /// Negative and positive both shifted by the same amount (+0.3, +0.3).
    /// The textbook instrument drift case — everything moved uniformly.
    /// Expect: neg shift ≈ +0.3, pos shift ≈ +0.3, DriftType::InstrumentDrift.
    pub fn uniform_instrument_drift(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.7,
                    cy: 0.7,
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

    /// Negative shifted +0.3, positive shifted +0.7 — different amounts.
    /// Could be a voltage drift that has a non-linear effect across the dynamic range.
    /// Expect: DriftType::Ambiguous.
    pub fn differential_drift(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.7,
                    cy: 0.7,
                    sx: 0.12,
                    sy: 0.12,
                    n: 3000,
                },
                Cluster {
                    cx: 3.2,
                    cy: 3.2,
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Negative both shifted AND wider in the test.
    /// Suggests a voltage change that moved and spread the negative simultaneously.
    /// Expect: neg shift ≈ +0.3, width ratio > 1.5, DriftType::Ambiguous.
    pub fn negative_shifted_and_wider(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.7,
                    cy: 0.7,
                    sx: 0.30,
                    sy: 0.30,
                    n: 3000,
                },
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Bimodal negative — two clusters in the lower-left quadrant.
    /// Could happen with a mixed sample (two cell types with different autofluorescence)
    /// or with a compensation artefact that splits the negative.
    /// Tests robustness of KDE peak-finding when the negative is not unimodal.
    pub fn bimodal_negative(seed: u64) -> (DataFrame, DataFrame) {
        let mut rng = StdRng::seed_from_u64(seed);
        let qc = sample_clusters(
            &[
                Cluster {
                    cx: 0.3,
                    cy: 0.3,
                    sx: 0.10,
                    sy: 0.10,
                    n: 2000,
                },
                Cluster {
                    cx: 0.8,
                    cy: 0.6,
                    sx: 0.10,
                    sy: 0.10,
                    n: 1000,
                }, // second negative mode
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.18,
                    sy: 0.18,
                    n: 800,
                },
            ],
            &mut rng,
        );
        let test = sample_clusters(
            &[
                Cluster {
                    cx: 0.3,
                    cy: 0.3,
                    sx: 0.10,
                    sy: 0.10,
                    n: 2000,
                },
                Cluster {
                    cx: 0.8,
                    cy: 0.6,
                    sx: 0.10,
                    sy: 0.10,
                    n: 1000,
                },
                Cluster {
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.18,
                    sy: 0.18,
                    n: 800,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Very sparse positive population in the test (e.g. a rare antigen).
    /// Tests the min_events guard — should degrade gracefully to no positive analysis.
    pub fn sparse_positive_in_test(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
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
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.18,
                    sy: 0.18,
                    n: 20,
                }, // below min_events
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Dead cell / debris cluster in the lower-right quadrant — high x, low y.
    /// Common in clinical samples. Should not affect the negative KDE since
    /// it sits outside the negative region (x > gate lower edge).
    pub fn debris_cluster(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        let test_main = sample_clusters(
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        // Debris: high x, low y — lower-right, outside the negative extraction region
        let debris = sample_clusters(
            &[Cluster {
                cx: 3.2,
                cy: 0.3,
                sx: 0.40,
                sy: 0.20,
                n: 500,
            }],
            &mut rng,
        );
        let (test_x, test_y) = concat_events(test_main, debris);
        (make_df(qc.0, qc.1), make_df(test_x, test_y))
    }

    /// High-expression sample — positive population sits near the top of the axis range.
    /// Tests that the positive KDE doesn't get clipped by the axis boundary.
    pub fn high_expression_positive(seed: u64) -> (DataFrame, DataFrame) {
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
                    cx: 4.0,
                    cy: 4.0,
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                }, // near axis edge
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
                    cx: 4.0,
                    cy: 4.0,
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                },
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Much larger positive population in the test (e.g. patient is responding to treatment).
    /// Negative identical. Purely biological. Expect: neg shift ≈ 0, DriftType::Biological.
    pub fn larger_positive_in_test(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 300,
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
                    cx: 2.5,
                    cy: 2.5,
                    sx: 0.18,
                    sy: 0.18,
                    n: 2000,
                }, // much larger
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    /// Negative shifted on x only, positive shifted on both axes.
    /// Tests that axis-independent shifts are correctly decomposed.
    pub fn asymmetric_drift(seed: u64) -> (DataFrame, DataFrame) {
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
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
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
                }, // x only
                Cluster {
                    cx: 2.8,
                    cy: 2.8,
                    sx: 0.18,
                    sy: 0.18,
                    n: 900,
                }, // both axes
            ],
            &mut rng,
        );
        (make_df(qc.0, qc.1), make_df(test.0, test.1))
    }

    // ─── Tests ────────────────────────────────────────────────────────────────────

    #[cfg(test)]
    mod extended_flow_tests {
        use super::*;

        fn run_shift(
            label: &str,
            qc: &DataFrame,
            test: &DataFrame,
            gate: &GateBoundary,
            expected_neg_dx: f64,
            expected_neg_dy: f64,
        ) {
            let result = analyse_population_shift(
                (qc.column("x").unwrap(), qc.column("y").unwrap()),
                (test.column("x").unwrap(), test.column("y").unwrap()),
                (-1.0, 4.5),
                (-1.0, 4.5),
                gate,
                0.1, // negative_margin
                512, // kde resolution
                50,  // min events
                0.1, // significant_shift
                1.5, // significant_width_ratio
            );

            match result {
                Ok(r) => println!(
                    "[{label}]\n\
                   neg shift:    dx={:.4}, dy={:.4}  (expected dx={:.4}, dy={:.4})\n\
                   neg error:    dx={:.4}, dy={:.4}\n\
                   width ratio:  x={:.2}, y={:.2}{}{}\n\
                   pos shift:    {}\n\
                   pos width:    {}\n\
                   drift type:   {}\n",
                    r.negative_dx,
                    r.negative_dy,
                    expected_neg_dx,
                    expected_neg_dy,
                    (r.negative_dx - expected_neg_dx).abs(),
                    (r.negative_dy - expected_neg_dy).abs(),
                    r.width_ratio_x,
                    r.width_ratio_y,
                    if r.width_warning_x(1.5) {
                        "  ⚠ x wider"
                    } else {
                        ""
                    },
                    if r.width_warning_y(1.5) {
                        "  ⚠ y wider"
                    } else {
                        ""
                    },
                    match (r.positive_dx, r.positive_dy) {
                        (Some(dx), Some(dy)) => format!("dx={dx:.4}, dy={dy:.4}"),
                        _ => "no positive population detected".into(),
                    },
                    match (r.test_positive_width_x, r.test_positive_width_y) {
                        (Some(wx), Some(wy)) => format!("test x={wx:.4}, y={wy:.4}"),
                        _ => "n/a".into(),
                    },
                    r.drift_type,
                ),
                Err(e) => println!("[{label}] Err: {e}\n"),
            }
        }

        fn gate() -> GateBoundary {
            GateBoundary {
                x_lower: 1.2,
                y_lower: 1.2,
            }
        }

        // ── Requested ──────────────────────────────────────────────────────────────

        #[test]
        fn test_distinct_positive_shifted() {
            // Neg should be clean, pos should show the shift as biological.
            let (qc, test) = distinct_positive_shifted_in_test(42);
            run_shift("distinct_positive_shifted", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_distinct_positive_wider() {
            // Neg clean, pos wider — no shift but increased spread in test.
            let (qc, test) = distinct_positive_wider_in_test(42);
            run_shift("distinct_positive_wider", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_smear_positive_shifted() {
            // Neg clean. Smear has moved. Positive shift should be visible.
            let (qc, test) = smear_positive_shifted_in_test(42);
            run_shift("smear_positive_shifted", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_smear_positive_wider() {
            // Neg clean. Smear has broadened.
            let (qc, test) = smear_positive_wider_in_test(42);
            run_shift("smear_positive_wider", &qc, &test, &gate(), 0.0, 0.0);
        }

        // ── Suggested ──────────────────────────────────────────────────────────────

        #[test]
        fn test_uniform_instrument_drift() {
            // Both neg and pos shifted by same amount — should be InstrumentDrift.
            let (qc, test) = uniform_instrument_drift(42);
            run_shift("uniform_instrument_drift", &qc, &test, &gate(), 0.3, 0.3);
        }

        #[test]
        fn test_differential_drift() {
            // Neg and pos shifted by different amounts — Ambiguous.
            let (qc, test) = differential_drift(42);
            run_shift("differential_drift", &qc, &test, &gate(), 0.3, 0.3);
        }

        #[test]
        fn test_negative_shifted_and_wider() {
            // Neg both shifted and broader — voltage change.
            let (qc, test) = negative_shifted_and_wider(42);
            run_shift("negative_shifted_and_wider", &qc, &test, &gate(), 0.3, 0.3);
        }

        #[test]
        fn test_bimodal_negative() {
            // Two clusters in the negative quadrant — KDE should lock onto the larger one.
            // No expected shift since the clusters are identical between QC and test.
            let (qc, test) = bimodal_negative(42);
            run_shift("bimodal_negative", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_sparse_positive_in_test() {
            // Positive below min_events in test — should degrade to None gracefully.
            let (qc, test) = sparse_positive_in_test(42);
            run_shift("sparse_positive_in_test", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_debris_cluster() {
            // Debris in lower-right — should not affect the negative region analysis.
            let (qc, test) = debris_cluster(42);
            run_shift("debris_cluster", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_high_expression_positive() {
            // Positive near axis ceiling — tests KDE doesn't get clipped.
            let (qc, test) = high_expression_positive(42);
            run_shift("high_expression_positive", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_larger_positive_in_test() {
            // Much bigger positive in test — purely biological, neg should be clean.
            let (qc, test) = larger_positive_in_test(42);
            run_shift("larger_positive_in_test", &qc, &test, &gate(), 0.0, 0.0);
        }

        #[test]
        fn test_asymmetric_drift() {
            // Neg shifted on x only, pos shifted on both.
            // Tests that x and y are decomposed independently.
            let (qc, test) = asymmetric_drift(42);
            run_shift("asymmetric_drift", &qc, &test, &gate(), 0.3, 0.0);
        }
    }

    #[test]
    fn test_all_scenarios() {
        // Gate sits at (1.2, 1.2) — negatives cluster around (0.4, 0.4) so
        // they sit well below the gate edge, positives around (2.5, 2.5).
        let gate = GateBoundary {
            x_lower: 1.2,
            y_lower: 1.2,
        };

        let cases: &[(&str, fn(u64) -> (DataFrame, DataFrame), f64, f64)] = &[
            ("wider_negative_x", wider_negative_x, 0.0, 0.0),
            ("wider_negative_y", wider_negative_y, 0.0, 0.0),
            ("negative_shifted_x", negative_shifted_x, 0.3, 0.0),
            ("negative_shifted_y", negative_shifted_y, 0.0, 0.3),
            ("positive_only_in_qc", positive_only_in_qc, 0.0, 0.0),
            ("positive_only_in_test", positive_only_in_test, 0.0, 0.0),
            (
                "smeared_positive_only_in_qc",
                smeared_positive_only_in_qc,
                0.0,
                0.0,
            ),
            (
                "smeared_positive_only_in_test",
                smeared_positive_only_in_test,
                0.0,
                0.0,
            ),
            (
                "positive_shifted_in_test",
                positive_shifted_in_test,
                0.0,
                0.0,
            ),
        ];

        for (label, make_data, exp_dx, exp_dy) in cases {
            let (qc, test) = make_data(42);
            run_shift(label, &qc, &test, &gate, *exp_dx, *exp_dy);
        }
    }
}
