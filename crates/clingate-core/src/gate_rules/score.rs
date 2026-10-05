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

use crate::gate_rules::autogate::{Measurement, admitted_by, describe, solve_all_reporting};
use crate::gate_rules::rule_store::RuleStore;
use crate::gate_rules::run::{RunInputs, measure_many};
use crate::gate_rules::threshold::interquartile_spread;
use crate::gate_rules::trial::only;
use crate::gates::GateState;

/// How many of a gate's samples furthest from the hand gating are named.
pub const WORST_NAMED: usize = 3;

/// One gate on one sample: the rule's answer beside the hand-drawn one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoreRow {
    pub gate_id: String,
    pub gate: String,
    pub file: String,
    pub specimen: Option<String>,
    pub sample_type: Option<String>,
    /// "moved", "kept" (the hand gate already meets the rule) or "not placed".
    pub what: &'static str,
    pub hand_line: Option<f64>,
    pub rule_line: Option<f64>,
    /// The rule's line minus the hand line, in the sample's own parent's
    /// interquartile ranges; `None` for a rule with no line.
    pub off_iqrs: Option<f64>,
    /// The fraction of the parent each gate holds.
    pub hand_holds: Option<f64>,
    pub rule_holds: Option<f64>,
    /// The rule's % of the parent minus the hand gate's, in percentage points.
    pub holds_difference: Option<f64>,
    pub confidence: Option<f64>,
    pub why_not: Option<String>,
}

/// One gate across every sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GateScore {
    pub gate_id: String,
    pub gate: String,
    /// Samples the rule placed or kept, and those it could not place.
    pub scored: usize,
    pub not_placed: usize,
    /// How far the rule's line is from the hand line, ignoring which side.
    pub median_off_iqrs: Option<f64>,
    pub worst_off_iqrs: Option<f64>,
    /// How far apart the two gates' % of the parent are, ignoring which side.
    pub median_holds_difference: Option<f64>,
    pub worst_holds_difference: Option<f64>,
    /// The files furthest from the hand gating, worst first.
    pub worst: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Score {
    pub files_read: usize,
    /// Files that could not be read or measured.
    pub problems: Vec<String>,
    /// Worst first.
    pub gates: Vec<GateScore>,
    /// Gate by gate, each gate's worst first.
    pub rows: Vec<ScoreRow>,
}

/// How far `row` is from the hand gating, for ordering: the line where there
/// is one, the % held where there is not.
pub fn distance(row: &ScoreRow) -> f64 {
    row.off_iqrs
        .or(row.holds_difference.map(|d| d / 100.0))
        .map_or(f64::NEG_INFINITY, f64::abs)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    (n > 0).then(|| (values[(n - 1) / 2] + values[n / 2]) / 2.0)
}

fn worst(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::max)
}

/// How far `rule_line` is from the hand line on `measured`, in its parent's
/// interquartile ranges.
fn off_in_iqrs(measured: &Measurement, rule_line: f64) -> Option<f64> {
    let line = measured.line.as_ref()?;
    let mut sorted = line.values.clone();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let iqr = (!sorted.is_empty()).then(|| interquartile_spread(&sorted))?;
    (iqr > 0.0).then(|| (rule_line - line.current) / iqr)
}

