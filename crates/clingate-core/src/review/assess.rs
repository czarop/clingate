//! Assessing a rules run: which placements need a person to look at them.
//!
//! A placement is flagged when:
//!
//! - **its gate sits [`POSITION_LIMIT`] or more of its parent's interquartile
//!   ranges from where its peers put theirs.** Each gate's position is read
//!   in its own parent's IQRs from that parent's median, and compared with
//!   the median of its **peers** - the other samples of the same kind (the
//!   sample type the pairing names) whose placement the rule was confident
//!   in. A plain distance rather than a score against how much the peers
//!   vary: percent positive, spread and shape vary between donors in data a
//!   rule is right on, and peers that agree closely made a small difference
//!   look enormous;
//! - the rule's own confidence is below [`REVIEW_FLOOR`] - unless it read a
//!   control big enough to trust and reached its band;
//! - or it could not reach its band.
//!
//! Each is said in words, with the numbers, so a person - or Claude - can
//! see why it was flagged.
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
/// A gate this many of its parent's IQRs from where its peers put theirs, or
/// more, is flagged.
pub const POSITION_LIMIT: f64 = 2.0;
/// A reason this severe, or more, flags its placement.
pub const FLAG: f64 = 3.0;

/// One thing unusual about a placement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reason {
    /// What was found: `gate_position`, `low_confidence` or `outside_band`.
    pub measure: &'static str,
    /// In words, with the numbers.
    pub says: String,
    /// How unusual, on the same scale for every measure: [`FLAG`] and over
    /// is worth a look.
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
    /// The peer that placed this gate most typically - the one closest to
    /// the peers' median on where the gate sits - to show beside it.
    pub typical_peer: Option<SampleRef>,
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
    /// The placements the run could not make - see [`unplaced`].
    pub unplaced: Vec<Unplaced>,
}

/// A gate the run could not place, for one reason, and on which samples.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unplaced {
    pub gate: String,
    pub parent_gate: Option<String>,
    /// As the run said it.
    pub reason: String,
    /// The samples it was refused on; none for a problem with the rule
    /// itself, said before any sample was read.
    pub samples: Vec<SampleRef>,
    /// Refused on every sample the run reached it on, for this one reason:
    /// the rule's settings or the gates as drawn, not the data.
    pub everywhere: bool,
    /// What checking the rule against the gates as drawn finds wrong with it.
    pub rule_problem: Option<String>,
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
    /// Read on a control big enough to trust, and in its band: not flagged
    /// for its confidence.
    trusted: bool,
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

    /// The fraction of this sample's population beyond the line, on the
    /// gate's side.
    ///
    /// Read off the sample's own distribution. The run's `above_the_line` is
    /// counted on whichever population the rule is judged on - for a rule
    /// that reads the FMO, the FMO's - so comparing samples by it compared
    /// references, and a flag quoted the FMO's fraction as the sample's. It
    /// is the fallback only where no distribution was kept.
    fn fraction_beyond(&self) -> Option<f64> {
        let from_shape = || {
            let below = self.shape?.fraction_below(self.line?);
            Some(match self.bound? {
                Bound::Above => 1.0 - below,
                Bound::Below => below,
            })
        };
        from_shape().or(self.beyond)
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
        trusted: crate::gate_rules::autogate::trusted_control(
            p.read_on_control,
            p.reference_events,
            p.in_band,
        ),
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
        trusted: false,
        line: k.line,
        beyond: k.above_the_line,
        shape: k.shape.as_ref(),
        bound: k.bound,
        placed: None,
    }
}

