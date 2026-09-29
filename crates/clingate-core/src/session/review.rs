//! Reviewing a rules run, for the tools for Claude: report a badly placed
//! gate, and mark the run reviewed - through the same functions the app's
//! report dialog and Gate Rules tab call (see [`crate::review`]).

use std::path::PathBuf;

use serde::Serialize;

use super::{Refusal, Session, failed};
use crate::gate_rules::run::RunInputs;
use crate::review::report::{self, Decision, Problem, ReportRequest};

#[derive(Debug, Clone, Serialize)]
pub struct Reported {
    pub file: PathBuf,
    pub gate: String,
    pub sample: String,
    pub problem: &'static str,
    /// What the rules did for this gate on this sample, in a line.
    pub rule_did: String,
    /// How many of the population's events the report keeps, on this sample
    /// and the sample the rule read.
    pub events_kept: usize,
    pub reference_events_kept: Option<usize>,
    pub next: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reviewed {
    pub accepted: usize,
    pub reported: usize,
    pub moved_without_a_report: usize,
    pub left_alone: usize,
    pub review_file: PathBuf,
    /// Where it was copied in the review library, when one is set.
    pub library_copy: Option<PathBuf>,
}

/// A run's assessment, for the tools: the worst flags, and every gate.
#[derive(Debug, Clone, Serialize)]
pub struct RunAssessment {
    pub run_applied_at: String,
    pub placements: usize,
    /// How many placements are in each pile of the Review tab: needs a look,
    /// passed, reported, changed since.
    pub piles: Vec<(crate::review::board::Pile, usize)>,
    /// The worst [`FLAGS_SHOWN`] of those needing a look, worst first -
    /// flagged, and not yet reported, changed or judged to look right.
    pub flags: Vec<crate::review::assess::Flag>,
    pub gates: Vec<crate::review::assess::GateSummary>,
    pub next: &'static str,
}

/// How many flags an assessment hands the tools at once.
pub const FLAGS_SHOWN: usize = 40;

impl Session {
    fn kept_run(&self) -> Result<crate::review::RunRecord, Refusal> {
        crate::review::RunRecord::load(&self.folder)
            .map_err(failed)?
            .ok_or_else(|| {
                failed(
                    "no rules run has been applied in this workspace: preview_rules and \
                     apply_rule_placements first, or run the rules in the app",
                )
            })
    }

    /// Which placements of the last applied run need a look, and why - as
    /// the Gate Rules tab's review list shows them.
    pub fn assess_run(&self) -> Result<RunAssessment, Refusal> {
        use crate::review::board::Pile;
        let run = self.kept_run()?;
        let assessment =
            crate::review::assess::assess(&run, Some((&self.gates, self.metadata.metadata())));
        let board =
            crate::review::board::board_in(&self.folder, &self.gates, self.metadata.metadata())
                .map_err(failed)?
                .ok_or_else(|| failed("no rules run has been applied in this workspace"))?;
        Ok(RunAssessment {
            run_applied_at: assessment.run_applied_at,
            placements: board.entries.len(),
            piles: Pile::ALL.iter().map(|p| (*p, board.count(*p))).collect(),
            flags: board
                .pile(Pile::NeedsALook)
                .filter_map(|e| e.flag.clone())
                .take(FLAGS_SHOWN)
                .collect(),
            gates: assessment.gates,
            next: "compare_to_peers shows one flagged sample's distribution beside its peers'; \
                   show the user the flags and let them decide what is wrong",
        })
    }

    /// Mark a placement of the last run as looking right despite its flag -
    /// or take the mark back - as the Review tab's Looks right does. Only on
    /// the user's word.
    pub fn mark_looks_right(
        &self,
        population: &str,
        sample: &str,
        looks_right: bool,
    ) -> Result<String, Refusal> {
        let run = self.kept_run()?;
        let (node, _) = self.one_population(population)?;
        let gate_id = self
            .gates
            .gate_for_node(&node)
            .cloned()
            .ok_or_else(|| failed("that population has no gate"))?;
        let stub = self.one_sample(sample)?;
        let id = self
            .metadata
            .file_name_to_gating_id()
            .get(&stub.name)
            .cloned()
            .ok_or_else(|| failed(format!("{}: no metadata row names this sample", stub.name)))?;
        let on_board = run
            .placed
            .iter()
            .any(|p| *p.gate_id == *gate_id && *p.sample.id == *id)
            || run
                .kept
                .iter()
                .any(|k| k.met_rule && *k.gate_id == *gate_id && *k.sample.id == *id);
        if !on_board {
            return Err(failed(
                "the last applied run did not place or keep this gate on this sample",
            ));
        }
        crate::review::board::LooksRight::set(&self.folder, &run, &gate_id, &id, looks_right)
            .map_err(failed)?;
        Ok(if looks_right {
            format!("{} on {}: marked as looking right", population, stub.name)
        } else {
            format!("{} on {}: the mark is taken back", population, stub.name)
        })
    }

