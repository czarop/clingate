//! What a population *is*, rather than where it sits on one plot.
//!
//! The rules in [`rule`](super::rule) all position a gate by reading one
//! parameter: find the negative, find the valley, take a percentage. That works
//! where a population is separated along a single axis, and it has nothing to
//! say where it is not - a smear with no dip, several clusters near each other,
//! a population that moves in both axes at once. MAIT cells are the case that
//! defeats every one of them.
//!
//! This module answers the question those rules cannot ask: *are these the same
//! cells?* A population is described by where it sits across every marker on
//! the panel - its phenotype - and that description is then used to find the
//! same cells in another sample. Where they turn out to be on the plot is an
//! answer rather than an assumption, so the gate can be moved and reshaped to
//! fit them rather than pushed along one axis and hoped for.
//!
//! ## Each sample is its own reference frame
//!
//! Comparing a marker's raw value between two samples would need the two to be
//! on the same scale, which is run-to-run normalisation - explicitly not wanted
//! here, because it flattens the donor differences that are the entire point.
//!
//! So nothing is compared in raw units. Every marker is expressed as a robust
//! z against *the parent population of the same sample*: how far this cell sits
//! from the middle of the population it was gated out of, in units of that
//! population's own spread. A signature says "high for CD161 relative to its
//! parent, middling for CD4", which is a statement about the cell's place among
//! its own neighbours, and means the same thing in every sample without
//! anything being rescaled across samples.
//!
//! Median and MAD rather than mean and standard deviation, because the parent
//! contains the very population being described - a bright 5% would drag a mean
//! and inflate a standard deviation, and shrink the z of the cells that caused
//! it.
//!
//! ## The baseline is measured with the tail cut off
//!
//! Robust is not enough on its own. A MAD is contaminated by the population it
//! is about to measure, and the contamination scales with how common that
//! population is - which is precisely what differs between samples. Measured on
//! a population sitting 14 parent-widths out:
//!
//! | population is | z of the population, plain MAD | tail excluded |
//! |---------------|-------------------------------|---------------|
//! | 1% of parent  | 13.68                         | 13.87         |
//! | 7%            | 12.53                         | 13.81         |
//! | 20%           |  9.82                         | 13.78         |
//!
//! Untreated, the same cells read four widths lower in a sample that has more
//! of them, which would put the gate somewhere else for no reason but abundance
//! - the 7%-against-1% case. Excluding everything past [`TRIM`] spreads before
//! taking the median and the MAD removes the dependence completely. The same
//! estimator runs on the reference and on every sample, so whatever bias
//! truncation leaves is the same bias on both sides.
//!
//! What remains is sampling noise, about 0.5 of a width at that distance, and
//! that is what [`BASELINE_SLIP`] is for.

use std::sync::Arc;

/// The markers a phenotype is measured in, and where a population sits in them.
///
/// Both the centre and the spread are needed. A population is not a point: it
/// is elongated in some directions and tight in others, and markers move
/// together - a cell high for one activation marker is usually high for the
/// next. Scoring each marker on its own would count those twice and would treat
/// a wide direction as strictly as a narrow one. The covariance is what makes
/// the distance below an honest measure of "unlike these cells".
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    /// The channels, in the order every vector here uses.
    pub markers: Vec<Arc<str>>,
    /// The population's centre, in robust-z units against its own parent.
    pub centre: Vec<f64>,
    /// The inverse of the population's covariance in that space, row-major.
    pub precision: Vec<f64>,
    /// The distance that takes in [`KEEP`] of the reference population's own
    /// members - the radius that describes these cells, measured rather than
    /// assumed.
    pub cut: f64,
    /// How many events the signature was built from. A signature drawn from a
    /// handful of cells is a guess, and the report says so rather than the
    /// solver silently trusting it.
    pub members: usize,
}

/// The fraction of a population the distance cut takes in.
///
/// Not all of it: the last few percent of any gated population are the events
/// nearest the line the person drew, and half of them are there because a hand
/// drawn boundary has to fall somewhere. Taking the radius that holds 95%
/// describes the population rather than the edge of the drawing.
pub const KEEP: f64 = 0.95;

