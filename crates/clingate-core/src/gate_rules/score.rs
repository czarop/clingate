//! Scoring a ruleset against gating done by hand.
//!
//! Every ruled gate is run on its own, measured under the gates as they are
//! drawn - so a gate a rule gets wrong cannot make the gates below it look
//! wrong too - on the full files, each read once for every gate. On each
//! sample the rule's gate and the hand-drawn one are compared by the events
//! they hold: how many of the hand gate's events the rule's gate also holds,
//! how many it holds that the hand gate does not, and one agreement that
//! forgives a difference no bigger than counting noise - which is large for
//! a gate of a few dozen cells and small for one of thousands. That works for
//! any shape of gate. Beside it: how far the events the rule's gate holds sit
//! from those the hand gate holds, and for a rule that moves one edge, how
//! far it moved it. Nothing is moved or recorded.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use flow_gates::EventIndex;
use serde::Serialize;

use crate::gate_rules::autogate::{
    LineReading, Measurement, Placement, Report, admitted_by, describe, solve_all_reporting,
};
use crate::gate_rules::rule_store::{RuleStore, RuleTarget};
use crate::gate_rules::run::{RunInputs, measure_many};
use crate::gate_rules::threshold::interquartile_spread;
use crate::gate_rules::trial::{only, sample_type_name, specimen_name};
use crate::gates::GateState;
use crate::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::MetaDataFileMap;

/// How many counting-noise widths of disagreement are forgiven: a gate of
/// `n` events is not placed more precisely than about `sqrt(n)` of them.
pub const NOISE_WIDTHS: f64 = 2.0;
/// A sample agreeing less than this with the hand gating is off.
pub const OFF_BELOW: f64 = 0.8;
/// How many of a gate's off samples are named.
pub const OFF_NAMED: usize = 5;
/// Fewer kept events than this in either gate, and no shift is read.
pub const SHIFT_EVENTS: usize = 10;

/// The events two gates hold on one sample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Shared {
    pub hand: usize,
    pub rule: usize,
    pub both: usize,
}

impl Shared {
    /// The share of the hand gate's events the rule's gate also holds.
    pub fn caught(&self) -> Option<f64> {
        (self.hand > 0).then(|| self.both as f64 / self.hand as f64)
    }

    /// The share of the rule's gate's events the hand gate does not hold.
    pub fn extra(&self) -> Option<f64> {
        (self.rule > 0).then(|| (self.rule - self.both) as f64 / self.rule as f64)
    }

    /// 1 less the events held by only one of the two gates, as a share of
    /// both gates' events, after forgiving [`NOISE_WIDTHS`] counting-noise
    /// widths of them: 1 for gates that agree to within counting noise,
    /// 0 for gates holding none of the same events.
    pub fn agreement(&self) -> f64 {
        let total = (self.hand + self.rule) as f64;
        if total == 0.0 {
            return 1.0;
        }
        let apart = total - 2.0 * self.both as f64;
        let forgiven = NOISE_WIDTHS * total.sqrt();
        1.0 - (apart - forgiven).max(0.0) / total
    }
}

/// One gate on one sample: the rule's answer beside the hand-drawn one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoreRow {
    pub gate_id: String,
    /// The gate and the population it is drawn on.
    pub gate: String,
    pub file: String,
    pub specimen: Option<String>,
    pub sample_type: Option<String>,
    /// "moved", "kept" (the hand gate already meets the rule), "reference"
    /// (what the rule calibrates from, not scored) or "not placed".
    pub what: &'static str,
    /// The events each gate holds, and those both hold.
    pub events: Option<Shared>,
    /// See [`Shared::agreement`], [`Shared::caught`] and [`Shared::extra`].
    pub agreement: Option<f64>,
    pub caught: Option<f64>,
    pub extra: Option<f64>,
    /// The fraction of the parent each gate holds, and the rule's % less the
    /// hand gate's, in percentage points.
    pub hand_holds: Option<f64>,
    pub rule_holds: Option<f64>,
    pub holds_difference: Option<f64>,
    /// How far the middle of the events the rule's gate holds sits from the
    /// middle of those the hand gate holds, on each of the plot's axes, in
    /// the parent's interquartile ranges on that axis.
    pub shift_iqrs: Vec<(String, f64)>,
    /// For a rule that moves one edge: where the hand gate's edge sits, where
    /// the rule puts it, and the difference in the parent's interquartile
    /// ranges on that parameter.
    pub hand_edge: Option<f64>,
    pub rule_edge: Option<f64>,
    pub edge_off_iqrs: Option<f64>,
    pub confidence: Option<f64>,
    /// Why the rule could not place the gate on this sample.
    pub why_not: Option<String>,
}

