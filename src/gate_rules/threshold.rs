//! Solving for the position of a one-dimensional threshold.
//!
//! Every rule here reduces to the same shape: given the values of the parent
//! population on one axis, return the coordinate at which to put the gate edge.
//! The caller decides which axis and which edge, so the same solvers serve a
//! rectangle whose left edge moves, a bisector arm, or a quadrant centre line.
//!
//! Two conventions, both chosen to match what the rest of the editor already
//! does rather than to be tidy in isolation:
//!
//! - A gate admits an event when its value is **strictly greater** than the
//!   lower edge, because that is what `filter_events_to_mask` does
//!   (`gt(min) & lt(max)`). Counting any other way would report a fraction the
//!   gate does not actually capture.
//! - Values arrive in the axis's **display space** - arcsinh for a fluorescence
//!   channel, linear for scatter. Quantiles do not care, since a monotone
//!   transform preserves order, but an offset in data units very much does: it
//!   is a visual shift, and the space the analyst sees is the arcsinh one.

/// Where a threshold ended up, and what the gate would actually capture there.
///
/// `events_admitted` is counted at the chosen coordinate rather than assumed
/// from the target, so ties in the data cannot make it lie.
#[derive(Debug, Clone, PartialEq)]
pub struct Threshold {
    /// The coordinate for the gate edge, in the axis's display space.
    pub x: f64,
    /// How many events the gate admits there.
    pub events_admitted: usize,
    /// Those events as a fraction of the parent population.
    pub fraction_admitted: f64,
    /// How many finite events the rule was solved against.
    pub parent_events: usize,
    /// How much the gate's contents move when the edge does: the change in
    /// admitted count over a window of [`STABILITY_WINDOW`] either side,
    /// relative to the count itself.
    ///
    /// Zero means the edge sits in empty space and nudging it changes nothing.
    /// A swing of 1 means a small move changes the contents by as much as the
    /// gate holds.
    ///
    /// This replaced the width of the gap between the two events the edge
    /// separates, which looked like the obvious measure and is useless in
    /// practice: in a continuum the gap between adjacent order statistics is
    /// about the spread divided by the event count, so on 50,000 real events it
    /// came out between 0.0003 and 0.006 of the interquartile width on every
    /// fluorescence channel - never approaching 1, and ranking nothing. A window
    /// wide enough to contain many events asks the question that actually
    /// matters: move this gate slightly, and does the answer change?
    pub count_swing: f64,
    /// The interquartile width of the parent population, for reading
    /// `separation` and any displacement against the scale of the data rather
    /// than in absolute units.
    pub parent_spread: f64,
    pub status: Status,
}

/// Whether the rule was satisfiable, for the report.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// The fraction the gate captures lies inside the band the rule asked for.
    InBand,
    /// It does not, and no position would have: a band is a range of fractions,
    /// but a gate can only admit a whole number of events, so a band narrower
    /// than one event's worth may contain no achievable fraction at all. This
    /// is the low-count case - the threshold is the closest achievable, and the
    /// caller should flag it rather than trust it.
    OutOfBand { band: (f64, f64) },
    /// The rule named a position directly, so there was no band to satisfy.
    NoBand,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// The parent population is empty, so there is nothing to position against.
    NoEvents,
    /// Every value was NaN or infinite.
    NoFiniteValues,
    /// A fraction outside 0..=1, or a band whose lower bound exceeds its upper.
    BadBand { band: (f64, f64) },
    /// A percentile outside 0..=100.
    BadPercentile(f64),
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::NoEvents => write!(f, "the parent population is empty"),
            SolveError::NoFiniteValues => write!(f, "no finite values in the parent population"),
            SolveError::BadBand { band } => {
                write!(
                    f,
                    "a band of {} to {} is not a fraction range",
                    band.0, band.1
                )
            }
            SolveError::BadPercentile(p) => write!(f, "{p} is not a percentile"),
        }
    }
}

impl std::error::Error for SolveError {}

/// The finite values, sorted large to small.
fn descending(values: &[f64]) -> Result<Vec<f64>, SolveError> {
    if values.is_empty() {
        return Err(SolveError::NoEvents);
    }
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return Err(SolveError::NoFiniteValues);
    }
    // Every value is finite, so the comparison is total.
    sorted.sort_by(|a, b| b.partial_cmp(a).expect("finite values compare"));
    Ok(sorted)
}

/// What a gate at `x` admits, counted the way the filter counts it.
fn admitted(sorted_desc: &[f64], x: f64) -> (usize, f64) {
    let n = sorted_desc.partition_point(|v| *v > x);
    (n, n as f64 / sorted_desc.len() as f64)
}

