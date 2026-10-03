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
use crate::gate_rules::rule_store::Bound;
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
/// solver is free to choose within it - see [`tail_fraction`] - and where in
/// it the gate lands is [`BandAim`]'s to say.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TailFractionRule {
    /// Acceptable fractions of the parent population, as fractions not
    /// percentages: 0.2% to 0.5% is `(0.002, 0.005)`.
    pub band: (f64, f64),
    /// Where in the band the gate is put.
    #[serde(default)]
    pub aim: BandAim,
    /// Which files the band is counted on: each specimen's own, or every one
    /// of that kind in the run - the workspace - together.
    #[serde(default)]
    pub pool: Pool,
    #[serde(default)]
    pub confidence: CountAndSeparation,
}

/// Which files a band rule counts its band on.
///
/// A band of 0.2-0.5% of an FMX of 700 events is one to three events: each
/// specimen's line is then set by where a couple of stray events happen to
/// fall. A gating guide that says "per run in the first instance" means one
/// line for the run - every file analysed together, which is the workspace -
/// set on all of its controls together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Pool {
    /// Each specimen's own file - its own FMX - and a line for each specimen.
    #[default]
    Specimen,
    /// Every file of that kind in the run - the workspace - together, and one
    /// line for every specimen.
    Run,
}

impl Pool {
    pub const ALL: [Pool; 2] = [Pool::Specimen, Pool::Run];

    pub fn key(self) -> &'static str {
        match self {
            Pool::Specimen => "Specimen",
            Pool::Run => "Run",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn choice(self) -> &'static str {
        match self {
            Pool::Specimen => "each specimen's own file - a line per specimen",
            Pool::Run => "every file of that kind in the run together - one line for all",
        }
    }
}

/// Where in its band a band rule puts the gate.
///
/// The search slides the gate and halves its range each step. Stopping at the
/// first position inside the band lands anywhere in it - where depends on the
/// ends of the search range, which are the population's most extreme events -
/// so two alike samples can land at opposite edges. Carrying on to the middle
/// lands every sample at the same fraction, as near as its events allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BandAim {
    /// Stop at the first position inside the band, and leave a gate that is
    /// already inside it where it is.
    #[default]
    AnywhereInBand,
    /// Carry on to the position nearest the band's middle, and leave a gate
    /// where it is only if it already holds within a tenth of the band's
    /// width of the middle.
    Middle,
}

impl BandAim {
    pub const ALL: [BandAim; 2] = [BandAim::AnywhereInBand, BandAim::Middle];

    /// The serialised name, which is also what the menu round-trips on.
    pub fn key(self) -> &'static str {
        match self {
            BandAim::AnywhereInBand => "AnywhereInBand",
            BandAim::Middle => "Middle",
        }
    }

    /// For a menu being chosen from cold.
    pub fn choice(self) -> &'static str {
        match self {
            BandAim::AnywhereInBand => {
                "anywhere in the band - stop at the first position inside it"
            }
            BandAim::Middle => "the middle of the band - the same fraction on every sample",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }
}

impl TailFractionRule {
    pub fn new(band: (f64, f64)) -> Self {
        Self {
            band,
            aim: BandAim::default(),
            pool: Pool::default(),
            confidence: CountAndSeparation::default(),
        }
    }

    /// The same band, aimed at its middle.
    pub fn aimed(band: (f64, f64), aim: BandAim) -> Self {
        Self {
            aim,
            ..Self::new(band)
        }
    }

