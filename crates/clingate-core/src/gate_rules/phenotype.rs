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
//! cells?* A population is described marker by marker - where its cells sit on
//! each - and a cell in another sample is the same only if it is one of the
//! population on **every** marker: on the same side of the valley, if the
//! population is wholly positive or negative there, or within its range if it
//! is dim. Brightness may drift between samples; which markers are positive
//! and which negative may not. A CD8 T cell
//! that is CD4-positive is a different cell, however well the other markers
//! agree, so no marker can be outvoted by the rest.
//!
//! ## Each sample is its own reference frame
//!
//! Comparing a marker's raw value between two samples would need the two to be
//! on the same scale, which is run-to-run normalisation - explicitly not wanted
//! here, because it flattens the donor differences that are the entire point.
//!
//! So every marker is read against landmarks of *the parent population of the
//! same sample*: 0 at its negative's peak and 1 at the valley above it, where a
//! person would put a positive gate. "CD8 at 3, CD4 at 0" means the same thing
//! in every sample, whichever way the stain drifted, and does not move with how
//! many cells are positive - which is what differs between donors.
//!
//! A marker with no valley in one of the two samples - all negative, or a
//! smear - is read in robust z instead, on both: spreads from the parent's
//! middle. Median and MAD, with the tail cut off before measuring them; see
//! [`TRIM`]. That frame does move with abundance, measured on a population
//! sitting 14 parent-widths out:
//!
//! | population is | z of the population, plain MAD | tail excluded |
//! |---------------|-------------------------------|---------------|
//! | 1% of parent  | 13.68                         | 13.87         |
//! | 7%            | 12.53                         | 13.81         |
//! | 20%           |  9.82                         | 13.78         |
//!
//! Cutting the tail removes the dependence for a population that is a small
//! part of its parent; one that is half of it moves the parent's middle, which
//! is why landmarks come first.

use std::sync::Arc;

/// A population as a matrix: `markers` values per event, row-major.
///
/// A borrowed flat slice rather than a `Vec<Vec<_>>`. A panel of thirty
/// markers over forty thousand events is more than a million numbers, and a
/// vector per event would add its own header to each one - more memory in
/// bookkeeping than in data, scattered across the heap for a workload that
/// reads every value in order.
///
/// `f32` because that is what an FCS file holds and what polars hands back, so
/// there is nothing to convert and no precision to lose. The arithmetic below
/// accumulates in `f64`.
#[derive(Clone, Copy, Debug)]
pub struct Rows<'a> {
    values: &'a [f32],
    markers: usize,
}

impl<'a> Rows<'a> {
    /// `values` must hold a whole number of rows of `markers`.
    pub fn new(values: &'a [f32], markers: usize) -> Option<Self> {
        if markers == 0 || values.len() % markers != 0 {
            return None;
        }
        Some(Self { values, markers })
    }

    pub fn markers(&self) -> usize {
        self.markers
    }

    /// How many events.
    pub fn len(&self) -> usize {
        self.values.len() / self.markers
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn row(&self, at: usize) -> &'a [f32] {
        &self.values[at * self.markers..(at + 1) * self.markers]
    }

    pub fn rows(&self) -> impl Iterator<Item = &'a [f32]> + '_ {
        self.values.chunks_exact(self.markers)
    }

    /// One marker's values across every event.
    pub fn column(&self, marker: usize) -> impl Iterator<Item = f64> + '_ {
        self.values
            .iter()
            .skip(marker)
            .step_by(self.markers)
            .map(|v| *v as f64)
    }

    /// Only the rows at `keep`, copied out.
    ///
    /// Used where a subset has to outlive the borrow - the reference gate's
    /// members, which are chosen from the parent and then described.
    pub fn select(&self, keep: &[usize]) -> Vec<f32> {
        let mut out = Vec::with_capacity(keep.len() * self.markers);
        for at in keep {
            out.extend_from_slice(self.row(*at));
        }
        out
    }
}

/// The fraction of a population its ranges take in, every marker together.
///
/// Not all of it: the last few percent of any gated population are the events
/// nearest the line the person drew, and half of them are there because a hand
/// drawn boundary has to fall somewhere. Ranges holding 95% describe the
/// population rather than the edge of the drawing.
pub const KEEP: f64 = 0.95;

/// A population's middle and spread on one marker, from its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// How much of a population's distance from the origin of its frame is
/// uncertainty.
///
/// Landmarks and baselines are estimated from a finite sample, so the unit
/// they set carries a few percent of noise, and that noise multiplies with
/// distance: a population sitting 14 units out moves more than one unit when
/// the unit moves 10%. Each range is widened by this share of how far out its
/// ends sit, so the same cells are not rejected in the next sample for the
/// noise in where its frame was measured.
pub const BASELINE_SLIP: f64 = 0.10;

