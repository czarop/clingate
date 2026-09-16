//! The Gate Rules tab: say where each gate belongs, once.
//!
//! A rule names a *population* - "Ki67+ of CD4+" - rather than a container, so
//! one rule covers every place that gate appears. In a real export "CD279+"
//! occupied twenty-five containers; four rules covered the whole panel.

use crate::gate_editor::gates::GateState;
use crate::gate_rules::rule::{PercentileOffsetRule, Rule, TailFractionRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::omiq::metadata::{MetaDataStore, MetaDataStoreStoreExt};
use dioxus::prelude::*;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

static CSS_STYLE: Asset = asset!("assets/gate_rules.css");

/// Any gate that appears in the document: its name, the parents it is drawn
/// under, and the parameters its plots use.
#[derive(Clone, PartialEq, Default)]
struct GateChoices {
    names: Vec<Arc<str>>,
    /// Parent names seen for a given gate name, for "Ki67+ of CD4+".
    parents: Vec<(Arc<str>, Vec<Arc<str>>)>,
    /// Parameters seen for a given gate name. A gate is usually drawn on the
    /// same pair everywhere, but not always, so every one seen is offered.
    parameters: Vec<(Arc<str>, Vec<Arc<str>>)>,
}

impl GateChoices {
    fn parents_of(&self, gate: &str) -> &[Arc<str>] {
        self.parents
            .iter()
            .find(|(g, _)| &**g == gate)
            .map(|(_, p)| p.as_slice())
            .unwrap_or(&[])
    }
    fn parameters_of(&self, gate: &str) -> &[Arc<str>] {
        self.parameters
            .iter()
            .find(|(g, _)| &**g == gate)
            .map(|(_, p)| p.as_slice())
            .unwrap_or(&[])
    }
}

fn push_unique(list: &mut Vec<Arc<str>>, value: Arc<str>) {
    if !list.iter().any(|v| *v == value) {
        list.push(value);
    }
}

fn entry<'a>(map: &'a mut Vec<(Arc<str>, Vec<Arc<str>>)>, key: &Arc<str>) -> &'a mut Vec<Arc<str>> {
    if let Some(at) = map.iter().position(|(k, _)| k == key) {
        return &mut map[at].1;
    }
    map.push((key.clone(), Vec::new()));
    &mut map.last_mut().expect("just pushed").1
}

/// Walk the tree once for everything the form needs to offer.
fn choices(state: &GateState) -> GateChoices {
    let mut out = GateChoices::default();
    for (node, placement) in state.placements() {
        let Some(gate) = state.registered_gate(&placement.gate_id) else {
            continue;
        };
        let name: Arc<str> = Arc::from(gate.get_name());
        push_unique(&mut out.names, name.clone());

        let (x, y) = gate.get_params();
        let params = entry(&mut out.parameters, &name);
        push_unique(params, x);
        push_unique(params, y);

        if let Some(parent_name) = state
            .parent_node(node)
            .and_then(|p| state.gate_for_node(&p).cloned())
            .and_then(|id| state.registered_gate(&id))
            .map(|g| Arc::from(g.get_name()) as Arc<str>)
        {
            push_unique(entry(&mut out.parents, &name), parent_name);
        }
    }
    out.names.sort();
    out
}

const ANY_PARENT: &str = "__any__";

/// A file id as the name a person knows it by, falling back to the id itself
/// when the metadata has not been loaded.
fn name_of(files: &[(Arc<str>, Arc<str>)], id: &str) -> String {
    files
        .iter()
        .find(|(_, file_id)| &**file_id == id)
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| id.to_string())
}

