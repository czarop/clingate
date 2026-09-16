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
}

impl Rule {
    /// Solve and score, dispatching to the variant's own implementation.
    pub fn apply(&self, values: &[f64], reference_x: Option<f64>) -> Result<Solved, SolveError> {
        match self {
            Rule::TailFraction(r) => r.apply(values, reference_x),
            Rule::PercentileOffset(r) => r.apply(values, reference_x),
        }
    }

    /// Score a result this rule's own way, for a threshold arrived at by some
    /// other means than [`Rule::solve`] - sliding the gate until it captures
    /// the right fraction, say.
    pub fn assess(
        &self,
        threshold: &Threshold,
        reference_x: Option<f64>,
    ) -> crate::gate_rules::confidence::Confidence {
        match self {
            Rule::TailFraction(r) => r.confidence_model().assess(threshold, reference_x),
            Rule::PercentileOffset(r) => r.confidence_model().assess(threshold, reference_x),
        }
    }

    pub fn solve(&self, values: &[f64]) -> Result<Threshold, SolveError> {
        match self {
            Rule::TailFraction(r) => r.solve(values),
            Rule::PercentileOffset(r) => r.solve(values),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Rule::TailFraction(r) => r.describe(),
            Rule::PercentileOffset(r) => r.describe(),
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
            Rule::PercentileOffset(_) => None,
        }
    }

    /// The name of the kind, for the Gate Rules tab's list.
    pub fn kind(&self) -> &'static str {
        match self {
            Rule::TailFraction(_) => "Tail fraction",
            Rule::PercentileOffset(_) => "Percentile offset",
        }
    }
}
