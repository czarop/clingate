//! The reports to send on: the test report on the workspace as it stands
//! (see [`crate::test_report`]) and the report on one rule's last run (see
//! [`crate::rule_report`]).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::{Refusal, Session, failed};
use crate::gate_rules::run::RunInputs;
use crate::rule_report::{Picture, RuleReportJob, rule_report};
use crate::test_report::{TestJob, keep, stamped_now, test_report};

/// A test report, kept in the workspace.
#[derive(Debug, Clone, Serialize)]
pub struct TestReportAnswer {
    pub kept_at: PathBuf,
    pub report: String,
    pub next: &'static str,
}

/// A report on one rule's last run, kept in the workspace.
#[derive(Debug, Clone, Serialize)]
pub struct RuleReportAnswer {
    pub kept_at: PathBuf,
    pub report: String,
    /// Named as the report names them.
    #[serde(skip)]
    pub pictures: Vec<Picture>,
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

    /// What a report on the rule of `population` reads: the workspace as it
    /// stands, made `made`.
    pub fn rule_job(&self, population: &str, made: String) -> Result<RuleReportJob, Refusal> {
        let (node, _) = self.one_population(population)?;
        let gate_id = self
            .gates
            .gate_for_node(&node)
            .cloned()
            .ok_or_else(|| failed("that population has no gate"))?;
        Ok(RuleReportJob {
            workspace: self.workspace_name(),
            made,
            folder: self.folder.clone(),
            gates: self.gates.clone(),
            inputs: RunInputs::assemble(
                Some(&self.files),
                &self.compensation,
                &self.metadata,
                &self.axes.settings,
                &self.rules.clone().unwrap_or_default(),
            ),
            axes: self.axes.settings.clone(),
            gate_id: gate_id.to_string(),
        })
    }

    /// Report how the rule of `population` did in the last applied run, on
    /// every sample, with each sample's plot, and keep it in the workspace.
    /// Changes nothing else.
    pub fn rule_report(&self, population: &str) -> Result<RuleReportAnswer, Refusal> {
        let (made, stamp) = stamped_now();
        let job = self.rule_job(population, made)?;
        let report = rule_report(&job, &AtomicBool::new(false), |_, _| {}).map_err(failed)?;
        let kept_at = crate::rule_report::keep(&self.folder, &report, &stamp)
            .map_err(|e| failed(format!("the report could not be kept: {e}")))?;
        Ok(RuleReportAnswer {
            kept_at,
            report: report.markdown,
            pictures: report.pictures,
            next: "the rule's report, kept with its pictures in kept_at's folder for the user \
                   to send on; the pictures are in the order its Pictures section lists them. \
                   Read which samples the rule placed badly, flagged or not, and tell the user \
                   what stands out. Nothing in the workspace changed",
        })
    }

    fn workspace_name(&self) -> String {
        self.folder.file_name().map_or_else(
            || self.folder.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}