    /// The fractions a gate may already hold and be left where it is.
    pub fn kept_band(&self) -> (f64, f64) {
        match self.aim {
            BandAim::AnywhereInBand => self.band,
            BandAim::Middle => {
                let middle = (self.band.0 + self.band.1) / 2.0;
                let slack = (self.band.1 - self.band.0) / 10.0;
                (middle - slack, middle + slack)
            }
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
        let mut said = format!(
            "capture {:.3}% to {:.3}% of the parent population",
            self.band.0 * 100.0,
            self.band.1 * 100.0
        );
        if self.aim == BandAim::Middle {
            said.push_str(", aiming for the middle");
        }
        if self.pool == Pool::Run {
            said.push_str(", counted on all of the run's files of that kind together - one line for every specimen");
        }
        said
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
    ValleyOrSmear(ValleyOrSmearRule),
    MatchThePhenotype(PhenotypeRule),
    FromAnotherGate(FromGateRule),
    NextToGate(NextToRule),
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
    /// Where the gate goes on a sample with no valley to find - a smear: the
    /// edge of this gate there, usually the same gate under another parent.
    /// A run places it first when a rule places it.
    #[serde(default)]
    pub fallback: Option<crate::gate_rules::rule_store::RuleTarget>,
}

impl Default for ValleyRule {
    fn default() -> Self {
        Self {
            smoothing: 1.0,
            confidence: CountAndSeparation::default(),
            fallback: None,
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
        let found =
            crate::gate_rules::threshold::valley_for_gate(values, self.smoothing, reference_x)?;
        Ok(ValleyRead {
            peak: found.peak,
            bottom: found.bottom,
            depth: found.depth,
            offset: reference_x - found.bottom,
            at: reference_x,
        })
    }

    /// Find this sample's valley and put the gate the same distance from it.
    /// `gate` is where the gate stands on this sample now.
    pub fn place(
        &self,
        values: &[f64],
        offset: f64,
        gate: f64,
    ) -> Result<ValleyRead, crate::gate_rules::threshold::NoValley> {
        let found = crate::gate_rules::threshold::valley_for_gate(values, self.smoothing, gate)?;
        Ok(ValleyRead {
            peak: found.peak,
            bottom: found.bottom,
            depth: found.depth,
            offset,
            at: found.bottom + offset,
        })
    }

    /// The fallback as a rule from another gate: this gate's leading edge on
    /// `parameter` - the one the valley would have set - where the fallback's
    /// same edge is.
    pub fn fallback_rule(&self, parameter: &Arc<str>, bound: Bound) -> Option<FromGateRule> {
        let side = match bound {
            Bound::Above => Side::Lower,
            Bound::Below => Side::Upper,
        };
        Some(FromGateRule {
            same_shape_as: None,
            edges: vec![EdgeFrom {
                anchor: self.fallback.clone()?,
                parameter: parameter.clone(),
                side,
                anchor_side: side,
                gap: 0.0,
            }],
        })
    }

    pub fn describe(&self) -> String {
        let mut how =
            "in the dip between the negative and the positive, as on the reference".to_string();
        if self.smoothing != 1.0 {
            how.push_str(&format!(", smoothed x{:.2}", self.smoothing));
        }
        if let Some(fallback) = &self.fallback {
            how.push_str(&format!("; with no dip, where {} is", fallback.describe()));
        }
        how
    }
}

/// "In the valley where there is one; where there is a smear, as on an
/// example of one."
///
/// One rule for a gate that is a clear population on some samples and a
/// smear on others. Each sample is read for a dip between its negative and
/// its positive: with one, the gate goes in it, as [`ValleyRule`] puts it;
/// without, it goes as far above the negative as on a hand-gated smear, as
/// [`AboveTheNegativeRule`] puts it - or where another gate is, with
/// `fallback`.
///
/// The smear example is the reference when the reference is itself a smear.
/// When the reference has a dip, its gate says nothing about where a smear is
/// cut, so a run stops at the first smear for a person to gate it by hand,
/// and that sample becomes `smear_example`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValleyOrSmearRule {
    /// Scales the bandwidth the dip is looked for with - see
    /// [`ValleyRule::smoothing`].
    #[serde(default = "one")]
    pub smoothing: f64,
    #[serde(default)]
    pub confidence: CountAndSeparation,
    /// On a smear, where this gate is - usually the same gate under another
    /// parent - rather than as on the smear example.
    #[serde(default)]
    pub fallback: Option<crate::gate_rules::rule_store::RuleTarget>,
    /// The hand-gated sample a smear is placed from, named as a rule names a
    /// file, once one is known.
    #[serde(default)]
    pub smear_example: Option<Arc<str>>,
}

impl Default for ValleyOrSmearRule {
    fn default() -> Self {
        Self {
            smoothing: 1.0,
            confidence: CountAndSeparation::default(),
            fallback: None,
            smear_example: None,
        }
    }
}

impl ValleyOrSmearRule {
    /// The rule a sample with a dip is placed by.
    pub fn valley(&self) -> ValleyRule {
        ValleyRule {
            smoothing: self.smoothing,
            confidence: self.confidence.clone(),
            fallback: self.fallback.clone(),
        }
    }

    /// The rule a smear is placed by, against the smear example: the gate on
    /// a smear sits in the dim cells, so the negative is its peak, not all
    /// that is below the gate.
    pub fn smear(&self) -> AboveTheNegativeRule {
        AboveTheNegativeRule {
            find: NegativeFinder::NegativePeak,
            confidence: self.confidence.clone(),
            ..AboveTheNegativeRule::default()
        }
    }

    pub fn describe(&self) -> String {
        let mut how = "in the dip between the negative and the positive, as on the reference; \
                       on a smear, "
            .to_string();
        match (&self.fallback, &self.smear_example) {
            (Some(fallback), _) => how.push_str(&format!("where {} is", fallback.describe())),
            (None, Some(example)) => {
                how.push_str(&format!("as far above the negative as on {example}"))
            }
            (None, None) => how.push_str("as far above the negative as on a smear gated by hand"),
        }
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
            Rule::AboveTheNegative(_)
            | Rule::InTheValley(_)
            | Rule::ValleyOrSmear(_)
            | Rule::MatchThePhenotype(_)
            | Rule::FromAnotherGate(_)
            | Rule::NextToGate(_) => Err(SolveError::BadBand { band: (0.0, 0.0) }),
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
            Rule::ValleyOrSmear(r) => r.confidence.assess(threshold, reference_x),
            Rule::MatchThePhenotype(_) | Rule::FromAnotherGate(_) | Rule::NextToGate(_) => {
                return None;
            }
        })
    }