/// Added to the covariance diagonal before inverting it.
///
/// Two markers that move together almost perfectly - and panels are full of
/// them - make the covariance singular or nearly so, and its inverse then has
/// enormous entries in the direction with no spread. A cell a hair off that
/// direction scores as infinitely unlike the population. A small ridge is the
/// standard answer: it says "no direction is narrower than this", which is
/// honest about what a finite sample can resolve.
pub const RIDGE: f64 = 1e-3;

/// A population's middle and spread on one marker, from its parent.
///
/// The pair a robust z is taken against. Held per marker for one sample.
#[derive(Clone, Debug, PartialEq)]
pub struct Baseline {
    pub median: f64,
    /// The median absolute deviation, scaled so it estimates a standard
    /// deviation for normally distributed values.
    pub spread: f64,
}

/// 1.4826: the factor that makes a MAD estimate the standard deviation of a
/// normal distribution, so a robust z and an ordinary one read the same.
const MAD_TO_SIGMA: f64 = 1.482_602_218_505_602;

/// The smallest spread a marker is allowed to have.
///
/// A channel where more than half the parent shares one value - an empty
/// detector, a saturated one - has a MAD of zero, and dividing by it makes
/// every cell infinitely far from the middle. Flooring it turns a degenerate
/// marker into an uninformative one, which is what it is.
const MIN_SPREAD: f64 = 1e-6;

/// How far out a value may be and still count towards the baseline.
///
/// Four spreads keeps the bulk of any population that is vaguely unimodal and
/// drops the bright or dim cluster whose abundance would otherwise set the
/// scale everything else is measured against. See the module comment.
pub const TRIM: f64 = 4.0;

/// How much of a population's distance from the middle is uncertainty.
///
/// A baseline is estimated from a finite sample, so its spread carries a few
/// percent of noise, and that noise multiplies with distance: a population
/// sitting 14 widths out moves half a width when the estimate moves 3%. Tight
/// populations far from the middle are therefore known less precisely than
/// their own spread suggests, and a signature that believed its own spread
/// would reject the very cells it describes in the next sample.
///
/// So each marker's variance gains `(centre * BASELINE_SLIP)^2`. It is a
/// statement about what a finite sample can resolve, not a fudge factor.
pub const BASELINE_SLIP: f64 = 0.10;

impl Baseline {
    /// Read one marker's middle and spread from a population, with the tail
    /// that a gated population would sit in excluded.
    ///
    /// Two passes: a plain robust estimate, then the same estimate over the
    /// values it says are within [`TRIM`] spreads. One pass is enough - the
    /// first estimate is already robust enough to say which values are far out,
    /// it is only its *scale* that the far-out ones distort.
    pub fn of(values: &[f64]) -> Self {
        let rough = Self::untrimmed(values);
        let kept: Vec<f64> = values
            .iter()
            .copied()
            .filter(|v| rough.z(*v).abs() <= TRIM)
            .collect();
        // Everything is in the tail, so there is no bulk to prefer. Keeping the
        // rough estimate beats reporting the spread of an empty set.
        if kept.len() < 8 || kept.len() == values.len() {
            return rough;
        }
        Self::untrimmed(&kept)
    }

    fn untrimmed(values: &[f64]) -> Self {
        let median = median_of(values);
        let mut deviations: Vec<f64> = values.iter().map(|v| (v - median).abs()).collect();
        let spread = (median_of_mut(&mut deviations) * MAD_TO_SIGMA).max(MIN_SPREAD);
        Self { median, spread }
    }

    /// Where one value sits, in spreads from the middle.
    pub fn z(&self, value: f64) -> f64 {
        (value - self.median) / self.spread
    }
}

/// The baseline for every marker of one sample's parent population.
///
/// `rows` is one row per event, each holding the markers in a fixed order.
pub fn baselines(rows: &[Vec<f64>], markers: usize) -> Vec<Baseline> {
    (0..markers)
        .map(|at| {
            let column: Vec<f64> = rows.iter().filter_map(|row| row.get(at).copied()).collect();
            Baseline::of(&column)
        })
        .collect()
}

/// Put a population into robust-z space against its parent's baselines.
pub fn to_z(rows: &[Vec<f64>], baselines: &[Baseline]) -> Vec<Vec<f64>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .zip(baselines.iter())
                .map(|(value, base)| base.z(*value))
                .collect()
        })
        .collect()
}