/// Position a gate edge so it admits a fraction of the parent population inside
/// `band`, aiming for the middle of the band.
///
/// The band is a range of acceptable fractions, not a target, so the first job
/// is to turn it into a whole number of events: the midpoint fraction gives a
/// count, and that count is pulled inside the band's own integer range whenever
/// one exists, so a satisfiable rule is satisfied.
///
/// The coordinate then goes **midway between the two events that bracket that
/// count**. Any coordinate in that gap admits the same events, so the midpoint
/// is simply the choice furthest from either neighbour - the one least likely
/// to change what the gate captures when the next sample's cells land a little
/// differently. It is also where a person puts the line by eye.
pub fn tail_fraction(values: &[f64], band: (f64, f64)) -> Result<Threshold, SolveError> {
    let (lower, upper) = band;
    if !(0.0..=1.0).contains(&lower) || !(0.0..=1.0).contains(&upper) || lower > upper {
        return Err(SolveError::BadBand { band });
    }
    let sorted = descending(values)?;
    let n = sorted.len();
    let n_f = n as f64;

    // The midpoint of the band, as a count.
    let mut target = ((lower + upper) / 2.0 * n_f).round() as usize;
    // The counts the band itself allows. When the band spans less than one
    // event this range is empty and nothing can satisfy it.
    let band_lo = (lower * n_f).ceil() as usize;
    let band_hi = (upper * n_f).floor() as usize;
    if band_lo <= band_hi {
        target = target.clamp(band_lo, band_hi);
    }
    let target = nearest_achievable(&sorted, target.min(n));

    let x = if target == 0 {
        // Admit nothing: sit on the largest event, which is strictly outside.
        sorted[0]
    } else if target == n {
        // Admit everything: sit below the smallest.
        let span = sorted[0] - sorted[n - 1];
        sorted[n - 1] - if span > 0.0 { span / n_f } else { 1.0 }
    } else {
        // `target` events lie above sorted[target]; put the edge between the
        // last one admitted and the first one excluded.
        (sorted[target - 1] + sorted[target]) / 2.0
    };

    let spread = interquartile_spread(&sorted);
    let (events_admitted, fraction_admitted) = admitted(&sorted, x);
    let status = if (lower..=upper).contains(&fraction_admitted) {
        Status::InBand
    } else {
        Status::OutOfBand { band }
    };

    Ok(Threshold {
        x,
        events_admitted,
        fraction_admitted,
        parent_events: n,
        count_swing: count_swing_at(&sorted, x, spread),
        parent_spread: spread,
        status,
    })
}

/// How far the edge is nudged when measuring [`Threshold::count_swing`], as a
/// fraction of the population's interquartile width.
///
/// It lives here rather than in the confidence model because measuring it needs
/// the values, which the model never sees. A tenth of the spread is small
/// enough to be a plausible hand adjustment and wide enough to contain many
/// events in a dense region.
pub const STABILITY_WINDOW: f64 = 0.1;

/// The relative change in admitted count when the edge moves either way by
/// [`STABILITY_WINDOW`] of the interquartile spread.
///
/// Returns 0 when there is no spread to measure against or nothing admitted,
/// both of which the confidence model treats as no information rather than as
/// a clean placement.
fn count_swing_at(sorted_desc: &[f64], x: f64, spread: f64) -> f64 {
    if spread <= 0.0 {
        return 0.0;
    }
    let w = STABILITY_WINDOW * spread;
    let below = sorted_desc.partition_point(|v| *v > x - w);
    let above = sorted_desc.partition_point(|v| *v > x + w);
    let admitted = sorted_desc.partition_point(|v| *v > x);
    if admitted == 0 {
        return 0.0;
    }
    (below - above) as f64 / admitted as f64
}

/// The interquartile width of the population, as a scale to read other
/// distances against. Robust to the tail, which is exactly the part a rule is
/// usually placing an edge in.
pub fn interquartile_spread(sorted_desc: &[f64]) -> f64 {
    percentile_of_descending(sorted_desc, 75.0) - percentile_of_descending(sorted_desc, 25.0)
}

/// Position a gate edge a fixed visual distance above a percentile of the
/// parent population.
///
/// For the case where no obvious positive population exists and the negative
/// runs into a shoulder: take the top of the negative and step off it. On an
/// FMO the parent population *is* the negative, which is what makes this a
/// single-sample rule.
///
/// `offset` is in the axis's display units, because it stands for a visual
/// shift - the same offset means something quite different either side of an
/// arcsinh transform.
pub fn percentile_offset(
    values: &[f64],
    percentile: f64,
    offset: f64,
) -> Result<Threshold, SolveError> {
    if !(0.0..=100.0).contains(&percentile) {
        return Err(SolveError::BadPercentile(percentile));
    }
    let sorted = descending(values)?;
    let x = percentile_of_descending(&sorted, percentile) + offset;
    let spread = interquartile_spread(&sorted);
    let (events_admitted, fraction_admitted) = admitted(&sorted, x);

    Ok(Threshold {
        x,
        events_admitted,
        fraction_admitted,
        parent_events: sorted.len(),
        count_swing: count_swing_at(&sorted, x, spread),
        parent_spread: spread,
        status: Status::NoBand,
    })
}

