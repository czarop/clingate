//! Assessing a rules run: which placements need a person to look at them.
//!
//! A placement's own confidence says how sure the rule was on that sample,
//! judged against its reference. What it cannot say is whether the sample,
//! and the gate on it, look like the rest of the run. This does: for each
//! gate the rules placed, each sample is compared with its **peers** - the
//! other samples of the same kind (the sample type the pairing names) whose
//! placement the rule was confident in - on:
//!
//! - **where the gate sits against the sample's own peaks**: between its
//!   negative and its positive, as a fraction of the way from one to the
//!   other. Percent positive can move for real reasons - a different
//!   timepoint, a stimulated sample - but a gate that sits in the valley on
//!   fifty samples and in the middle of the positive on one has been placed
//!   differently;
//! - **where it sits against the sample's spread**, in interquartile ranges
//!   from its median, for a population with no second peak;
//! - **what it lets through** - the fraction beyond the line;
//! - **the distribution itself**: shifted, wider or narrower than its peers,
//!   or with a different number of peaks - what usually explains the rest;
//! - and the rule's own confidence, and whether it reached its band.
//!
//! Each comparison is a robust z-score against the peers (median and median
//! absolute deviation, leaving the sample itself out). Whatever is unusual
//! is said in words, with the sample's value and the peers' typical one, so
//! a person - or Claude - can see why it was flagged.
//!
//! Everything here is read from the run kept in the workspace, so assessing
//! a run reads no files and gives the same answer every time, in the app and
//! in the tools for Claude.

use std::collections::BTreeMap;

use serde::Serialize;

use super::run_record::{PlacementStatus, RunRecord, SampleRef, placement_status};
use super::shape::Shape;
use crate::gate_rules::rule_store::Bound;
use crate::gates::GateState;
use crate::omiq::metadata::MetaDataFileMap;

/// A placement at or above this confidence, and in its band, is one its
/// peers are measured against.
pub const CONFIDENT: f64 = 0.5;
/// Below this confidence a placement is flagged whatever its peers say - the
/// Gate Rules tab's review line.
pub const REVIEW_FLOOR: f64 = 0.30;
/// Fewer confident peers than this, and the sample is compared with every
/// other sample of its kind instead.
pub const MIN_PEERS: usize = 3;
/// A comparison scoring this far from its peers is said; this far and more
/// is worth a look.
pub const NOTABLE: f64 = 2.0;
pub const FLAG: f64 = 3.0;

/// One thing unusual about a placement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reason {
    /// What was compared - see [`Measure`].
    pub measure: &'static str,
    /// In words, with the numbers.
    pub says: String,
    /// How unusual, on the same scale for every measure: 2 is notable, 3 and
    /// over worth a look.
    pub severity: f64,
}

/// A placement worth a look, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Flag {
    pub gate_id: String,
    pub gate: String,
    pub parent_gate: Option<String>,
    pub sample: SampleRef,
    /// Whether the rule moved the gate, or left it where it already met the
    /// rule.
    pub moved: bool,
    pub confidence: Option<f64>,
    pub weakest: Option<String>,
    /// The worst of its reasons.
    pub severity: f64,
    pub reasons: Vec<Reason>,
    /// Whether the gate is still where the rule left it - one already moved
    /// by a person has most likely been dealt with.
    pub status: Option<PlacementStatus>,
    /// How many peers it was compared with, and whether they were confident
    /// ones.
    pub peers: usize,
    pub confident_peers: bool,
}

