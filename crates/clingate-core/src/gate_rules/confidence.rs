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
    /// A score outside 0 to 1 is clamped into it.
    ///
    /// A score that is not a number - something that could not be measured,
    /// such as a ratio with nothing to divide by - scores 0 and says so. It
    /// used to stay NaN, which the overall's `min` then passed over, so the
    /// gate was ranked as trustworthy for exactly the reason it should be
    /// looked at (B-CONF-1).
    pub fn new(name: &'static str, score: f64, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if score.is_nan() {
            return Self {
                name,
                score: 0.0,
                detail: if detail.is_empty() {
                    "could not be measured".to_string()
                } else {
                    format!("could not be measured: {detail}")
                },
            };
        }
        Self {
            name,
            score: score.clamp(0.0, 1.0),
            detail,
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
    ///
    /// A NaN score - which [`Component::new`] never makes, but the fields are
    /// public - counts as 0 rather than being skipped by `min`.
    pub fn from_components(components: Vec<Component>) -> Self {
        let score = components
            .iter()
            .map(|c| if c.score.is_nan() { 0.0 } else { c.score })
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
    fn assess(&self, threshold: &Threshold) -> Confidence;
}

// ─── The model both threshold rules start on ──────────────────────────────────

/// Component names, so a report can look one up without matching on prose.
pub const EVENTS: &str = "parent event count";
pub const ADMITTED: &str = "events in the gate";
pub const STABILITY: &str = "stability of the gate's contents";
pub const BAND: &str = "rule satisfied";
pub const VALLEY: &str = "depth of the valley it sat in";
pub const RIGHT_SIDE: &str = "the negative's right side against the reference";
pub const MATCHED: &str = "events matching the phenotype";
pub const PURITY: &str = "how much else the gate holds";
pub const CAUGHT: &str = "how much of the population the gate holds";
pub const ONE_CLOUD: &str = "whether the matched cells form one cloud";
pub const STEADY: &str = "whether the edges agree between halves of the events";
pub const FALLBACK: &str = "no valley, so placed from another gate";
pub const HELD_BACK: &str = "held back off another gate";

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
    /// The count swing at which stability is judged to have halved. A swing of
    /// 1 means nudging the edge changes the gate's contents by as much as it
    /// holds.
    pub swing_half: f64,
}

impl Default for ConfidenceLimits {
    fn default() -> Self {
        Self {
            // Parental gates normally hold thousands to tens of thousands.
            events_full: 10_000.0,
            events_floor: 100.0,
            swing_half: 1.0,
        }
    }
}

/// The model both starting rules use: how much data there was, how much of it
/// landed in the gate, how cleanly the edge separates it, and whether the rule
/// was satisfiable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CountAndSeparation {
    pub limits: ConfidenceLimits,
}

