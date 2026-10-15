//! Copying the rules under one population to the same gates under another.
//!
//! A panel gates the same markers under several populations - CD4 and CD8 T
//! cells, MAIT cells, gamma-delta T cells - and their rules are mostly the
//! same. A copy takes every rule written for a gate directly under the
//! population it is copied from and writes it for the gate of the same name
//! under the population it is copied to. A gate it reads another gate's
//! position from - the gate it takes the shape of, sits next to, or falls back
//! to - is read under the new population too, where it was under the old one.
//!
//! A rule is left out, saying why, where the new population has no gate of
//! that name, where that gate is not drawn on the parameter the rule moves,
//! where a gate it reads is not under the new population, or where the gate
//! already has a rule and the copy is not to replace it.

use std::sync::Arc;

use serde::Serialize;

use crate::gate_rules::rule::Rule;
use crate::gate_rules::rule_store::{GateRule, RuleStore, RuleTarget};
use crate::gates::GateState;
use crate::gates::gate_store::NodeId;

/// A rule copied: where it is written, and the rule it replaces.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Copied {
    pub target: RuleTarget,
    pub rule: GateRule,
    pub replaces: Option<GateRule>,
}

/// A rule not copied, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotCopied {
    pub target: RuleTarget,
    pub reason: String,
}

/// The rules of one population copied to another.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Copy {
    pub copied: Vec<Copied>,
    pub not_copied: Vec<NotCopied>,
}

impl Copy {
    /// `rules` with the copies written in.
    pub fn written_into(&self, rules: &RuleStore) -> RuleStore {
        let mut written = rules.clone();
        for copied in &self.copied {
            written.insert(copied.target.clone(), copied.rule.clone());
        }
        written
    }
}

/// The gates directly under `node`, by name, with the parameters each is
/// drawn on.
fn gates_under(state: &GateState, node: &NodeId) -> Vec<(Arc<str>, (Arc<str>, Arc<str>))> {
    state
        .child_nodes(node)
        .iter()
        .filter_map(|child| {
            let gate_id = state.gate_for_node(child)?;
            let gate = state.registered_gate(gate_id)?;
            let name = state
                .population_name(gate_id)
                .unwrap_or_else(|| Arc::from(gate.get_name()));
            Some((name, gate.get_params()))
        })
        .collect()
}

/// Every rule written for a gate directly under `from`, copied to the gate of
/// the same name under `to`. A gate under `to` that already has a rule keeps
/// it unless `replace`.
pub fn copy_rules(
    state: &GateState,
    rules: &RuleStore,
    from: &NodeId,
    to: &NodeId,
    replace: bool,
) -> Copy {
    let names = crate::gates::gate_paths::unique_names(state);
    let (Some(from_name), Some(to_name)) = (names.get(from).cloned(), names.get(to).cloned())
    else {
        return Copy::default();
    };
    let under_to = gates_under(state, to);
    let mut copy = Copy::default();
    for (gate, _) in gates_under(state, from) {
        let Some(rule) = rules.get(&RuleTarget::under(gate.clone(), from_name.clone())) else {
            continue;
        };
        let target = RuleTarget::under(gate.clone(), to_name.clone());
        let mut refuse = |reason: String| {
            copy.not_copied.push(NotCopied {
                target: target.clone(),
                reason,
            })
        };
        let Some((_, params)) = under_to.iter().find(|(name, _)| *name == gate) else {
            refuse(format!("{to_name} has no gate called {gate}"));
            continue;
        };
        let mut rule = rule.clone();
        for anchor in rule.rule.anchors_mut() {
            if anchor.parent.as_ref() == Some(&from_name) {
                anchor.parent = Some(to_name.clone());
            }
        }
        if let Some(why) = why_it_cannot_place(&rule, params, &under_to, &to_name) {
            refuse(why);
            continue;
        }
        match rules.get(&target) {
            Some(existing) if *existing == rule => refuse("already has this rule".into()),
            Some(existing) if !replace => refuse(format!(
                "already has a rule of its own, kept: {}",
                existing.rule.describe()
            )),
            existing => copy.copied.push(Copied {
                target,
                rule,
                replaces: existing.cloned(),
            }),
        }
    }
    copy
}

/// Why `rule`, copied to a gate drawn on `params` under `to_name`, could not
/// place it: the gate is not drawn on the parameter the rule moves, or a gate
/// the rule reads is not under the new population.
fn why_it_cannot_place(
    rule: &GateRule,
    (x, y): &(Arc<str>, Arc<str>),
    under_to: &[(Arc<str>, (Arc<str>, Arc<str>))],
    to_name: &Arc<str>,
) -> Option<String> {
    let moves_a_parameter = !rule.rule.reads_another_gate()
        && !matches!(rule.rule, Rule::MatchThePhenotype(_))
        && !rule.parameter.trim().is_empty();
    if moves_a_parameter && rule.parameter != *x && rule.parameter != *y {
        return Some(format!(
            "the gate is drawn on {x} and {y}, not {}",
            rule.parameter
        ));
    }
    rule.rule
        .anchors()
        .into_iter()
        .filter(|anchor| anchor.parent.as_ref() == Some(to_name))
        .find(|anchor| !under_to.iter().any(|(name, _)| *name == anchor.gate))
        .map(|anchor| format!("it reads {}, which {to_name} does not have", anchor.gate))
}

/// Every population with a gate directly under it that has a rule of its own,
/// by the name a rule gives it - what a copy can be made from.
pub fn populations_with_rules(state: &GateState, rules: &RuleStore) -> Vec<(NodeId, Arc<str>)> {
    let mut found: Vec<(NodeId, Arc<str>)> = crate::gates::gate_paths::unique_names(state)
        .into_iter()
        .filter(|(node, name)| {
            gates_under(state, node).iter().any(|(gate, _)| {
                rules
                    .get(&RuleTarget::under(gate.clone(), name.clone()))
                    .is_some()
            })
        })
        .collect();
    found.sort_by(|a, b| crate::gate_rules::rule_store::human_order(&a.1, &b.1));
    found
}

/// Every other population with a gate of the same name as one directly under
/// `from` - where its rules can be copied to.
pub fn siblings_of(state: &GateState, from: &NodeId) -> Vec<(NodeId, Arc<str>)> {
    let names: Vec<Arc<str>> = gates_under(state, from)
        .into_iter()
        .map(|(gate, _)| gate)
        .collect();
    let mut found: Vec<(NodeId, Arc<str>)> = crate::gates::gate_paths::unique_names(state)
        .into_iter()
        .filter(|(node, _)| node != from)
        .filter(|(node, _)| {
            gates_under(state, node)
                .iter()
                .any(|(gate, _)| names.contains(gate))
        })
        .collect();
    found.sort_by(|a, b| crate::gate_rules::rule_store::human_order(&a.1, &b.1));
    found
}