impl Baseline {
    /// Read one marker's middle and spread from a population, with the tail
    /// that a gated population would sit in excluded.
    ///
    /// Two passes: a plain robust estimate, then the same estimate over the
    /// values it says are within [`TRIM`] spreads. One pass is enough - the
    /// first estimate is already robust enough to say which values are far out,
    /// it is only its *scale* that the far-out ones distort.
    ///
    /// A value that is not a number - NaN or infinite, from a corrupt event or
    /// a transform gone wrong - is no measurement of the population and is
    /// left out, as the threshold solvers leave it out (B-PHEN-1).
    pub fn of(values: &[f64]) -> Self {
        let values: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        let rough = Self::untrimmed(&values);
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
        Self {
            median,
            spread: spread_about(values, median),
        }
    }

    /// Where one value sits, in spreads from the middle.
    pub fn z(&self, value: f64) -> f64 {
        (value - self.median) / self.spread
    }
}

/// The robust spread of `values` about `middle`.
fn spread_about(values: &[f64], middle: f64) -> f64 {
    let mut deviations: Vec<f64> = values.iter().map(|v| (v - middle).abs()).collect();
    (median_of_mut(&mut deviations) * MAD_TO_SIGMA).max(MIN_SPREAD)
}

fn median_of(values: &[f64]) -> f64 {
    let mut copy = values.to_vec();
    median_of_mut(&mut copy)
}

fn median_of_mut(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

/// How one marker is read on one sample: `(value - origin) / unit`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: f64,
    pub unit: f64,
    /// Whether the origin is the negative's peak and the unit the way to the
    /// valley above it; otherwise the parent's middle and spread.
    pub by_landmarks: bool,
}

impl Frame {
    pub fn read(&self, value: f64) -> f64 {
        (value - self.origin) / self.unit
    }

    pub fn value_at(&self, read: f64) -> f64 {
        self.origin + read * self.unit
    }
}

/// The two frames one marker can be read in on one sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frames {
    /// The negative's peak and the valley above it, where there is a valley.
    pub landmarks: Option<Frame>,
    pub spread: Frame,
}

impl Frames {
    /// One marker's frames, from its values across a parent population.
    pub fn of(values: &[f64]) -> Self {
        let values: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        let baseline = Baseline::of(&values);
        let landmarks = crate::gate_rules::threshold::first_valley(&values, 1.0)
            .ok()
            .filter(|valley| valley.bottom - valley.peak > MIN_SPREAD)
            .map(|valley| Frame {
                origin: valley.peak,
                unit: valley.bottom - valley.peak,
                by_landmarks: true,
            });
        Self {
            landmarks,
            spread: Frame {
                origin: baseline.median,
                unit: baseline.spread,
                by_landmarks: false,
            },
        }
    }

    /// The frame this and `other` share: landmarks when both have them, so a
    /// marker is read the same way on both samples.
    pub fn shared_with(&self, other: &Frames) -> (Frame, Frame) {
        match (self.landmarks, other.landmarks) {
            (Some(mine), Some(theirs)) => (mine, theirs),
            _ => (self.spread, other.spread),
        }
    }
}

/// Every marker's frames on one sample's parent population.
pub fn frames(parent: Rows<'_>) -> Vec<Frames> {
    (0..parent.markers())
        .map(|at| Frames::of(&parent.column(at).collect::<Vec<_>>()))
        .collect()
}

/// Where a population sits on one marker, in its own sample's raw values.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// The frames of the parent it was gated out of.
    pub frames: Frames,
    /// The range that holds its cells - see [`KEEP`].
    pub low: f64,
    pub high: f64,
    /// Its median and robust spread.
    pub middle: f64,
    pub spread: f64,
}

impl Profile {
    /// What the population is on this marker, read in `frame`: its range
    /// widened by [`BASELINE_SLIP`] of how far out each end sits, then as
    /// [`Identity::of`] says.
    fn identity(&self, frame: &Frame) -> Identity {
        let widened = |end: f64, outwards: f64| {
            let read = frame.read(end);
            read + outwards * BASELINE_SLIP * read.abs()
        };
        Identity::of(
            widened(self.low, -1.0),
            widened(self.high, 1.0),
            frame.by_landmarks,
        )
    }
}

/// What a cell must be on one marker to be one of a population.
///
/// Which side of the line between negative and positive a population is on
/// is what it is; how bright it is drifts from donor to donor. So a
/// population wholly on one side asks only that a cell be on that side too,
/// however bright or dim; one across the line - dim, or in the middle of a
/// parent with no valley - asks that it be within the population's range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Identity {
    /// Above this.
    Above(f64),
    /// Below this.
    Below(f64),
    /// Between these.
    Between(f64, f64),
}

