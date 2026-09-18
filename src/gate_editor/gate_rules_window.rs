//! The Gate Rules tab: say where each gate belongs, once.
//!
//! A rule names a *population* - "Ki67+ of CD4+" - rather than a container, so
//! one rule covers every place that gate appears. In a real export "CD279+"
//! occupied twenty-five containers; four rules covered the whole panel.

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_single::boolean_gates::BooleanGate;
use crate::gate_editor::gates::gate_store::GateId;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::pairing_controls::PairingColumns;
use crate::gate_editor::plots::axis_store::{AxisStore, AxisStoreStoreExt};
use crate::gate_rules::autogate::{Report, describe, measure_file, position_all};
use crate::gate_rules::rule::{
    AboveTheNegativeRule, NegativeFinder, PercentileOffsetRule, Rule, TailFractionRule,
};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::omiq::metadata::{MetaDataStore, MetaDataStoreStoreExt};
use dioxus::prelude::*;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

static CSS_STYLE: Asset = asset!("assets/gate_rules.css");

/// The gates a rule can be written against, arranged the way a person names
/// them: pick the population first, then the gate drawn on it.
///
/// Booleans and ghosts are left out. A boolean has no geometry to slide - it is
/// a statement about other gates - and a ghost has no position in the tree at
/// all, only an id some boolean still refers to. Neither can be positioned, so
/// offering them would only invite a rule that can never run.
#[derive(Clone, PartialEq, Default)]
pub struct GateChoices {
    /// Parents holding at least one gate a rule could position.
    pub parents: Vec<Arc<str>>,
    /// The gates drawn on each parent.
    pub children: Vec<(Arc<str>, Vec<Arc<str>>)>,
    /// Parameters seen for a given gate name. A gate is usually drawn on the
    /// same pair everywhere, but not always, so every one seen is offered.
    pub parameters: Vec<(Arc<str>, Vec<Arc<str>>)>,
}

