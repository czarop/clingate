//! Telling spillover errors from autofluorescence left in, on the
//! workspace's data - see [`crate::spillover`]. Nothing is moved or written.

use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::{Refusal, Session, failed};
use crate::gate_rules::rule_store::RuleStore;
use crate::spillover::Finding;

/// Specimens the check reads by default, and at most.
pub const SPILLOVER_SPECIMENS: usize = 24;
pub const SPILLOVER_SPECIMENS_MAX: usize = 200;

/// What the check found, for the tools.
#[derive(Debug, Clone, Serialize)]
pub struct SpilloverAnswer {
    pub population: String,
    pub specimens_read: usize,
    pub specimens: usize,
    pub stained_files: usize,
    pub unstained_files: usize,
    /// Each finding in a sentence, with what to do.
    pub says: Vec<String>,
    pub findings: Vec<Finding>,
    pub problems: Vec<String>,
    pub next: &'static str,
}

impl Session {
    /// Look for spillover errors and autofluorescence left in, in
    /// `population`, across up to `specimens` specimens spread through the
    /// dataset, with their unstained files where they have them.
    pub fn check_spillover(
        &self,
        population: &str,
        specimens: Option<usize>,
    ) -> Result<SpilloverAnswer, Refusal> {
        let (node, facts) = self.one_population(population)?;
        let rules = self.rules.clone().unwrap_or_else(RuleStore::default);
        let wanted = specimens
            .unwrap_or(SPILLOVER_SPECIMENS)
            .clamp(1, SPILLOVER_SPECIMENS_MAX);
        let (inputs, read, total) = self.some_specimens(&rules, wanted, &[]);
        let job = crate::spillover::job_for(&self.gates, &inputs, &node).map_err(failed)?;
        let checked =
            crate::spillover::run(&job, &AtomicBool::new(false), |_, _| {}).map_err(failed)?;
        Ok(SpilloverAnswer {
            population: facts.full_path(),
            specimens_read: read,
            specimens: total,
            stained_files: checked.stained,
            unstained_files: checked.unstained,
            says: checked.findings.iter().map(Finding::says).collect(),
            findings: checked.findings,
            problems: checked.problems,
            next: "tell the user each finding and what to do about it: a spillover value to \
                   change in the compensation, samples whose dye to check, or autofluorescence \
                   to extract in the unmixing, with the channel to look in on the unstained. \
                   An empty list means nothing leans by a negative's width or more",
        })
    }
}
