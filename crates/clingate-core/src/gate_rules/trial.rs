//! Trying candidate rules for one gate on the files as they are, moving
//! nothing.
//!
//! Choosing a rule is a guess until it has been tried. This runs each
//! candidate through the same measuring and solving a rules run does - each
//! file read once for all of them - and says, sample by sample and summed up
//! by sample type, what the gate would hold under each, beside what it holds
//! as the gates stand now. Nothing is written: not the gates, not the run
//! record, not the rules.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use crate::gate_rules::autogate::{admitted_by, describe, solve_all_reporting};
use crate::gate_rules::rule_store::{GateRule, RuleStore, RuleTarget};
use crate::gate_rules::run::{RunInputs, measure_many};
use crate::gates::GateState;

/// The most candidates tried at once.
pub const MOST_CANDIDATES: usize = 4;

/// Spread of a set of fractions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Spread {
    pub min: f64,
    pub p10: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
}

impl Spread {
    fn of(mut values: Vec<f64>) -> Option<Self> {
        values.retain(|v| v.is_finite());
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        let at = |q: f64| {
            let i = q * (values.len() - 1) as f64;
            let (lo, hi) = (i.floor() as usize, i.ceil() as usize);
            values[lo] + (values[hi] - values[lo]) * (i - lo as f64)
        };
        Some(Self {
            min: values[0],
            p10: at(0.1),
            median: at(0.5),
            p90: at(0.9),
            max: values[values.len() - 1],
        })
    }
}

/// What the gate holds across one kind of sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypeSpread {
    /// The sample type the pairing gives, or "unknown".
    pub sample_type: String,
    pub samples: usize,
    /// The fraction of each sample's parent inside the gate.
    pub holds: Option<Spread>,
}

/// One way of placing the gate, summed up.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// Moved by the rule.
    pub placed: usize,
    /// Left where they were: already meeting the rule, or the reference.
    pub kept: usize,
    pub not_placed: usize,
    /// Placed with confidence under the review line, or outside the band.
    pub flagged: usize,
    pub by_type: Vec<TypeSpread>,
    /// Why samples were not placed, each reason once with how many.
    pub not_placed_because: Vec<(String, usize)>,
}

/// What one candidate did to one sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cell {
    /// The fraction of the sample's parent the gate would hold.
    pub holds: Option<f64>,
    /// Where the gate's line would sit, in the plot's units.
    pub line: Option<f64>,
    /// "moved", "kept", "reference", or "not placed".
    pub what: &'static str,
    pub confidence: Option<f64>,
    pub weakest: Option<&'static str>,
    pub flagged: bool,
    pub why_not: Option<String>,
}

/// One gated sample, under every candidate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub gate_id: String,
    pub file: String,
    pub specimen: Option<String>,
    pub sample_type: Option<String>,
    /// What the gate holds as it stands now.
    pub current_holds: Option<f64>,
    /// One per candidate, in the order given.
    pub candidates: Vec<Cell>,
}

/// Every candidate tried.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trial {
    pub gate: String,
    pub files_read: usize,
    /// Files that could not be read or measured.
    pub problems: Vec<String>,
    /// The gates as they stand now, summed up the same way.
    pub current: Vec<TypeSpread>,
    pub candidates: Vec<Summary>,
    pub rows: Vec<Row>,
}

/// The workspace's rules with only `target`'s rule, set to `rule`: the
/// pairing and hand-picked references kept, every other gate left out so
/// that nothing else is measured.
fn only(base: &RuleStore, target: &RuleTarget, rule: &GateRule) -> RuleStore {
    let mut store = base.clone();
    let others: Vec<RuleTarget> = store
        .entries()
        .iter()
        .map(|e| e.target.clone())
        .filter(|t| t != target)
        .collect();
    for other in others {
        store.remove(&other);
    }
    store.insert(target.clone(), rule.clone());
    store
}

