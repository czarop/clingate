//! Scoring a ruleset against gating done by hand.
//!
//! Every ruled gate is run on its own, measured under the gates as they are
//! drawn - so a gate a rule gets wrong cannot make the gates below it look
//! wrong too - on the full files, each read once for every gate. On each
//! sample it says how far the rule's line is from the one drawn by hand, in
//! the sample's own parent's interquartile ranges, and how much more or less
//! of the parent the rule's gate would hold. Nothing is moved or recorded.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use crate::gate_rules::autogate::{
    LineReading, Measurement, Placement, Report, admitted_by, describe, solve_all_reporting,
};
use crate::gate_rules::rule_store::{RuleStore, RuleTarget};
use crate::gate_rules::run::{RunInputs, measure_many};
use crate::gate_rules::threshold::interquartile_spread;
use crate::gate_rules::trial::{only, sample_type_name, specimen_name};
use crate::gates::GateState;
use crate::omiq::metadata::MetaDataFileMap;

/// How many of a gate's samples furthest from the hand gating are named.
pub const WORST_NAMED: usize = 3;

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
    /// Where the hand-drawn gate's line sits, and where the rule would put it.
    pub hand_line: Option<f64>,
    pub rule_line: Option<f64>,
    /// The rule's line minus the hand line, in the sample's own parent's
    /// interquartile ranges; `None` for a rule with no line, or a parent
    /// with no spread.
    pub off_iqrs: Option<f64>,
    /// The fraction of the parent each gate holds.
    pub hand_holds: Option<f64>,
    pub rule_holds: Option<f64>,
    /// The rule's % of the parent minus the hand gate's, in percentage points.
    pub holds_difference: Option<f64>,
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
    /// How far the rule's line is from the hand line, ignoring which side.
    pub median_off_iqrs: Option<f64>,
    pub worst_off_iqrs: Option<f64>,
    /// How far apart the two gates' % of the parent are, ignoring which side.
    pub median_holds_difference: Option<f64>,
    pub worst_holds_difference: Option<f64>,
    /// The files furthest from the hand gating, worst first; none that the
    /// rule leaves where the hand put it.
    pub worst: Vec<String>,
}

/// Every scored gate, and every sample of each.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Score {
    pub files_read: usize,
    /// Files that could not be read or measured, and rules refused before
    /// any sample was read.
    pub problems: Vec<String>,
    /// Furthest from the hand gating first.
    pub gates: Vec<GateScore>,
    /// Gate by gate, each gate's furthest first.
    pub rows: Vec<ScoreRow>,
}

/// How far `row` is from the hand gating, for ordering: a line's distance in
/// IQRs ranks before a % held, since the two are on different scales; rows
/// with neither come last.
pub fn distance(row: &ScoreRow) -> (u8, f64) {
    match (row.off_iqrs, row.holds_difference) {
        (Some(off), _) => (2, off.abs()),
        (None, Some(difference)) => (1, difference.abs()),
        (None, None) => (0, 0.0),
    }
}

/// Furthest from the hand gating first.
pub fn furthest_first(a: &(u8, f64), b: &(u8, f64)) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then(b.1.total_cmp(&a.1))
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    (n > 0).then(|| (values[(n - 1) / 2] + values[n / 2]) / 2.0)
}