impl ConfidenceModel for CountAndSeparation {
    fn assess(&self, t: &Threshold) -> Confidence {
        let components = vec![
            Component::new(
                EVENTS,
                event_count_score(t.parent_events as f64, &self.limits),
                format!("{} events in the parent population", t.parent_events),
            ),
            Component::new(
                ADMITTED,
                admitted_count_score(t.events_admitted, t.parent_events),
                format!(
                    "{} events admitted ({:.3}%), {} excluded",
                    t.events_admitted,
                    t.fraction_admitted * 100.0,
                    t.parent_events.saturating_sub(t.events_admitted)
                ),
            ),
            Component::new(
                STABILITY,
                stability_score(t, &self.limits),
                stability_detail(t),
            ),
            Component::new(BAND, band_score(t), band_detail(t)),
        ];
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

/// The smaller of the two sides the edge divides carries Poisson noise of about
/// `sqrt(k)`, so the relative error on the fraction the rule was aiming at is
/// `1/sqrt(k)`.
///
/// This is the component the parent count misses. A rule asking for 0.2% of
/// 2,000 events is asking about four cells: the parent is comfortable, the
/// answer is not, and no amount of parent population fixes it.
///
/// **The smaller side, not the admitted one.** A negative gate asked to hold
/// 99.7% of its parent admits nearly all of it, and scoring on that count says
/// the placement is certain - when where the edge sits is decided entirely by
/// the hundred-odd events it excludes. The noise lives on whichever side is
/// scarce, and an edge is equally uncertain whether the scarce side is the one
/// being kept or the one being cut.
fn admitted_count_score(k: usize, parent: usize) -> f64 {
    let scarce = k.min(parent.saturating_sub(k));
    if scarce == 0 {
        return 0.0;
    }
    (1.0 - 1.0 / (scarce as f64).sqrt()).clamp(0.0, 1.0)
}

/// Move the edge slightly and see whether the answer survives.
///
/// `1 / (1 + swing)` falls from 1 at no swing through a half at `swing_half`,
/// and never reaches zero - a fragile placement is still a placement. On real
/// data this spread from 0.08 for a channel where a nudge changed the contents
/// twelve times over to 0.89 for a cleanly separated one, which is the range a
/// ranking needs.
fn stability_score(t: &Threshold, limits: &ConfidenceLimits) -> f64 {
    if t.parent_spread <= 0.0 || t.events_admitted == 0 {
        // Nothing was measured against anything, so claim nothing.
        return 0.0;
    }
    let scale = if limits.swing_half > 0.0 {
        limits.swing_half
    } else {
        1.0
    };
    (1.0 / (1.0 + t.count_swing / scale)).clamp(0.0, 1.0)
}

fn stability_detail(t: &Threshold) -> String {
    if t.parent_spread <= 0.0 {
        return "the population has no interquartile spread to measure against".into();
    }
    format!(
        "nudging the edge by a tenth of the spread changes the contents by {:.0}%",
        t.count_swing * 100.0
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

// ─── The side of the negative above-the-negative does not read ───────────────

/// Where the gate sits against the negative's right side, here and on the
/// reference - the check above-the-negative cannot make for itself.
///
/// That rule reads the negative's left side and paces the gate out as if the
/// right were its mirror. When a negative changes shape - its median shifts
/// up the arcsinh scale, where the right side is squeezed more than the left;
/// its tail shortens - the mirror no longer fits, and the gate lands further
/// out from where the negative actually ends than on the reference. This
/// measures that directly: how many right-side widths above the peak the gate
/// sits (`k`), against the same on the reference (`k_ref`).
///
/// Scored by direction, because the two directions mean different things:
///
/// - **Further out** (`k / k_ref` above 1): positives cannot cause it - they
///   only ever widen the right side, which pulls `k` down. So this is the
///   negative's own shape, and the gate is likely too high. 1.25 times the
///   reference's is still fine; twice it scores 0.
/// - **Closer in** (below 1): the right side has widened, which is what a
///   positive smear running into the negative does. That is often the sample
///   being stimulated, not wrong, so it never scores below 0.5: noted, and
///   ranked lower, but not flagged on its own.
///
/// `None` where nothing can be compared - the reference's gate at or below its
/// own peak, or its right side unreadable.
pub fn right_side(
    reference_at: f64,
    reference: crate::gate_rules::threshold::PeakSides,
    at: f64,
    here: crate::gate_rules::threshold::PeakSides,
) -> Option<Component> {
    let reference_right = reference.right.filter(|w| *w > 0.0)?;
    let k_ref = (reference_at - reference.peak) / reference_right;
    if !k_ref.is_finite() || k_ref <= 0.0 {
        return None;
    }
    let Some(right) = here.right.filter(|w| *w > 0.0) else {
        return Some(Component::new(
            RIGHT_SIDE,
            0.5,
            format!(
                "the negative's right side never falls to {:.0}% of its peak - it runs into what \
                 is above it, so where it ends cannot be read",
                crate::gate_rules::threshold::SIDE_HEIGHT * 100.0
            ),
        ));
    };
    let k = (at - here.peak) / right;
    let ratio = k / k_ref;
    let score = if ratio >= 1.0 {
        // 1 up to 1.25, 0 by 2.
        1.0 - ((ratio - 1.25) / 0.75).clamp(0.0, 1.0)
    } else {
        // 1 down to 0.7, 0.5 by 0.4 and below.
        1.0 - 0.5 * ((0.7 - ratio) / 0.3).clamp(0.0, 1.0)
    };
    let sides = |s: &crate::gate_rules::threshold::PeakSides, right: f64| match s.left {
        Some(left) if left > 0.0 => format!("right side {:.2} times the left", right / left),
        _ => "left side unreadable".to_string(),
    };
    Some(Component::new(
        RIGHT_SIDE,
        score,
        format!(
            "the gate sits {k:.2} right-side widths above the negative's peak against {k_ref:.2} \
             on the reference ({:.0}% as far out; {} here, {} on the reference) - the rule reads \
             only the left side",
            ratio * 100.0,
            sides(&here, right),
            sides(&reference, reference_right),
        ),
    ))
}

// ─── The model a phenotype rule is judged on ─────────────────────────────────

/// What a phenotype rule found, as the scorer needs it.
///
/// Its own model rather than [`Threshold`]: that describes a cut along one
/// axis - where it sits, what it admits, how a nudge changes it - and none of
/// those exist here. A rule that identifies cells and draws a boundary round
/// them is trustworthy or not for entirely different reasons, and squeezing it
/// into the other shape would produce a number that looked comparable and was
/// not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchEvidence {
    /// Events matching the phenotype, and the parent they came from.
    pub matched: usize,
    pub parent: usize,
    /// The fraction of what the fitted gate holds that is the population.
    pub purity: f64,
    /// The fraction of the population the fitted gate holds.
    pub caught: f64,
    /// How many separate clouds the matched cells formed.
    pub pieces: usize,
    /// Of the cells the edges placed from either half of the events hold,
    /// the share both hold; `None` where no edges were placed.
    pub steady: Option<f64>,
}

/// Judge a phenotype rule's placement.
pub fn assess_match(found: MatchEvidence) -> Confidence {
    let mut components = Vec::new();

    // The same scarce-side count rule the threshold rules use: what limits a
    // measurement is whichever side of the boundary has fewer events.
    components.push(Component::new(
        MATCHED,
        admitted_count_score(found.matched, found.parent),
        format!("{} of {} events matched", found.matched, found.parent),
    ));

    // Purity and catch are already fractions of exactly the thing being
    // asked about, so they are their own scores. Purity low is not a fault in
    // the fitting - it is the population not being separated on these two
    // axes, which is the honest reason to distrust the gate.
    components.push(Component::new(
        PURITY,
        if found.purity.is_finite() {
            found.purity
        } else {
            0.0
        },
        format!(
            "{:.0}% of what the gate holds is the population",
            found.purity * 100.0
        ),
    ));
    components.push(Component::new(
        CAUGHT,
        if found.caught.is_finite() {
            found.caught
        } else {
            0.0
        },
        format!(
            "the gate holds {:.0}% of the population",
            found.caught * 100.0
        ),
    ));

    // Two clouds means one outline cannot describe them, whichever is drawn.
    components.push(Component::new(
        ONE_CLOUD,
        match found.pieces {
            0 => 0.0,
            1 => 1.0,
            n => 1.0 / n as f64,
        },
        match found.pieces {
            1 => "the matched cells form one cloud".to_string(),
            n => format!("the matched cells form {n} separate clouds"),
        },
    ));

    // An edge placed on a flat stretch of density - a smear with no real dip -
    // lands wherever the noise puts it, and two halves of the same events
    // then put it in different places. Agreement is a share of cells, so it
    // needs no scale and counts what a wandering edge costs.
    if let Some(steady) = found.steady {
        components.push(Component::new(
            STEADY,
            if steady.is_finite() { steady } else { 0.0 },
            format!(
                "placed from either half of the events, the gate holds {:.0}% the same cells",
                steady * 100.0
            ),
        ));
    }
    Confidence::from_components(components)
}