/// Try each of `candidates` as the rule for `target` on the files in
/// `inputs`, moving nothing.
pub fn try_rules(
    gates: &GateState,
    inputs: &RunInputs,
    target: &RuleTarget,
    candidates: &[GateRule],
    cancel: &AtomicBool,
) -> Result<Trial, String> {
    if candidates.is_empty() {
        return Err("no candidate rule to try".into());
    }
    if candidates.len() > MOST_CANDIDATES {
        return Err(format!(
            "at most {MOST_CANDIDATES} candidates at once - {} given",
            candidates.len()
        ));
    }
    let stores: Vec<RuleStore> = candidates
        .iter()
        .map(|rule| only(&inputs.rules, target, rule))
        .collect();
    let (measured, problems) = measure_many(
        gates,
        &inputs.files,
        &inputs.compensation,
        &inputs.names,
        &inputs.cofactors,
        &inputs.metadata,
        &stores,
        cancel,
        |_, _| {},
    );
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("stopped".into());
    }
    let files_read = inputs.files.len();
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

    // What each gated sample would hold, candidate by candidate.
    let mut rows: BTreeMap<(String, String), Row> = BTreeMap::new();
    let mut summaries = Vec::new();
    let blank = || Cell {
        holds: None,
        line: None,
        what: "not placed",
        confidence: None,
        weakest: None,
        flagged: false,
        why_not: None,
    };
    for (at, (store, (measurements, unmeasured))) in stores.iter().zip(&measured).enumerate() {
        let index_of = |gate_id: &str, file: &str| {
            measurements
                .iter()
                .find(|m| &*m.gate_id == gate_id && &*m.file == file)
                .map(|m| &m.index)
        };
        let (report, placements) = solve_all_reporting(
            gates,
            store,
            measurements,
            unmeasured,
            metadata,
            |_, _| {},
            cancel,
        );
        let new_row = |gate_id: &str, file: &str| Row {
            gate_id: gate_id.to_string(),
            file: file.to_string(),
            specimen: specimen(file),
            sample_type: sample_type(file),
            current_holds: None,
            candidates: vec![blank(); candidates.len()],
        };
        fn row_for<'r>(
            rows: &'r mut BTreeMap<(String, String), Row>,
            gate_id: &str,
            file: &str,
            new_row: &dyn Fn(&str, &str) -> Row,
        ) -> &'r mut Row {
            rows.entry((gate_id.to_string(), file.to_string()))
                .or_insert_with(|| new_row(gate_id, file))
        }
        let mut summary = Summary {
            placed: report.positioned.len(),
            kept: report.unchanged.len() + report.reference.len(),
            not_placed: 0,
            flagged: 0,
            by_type: Vec::new(),
            not_placed_because: Vec::new(),
        };
        for (p, placement) in report.positioned.iter().zip(&placements) {
            let flagged = p.confidence < crate::review::assess::REVIEW_FLOOR || !p.in_band;
            summary.flagged += usize::from(flagged);
            let holds = index_of(&p.gate_id, &p.file).and_then(|i| admitted_by(&placement.gate, i));
            row_for(&mut rows, &p.gate_id, &p.file, &new_row).candidates[at] = Cell {
                holds,
                line: Some(p.to).filter(|v| v.is_finite()),
                what: "moved",
                confidence: Some(p.confidence),
                weakest: p.weakest,
                flagged,
                why_not: None,
            };
        }
        for (list, what) in [
            (&report.unchanged, "kept"),
            (&report.reference, "reference"),
        ] {
            for u in list {
                let holds = index_of(&u.gate_id, &u.file).and_then(|i| {
                    gates
                        .gate_for_file(&u.gate_id, &u.file, metadata)
                        .and_then(|g| admitted_by(&g, i))
                });
                row_for(&mut rows, &u.gate_id, &u.file, &new_row).candidates[at] = Cell {
                    holds,
                    line: u.line,
                    what,
                    ..blank()
                };
            }
        }
        let mut because: BTreeMap<String, usize> = BTreeMap::new();
        for s in &report.skipped {
            *because.entry(s.reason.clone()).or_default() += 1;
            summary.not_placed += 1;
            if s.file.is_empty() {
                continue;
            }
            if let Some(m) = measurements
                .iter()
                .find(|m| m.file == s.file && m.gate == s.gate)
            {
                row_for(&mut rows, &m.gate_id, &m.file, &new_row).candidates[at].why_not =
                    Some(s.reason.clone());
            }
        }
        summary.not_placed_because = because.into_iter().collect();
        summaries.push(summary);
    }

    // What the gate holds as it stands, on the same samples.
    for row in rows.values_mut() {
        let index = measured.iter().find_map(|(ms, _)| {
            ms.iter()
                .find(|m| *m.gate_id == *row.gate_id && *m.file == *row.file)
                .map(|m| &m.index)
        });
        let gate_id: crate::gates::gate_store::GateId = Arc::from(row.gate_id.as_str());
        let file: Arc<str> = Arc::from(row.file.as_str());
        row.current_holds = index.and_then(|i| {
            gates
                .gate_for_file(&gate_id, &file, metadata)
                .and_then(|g| admitted_by(&g, i))
        });
    }

    let spread_by_type = |holds: &dyn Fn(&Row) -> Option<f64>| -> Vec<TypeSpread> {
        let mut by: BTreeMap<String, (usize, Vec<f64>)> = BTreeMap::new();
        for row in rows.values() {
            let slot = by
                .entry(row.sample_type.clone().unwrap_or_else(|| "unknown".into()))
                .or_default();
            slot.0 += 1;
            if let Some(h) = holds(row) {
                slot.1.push(h);
            }
        }
        by.into_iter()
            .map(|(sample_type, (samples, values))| TypeSpread {
                sample_type,
                samples,
                holds: Spread::of(values),
            })
            .collect()
    };
    for (at, summary) in summaries.iter_mut().enumerate() {
        summary.by_type = spread_by_type(&|r: &Row| r.candidates[at].holds);
    }
    let current = spread_by_type(&|r: &Row| r.current_holds);

    Ok(Trial {
        gate: describe(&target.gate, target.parent.as_deref()),
        files_read,
        problems,
        current,
        candidates: summaries,
        rows: rows.into_values().collect(),
    })
}
