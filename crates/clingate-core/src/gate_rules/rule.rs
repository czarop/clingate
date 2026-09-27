//! The rules themselves: the ways a gate edge's position can be decided.
//!
//! Two things are deliberately kept out of a rule.
//!
//! *Which sample to measure.* A rule answers "given these values, where does
//! the line go". The layer above decides which values - the FMO or the full
//! stain, and which parent chain. That is what lets every rule be exercised
//! against a plain slice of f64, with no store, no polars and no metadata.
//!
//! *Which edge to move.* A rule returns a coordinate on one axis. The layer
//! above knows whether that is a rectangle's left edge, a bisector's arm or a
//! quadrant's centre line. New gate shapes therefore cost new application code,
//! not new rules.
//!
//! [`Rule`] is the stored form. It is a closed set on purpose: it serialises to
//! the sidecar with a plain derive, it drives the Gate Rules tab exhaustively,
//! and adding a variant makes the compiler name every place that has to handle
//! it. The trait is the contract each variant implements; the enum is one match
//! in one file.

use crate::gate_rules::confidence::{Confidence, ConfidenceModel, CountAndSeparation};
use crate::gate_rules::threshold::{SolveError, Threshold, percentile_offset, tail_fraction};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A position and the confidence in it, which is what every caller wants.
#[derive(Debug, Clone, PartialEq)]
pub struct Solved {
    pub threshold: Threshold,
    pub confidence: Confidence,
}

/// One way of deciding where a gate edge belongs.
pub trait PositioningRule {
    /// The confidence model this rule is judged by - its own, with its own
    /// parameters, so that models can diverge as rules do.
    type Confidence: ConfidenceModel;

    /// Where the edge goes, given the parent population on the rule's axis, in
    /// the axis's display space.
    fn solve(&self, values: &[f64]) -> Result<Threshold, SolveError>;

    fn confidence_model(&self) -> &Self::Confidence;

    /// One line for the report and the Gate Rules tab.
    fn describe(&self) -> String;

    /// Solve and score in one step. `reference_x` is where the same gate sits on
    /// the QC or template sample, when there is one.
    fn apply(&self, values: &[f64], reference_x: Option<f64>) -> Result<Solved, SolveError> {
        let threshold = self.solve(values)?;
        let confidence = self.confidence_model().assess(&threshold, reference_x);
        Ok(Solved {
            threshold,
            confidence,
        })
    }
}

/// "The FMO gate should contain 0.2% to 0.5% of events."
///
/// The band is a range of acceptable fractions rather than a target, so the
/// solver is free to choose within it - see [`tail_fraction`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TailFractionRule {
    /// Acceptable fractions of the parent population, as fractions not
    /// percentages: 0.2% to 0.5% is `(0.002, 0.005)`.
    pub band: (f64, f64),
    #[serde(default)]
    pub confidence: CountAndSeparation,
}

impl TailFractionRule {
    pub fn new(band: (f64, f64)) -> Self {
        Self {
            band,
            confidence: CountAndSeparation::default(),
        }
    }
}

impl PositioningRule for TailFractionRule {
    type Confidence = CountAndSeparation;

    fn solve(&self, values: &[f64]) -> Result<Threshold, SolveError> {
        tail_fraction(values, self.band)
    }

    fn confidence_model(&self) -> &Self::Confidence {
        &self.confidence
    }

    fn describe(&self) -> String {
        format!(
            "capture {:.3}% to {:.3}% of the parent population",
            self.band.0 * 100.0,
            self.band.1 * 100.0
        )
    }
}

/// "A fixed step above the 99th percentile of the negative."
///
/// For the shoulder case, where no distinct positive population exists to aim
/// at. The offset is in the axis's display units because it stands for a visual
/// shift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PercentileOffsetRule {
    pub percentile: f64,
    pub offset: f64,
    #[serde(default)]
    pub confidence: CountAndSeparation,
}

impl PercentileOffsetRule {
    pub fn new(percentile: f64, offset: f64) -> Self {
        Self {
            percentile,
            offset,
            confidence: CountAndSeparation::default(),
        }
    }
}

impl PositioningRule for PercentileOffsetRule {
    type Confidence = CountAndSeparation;

