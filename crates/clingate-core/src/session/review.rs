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

impl Session {
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