/// A gate across the run, in a line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GateSummary {
    pub gate_id: String,
    pub gate: String,
    pub parent_gate: Option<String>,
    /// Placements by sample type: how many, how many confident, how many
    /// flagged.
    pub by_type: Vec<TypeSummary>,
    pub median_confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypeSummary {
    pub sample_type: Option<String>,
    pub samples: usize,
    pub confident: usize,
    pub flagged: usize,
    /// Where the confident ones put the gate against their own peaks, as a
    /// fraction of the way from negative to positive - `None` where the
    /// population has no second peak.
    pub typical_position_between_peaks: Option<f64>,
    /// The fraction beyond the line, typically.
    pub typical_fraction_beyond: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assessment {
    pub run_applied_at: String,
    pub placements: usize,
    /// Worst first.
    pub flags: Vec<Flag>,
    pub gates: Vec<GateSummary>,
}

/// One placement, as the assessment reads it.
struct Item<'a> {
    gate_id: &'a str,
    gate: &'a str,
    parent_gate: Option<&'a str>,
    sample: &'a SampleRef,
    moved: bool,
    confidence: Option<f64>,
    weakest: Option<&'a str>,
    in_band: bool,
    /// Kept for being a reference, not for meeting the rule.
    reference: bool,
    line: Option<f64>,
    beyond: Option<f64>,
    shape: Option<&'a Shape>,
    bound: Option<Bound>,
    /// The placed record, for its status.
    placed: Option<&'a super::run_record::PlacedRecord>,
}

impl Item<'_> {
    fn confident(&self) -> bool {
        if self.reference {
            return false;
        }
        match self.confidence {
            Some(c) => c >= CONFIDENT && self.in_band,
            // Left alone because it already met its rule.
            None => true,
        }
    }

    /// Where the line sits between the two highest peaks, 0 at the lower
    /// and 1 at the upper.
    fn between_peaks(&self) -> Option<f64> {
        let (lo, hi) = self.shape?.two_peaks()?;
        let line = self.line?;
        (hi > lo).then(|| (line - lo) / (hi - lo))
    }

    /// Where the line sits against the spread: IQRs from the median.
    fn against_spread(&self) -> Option<f64> {
        let shape = self.shape?;
        let iqr = shape.iqr();
        let line = self.line?;
        (iqr > 0.0).then(|| (line - shape.median()) / iqr)
    }

    fn measure(&self, measure: Measure) -> Option<f64> {
        match measure {
            Measure::BetweenPeaks => self.between_peaks(),
            Measure::AgainstSpread => self.against_spread(),
            // On the log-odds scale: what a gate lets through differs by
            // proportion - 12% against 20% is as far as 1.2% against 2% -
            // not by percentage points.
            Measure::FractionBeyond => self.fraction_beyond().map(logit),
            Measure::Shift | Measure::Spread => None,
        }
    }

    /// The fraction of the population beyond the line, on the gate's side.
    fn fraction_beyond(&self) -> Option<f64> {
        if let Some(b) = self.beyond {
            return Some(b);
        }
        let below = self.shape?.fraction_below(self.line?);
        Some(match self.bound? {
            Bound::Above => 1.0 - below,
            Bound::Below => below,
        })
    }
}

fn from_placed(p: &super::run_record::PlacedRecord) -> Item<'_> {
    Item {
        gate_id: &p.gate_id,
        gate: &p.gate,
        parent_gate: p.parent_gate.as_deref(),
        sample: &p.sample,
        moved: true,
        confidence: Some(p.confidence),
        weakest: p.weakest.as_deref(),
        in_band: p.in_band,
        reference: false,
        line: p.to,
        beyond: p.above_the_line,
        shape: p.shape.as_ref(),
        bound: p.bound,
        placed: Some(p),
    }
}

fn from_kept(k: &super::run_record::KeptRecord) -> Item<'_> {
    Item {
        gate_id: &k.gate_id,
        gate: &k.gate,
        parent_gate: k.parent_gate.as_deref(),
        sample: &k.sample,
        moved: false,
        confidence: None,
        weakest: None,
        in_band: true,
        reference: !k.met_rule,
        line: k.line,
        beyond: k.above_the_line,
        shape: k.shape.as_ref(),
        bound: k.bound,
        placed: None,
    }
}