    fn solve(&self, values: &[f64]) -> Result<Threshold, SolveError> {
        percentile_offset(values, self.percentile, self.offset)
    }

    fn confidence_model(&self) -> &Self::Confidence {
        &self.confidence
    }

    fn describe(&self) -> String {
        format!(
            "sit {} the {}th percentile of the negative by {}",
            if self.offset < 0.0 { "below" } else { "above" },
            self.percentile,
            self.offset.abs()
        )
    }
}

/// The stored form of a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Rule {
    TailFraction(TailFractionRule),
    PercentileOffset(PercentileOffsetRule),
    AboveTheNegative(AboveTheNegativeRule),
    InTheValley(ValleyRule),
    MatchThePhenotype(PhenotypeRule),
}

/// "Where I put it on the QC, relative to that sample's negative."
///
/// The other rules are complete specifications - "0.2% to 0.5% of the parent"
/// can be handed to any sample and it knows what to do. This one is not. "A bit
/// above the negative" does not say how much, and the only place that number
/// exists is in where the gate was placed on the reference sample. So it is
/// calibrated before it is applied: read how far above that sample's negative
/// the gate sits, in units of that negative's own width, then put the gate the
/// same number of widths above every other sample's negative.
///
/// Because the distance is measured in widths rather than units, a sample whose
/// negative has drifted or broadened - which is what difficult unmixing does
/// from run to run - carries the gate with it.
///
/// Nothing here needs an FMO: the negative is read from the sample being gated.
/// How the negative population is found.
///
/// The two differ only here - the calibration, the scale and the nudge are
/// identical - so a difference in where they put a gate is a difference between
/// the estimators and nothing else.
/// Which to pick is decided by the shape of the plot, and the two are not
/// interchangeable - measured on synthetic populations where only the amount of
/// smear changes:
///
/// | smear | `NegativePeak` centre | `BelowTheGate` centre |
/// |-------|----------------------|----------------------|
/// | 4%    | +0.060               | +0.017               |
/// | 16%   | +0.093               | +0.082               |
/// | 35%   | +0.118               | +0.182               |
///
/// Dim cells that are not detectably positive are counted as negative by both,
/// and the more of them a sample has the further right its negative reads. That
/// matters because the bias scales with the positive fraction, so it differs
/// between the reference and the sample and corrupts the transfer rather than
/// just the measurement. Below-the-gate drifts three times as far, because
/// cutting at the gate swallows the whole smear; the peak finder only sees the
/// smear as a shoulder pushing on its mode.
///
/// The reverse holds where the populations are separate: with a real valley to
/// cut at, below-the-gate was about five times the sharper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NegativeFinder {
    /// From the shape of the density: the leftmost bump tall enough to count.
    ///
    /// Needs a bandwidth and a prominence threshold, neither of which comes
    /// from the data. For a smear, where there is no valley to cut at and the
    /// dim cells would otherwise be counted as negative wholesale.
    #[serde(alias = "DensityPeak")]
    NegativePeak,
    /// From the events below the line, improving on where the line already is.
    ///
    /// No bandwidth and no threshold. Assumes instead that the gate starts
    /// roughly right, which it does: it came from a sample gated by hand. For
    /// separated populations, where the gate sits in the valley and everything
    /// below it really is the negative.
    #[default]
    #[serde(alias = "RefineFromGate")]
    BelowTheGate,
}

impl NegativeFinder {
    /// The mechanism, for prose that already has the context.
    pub fn label(self) -> &'static str {
        match self {
            NegativeFinder::NegativePeak => "the negative's own peak",
            NegativeFinder::BelowTheGate => "the events below the gate",
        }
    }

    /// The mechanism *and* when to reach for it, for a menu being chosen from
    /// cold.
    pub fn choice(self) -> &'static str {
        match self {
            NegativeFinder::NegativePeak => "the negative's own peak - when the positives smear",
            NegativeFinder::BelowTheGate => {
                "the events below the gate - when the peaks are separate"
            }
        }
    }

    pub const ALL: [NegativeFinder; 2] =
        [NegativeFinder::BelowTheGate, NegativeFinder::NegativePeak];

    /// The serialised name, which is also what the menu round-trips on.
    pub fn key(self) -> &'static str {
        match self {
            NegativeFinder::NegativePeak => "NegativePeak",
            NegativeFinder::BelowTheGate => "BelowTheGate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AboveTheNegativeRule {
    /// Multiplies the calibrated distance. 1.0 places the gate exactly where
    /// the reference says; 1.1 sits a tenth further out.
    #[serde(default = "one")]
    pub scale: f64,
    /// Added afterwards, in the axis's own units, for a nudge that has nothing
    /// to do with how wide the negative is.
    #[serde(default)]
    pub nudge: f64,
    /// How the negative is found.
    #[serde(default)]
    pub find: NegativeFinder,
    #[serde(default)]
    pub confidence: CountAndSeparation,
}