/// How far `rule_line` is from the hand line `line` was read at, in its
/// parent's interquartile ranges.
pub fn off_in_iqrs(line: &LineReading, rule_line: f64) -> Option<f64> {
    let mut sorted = line.values.clone();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let iqr = (!sorted.is_empty()).then(|| interquartile_spread(&sorted))?;
    (iqr > 0.0).then(|| (rule_line - line.current) / iqr)
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
        hand_line: None,
        rule_line: None,
        off_iqrs: None,
        hand_holds: None,
        rule_holds: None,
        holds_difference: None,
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

    /// A row for `measured`, with the rule's answer as given.
    fn row(
        &self,
        measured: &Measurement,
        what: &'static str,
        rule_line: Option<f64>,
        rule_holds: Option<f64>,
    ) -> ScoreRow {
        let hand_holds = self
            .gates
            .gate_for_file(&measured.gate_id, &measured.file, self.metadata)
            .and_then(|gate| admitted_by(&gate, &measured.index));
        ScoreRow {
            gate_id: measured.gate_id.to_string(),
            gate: describe(&measured.gate, measured.parent_gate.as_deref()),
            file: measured.file.to_string(),
            specimen: specimen_name(self.metadata, &self.store.pairing, &measured.file),
            sample_type: sample_type_name(self.metadata, &self.store.pairing, &measured.file),
            what,
            hand_line: measured.line.as_ref().map(|line| line.current),
            rule_line,
            off_iqrs: measured
                .line
                .as_ref()
                .zip(rule_line)
                .and_then(|(line, at)| off_in_iqrs(line, at)),
            hand_holds,
            rule_holds,
            holds_difference: rule_holds
                .zip(hand_holds)
                .map(|(rule, hand)| (rule - hand) * 100.0),
            confidence: None,
            why_not: None,
        }
    }

    /// Every row the run gives: moved, kept, references and not placed.
    fn rows(&self, report: &Report, placements: &[Placement]) -> Vec<ScoreRow> {
        let mut rows = Vec::new();
        for (placed, placement) in report.positioned.iter().zip(placements) {
            let Some(measured) = self.measurement(&placed.gate_id, &placed.file) else {
                continue;
            };
            let rule_line = measured
                .line
                .is_some()
                .then_some(placed.to)
                .filter(|at| at.is_finite());
            let rule_holds = admitted_by(&placement.gate, &measured.index);
            rows.push(ScoreRow {
                confidence: Some(placed.confidence),
                ..self.row(measured, "moved", rule_line, rule_holds)
            });
        }
        for left in &report.unchanged {
            if let Some(measured) = self.measurement(&left.gate_id, &left.file) {
                let hand = self.row(measured, "kept", None, None);
                rows.push(self.row(measured, "kept", hand.hand_line, hand.hand_holds));
            }
        }
        for reference in &report.reference {
            if let Some(measured) = self.measurement(&reference.gate_id, &reference.file) {
                rows.push(self.row(measured, "reference", None, None));
            }
        }
        for refused in &report.unplaced {
            rows.push(match self.measurement(&refused.gate_id, &refused.file) {
                Some(measured) => ScoreRow {
                    why_not: Some(refused.reason.clone()),
                    ..self.row(measured, "not placed", None, None)
                },
                None => unmeasured_row(self.metadata, self.store, refused),
            });
        }
        rows
    }
}

/// One gate summed up, from its rows furthest first.
fn summarise(gate_id: String, rows: &[ScoreRow]) -> GateScore {
    let count = |what: &str| rows.iter().filter(|row| row.what == what).count();
    let scored: Vec<&ScoreRow> = rows
        .iter()
        .filter(|row| matches!(row.what, "moved" | "kept"))
        .collect();
    let offs: Vec<f64> = scored
        .iter()
        .filter_map(|row| row.off_iqrs.map(f64::abs))
        .collect();
    let held: Vec<f64> = scored
        .iter()
        .filter_map(|row| row.holds_difference.map(f64::abs))
        .collect();
    GateScore {
        gate: rows[0].gate.clone(),
        gate_id,
        scored: scored.len(),
        not_placed: count("not placed"),
        references: count("reference"),
        worst_off_iqrs: offs.iter().copied().reduce(f64::max),
        median_off_iqrs: median(offs),
        worst_holds_difference: held.iter().copied().reduce(f64::max),
        median_holds_difference: median(held),
        worst: scored
            .iter()
            .filter(|row| distance(row).1 > 0.0)
            .take(WORST_NAMED)
            .map(|row| row.file.clone())
            .collect(),
    }
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
            rows.sort_by(|a, b| furthest_first(&distance(a), &distance(b)));
            (summarise(gate_id, &rows), rows)
        })
        .collect();
    scored.sort_by(|(_, a), (_, b)| furthest_first(&distance(&a[0]), &distance(&b[0])));
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
    fn the_distance_is_read_in_the_parent_s_interquartile_ranges() {
        let line = reading(10.0, (0..=100).map(f64::from).collect());
        assert_eq!(off_in_iqrs(&line, 110.0), Some(2.0));
        assert_eq!(off_in_iqrs(&line, -15.0), Some(-0.5));
        assert_eq!(off_in_iqrs(&line, 10.0), Some(0.0));
    }

    #[test]
    fn a_parent_with_no_spread_or_no_events_gives_no_distance() {
        assert_eq!(off_in_iqrs(&reading(1.0, vec![3.0; 50]), 5.0), None);
        assert_eq!(off_in_iqrs(&reading(1.0, Vec::new()), 5.0), None);
    }

    /// A line comes before a % held, whatever the numbers, and rows with
    /// neither come last.
    #[test]
    fn rows_are_ordered_lines_first_then_by_how_far_off() {
        let mut keys = [(1, 30.0), (2, 0.5), (0, 0.0), (2, 1.5), (1, 2.0)];
        keys.sort_by(furthest_first);
        assert_eq!(keys, [(2, 1.5), (2, 0.5), (1, 30.0), (1, 2.0), (0, 0.0)]);
    }
}