/// The comparisons, each with the least spread among peers it is judged
/// against - so a run of near-identical peers does not make a hair's
/// difference look enormous.
#[derive(Clone, Copy)]
enum Measure {
    BetweenPeaks,
    AgainstSpread,
    FractionBeyond,
    Shift,
    Spread,
}

impl Measure {
    fn key(self) -> &'static str {
        match self {
            Measure::BetweenPeaks => "gate_between_peaks",
            Measure::AgainstSpread => "gate_against_spread",
            Measure::FractionBeyond => "fraction_beyond_line",
            Measure::Shift => "distribution_shift",
            Measure::Spread => "distribution_spread",
        }
    }

    /// The smallest scale a peer spread is taken to have.
    fn floor(self) -> f64 {
        match self {
            Measure::BetweenPeaks => 0.04,
            Measure::AgainstSpread => 0.08,
            // In log-odds: about a sixth either way, relative.
            Measure::FractionBeyond => 0.15,
            Measure::Shift => 0.08,
            Measure::Spread => 0.06,
        }
    }

    /// How much an unusual value here counts: the gate's position is what is
    /// being judged; what it lets through, and the distribution, can differ
    /// for real reasons and explain rather than accuse.
    fn weight(self) -> f64 {
        match self {
            Measure::BetweenPeaks | Measure::AgainstSpread => 1.0,
            Measure::FractionBeyond => 0.75,
            Measure::Shift => 0.6,
            Measure::Spread => 0.5,
        }
    }
}

fn logit(p: f64) -> f64 {
    let p = p.clamp(1e-4, 1.0 - 1e-4);
    (p / (1.0 - p)).ln()
}

fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    Some(if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    })
}

