//! The test report on the workspace as it stands: see [`crate::test_report`].

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::{Refusal, Session, failed};
use crate::gate_rules::run::RunInputs;
use crate::test_report::{TestJob, keep, stamped_now, test_report};

/// A test report, kept in the workspace.
#[derive(Debug, Clone, Serialize)]
pub struct TestReportAnswer {
    pub kept_at: PathBuf,
    pub report: String,
    pub next: &'static str,
}

impl Session {
    /// What the report reads: the workspace as it stands, made `made`.
    pub fn test_job(&self, made: String) -> Result<TestJob, Refusal> {
        let rules = self.rules_or_refuse()?;
        Ok(TestJob {
            workspace: self.workspace_name(),
            made,
            gates: self.gates.clone(),
            inputs: RunInputs::assemble(
                Some(&self.files),
                &self.compensation,
                &self.metadata,
                &self.axes.settings,
                rules,
            ),
            settings: self.pick_settings(),
        })
    }

    /// Pick a rule for every gate, score the picks and the rules as they
    /// stand against the hand gating, and keep the report in the workspace.
    /// Changes nothing else.
    pub fn test_report(&self) -> Result<TestReportAnswer, Refusal> {
        let (made, stamp) = stamped_now();
        let job = self.test_job(made)?;
        let report = test_report(&job, &AtomicBool::new(false), |_| {}).map_err(failed)?;
        let kept_at = keep(&self.folder, &report, &stamp)
            .map_err(|e| failed(format!("the report could not be kept: {e}")))?;
        Ok(TestReportAnswer {
            kept_at,
            report,
            next: "the test report, also kept at kept_at for the user to send on: read it for \
                   what the picker did on each gate and which samples it got wrong, and tell \
                   the user what stands out. Nothing in the workspace changed",
        })
    }

    fn workspace_name(&self) -> String {
        self.folder.file_name().map_or_else(
            || self.folder.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}