/// The loaded files, by the name a person recognises them by.
///
/// Rules address files by the id the gating document uses, which is opaque -
/// nobody picks `1TBQ` off a list. The metadata carries the mapping back to
/// the file name, so that is what the form shows.
fn loaded_files(map: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>) -> Vec<(Arc<str>, Arc<str>)> {
    let mut out: Vec<(Arc<str>, Arc<str>)> = map
        .iter()
        .map(|(name, id)| (name.clone(), id.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[component]
pub fn GateRulesWindow() -> Element {
    let gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let mut rules = use_context::<Signal<RuleStore>>();

    let choices = use_memo(move || choices(&gate_store.read()));
    let files = use_memo(move || loaded_files(&metadata_store.file_name_to_gating_id().read()));

    // The form.
    let mut gate = use_signal(String::new);
    let mut parent = use_signal(|| ANY_PARENT.to_string());
    let mut parameter = use_signal(String::new);
    let mut bound = use_signal(|| "Above".to_string());
    let mut measured_on = use_signal(|| "FMX".to_string());
    let mut kind = use_signal(|| "TailFraction".to_string());
    let mut low = use_signal(|| "0.2".to_string());
    let mut high = use_signal(|| "0.5".to_string());
    let mut percentile = use_signal(|| "99".to_string());
    let mut offset = use_signal(|| "0.5".to_string());
    let mut gated_file = use_signal(String::new);
    let mut reference_type = use_signal(|| "FMX".to_string());
    let mut reference_file = use_signal(String::new);
    let mut sidecar = use_signal(|| "gate_rules.json".to_string());
    let mut message = use_signal(|| None::<String>);

    // Picking a gate offers only the parents and parameters that gate is drawn
    // with, so the form cannot name a combination the document does not have.
    let selected_parents = use_memo(move || choices.read().parents_of(&gate()).to_vec());
    let selected_parameters = use_memo(move || choices.read().parameters_of(&gate()).to_vec());

    let mut add = move || {
        let name = gate();
        if name.is_empty() {
            message.set(Some("Choose a gate first".into()));
            return;
        }
        let param = parameter();
        if param.is_empty() {
            message.set(Some("Choose the parameter the rule positions".into()));
            return;
        }
        let rule = match kind().as_str() {
            "PercentileOffset" => {
                let (Ok(p), Ok(o)) = (percentile().parse::<f64>(), offset().parse::<f64>()) else {
                    message.set(Some("The percentile and offset must be numbers".into()));
                    return;
                };
                Rule::PercentileOffset(PercentileOffsetRule::new(p, o))
            }
            _ => {
                let (Ok(l), Ok(h)) = (low().parse::<f64>(), high().parse::<f64>()) else {
                    message.set(Some("The band must be two numbers".into()));
                    return;
                };
                if l > h {
                    message.set(Some("The band's lower bound is above its upper".into()));
                    return;
                }
                // Typed as percentages, stored as fractions.
                Rule::TailFraction(TailFractionRule::new((l / 100.0, h / 100.0)))
            }
        };
        let target = match parent().as_str() {
            ANY_PARENT => RuleTarget::named(name.as_str()),
            p => RuleTarget::under(name.as_str(), p),
        };
        let described = target.describe();
        rules.write().insert(
            target,
            GateRule {
                parameter: Arc::from(param.as_str()),
                bound: if bound() == "Below" {
                    Bound::Below
                } else {
                    Bound::Above
                },
                measured_on: match measured_on().as_str() {
                    "" | "Itself" => MeasuredOn::Itself,
                    t => MeasuredOn::Partner(Arc::from(t)),
                },
                rule,
            },
        );
        message.set(Some(format!("Rule set for {described}")));
    };

    rsx! {
        document::Link { rel: "stylesheet", href: CSS_STYLE }
        div { class: "gate_rules",
            h2 { "Gate rules" }
            p { class: "gate_rules-hint",
                "A rule names a population, not a gate on one plot. Leaving the parent as "
                em { "any" }
                " applies it wherever that gate appears."
            }

            // ── the rules that exist ──────────────────────────────────────
            if rules.read().is_empty() {
                p { class: "gate_rules-empty", "No rules yet." }
            } else {
                table { class: "gate_rules-table",
                    thead {
                        tr {
                            th { "Applies to" }
                            th { "Parameter" }
                            th { "Keeps" }
                            th { "Measured on" }
                            th { "Rule" }
                            th { }
                        }
                    }
                    tbody {
                        for entry in rules.read().entries().to_vec() {
                            tr { key: "{entry.target.describe()}",
                                td { "{entry.target.describe()}" }
                                td { "{entry.rule.parameter}" }
                                td {
                                    match entry.rule.bound {
                                        Bound::Above => "above",
                                        Bound::Below => "below",
                                    }
                                }
                                td {
                                    match &entry.rule.measured_on {
                                        MeasuredOn::Itself => "the sample itself".to_string(),
                                        MeasuredOn::Partner(t) => format!("its {t}"),
                                        MeasuredOn::File(f) => format!("{f}"),
                                    }
                                }
                                td { "{entry.rule.rule.describe()}" }
                                td {
                                    button {
                                        class: "gate_rules-remove",
                                        onclick: {
                                            let target = entry.target.clone();
                                            move |_| {
                                                rules.write().remove(&target);
                                            }
                                        },
                                        "remove"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── adding one ────────────────────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Add a rule" }

                label { "Gate" }
                select {
                    value: "{gate}",
                    onchange: move |e| {
                        gate.set(e.value());
                        parent.set(ANY_PARENT.to_string());
                        parameter.set(String::new());
                    },
                    option { value: "", "choose a gate" }
                    for name in choices.read().names.clone() {
                        option { value: "{name}", "{name}" }
                    }
                }

                label { "Of parent" }
                select {
                    value: "{parent}",
                    onchange: move |e| parent.set(e.value()),
                    option { value: ANY_PARENT, "any" }
                    for name in selected_parents.read().clone() {
                        option { value: "{name}", "{name}" }
                    }
                }

                label { "Positions on" }
                select {
                    value: "{parameter}",
                    onchange: move |e| parameter.set(e.value()),
                    option { value: "", "choose a parameter" }
                    for name in selected_parameters.read().clone() {
                        option { value: "{name}", "{name}" }
                    }
                }

                label { "Gate keeps events" }
                select {
                    value: "{bound}",
                    onchange: move |e| bound.set(e.value()),
                    option { value: "Above", "above the line" }
                    option { value: "Below", "below the line" }
                }

                label { "Measured on" }
                input {
                    value: "{measured_on}",
                    oninput: move |e| measured_on.set(e.value()),
                    placeholder: "FMX, or Itself",
                }

                label { "Rule" }
                select {
                    value: "{kind}",
                    onchange: move |e| kind.set(e.value()),
                    option { value: "TailFraction", "capture a percentage of the parent" }
                    option { value: "PercentileOffset", "step above a percentile" }
                }

                if kind() == "PercentileOffset" {
                    label { "Percentile" }
                    input {
                        r#type: "number",
                        value: "{percentile}",
                        oninput: move |e| percentile.set(e.value()),
                    }
                    label { "Offset" }
                    input {
                        r#type: "number",
                        step: "0.1",
                        value: "{offset}",
                        oninput: move |e| offset.set(e.value()),
                    }
                } else {
                    label { "Capture between (%)" }
                    div { class: "gate_rules-band",
                        input {
                            r#type: "number",
                            step: "0.01",
                            value: "{low}",
                            oninput: move |e| low.set(e.value()),
                        }
                        span { "to" }
                        input {
                            r#type: "number",
                            step: "0.01",
                            value: "{high}",
                            oninput: move |e| high.set(e.value()),
                        }
                    }
                }

                button { class: "gate_rules-add", onclick: move |_| add(), "Add rule" }
            }

            // ── which files belong together ───────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Sample pairing" }
                p { class: "gate_rules-hint gate_rules-span",
                    "Naming conventions differ between datasets, so the columns that group a specimen's files are named here rather than guessed."
                }

                label { "Sample ID column" }
                input {
                    value: "{rules.read().pairing.sample_id_column}",
                    oninput: move |e| {
                        rules.write().pairing.sample_id_column = Arc::from(e.value().as_str());
                    },
                }

                label { "Sample type column" }
                input {
                    value: "{rules.read().pairing.sample_type_column}",
                    oninput: move |e| {
                        rules.write().pairing.sample_type_column = Arc::from(e.value().as_str());
                    },
                }
            }

            // ── references chosen by hand ─────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Reference files" }
                p { class: "gate_rules-hint gate_rules-span",
                    "The pairing finds the right partner in the ordinary case. Where it cannot - an FMO that was never run, a stand-in from another specimen - name the file to measure here."
                }

                if !rules.read().references().is_empty() {
                    div { class: "gate_rules-span",
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Gating" }
                                    th { "Asking for" }
                                    th { "Measures" }
                                    th { }
                                }
                            }
                            tbody {
                                for reference in rules.read().references().to_vec() {
                                    tr { key: "{reference.gated}/{reference.sample_type}",
                                        td { "{name_of(&files.read(), &reference.gated)}" }
                                        td { "{reference.sample_type}" }
                                        td { "{name_of(&files.read(), &reference.reference)}" }
                                        td {
                                            button {
                                                class: "gate_rules-remove",
                                                onclick: {
                                                    let gated = reference.gated.clone();
                                                    let sample_type = reference.sample_type.clone();
                                                    move |_| {
                                                        rules.write().clear_reference(&gated, &sample_type);
                                                    }
                                                },
                                                "clear"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                label { "When gating" }
                select {
                    value: "{gated_file}",
                    onchange: move |e| gated_file.set(e.value()),
                    option { value: "", "choose a file" }
                    for (name , id) in files.read().clone() {
                        option { value: "{id}", "{name}" }
                    }
                }

                label { "And the rule asks for" }
                input {
                    value: "{reference_type}",
                    oninput: move |e| reference_type.set(e.value()),
                    placeholder: "FMX",
                }

                label { "Measure instead" }
                select {
                    value: "{reference_file}",
                    onchange: move |e| reference_file.set(e.value()),
                    option { value: "", "choose a file" }
                    for (name , id) in files.read().clone() {
                        option { value: "{id}", "{name}" }
                    }
                }

                button {
                    class: "gate_rules-add",
                    onclick: move |_| {
                        let (gated, reference) = (gated_file(), reference_file());
                        if gated.is_empty() || reference.is_empty() {
                            message.set(Some("Choose both files".into()));
                            return;
                        }
                        rules
                            .write()
                            .set_reference(
                                Arc::from(gated.as_str()),
                                Arc::from(reference_type().as_str()),
                                Arc::from(reference.as_str()),
                            );
                        message.set(Some("Reference set".into()));
                    },
                    "Set reference"
                }
            }

            // ── the sidecar ───────────────────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Sidecar" }
                label { "File" }
                input {
                    value: "{sidecar}",
                    oninput: move |e| sidecar.set(e.value()),
                }
                div { class: "gate_rules-band",
                    button {
                        onclick: move |_| {
                            let path = PathBuf::from(sidecar());
                            match rules.read().save(&path) {
                                Ok(()) => message.set(Some(format!("Saved to {}", path.display()))),
                                Err(e) => message.set(Some(format!("Could not save: {e}"))),
                            }
                        },
                        "Save"
                    }
                    button {
                        onclick: move |_| {
                            let path = PathBuf::from(sidecar());
                            match RuleStore::load(&path) {
                                Ok(loaded) => {
                                    let n = loaded.len();
                                    rules.set(loaded);
                                    message.set(Some(format!("Loaded {n} rules")));
                                }
                                Err(e) => message.set(Some(format!("Could not load: {e}"))),
                            }
                        },
                        "Load"
                    }
                }
            }

            if let Some(text) = message() {
                p { class: "gate_rules-message", "{text}" }
            }
        }
    }
}
