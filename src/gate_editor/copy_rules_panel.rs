//! The Gate Rules tab's copy of one population's rules to its siblings - see
//! [`clingate_core::gate_rules::siblings`]. The tools for Claude copy the
//! same way with `copy_rules`.

use std::sync::Arc;

use dioxus::prelude::*;

use clingate_core::gate_rules::rule_store::RuleStore;
use clingate_core::gate_rules::siblings::{Copy, copy_rules, populations_with_rules, siblings_of};
use clingate_core::gates::gate_store::NodeId;

use crate::components::toast::{say, use_toast};
use crate::gate_editor::workspace_window::GateStore;

/// A copy from the panel, on the gates and rules as they stand.
#[derive(Clone, Copy)]
pub(crate) struct CopyRun {
    gates: GateStore,
    rules: Signal<RuleStore>,
}

impl CopyRun {
    pub(crate) fn from_context() -> Self {
        Self {
            gates: use_context(),
            rules: use_context(),
        }
    }

    /// The rules under `from` copied to each of `to` in turn, each copy
    /// seeing the ones before it.
    pub(crate) fn copy(&self, from: &NodeId, to: &[NodeId], replace: bool) -> Copy {
        let gates = self.gates.read();
        let mut rules = self.rules.peek().clone();
        let mut all = Copy::default();
        for sibling in to {
            let copy = copy_rules(&gates, &rules, from, sibling, replace);
            rules = copy.written_into(&rules);
            all.copied.extend(copy.copied);
            all.not_copied.extend(copy.not_copied);
        }
        all
    }

    /// Write `copy`'s rules into the rules list.
    pub(crate) fn take(&mut self, copy: &Copy) {
        let written = copy.written_into(&self.rules.peek());
        self.rules.set(written);
    }
}

/// Choose a population with rules and the siblings to copy them to; see
/// what would be copied and left out; copy.
#[component]
pub fn CopyRulesPanel() -> Element {
    let mut run = CopyRun::from_context();
    let gates = use_context::<GateStore>();
    let rules = use_context::<Signal<RuleStore>>();
    let toasts = use_toast();
    let mut from = use_signal(|| None::<NodeId>);
    let mut to = use_signal(Vec::<NodeId>::new);
    let mut replace = use_signal(|| false);

    let sources = use_memo(move || populations_with_rules(&gates.read(), &rules.read()));
    let siblings = use_memo(move || {
        from()
            .map(|node| siblings_of(&gates.read(), &node))
            .unwrap_or_default()
    });
    let preview = use_memo(move || {
        let _ = rules.read();
        from().map(|node| run.copy(&node, &to(), replace()))
    });

    let name_of = |found: &[(NodeId, Arc<str>)], node: &NodeId| {
        found
            .iter()
            .find(|(n, _)| n == node)
            .map(|(_, name)| name.to_string())
            .unwrap_or_default()
    };

    rsx! {
        fieldset { class: "gate_rules-form",
            legend { "Copy rules to sibling populations" }
            p { class: "gate_rules-hint gate_rules-span",
                "Copies every rule written for a gate directly under one population to the gate of the same name under the populations you tick - CD4 T cells' rules to CD8, MAIT and gamma-delta T cells, say. A gate a rule reads under the first population is read under the new one. Nothing is written until you copy; Save the rules below to keep them."
            }
            label { "Copy from" }
            select {
                onchange: move |e| {
                    let chosen = e.value();
                    from.set(
                        sources
                            .read()
                            .iter()
                            .find(|(node, _)| node.as_str() == chosen)
                            .map(|(node, _)| node.clone()),
                    );
                    to.set(Vec::new());
                },
                option { value: "", selected: from().is_none(), "choose a population with rules" }
                for (node , name) in sources.read().iter() {
                    option {
                        key: "{node.as_str()}",
                        value: "{node.as_str()}",
                        selected: from().as_ref() == Some(node),
                        "{name}"
                    }
                }
            }
            if from().is_some() {
                label { "To" }
                div { class: "gate_rules-span",
                    if siblings.read().is_empty() {
                        "No other population has a gate of the same name."
                    }
                    for (node , name) in siblings.read().iter().cloned() {
                        label { key: "{node.as_str()}", class: "gate_rules-check",
                            input {
                                r#type: "checkbox",
                                checked: to().contains(&node),
                                onchange: move |e| {
                                    let mut ticked = to.write();
                                    ticked.retain(|n| *n != node);
                                    if e.checked() {
                                        ticked.push(node.clone());
                                    }
                                },
                            }
                            " {name}"
                        }
                    }
                }
                label { class: "gate_rules-check gate_rules-span",
                    input {
                        r#type: "checkbox",
                        checked: replace(),
                        onchange: move |e| replace.set(e.checked()),
                    }
                    " Replace a rule a gate there already has"
                }
            }
            if let Some(copy) = preview() {
                if !to().is_empty() {
                    table { class: "gate_rules-table gate_rules-span",
                        thead {
                            tr {
                                th { "Gate" }
                                th { "" }
                            }
                        }
                        tbody {
                            for (at , copied) in copy.copied.iter().enumerate() {
                                tr { key: "c{at}",
                                    td { "{copied.target.describe()}" }
                                    td {
                                        "{copied.rule.rule.describe()}"
                                        if copied.replaces.is_some() {
                                            " - replaces its rule"
                                        }
                                    }
                                }
                            }
                            for (at , left) in copy.not_copied.iter().enumerate() {
                                tr { key: "n{at}", class: "gate_rules-weak",
                                    td { "{left.target.describe()}" }
                                    td { "not copied: {left.reason}" }
                                }
                            }
                        }
                    }
                    button {
                        class: "gate_rules-add",
                        disabled: copy.copied.is_empty(),
                        onclick: move |_| {
                            let Some(copy) = preview() else { return };
                            run.take(&copy);
                            let from_name = from()
                                .map(|node| name_of(&sources.read(), &node))
                                .unwrap_or_default();
                            say(
                                &toasts,
                                format!(
                                    "Copied {} rule(s) from {from_name} - Save the rules below to keep them",
                                    copy.copied.len()
                                ),
                            );
                            to.set(Vec::new());
                        },
                        "Copy {copy.copied.len()} rule(s)"
                    }
                }
            }
        }
    }
}