fn median_of(values: &[f64]) -> f64 {
    let mut copy = values.to_vec();
    median_of_mut(&mut copy)
}

fn median_of_mut(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

/// The mean of each column.
fn centre_of(rows: &[Vec<f64>], markers: usize) -> Vec<f64> {
    let mut centre = vec![0.0; markers];
    if rows.is_empty() {
        return centre;
    }
    for row in rows {
        for (at, value) in row.iter().enumerate().take(markers) {
            centre[at] += value;
        }
    }
    for value in &mut centre {
        *value /= rows.len() as f64;
    }
    centre
}

/// The covariance of a population, row-major and symmetric.
pub fn covariance(rows: &[Vec<f64>], markers: usize, centre: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; markers * markers];
    if rows.len() < 2 {
        // One event has no spread to measure. An identity covariance says "I
        // know where it is and nothing about its shape", which is true, and
        // leaves the distance below an ordinary euclidean one.
        for at in 0..markers {
            out[at * markers + at] = 1.0;
        }
        return out;
    }
    for row in rows {
        for i in 0..markers {
            let di = row[i] - centre[i];
            for j in i..markers {
                out[i * markers + j] += di * (row[j] - centre[j]);
            }
        }
    }
    let n = (rows.len() - 1) as f64;
    for i in 0..markers {
        for j in i..markers {
            let value = out[i * markers + j] / n;
            out[i * markers + j] = value;
            out[j * markers + i] = value;
        }
    }
    out
}

/// Invert a symmetric positive-definite matrix, with a ridge on the diagonal.
///
/// Cholesky rather than a general inverse: the matrix is a covariance, so it is
/// symmetric and - once the ridge is on - positive definite, and the
/// decomposition both exploits that and reports when it does not hold. `None`
/// means the matrix was not positive definite even with the ridge, which for a
/// covariance means something is wrong with the data rather than with the
/// arithmetic.
pub fn invert_spd(matrix: &[f64], n: usize, ridge: f64) -> Option<Vec<f64>> {
    if n == 0 || matrix.len() != n * n {
        return None;
    }
    // L, lower triangular, with A + ridge*I = L L^T.
    let mut l = vec![0.0f64; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = matrix[i * n + j];
            if i == j {
                sum += ridge;
            }
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if sum <= 0.0 || !sum.is_finite() {
                    return None;
                }
                l[i * n + i] = sum.sqrt();
            } else {
                l[i * n + j] = sum / l[j * n + j];
            }
        }
    }

    // Invert L in place into `li`, then A^-1 = L^-T L^-1.
    let mut li = vec![0.0f64; n * n];
    for i in 0..n {
        li[i * n + i] = 1.0 / l[i * n + i];
        for j in 0..i {
            let mut sum = 0.0;
            for k in j..i {
                sum += l[i * n + k] * li[k * n + j];
            }
            li[i * n + j] = -sum / l[i * n + i];
        }
    }

    let mut out = vec![0.0f64; n * n];
    for i in 0..n {
        for j in 0..n {
            let mut sum = 0.0;
            for k in i.max(j)..n {
                sum += li[k * n + i] * li[k * n + j];
            }
            out[i * n + j] = sum;
            out[j * n + i] = sum;
        }
    }
    Some(out)
}

/// How unlike the signature one event is, squared.
///
/// Mahalanobis: the distance measured in the population's own shape, so a step
/// along a direction the population is wide in counts for less than the same
/// step where it is narrow.
pub fn distance_squared(signature: &Signature, z: &[f64]) -> f64 {
    let n = signature.centre.len();
    if z.len() < n {
        return f64::INFINITY;
    }
    let mut delta = vec![0.0; n];
    for at in 0..n {
        delta[at] = z[at] - signature.centre[at];
    }
    let mut total = 0.0;
    for i in 0..n {
        let mut row = 0.0;
        for j in 0..n {
            row += signature.precision[i * n + j] * delta[j];
        }
        total += delta[i] * row;
    }
    total.max(0.0)
}