fn one() -> f64 {
    1.0
}

impl Default for AboveTheNegativeRule {
    fn default() -> Self {
        Self {
            scale: 1.0,
            nudge: 0.0,
            find: NegativeFinder::default(),
            confidence: CountAndSeparation::default(),
        }
    }
}

/// What a finder read off one sample, and the gate position that follows.
///
/// Returned rather than the bare number both `calibrate` and `place` used to
/// give back, so a report can show the reading the placement was actually made
/// from. Computing it a second time for display would let the two drift, and
/// then the figure on screen would no longer explain the gate on the plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NegativeRead {
    /// The centre of this sample's negative, in the axis's display space.
    pub centre: f64,
    /// Its width, as a one-sigma equivalent measured on the left flank.
    pub spread: f64,
    /// How many events that width was measured from.
    pub flank_events: usize,
    /// How many widths above `centre` the gate sits - read from the reference
    /// when calibrating, applied as given when placing.
    pub widths: f64,
    /// Where that puts the gate on the axis. For a calibration this is the
    /// position a person drew, by definition.
    pub at: f64,
}

impl AboveTheNegativeRule {
    /// How far above the reference's negative its gate sits, in widths.
    ///
    /// `None` when the reference has no readable negative, which is a refusal
    /// rather than a zero: a gate placed off an unreadable peak is worse than
    /// one left alone.
    /// `values` is the whole parent population; `shadow` pairs each event with
    /// its distance from the gate's boundary at that event's own height.
    pub fn calibrate(
        &self,
        values: &[f64],
        shadow: &[(f64, f64)],
        reference_x: f64,
    ) -> Option<NegativeRead> {
        use crate::gate_rules::threshold as t;
        // Nothing to iterate here: the gate is already where a person put it,
        // so one look at its shadow is the whole answer - offset 0.0.
        let peak = match self.find {
            NegativeFinder::NegativePeak => t::negative_peak(values)?,
            NegativeFinder::BelowTheGate => t::negative_below(shadow, 0.0)?,
        };
        Some(NegativeRead {
            centre: peak.centre,
            spread: peak.spread,
            flank_events: peak.flank_events,
            widths: (reference_x - peak.centre) / peak.spread,
            at: reference_x,
        })
    }

    /// Where the gate belongs on a sample, given that calibration.
    ///
    /// `start` is where the gate's leading extent sits now, used to turn a slide
    /// distance back into a position on the axis. The density finder ignores
    /// the gate entirely; the refining one improves on it.
    pub fn place(
        &self,
        values: &[f64],
        shadow: &[(f64, f64)],
        widths: f64,
        start: f64,
    ) -> Option<NegativeRead> {
        use crate::gate_rules::threshold as t;
        let at =
            |peak: t::NegativePeak| peak.centre + widths * self.scale * peak.spread + self.nudge;
        let peak = match self.find {
            NegativeFinder::NegativePeak => t::negative_peak(values)?,
            // The refining finder works in slide distances, so it searches from
            // the gate where it stands - offset zero - rather than from a value
            // on the axis. `at` then reads back onto the axis as usual.
            NegativeFinder::BelowTheGate => t::refine_from(shadow, 0.0, |peak| at(peak) - start)?,
        };
        Some(NegativeRead {
            centre: peak.centre,
            spread: peak.spread,
            flank_events: peak.flank_events,
            widths,
            at: at(peak),
        })
    }

    pub fn describe(&self) -> String {
        let mut how = format!(
            "as far above the negative as the reference sits, found from {}",
            self.find.label()
        );
        if self.scale != 1.0 {
            how.push_str(&format!(", times {:.2}", self.scale));
        }
        if self.nudge != 0.0 {
            how.push_str(&format!(", {:+.3} on the axis", self.nudge));
        }
        how
    }
}