/// One gate across every sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GateScore {
    pub gate_id: String,
    pub gate: String,
    /// Samples the rule moved or kept, those it could not place, and the
    /// references it calibrates from.
    pub scored: usize,
    pub not_placed: usize,
    pub references: usize,
    /// The typical and the lowest agreement: a high median and a low lowest
    /// is a few samples far off, a low median is all of them a little off.
    pub median_agreement: Option<f64>,
    pub lowest_agreement: Option<f64>,
    /// How many scored samples agree less than [`OFF_BELOW`], and the first
    /// [`OFF_NAMED`] of them, furthest off first.
    pub off: usize,
    pub off_samples: Vec<String>,
    /// Typically: how much of the hand gate the rule catches, and how much
    /// it holds beyond it - which way the rule is wrong.
    pub median_caught: Option<f64>,
    pub median_extra: Option<f64>,
    /// Typically, with sign: the rule's % of the parent less the hand gate's,
    /// and for a rule that moves one edge, how far it moves it.
    pub median_holds_difference: Option<f64>,
    pub median_edge_off_iqrs: Option<f64>,
}

/// Every scored gate, and every sample of each.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Score {
    pub files_read: usize,
    /// Files that could not be read or measured, and rules refused before
    /// any sample was read.
    pub problems: Vec<String>,
    /// Most samples off first, then the lowest typical agreement.
    pub gates: Vec<GateScore>,
    /// Gate by gate, each gate's least agreeing first.
    pub rows: Vec<ScoreRow>,
}

/// Least agreeing first; rows with no agreement - not placed, references -
/// last.
pub fn least_agreeing_first(a: &ScoreRow, b: &ScoreRow) -> std::cmp::Ordering {
    let key = |row: &ScoreRow| row.agreement.unwrap_or(f64::INFINITY);
    key(a).total_cmp(&key(b))
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    (n > 0).then(|| (values[(n - 1) / 2] + values[n / 2]) / 2.0)
}

/// How far `rule_edge` is from the hand edge `line` was read at, in its
/// parent's interquartile ranges.
pub fn off_in_iqrs(line: &LineReading, rule_edge: f64) -> Option<f64> {
    let mut sorted = line.values.clone();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let iqr = (!sorted.is_empty()).then(|| interquartile_spread(&sorted))?;
    (iqr > 0.0).then(|| (rule_edge - line.current) / iqr)
}

/// The one gate a drawable gate is, or none for a gate of several parts.
fn single(gate: &Arc<dyn DrawableGate>) -> Option<&flow_gates::Gate> {
    (!gate.is_composite())
        .then(|| gate.get_gate_ref(None))
        .flatten()
}

/// The events `hand` and `rule` hold in `index`, every one counted.
pub fn shared_events(
    index: &EventIndex,
    hand: &flow_gates::Gate,
    rule: &flow_gates::Gate,
) -> Option<Shared> {
    let mut in_hand = vec![false; index.len()];
    let hand_events = index.filter_by_gate(hand).ok()?;
    for &event in &hand_events {
        in_hand[event] = true;
    }
    let rule_events = index.filter_by_gate(rule).ok()?;
    Some(Shared {
        hand: hand_events.len(),
        rule: rule_events.len(),
        both: rule_events.iter().filter(|&&event| in_hand[event]).count(),
    })
}