    /// One sample's placement of a population's gate beside its peers', in
    /// numbers - from the last applied run.
    pub fn compare_to_peers(
        &self,
        population: &str,
        sample: &str,
    ) -> Result<crate::review::assess::PeerComparison, Refusal> {
        let run = self.kept_run()?;
        let (node, _) = self.one_population(population)?;
        let gate_id = self
            .gates
            .gate_for_node(&node)
            .cloned()
            .ok_or_else(|| failed("that population has no gate"))?;
        let stub = self.one_sample(sample)?;
        let id = self
            .metadata
            .file_name_to_gating_id()
            .get(&stub.name)
            .cloned()
            .ok_or_else(|| failed(format!("{}: no metadata row names this sample", stub.name)))?;
        crate::review::assess::compare_to_peers(&run, &gate_id, &id).map_err(failed)
    }

    /// The review library reviews are copied into - clingate's setting on
    /// this computer, read when the workspace opened.
    pub fn review_library(&self) -> Option<&std::path::Path> {
        self.review_library.as_deref()
    }

    /// Use another review library for this session, or none.
    pub fn set_review_library(&mut self, library: Option<PathBuf>) {
        self.review_library = library;
    }

    fn run_inputs(&self) -> RunInputs {
        RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            &self.rules.clone().unwrap_or_default(),
        )
    }

    /// Report the gate of `population` as badly placed on `sample`, as the
    /// app's report dialog does. Kept in the workspace's reviews folder; the
    /// correction is added when the workspace is next saved.
    pub fn report_placement(
        &self,
        population: &str,
        sample: &str,
        problem: &str,
        note: &str,
    ) -> Result<Reported, Refusal> {
        let problem = Problem::from_key(problem).ok_or_else(|| {
            failed(format!(
                "the problem is one of: {}",
                Problem::ALL
                    .iter()
                    .map(|p| format!("{} ({})", p.key(), p.describe()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let (node, facts) = self.one_population(population)?;
        let stub = self.one_sample(sample)?;
        let id = self
            .metadata
            .file_name_to_gating_id()
            .get(&stub.name)
            .cloned()
            .ok_or_else(|| failed(format!("{}: no metadata row names this sample", stub.name)))?;
        let request = ReportRequest {
            node,
            sample: id,
            problem,
            note: note.to_string(),
        };
        let made = report::gather(
            &self.folder,
            &request,
            &self.gates,
            &self.run_inputs(),
            &self.axes.settings,
        )
        .map_err(failed)?;
        let file = made.save(&self.folder).map_err(failed)?;
        Ok(Reported {
            file,
            gate: facts.full_path(),
            sample: stub.name.to_string(),
            problem: problem.describe(),
            rule_did: match &made.decision {
                Decision::Placed(p) => format!(
                    "the rule moved it from {} to {} with confidence {:.2} (weakest: {})",
                    p.from.map_or("-".into(), |v| format!("{v:.3}")),
                    p.to.map_or("-".into(), |v| format!("{v:.3}")),
                    p.confidence,
                    p.weakest.as_deref().unwrap_or("-")
                ),
                Decision::Kept(_) => "the rule left it where it was".into(),
                Decision::NotPlaced { why } => why.clone(),
            },
            events_kept: made.data.events_subsample.len(),
            reference_events_kept: made
                .reference_data
                .as_ref()
                .map(|d| d.events_subsample.len()),
            next: "fix the gate if the user wants it fixed; the fix is recorded when the \
                   workspace is saved",
        })
    }

    /// Mark the last applied rules run reviewed, as the Gate Rules tab's
    /// button does: every placement not reported and still where the rule
    /// put it counts as accepted.
    pub fn mark_run_reviewed(&self) -> Result<Reviewed, Refusal> {
        let (review, copied) = report::mark_reviewed(
            &self.folder,
            &self.gates,
            self.metadata.metadata(),
            self.review_library.as_deref(),
        )
        .map_err(failed)?;
        Ok(Reviewed {
            accepted: review.accepted(),
            reported: review.reported(),
            moved_without_a_report: review.moved_unreported(),
            left_alone: review.kept.len(),
            review_file: self
                .folder
                .join(crate::review::REVIEWS_DIR)
                .join(report::REVIEW_FILE),
            library_copy: copied,
        })
    }
}
