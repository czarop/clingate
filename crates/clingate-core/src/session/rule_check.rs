//! A rule as it is written, checked against the workspace it is written for.
//!
//! The rules file holds names the way a run reads them: a parameter by its
//! channel, a reference file by its row in the metadata. Claude - and anyone
//! else writing a rule by hand - names things the way the tools show them: a
//! marker, a file name. Nothing turned one into the other, and nothing checked,
//! so a rule naming `CD19` or `QC1_..._FS.fcs` was written without a murmur
//! and then failed on every file of every run as "no reference sample to
//! measure". Here the names are resolved once, when the rule is written, and
//! whatever cannot be resolved is refused with what would be.

use std::sync::Arc;

use super::{Refusal, Session, failed};
use crate::gate_rules::rule::Rule;
use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleTarget, SamplePairing};

impl Session {
    /// `rule` with every name put the way the rules file holds it, and a line
    /// for each name that was changed.
    pub(crate) fn resolve_rule(
        &self,
        mut rule: GateRule,
    ) -> Result<(GateRule, Vec<String>), Refusal> {
        let mut notes = Vec::new();
        if !rule.parameter.trim().is_empty() {
            let channel = self.channel_named(&rule.parameter)?;
            if *channel != *rule.parameter {
                notes.push(format!(
                    "parameter {} is the channel {channel}",
                    rule.parameter
                ));
            }
            rule.parameter = channel;
        }
        if let Rule::MatchThePhenotype(wanted) = &mut rule.rule {
            let mut markers = Vec::with_capacity(wanted.markers.len());
            for marker in &wanted.markers {
                let channel = self.channel_named(marker)?;
                if *channel != **marker {
                    notes.push(format!("marker {marker} is the channel {channel}"));
                }
                markers.push(channel);
            }
            wanted.markers = markers;
        }
        match &rule.measured_on {
            MeasuredOn::File(named) => {
                let id = self.metadata_row_named(named)?;
                if *id != **named {
                    notes.push(format!("the reference {named} is the file {id}"));
                }
                rule.measured_on = MeasuredOn::File(id);
            }
            MeasuredOn::Partner(kind) => {
                let types = self.sample_types();
                if !types.iter().any(|t| **t == **kind) {
                    return Err(failed(format!(
                        "no file has the sample type {kind} under \"{}\" - the types in this \
                         workspace are: {}",
                        self.pairing().sample_type_column,
                        list(&types)
                    )));
                }
            }
            MeasuredOn::Itself => {}
        }
        Ok((rule, notes))
    }

    /// Refuses a rule for gates that are not there, or that it cannot move.
    pub(crate) fn check_target(&self, target: &RuleTarget, rule: &GateRule) -> Result<(), Refusal> {
        let (nodes, _) = self.population_facts();
        let mut same_name = Vec::new();
        let mut matched = Vec::new();
        for node in &nodes {
            let Some((here, _, params)) = self.target_of(node) else {
                continue;
            };
            if here.gate != target.gate {
                continue;
            }
            if target.parent.is_none() || target.parent == here.parent {
                matched.push((here.clone(), params));
            }
            same_name.push(here);
        }
        if same_name.is_empty() {
            return Err(failed(format!(
                "there is no gate called {} - a rule names a gate as list_populations shows it",
                target.gate
            )));
        }
        if matched.is_empty() {
            return Err(failed(format!(
                "no {} gate is drawn under {} - its parents are: {}. A parent is named as \
                 shortly as it can be while naming only itself, so under a subset it can be \
                 a path like 'CD161+Va7.2+ / CD4+CD8-'.",
                target.gate,
                target.parent.as_deref().unwrap_or(""),
                list(
                    &same_name
                        .iter()
                        .map(|t| Arc::from(t.parent.as_deref().unwrap_or("(the top)")))
                        .collect::<Vec<Arc<str>>>()
                )
            )));
        }
        if let Rule::MatchThePhenotype(_) = &rule.rule {
            if !matches!(rule.measured_on, MeasuredOn::File(_)) {
                return Err(failed(
                    "a phenotype rule describes the population from one sample gated by hand, so \
                     it is measured on one named file: {\"File\": \"<sample>\"}",
                ));
            }
            return Ok(());
        }
        for (here, (x, y)) in &matched {
            if *rule.parameter != **x && *rule.parameter != **y {
                return Err(failed(format!(
                    "{} is drawn on {x} and {y}, so a rule for it slides its edge along one of \
                     those - not {}",
                    here.describe(),
                    rule.parameter
                )));
            }
        }
        Ok(())
    }

    /// What is wrong with a rule already in the rules file, if anything - for
    /// a rules file written before these checks, or by hand.
    pub(crate) fn rule_problems(&self, target: &RuleTarget, rule: &GateRule) -> Vec<String> {
        let mut problems = Vec::new();
        match self.resolve_rule(rule.clone()) {
            Err(refused) => problems.push(refused.to_string()),
            Ok((_, notes)) => problems.extend(
                notes
                    .into_iter()
                    .map(|n| format!("names things the run cannot find: {n}")),
            ),
        }
        if let Err(refused) = self.check_target(target, rule) {
            problems.push(refused.to_string());
        }
        if let Some(store) = &self.rules {
            let described = target.describe();
            problems.extend(
                crate::gate_rules::autogate::linked_conflicts(&self.gates, store)
                    .into_iter()
                    .filter(|c| c.rules.contains(&described))
                    .map(|c| c.reason),
            );
            if crate::gate_rules::autogate::rules_reaching_nothing(&self.gates, store)
                .contains(target)
                && problems.is_empty()
            {
                problems.push(
                    "every gate this rule names has a more specific rule, so it reaches none"
                        .to_string(),
                );
            }
        }
        problems
    }

    fn channel_named(&self, name: &str) -> Result<Arc<str>, Refusal> {
        Ok(self.one_parameter(name)?.fluoro.clone())
    }

    /// A file's row in the metadata: the name as given if it is one, else the
    /// one sample the words name.
    fn metadata_row_named(&self, named: &str) -> Result<Arc<str>, Refusal> {
        if self.metadata.metadata().contains_key(named) {
            return Ok(Arc::from(named));
        }
        let stub = self.one_sample(named)?;
        self.gating_id(stub)
            .ok_or_else(|| failed(format!("{} has no row in the metadata", stub.name)))
    }

    fn pairing(&self) -> SamplePairing {
        self.rules
            .as_ref()
            .map(|r| r.pairing.clone())
            .unwrap_or_default()
    }

    /// Every sample type some file in the workspace has.
    fn sample_types(&self) -> Vec<Arc<str>> {
        let pairing = self.pairing();
        let mut types: Vec<Arc<str>> = self
            .metadata
            .metadata()
            .values()
            .filter_map(|columns| pairing.sample_type_of(columns))
            .collect();
        types.sort();
        types.dedup();
        types
    }
}

fn list(names: &[Arc<str>]) -> String {
    if names.is_empty() {
        return "none".to_string();
    }
    names
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