/// How far the middle of what `rule` holds sits from the middle of what
/// `hand` holds, on each axis of `points`, in the IQRs of `points` on that
/// axis; an axis with no spread, or a gate holding too few points, gives
/// nothing.
pub fn shift_in_iqrs(
    points: &[(f32, f32)],
    hand: &flow_gates::Gate,
    rule: &flow_gates::Gate,
) -> Option<(Option<f64>, Option<f64>)> {
    let (xs, ys): (Vec<f32>, Vec<f32>) = points.iter().copied().unzip();
    let index = EventIndex::build(&xs, &ys).ok()?;
    let held = |gate| index.filter_by_gate(gate).ok().filter(|e| e.len() >= SHIFT_EVENTS);
    let (hand, rule) = (held(hand)?, held(rule)?);
    let axis = |values: &[f32]| {
        let middle = |events: &[usize]| {
            median(events.iter().map(|&e| f64::from(values[e])).collect()).unwrap_or(0.0)
        };
        let mut sorted: Vec<f64> = values.iter().map(|&v| f64::from(v)).collect();
        sorted.sort_by(|a, b| b.total_cmp(a));
        let iqr = interquartile_spread(&sorted);
        (iqr > 0.0).then(|| (middle(&rule) - middle(&hand)) / iqr)
    };
    Some((axis(&xs), axis(&ys)))
}

/// A row for a sample the rule could not measure.
fn unmeasured_row(
    metadata: &MetaDataFileMap,
    store: &RuleStore,
    refused: &crate::gate_rules::autogate::Unplaced,
) -> ScoreRow {
    ScoreRow {
        gate_id: refused.gate_id.to_string(),
        gate: describe(&refused.gate, refused.parent_gate.as_deref()),
        file: refused.file.to_string(),
        specimen: specimen_name(metadata, &store.pairing, &refused.file),
        sample_type: sample_type_name(metadata, &store.pairing, &refused.file),
        what: "not placed",
        events: None,
        agreement: None,
        caught: None,
        extra: None,
        hand_holds: None,
        rule_holds: None,
        holds_difference: None,
        shift_iqrs: Vec::new(),
        hand_edge: None,
        rule_edge: None,
        edge_off_iqrs: None,
        confidence: None,
        why_not: Some(refused.reason.clone()),
    }
}

/// One rule's run, read back sample by sample.
struct RunRows<'a> {
    gates: &'a GateState,
    metadata: &'a MetaDataFileMap,
    store: &'a RuleStore,
    measurements: &'a [Measurement],
}