/// The achievable count nearest `target`.
///
/// A count is only achievable if an edge can separate it from the rest, which
/// needs the two events either side of it to differ. Where they do not - a run
/// of identical values, or a target that lands inside a dense cloud - no
/// coordinate admits exactly that many, and placing the edge there anyway puts
/// it in a zero-width gap: hard against the data, and one stray cell away from
/// admitting something quite different.
///
/// Searching outward for the nearest count that *can* be separated is what puts
/// the edge in the empty space instead. It is not a compromise on aiming for the
/// middle of the band - the midpoint still chooses which gap - it is what makes
/// that aim reachable. Zero is always achievable, so this terminates.
fn nearest_achievable(sorted_desc: &[f64], target: usize) -> usize {
    let n = sorted_desc.len();
    let separable = |k: usize| k == 0 || k == n || sorted_desc[k - 1] > sorted_desc[k];

    if separable(target) {
        return target;
    }
    for step in 1..=n {
        if let Some(below) = target.checked_sub(step)
            && separable(below)
        {
            return below;
        }
        let above = target + step;
        if above <= n && separable(above) {
            return above;
        }
    }
    0
}

/// A percentile by linear interpolation between order statistics, reading a
/// descending slice. Kept separate so the interpolation can be tested on its
/// own - an off-by-one here moves every gate built on it.
pub fn percentile_of_descending(sorted_desc: &[f64], percentile: f64) -> f64 {
    let n = sorted_desc.len();
    if n == 1 {
        return sorted_desc[0];
    }
    // Rank from the bottom, so the 99th percentile is near the top.
    let rank = (percentile / 100.0) * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let ascending = |i: usize| sorted_desc[n - 1 - i];
    if lo == hi {
        return ascending(lo);
    }
    let weight = rank - lo as f64;
    ascending(lo) * (1.0 - weight) + ascending(hi) * weight
}

/// Where a population's negative sits, and how wide it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NegativePeak {
    /// The centre of the negative population.
    pub centre: f64,
    /// Its width, as a one-sigma equivalent.
    pub spread: f64,
}

/// Where one sigma falls *within the left flank*.
///
/// One standard deviation below a Gaussian's centre is its 15.87th percentile,
/// but only the events below the centre are being looked at - half the
/// distribution - so the same point sits at 0.1587/0.5 of that slice. Using the
/// whole-distribution figure against half the events reaches further down the
/// tail and reads the peak as half again as wide as it is.
const ONE_SIGMA_IN_LEFT_FLANK: f64 = 0.1587 / 0.5;

/// How wide the negative is, given where its centre sits.
///
/// Measured on the left flank and mirrored. The right flank runs into the
/// positives, so anything measured across the whole peak is inflated by however
/// many positives that sample happens to have - precisely the variation this
/// rule exists to see past, so measuring it into the answer would defeat the
/// point. The left flank is uncontaminated.
fn left_flank_sigma(values: &[f64], centre: f64) -> Option<f64> {
    let mut below: Vec<f64> = values.iter().copied().filter(|v| *v <= centre).collect();
    if below.len() < 2 {
        return None;
    }
    below.sort_by(f64::total_cmp);
    let at = ((below.len() as f64) * ONE_SIGMA_IN_LEFT_FLANK) as usize;
    let sigma = centre - below[at.min(below.len() - 1)];
    (sigma > 0.0).then_some(sigma)
}

fn median_of(sorted: &[f64]) -> f64 {
    sorted[sorted.len() / 2]
}

/// The negative, read from the events below a line that already roughly
/// separates it.
///
/// No peak finding and no density estimate: the line does the separating, so
/// the events below it are the negative and a plain median is its centre. That
/// removes both of the assumptions [`negative_peak`] depends on - a bandwidth,
/// and a bump being tall enough to count - and replaces them with one that is
/// true by construction: the gate starts roughly right, because it came from a
/// sample someone gated by hand.
///
/// `at` is how far the gate has been slid from where it sits now, so `0.0`
/// means the gate as it stands. A single pass is enough when the gate is
/// already where it belongs, which is the calibration case; [`refine_from`]
/// iterates for the case where it is not.
pub fn negative_below(shadow: &[(f64, f64)], at: f64) -> Option<NegativePeak> {
    // `shadow` pairs each event's value with its distance from the gate's
    // boundary at that event's own height. The gate translates rigidly, so a
    // gate slid by `at` has exactly the events with a smaller offset in its
    // shadow - true whether the boundary is a straight edge or a slanted one,
    // and silent about events the gate never reached.
    let mut below: Vec<f64> = shadow
        .iter()
        .filter(|(_, offset)| *offset <= at)
        .map(|(value, _)| *value)
        .collect();
    if below.len() < 2 {
        return None;
    }
    below.sort_by(f64::total_cmp);
    let centre = median_of(&below);
    Some(NegativePeak {
        centre,
        spread: left_flank_sigma(&below, centre)?,
    })
}

