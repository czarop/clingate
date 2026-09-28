//! The rules, run as a preview whose placements are applied only when asked,
//! and the gating file written only when asked.
//!
//! A preview runs every rule the way the Gate Rules tab's button does and
//! keeps what it would place, moving nothing. Applying writes those
//! placements into this session's gates - and refuses if the gates have
//! changed since the preview, since the answers were measured on gates that
//! are no longer there. Writing the gating file is a third, separate step,
//! and never overwrites a file unless told to.

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

#[derive(Debug, Clone, Serialize)]
pub struct SavedGating {
    pub file: PathBuf,
    pub gates: usize,
}

impl Session {
    fn rules_or_refuse(&self) -> Result<&crate::gate_rules::rule_store::RuleStore, Refusal> {
        self.rules.as_ref().ok_or_else(|| {
            failed(format!(
                "this workspace has no rules: the Gate Rules tab saves them as {} beside the \
                 gating file",
                super::RULES_FILE
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
        let inputs = RunInputs {
            files: self
                .files
                .file_list()
                .iter()
                .map(|stub| (stub.name.clone(), stub.get_filepath().to_path_buf()))
                .collect(),
            compensation: self.compensation.clone(),
            names: self.metadata.file_name_to_gating_id().clone(),
            cofactors: RunInputs::cofactors_of(&self.axes.settings),
            metadata: self.metadata.metadata().clone(),
            rules,
        };
        let outcome = run_rules(&self.gates, &inputs, |_| {}, &AtomicBool::new(false));
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
                "nothing has moved yet: apply_rule_placements applies these to this session's \
                 gates, and save_gating writes them to a file"
                    .to_string()
            },
            would_move,
        };
        self.pending = Some(Pending {
            snapshot: self.gates.clone(),
            placements: outcome.placements,
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
        crate::gate_rules::autogate::apply_placements(&mut self.gates, &pending.placements);
        Ok(format!(
            "{} placement(s) applied to this session's gates; nothing is written to disk until \
             save_gating",
            pending.placements.len()
        ))
    }

    /// Write the gates, as they now stand, as an Omiq gating file in the
    /// workspace folder. `file_name` is a name, not a path; an existing file
    /// is never replaced unless `overwrite` says so.
    pub fn save_gating(&self, file_name: &str, overwrite: bool) -> Result<SavedGating, Refusal> {
        let name = file_name.trim();
        let plain = std::path::Path::new(name)
            .file_name()
            .is_some_and(|n| n == std::ffi::OsStr::new(name));
        if name.is_empty() || !plain || name.starts_with('.') {
            return Err(failed(
                "give a file name alone, with no folder: it is written into the workspace folder",
            ));
        }
        let name = if name.ends_with(".omiqgt") {
            name.to_string()
        } else {
            format!("{name}.omiqgt")
        };
        let path = self.folder.join(&name);
        if path.exists() && !overwrite {
            return Err(failed(format!(
                "{} already exists: ask the user whether to replace it, or choose another name",
                path.display()
            )));
        }
        let document = crate::omiq::serialise::to_omiq_document(
            &self.gates,
            self.metadata.metadata(),
            &self.axes.settings,
        )
        .map_err(failed)?;
        let text = serde_json::to_string_pretty(&document).map_err(failed)?;
        std::fs::write(&path, text).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok(SavedGating {
            file: path,
            gates: self.gates.gate_count(),
        })
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
