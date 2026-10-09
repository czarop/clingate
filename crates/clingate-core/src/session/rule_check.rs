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
        if rule.rule.reads_another_gate() {
            rule.parameter = Arc::from("");
            rule.measured_on = MeasuredOn::Itself;
        }
        if let Rule::NextToGate(next) = &mut rule.rule {
            next.parameter = self.channel_noted(&next.parameter, &mut notes)?;
        }
        if let Rule::FromAnotherGate(from) = &mut rule.rule {
            for edge in &mut from.edges {
                edge.parameter = self.channel_noted(&edge.parameter, &mut notes)?;
            }
        }
        if !rule.parameter.trim().is_empty() {
            rule.parameter = self.channel_noted(&rule.parameter, &mut notes)?;
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
            let mut pinned = Vec::with_capacity(wanted.pinned.len());
            for marker in &wanted.pinned {
                let channel = self.channel_named(marker)?;
                if *channel != **marker {
                    notes.push(format!("pinned marker {marker} is the channel {channel}"));
                }
                pinned.push(channel);
            }
            wanted.pinned = pinned;
        }
        if let Rule::ValleyOrSmear(either) = &mut rule.rule
            && let Some(named) = &either.smear_example
        {
            let id = self.metadata_row_named(named)?;
            if *id != **named {
                notes.push(format!("the smear example {named} is the file {id}"));
            }
            either.smear_example = Some(id);
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
        if let Rule::TailFraction(band) = &rule.rule
            && band.pool == crate::gate_rules::rule::Pool::Run
            && !matches!(
                rule.measured_on,
                MeasuredOn::Partner(_) | MeasuredOn::Itself
            )
        {
            return Err(failed(
                "a band counted on the whole run reads every file of one kind together - \
                 measured on {\"Partner\": \"FMX\"} for all the FMX files, or \"Itself\" for \
                 the gated samples - not one named file",
            ));
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
        if let Rule::NextToGate(next) = &rule.rule {
            return self.check_next_to(target, next);
        }
        if let Rule::MatchThePhenotype(wanted) = &rule.rule {
            if !matches!(rule.measured_on, MeasuredOn::File(_)) {
                return Err(failed(
                    "a phenotype rule describes the population from one sample gated by hand, so \
                     it is measured on one named file: {\"File\": \"<sample>\"}",
                ));
            }
            return check_pinned(wanted, &matched);
        }
        if matches!(rule.rule, Rule::ValleyOrSmear(_))
            && !matches!(rule.measured_on, MeasuredOn::File(_))
        {
            return Err(failed(
                "a valley-or-smear rule places every sample from one gated by hand, so it is \
                 measured on one named file: {\"File\": \"<sample>\"}",
            ));
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
        let fallback = match &rule.rule {
            Rule::ValleyOrSmear(either) => {
                either.valley().fallback_rule(&rule.parameter, rule.bound)
            }
            _ => None,
        };
        if let Some(fallback) = fallback {
            return self.check_follow(target, &fallback);
        }
        Ok(())
    }

    /// Refuses a rule next to another gate that could never place its gate:
    /// another gate that is not one gate, the gate itself, one not on the same
    /// plot, a shape that cannot be brought up to it, or a parameter the gate
    /// is not drawn on.
    fn check_next_to(
        &self,
        target: &RuleTarget,
        next: &crate::gate_rules::rule::NextToRule,
    ) -> Result<(), Refusal> {
        use crate::gate_rules::autogate::anchor_gate;
        if let Some(problem) = crate::gate_rules::rule::gap_problem(next.gap) {
            return Err(failed(problem.to_string()));
        }
        let anchor = anchor_gate(&self.gates, &next.anchor).map_err(failed)?;
        let identity = |id: &crate::gates::gate_store::GateId| self.gates.gate_identity(id);
        let anchor_gate = self
            .gates
            .registered_gate(&anchor)
            .ok_or_else(|| failed(format!("{} has no gate", next.anchor.describe())))?;
        let (nodes, _) = self.population_facts();
        for node in &nodes {
            let Some((here, id, (x, y))) = self.target_of(node) else {
                continue;
            };
            if here.gate != target.gate || (target.parent.is_some() && target.parent != here.parent)
            {
                continue;
            }
            if identity(&id) == identity(&anchor) {
                return Err(failed(format!(
                    "{} cannot sit next to itself - name the gate beside it",
                    target.describe()
                )));
            }
            let shaped = self.gates.registered_gate(&id).and_then(|g| {
                g.get_gate_ref(None).map(|inner| {
                    matches!(
                        inner.geometry,
                        flow_gates::GateGeometry::Rectangle { .. }
                            | flow_gates::GateGeometry::Polygon { .. }
                    )
                })
            });
            if shaped != Some(true) {
                return Err(failed(format!(
                    "{} is not a rectangle or a polygon, so it cannot be brought up to another gate",
                    target.describe()
                )));
            }
            if *next.parameter != *x && *next.parameter != *y {
                return Err(failed(format!(
                    "{} is drawn on {x} and {y}, so it does not move along {}",
                    target.describe(),
                    next.parameter
                )));
            }
            let parent = self.gates.parent_node(node);
            let anchor_axes = anchor_gate.get_params();
            let beside = self
                .gates
                .nodes_for_gate(&anchor)
                .iter()
                .any(|at| self.gates.parent_node(at) == parent)
                && crate::gates::gate_contact::same_axes(
                    &anchor_axes,
                    &(Arc::from(x.as_str()), Arc::from(y.as_str())),
                );
            if !beside {
                return Err(failed(format!(
                    "{} is not on the same plot as {} - the gate it sits next to is drawn \
                     beside it, under the same parent and on the same two parameters",
                    next.anchor.describe(),
                    target.describe()
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
            if mine
                .iter()
                .any(|(own, _)| self.gates.gate_identity(own) == self.gates.gate_identity(&id))
            {
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
    /// a rules file written by hand.
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
            problems.extend(
                crate::gate_rules::autogate::edges_over_their_anchors(&self.gates, store)
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

    /// The channel `parameter` names, noting it in `notes` when the name
    /// was not the channel's own.
    fn channel_noted(&self, parameter: &str, notes: &mut Vec<String>) -> Result<Arc<str>, Refusal> {
        let channel = self.channel_named(parameter)?;
        if *channel != *parameter {
            notes.push(format!("parameter {parameter} is the channel {channel}"));
        }
        Ok(channel)
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

/// Refuses a pinned marker the rule does not read, or that is not one of the
/// two axes a gate it reaches is drawn on - it has no edge there to pin.
fn check_pinned(
    wanted: &crate::gate_rules::rule::PhenotypeRule,
    matched: &[(RuleTarget, (String, String))],
) -> Result<(), Refusal> {
    for marker in &wanted.pinned {
        if !wanted.markers.is_empty() && !wanted.markers.contains(marker) {
            return Err(failed(format!(
                "{marker} is pinned but is not one of the rule's markers ({}): a pinned marker \
                 is one the rule reads",
                list(&wanted.markers)
            )));
        }
        if let Some((here, (x, y))) = matched
            .iter()
            .find(|(_, (x, y))| **marker != *x.as_str() && **marker != *y.as_str())
        {
            return Err(failed(format!(
                "{} is drawn on {x} and {y}, so it has no edge on {marker} to pin",
                here.describe()
            )));
        }
    }
    Ok(())
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