/// How far `x` is from `peers`, in robust standard deviations: the median
/// and 1.4826 times the median absolute deviation, no smaller than `floor`.
fn robust_z(x: f64, peers: &[f64], floor: f64) -> Option<(f64, f64)> {
    let mut v = peers.to_vec();
    let centre = median(&mut v)?;
    let mut dev: Vec<f64> = peers.iter().map(|p| (p - centre).abs()).collect();
    let mad = median(&mut dev)? * 1.4826;
    Some(((x - centre) / mad.max(floor), centre))
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// Assess the run: what each placement looks like against its peers. With
/// the gates as they stand, each flag says whether its gate has been moved
/// since.
pub fn assess(run: &RunRecord, now: Option<(&GateState, &MetaDataFileMap)>) -> Assessment {
    let items: Vec<Item> = run
        .placed
        .iter()
        .map(from_placed)
        .chain(run.kept.iter().map(from_kept))
        .collect();

    // Peers: the same gate, the same kind of sample.
    let mut groups: BTreeMap<(&str, Option<&str>), Vec<usize>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        groups
            .entry((item.gate_id, item.sample.sample_type.as_deref()))
            .or_default()
            .push(i);
    }

    let mut flags = Vec::new();
    for members in groups.values() {
        for &i in members {
            let item = &items[i];
            if item.reference {
                // A reference is what the others are calibrated from, not a
                // placement to judge.
                continue;
            }
            let others: Vec<&Item> = members
                .iter()
                .filter(|&&j| j != i)
                .map(|&j| &items[j])
                .filter(|o| !o.reference)
                .collect();
            let confident: Vec<&Item> = others.iter().copied().filter(|o| o.confident()).collect();
            let (peers, confident_peers) = if confident.len() >= MIN_PEERS {
                (confident, true)
            } else {
                (others, false)
            };

            let mut reasons = Vec::new();
            if let Some(c) = item.confidence
                && c < REVIEW_FLOOR
            {
                reasons.push(Reason {
                    measure: "low_confidence",
                    says: format!(
                        "the rule's own confidence is {c:.2}, held down by {}",
                        item.weakest.unwrap_or("-")
                    ),
                    severity: FLAG + (REVIEW_FLOOR - c) / REVIEW_FLOOR,
                });
            }
            if item.moved && !item.in_band {
                reasons.push(Reason {
                    measure: "outside_band",
                    says: "the rule could not bring the gate inside the band it asks for".into(),
                    severity: FLAG,
                });
            }

            if peers.len() >= MIN_PEERS {
                let two_peaks = item.between_peaks();
                let measures: [(Measure, Option<f64>); 3] = [
                    (Measure::BetweenPeaks, two_peaks),
                    (
                        Measure::AgainstSpread,
                        // With two peaks the position between them says it.
                        if two_peaks.is_none() {
                            item.against_spread()
                        } else {
                            None
                        },
                    ),
                    (
                        Measure::FractionBeyond,
                        item.measure(Measure::FractionBeyond),
                    ),
                ];
                for (measure, here) in measures {
                    let Some(here) = here else { continue };
                    let theirs: Vec<f64> =
                        peers.iter().filter_map(|p| p.measure(measure)).collect();
                    if theirs.len() < MIN_PEERS {
                        continue;
                    }
                    let Some((z, typical)) = robust_z(here, &theirs, measure.floor()) else {
                        continue;
                    };
                    let severity = z.abs() * measure.weight();
                    if severity < NOTABLE {
                        continue;
                    }
                    let says = match measure {
                        Measure::BetweenPeaks => format!(
                            "the gate sits {:.0}% of the way from the negative peak to the positive, where its peers put it at {:.0}%",
                            here * 100.0,
                            typical * 100.0
                        ),
                        Measure::AgainstSpread => format!(
                            "the gate sits {here:.2} IQRs from the median, where its peers put it at {typical:.2}"
                        ),
                        _ => format!(
                            "{} of the population is beyond the line, against {} typically",
                            pct(logistic(here)),
                            pct(logistic(typical))
                        ),
                    };
                    reasons.push(Reason {
                        measure: measure.key(),
                        says,
                        severity,
                    });
                }

                // The distribution itself: what usually explains the rest.
                if let Some(shape) = item.shape {
                    let shapes: Vec<&Shape> = peers.iter().filter_map(|p| p.shape).collect();
                    if shapes.len() >= MIN_PEERS {
                        let mut iqrs: Vec<f64> = shapes.iter().map(|s| s.iqr()).collect();
                        let typical_iqr = median(&mut iqrs).unwrap_or(0.0);
                        if typical_iqr > 0.0 {
                            let medians: Vec<f64> =
                                shapes.iter().map(|s| s.median() / typical_iqr).collect();
                            if let Some((z, typical)) = robust_z(
                                shape.median() / typical_iqr,
                                &medians,
                                Measure::Shift.floor(),
                            ) {
                                let severity = z.abs() * Measure::Shift.weight();
                                if severity >= NOTABLE {
                                    reasons.push(Reason {
                                        measure: Measure::Shift.key(),
                                        says: format!(
                                            "the population's median is {:+.2} IQRs from its peers'",
                                            shape.median() / typical_iqr - typical
                                        ),
                                        severity,
                                    });
                                }
                            }
                            let spreads: Vec<f64> =
                                shapes.iter().map(|s| (s.iqr().max(1e-12)).ln()).collect();
                            if let Some((z, typical)) = robust_z(
                                shape.iqr().max(1e-12).ln(),
                                &spreads,
                                Measure::Spread.floor(),
                            ) {
                                let severity = z.abs() * Measure::Spread.weight();
                                if severity >= NOTABLE {
                                    reasons.push(Reason {
                                        measure: Measure::Spread.key(),
                                        says: format!(
                                            "the population is {:.1}x as spread as its peers'",
                                            (shape.iqr().max(1e-12).ln() - typical).exp()
                                        ),
                                        severity,
                                    });
                                }
                            }
                        }
                        // One peak where the peers have two, or the reverse.
                        let has_two = |s: &Shape| s.peaks.len() >= 2;
                        let peers_two = shapes.iter().filter(|s| has_two(s)).count();
                        let majority_two = peers_two * 4 >= shapes.len() * 3;
                        let majority_one = peers_two * 4 <= shapes.len();
                        if (majority_two && !has_two(shape)) || (majority_one && has_two(shape)) {
                            reasons.push(Reason {
                                measure: "peaks",
                                says: format!(
                                    "the population has {} where {} of its {} peers have {}",
                                    if has_two(shape) {
                                        "two peaks"
                                    } else {
                                        "one peak"
                                    },
                                    if majority_two {
                                        peers_two
                                    } else {
                                        shapes.len() - peers_two
                                    },
                                    shapes.len(),
                                    if majority_two { "two" } else { "one" }
                                ),
                                severity: NOTABLE + 0.5,
                            });
                        }
                    }
                }
            }

            let severity = reasons.iter().map(|r| r.severity).fold(0.0, f64::max);
            if severity < FLAG {
                continue;
            }
            reasons.sort_by(|a, b| b.severity.total_cmp(&a.severity));
            flags.push(Flag {
                gate_id: item.gate_id.to_string(),
                gate: item.gate.to_string(),
                parent_gate: item.parent_gate.map(str::to_string),
                sample: item.sample.clone(),
                moved: item.moved,
                confidence: item.confidence,
                weakest: item.weakest.map(str::to_string),
                severity,
                reasons,
                status: match (now, item.placed) {
                    (Some((state, metadata)), Some(p)) => {
                        Some(placement_status(p, state, metadata))
                    }
                    _ => None,
                },
                peers: peers.len(),
                confident_peers,
            });
        }
    }
    // Worst first; a gate already moved by a person after those still as
    // placed.
    flags.sort_by(|a, b| {
        let dealt = |f: &Flag| matches!(f.status, Some(PlacementStatus::Moved));
        dealt(a)
            .cmp(&dealt(b))
            .then(b.severity.total_cmp(&a.severity))
    });

    // Each gate in a line.
    let mut gates: BTreeMap<&str, GateSummary> = BTreeMap::new();
    for ((gate_id, sample_type), members) in &groups {
        let first = &items[members[0]];
        let summary = gates.entry(gate_id).or_insert_with(|| GateSummary {
            gate_id: gate_id.to_string(),
            gate: first.gate.to_string(),
            parent_gate: first.parent_gate.map(str::to_string),
            by_type: Vec::new(),
            median_confidence: None,
        });
        let judged: Vec<&Item> = members
            .iter()
            .map(|&i| &items[i])
            .filter(|i| !i.reference)
            .collect();
        let confident: Vec<&Item> = judged.iter().copied().filter(|i| i.confident()).collect();
        let mut positions: Vec<f64> = confident.iter().filter_map(|i| i.between_peaks()).collect();
        let mut beyond: Vec<f64> = confident
            .iter()
            .filter_map(|i| i.fraction_beyond())
            .collect();
        summary.by_type.push(TypeSummary {
            sample_type: sample_type.map(str::to_string),
            samples: judged.len(),
            confident: confident.len(),
            flagged: flags
                .iter()
                .filter(|f| {
                    f.gate_id == *gate_id && f.sample.sample_type.as_deref() == *sample_type
                })
                .count(),
            typical_position_between_peaks: median(&mut positions),
            typical_fraction_beyond: median(&mut beyond),
        });
    }
    for summary in gates.values_mut() {
        let mut c: Vec<f64> = run
            .placed
            .iter()
            .filter(|p| p.gate_id == summary.gate_id)
            .map(|p| p.confidence)
            .collect();
        summary.median_confidence = median(&mut c);
    }

    Assessment {
        run_applied_at: run.applied_at.clone(),
        placements: items.iter().filter(|i| !i.reference).count(),
        flags,
        gates: gates.into_values().collect(),
    }
}

