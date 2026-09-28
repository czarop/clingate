//! The rules, run as a preview whose placements are applied only when asked.
//!
//! A preview runs every rule the way the Gate Rules tab's button does and
//! keeps what it would place, moving nothing. Applying writes those
//! placements into the working copy as one undo step - and refuses if the
//! gates have changed since the preview, since the answers were measured on
//! gates that are no longer there. Saving is a separate step (see `edits`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::{Refusal, Session, failed, round};
use crate::gate_rules::autogate::{Placement, describe};
use crate::gate_rules::rule_store::{Bound, MeasuredOn};
use crate::gate_rules::run::{RunInputs, run_rules};
use crate::gates::GateState;

/// Below this confidence a placement is worth opening by hand - the Gate
/// Rules tab's line, measured against a hand-gated export.
pub const REVIEW_FLOOR: f64 = 0.30;

/// What a preview proposed, kept until it is applied.
pub(crate) struct Pending {
    /// The gates as the preview measured them.
    snapshot: GateState,
    placements: Vec<Placement>,
    /// What the run decided, kept in the workspace once applied.
    record: crate::review::RunRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct RulesView {
    pub file: Option<PathBuf>,
    /// The metadata column that says which files are one specimen, and the
    /// one that says what kind of sample each is (a full stain, an FMO...).
    pub specimen_column: String,
    pub sample_type_column: String,
    pub rules: Vec<RuleRow>,
    /// References chosen by hand for particular files.
    pub reference_overrides: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleRow {
    /// "Ki67+ of CD4+": the gate, and the population it is drawn on.
    pub population: String,
    pub parameter: String,
    /// Whether the gate keeps events above its line or below it.
    pub keeps: &'static str,
    /// Which sample the rule reads to decide where the line goes.
    pub measured_on: String,
    pub rule: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RulesPreview {
    /// Gates the rules would move, one per specimen.
    pub would_move: Vec<Move>,
    /// How many of those are worth opening by hand before applying: weak, or
    /// unable to reach the band the rule asked for.
    pub needs_review: usize,
    /// Gates already where their rule wants them.
    pub already_in_place: Vec<Kept>,
    /// Specimens the rules calibrate from, left alone.
    pub references: Vec<Kept>,
    pub not_positioned: Vec<NotPositioned>,
    pub next: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Move {
    pub gate: String,
    pub specimen: String,
    /// The sample the rule read.
    pub measured_on: String,
    /// Where the line was, and where it would go, on the rule's parameter -
    /// in the units the plots are drawn in.
    pub from: f64,
    pub to: f64,
    pub confidence: f64,
    /// What holds the confidence down most.
    pub weakest: Option<&'static str>,
    /// Whether the placement lands inside the band the rule asks for.
    pub in_band: bool,
    /// The percentage of the reference population the moved gate admits.
    pub admits_percent: f64,
    pub review: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Kept {
    pub gate: String,
    pub specimen: String,
    pub admits_percent: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotPositioned {
    pub gate: String,
    pub sample: Option<String>,
    pub reason: String,
}

impl Session {
    fn rules_or_refuse(&self) -> Result<&crate::gate_rules::rule_store::RuleStore, Refusal> {
        self.rules.as_ref().ok_or_else(|| {
            failed(format!(
                "this workspace has no rules: the Gate Rules tab saves them as {} in the \
                 workspace's {} folder",
                crate::workspace::RULES_FILE,
                crate::workspace::RULES_DIR
            ))
        })
    }

    /// The rules the workspace holds, as the Gate Rules tab lists them.
    pub fn rules_view(&self) -> Result<RulesView, Refusal> {
        let store = self.rules_or_refuse()?;
        let file = match &self.parts.rules {
            super::PartState::Loaded { file } => Some(file.clone()),
            _ => None,
        };
        Ok(RulesView {
            file,
            specimen_column: store.pairing.sample_id_column.to_string(),
            sample_type_column: store.pairing.sample_type_column.to_string(),
            rules: store
                .entries()
                .iter()
                .map(|entry| RuleRow {
                    population: entry.target.describe(),
                    parameter: entry.rule.parameter.to_string(),
                    keeps: match entry.rule.bound {
                        Bound::Above => "above the line",
                        Bound::Below => "below the line",
                    },
                    measured_on: match &entry.rule.measured_on {
                        MeasuredOn::Partner(kind) => {
                            format!("the specimen's {kind} sample")
                        }
                        MeasuredOn::Itself => "the sample being gated".to_string(),
                        MeasuredOn::File(file) => format!("the file {}", self.sample_name(file)),
                    },
                    rule: entry.rule.rule.describe(),
                })
                .collect(),
            reference_overrides: store.references().len(),
        })
    }

    /// Run every rule and say what it would place, moving nothing. The
    /// placements are kept for [`Session::apply_previewed_rules`].
    pub fn preview_rules(&mut self) -> Result<RulesPreview, Refusal> {
        let rules = self.rules_or_refuse()?.clone();
        let inputs = RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            &rules,
        );
        let outcome = run_rules(&self.gates, &inputs, |_| {}, &AtomicBool::new(false));
        let record = crate::review::RunRecord::of_run(
            &outcome.report,
            &outcome.placements,
            &inputs.rules,
            &self.metadata,
        );
        let report = outcome.report;

        let would_move: Vec<Move> = report
            .positioned
            .iter()
            .map(|p| {
                let review = p.confidence < REVIEW_FLOOR || !p.in_band;
                Move {
                    gate: describe(&p.gate, p.parent_gate.as_deref()),
                    specimen: p.specimen.to_string(),
                    measured_on: self.sample_name(&p.measured_on),
                    from: round(p.from, 4),
                    to: round(p.to, 4),
                    confidence: round(p.confidence, 3),
                    weakest: p.weakest,
                    in_band: p.in_band,
                    admits_percent: round(p.achieved * 100.0, 3),
                    review,
                }
            })
            .collect();
        let kept = |list: &[crate::gate_rules::autogate::Unchanged]| -> Vec<Kept> {
            list.iter()
                .map(|k| Kept {
                    gate: describe(&k.gate, k.parent_gate.as_deref()),
                    specimen: k.specimen.to_string(),
                    admits_percent: round(k.achieved * 100.0, 3),
                })
                .collect()
        };
        let preview = RulesPreview {
            needs_review: would_move.iter().filter(|m| m.review).count(),
            already_in_place: kept(&report.unchanged),
            references: kept(&report.reference),
            not_positioned: report
                .skipped
                .iter()
                .map(|s| NotPositioned {
                    gate: if s.gate.is_empty() {
                        "every rule".to_string()
                    } else {
                        describe(&s.gate, s.parent_gate.as_deref())
                    },
                    sample: (!s.file.is_empty()).then(|| self.sample_name(&s.file)),
                    reason: s.reason.clone(),
                })
                .collect(),
            next: if would_move.is_empty() {
                "nothing would move".to_string()
            } else {
                "nothing has moved yet: apply_rule_placements applies these to the working \
                 copy, and save_gating saves it"
                    .to_string()
            },
            would_move,
        };
        self.pending = Some(Pending {
            snapshot: self.gates.clone(),
            placements: outcome.placements,
            record,
        });
        Ok(preview)
    }

    /// Apply what the last preview proposed to this session's gates. Nothing
    /// is written to disk.
    pub fn apply_previewed_rules(&mut self) -> Result<String, Refusal> {
        let Some(pending) = self.pending.take() else {
            return Err(failed(
                "there is no preview to apply: run preview_rules first",
            ));
        };
        if !self.gates.unchanged_since(&pending.snapshot) {
            return Err(failed(
                "the gates have changed since the preview, so its placements no longer apply: \
                 run preview_rules again",
            ));
        }
        // The whole run is one step of the working copy, as in the app.
        let before = self.working_state();
        crate::gate_rules::autogate::apply_placements(&mut self.gates, &pending.placements);
        self.edited(before);
        // Kept for reviewing the run, as the app keeps it.
        let kept = match pending.record.applied(&self.folder) {
            Ok(_) => String::new(),
            Err(e) => format!(" (the run's record could not be kept for review: {e})"),
        };
        Ok(format!(
            "{} placement(s) applied to the working copy; nothing is saved until save_gating \
             (undo takes them back){kept}",
            pending.placements.len()
        ))
    }

    /// A file's name in the program, from its gating id.
    fn sample_name(&self, file: &Arc<str>) -> String {
        let names: HashMap<&Arc<str>, &Arc<str>> = self
            .metadata
            .file_name_to_gating_id()
            .iter()
            .map(|(name, id)| (id, name))
            .collect();
        names
            .get(file)
            .map(|n| n.to_string())
            .unwrap_or_else(|| file.to_string())
    }
}