    pub fn solve(&self, values: &[f64]) -> Result<Threshold, SolveError> {
        match self {
            Rule::TailFraction(r) => r.solve(values),
            Rule::PercentileOffset(r) => r.solve(values),
            Rule::AboveTheNegative(_)
            | Rule::InTheValley(_)
            | Rule::ValleyOrSmear(_)
            | Rule::MatchThePhenotype(_)
            | Rule::FromAnotherGate(_)
            | Rule::NextToGate(_) => Err(SolveError::BadBand { band: (0.0, 0.0) }),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Rule::TailFraction(r) => r.describe(),
            Rule::PercentileOffset(r) => r.describe(),
            Rule::AboveTheNegative(r) => r.describe(),
            Rule::InTheValley(r) => r.describe(),
            Rule::ValleyOrSmear(r) => r.describe(),
            Rule::MatchThePhenotype(r) => r.describe(),
            Rule::FromAnotherGate(r) => r.describe(),
            Rule::NextToGate(r) => r.describe(),
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
            | Rule::ValleyOrSmear(_)
            | Rule::MatchThePhenotype(_)
            | Rule::FromAnotherGate(_)
            | Rule::NextToGate(_) => None,
        }
    }

    /// The fractions a gate may already hold and be left where it is - the
    /// band itself, or its middle for a band rule aiming there.
    pub fn kept_band(&self) -> Option<(f64, f64)> {
        match self {
            Rule::TailFraction(r) => Some(r.kept_band()),
            _ => None,
        }
    }

    /// Where in the band a band rule aims; `None` for any other rule.
    pub fn band_aim(&self) -> Option<BandAim> {
        match self {
            Rule::TailFraction(r) => Some(r.aim),
            _ => None,
        }
    }

    /// Whether the gate's place is read off another gate on the same sample,
    /// not off the events: measured on its own sample, whatever it says.
    pub fn reads_another_gate(&self) -> bool {
        matches!(self, Rule::FromAnotherGate(_) | Rule::NextToGate(_))
    }

    /// The gates this rule reads a position from, which a run places first.
    pub fn anchors(&self) -> Vec<&crate::gate_rules::rule_store::RuleTarget> {
        match self {
            Rule::FromAnotherGate(r) => r.anchors(),
            Rule::InTheValley(r) => r.fallback.iter().collect(),
            Rule::ValleyOrSmear(r) => r.fallback.iter().collect(),
            Rule::NextToGate(r) => vec![&r.anchor],
            _ => Vec::new(),
        }
    }

    /// The name of the kind, for the Gate Rules tab's list.
    pub fn kind(&self) -> &'static str {
        match self {
            Rule::TailFraction(_) => "Tail fraction",
            Rule::PercentileOffset(_) => "Percentile offset",
            Rule::AboveTheNegative(_) => "Above the negative",
            Rule::InTheValley(_) => "In the valley",
            Rule::ValleyOrSmear(_) => "Valley or smear",
            Rule::MatchThePhenotype(_) => "Match the phenotype",
            Rule::FromAnotherGate(_) => "From another gate",
            Rule::NextToGate(_) => "Next to another gate",
        }
    }
}