// ── one sample against its peers ─────────────────────────────────────────

/// The percentiles a comparison is given in.
pub const COMPARED_PERCENTILES: [u32; 9] = [1, 5, 10, 25, 50, 75, 90, 95, 99];

/// The spread of the peers' values at one point.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Band {
    pub p10: f64,
    pub median: f64,
    pub p90: f64,
}

fn band(values: &[f64]) -> Option<Band> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    Some(Band {
        p10: at(0.1),
        median: at(0.5),
        p90: at(0.9),
    })
}

/// A sample's placement of one gate beside its peers', in numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PeerComparison {
    pub gate: String,
    pub parent_gate: Option<String>,
    pub sample: SampleRef,
    pub peers: Vec<String>,
    pub confident_peers: bool,
    /// Which side of the line the gate keeps.
    pub keeps: Option<&'static str>,
    pub events: Option<usize>,
    pub peer_events: Option<Band>,
    /// The sample's percentiles on the rule's parameter, and the peers'
    /// spread at each.
    pub percentiles: Vec<(u32, Option<f64>, Option<Band>)>,
    /// The sample's peaks, and the peers' negative and positive peaks.
    pub peaks: Vec<super::shape::Peak>,
    pub peer_negative_peak: Option<Band>,
    pub peer_positive_peak: Option<Band>,
    /// Where the line is, and where the peers' lines are.
    pub line: Option<f64>,
    pub peer_lines: Option<Band>,
    pub position_between_peaks: Option<f64>,
    pub peer_positions_between_peaks: Option<Band>,
    pub fraction_beyond: Option<f64>,
    pub peer_fractions_beyond: Option<Band>,
    pub confidence: Option<f64>,
    pub weakest: Option<String>,
    /// Why it was flagged, if it was.
    pub reasons: Vec<Reason>,
}