impl Identity {
    /// From a population's range, `low` to `high`, read in a frame: by its
    /// landmarks, the line is the valley at 1, and a population wholly above
    /// or below it is positive or negative. Without them there is no such
    /// line, and only the far side is let go: a population above the
    /// parent's middle may be brighter, one below it dimmer.
    pub fn of(low: f64, high: f64, by_landmarks: bool) -> Self {
        let line = if by_landmarks { 1.0 } else { 0.0 };
        match (low > line, high < line, by_landmarks) {
            (true, _, true) => Identity::Above(line),
            (_, true, true) => Identity::Below(line),
            (true, _, false) => Identity::Above(low),
            (_, true, false) => Identity::Below(high),
            _ => Identity::Between(low, high),
        }
    }

    /// Whether `value` is one of the population on this marker.
    pub fn holds(&self, value: f64) -> bool {
        match *self {
            Identity::Above(line) => value > line,
            Identity::Below(line) => value < line,
            Identity::Between(low, high) => (low..=high).contains(&value),
        }
    }

    /// The same, read in `frame` and given in its values.
    fn in_values(&self, frame: &Frame) -> Self {
        match *self {
            Identity::Above(line) => Identity::Above(frame.value_at(line)),
            Identity::Below(line) => Identity::Below(frame.value_at(line)),
            Identity::Between(low, high) => {
                Identity::Between(frame.value_at(low), frame.value_at(high))
            }
        }
    }
}

/// A population marker by marker: what a cell must be to be one of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    /// The channels, in the order every vector here uses.
    pub markers: Vec<Arc<str>>,
    pub profiles: Vec<Profile>,
    /// How many events the signature was built from.
    pub members: usize,
}

impl Signature {
    /// Describe a population by where its members sit on each marker.
    ///
    /// `members` are the events inside the reference gate and `parent` is the
    /// population they were gated out of, both as raw marker values in the same
    /// column order.
    pub fn describe(markers: Vec<Arc<str>>, members: Rows<'_>, parent: Rows<'_>) -> Option<Self> {
        let n = markers.len();
        if n == 0 || n != members.markers() || n != parent.markers() || members.is_empty() {
            return None;
        }
        let columns: Vec<Vec<f64>> = (0..n)
            .map(|at| {
                let mut column: Vec<f64> = members.column(at).collect();
                column.sort_by(f64::total_cmp);
                column
            })
            .collect();
        let tail = tail_holding(&columns, members, KEEP);
        let profiles = frames(parent)
            .into_iter()
            .zip(&columns)
            .map(|(frames, column)| {
                let middle = median_of(column);
                Profile {
                    frames,
                    low: quantile_of_sorted(column, tail),
                    high: quantile_of_sorted(column, 1.0 - tail),
                    middle,
                    spread: spread_about(column, middle),
                }
            })
            .collect();
        Some(Self {
            markers,
            profiles,
            members: members.len(),
        })
    }

    /// Which of `parent`'s events are these cells: one of them on every
    /// marker, each read in the frame this sample shares with the reference.
    pub fn find_in(&self, parent: Rows<'_>) -> Matched {
        if self.markers.len() != parent.markers() {
            return Matched {
                members: Vec::new(),
                parent: parent.len(),
                reads: Vec::new(),
            };
        }
        let shared: Vec<(Frame, Frame)> = self
            .profiles
            .iter()
            .zip(frames(parent))
            .map(|(profile, here)| profile.frames.shared_with(&here))
            .collect();
        let identities: Vec<Identity> = self
            .profiles
            .iter()
            .zip(&shared)
            .map(|(profile, (there, _))| profile.identity(there))
            .collect();
        let accepted: Vec<Identity> = identities
            .iter()
            .zip(&shared)
            .map(|(identity, (_, here))| identity.in_values(here))
            .collect();
        let members: Vec<usize> = parent
            .rows()
            .enumerate()
            .filter(|(_, row)| {
                row.iter()
                    .zip(&accepted)
                    .all(|(value, identity)| identity.holds(f64::from(*value)))
            })
            .map(|(at, _)| at)
            .collect();
        let reads = self
            .markers
            .iter()
            .enumerate()
            .map(|(at, marker)| {
                let (there, here) = shared[at];
                let profile = &self.profiles[at];
                let values: Vec<f64> = members
                    .iter()
                    .map(|event| f64::from(parent.row(*event)[at]))
                    .collect();
                MarkerRead {
                    marker: marker.clone(),
                    by_landmarks: there.by_landmarks,
                    identity: identities[at],
                    reference_middle: there.read(profile.middle),
                    reference_spread: profile.spread / there.unit,
                    middle: if values.is_empty() {
                        f64::NAN
                    } else {
                        here.read(median_of(&values))
                    },
                }
            })
            .collect();
        Matched {
            members,
            parent: parent.len(),
            reads,
        }
    }
}

