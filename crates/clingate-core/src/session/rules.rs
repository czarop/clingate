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
    /// The events it read, kept with it.
    events: crate::review::events::KeptEvents,
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
    /// Why the rule cannot run as written, if it cannot: a name the run will
    /// not find, a gate that is not there. Rewrite it with update_rule.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
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
    /// Phenotype rules whose reference edge on a marker lies within that
    /// marker's negative, a line each: ask the user whether to pin it there.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub could_pin: Vec<String>,
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
    pub(super) fn rules_or_refuse(&self) -> Result<&crate::gate_rules::rule_store::RuleStore, Refusal> {
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
                    problems: self.rule_problems(&entry.target, &entry.rule),
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
                let review = p.needs_review(REVIEW_FLOOR);
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
        let mut could_pin: Vec<String> = Vec::new();
        for placed in &report.positioned {
            for marker in placed.phenotype.iter().flat_map(|read| &read.could_pin) {
                let line = format!(
                    "{}: on the reference its edge on {marker} lies within the negative's own \
                     spread - drawn against the negative. Ask the user whether to pin it there \
                     (\"pinned\": [\"{marker}\"] in the rule); change nothing until they say",
                    describe(&placed.gate, placed.parent_gate.as_deref())
                );
                if !could_pin.contains(&line) {
                    could_pin.push(line);
                }
            }
        }
        let preview = RulesPreview {
            could_pin,
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
            events: outcome.events,
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
        crate::gate_rules::autogate::apply_placements(
            &mut self.gates,
            &pending.placements,
            self.metadata.metadata(),
        );
        self.edited(before);
        // Kept for reviewing the run, as the app keeps it.
        let kept = match pending.record.applied(&self.folder, &pending.events) {
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
    pub(crate) fn sample_name(&self, file: &Arc<str>) -> String {
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

    /// `candidates` with their markers and samples named as a run reads them,
    /// each checked against `target`'s gate.
    pub(super) fn resolved_candidates(
        &self,
        target: &crate::gate_rules::rule_store::RuleTarget,
        candidates: &[crate::gate_rules::rule_store::GateRule],
    ) -> Result<Vec<crate::gate_rules::rule_store::GateRule>, Refusal> {
        let mut resolved = Vec::with_capacity(candidates.len());
        for (at, candidate) in candidates.iter().enumerate() {
            let (rule, _) = self
                .resolve_rule(candidate.clone())
                .map_err(|e| failed(format!("candidate {}: {e}", at + 1)))?;
            self.check_target(target, &rule)
                .map_err(|e| failed(format!("candidate {}: {e}", at + 1)))?;
            resolved.push(rule);
        }
        Ok(resolved)
    }

    /// Try up to four candidate rules for one population's gate on the files
    /// as they are, moving nothing - see [`crate::gate_rules::trial`]. The
    /// workspace's pairing and hand-picked references are used; its other
    /// rules are not touched.
    pub fn try_rules(
        &self,
        population: &str,
        candidates: &[crate::gate_rules::rule_store::GateRule],
        max_rows: Option<usize>,
    ) -> Result<TrialAnswer, Refusal> {
        let (node, _) = self.one_population(population)?;
        let (target, _, _) = self
            .target_of(&node)
            .ok_or_else(|| failed("that population has no gate"))?;
        let resolved = self.resolved_candidates(&target, candidates)?;
        let candidates = resolved.as_slice();
        let rules = self.rules.clone().unwrap_or_default();
        let inputs = RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            &rules,
        );
        let trial = crate::gate_rules::trial::try_rules(
            &self.gates,
            &inputs,
            &target,
            candidates,
            &AtomicBool::new(false),
        )
        .map_err(failed)?;

        let r = |v: Option<f64>| v.map(|v| round(v, 5));
        let rows_total = trial.rows.len();
        // Most telling first: flagged under any candidate, then where the
        // candidates disagree most about what the gate holds.
        let mut rows = trial.rows;
        let disagreement = |row: &crate::gate_rules::trial::Row| {
            let held: Vec<f64> = row
                .candidates
                .iter()
                .filter_map(|c| c.holds)
                .chain(row.current_holds)
                .collect();
            let lo = held.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = held.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if hi >= lo { hi - lo } else { 0.0 }
        };
        rows.sort_by(|a, b| {
            let flagged = |row: &crate::gate_rules::trial::Row| {
                row.candidates
                    .iter()
                    .any(|c| c.flagged || c.what == "not placed")
            };
            flagged(b)
                .cmp(&flagged(a))
                .then(disagreement(b).total_cmp(&disagreement(a)))
        });
        let shown = max_rows.unwrap_or(TRIAL_ROWS).clamp(1, TRIAL_ROWS_MAX);
        let rows = rows
            .into_iter()
            .take(shown)
            .map(|row| TrialRow {
                sample: self.sample_name(&Arc::from(row.file.as_str())),
                sample_type: row.sample_type,
                specimen: row.specimen,
                current_holds: r(row.current_holds),
                candidates: row
                    .candidates
                    .into_iter()
                    .map(|mut c| {
                        c.holds = r(c.holds);
                        c.line = r(c.line);
                        c.confidence = c.confidence.map(|v| round(v, 3));
                        c
                    })
                    .collect(),
            })
            .collect();
        Ok(TrialAnswer {
            gate: trial.gate,
            specimen_column: rules.pairing.sample_id_column.to_string(),
            sample_type_column: rules.pairing.sample_type_column.to_string(),
            files_read: trial.files_read,
            problems: trial.problems,
            current: trial.current,
            candidates: candidates
                .iter()
                .zip(trial.candidates)
                .map(|(rule, summary)| TrialCandidate {
                    rule: crate::session::describe_rule(rule),
                    summary,
                })
                .collect(),
            rows_total,
            rows,
            next: "holds is the fraction of each sample's parent the gate would hold - compare \
                   each candidate's spread per sample type with the gates as they stand \
                   (current) and with the hand-gated reference; look at the flagged rows. \
                   Nothing has moved. update_rule writes a candidate to the rules file only when \
                   the user says to",
        })
    }
}

/// Rows a trial lists by default, and at most.
pub const TRIAL_ROWS: usize = 30;
pub const TRIAL_ROWS_MAX: usize = 300;

/// One candidate, and what it did.
#[derive(Debug, Clone, Serialize)]
pub struct TrialCandidate {
    pub rule: String,
    pub summary: crate::gate_rules::trial::Summary,
}

/// One sample under every candidate.
#[derive(Debug, Clone, Serialize)]
pub struct TrialRow {
    pub sample: String,
    pub sample_type: Option<String>,
    pub specimen: Option<String>,
    pub current_holds: Option<f64>,
    pub candidates: Vec<crate::gate_rules::trial::Cell>,
}

/// A trial, for the tools.
#[derive(Debug, Clone, Serialize)]
pub struct TrialAnswer {
    pub gate: String,
    pub specimen_column: String,
    pub sample_type_column: String,
    pub files_read: usize,
    pub problems: Vec<String>,
    /// What the gate holds as the gates stand now, by sample type.
    pub current: Vec<crate::gate_rules::trial::TypeSpread>,
    pub candidates: Vec<TrialCandidate>,
    pub rows_total: usize,
    pub rows: Vec<TrialRow>,
    pub next: &'static str,
}