impl RunRows<'_> {
    fn measurement(&self, gate_id: &str, file: &str) -> Option<&Measurement> {
        self.measurements
            .iter()
            .find(|m| &*m.gate_id == gate_id && &*m.file == file)
    }

    fn hand_gate(&self, measured: &Measurement) -> Option<Arc<dyn DrawableGate>> {
        self.gates
            .gate_for_file(&measured.gate_id, &measured.file, self.metadata)
    }

    /// A row for `measured` with nothing compared: a reference, or a sample
    /// the rule could not place.
    fn bare(&self, measured: &Measurement, what: &'static str) -> ScoreRow {
        ScoreRow {
            gate_id: measured.gate_id.to_string(),
            gate: describe(&measured.gate, measured.parent_gate.as_deref()),
            file: measured.file.to_string(),
            specimen: specimen_name(self.metadata, &self.store.pairing, &measured.file),
            sample_type: sample_type_name(self.metadata, &self.store.pairing, &measured.file),
            what,
            events: None,
            agreement: None,
            caught: None,
            extra: None,
            hand_holds: self
                .hand_gate(measured)
                .and_then(|gate| admitted_by(&gate, &measured.index)),
            rule_holds: None,
            holds_difference: None,
            shift_iqrs: Vec::new(),
            hand_edge: measured.line.as_ref().map(|line| line.current),
            rule_edge: None,
            edge_off_iqrs: None,
            confidence: None,
            why_not: None,
        }
    }

    /// A row for `measured`, the hand gate against `rule`.
    fn compared(
        &self,
        measured: &Measurement,
        what: &'static str,
        rule: &Arc<dyn DrawableGate>,
        rule_edge: Option<f64>,
    ) -> ScoreRow {
        let bare = self.bare(measured, what);
        let hand = self.hand_gate(measured);
        let pair = hand.as_ref().and_then(single).zip(single(rule));
        let events = pair.and_then(|(hand, rule)| {
            shared_events(&measured.index.event_index, hand, rule)
        });
        let shift = pair
            .and_then(|(hand, rule)| shift_in_iqrs(&measured.kept_events, hand, rule))
            .map(|(x, y)| {
                [(&measured.params.0, x), (&measured.params.1, y)]
                    .into_iter()
                    .filter_map(|(axis, off)| off.map(|off| (axis.to_string(), off)))
                    .collect()
            })
            .unwrap_or_default();
        let rule_holds = admitted_by(rule, &measured.index);
        ScoreRow {
            events,
            agreement: events.map(|e| e.agreement()),
            caught: events.and_then(|e| e.caught()),
            extra: events.and_then(|e| e.extra()),
            rule_holds,
            holds_difference: rule_holds
                .zip(bare.hand_holds)
                .map(|(rule, hand)| (rule - hand) * 100.0),
            shift_iqrs: shift,
            rule_edge,
            edge_off_iqrs: measured
                .line
                .as_ref()
                .zip(rule_edge)
                .and_then(|(line, at)| off_in_iqrs(line, at)),
            ..bare
        }
    }

    /// Every row the run gives: moved, kept, references and not placed.
    fn rows(&self, report: &Report, placements: &[Placement]) -> Vec<ScoreRow> {
        let mut rows = Vec::new();
        for (placed, placement) in report.positioned.iter().zip(placements) {
            let Some(measured) = self.measurement(&placed.gate_id, &placed.file) else {
                continue;
            };
            let rule_edge = measured
                .line
                .is_some()
                .then_some(placed.to)
                .filter(|at| at.is_finite());
            rows.push(ScoreRow {
                confidence: Some(placed.confidence),
                ..self.compared(measured, "moved", &placement.gate, rule_edge)
            });
        }
        for left in &report.unchanged {
            if let Some(measured) = self.measurement(&left.gate_id, &left.file)
                && let Some(hand) = self.hand_gate(measured)
            {
                let edge = measured.line.as_ref().map(|line| line.current);
                rows.push(self.compared(measured, "kept", &hand, edge));
            }
        }
        for reference in &report.reference {
            if let Some(measured) = self.measurement(&reference.gate_id, &reference.file) {
                rows.push(self.bare(measured, "reference"));
            }
        }
        for refused in &report.unplaced {
            rows.push(match self.measurement(&refused.gate_id, &refused.file) {
                Some(measured) => ScoreRow {
                    why_not: Some(refused.reason.clone()),
                    ..self.bare(measured, "not placed")
                },
                None => unmeasured_row(self.metadata, self.store, refused),
            });
        }
        rows
    }
}

/// One gate summed up, from its rows least agreeing first.
fn summarise(gate_id: String, rows: &[ScoreRow]) -> GateScore {
    let count = |what: &str| rows.iter().filter(|row| row.what == what).count();
    let scored: Vec<&ScoreRow> = rows
        .iter()
        .filter(|row| matches!(row.what, "moved" | "kept"))
        .collect();
    let all = |of: fn(&ScoreRow) -> Option<f64>| -> Vec<f64> {
        scored.iter().filter_map(|row| of(row)).collect()
    };
    let agreements = all(|row| row.agreement);
    let off: Vec<&&ScoreRow> = scored
        .iter()
        .filter(|row| row.agreement.is_some_and(|a| a < OFF_BELOW))
        .collect();
    GateScore {
        gate: rows[0].gate.clone(),
        gate_id,
        scored: scored.len(),
        not_placed: count("not placed"),
        references: count("reference"),
        lowest_agreement: agreements.iter().copied().reduce(f64::min),
        median_agreement: median(agreements),
        off: off.len(),
        off_samples: off
            .iter()
            .take(OFF_NAMED)
            .map(|row| row.file.clone())
            .collect(),
        median_caught: median(all(|row| row.caught)),
        median_extra: median(all(|row| row.extra)),
        median_holds_difference: median(all(|row| row.holds_difference)),
        median_edge_off_iqrs: median(all(|row| row.edge_off_iqrs)),
    }
}

