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
        // A gate that follows another has no parameter of its own and reads
        // its own sample: whatever was given there means nothing, so it is
        // not kept to confuse the list.
        if let Rule::FromAnotherGate(from) = &mut rule.rule {
            rule.parameter = Arc::from("");
            rule.measured_on = MeasuredOn::Itself;
            for edge in &mut from.edges {
                let channel = self.channel_named(&edge.parameter)?;
                if *channel != *edge.parameter {
                    notes.push(format!(
                        "parameter {} is the channel {channel}",
                        edge.parameter
                    ));
                }
                edge.parameter = channel;
            }
        }
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
        if let Rule::FromAnotherGate(from) = &rule.rule {
            return self.check_follow(target, from);
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

    /// Refuses a rule from another gate that could never place its gate: an
    /// anchor that is not one gate, the gate itself, a shape it cannot take,
    /// or an edge on a parameter one of the two is not drawn on.
    fn check_follow(
        &self,
        target: &RuleTarget,
        from: &crate::gate_rules::rule::FromGateRule,
    ) -> Result<(), Refusal> {
        use crate::gate_rules::autogate::anchor_gate;
        if let Some(problem) = from.problem() {
            return Err(failed(problem));
        }
        let (nodes, _) = self.population_facts();
        let mine: Vec<(crate::gates::gate_store::GateId, (String, String))> = nodes
            .iter()
            .filter_map(|node| self.target_of(node))
            .filter(|(here, _, _)| {
                here.gate == target.gate
                    && (target.parent.is_none() || target.parent == here.parent)
            })
            .map(|(_, id, params)| (id, params))
            .collect();
        let anchor = |named: &RuleTarget| {
            let id = anchor_gate(&self.gates, named).map_err(failed)?;
            // Compared as gates: a quadrant's corners are one gate.
            let identity = |id: &crate::gates::gate_store::GateId| {
                self.gates
                    .registered_gate(id)
                    .map(|g| g.get_id())
                    .unwrap_or_else(|| id.clone())
            };
            if mine.iter().any(|(own, _)| identity(own) == identity(&id)) {
                return Err(failed(format!(
                    "{} cannot follow itself - name the gate it takes its position from",
                    target.describe()
                )));
            }
            self.gates
                .registered_gate(&id)
                .map(|gate| (id, gate))
                .ok_or_else(|| failed(format!("{} has no gate", named.describe())))
        };
        let same_axes = |a: &(String, String), b: (&str, &str)| {
            (a.0 == b.0 && a.1 == b.1) || (a.0 == b.1 && a.1 == b.0)
        };
        if let Some(named) = &from.same_shape_as {
            let (_, theirs) = anchor(named)?;
            let (tx, ty) = theirs.get_params();
            for (id, params) in &mine {
                if !same_axes(params, (&tx, &ty)) {
                    return Err(failed(format!(
                        "{} is drawn on {tx} and {ty}, and {} on {} and {}, so it cannot take                          that shape",
                        named.describe(),
                        target.describe(),
                        params.0,
                        params.1
                    )));
                }
                let here = self.gates.registered_gate(id);
                if here.is_some_and(|g| g.is_composite() != theirs.is_composite()) {
                    return Err(failed(format!(
                        "one of {} and {} is a quadrant and the other is not, so one cannot                          take the other's shape",
                        target.describe(),
                        named.describe()
                    )));
                }
            }
        }
        for edge in &from.edges {
            let (_, theirs) = anchor(&edge.anchor)?;
            let (tx, ty) = theirs.get_params();
            if *edge.parameter != *tx && *edge.parameter != *ty {
                return Err(failed(format!(
                    "{} is drawn on {tx} and {ty}, so it has no edge on {}",
                    edge.anchor.describe(),
                    edge.parameter
                )));
            }
            for (own, params) in &mine {
                if *edge.parameter != *params.0 && *edge.parameter != *params.1 {
                    return Err(failed(format!(
                        "{} is drawn on {} and {}, so it has no edge on {} to set",
                        target.describe(),
                        params.0,
                        params.1,
                        edge.parameter
                    )));
                }
                if self
                    .gates
                    .registered_gate(own)
                    .is_some_and(|g| g.is_composite())
                {
                    return Err(failed(format!(
                        "{} is a quadrant, whose edges are its lines - it follows another                          quadrant with same_shape_as",
                        target.describe()
                    )));
                }
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
            problems.extend(
                crate::gate_rules::autogate::anchor_problems(&self.gates, store)
                    .into_iter()
                    .filter(|p| p.target == *target)
                    .map(|p| p.reason),
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