/// The share of each marker's two tails left out so that `keep` of the
/// members are within range on every marker at once.
///
/// Measured on the members rather than assumed: markers that move together
/// leave out the same cells, and independent ones leave out different cells,
/// and only the members say which.
fn tail_holding(columns: &[Vec<f64>], members: Rows<'_>, keep: f64) -> f64 {
    let held = |tail: f64| {
        let ranges: Vec<(f64, f64)> = columns
            .iter()
            .map(|c| {
                (
                    quantile_of_sorted(c, tail),
                    quantile_of_sorted(c, 1.0 - tail),
                )
            })
            .collect();
        let inside = members
            .rows()
            .filter(|row| {
                row.iter()
                    .zip(&ranges)
                    .all(|(v, (low, high))| (*low..=*high).contains(&f64::from(*v)))
            })
            .count();
        inside as f64 / members.len() as f64
    };
    let (mut wide, mut narrow) = (0.0, (1.0 - keep) / 2.0);
    if held(narrow) >= keep {
        return narrow;
    }
    for _ in 0..30 {
        let mid = (wide + narrow) / 2.0;
        if held(mid) >= keep {
            wide = mid;
        } else {
            narrow = mid;
        }
    }
    wide
}

/// The value `q` of the way up sorted `values`, by nearest rank.
fn quantile_of_sorted(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values[((values.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize]
}

/// Where the matched cells sit on one marker against the reference's, both
/// in the frame the two samples share.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkerRead {
    pub marker: Arc<str>,
    pub by_landmarks: bool,
    /// What a cell had to be on this marker to be matched.
    pub identity: Identity,
    pub reference_middle: f64,
    pub reference_spread: f64,
    /// NaN where nothing matched.
    pub middle: f64,
}

impl MarkerRead {
    /// Whether the matched cells' middle has left the reference population:
    /// on the wrong side of the line for a positive or negative population;
    /// for one across it, further from its middle than its own spread,
    /// allowing for the noise in the frames.
    pub fn drifted(&self) -> bool {
        match self.identity {
            Identity::Between(..) => {
                let allowed = self.reference_spread + BASELINE_SLIP * self.reference_middle.abs();
                !((self.middle - self.reference_middle).abs() <= allowed)
            }
            identity => !identity.holds(self.middle),
        }
    }
}

/// The fewest matched cells a gate is moved onto.
pub const FEWEST_MATCHED: usize = 50;

/// The least share of its parent the matched cells may be, against the
/// reference population's share of its own: a population a fifth as common
/// may be real; rarer, the cells that match are mostly near misses from the
/// populations round it.
pub const LEAST_SHARE: f64 = 0.2;

/// The share of the matched cells the largest cloud they form on the plot
/// must hold for them to be one population.
pub const ONE_CLOUD: f64 = 0.8;

/// The events of one sample that match a signature.
pub struct Matched {
    /// Indices into the parent population.
    pub members: Vec<usize>,
    pub parent: usize,
    /// Per marker, where the matched cells sit against the reference.
    pub reads: Vec<MarkerRead>,
}

impl Matched {
    /// What fraction of the parent matched.
    pub fn fraction(&self) -> f64 {
        if self.parent == 0 {
            return 0.0;
        }
        self.members.len() as f64 / self.parent as f64
    }

    /// Why these cells are too weak a match to move a gate onto, if they are,
    /// against `reference_share`, the reference population's share of its
    /// parent.
    pub fn weak(&self, reference_share: f64) -> Option<String> {
        if self.members.len() < FEWEST_MATCHED {
            return Some(format!(
                "only {} cells match the reference population; {FEWEST_MATCHED} are needed",
                self.members.len()
            ));
        }
        if self.fraction() < LEAST_SHARE * reference_share {
            return Some(format!(
                "{:.3}% of the parent matched against {:.3}% on the reference - under a fifth \
                 as common, so most of the cells matched are likely near misses",
                self.fraction() * 100.0,
                reference_share * 100.0
            ));
        }
        self.reads.iter().find(|read| read.drifted()).map(|read| {
            format!(
                "on {} the matched cells sit at {:.2} against {:.2} on the reference ({}) - \
                 not the same cells",
                read.marker,
                read.middle,
                read.reference_middle,
                if read.by_landmarks {
                    "0 at the negative's peak, 1 at the valley above it"
                } else {
                    "in spreads from the parent's middle"
                }
            )
        })
    }
}