/// Compare `sample`'s placement of `gate_id` with its peers', from the run.
pub fn compare_to_peers(
    run: &RunRecord,
    gate_id: &str,
    sample: &str,
) -> Result<PeerComparison, String> {
    let assessment = assess(run, None);
    let mine = run
        .placed
        .iter()
        .find(|p| p.gate_id == gate_id && p.sample.id == sample)
        .map(|p| {
            (
                p.sample.clone(),
                p.to,
                p.shape.clone(),
                p.bound,
                Some(p.confidence),
                p.weakest.clone(),
                p.above_the_line,
                p.gate.clone(),
                p.parent_gate.clone(),
            )
        })
        .or_else(|| {
            run.kept
                .iter()
                .find(|k| k.gate_id == gate_id && k.sample.id == sample)
                .map(|k| {
                    (
                        k.sample.clone(),
                        k.line,
                        k.shape.clone(),
                        k.bound,
                        None,
                        None,
                        k.above_the_line,
                        k.gate.clone(),
                        k.parent_gate.clone(),
                    )
                })
        })
        .ok_or("the last applied run did not place or keep this gate on this sample")?;
    let (me, line, shape, bound, confidence, weakest, beyond, gate, parent_gate) = mine;

    // The same items the assessment builds, for this gate and this kind.
    let others: Vec<Item> = run
        .placed
        .iter()
        .map(from_placed)
        .chain(run.kept.iter().map(from_kept))
        .filter(|i| {
            i.gate_id == gate_id
                && i.sample.id != sample
                && i.sample.sample_type == me.sample_type
                && !i.reference
        })
        .collect();
    let confident: Vec<&Item> = others.iter().filter(|o| o.confident()).collect();
    let (peers, confident_peers): (Vec<&Item>, bool) = if confident.len() >= MIN_PEERS {
        (confident, true)
    } else {
        (others.iter().collect(), false)
    };
    let collect = |f: &dyn Fn(&Item) -> Option<f64>| -> Option<Band> {
        band(&peers.iter().filter_map(|p| f(p)).collect::<Vec<_>>())
    };
    let here = Item {
        gate_id,
        gate: &gate,
        parent_gate: parent_gate.as_deref(),
        sample: &me,
        moved: confidence.is_some(),
        confidence,
        weakest: weakest.as_deref(),
        in_band: true,
        reference: false,
        line,
        beyond,
        shape: shape.as_ref(),
        bound,
        placed: None,
    };

    Ok(PeerComparison {
        gate: gate.clone(),
        parent_gate: parent_gate.clone(),
        sample: me.clone(),
        peers: peers
            .iter()
            .map(|p| p.sample.name.clone().unwrap_or_else(|| p.sample.id.clone()))
            .collect(),
        confident_peers,
        keeps: bound.map(|b| match b {
            Bound::Above => "above the line",
            Bound::Below => "below the line",
        }),
        events: shape.as_ref().map(|s| s.events),
        peer_events: collect(&|p| p.shape.map(|s| s.events as f64)),
        percentiles: COMPARED_PERCENTILES
            .iter()
            .map(|&q| {
                (
                    q,
                    shape.as_ref().map(|s| s.percentiles[q as usize]),
                    collect(&|p| p.shape.map(|s| s.percentiles[q as usize])),
                )
            })
            .collect(),
        peaks: shape.as_ref().map(|s| s.peaks.clone()).unwrap_or_default(),
        peer_negative_peak: collect(&|p| p.shape.and_then(|s| s.two_peaks()).map(|t| t.0)),
        peer_positive_peak: collect(&|p| p.shape.and_then(|s| s.two_peaks()).map(|t| t.1)),
        line,
        peer_lines: collect(&|p| p.line),
        position_between_peaks: here.between_peaks(),
        peer_positions_between_peaks: collect(&|p| p.between_peaks()),
        fraction_beyond: here.fraction_beyond(),
        peer_fractions_beyond: collect(&|p| p.fraction_beyond()),
        confidence,
        weakest,
        reasons: assessment
            .flags
            .into_iter()
            .find(|f| f.gate_id == gate_id && f.sample.id == sample)
            .map(|f| f.reasons)
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::run_record::{ComponentRecord, PlacedRecord, SampleRef};
    use crate::review::shape::summarise;
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal};

    /// A negative at 0 and a positive at `pos`, one fifth positive.
    fn population(pos: f64, seed: u64) -> Shape {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let neg = Normal::new(0.0, 0.3).unwrap();
        let hi = Normal::new(pos, 0.4).unwrap();
        let values: Vec<f64> = (0..5_000)
            .map(|i| {
                if i % 5 == 0 {
                    hi.sample(&mut rng)
                } else {
                    neg.sample(&mut rng)
                }
            })
            .collect();
        summarise(&values).unwrap()
    }

    fn placed(n: usize, line: f64, shape: Shape, confidence: f64) -> PlacedRecord {
        PlacedRecord {
            gate_id: "g".into(),
            gate: "CD279+".into(),
            parent_gate: Some("CD4+".into()),
            specimen_column: "SampleID".into(),
            specimen: format!("s{n}"),
            sample: SampleRef {
                id: format!("f{n}"),
                name: Some(format!("s{n}_FS.fcs")),
                sample_type: Some("FS".into()),
            },
            measured_on: SampleRef {
                id: format!("r{n}"),
                name: None,
                sample_type: Some("FMX".into()),
            },
            from: Some(1.0),
            to: Some(line),
            confidence,
            weakest: Some("rule satisfied".into()),
            components: vec![ComponentRecord {
                name: "rule satisfied".into(),
                score: confidence,
                detail: String::new(),
            }],
            achieved: None,
            above_the_line: None,
            reference_events: 1000,
            in_band: true,
            negative: None,
            valley: None,
            phenotype: None,
            placed_at: Vec::new(),
            shape: Some(shape),
            bound: Some(Bound::Above),
        }
    }

    /// Ten samples gated in the valley at 1.5 between peaks at 0 and 3.
    fn run_with(odd_one: Option<PlacedRecord>) -> RunRecord {
        let mut placed: Vec<PlacedRecord> = (0..10)
            .map(|n| {
                placed(
                    n,
                    1.5 + (n as f64 - 5.0) * 0.02,
                    population(3.0, n as u64),
                    0.8,
                )
            })
            .collect();
        placed.extend(odd_one);
        RunRecord {
            format: 1,
            applied_at: "t".into(),
            rules: Default::default(),
            placed,
            kept: Vec::new(),
            skipped: Vec::new(),
        }
    }

    #[test]
    fn a_run_placed_alike_raises_nothing() {
        let a = assess(&run_with(None), None);
        assert!(a.flags.is_empty(), "{:#?}", a.flags);
        assert_eq!(a.placements, 10);
        let summary = &a.gates[0].by_type[0];
        assert_eq!(
            (summary.samples, summary.confident, summary.flagged),
            (10, 10, 0)
        );
        let typical = summary.typical_position_between_peaks.unwrap();
        assert!((typical - 0.5).abs() < 0.1, "{summary:?}");
    }

    #[test]
    fn a_gate_in_the_positive_is_flagged_though_the_rule_was_sure() {
        // Confident, in its band - and in the middle of the positive peak.
        let odd = placed(99, 2.9, population(3.0, 99), 0.9);
        let a = assess(&run_with(Some(odd)), None);
        assert_eq!(a.flags.len(), 1, "{:#?}", a.flags);
        let flag = &a.flags[0];
        assert_eq!(flag.sample.id, "f99");
        assert_eq!(flag.reasons[0].measure, "gate_between_peaks", "{flag:#?}");
        assert!(
            flag.reasons[0]
                .says
                .contains("of the way from the negative")
        );
        assert!(flag.confident_peers && flag.peers == 10);
    }

    #[test]
    fn a_shifted_sample_gated_consistently_is_not_blamed_for_the_shift() {
        // Its positive has moved to 4, and the rule followed it into the
        // valley: the gate is where it should be.
        let odd = placed(99, 2.0, population(4.0, 99), 0.9);
        let a = assess(&run_with(Some(odd)), None);
        let gate_reasons: Vec<&str> = a
            .flags
            .iter()
            .flat_map(|f| f.reasons.iter().map(|r| r.measure))
            .filter(|m| *m == "gate_between_peaks")
            .collect();
        assert!(gate_reasons.is_empty(), "{:#?}", a.flags);
    }

    #[test]
    fn low_confidence_is_flagged_whatever_the_peers_say() {
        let odd = placed(99, 1.5, population(3.0, 99), 0.1);
        let a = assess(&run_with(Some(odd)), None);
        assert_eq!(a.flags.len(), 1);
        assert_eq!(a.flags[0].reasons[0].measure, "low_confidence");
    }

    #[test]
    fn too_few_samples_to_compare_flags_only_what_the_rule_said() {
        let mut run = run_with(None);
        run.placed.truncate(2);
        run.placed[0].to = Some(2.9);
        assert!(assess(&run, None).flags.is_empty());
    }

    #[test]
    fn a_sample_is_shown_against_its_peers_in_numbers() {
        let odd = placed(99, 2.9, population(3.0, 99), 0.9);
        let run = run_with(Some(odd));
        let c = compare_to_peers(&run, "g", "f99").unwrap();
        assert_eq!(c.peers.len(), 10);
        assert!(c.confident_peers);
        assert_eq!(c.percentiles.len(), 9);
        let lines = c.peer_lines.unwrap();
        assert!((lines.median - 1.5).abs() < 0.1);
        assert!(c.position_between_peaks.unwrap() > 0.8);
        assert!(!c.reasons.is_empty());
        assert!(compare_to_peers(&run, "g", "nobody").is_err());
    }
}