// ── matching a population by what it is ──────────────────────────────────

/// What to do with the outline once the population has been found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeFit {
    /// Carry the gate as drawn edge by edge, without changing its shape.
    ///
    /// For an outline that carries meaning the data does not: a rectangle that
    /// stands for a quadrant, a shape agreed with a collaborator, a gate that
    /// has to stay comparable with how it was drawn before. Each edge goes
    /// where it sits on the reference against the sample's own negative and
    /// valley, so the gate may grow or shrink; what it looks like is kept.
    ///
    /// Also the only option that preserves the *kind* of gate - a rectangle
    /// stays a rectangle, an ellipse an ellipse.
    #[default]
    KeepShape,
    /// Slide the gate as drawn, its size and shape unchanged.
    MoveOnly,
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
            ShapeFit::MoveOnly => "move it only, the same size",
            ShapeFit::DrawPolygon => "draw a new polygon round the cells",
        }
    }

    pub fn choice(self) -> &'static str {
        match self {
            ShapeFit::KeepShape => {
                "keep the shape - move and resize it, and stay the kind of gate it is"
            }
            ShapeFit::MoveOnly => "move only - slide it, the same size and shape",
            ShapeFit::DrawPolygon => {
                "draw a new polygon - follow the cells, whatever shape they make"
            }
        }
    }

    pub const ALL: [ShapeFit; 3] = [
        ShapeFit::KeepShape,
        ShapeFit::MoveOnly,
        ShapeFit::DrawPolygon,
    ];

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|fit| fit.key() == key)
    }

    /// The serialised name, which is also what the menu round-trips on.
    pub fn key(self) -> &'static str {
        match self {
            ShapeFit::KeepShape => "KeepShape",
            ShapeFit::MoveOnly => "MoveOnly",
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
    /// The markers whose edge is pinned to their negative: the side of the
    /// gate nearest the marker's negative keeps as many of the negative's
    /// widths from its peak as on the reference, rather than its place
    /// between the populations. For an edge drawn against the negative, where
    /// what lies between it and the population varies from sample to sample.
    /// Each must be one of the gate's two axes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<Arc<str>>,
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
            "find the cells that match on {markers}, then {}{}",
            self.fit.label(),
            pinned_said(&self.pinned, |m| m.to_string())
        )
    }
}

/// ", its edge on CD8 pinned to the negative", for a rule's description.
pub fn pinned_said(pinned: &[Arc<str>], name: impl Fn(&str) -> String) -> String {
    if pinned.is_empty() {
        return String::new();
    }
    let names: Vec<String> = pinned.iter().map(|m| name(m)).collect();
    format!(
        ", its edge on {} pinned to the negative",
        names.join(" and ")
    )
}

impl Default for PhenotypeRule {
    fn default() -> Self {
        Self {
            markers: Vec::new(),
            fit: ShapeFit::default(),
            keep: ninety_five(),
            smoothing: one(),
            vertices: two_dozen(),
            pinned: Vec::new(),
        }
    }
}

// ── following another gate ────────────────────────────────────────────────

/// Which edge of a gate, on one parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// The low end - the left edge on x, the bottom on y.
    Lower,
    /// The high end - the right edge on x, the top on y.
    Upper,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Lower => "lower",
            Side::Upper => "upper",
        }
    }
}

/// One edge of this gate set from one edge of another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeFrom {
    /// The gate whose edge this one takes, named as a rule names a gate.
    pub anchor: crate::gate_rules::rule_store::RuleTarget,
    /// The parameter both gates are drawn on, whose edge is set.
    pub parameter: Arc<str>,
    /// Which edge of this gate moves.
    pub side: Side,
    /// Which edge of the anchor it is set to.
    pub anchor_side: Side,
    /// Added to the anchor's edge, in the units the plot is drawn in. 0 puts
    /// the two edges together.
    #[serde(default)]
    pub gap: f64,
}