impl Signature {
    /// Describe a population by where its members sit across the markers.
    ///
    /// `members` are the events inside the reference gate and `parent` is the
    /// population they were gated out of, both as raw marker values in the same
    /// column order. The parent is what the members are measured against - see
    /// the module comment - so it has to be the same sample's.
    pub fn describe(
        markers: Vec<Arc<str>>,
        members: &[Vec<f64>],
        parent: &[Vec<f64>],
    ) -> Option<Self> {
        let n = markers.len();
        if n == 0 || members.is_empty() {
            return None;
        }
        let baselines = baselines(parent, n);
        let z = to_z(members, &baselines);
        let centre = centre_of(&z, n);
        let mut spread = covariance(&z, n, &centre);
        // What the baseline cannot resolve, added to what the population's own
        // shape says. See [`BASELINE_SLIP`].
        for at in 0..n {
            spread[at * n + at] += (centre[at] * BASELINE_SLIP).powi(2);
        }
        let precision = invert_spd(&spread, n, RIDGE)?;

        let mut draft = Self {
            markers,
            centre,
            precision,
            cut: f64::INFINITY,
            members: members.len(),
        };

        // The cut has to allow for two things, and the larger of them wins.
        //
        // What the population's own members actually do: a gated population is
        // not normal - it is whatever the person drew round - so the radius
        // holding `KEEP` of them is measured rather than assumed.
        let mut distances: Vec<f64> = z.iter().map(|row| distance_squared(&draft, row)).collect();
        let measured = quantile_of_mut(&mut distances, KEEP);

        // And what the metric itself implies. The covariance carries the
        // baseline slip, which is a distance the members of *another* sample
        // will be moved by and this sample's members will not: the reference's
        // own spread is measured against a baseline it was measured with. So
        // its members sit inside the radius the slip allows for, and a cut
        // taken from them alone would be tighter than the metric it is applied
        // in - rejecting the very cells the slip exists to admit.
        let modelled = chi_squared_quantile(n, KEEP);
        draft.cut = measured.max(modelled);
        Some(draft)
    }

    /// Which of `parent`'s events look like these cells.
    ///
    /// The parent is baselined against itself, so this sample is measured in
    /// its own frame and nothing is rescaled across samples.
    pub fn find_in(&self, parent: &[Vec<f64>]) -> Matched {
        let n = self.markers.len();
        let baselines = baselines(parent, n);
        let z = to_z(parent, &baselines);
        let distances: Vec<f64> = z.iter().map(|row| distance_squared(self, row)).collect();
        let members: Vec<usize> = distances
            .iter()
            .enumerate()
            .filter(|(_, d)| **d <= self.cut)
            .map(|(at, _)| at)
            .collect();
        Matched {
            members,
            distances,
            parent: parent.len(),
        }
    }
}

/// The events of one sample that match a signature.
pub struct Matched {
    /// Indices into the parent population.
    pub members: Vec<usize>,
    /// Every parent event's squared distance from the signature, in the same
    /// order as the parent. Kept whole rather than filtered, because how the
    /// distances are distributed is the evidence for whether the population is
    /// there at all.
    pub distances: Vec<f64>,
    pub parent: usize,
}

impl Matched {
    /// What fraction of the parent matched.
    pub fn fraction(&self) -> f64 {
        if self.parent == 0 {
            return 0.0;
        }
        self.members.len() as f64 / self.parent as f64
    }
}

/// The value a chi-squared with `k` degrees of freedom falls below with
/// probability `p`.
///
/// Wilson-Hilferty: a chi-squared's cube root is very nearly normal, which
/// turns the quantile into one line and is within a fraction of a percent
/// across the range this uses - far inside the precision the covariance it is
/// applied to can claim.
pub fn chi_squared_quantile(k: usize, p: f64) -> f64 {
    if k == 0 {
        return 0.0;
    }
    let k = k as f64;
    let z = normal_quantile(p);
    let term = 1.0 - 2.0 / (9.0 * k) + z * (2.0 / (9.0 * k)).sqrt();
    (k * term * term * term).max(0.0)
}

/// The standard normal's inverse cumulative distribution.
///
/// Acklam's rational approximation, accurate to about one part in a billion,
/// which is far more than anything here needs; it is used because a shorter
/// approximation would be no simpler to read and would need its error
/// justifying.
pub fn normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_690e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const LOW: f64 = 0.024_25;

    let p = p.clamp(1e-12, 1.0 - 1e-12);
    if p < LOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - LOW {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -normal_quantile(1.0 - p)
    }
}

fn quantile_of_mut(values: &mut [f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = ((values.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    values[at]
}