impl GateChoices {
    pub fn children_of(&self, parent: &str) -> &[Arc<str>] {
        self.children
            .iter()
            .find(|(p, _)| &**p == parent)
            .map(|(_, c)| c.as_slice())
            .unwrap_or(&[])
    }
    pub fn parameters_of(&self, gate: &str) -> &[Arc<str>] {
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

/// Whether a rule could ever position this gate.
///
/// Three kinds are left out. A boolean has no geometry to slide - it is a
/// statement about other gates. A ghost has no position in the tree at all,
/// only an id some boolean still refers to. And a composite is the container
/// holding a quadrant's corners rather than a gate anyone drew: it carries the
/// group's id as its name, so it appears in a list as something like `Mzk4`,
/// and a rule positioning one line has nothing to say about two crossing ones.
/// Its corners are separate gates with real names and stay on offer.
///
/// Shape is deliberately not checked. An ellipse is a real gate in a real
/// place, and a rule naming one should fail out loud when it runs rather than
/// vanish from a list with no explanation.
fn positionable(state: &GateState, gate_id: &GateId, gate: &Arc<dyn DrawableGate>) -> bool {
    !state.is_ghost(gate_id)
        && !gate.is_composite()
        && gate.as_any().downcast_ref::<BooleanGate>().is_none()
}

/// Walk the tree once for everything the form needs to offer.
pub fn choices(state: &GateState) -> GateChoices {
    let mut out = GateChoices::default();
    // Named by path where a bare name would name two populations at once.
    let names = crate::gate_editor::gates::gate_paths::unique_names(state);
    for (node, placement) in state.placements() {
        let Some(gate) = state.registered_gate(&placement.gate_id) else {
            continue;
        };
        if !positionable(state, &placement.gate_id, &gate) {
            continue;
        }
        let name: Arc<str> = Arc::from(gate.get_name());

        let (x, y) = gate.get_params();
        let params = entry(&mut out.parameters, &name);
        push_unique(params, x);
        push_unique(params, y);

        // A gate with no parent sits at the root and has no population to be
        // a fraction of, so there is nothing for a rule to measure it against.
        let Some(parent_name) = state.parent_node(node).and_then(|p| names.get(&p).cloned()) else {
            continue;
        };
        push_unique(&mut out.parents, parent_name.clone());
        push_unique(entry(&mut out.children, &parent_name), name);
    }
    out.parents.sort();
    for (_, children) in out.children.iter_mut() {
        children.sort();
    }
    out
}

/// Below this, a placement is worth opening by hand. Measured against the
/// hand-gated export: everything at or above it landed within 0.18 arcsinh
/// units of where a person had put it, which is a typical manual nudge.
const REVIEW_FLOOR: f64 = 0.30;

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
    let mut gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();
    let mut rules = use_context::<Signal<RuleStore>>();

    let choices = use_memo(move || choices(&gate_store.read()));
    let files = use_memo(move || loaded_files(&metadata_store.file_name_to_gating_id().read()));

    // The form.
    let mut gate = use_signal(String::new);
    let mut parent = use_signal(String::new);
    let mut parameter = use_signal(String::new);
    let mut bound = use_signal(|| "Above".to_string());
    let mut measured_on = use_signal(|| "FMX".to_string());
    let mut kind = use_signal(|| "TailFraction".to_string());
    let mut low = use_signal(|| "0.2".to_string());
    let mut high = use_signal(|| "0.5".to_string());
    let mut percentile = use_signal(|| "99".to_string());
    let mut offset = use_signal(|| "0.5".to_string());
    let mut calibrate_on = use_signal(String::new);
    let mut finder = use_signal(|| "RefineFromGate".to_string());
    let mut scale = use_signal(|| "1.0".to_string());
    let mut nudge = use_signal(|| "0.0".to_string());
    let mut gated_file = use_signal(String::new);
    let mut reference_type = use_signal(|| "FMX".to_string());
    let mut reference_file = use_signal(String::new);
    let mut fcs_dir = use_signal(default_fcs_dir);
    let mut running = use_signal(|| false);
    let mut report = use_signal(|| None::<Report>);
    let mut sidecar = use_signal(|| "gate_rules.json".to_string());
    let mut message = use_signal(|| None::<String>);

    // Picking a gate offers only the parents and parameters that gate is drawn
    // with, so the form cannot name a combination the document does not have.
    // Picking a parent narrows the gates, and picking a gate narrows the
    // parameters, so the form cannot name a combination the document lacks.
    let selected_children = use_memo(move || choices.read().children_of(&parent()).to_vec());
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
            "AboveTheNegative" => {
                let (Ok(s), Ok(n)) = (scale().parse::<f64>(), nudge().parse::<f64>()) else {
                    message.set(Some("The scale and nudge must be numbers".into()));
                    return;
                };
                Rule::AboveTheNegative(AboveTheNegativeRule {
                    scale: s,
                    nudge: n,
                    find: match finder().as_str() {
                        "DensityPeak" => NegativeFinder::DensityPeak,
                        _ => NegativeFinder::RefineFromGate,
                    },
                    ..AboveTheNegativeRule::default()
                })
            }
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
        if kind() == "AboveTheNegative" && calibrate_on().is_empty() {
            message.set(Some("Choose the sample to calibrate against".into()));
            return;
        }
        let target = match parent().as_str() {
            "" => RuleTarget::named(name.as_str()),
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
                measured_on: if kind() == "AboveTheNegative" {
                    // This rule calibrates against one named sample - the QC -
                    // rather than a partner of each specimen.
                    MeasuredOn::File(Arc::from(calibrate_on().as_str()))
                } else {
                    match measured_on().as_str() {
                        "" | "Itself" => MeasuredOn::Itself,
                        t => MeasuredOn::Partner(Arc::from(t)),
                    }
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

                // Population first, then the gate drawn on it - the order a
                // person says it in, and the order that makes the second list
                // short enough to read.
                label { "Population" }
                select {
                    value: "{parent}",
                    onchange: move |e| {
                        parent.set(e.value());
                        gate.set(String::new());
                        parameter.set(String::new());
                    },
                    option { value: "", "choose a population" }
                    for name in choices.read().parents.clone() {
                        option { value: "{name}", "{name}" }
                    }
                }

                label { "Gate" }
                select {
                    value: "{gate}",
                    onchange: move |e| {
                        gate.set(e.value());
                        parameter.set(String::new());
                    },
                    option { value: "", "choose a gate" }
                    for name in selected_children.read().clone() {
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

                if kind() != "AboveTheNegative" {
                    label { "Measured on" }
                    input {
                        value: "{measured_on}",
                        oninput: move |e| measured_on.set(e.value()),
                        placeholder: "FMX, or Itself",
                    }
                }

                label { "Rule" }
                select {
                    value: "{kind}",
                    onchange: move |e| kind.set(e.value()),
                    option { value: "TailFraction", "capture a percentage of the parent" }
                    option { value: "PercentileOffset", "step above a percentile" }
                    option { value: "AboveTheNegative", "above the negative, as on a reference sample" }
                }

                if kind() == "AboveTheNegative" {
                    label { "Calibrate on" }
                    select {
                        value: "{calibrate_on}",
                        onchange: move |e| calibrate_on.set(e.value()),
                        option { value: "", "choose the reference sample" }
                        for (name , id) in files.read().clone() {
                            option { value: "{id}", "{name}" }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Reads how far above that sample's negative its gate sits, in widths of that negative, and puts every other gate the same number of widths above its own. No FMO needed - the negative is read from the sample being gated."
                    }

                    label { "Find the negative" }
                    select {
                        value: "{finder}",
                        onchange: move |e| finder.set(e.value()),
                        option { value: "RefineFromGate", "from the events below the gate" }
                        option { value: "DensityPeak", "from the density's leftmost peak" }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Below-the-gate is the sharper of the two while a negative has not moved more than the calibrated distance, and sticks low beyond that. The density peak tracks any drift but disagrees with itself more between samples. Worth running both and comparing against your own gating."
                    }

                    label { "Scale" }
                    input {
                        r#type: "number",
                        step: "0.05",
                        value: "{scale}",
                        oninput: move |e| scale.set(e.value()),
                    }
                    label { "Nudge" }
                    input {
                        r#type: "number",
                        step: "0.01",
                        value: "{nudge}",
                        oninput: move |e| nudge.set(e.value()),
                    }
                } else if kind() == "PercentileOffset" {
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

                PairingColumns {}
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

            // ── running the rules ─────────────────────────────────────────
            fieldset { class: "gate_rules-form",
                legend { "Autogate" }
                p { class: "gate_rules-hint gate_rules-span",
                    "Solves every rule above against the loaded workflow and gives each specimen its own gate. The position drawn by hand stays put underneath, so this can be re-run or ignored."
                }

                label { "FCS folder" }
                input {
                    value: "{fcs_dir}",
                    oninput: move |e| fcs_dir.set(e.value()),
                }

                button {
                    class: "gate_rules-add",
                    disabled: running(),
                    onclick: move |_| async move {
                        if running() {
                            return;
                        }
                        running.set(true);
                        report.set(None);
                        message.set(Some("Reading FCS files...".into()));

                        let dir = fcs_dir();
                        let names = metadata_store.file_name_to_gating_id().read().clone();
                        let mut arcsinh: Vec<(Arc<str>, f32)> = Vec::new();
                        for (param, info) in axis_store.settings().read().iter() {
                            if info.is_arcsinh()
                                && let Some(cofactor) = info.get_cofactor()
                            {
                                arcsinh.push((param.clone(), cofactor));
                            }
                        }

                        // The reading is the slow part and wants no store, so
                        // it goes to a blocking thread; the measuring and
                        // placing want the store and are quick, so they stay
                        // here.
                        let loaded = tokio::task::spawn_blocking(move || {
                            load_scaled(&dir, &names, &arcsinh)
                        })
                        .await;

                        let (frames, mut problems) = match loaded {
                            Ok(v) => v,
                            Err(e) => {
                                running.set(false);
                                message.set(Some(format!("Could not read the files: {e}")));
                                return;
                            }
                        };
                        if frames.is_empty() {
                            running.set(false);
                            message.set(Some(if problems.is_empty() {
                                format!("No .fcs files in {}", fcs_dir())
                            } else {
                                problems.join("; ")
                            }));
                            return;
                        }

                        message.set(Some(format!("Solving over {} files...", frames.len())));
                        let metadata = metadata_store.metadata().read().clone();
                        let rules_now = rules.read().clone();

                        let mut state = gate_store.write();
                        let mut measured = Vec::new();
                        let mut unmeasured = Vec::new();
                        for (id, df) in &frames {
                            match measure_file(&state, id, df, &metadata, &rules_now) {
                                Ok((mut m, mut u)) => {
                                    measured.append(&mut m);
                                    unmeasured.append(&mut u);
                                }
                                Err(e) => problems.push(format!("{id}: {e}")),
                            }
                        }
                        let mut run = position_all(
                            &mut state,
                            &rules_now,
                            &measured,
                            &unmeasured,
                            &metadata,
                        );
                        drop(state);

                        for problem in problems {
                            run.skipped.push(crate::gate_rules::autogate::Skipped {
                                file: Arc::from(""),
                                gate: Arc::from(""),
                                parent_gate: None,
                                reason: problem,
                            });
                        }
                        message.set(Some(format!(
                            "Moved {} gates, left {} already in band and {} reference; {} need review",
                            run.positioned.len(),
                            run.unchanged.len(),
                            run.reference.len(),
                            run.needs_review(REVIEW_FLOOR).count()
                        )));
                        report.set(Some(run));
                        running.set(false);
                    },
                    if running() { "Working..." } else { "Solve and apply" }
                }
            }

            // ── what it did ───────────────────────────────────────────────
            if let Some(run) = report.read().as_ref() {
                div { class: "gate_rules-report",
                    if !run.positioned.is_empty() {
                        h3 { "Positioned" }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Measured on" }
                                    th { "From" }
                                    th { "To" }
                                    th { "Captured" }
                                    th { "Confidence" }
                                    th { "Weakest" }
                                }
                            }
                            tbody {
                                for placed in run.positioned.iter() {
                                    tr {
                                        class: if placed.confidence < REVIEW_FLOOR || !placed.in_band { "gate_rules-weak" } else { "" },
                                        td { "{placed.specimen}" }
                                        td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                        td { "{name_of(&files.read(), &placed.measured_on)}" }
                                        td { "{placed.from:.3}" }
                                        td { "{placed.to:.3}" }
                                        td {
                                            title: "of {placed.reference_events} events on the file it measured",
                                            "{placed.achieved * 100.0:.3}%"
                                            if !placed.in_band {
                                                " (outside the band - nearest achievable)"
                                            }
                                        }
                                        td { "{placed.confidence:.2}" }
                                        td { "{placed.weakest.unwrap_or(\"-\")}" }
                                    }
                                }
                            }
                        }
                    }
                    if !run.unchanged.is_empty() {
                        h3 { "Already in band - left alone" }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Captures" }
                                }
                            }
                            tbody {
                                for kept in run.unchanged.iter() {
                                    tr {
                                        td { "{kept.specimen}" }
                                        td { "{describe(&kept.gate, kept.parent_gate.as_deref())}" }
                                        td { "{kept.achieved * 100.0:.3}%" }
                                    }
                                }
                            }
                        }
                    }
                    if run.positioned.iter().any(|p| p.negative.is_some()) {
                        h3 { "How the negative was read" }
                        p { class: "gate_rules-note",
                            "The gate sits a fixed number of the negative's own widths above its centre, so a sample whose negative reads tighter gets a gate nearer to it. The ratio is that comparison made directly: below 1.00 this sample's negative measured narrower than the reference's."
                        }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Ref centre" }
                                    th { "Ref width" }
                                    th { "Centre" }
                                    th { "Width" }
                                    th { "Ratio" }
                                    th { "Widths" }
                                    th { "Placed at" }
                                    th { "Flank n" }
                                }
                            }
                            tbody {
                                for (placed , (reference , here)) in run
                                    .positioned
                                    .iter()
                                    .filter_map(|p| p.negative.map(|n| (p, n)))
                                {
                                    tr {
                                        td { "{placed.specimen}" }
                                        td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                        td { "{reference.centre:.3}" }
                                        td { "{reference.spread:.3}" }
                                        td { "{here.centre:.3}" }
                                        td { "{here.spread:.3}" }
                                        td {
                                            class: if (here.spread / reference.spread - 1.0).abs() > 0.15 { "gate_rules-weak" } else { "" },
                                            title: "this sample's negative width over the reference's",
                                            "{here.spread / reference.spread:.2}"
                                        }
                                        td { "{here.widths:.2}" }
                                        td { "{here.at:.3}" }
                                        td {
                                            title: "events the width was measured from",
                                            "{here.flank_events}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !run.reference.is_empty() {
                        h3 { "Reference - left as drawn" }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Captures" }
                                }
                            }
                            tbody {
                                for kept in run.reference.iter() {
                                    tr {
                                        td { "{kept.specimen}" }
                                        td { "{describe(&kept.gate, kept.parent_gate.as_deref())}" }
                                        td { "{kept.achieved * 100.0:.3}%" }
                                    }
                                }
                            }
                        }
                    }
                    if !run.skipped.is_empty() {
                        h3 { "Not positioned" }
                        ul { class: "gate_rules-skipped",
                            for missed in run.skipped.iter() {
                                li {
                                    "{describe(&missed.gate, missed.parent_gate.as_deref())} {missed.file}: {missed.reason}"
                                }
                            }
                        }
                    }
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

/// Where the app was last told to find its FCS files.
///
/// The same `file_paths.txt` the main window reads, so the tab opens pointing
/// at the files already loaded rather than at nothing.
fn default_fcs_dir() -> String {
    std::fs::read_to_string("file_paths.txt")
        .ok()
        .and_then(|c| {
            c.lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_default()
}

/// Read every FCS in `dir` and scale it exactly as the plots do.
///
/// The gates live in scaled coordinates, so measuring raw events would put
/// every threshold in a different space from the gate it is meant to move.
/// Returns the files it could read, paired with the id the gating document
/// knows them by, and a line per file it could not.
fn load_scaled(
    dir: &str,
    names: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    arcsinh: &[(Arc<str>, f32)],
) -> (Vec<(Arc<str>, polars::prelude::DataFrame)>, Vec<String>) {
    use polars::prelude::*;

    let mut loaded = Vec::new();
    let mut problems = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => return (loaded, vec![format!("{dir}: {e}")]),
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fcs")))
        .collect();
    paths.sort();

    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // The metadata export's name, exactly. Matching on a stem instead is
        // how one donor's gates came to be scored against another's population.
        let Some(id) = names.get(name) else {
            problems.push(format!("{name}: no metadata row with this name"));
            continue;
        };
        let frame = (|| -> anyhow::Result<DataFrame> {
            let fcs = flow_fcs::Fcs::open(path.to_str().unwrap_or_default())?;
            let params: Vec<(&str, f32)> = arcsinh.iter().map(|(k, v)| (k.as_ref(), *v)).collect();
            let scaled = (*fcs.apply_arcsinh_transforms(&params)?).clone();
            Ok(scaled.with_row_index("original_index".into(), None)?)
        })();
        match frame {
            Ok(df) => loaded.push((id.clone(), df)),
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    (loaded, problems)
}
