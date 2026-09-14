//! How much to trust a gate a rule placed.
//!
//! A rule always produces a position. Whether that position deserves to be used
//! without a person looking at it is a different question, and the point of a
//! score is to sort a run of hundreds of gates so the handful that need review
//! rise to the top.
//!
//! Four things are scored separately and the weakest one wins. Separately,
//! because "low confidence" is not actionable but "low confidence: 3 events in
//! the gate" is; weakest-wins rather than a product, because a gate that is
//! merely good on four counts should not be dragged down to a warning by
//! multiplying four 0.8s together, while one that is genuinely bad on a single
//! count must not be rescued by the other three.
//!
//! **The thresholds below are guesses.** They are shaped like the right curves -
//! the statistics behind each is noted - but the constants have not been
//! calibrated against real gating yet. That is what the scoring harness is for:
//! run rules over a hand-gated dataset, see where the score disagrees with a
//! person's judgement, and move the constants. Until then, treat the score as
//! a ranking rather than a measurement.

use crate::gate_rules::threshold::{Status, Threshold};

/// Where each component stops being a concern. Tunable because the constants
/// are not yet earned - see the module note.
#[derive(Debug, Clone, PartialEq)]
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

/// The parts of the score, each 0 to 1, and the overall.
#[derive(Debug, Clone, PartialEq)]
pub struct Confidence {
    /// Statistical power of the parent population.
    pub events: f64,
    /// Noise on the count the rule actually targets.
    pub admitted: f64,
    /// How far the edge sits from the nearest event on either side.
    pub separation: f64,
    /// Whether the rule was satisfiable.
    pub band: f64,
    /// How far the gate moved from its reference position. `None` when there is
    /// no reference to compare against.
    pub displacement: Option<f64>,
    /// The weakest component - what the gate should be ranked by.
    pub score: f64,
}

impl Confidence {
    /// Score a solved threshold.
    ///
    /// `reference_x` is where the same gate sits on the QC or template sample,
    /// when there is one. A rule that moves a gate a long way from it may be
    /// right - baselines do shift - but it is the first thing worth a person's
    /// eye, which is exactly what a low score is for.
    pub fn of(threshold: &Threshold, reference_x: Option<f64>, limits: &ConfidenceLimits) -> Self {
        let events = event_count_score(threshold.parent_events as f64, limits);
        let admitted = admitted_count_score(threshold.events_admitted);
        let separation = separation_score(threshold, limits);
        let band = band_score(threshold);
        let displacement =
            reference_x.map(|reference| displacement_score(threshold, reference, limits));

        let mut score = events.min(admitted).min(separation).min(band);
        if let Some(d) = displacement {
            score = score.min(d);
        }

        Self {
            events,
            admitted,
            separation,
            band,
            displacement,
            score,
        }
    }

    /// The component that dragged the score down, for the report to name.
    pub fn limiting_factor(&self) -> &'static str {
        let mut worst = ("parent event count", self.events);
        for (name, value) in [
            ("events in the gate", self.admitted),
            ("separation from the population", self.separation),
            ("the rule could not be satisfied", self.band),
        ] {
            if value < worst.1 {
                worst = (name, value);
            }
        }
        if let Some(d) = self.displacement
            && d < worst.1
        {
            worst = ("distance moved from the reference", d);
        }
        worst.0
    }
}

/// Log-scaled between the floor and the point where it stops mattering, because
/// the difference between 200 and 2000 events matters far more than between
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
/// answer is not. Four cells means a relative error of around a half, and no
/// amount of parent population fixes that.
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
fn separation_score(threshold: &Threshold, limits: &ConfidenceLimits) -> f64 {
    if threshold.parent_spread <= 0.0 {
        // Half the population sits on a single value. Nothing can be said about
        // separation, so claim nothing.
        return 0.0;
    }
    let relative = threshold.separation / threshold.parent_spread;
    (relative / limits.separation_full).clamp(0.0, 1.0)
}

/// A rule that could not be satisfied gets a score in proportion to how far
/// outside its band the result fell, measured against the band's own width so
/// that missing a tight band is not punished like missing a wide one.
fn band_score(threshold: &Threshold) -> f64 {
    let Status::OutOfBand { band } = threshold.status else {
        return 1.0;
    };
    let (lower, upper) = band;
    let miss = if threshold.fraction_admitted < lower {
        lower - threshold.fraction_admitted
    } else {
        threshold.fraction_admitted - upper
    };
    // A band of zero width has no scale of its own; fall back to its position.
    let scale = if upper > lower {
        upper - lower
    } else {
        upper.max(f64::EPSILON)
    };
    (1.0 - miss / scale).clamp(0.0, 1.0)
}

/// Distance from the reference position, against the spread of the population.
fn displacement_score(threshold: &Threshold, reference: f64, limits: &ConfidenceLimits) -> f64 {
    if threshold.parent_spread <= 0.0 {
        return 0.0;
    }
    let moved = (threshold.x - reference).abs() / threshold.parent_spread;
    (1.0 - moved / limits.displacement_limit).clamp(0.0, 1.0)
}