/// "In the same position as that gate", or "adjacent to that gate's edge".
///
/// A gating guide says this of a good many gates: the MAIT CD4-CD8+ gate goes
/// where the main one is, CD154 on MAIT cells where it is on CD4 T cells, the
/// CD19- gate against the left edge of CD19+CD14-. None of that is read from
/// the data - it is read from another gate, on the same sample, after that
/// gate has been placed. So a run places the anchor first (see
/// `autogate::rule_levels`) and this gate follows it, sample by sample.
///
/// Exactly one of the two is given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FromGateRule {
    /// Take this gate's whole shape - for a gate drawn on the same two
    /// parameters. A quadrant takes another quadrant's lines.
    #[serde(default)]
    pub same_shape_as: Option<crate::gate_rules::rule_store::RuleTarget>,
    /// Set these edges, each from its own anchor. A rectangle's edge moves on
    /// its own, the others staying where they are; any other shape slides
    /// whole until that edge is there.
    #[serde(default)]
    pub edges: Vec<EdgeFrom>,
}

impl FromGateRule {
    /// Every gate this one reads.
    pub fn anchors(&self) -> Vec<&crate::gate_rules::rule_store::RuleTarget> {
        self.same_shape_as
            .iter()
            .chain(self.edges.iter().map(|e| &e.anchor))
            .collect()
    }

    /// What is wrong with it as written, before any gate is looked at.
    pub fn problem(&self) -> Option<&'static str> {
        match (&self.same_shape_as, self.edges.is_empty()) {
            (Some(_), false) => Some(
                "a rule from another gate takes the whole shape or sets edges, not both - \
                 give same_shape_as or edges",
            ),
            (None, true) => {
                Some("a rule from another gate needs same_shape_as, or at least one edge to set")
            }
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match &self.same_shape_as {
            Some(anchor) => format!("the same shape as {}", anchor.describe()),
            None => self
                .edges
                .iter()
                .map(|e| {
                    let gap = if e.gap == 0.0 {
                        String::new()
                    } else {
                        format!(" {} {}", if e.gap < 0.0 { "-" } else { "+" }, e.gap.abs())
                    };
                    format!(
                        "{} edge on {} at the {} edge of {}{gap}",
                        e.side.label(),
                        e.parameter,
                        e.anchor_side.label(),
                        e.anchor.describe()
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        }
    }
}

/// How a gate placed next to another comes to meet it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Meet {
    /// Its side facing the other gate moves, every point of it alike; the
    /// side away stays. It shrinks back where it overlaps.
    #[default]
    GrowSide,
    /// Where it lies alongside the other gate, its facing side takes the
    /// other's outline - no gap anywhere along it. A polygon's only: a
    /// rectangle grows its side.
    FollowOutline,
    /// The whole gate slides, its shape as it is.
    Slide,
}

impl Meet {
    pub const ALL: [Meet; 3] = [Meet::GrowSide, Meet::FollowOutline, Meet::Slide];

    /// How the rules file writes it.
    pub fn key(self) -> &'static str {
        match self {
            Meet::GrowSide => "GrowSide",
            Meet::FollowOutline => "FollowOutline",
            Meet::Slide => "Slide",
        }
    }

    pub fn from_key(key: &str) -> Option<Meet> {
        Meet::ALL.into_iter().find(|meet| meet.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Meet::GrowSide => "growing its facing side",
            Meet::FollowOutline => "following the other's outline",
            Meet::Slide => "sliding whole",
        }
    }
}

/// "Next to that gate": against it along one axis, as close as it can be
/// without overlapping - the CD19- gate grown up to the CD19+CD14- gate
/// wherever a rule puts that one. The other gate is placed first; both are
/// on the same plot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NextToRule {
    /// The gate it sits next to, named as a rule names a gate.
    pub anchor: crate::gate_rules::rule_store::RuleTarget,
    /// The parameter it moves along.
    pub parameter: Arc<str>,
    /// Which side of the anchor it sits on: lower is to the left of it, or
    /// below it.
    pub side: Side,
    #[serde(default)]
    pub meet: Meet,
    /// Left between the two, in the plot's units. 0 is touching.
    #[serde(default)]
    pub gap: f64,
}

/// Why `gap` cannot be a next-to rule's gap, if it cannot.
pub fn gap_problem(gap: f64) -> Option<&'static str> {
    (!gap.is_finite() || gap < 0.0).then_some("the gap must be a number, 0 or more")
}

impl NextToRule {
    pub fn describe(&self) -> String {
        let side = match self.side {
            Side::Lower => "lower than",
            Side::Upper => "higher than",
        };
        let gap = if self.gap == 0.0 {
            String::new()
        } else {
            format!(", {} apart", self.gap)
        };
        format!(
            "next to {}, {side} it on {}, {}{gap}",
            self.anchor.describe(),
            self.parameter,
            self.meet.label()
        )
    }
}