/// "In the dip between the negative and the positive, where I put it on the QC."
///
/// The sibling of [`AboveTheNegativeRule`], and the difference is what each one
/// measures. That one reads the negative's centre and width and paces out a
/// fixed number of widths; this one reads the boundary itself.
///
/// Nothing is extrapolated here, so nothing is amplified - which is the failure
/// that motivated it. On a marker whose two populations had merged in one
/// sample, the negative measured 2.47 times wider than the reference's, and
/// because the gate sits a fixed number of widths out, that carried it six
/// widths past where it belonged and off the end of the data.
///
/// It needs two populations. Where the positives are a smear with no peak of
/// their own there is no dip to find and this rule has nothing to say;
/// `AboveTheNegative` is for those. The two are not competitors, they are for
/// different shapes of plot.
///
/// A shallow dip is placed, not refused, and scored on how deep it is against
/// the reference's dip (`confidence::VALLEY`), so it rises to the top for
/// review. Refusing hid the answer exactly when a person most wanted to see it
/// - a run came back with seven samples unplaced at depths of 5% to 24%
/// against a threshold of 25%, one of them short by a single point, and the
/// only way to find out where the gate would have gone was to change the
/// setting and run again. Only a density with no dip at all is refused,
/// because then there is nothing to place.
///
/// There used to be a `min_depth_fraction` here, the depth below which a
/// placement was refused. When refusing gave way to scoring it was left in the
/// form and the rules file, read by nothing (B-RULE-1), and it was removed. A
/// rules file that still has it loads as before; the value is ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValleyRule {
    /// Scales the density's bandwidth. Below 1 finds shallower dips and more
    /// noise; above 1 smooths shallow ones away. Exposed because which of those
    /// is wanted depends on the marker, and no automatic rule knows that.
    #[serde(default = "one")]
    pub smoothing: f64,
    #[serde(default)]
    pub confidence: CountAndSeparation,
}

impl Default for ValleyRule {
    fn default() -> Self {
        Self {
            smoothing: 1.0,
            confidence: CountAndSeparation::default(),
        }
    }
}

/// What the valley rule read off one sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValleyRead {
    /// The negative's own peak.
    pub peak: f64,
    /// The lowest point of the dip beyond it.
    pub bottom: f64,
    /// How deep that dip is, as a fraction of the lower peak either side.
    pub depth: f64,
    /// How far the gate sits from the bottom. Read from the reference and
    /// applied as given, so a person who habitually gates a little to one side
    /// of the true bottom has that reproduced rather than corrected.
    pub offset: f64,
    /// Where that puts the gate.
    pub at: f64,
}

impl ValleyRule {
    /// Read the reference's valley, and how far its gate sits from the bottom.
    pub fn calibrate(
        &self,
        values: &[f64],
        reference_x: f64,
    ) -> Result<ValleyRead, crate::gate_rules::threshold::NoValley> {
        let found = crate::gate_rules::threshold::first_valley(values, self.smoothing)?;
        Ok(ValleyRead {
            peak: found.peak,
            bottom: found.bottom,
            depth: found.depth,
            offset: reference_x - found.bottom,
            at: reference_x,
        })
    }

    /// Find this sample's valley and put the gate the same distance from it.
    pub fn place(
        &self,
        values: &[f64],
        offset: f64,
    ) -> Result<ValleyRead, crate::gate_rules::threshold::NoValley> {
        let found = crate::gate_rules::threshold::first_valley(values, self.smoothing)?;
        Ok(ValleyRead {
            peak: found.peak,
            bottom: found.bottom,
            depth: found.depth,
            offset,
            at: found.bottom + offset,
        })
    }

    pub fn describe(&self) -> String {
        let mut how =
            "in the dip between the negative and the positive, as on the reference".to_string();
        if self.smoothing != 1.0 {
            how.push_str(&format!(", smoothed x{:.2}", self.smoothing));
        }
        how
    }
}

