//! How much to trust a gate a rule placed.
//!
//! A rule always produces a position. Whether that position deserves to be used
//! without a person looking at it is a different question, and the point of a
//! score is to sort a run of hundreds of gates so the handful that need review
//! rise to the top.
//!
//! Scoring is a trait rather than a function because it belongs to the rule.
//! The two rules here start on the same model - the things that make any
//! threshold trustworthy - but a rule that finds a positive population will want
//! to say something about peak prominence, and one that steps off a shoulder
//! will want to say something about how flat that shoulder is. Each rule naming
//! its own model is what lets those diverge without disturbing each other.
//!
//! Components are named and carried, not collapsed: "low confidence" is not
//! actionable, "low confidence: 3 events in the gate" is.

use crate::gate_rules::threshold::{Status, Threshold};
use serde::{Deserialize, Serialize};

/// One reason to trust or distrust a placement, scored 0 to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub name: &'static str,
    pub score: f64,
    /// The measurement behind the score, for the report to quote.
    pub detail: String,
}

impl Component {
    pub fn new(name: &'static str, score: f64, detail: impl Into<String>) -> Self {
        Self {
            name,
            score: score.clamp(0.0, 1.0),
            detail: detail.into(),
        }
    }
}

/// The parts, and the overall.
#[derive(Debug, Clone, PartialEq)]
pub struct Confidence {
    pub components: Vec<Component>,
    pub score: f64,
}

impl Confidence {
    /// The overall is the weakest component, not the product.
    ///
    /// A gate that is merely good on four counts should not be multiplied down
    /// into a warning - four 0.8s make 0.41 - while one that is genuinely bad on
    /// a single count must not be rescued by the other three. For a ranking
    /// whose job is to surface gates for review, the weakest link is the
    /// question being asked.
    pub fn from_components(components: Vec<Component>) -> Self {
        let score = components
            .iter()
            .map(|c| c.score)
            .fold(1.0_f64, |acc, s| acc.min(s));
        Self { components, score }
    }

    /// The component that held the score down, for the report to name.
    pub fn weakest(&self) -> Option<&Component> {
        self.components
            .iter()
            .min_by(|a, b| a.score.total_cmp(&b.score))
    }

    pub fn get(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }
}

/// How a rule's results are judged. Each rule names its own, so the components
/// can be particular to what that rule actually did.
pub trait ConfidenceModel {
    /// `reference_x` is where the same gate sits on the QC or template sample,
    /// when there is one.
    fn assess(&self, threshold: &Threshold, reference_x: Option<f64>) -> Confidence;
}

// ─── The model both threshold rules start on ──────────────────────────────────

/// Component names, so a report can look one up without matching on prose.
pub const EVENTS: &str = "parent event count";
pub const ADMITTED: &str = "events in the gate";
pub const SEPARATION: &str = "separation from the population";
pub const BAND: &str = "rule satisfied";
pub const DISPLACEMENT: &str = "distance moved from the reference";

/// Where each component stops being a concern.
///
/// **These constants are guesses.** Each curve is shaped by the statistic
/// behind it - noted at the function - but none has been calibrated against
/// real gating. That is what the scoring harness is for: run rules over a
/// hand-gated dataset, see where the score disagrees with a person's judgement,
/// and move the numbers. Until then the score ranks rather than measures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceLimits {
    /// Parent events at which count is no longer a worry at all.
    pub events_full: f64,
    /// Parent events below which a placement is not worth anything.
    pub events_floor: f64,
    /// Separation, as a multiple of the parent's interquartile width, at which
    /// the edge is unambiguously in empty space.
    pub separation_full: f64,
    /// Displacement from the reference gate, as a multiple of the parent's
    /// interquartile width, at which the move is too large to trust.
    pub displacement_limit: f64,
}

impl Default for ConfidenceLimits {
    fn default() -> Self {
        Self {
            // Parental gates normally hold thousands to tens of thousands.
            events_full: 10_000.0,
            events_floor: 100.0,
            // A gap as wide as the middle half of the population is a clean
            // separation by any eye.
            separation_full: 1.0,
            // Two interquartile widths is a long way for a gate to travel
            // between samples.
            displacement_limit: 2.0,
        }
    }
}

/// The model both starting rules use: how much data there was, how much of it
/// landed in the gate, how cleanly the edge separates it, whether the rule was
/// satisfiable, and how far the gate travelled.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CountAndSeparation {
    pub limits: ConfidenceLimits,
}