/// The peer to show beside a flagged placement: of its peers that are not
/// flagged themselves, the one whose gate sits nearest the middle of theirs.
///
/// Never another flagged placement - an outlier beside an outlier shows
/// nothing - and so, for placements compared with the same peers, the same
/// sample each time.
fn typical_of(
    gate_id: &str,
    peers: &[(SampleRef, Option<f64>)],
    flagged: &std::collections::HashSet<(String, String)>,
) -> Option<SampleRef> {
    let unflagged: Vec<&(SampleRef, Option<f64>)> = peers
        .iter()
        .filter(|(s, _)| !flagged.contains(&(gate_id.to_string(), s.id.clone())))
        .collect();
    let valued: Vec<(f64, &SampleRef)> = unflagged
        .iter()
        .filter_map(|(s, v)| v.map(|v| (v, s)))
        .collect();
    let mut values: Vec<f64> = valued.iter().map(|(v, _)| *v).collect();
    median(&mut values)
        .and_then(|m| {
            valued
                .iter()
                .min_by(|a, b| {
                    (a.0 - m)
                        .abs()
                        .total_cmp(&(b.0 - m).abs())
                        .then_with(|| a.1.id.cmp(&b.1.id))
                })
                .map(|(_, s)| (*s).clone())
        })
        .or_else(|| unflagged.first().map(|(s, _)| s.clone()))
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

/// Where `item`'s gate sits against where `peers` put theirs, said if it is
/// [`POSITION_LIMIT`] or more of its parent's IQRs away.
fn position_against_peers(item: &Item, peers: &[&Item]) -> Option<Reason> {
    let here = item.against_spread()?;
    let mut theirs: Vec<f64> = peers.iter().filter_map(|p| p.against_spread()).collect();
    if theirs.len() < MIN_PEERS {
        return None;
    }
    let typical = median(&mut theirs)?;
    let off = here - typical;
    (off.abs() >= POSITION_LIMIT).then(|| Reason {
        measure: "gate_position",
        says: format!(
            "the gate sits {here:.2} IQRs from the median, where its peers put it at \
             {typical:.2}: {off:+.2} IQRs from them"
        ),
        severity: FLAG * off.abs() / POSITION_LIMIT,
    })
}

/// Assess the run: what each placement looks like against its peers. With
/// the gates as they stand, each flag says whether its gate has been moved
/// since.
impl Unplaced {
    /// In a line, for a person: a gate refused everywhere says that its rule
    /// needs changing, and how where the check of the rule can tell.
    pub fn says(&self) -> String {
        let gate = crate::gate_rules::autogate::describe(&self.gate, self.parent_gate.as_deref());
        let rule = self
            .rule_problem
            .as_ref()
            .map(|problem| format!(" The rule: {problem}."))
            .unwrap_or_default();
        if self.samples.is_empty() {
            return format!("{gate}: {}.{rule}", self.reason);
        }
        if self.everywhere {
            return format!(
                "{gate} was not placed on any sample, each time because {}. That is its rule or \
                 the gates as drawn, not the data: change the rule.{rule}",
                self.reason
            );
        }
        format!(
            "{gate} on {}: {}.{rule}",
            samples_named(&self.samples),
            self.reason
        )
    }
}

/// How many samples, and the first few by name.
fn samples_named(samples: &[SampleRef]) -> String {
    const NAMED: usize = 3;
    let names: Vec<&str> = samples
        .iter()
        .take(NAMED)
        .map(|s| s.name.as_deref().unwrap_or(&s.id))
        .collect();
    let more = samples.len().saturating_sub(NAMED);
    match (samples.len(), more) {
        (1, _) => format!("1 sample ({})", names[0]),
        (n, 0) => format!("{n} samples ({})", names.join(", ")),
        (n, more) => format!("{n} samples ({}, and {more} more)", names.join(", ")),
    }
}

/// The placements `run` could not make, a gate and a reason at a time: the
/// gates refused everywhere first, then the most refused. Read against the
/// gates as they are `now`, where given, for what is wrong with the rule.
pub fn unplaced(run: &RunRecord, now: Option<&GateState>) -> Vec<Unplaced> {
    let rule_problems = now
        .map(|state| crate::gate_rules::autogate::edges_over_their_anchors(state, &run.rules))
        .unwrap_or_default();
    let mut found: Vec<Unplaced> = Vec::new();
    for skipped in &run.skipped {
        let same = |u: &&mut Unplaced| {
            u.gate == skipped.gate
                && u.parent_gate == skipped.parent_gate
                && u.reason == skipped.reason
        };
        let at = match found.iter_mut().position(|u| same(&u)) {
            Some(at) => at,
            None => {
                found.push(Unplaced {
                    gate: skipped.gate.clone(),
                    parent_gate: skipped.parent_gate.clone(),
                    reason: skipped.reason.clone(),
                    samples: Vec::new(),
                    everywhere: false,
                    rule_problem: rule_problems
                        .iter()
                        .find(|p| {
                            *p.target.gate == *skipped.gate
                                && p.target.parent.as_deref().is_none_or(|parent| {
                                    Some(parent) == skipped.parent_gate.as_deref()
                                })
                        })
                        .map(|p| p.reason.clone()),
                });
                found.len() - 1
            }
        };
        if !skipped.sample.id.is_empty() {
            found[at].samples.push(skipped.sample.clone());
        }
    }
    for u in &mut found {
        let of_gate =
            |gate: &str, parent: &Option<String>| gate == u.gate && *parent == u.parent_gate;
        let placed_somewhere = run.placed.iter().any(|p| of_gate(&p.gate, &p.parent_gate))
            || run
                .kept
                .iter()
                .any(|k| k.met_rule && of_gate(&k.gate, &k.parent_gate));
        let refused_otherwise = run.skipped.iter().any(|s| {
            of_gate(&s.gate, &s.parent_gate) && s.reason != u.reason && !s.sample.id.is_empty()
        });
        u.everywhere = !placed_somewhere && !refused_otherwise;
    }
    found.sort_by_key(|u| (!u.everywhere, std::cmp::Reverse(u.samples.len())));
    found
}

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
    // Each flag's peers, with where each put the gate - to choose the one to
    // show beside it once it is known which peers are flagged themselves.
    let mut candidates: Vec<Vec<(SampleRef, Option<f64>)>> = Vec::new();
    for members in groups.values() {
        for &i in members {
            let item = &items[i];
            if item.reference {
                // What the others are calibrated from, not a placement to
                // judge.
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
            // A trusted placement's confidence is held down by its control's
            // count, not by anything wrong with where it went.
            if let Some(c) = item.confidence
                && c < REVIEW_FLOOR
                && !item.trusted
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

            reasons.extend(position_against_peers(item, &peers));

            let severity = reasons.iter().map(|r| r.severity).fold(0.0, f64::max);
            if severity < FLAG {
                continue;
            }
            reasons.sort_by(|a, b| b.severity.total_cmp(&a.severity));
            candidates.push(
                peers
                    .iter()
                    .map(|p| (p.sample.clone(), p.against_spread()))
                    .collect::<Vec<_>>(),
            );
            flags.push(Flag {
                // Chosen once every flag is known: see `typical_of`.
                typical_peer: None,
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
    let flagged: std::collections::HashSet<(String, String)> = flags
        .iter()
        .map(|f| (f.gate_id.clone(), f.sample.id.clone()))
        .collect();
    for (flag, peers) in flags.iter_mut().zip(&candidates) {
        flag.typical_peer = typical_of(&flag.gate_id, peers, &flagged);
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
        unplaced: unplaced(run, now.map(|(state, _)| state)),
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
        trusted: false,
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
    use crate::review::run_record::{ComponentRecord, KeptRecord, PlacedRecord, SampleRef};
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
            read_on_control: false,
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

    fn one_peak(centre: f64, sd: f64, seed: u64) -> Shape {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let d = Normal::new(centre, sd).unwrap();
        summarise(&(0..5_000).map(|_| d.sample(&mut rng)).collect::<Vec<_>>()).unwrap()
    }

    fn with_type(mut p: PlacedRecord, sample_type: &str) -> PlacedRecord {
        p.sample.sample_type = Some(sample_type.into());
        p
    }

    fn run_of(placed: Vec<PlacedRecord>, kept: Vec<KeptRecord>) -> RunRecord {
        RunRecord {
            format: 1,
            applied_at: "t".into(),
            rules: Default::default(),
            placed,
            kept,
            skipped: Vec::new(),
        }
    }

    fn kept_at(n: usize, line: f64, shape: Shape, met_rule: bool) -> KeptRecord {
        KeptRecord {
            gate_id: "g".into(),
            gate: "CD279+".into(),
            parent_gate: Some("CD4+".into()),
            specimen: format!("k{n}"),
            sample: SampleRef {
                id: format!("k{n}"),
                name: Some(format!("k{n}_FS.fcs")),
                sample_type: Some("FS".into()),
            },
            met_rule,
            achieved: None,
            above_the_line: None,
            line: Some(line),
            shape: Some(shape),
            bound: Some(Bound::Above),
            measured_on: None,
        }
    }

    fn ten_alike() -> Vec<PlacedRecord> {
        (0..10)
            .map(|n| {
                placed(
                    n,
                    1.5 + (n as f64 - 5.0) * 0.02,
                    population(3.0, n as u64),
                    0.8,
                )
            })
            .collect()
    }

    #[test]
    fn what_a_gate_lets_through_is_read_off_the_sample_s_own_population() {
        let mut p = placed(0, 1.5, population(3.0, 1), 0.8);
        // The run's figure, counted on the FMO the rule read.
        p.above_the_line = Some(0.9);
        let item = from_placed(&p);
        let own = item.fraction_beyond().unwrap();
        assert!(
            (own - 0.2).abs() < 0.03,
            "a fifth of this sample is positive: {own}"
        );
        // The other side of the line keeps the rest.
        p.bound = Some(Bound::Below);
        let below = from_placed(&p).fraction_beyond().unwrap();
        assert!((own + below - 1.0).abs() < 1e-9);
        // With no distribution kept, the run's figure is all there is.
        p.shape = None;
        assert_eq!(from_placed(&p).fraction_beyond(), Some(0.9));
    }

    #[test]
    fn samples_are_compared_only_with_samples_of_their_own_kind() {
        // The FS gated in the valley, the FMX in the positive: each
        // consistent with its own kind.
        let mut placed_all = ten_alike();
        placed_all.extend(
            (10..20).map(|n| with_type(placed(n, 2.9, population(3.0, n as u64), 0.8), "FMX")),
        );
        let a = assess(&run_of(placed_all, Vec::new()), None);
        assert!(a.flags.is_empty(), "{:#?}", a.flags);
        assert_eq!(a.gates[0].by_type.len(), 2);
    }

    #[test]
    fn a_reference_is_neither_judged_nor_a_peer() {
        // A reference with its gate in the positive, and nine alike.
        let mut placed_all = ten_alike();
        placed_all.truncate(9);
        let reference = kept_at(0, 2.9, population(3.0, 50), false);
        let a = assess(&run_of(placed_all, vec![reference]), None);
        assert!(a.flags.is_empty(), "{:#?}", a.flags);
        assert_eq!(a.placements, 9, "references are not placements");
        assert_eq!(a.gates[0].by_type[0].samples, 9);
    }

    #[test]
    fn a_gate_the_run_left_alone_is_judged_like_any_other() {
        let odd = kept_at(1, 2.9, population(3.0, 51), true);
        let a = assess(&run_of(ten_alike(), vec![odd]), None);
        assert_eq!(a.flags.len(), 1, "{:#?}", a.flags);
        let flag = &a.flags[0];
        assert_eq!(flag.sample.id, "k1");
        assert!(!flag.moved);
        assert_eq!(flag.confidence, None);
        assert_eq!(flag.reasons[0].measure, "gate_position");
    }

    #[test]
    fn with_too_few_confident_peers_every_peer_of_its_kind_is_used() {
        // Unsure of all of them, but not so unsure as to flag that alone.
        let mut placed_all: Vec<PlacedRecord> = ten_alike()
            .into_iter()
            .map(|mut p| {
                p.confidence = 0.4;
                p
            })
            .collect();
        placed_all.push(placed(99, 2.9, population(3.0, 99), 0.4));
        let a = assess(&run_of(placed_all, Vec::new()), None);
        assert_eq!(a.flags.len(), 1, "{:#?}", a.flags);
        assert!(!a.flags[0].confident_peers);
        assert_eq!(a.flags[0].peers, 10);
        assert_eq!(a.gates[0].by_type[0].confident, 0);
    }

    #[test]
    fn a_placement_outside_its_band_is_flagged_as_such() {
        let mut odd = placed(99, 1.5, population(3.0, 99), 0.9);
        odd.in_band = false;
        let a = assess(&run_with(Some(odd)), None);
        assert_eq!(a.flags.len(), 1);
        assert_eq!(a.flags[0].reasons[0].measure, "outside_band");
        // And it does not count among the confident.
        assert_eq!(a.gates[0].by_type[0].confident, 10);
    }

    #[test]
    fn the_typical_peer_is_the_one_nearest_the_peers_middle() {
        // Lines from 1.30 to 1.70 in steps: the middle one is f5.
        let mut placed_all: Vec<PlacedRecord> = (0..11)
            .map(|n| placed(n, 1.3 + n as f64 * 0.04, population(3.0, 7), 0.8))
            .collect();
        placed_all.push(placed(99, 2.9, population(3.0, 7), 0.8));
        let a = assess(&run_of(placed_all, Vec::new()), None);
        let flag = a.flags.iter().find(|f| f.sample.id == "f99").unwrap();
        assert_eq!(flag.typical_peer.as_ref().unwrap().id, "f5");
    }

    #[test]
    fn with_one_peak_the_gate_is_judged_against_the_spread() {
        let mut placed_all: Vec<PlacedRecord> = (0..10)
            .map(|n| placed(n, 1.0 + n as f64 * 0.01, one_peak(0.0, 1.0, n as u64), 0.8))
            .collect();
        placed_all.push(placed(99, 4.0, one_peak(0.0, 1.0, 99), 0.8));
        let a = assess(&run_of(placed_all, Vec::new()), None);
        let flag = a
            .flags
            .iter()
            .find(|f| f.sample.id == "f99")
            .expect("flagged");
        assert_eq!(flag.reasons[0].measure, "gate_position", "{flag:#?}");
        assert!(flag.reasons[0].says.contains("IQRs from the median"));
        assert!(a.flags.iter().all(|f| f.sample.id == "f99"));
    }

    /// A population three times as wide as its peers', or with one peak
    /// where they have two, is how donors differ: with its gate where its
    /// peers put theirs, it is not flagged.
    #[test]
    fn a_differently_shaped_population_gated_like_its_peers_is_not_flagged() {
        let peers = run_with(None);
        let wide = one_peak(1.0, 3.0, 97);
        let mut at: Vec<f64> = peers
            .placed
            .iter()
            .map(|p| {
                let shape = p.shape.as_ref().unwrap();
                (p.to.unwrap() - shape.median()) / shape.iqr()
            })
            .collect();
        at.sort_by(f64::total_cmp);
        let typical = (at[4] + at[5]) / 2.0;
        let line = wide.median() + typical * wide.iqr();
        let a = assess(&run_with(Some(placed(97, line, wide, 0.9))), None);
        assert!(a.flags.is_empty(), "{:#?}", a.flags);
    }

    /// Two of its parent's IQRs from its peers is the line: 1.9 is not
    /// flagged, 2.1 is, either side.
    #[test]
    fn a_gate_two_parent_iqrs_from_its_peers_is_flagged() {
        let shape = one_peak(0.0, 1.0, 5);
        let iqr = shape.iqr();
        let peers: Vec<PlacedRecord> = (0..10)
            .map(|n| placed(n, 1.0, shape.clone(), 0.8))
            .collect();
        let flagged = |off: f64| {
            let mut all = peers.clone();
            all.push(placed(99, 1.0 + off * iqr, shape.clone(), 0.8));
            assess(&run_of(all, Vec::new()), None).flags
        };
        assert!(flagged(1.9).is_empty());
        assert!(flagged(-1.9).is_empty());
        for off in [2.1, -2.1] {
            let flags = flagged(off);
            assert_eq!(flags.len(), 1, "{off}: {flags:#?}");
            assert_eq!(flags[0].reasons[0].measure, "gate_position");
            assert!(
                flags[0].reasons[0]
                    .says
                    .contains(&format!("{off:+.2} IQRs from them")),
                "{}",
                flags[0].reasons[0].says
            );
        }
    }

    #[test]
    fn each_gate_is_summed_up_across_the_run() {
        let odd = placed(99, 2.9, population(3.0, 99), 0.2);
        let a = assess(&run_with(Some(odd)), None);
        assert_eq!(a.gates.len(), 1);
        let gate = &a.gates[0];
        assert_eq!(
            (gate.gate.as_str(), gate.parent_gate.as_deref()),
            ("CD279+", Some("CD4+"))
        );
        assert_eq!(gate.median_confidence, Some(0.8));
        let fs = &gate.by_type[0];
        assert_eq!(fs.sample_type.as_deref(), Some("FS"));
        assert_eq!((fs.samples, fs.confident, fs.flagged), (11, 10, 1));
        assert!((fs.typical_position_between_peaks.unwrap() - 0.5).abs() < 0.1);
        let beyond = fs.typical_fraction_beyond.unwrap();
        assert!((beyond - 0.2).abs() < 0.03, "{beyond}");
    }

    #[test]
    fn a_kept_sample_is_compared_too_and_an_unflagged_one_has_no_reasons() {
        let run = run_of(
            ten_alike(),
            vec![kept_at(1, 1.5, population(3.0, 60), true)],
        );
        let c = compare_to_peers(&run, "g", "k1").unwrap();
        assert_eq!(c.confidence, None);
        assert_eq!(c.keeps, Some("above the line"));
        assert!(c.reasons.is_empty());
        assert_eq!(c.events, Some(5_000));
        let events = c.peer_events.unwrap();
        assert_eq!(
            (events.p10, events.median, events.p90),
            (5_000.0, 5_000.0, 5_000.0)
        );
        for (_, here, peers) in &c.percentiles {
            let peers = peers.as_ref().unwrap();
            assert!(here.is_some());
            assert!(peers.p10 <= peers.median && peers.median <= peers.p90);
        }
        let neg = c.peer_negative_peak.unwrap();
        let pos = c.peer_positive_peak.unwrap();
        assert!(neg.median.abs() < 0.2 && (pos.median - 3.0).abs() < 0.3);
        assert_eq!(c.peaks.len(), 2);
        assert!((c.position_between_peaks.unwrap() - 0.5).abs() < 0.1);
    }

    /// A negative at 0 and a fraction `share` positive at `at`.
    fn mixed(share: f64, at: f64, seed: u64) -> Shape {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let neg = Normal::new(0.0, 0.5).unwrap();
        let pos = Normal::new(at, 0.5).unwrap();
        let every = (1.0 / share).round() as usize;
        summarise(
            &(0..5_000)
                .map(|i| {
                    if i % every == 0 {
                        pos.sample(&mut rng)
                    } else {
                        neg.sample(&mut rng)
                    }
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    /// The CD69 plate: 31 samples with one population and the gate well
    /// out past it, and four with a second population and the gate through
    /// the middle of them.
    fn cd69_plate() -> RunRecord {
        let mut placed_all: Vec<PlacedRecord> = (0..31)
            .map(|n| {
                placed(
                    n,
                    1.6 + (n % 5) as f64 * 0.02,
                    one_peak(0.0, 0.5, n as u64),
                    0.8,
                )
            })
            .collect();
        for (k, share) in [0.33, 0.5, 0.4, 0.33].into_iter().enumerate() {
            placed_all.push(placed(90 + k, 0.8, mixed(share, 2.5, 90 + k as u64), 0.75));
        }
        run_of(placed_all, Vec::new())
    }

    #[test]
    fn outliers_are_shown_beside_the_same_typical_sample_one_of_the_majority() {
        let a = assess(&cd69_plate(), None);
        let outliers: Vec<&Flag> = a
            .flags
            .iter()
            .filter(|f| f.sample.id.starts_with("f9"))
            .collect();
        assert_eq!(outliers.len(), 4, "{:#?}", a.flags);
        let shown: Vec<&str> = outliers
            .iter()
            .map(|f| f.typical_peer.as_ref().expect("a typical peer").id.as_str())
            .collect();
        for id in &shown {
            assert!(
                !id.starts_with("f9"),
                "another outlier shown as typical: {shown:?}"
            );
        }
        assert!(shown.windows(2).all(|w| w[0] == w[1]), "{shown:?}");
    }

    #[test]
    fn a_two_peaked_outlier_s_gate_is_judged_against_one_peaked_peers() {
        let a = assess(&cd69_plate(), None);
        for f in a.flags.iter().filter(|f| f.sample.id.starts_with("f9")) {
            let measures: Vec<&str> = f.reasons.iter().map(|r| r.measure).collect();
            assert!(
                measures.contains(&"gate_position"),
                "{}: where its gate sits was not compared: {measures:?}",
                f.sample.id
            );
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
        assert_eq!(flag.reasons[0].measure, "gate_position", "{flag:#?}");
        assert!(flag.confident_peers && flag.peers == 10);
        // Beside it, a peer that placed the gate in the valley.
        let peer = flag.typical_peer.as_ref().expect("a typical peer");
        assert_ne!(peer.id, "f99");
        assert!(peer.id.starts_with('f'));
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
            .filter(|m| *m == "gate_position")
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

    /// Weak only for its count, read on a control of more than 300 events
    /// and in its band: not flagged for its confidence. At 300 events, or out
    /// of its band, or not read on a control, it is.
    #[test]
    fn a_placement_in_band_on_a_control_of_over_300_events_is_not_flagged_for_its_confidence() {
        let on_control = |events: usize, in_band: bool, read_on_control: bool| {
            let mut odd = placed(99, 1.5, population(3.0, 99), 0.1);
            (odd.reference_events, odd.in_band, odd.read_on_control) =
                (events, in_band, read_on_control);
            assess(&run_with(Some(odd)), None).flags.len()
        };
        assert_eq!(on_control(301, true, true), 0);
        assert_eq!(on_control(300, true, true), 1);
        assert_eq!(on_control(301, false, true), 1);
        assert_eq!(on_control(301, true, false), 1);
    }

    /// The CD218a gates an FMX with background pushed far right: trusted for
    /// its count, but far from where its peers put theirs.
    #[test]
    fn a_placement_on_a_trusted_control_is_still_flagged_for_where_its_gate_sits() {
        let mut odd = placed(99, 2.9, population(3.0, 99), 0.1);
        (odd.reference_events, odd.in_band, odd.read_on_control) = (301, true, true);
        let a = assess(&run_with(Some(odd)), None);
        assert_eq!(a.flags.len(), 1, "{:#?}", a.flags);
        let measures: Vec<&str> = a.flags[0].reasons.iter().map(|r| r.measure).collect();
        assert_eq!(measures, ["gate_position"]);
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

    // ── what the run could not place ─────────────────────────────────────

    fn refused(gate: &str, sample: &str, reason: &str) -> crate::review::run_record::SkippedRecord {
        crate::review::run_record::SkippedRecord {
            gate: gate.into(),
            parent_gate: Some("CD4+".into()),
            sample: SampleRef {
                id: sample.into(),
                name: (!sample.is_empty()).then(|| format!("{sample}_FS.fcs")),
                sample_type: None,
            },
            reason: reason.into(),
        }
    }

    const OVERLAP: &str = "it would overlap CD8+ on the same plot, so it was left where it was";

    #[test]
    fn a_gate_refused_on_every_sample_for_one_reason_is_refused_everywhere_and_comes_first() {
        let mut run = run_of(vec![placed(0, 1.5, population(3.0, 0), 0.8)], Vec::new());
        run.skipped = vec![
            // CD279+ is placed on one sample, so its one refusal is the data's.
            refused("CD279+", "f9", "no valley"),
            refused("CD4-CD8-", "a", OVERLAP),
            refused("CD4-CD8-", "b", OVERLAP),
            refused("CD4-CD8-", "c", OVERLAP),
        ];
        let found = unplaced(&run, None);
        let gates: Vec<(&str, usize, bool)> = found
            .iter()
            .map(|u| (u.gate.as_str(), u.samples.len(), u.everywhere))
            .collect();
        assert_eq!(gates, [("CD4-CD8-", 3, true), ("CD279+", 1, false)]);
    }

    #[test]
    fn a_gate_refused_for_two_reasons_is_not_said_to_be_the_rule_s_fault() {
        let mut run = run_of(Vec::new(), Vec::new());
        run.skipped = vec![
            refused("CD4-CD8-", "a", OVERLAP),
            refused("CD4-CD8-", "b", "too few events"),
        ];
        let found = unplaced(&run, None);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|u| !u.everywhere), "{found:?}");
    }

    #[test]
    fn a_reference_left_where_it_was_drawn_is_not_a_placement() {
        let mut run = run_of(Vec::new(), vec![kept_at(0, 1.0, population(3.0, 0), false)]);
        run.skipped = vec![
            refused("CD279+", "a", OVERLAP),
            refused("CD279+", "b", OVERLAP),
        ];
        assert!(unplaced(&run, None)[0].everywhere);
        // One that met its rule is.
        run.kept = vec![kept_at(0, 1.0, population(3.0, 0), true)];
        assert!(!unplaced(&run, None)[0].everywhere);
    }

    #[test]
    fn an_unplaced_gate_says_what_happened_in_a_line() {
        let mut run = run_of(vec![placed(0, 1.5, population(3.0, 0), 0.8)], Vec::new());
        run.skipped = vec![
            refused("CD279+", "a", "no valley"),
            refused("CD279+", "b", "no valley"),
            refused("CD279+", "c", "no valley"),
            refused("CD279+", "d", "no valley"),
            refused("CD4-CD8-", "a", OVERLAP),
            refused("CD69+", "", "this rule reaches no gate"),
        ];
        let says: Vec<String> = unplaced(&run, None).iter().map(Unplaced::says).collect();
        assert_eq!(
            says,
            [
                format!(
                    "CD4-CD8- of CD4+ was not placed on any sample, each time because {OVERLAP}. \
                     That is its rule or the gates as drawn, not the data: change the rule."
                ),
                "CD69+ of CD4+: this rule reaches no gate.".to_string(),
                "CD279+ of CD4+ on 4 samples (a_FS.fcs, b_FS.fcs, c_FS.fcs, and 1 more): \
                 no valley."
                    .to_string(),
            ]
        );
    }

    #[test]
    fn what_is_wrong_with_the_rule_is_said_after_the_refusal() {
        let refusal = Unplaced {
            gate: "CD4-CD8-".into(),
            parent_gate: None,
            reason: OVERLAP.into(),
            samples: vec![SampleRef {
                id: "a".into(),
                name: None,
                sample_type: None,
            }],
            everywhere: false,
            rule_problem: Some("its gaps reach into CD8+".into()),
        };
        assert_eq!(
            refusal.says(),
            format!("CD4-CD8- on 1 sample (a): {OVERLAP}. The rule: its gaps reach into CD8+.")
        );
    }
}