impl Rule {
    /// Solve and score, dispatching to the variant's own implementation.
    pub fn apply(&self, values: &[f64], reference_x: Option<f64>) -> Result<Solved, SolveError> {
        match self {
            Rule::TailFraction(r) => r.apply(values, reference_x),
            Rule::PercentileOffset(r) => r.apply(values, reference_x),
            // Calibrated against another sample, so it cannot be solved from
            // one population alone - see `AboveTheNegativeRule`. The phenotype
            // rule is not a threshold at all: it does not move an edge along
            // one parameter, it replaces a geometry, so there is nothing here
            // for it to return.
            Rule::AboveTheNegative(_) | Rule::InTheValley(_) | Rule::MatchThePhenotype(_) => {
                Err(SolveError::BadBand { band: (0.0, 0.0) })
            }
        }
    }

    /// Score a result this rule's own way, for a threshold arrived at by some
    /// other means than [`Rule::solve`] - sliding the gate until it captures
    /// the right fraction, say.
    ///
    /// `None` for a rule that never produces a threshold. That is the
    /// phenotype rule, which is judged on what it matched rather than on where
    /// a line went - see `confidence::assess_match`. Returning an `Option`
    /// rather than some stand-in score is deliberate: a number on the same
    /// scale as the others, arrived at from different evidence, would be
    /// compared with them.
    pub fn assess(
        &self,
        threshold: &Threshold,
        reference_x: Option<f64>,
    ) -> Option<crate::gate_rules::confidence::Confidence> {
        Some(match self {
            Rule::TailFraction(r) => r.confidence_model().assess(threshold, reference_x),
            Rule::PercentileOffset(r) => r.confidence_model().assess(threshold, reference_x),
            Rule::AboveTheNegative(r) => r.confidence.assess(threshold, reference_x),
            Rule::InTheValley(r) => r.confidence.assess(threshold, reference_x),
            Rule::MatchThePhenotype(_) => return None,
        })
    }

    pub fn solve(&self, values: &[f64]) -> Result<Threshold, SolveError> {
        match self {
            Rule::TailFraction(r) => r.solve(values),
            Rule::PercentileOffset(r) => r.solve(values),
            Rule::AboveTheNegative(_) | Rule::InTheValley(_) | Rule::MatchThePhenotype(_) => {
                Err(SolveError::BadBand { band: (0.0, 0.0) })
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Rule::TailFraction(r) => r.describe(),
            Rule::PercentileOffset(r) => r.describe(),
            Rule::AboveTheNegative(r) => r.describe(),
            Rule::InTheValley(r) => r.describe(),
            Rule::MatchThePhenotype(r) => r.describe(),
        }
    }

    /// The fractions of the parent this rule will accept, when it is the kind
    /// of rule that accepts a range at all.
    ///
    /// What it is for: a gate already capturing a fraction inside the band
    /// satisfies the rule, and the best thing to do with it is nothing. Solving
    /// anyway is work for no gain, and can actively make things worse - a band
    /// narrow enough to allow only one or two whole events can be missed by the
    /// solver even where the current position hits it.
    ///
    /// `None` for a rule that names a position rather than a range: there is no
    /// "already correct" to test against, so it is always re-solved.
    pub fn accepted_band(&self) -> Option<(f64, f64)> {
        match self {
            Rule::TailFraction(r) => Some(r.band),
            // A position, not a range. The phenotype rule has no band either:
            // a gate is right when it holds the cells that match, and whether
            // it does cannot be read off a fraction of the parent.
            Rule::PercentileOffset(_)
            | Rule::AboveTheNegative(_)
            | Rule::InTheValley(_)
            | Rule::MatchThePhenotype(_) => None,
        }
    }

    /// The name of the kind, for the Gate Rules tab's list.
    pub fn kind(&self) -> &'static str {
        match self {
            Rule::TailFraction(_) => "Tail fraction",
            Rule::PercentileOffset(_) => "Percentile offset",
            Rule::AboveTheNegative(_) => "Above the negative",
            Rule::InTheValley(_) => "In the valley",
            Rule::MatchThePhenotype(_) => "Match the phenotype",
        }
    }
}

// ── matching a population by what it is ──────────────────────────────────