impl ConfidenceModel for CountAndSeparation {
    fn assess(&self, t: &Threshold, reference_x: Option<f64>) -> Confidence {
        let mut components = vec![
            Component::new(
                EVENTS,
                event_count_score(t.parent_events as f64, &self.limits),
                format!("{} events in the parent population", t.parent_events),
            ),
            Component::new(
                ADMITTED,
                admitted_count_score(t.events_admitted),
                format!(
                    "{} events admitted ({:.3}%)",
                    t.events_admitted,
                    t.fraction_admitted * 100.0
                ),
            ),
            Component::new(
                SEPARATION,
                separation_score(t, &self.limits),
                separation_detail(t),
            ),
            Component::new(BAND, band_score(t), band_detail(t)),
        ];
        if let Some(reference) = reference_x {
            components.push(Component::new(
                DISPLACEMENT,
                displacement_score(t, reference, &self.limits),
                displacement_detail(t, reference),
            ));
        }
        Confidence::from_components(components)
    }
}

/// Log-scaled between the floor and the point where it stops mattering, because
/// the difference between 200 and 2,000 events matters far more than between
/// 20,000 and 200,000.
fn event_count_score(n: f64, limits: &ConfidenceLimits) -> f64 {
    if n >= limits.events_full {
        return 1.0;
    }
    if n <= limits.events_floor {
        return 0.0;
    }
    ((n / limits.events_floor).ln() / (limits.events_full / limits.events_floor).ln())
        .clamp(0.0, 1.0)
}

/// The count inside the gate carries Poisson noise of about `sqrt(k)`, so the
/// relative error on the fraction the rule was aiming at is `1/sqrt(k)`.
///
/// This is the component the parent count misses. A rule asking for 0.2% of
/// 2,000 events is asking about four cells: the parent is comfortable, the
/// answer is not, and no amount of parent population fixes it.
fn admitted_count_score(k: usize) -> f64 {
    if k == 0 {
        return 0.0;
    }
    (1.0 - 1.0 / (k as f64).sqrt()).clamp(0.0, 1.0)
}

/// The gap the edge sits in, read against the spread of the population.
///
/// Absolute width means nothing on its own - a gap of 0.1 is enormous on a
/// tight negative and negligible on a smeared one - so it is normalised by the
/// interquartile width.
fn separation_score(t: &Threshold, limits: &ConfidenceLimits) -> f64 {
    if t.parent_spread <= 0.0 {
        // Half the population sits on a single value. Nothing can be said about
        // separation, so claim nothing.
        return 0.0;
    }
    ((t.separation / t.parent_spread) / limits.separation_full).clamp(0.0, 1.0)
}

fn separation_detail(t: &Threshold) -> String {
    if t.parent_spread <= 0.0 {
        return "the population has no interquartile spread to measure against".into();
    }
    format!(
        "a gap of {:.4}, {:.2} times the interquartile spread",
        t.separation,
        t.separation / t.parent_spread
    )
}

/// A rule that could not be satisfied scores in proportion to how far outside
/// its band the result fell, measured against the band's own width so that
/// missing a tight band is not punished like missing a wide one.
fn band_score(t: &Threshold) -> f64 {
    let Status::OutOfBand { band } = t.status else {
        return 1.0;
    };
    let (lower, upper) = band;
    let miss = if t.fraction_admitted < lower {
        lower - t.fraction_admitted
    } else {
        t.fraction_admitted - upper
    };
    // A band of zero width has no scale of its own; fall back to its position.
    let scale = if upper > lower {
        upper - lower
    } else {
        upper.max(f64::EPSILON)
    };
    (1.0 - miss / scale).clamp(0.0, 1.0)
}

fn band_detail(t: &Threshold) -> String {
    match t.status {
        Status::InBand => "the gate captures a fraction inside the band".into(),
        Status::NoBand => "the rule names a position directly".into(),
        Status::OutOfBand { band } => format!(
            "no position satisfies {:.3}% to {:.3}%; the nearest captures {:.3}%",
            band.0 * 100.0,
            band.1 * 100.0,
            t.fraction_admitted * 100.0
        ),
    }
}

/// Distance from the reference position, against the spread of the population.
fn displacement_score(t: &Threshold, reference: f64, limits: &ConfidenceLimits) -> f64 {
    if t.parent_spread <= 0.0 {
        return 0.0;
    }
    let moved = (t.x - reference).abs() / t.parent_spread;
    (1.0 - moved / limits.displacement_limit).clamp(0.0, 1.0)
}

fn displacement_detail(t: &Threshold, reference: f64) -> String {
    if t.parent_spread <= 0.0 {
        return "the population has no interquartile spread to measure against".into();
    }
    format!(
        "moved {:.4} from {:.4}, {:.2} times the interquartile spread",
        t.x - reference,
        reference,
        (t.x - reference).abs() / t.parent_spread
    )
}