/// Score every rule in `inputs` against the gates as drawn in `gates`,
/// moving nothing. `which` keeps only the rules it accepts.
pub fn score_rules(
    gates: &GateState,
    inputs: &RunInputs,
    which: impl Fn(&crate::gate_rules::rule_store::RuleTarget) -> bool,
    cancel: &AtomicBool,
) -> Result<Score, String> {
    let stores: Vec<RuleStore> = inputs
        .rules
        .entries()
        .iter()
        .filter(|e| which(&e.target))
        .map(|e| only(&inputs.rules, &e.target, &e.rule))
        .collect();
    if stores.is_empty() {
        return Err("no rule to score".into());
    }
    let (measured, problems) = measure_many(
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
    let metadata = &inputs.metadata;
    let pairing = &inputs.rules.pairing;
    let sample_type = |file: &str| -> Option<String> {
        metadata
            .get(file)
            .and_then(|row| pairing.sample_type_of(row))
            .map(|t| t.to_string())
    };
    let specimen = |file: &str| -> Option<String> {
        metadata
            .get(file)
            .and_then(|row| row.get(&pairing.sample_id_column))
            .map(|s| s.to_string())
    };

    let mut rows: Vec<ScoreRow> = Vec::new();
    for (store, (measurements, unmeasured)) in stores.iter().zip(&measured) {
        let (report, placements) =
            solve_all_reporting(gates, store, measurements, unmeasured, metadata, |_, _| {}, cancel);
        let measurement = |gate_id: &str, file: &str| {
            measurements
                .iter()
                .find(|m| &*m.gate_id == gate_id && &*m.file == file)
        };
        let hand_holds = |m: &Measurement| {
            gates
                .gate_for_file(&m.gate_id, &m.file, metadata)
                .and_then(|g| admitted_by(&g, &m.index))
        };
        let row = |m: &Measurement, what, rule_line: Option<f64>, rule_holds: Option<f64>| {
            let hand_holds = hand_holds(m);
            ScoreRow {
                gate_id: m.gate_id.to_string(),
                gate: describe(&m.gate, m.parent_gate.as_deref()),
                file: m.file.to_string(),
                specimen: specimen(&m.file),
                sample_type: sample_type(&m.file),
                what,
                hand_line: m.line.as_ref().map(|l| l.current),
                rule_line,
                off_iqrs: rule_line.and_then(|at| off_in_iqrs(m, at)),
                hand_holds,
                rule_holds,
                holds_difference: rule_holds
                    .zip(hand_holds)
                    .map(|(rule, hand)| (rule - hand) * 100.0),
                confidence: None,
                why_not: None,
            }
        };
        for (p, placement) in report.positioned.iter().zip(&placements) {
            let Some(m) = measurement(&p.gate_id, &p.file) else {
                continue;
            };
            let rule_line = m.line.is_some().then_some(p.to).filter(|v| v.is_finite());
            let rule_holds = admitted_by(&placement.gate, &m.index);
            rows.push(ScoreRow {
                confidence: Some(p.confidence),
                ..row(m, "moved", rule_line, rule_holds)
            });
        }
        for u in &report.unchanged {
            if let Some(m) = measurement(&u.gate_id, &u.file) {
                let hand = m.line.as_ref().map(|l| l.current);
                rows.push(row(m, "kept", hand, hand_holds(m)));
            }
        }
        for s in &report.skipped {
            if let Some(m) = measurements
                .iter()
                .find(|m| m.file == s.file && m.gate == s.gate)
            {
                rows.push(ScoreRow {
                    why_not: Some(s.reason.clone()),
                    ..row(m, "not placed", None, None)
                });
            }
        }
    }

    let mut by_gate: BTreeMap<String, Vec<ScoreRow>> = BTreeMap::new();
    for row in rows {
        by_gate.entry(row.gate_id.clone()).or_default().push(row);
    }
    let mut gate_scores = Vec::new();
    let mut ordered = Vec::new();
    for (gate_id, mut rows) in by_gate {
        rows.sort_by(|a, b| distance(b).total_cmp(&distance(a)));
        let offs: Vec<f64> = rows.iter().filter_map(|r| r.off_iqrs.map(f64::abs)).collect();
        let held: Vec<f64> = rows
            .iter()
            .filter_map(|r| r.holds_difference.map(f64::abs))
            .collect();
        let not_placed = rows.iter().filter(|r| r.what == "not placed").count();
        gate_scores.push(GateScore {
            gate: rows[0].gate.clone(),
            gate_id,
            scored: rows.len() - not_placed,
            not_placed,
            median_off_iqrs: median(offs.clone()),
            worst_off_iqrs: worst(&offs),
            median_holds_difference: median(held.clone()),
            worst_holds_difference: worst(&held),
            worst: rows
                .iter()
                .filter(|r| r.what != "not placed")
                .take(WORST_NAMED)
                .map(|r| r.file.clone())
                .collect(),
        });
        ordered.extend(rows);
    }
    let gate_distance = |g: &GateScore| {
        g.worst_off_iqrs
            .or(g.worst_holds_difference.map(|d| d / 100.0))
            .unwrap_or(f64::NEG_INFINITY)
    };
    gate_scores.sort_by(|a, b| gate_distance(b).total_cmp(&gate_distance(a)));

    Ok(Score {
        files_read: inputs.files.len(),
        problems,
        gates: gate_scores,
        rows: ordered,
    })
}