/// How many passes before the answer is taken as settled.
const REFINE_PASSES: usize = 12;
/// The furthest a single pass may move the line, in widths of the negative.
///
/// Without it, a line that started far too low sees only the bottom of the
/// negative, reads a centre that is too low, moves down, and walks off the
/// axis. The cap makes that failure stop rather than run away.
const MAX_STEP_IN_WIDTHS: f64 = 1.0;

/// Find the negative by improving on where the line already is.
///
/// Each pass sees more of the negative than the last, so each correction is
/// smaller than the one before and the line closes on its place rather than
/// swinging past it. It stops early when a pass stops moving it, and gives up
/// the moment a pass moves it further than the pass before - a line that is
/// getting worse rather than better is one this cannot rescue.
///
/// `place` turns a centre and a width into the line's next position.
pub fn refine_from(
    shadow: &[(f64, f64)],
    start: f64,
    place: impl Fn(NegativePeak) -> f64,
) -> Option<NegativePeak> {
    let mut at = start;
    let mut found = negative_below(shadow, at)?;
    let mut last_step = f64::INFINITY;

    for _ in 0..REFINE_PASSES {
        let wanted = place(found);
        let step = wanted - at;
        if step.abs() < f64::EPSILON {
            break;
        }
        if step.abs() > last_step {
            // Moving further than last time means it is diverging, not settling.
            break;
        }
        let capped = step.clamp(
            -MAX_STEP_IN_WIDTHS * found.spread,
            MAX_STEP_IN_WIDTHS * found.spread,
        );
        at += capped;
        last_step = step.abs();
        let Some(next) = negative_below(shadow, at) else {
            break;
        };
        found = next;
    }
    Some(found)
}

/// Find the negative population: its centre, and how wide it is.
///
/// Two things here are deliberate and neither is the obvious choice.
///
/// **The centre is the leftmost prominent mode, not the tallest.** On a marker
/// where the positives outnumber the negatives the tallest peak *is* the
/// positive one, and a gate placed off it would sit above the population it was
/// meant to separate.
///
/// **The width is measured on the left flank and mirrored.** The right flank
/// runs into the positives, so anything measured across the whole peak - a
/// standard deviation most of all - is inflated by however many positives that
/// sample happens to have. That is precisely the variation this rule exists to
/// see past, so measuring it into the answer would defeat the point. The left
/// flank is uncontaminated: the distance from the centre down to the 16th
/// percentile of the events below it is a one-sigma width that does not care
/// what the positives are doing.
pub fn negative_peak(values: &[f64]) -> Option<NegativePeak> {
    if values.len() < 2 {
        return None;
    }
    let bandwidth = crate::gate_move::kde::silverman_bandwidth(values);
    if !bandwidth.is_finite() || bandwidth <= 0.0 {
        return None;
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() || !hi.is_finite() || hi <= lo {
        return None;
    }

    let (xs, density) = crate::gate_move::kde::kde_1d(values, (lo, hi), 512, bandwidth);
    let centre = leftmost_prominent_mode(&xs, &density)?;

    Some(NegativePeak {
        centre,
        spread: left_flank_sigma(values, centre)?,
    })
}

/// The first mode worth calling a population.
///
/// A local maximum counts only if it rises to a real fraction of the tallest
/// one; without that, noise on the shoulder of the negative reads as a peak and
/// the answer lands wherever the grid happened to wobble.
fn leftmost_prominent_mode(xs: &[f64], density: &[f64]) -> Option<f64> {
    /// How tall a bump must be, against the tallest, to count.
    const PROMINENCE: f64 = 0.25;

    let tallest = density
        .iter()
        .copied()
        .filter(|d| d.is_finite())
        .fold(f64::NEG_INFINITY, f64::max);
    if !tallest.is_finite() || tallest <= 0.0 {
        return None;
    }
    let floor = tallest * PROMINENCE;

    for i in 1..density.len().saturating_sub(1).min(xs.len()) {
        if density[i] >= floor && density[i] >= density[i - 1] && density[i] > density[i + 1] {
            return Some(xs[i]);
        }
    }
    // No interior peak clears the bar - a single smooth rise, say. Fall back to
    // the tallest point rather than refusing outright.
    Some(crate::gate_move::kde::kde_peak(xs, density))
}