/// Most samples off first, then the lowest typical agreement.
fn most_off_first(a: &GateScore, b: &GateScore) -> std::cmp::Ordering {
    let typical = |g: &GateScore| g.median_agreement.unwrap_or(f64::INFINITY);
    b.off.cmp(&a.off).then(typical(a).total_cmp(&typical(b)))
}

/// Score every rule in `inputs` that `which` accepts against the gates as
/// drawn in `gates`, moving nothing.
pub fn score_rules(
    gates: &GateState,
    inputs: &RunInputs,
    which: impl Fn(&RuleTarget) -> bool,
    cancel: &AtomicBool,
) -> Result<Score, String> {
    let stores: Vec<RuleStore> = inputs
        .rules
        .entries()
        .iter()
        .filter(|entry| which(&entry.target))
        .map(|entry| only(&inputs.rules, &entry.target, &entry.rule))
        .collect();
    if stores.is_empty() {
        return Err("no rule to score".into());
    }
    let (measured, mut problems) = measure_many(
        gates,
        &inputs.files,
        &inputs.compensation,
        &inputs.names,
        &inputs.cofactors,
        &inputs.metadata,
        &stores,
        None,
        cancel,
        |_, _| {},
    );
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("stopped".into());
    }

    let mut by_gate: BTreeMap<String, Vec<ScoreRow>> = BTreeMap::new();
    for (store, (measurements, unmeasured)) in stores.iter().zip(&measured) {
        let (report, placements) = solve_all_reporting(
            gates,
            store,
            measurements,
            unmeasured,
            &inputs.metadata,
            |_, _| {},
            cancel,
        );
        for refused in report.skipped.iter().filter(|s| s.file.is_empty()) {
            problems.push(format!(
                "{}: {}",
                describe(&refused.gate, refused.parent_gate.as_deref()),
                refused.reason
            ));
        }
        let run = RunRows {
            gates,
            metadata: &inputs.metadata,
            store,
            measurements,
        };
        for row in run.rows(&report, &placements) {
            by_gate.entry(row.gate_id.clone()).or_default().push(row);
        }
    }

    let mut scored: Vec<(GateScore, Vec<ScoreRow>)> = by_gate
        .into_iter()
        .map(|(gate_id, mut rows)| {
            rows.sort_by(least_agreeing_first);
            (summarise(gate_id, &rows), rows)
        })
        .collect();
    scored.sort_by(|(a, _), (b, _)| most_off_first(a, b));
    let (gate_scores, rows): (Vec<GateScore>, Vec<Vec<ScoreRow>>) = scored.into_iter().unzip();

    Ok(Score {
        files_read: inputs.files.len(),
        problems,
        gates: gate_scores,
        rows: rows.into_iter().flatten().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate_rules::rule_store::Bound;

    fn reading(current: f64, values: Vec<f64>) -> LineReading {
        LineReading {
            parameter: "X".into(),
            bound: Bound::Above,
            current,
            values,
            shadow: Vec::new(),
        }
    }

    /// Values 0 to 100: quartiles 25 and 75, an IQR of 50.
    #[test]
    fn an_edge_s_distance_is_read_in_the_parent_s_interquartile_ranges() {
        let line = reading(10.0, (0..=100).map(f64::from).collect());
        assert_eq!(off_in_iqrs(&line, 110.0), Some(2.0));
        assert_eq!(off_in_iqrs(&line, -15.0), Some(-0.5));
        assert_eq!(off_in_iqrs(&line, 10.0), Some(0.0));
    }

    #[test]
    fn a_parent_with_no_spread_or_no_events_gives_no_edge_distance() {
        assert_eq!(off_in_iqrs(&reading(1.0, vec![3.0; 50]), 5.0), None);
        assert_eq!(off_in_iqrs(&reading(1.0, Vec::new()), 5.0), None);
    }

    fn shared(hand: usize, rule: usize, both: usize) -> Shared {
        Shared { hand, rule, both }
    }

    /// 10,000 events each, 9,000 shared: 2,000 held by one gate only, of
    /// which 2 x sqrt(20,000) = 282.8 are forgiven - 1 - 1,717.2 / 20,000.
    #[test]
    fn agreement_forgives_only_counting_noise_on_a_big_gate() {
        let a = shared(10_000, 10_000, 9_000).agreement();
        assert!((a - (1.0 - (2_000.0 - 2.0 * 20_000f64.sqrt()) / 20_000.0)).abs() < 1e-12);
        assert!((a - 0.914_14).abs() < 1e-5, "{a}");
        assert_eq!(shared(10_000, 10_000, 10_000).agreement(), 1.0);
        let none = shared(10_000, 10_000, 0).agreement();
        assert!((none - 2.0 * 20_000f64.sqrt() / 20_000.0).abs() < 1e-12, "{none}");
    }

    /// 20 events each, 15 shared: the 10 held by one gate only are within
    /// 2 x sqrt(40) = 12.6 of counting noise, so the two agree.
    #[test]
    fn a_small_gate_is_forgiven_more() {
        assert_eq!(shared(20, 20, 15).agreement(), 1.0);
        // The same shares on a hundred times the events are not forgiven.
        assert!(shared(2_000, 2_000, 1_500).agreement() < 0.8);
    }

    #[test]
    fn caught_and_extra_say_which_way_the_rule_is_off() {
        let too_tight = shared(1_000, 600, 600);
        assert_eq!((too_tight.caught(), too_tight.extra()), (Some(0.6), Some(0.0)));
        let too_loose = shared(600, 1_000, 600);
        assert_eq!((too_loose.caught(), too_loose.extra()), (Some(1.0), Some(0.4)));
        let empty = shared(0, 0, 0);
        assert_eq!((empty.caught(), empty.extra(), empty.agreement()), (None, None, 1.0));
    }

    fn row(agreement: Option<f64>) -> ScoreRow {
        let mut row = unmeasured_row(
            &Default::default(),
            &RuleStore::default(),
            &crate::gate_rules::autogate::Unplaced {
                gate_id: "g".into(),
                gate: "G".into(),
                parent_gate: None,
                file: "f".into(),
                specimen: None,
                reason: String::new(),
            },
        );
        row.what = "moved";
        row.agreement = agreement;
        row
    }

    /// A few samples far off and every sample a little off read differently:
    /// the first by its lowest agreement and its off samples, the second by
    /// its median.
    #[test]
    fn a_few_far_off_is_told_apart_from_all_a_little_off() {
        let mut few: Vec<ScoreRow> = [0.3, 0.5, 0.97, 0.98, 0.98, 0.99, 0.99]
            .into_iter()
            .map(|a| row(Some(a)))
            .collect();
        few.sort_by(least_agreeing_first);
        let few = summarise("g".into(), &few);
        assert_eq!((few.median_agreement, few.lowest_agreement, few.off), (Some(0.98), Some(0.3), 2));

        let all: Vec<ScoreRow> = [0.84, 0.85, 0.85, 0.86, 0.86, 0.87, 0.88]
            .into_iter()
            .map(|a| row(Some(a)))
            .collect();
        let all = summarise("g".into(), &all);
        assert_eq!((all.median_agreement, all.lowest_agreement, all.off), (Some(0.86), Some(0.84), 0));

        let edge: Vec<ScoreRow> = [0.79, 0.8, 0.81].into_iter().map(|a| row(Some(a))).collect();
        assert_eq!(summarise("g".into(), &edge).off, 1, "under {OFF_BELOW} is off, at it is not");

        let mut gates = [all, few];
        gates.sort_by(most_off_first);
        assert_eq!(gates[0].off, 2, "the gate with samples off comes first");
    }
}