/// What to do with the outline once the population has been found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeFit {
    /// Move and resize the gate as drawn, without changing its shape.
    ///
    /// For an outline that carries meaning the data does not: a rectangle that
    /// stands for a quadrant, a shape agreed with a collaborator, a gate that
    /// has to stay comparable with how it was drawn before. The population
    /// decides where it sits and how big it is; what it looks like is kept.
    ///
    /// Also the only option that preserves the *kind* of gate - a rectangle
    /// stays a rectangle, an ellipse an ellipse.
    #[default]
    KeepShape,
    /// Draw a fresh polygon round the matched cells on every sample.
    ///
    /// For a population whose shape genuinely differs between donors, where
    /// the reference's outline is a record of one sample rather than a
    /// statement about the population. Turns the gate into a polygon whatever
    /// it started as, because no other geometry can express a traced contour.
    DrawPolygon,
}

impl ShapeFit {
    pub fn label(self) -> &'static str {
        match self {
            ShapeFit::KeepShape => "keep the shape, move and resize it",
            ShapeFit::DrawPolygon => "draw a new polygon round the cells",
        }
    }

    pub fn choice(self) -> &'static str {
        match self {
            ShapeFit::KeepShape => {
                "keep the shape - move and resize it, and stay the kind of gate it is"
            }
            ShapeFit::DrawPolygon => {
                "draw a new polygon - follow the cells, whatever shape they make"
            }
        }
    }

    pub const ALL: [ShapeFit; 2] = [ShapeFit::KeepShape, ShapeFit::DrawPolygon];

    /// The serialised name, which is also what the menu round-trips on.
    pub fn key(self) -> &'static str {
        match self {
            ShapeFit::KeepShape => "KeepShape",
            ShapeFit::DrawPolygon => "DrawPolygon",
        }
    }
}

/// "The cells that look like the ones I gated on the QC, wherever they are."
///
/// The rule for populations the others cannot reach. Every other rule reads
/// one parameter and moves one edge, which works where a population separates
/// along a single axis and has nothing to say where it does not - a smear with
/// no dip, several clusters near each other, a population that moves in both
/// axes at once.
///
/// This one identifies the population instead. The cells inside the reference
/// gate are described by where they sit across the chosen markers - their
/// phenotype - and that description is used to find the same cells in each
/// sample. Where they turn out to be on the plot is then an answer rather than
/// an assumption, and the gate is fitted to them.
///
/// Nothing is normalised between samples: each one's markers are read against
/// its own parent population, so donor differences are carried rather than
/// flattened. See [`phenotype`](super::phenotype).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhenotypeRule {
    /// The markers that say what this population *is*.
    ///
    /// Chosen per rule, because which markers define a population is knowledge
    /// about the biology that nothing in the data supplies. MAIT cells are
    /// TCR Va7.2 and CD161; a monocyte marker is not wrong about them, it is
    /// silent, and including it spends the distance budget on noise. Empty
    /// means every marker on the panel, which is a reasonable place to start
    /// and rarely where you finish.
    #[serde(default)]
    pub markers: Vec<Arc<str>>,
    /// Whether the drawn outline is kept or replaced.
    #[serde(default)]
    pub fit: ShapeFit,
    /// The fraction of the matched cells the gate should hold.
    ///
    /// Not all of them: the last few percent of any population are the ones
    /// the signature is least sure about, and a boundary drawn to include them
    /// is drawn around the doubt.
    #[serde(default = "ninety_five")]
    pub keep: f64,
    /// Scales the bandwidth of the density the outline is traced on. Below 1
    /// follows the cells more closely and picks up their noise; above 1 gives
    /// a smoother boundary. Ignored when the shape is kept.
    #[serde(default = "one")]
    pub smoothing: f64,
    /// About how many points the drawn polygon may have. Ignored when the
    /// shape is kept.
    #[serde(default = "two_dozen")]
    pub vertices: usize,
}

fn ninety_five() -> f64 {
    0.95
}

fn two_dozen() -> usize {
    24
}

impl PhenotypeRule {
    /// What this rule does, for the rules table.
    pub fn describe(&self) -> String {
        let markers = if self.markers.is_empty() {
            "every marker".to_string()
        } else {
            self.markers
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "find the cells that match on {markers}, then {}",
            self.fit.label()
        )
    }
}

impl Default for PhenotypeRule {
    fn default() -> Self {
        Self {
            markers: Vec::new(),
            fit: ShapeFit::default(),
            keep: ninety_five(),
            smoothing: one(),
            vertices: two_dozen(),
        }
    }
}
