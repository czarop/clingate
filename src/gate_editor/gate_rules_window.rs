//! The Gate Rules tab: say where each gate belongs, once.
//!
//! A rule names a *population* - "Ki67+ of CD4+" - rather than a container, so
//! one rule covers every place that gate appears. In a real export "CD279+"
//! occupied twenty-five containers; four rules covered the whole panel.

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::pairing_controls::PairingColumns;
use crate::gate_editor::path_picker::{Pick, PickPath};
use clingate_core::axis_store::{AxisStore, AxisStoreStoreExt};
use clingate_core::gate_rules::autogate::{Report, describe};
use clingate_core::gate_rules::choices::{carry_over, choices, describe_phenotype, marker_label};
use clingate_core::gate_rules::rule::{
    AboveTheNegativeRule, NegativeFinder, PercentileOffsetRule, PhenotypeRule, Rule, ShapeFit,
    TailFractionRule, ValleyRule,
};
use clingate_core::gate_rules::rule_store::{
    Bound, GateRule, MeasuredOn, RuleEntry, RuleStore, RuleTarget,
};
use clingate_core::gate_rules::run::{Progress, RunInputs, run_rules};
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::GateStateImplExt;
use clingate_core::omiq::metadata::{MetaDataStore, MetaDataStoreStoreExt};
use dioxus::prelude::*;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

static CSS_STYLE: Asset = asset!("assets/gate_rules.css");

/// Below this, a placement is worth opening by hand. Measured against the
/// hand-gated export: everything at or above it landed within 0.18 arcsinh
/// units of where a person had put it, which is a typical manual nudge.
const REVIEW_FLOOR: f64 = 0.30;

/// Below this, the verification table marks a gate's purity as worth a look.
///
/// Not a failure: a population that overlaps its neighbours on the two axes
/// the gate is drawn on cannot be gated cleanly there by anything, and the
/// honest response is to gate it somewhere else. The mark says "this number is
/// the reason to doubt the gate", which is what a person scanning a run needs.
const PURE_ENOUGH: f64 = 0.70;

/// How far a marker's centre may differ between the reference and a sample
/// before the table marks it.
///
/// In spreads of each sample's own parent, so it is already comparable. Three
/// is generous - a population really does shift between donors - and it is
/// there to catch the case that matters: a marker reading +8 on the reference
/// and +1 here has not been matched on, whatever the overall distance said.
const MARKER_DISAGREEMENT: f64 = 3.0;

/// A count as a percentage of its whole, for the report's tables.
fn fraction(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "-".to_string();
    }
    format!("{:.2}%", part as f64 / whole as f64 * 100.0)
}

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

/// Raise a running solve's stop flag the moment anything it reads changes:
/// the gates, the files, their compensation, the metadata, the scaling or the
/// rules.
///
/// A run works on a snapshot and writes its answers when it finishes. The tabs
/// are hidden rather than unmounted, so it carries on while the Workspace tab
/// loads another document or the editor moves a gate, and its answers would
/// then land on a document it never measured. Raising the flag stops it at
/// the next file instead of finishing work that will be thrown away.
///
/// This is only the early stop. Whether to write is decided when the run
/// ends, by comparing what it started from with what is there now - an
/// effect runs after the change that fires it, and a run can finish in
/// between.
///
/// Subscribes to the parts of the gate store that hold the gating, not to its
/// root: selecting a gate writes the store too, and that is not a change.
/// ([`GateStateImplExt::subscribe_to_gating`] is where that line is drawn.)
pub(crate) fn use_stop_run_on_change(cancel: Signal<Option<Arc<std::sync::atomic::AtomicBool>>>) {
    let gates = use_context::<crate::gate_editor::workspace_window::GateStore>();
    let metadata = use_context::<crate::gate_editor::workspace_window::MetadataStore>();
    let axes = use_context::<crate::gate_editor::workspace_window::AxesStore>();
    let rules = use_context::<Signal<RuleStore>>();
    let files = use_context::<Signal<Option<clingate_core::file_load::FcsFiles>>>();
    let compensation = use_context::<Signal<clingate_core::compensation::groups::Compensation>>();
    use_effect(move || {
        gates.subscribe_to_gating();
        let _ = compensation.read();
        let _ = metadata.metadata().read();
        let _ = metadata.file_name_to_gating_id().read();
        let _ = axes.settings().read();
        let _ = rules.read();
        let _ = files.read();
        if let Some(flag) = cancel.peek().as_ref() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

#[component]
pub fn GateRulesWindow() -> Element {
    let mut gate_store = use_context::<Store<GateState, CopyValue<GateState, SyncStorage>>>();
    let edits = use_context::<crate::gate_editor::edits::Edits>();
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
    let mut finder = use_signal(|| NegativeFinder::default().key().to_string());
    let mut scale = use_signal(|| "1.0".to_string());
    let mut smoothing = use_signal(|| "1.0".to_string());
    let mut nudge = use_signal(|| "0.0".to_string());
    // The phenotype rule's own fields. `outline_smoothing` is separate from
    // `smoothing` above even though the two are never on screen together: one
    // scales a density's bandwidth along one axis and the other a boundary's
    // in two, and a shared signal would carry a number tuned for one rule into
    // the other.
    let mut fit = use_signal(|| ShapeFit::default().key().to_string());
    let mut markers = use_signal(Vec::<String>::new);
    let mut keep = use_signal(|| "95".to_string());
    let mut outline_smoothing = use_signal(|| "1.0".to_string());
    let mut vertices = use_signal(|| "24".to_string());
    let mut gated_file = use_signal(String::new);
    let mut reference_type = use_signal(|| "FMX".to_string());
    let mut reference_file = use_signal(String::new);
    // The workspace's files: what a run measures. The same list the editor and
    // the gallery show, so the three cannot be looking at different
    // experiments.
    let filehandler = use_context::<Signal<Option<clingate_core::file_load::FcsFiles>>>();
    let compensation = use_context::<Signal<clingate_core::compensation::groups::Compensation>>();
    let mut running = use_signal(|| false);
    let mut report = use_signal(|| None::<Report>);
    let mut sidecar = use_signal(|| "gate_rules.json".to_string());
    let toasts = use_toast();
    // Not a message: it says what the form in front of you is currently doing,
    // and has to stay readable while you fill it in. A toast that faded after
    // five seconds would take away the one thing that distinguishes editing a
    // rule from adding one.
    let mut editing_note = use_signal(|| None::<String>);
    let mut progress = use_signal(|| None::<Progress>);
    // Set while a run is in flight, so the Stop button has something to raise.
    let mut cancel = use_signal(|| None::<Arc<std::sync::atomic::AtomicBool>>);
    use_stop_run_on_change(cancel);

    // Picking a gate offers only the parents and parameters that gate is drawn
    // with, so the form cannot name a combination the document does not have.
    // Picking a parent narrows the gates, and picking a gate narrows the
    // parameters, so the form cannot name a combination the document lacks.
    let selected_children = use_memo(move || choices.read().children_of(&parent()).to_vec());
    let selected_parameters = use_memo(move || choices.read().parameters_of(&gate()).to_vec());
    // Every channel the panel carries, for the phenotype rule's marker picker.
    // The gate's own two parameters are not enough: a population is identified
    // by markers the plot it is drawn on says nothing about, which is the
    // entire reason that rule exists.
    let panel = use_memo(move || {
        axis_store
            .sorted_settings()
            .read()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    });

    // The rule the form is standing in for, when it was opened by Edit. The
    // next Add replaces it, so a rule can be moved to another population rather
    // than only removed and retyped. Duplicate leaves this empty, which is the
    // quick way to cover a second population with the same rule.
    let mut editing = use_signal(|| None::<RuleTarget>);

    // The last run's report names gate positions in a document that has gone
    // once the gates are replaced, and is cleared with it.
    let generation = use_context::<Signal<crate::gate_editor::workspace_window::Generation>>();
    let document = use_memo(move || generation.read().document);
    use_effect(move || {
        document();
        report.set(None);
    });
    // A rule being edited that is no longer in the store - a new workspace
    // starts without rules - leaves the form claiming to edit nothing.
    use_effect(move || {
        let gone = editing
            .read()
            .as_ref()
            .is_some_and(|target| rules.read().get(target).is_none());
        if gone {
            editing.set(None);
            editing_note.set(None);
        }
    });

    let mut load = move |entry: RuleEntry, replacing: bool| {
        parent.set(
            entry
                .target
                .parent
                .clone()
                .map(|p| p.to_string())
                .unwrap_or_default(),
        );
        gate.set(entry.target.gate.to_string());
        parameter.set(entry.rule.parameter.to_string());
        bound.set(
            match entry.rule.bound {
                Bound::Above => "Above",
                Bound::Below => "Below",
            }
            .to_string(),
        );
        match &entry.rule.measured_on {
            MeasuredOn::Itself => measured_on.set("Itself".to_string()),
            MeasuredOn::Partner(t) => measured_on.set(t.to_string()),
            MeasuredOn::File(f) => calibrate_on.set(f.to_string()),
        }
        match &entry.rule.rule {
            Rule::TailFraction(r) => {
                kind.set("TailFraction".to_string());
                low.set(format!("{}", r.band.0 * 100.0));
                high.set(format!("{}", r.band.1 * 100.0));
            }
            Rule::PercentileOffset(r) => {
                kind.set("PercentileOffset".to_string());
                percentile.set(format!("{}", r.percentile));
                offset.set(format!("{}", r.offset));
            }
            Rule::AboveTheNegative(r) => {
                kind.set("AboveTheNegative".to_string());
                finder.set(r.find.key().to_string());
                scale.set(format!("{}", r.scale));
                nudge.set(format!("{}", r.nudge));
            }
            Rule::MatchThePhenotype(r) => {
                kind.set("MatchThePhenotype".to_string());
                fit.set(r.fit.key().to_string());
                markers.set(r.markers.iter().map(|m| m.to_string()).collect());
                keep.set(format!("{}", r.keep * 100.0));
                outline_smoothing.set(format!("{}", r.smoothing));
                vertices.set(format!("{}", r.vertices));
            }
            Rule::InTheValley(r) => {
                kind.set("InTheValley".to_string());
                smoothing.set(format!("{}", r.smoothing));
            }
        }
        editing.set(replacing.then(|| entry.target.clone()));
        editing_note.set(Some(if replacing {
            format!("Editing {} - Add rule saves it", entry.target.describe())
        } else {
            format!(
                "Copied {} - change it and Add rule",
                entry.target.describe()
            )
        }));
    };

    let mut add = move || {
        let name = gate();
        if name.is_empty() {
            warn(&toasts, "Choose a gate first");
            return;
        }
        let param = parameter();
        // A phenotype rule positions nothing along an axis, so there is no
        // parameter to name. Every other rule needs one.
        if param.is_empty() && kind() != "MatchThePhenotype" {
            warn(&toasts, "Choose the parameter the rule positions");
            return;
        }
        let rule = match kind().as_str() {
            "MatchThePhenotype" => {
                let (Ok(k), Ok(sm), Ok(v)) = (
                    keep().parse::<f64>(),
                    outline_smoothing().parse::<f64>(),
                    vertices().parse::<usize>(),
                ) else {
                    warn(
                        &toasts,
                        "The percentage, smoothing and point count must be numbers",
                    );
                    return;
                };
                if !(0.0..=100.0).contains(&k) {
                    warn(&toasts, "The percentage to hold must be between 0 and 100");
                    return;
                }
                if fit() == ShapeFit::DrawPolygon.key() && v < 3 {
                    warn(&toasts, "A polygon needs at least three points");
                    return;
                }
                Rule::MatchThePhenotype(PhenotypeRule {
                    markers: markers().iter().map(|m| Arc::from(m.as_str())).collect(),
                    fit: match fit().as_str() {
                        "DrawPolygon" => ShapeFit::DrawPolygon,
                        _ => ShapeFit::KeepShape,
                    },
                    // Typed as a percentage, stored as a fraction.
                    keep: k / 100.0,
                    smoothing: sm,
                    vertices: v,
                })
            }
            "InTheValley" => {
                let Ok(sm) = smoothing().parse::<f64>() else {
                    warn(&toasts, "The smoothing must be a number");
                    return;
                };
                Rule::InTheValley(ValleyRule {
                    smoothing: sm,
                    ..ValleyRule::default()
                })
            }
            "AboveTheNegative" => {
                let (Ok(s), Ok(n)) = (scale().parse::<f64>(), nudge().parse::<f64>()) else {
                    warn(&toasts, "The scale and nudge must be numbers");
                    return;
                };
                Rule::AboveTheNegative(AboveTheNegativeRule {
                    scale: s,
                    nudge: n,
                    find: match finder().as_str() {
                        "NegativePeak" => NegativeFinder::NegativePeak,
                        _ => NegativeFinder::BelowTheGate,
                    },
                    ..AboveTheNegativeRule::default()
                })
            }
            "PercentileOffset" => {
                let (Ok(p), Ok(o)) = (percentile().parse::<f64>(), offset().parse::<f64>()) else {
                    warn(&toasts, "The percentile and offset must be numbers");
                    return;
                };
                Rule::PercentileOffset(PercentileOffsetRule::new(p, o))
            }
            _ => {
                let (Ok(l), Ok(h)) = (low().parse::<f64>(), high().parse::<f64>()) else {
                    warn(&toasts, "The band must be two numbers");
                    return;
                };
                if l > h {
                    warn(&toasts, "The band's lower bound is above its upper");
                    return;
                }
                // Typed as percentages, stored as fractions.
                Rule::TailFraction(TailFractionRule::new((l / 100.0, h / 100.0)))
            }
        };
        // All three read a named reference sample rather than a partner of
        // each specimen.
        let calibrated = matches!(
            kind().as_str(),
            "AboveTheNegative" | "InTheValley" | "MatchThePhenotype"
        );
        if calibrated && calibrate_on().is_empty() {
            warn(&toasts, "Choose the sample to calibrate against");
            return;
        }
        let target = match parent().as_str() {
            "" => RuleTarget::named(name.as_str()),
            p => RuleTarget::under(name.as_str(), p),
        };
        let described = target.describe();
        // Moved to another population: the rule leaves where it was rather than
        // being copied there, which is what Edit means.
        if let Some(was) = editing.take()
            && was != target
        {
            rules.write().remove(&was);
        }
        rules.write().insert(
            target,
            GateRule {
                parameter: Arc::from(param.as_str()),
                bound: if bound() == "Below" {
                    Bound::Below
                } else {
                    Bound::Above
                },
                measured_on: if calibrated {
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
        say(&toasts, format!("Rule set for {described}"));
    };

    rsx! {
        document::Link { rel: "stylesheet", href: CSS_STYLE }
        div { class: "gate_rules",
            h2 { "Gate rules" }
            crate::gate_editor::edits::EditBar {}
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
                                // A phenotype rule positions nothing along an
                                // axis, so showing the parameter and the side
                                // it keeps would be showing two fields it does
                                // not read.
                                if matches!(entry.rule.rule, Rule::MatchThePhenotype(_)) {
                                    td { class: "gate_rules-hint", "the whole shape" }
                                    td { }
                                } else {
                                    td { "{entry.rule.parameter}" }
                                    td {
                                        match entry.rule.bound {
                                            Bound::Above => "above",
                                            Bound::Below => "below",
                                        }
                                    }
                                }
                                td {
                                    match &entry.rule.measured_on {
                                        MeasuredOn::Itself => "the sample itself".to_string(),
                                        MeasuredOn::Partner(t) => format!("its {t}"),
                                        MeasuredOn::File(f) => name_of(&files.read(), f),
                                    }
                                }
                                // A phenotype rule names its markers by the
                                // column it reads, which is what it has to
                                // store and not what anybody ticked. The
                                // panel is the only place the two are
                                // connected, and it lives here rather than in
                                // the rule.
                                match &entry.rule.rule {
                                    Rule::MatchThePhenotype(r) => rsx! {
                                        td { "{describe_phenotype(r, &panel.read())}" }
                                    },
                                    other => rsx! {
                                        td { "{other.describe()}" }
                                    },
                                }
                                td { class: "gate_rules-actions",
                                    button {
                                        class: "gate_rules-secondary",
                                        title: "Open this rule in the form below. Saving replaces it, so changing the population moves the rule rather than copying it.",
                                        onclick: {
                                            let entry = entry.clone();
                                            move |_| load(entry.clone(), true)
                                        },
                                        "edit"
                                    }
                                    button {
                                        class: "gate_rules-secondary",
                                        title: "Open a copy in the form below, leaving this one alone - the quick way to cover a second population with the same rule.",
                                        onclick: {
                                            let entry = entry.clone();
                                            move |_| load(entry.clone(), false)
                                        },
                                        "duplicate"
                                    }
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
                        let chosen = e.value();
                        let (keep_gate, keep_parameter) = carry_over(
                            &choices.read(),
                            &chosen,
                            &gate(),
                            &parameter(),
                        );
                        parent.set(chosen);
                        gate.set(keep_gate);
                        parameter.set(keep_parameter);
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
                    // `selected` on the option as well as `value` on the
                    // select. Edit sets the population and the gate in one go,
                    // and this list is derived from the population - so the
                    // value can reach the select before the matching option
                    // exists, and a select given a value it has no option for
                    // falls back to the first one. Opening a rule then showed
                    // "choose a gate" for a rule that plainly named one.
                    // Marking the option is order-independent: the browser
                    // honours it whenever the option is appended.
                    //
                    // The population's own select needs none of this, because
                    // its options do not depend on anything the same update
                    // sets.
                    for name in selected_children.read().clone() {
                        option {
                            value: "{name}",
                            selected: gate() == *name,
                            "{name}"
                        }
                    }
                }

                // A phenotype rule fits a whole shape to a population, so it
                // has no parameter it positions along and no leading side.
                // Leaving the fields on screen would invite a person to set
                // something the rule then ignores.
                if kind() != "MatchThePhenotype" {
                    label { "Positions on" }
                    select {
                        value: "{parameter}",
                        onchange: move |e| parameter.set(e.value()),
                        option { value: "", "choose a parameter" }
                        // Derived from the gate, which Edit sets in the same
                        // update - see the note on the gate's own list.
                        for name in selected_parameters.read().clone() {
                            option {
                                value: "{name}",
                                selected: parameter() == *name,
                                "{name}"
                            }
                        }
                    }

                    label { "Gate keeps events" }
                    select {
                        value: "{bound}",
                        onchange: move |e| bound.set(e.value()),
                        option { value: "Above", "above the line" }
                        option { value: "Below", "below the line" }
                    }
                }

                // The calibrated rules name one reference file rather than a
                // partner of each specimen, so the partner field means nothing.
                if !matches!(kind().as_str(), "AboveTheNegative" | "InTheValley" | "MatchThePhenotype") {
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
                    option { value: "InTheValley", "in the valley between the negative and the positive" }
                    option { value: "MatchThePhenotype", "find the cells that match the reference population" }
                }

                if kind() == "MatchThePhenotype" {
                    p { class: "gate_rules-hint gate_rules-span",
                        "Describes the cells inside the gate on the reference sample by where they sit across the markers below, then finds the same cells in every other sample and fits the gate to wherever they turn out to be. For populations the other rules cannot reach: a smear with no dip, several clusters near each other, anything that moves in both axes at once. Nothing is normalised between samples - each one's markers are read against its own parent - so donor differences are carried rather than flattened."
                    }

                    label { "Calibrate on" }
                    select {
                        value: "{calibrate_on}",
                        onchange: move |e| calibrate_on.set(e.value()),
                        option { value: "", "choose the reference sample" }
                        // Marked, not only valued: see the gate's list.
                        for (name , id) in files.read().clone() {
                            option {
                                value: "{id}",
                                selected: calibrate_on() == *id,
                                "{name}"
                            }
                        }
                    }

                    label { "Identified by" }
                    div { class: "gate_rules-markers",
                        if panel.read().is_empty() {
                            span { class: "gate_rules-hint",
                                "No panel loaded yet - open a file on the plots tab first."
                            }
                        }
                        for param in panel.read().clone() {
                            label { class: "gate_rules-marker",
                                input {
                                    r#type: "checkbox",
                                    checked: markers().iter().any(|m| *m == *param.fluoro),
                                    onchange: {
                                        let fluoro = param.fluoro.clone();
                                        move |e: FormEvent| {
                                            let name = fluoro.to_string();
                                            let mut chosen = markers();
                                            if e.checked() {
                                                if !chosen.contains(&name) {
                                                    chosen.push(name);
                                                }
                                            } else {
                                                chosen.retain(|m| *m != name);
                                            }
                                            markers.set(chosen);
                                        }
                                    },
                                }
                                "{param}"
                            }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        if markers().is_empty() {
                            "Nothing ticked means the whole panel, which is a reasonable place to start. Narrowing it is usually better: a marker that says nothing about this population still contributes noise to the distance, so ticking the four or five that define it beats ticking thirty."
                        } else {
                            "{markers().len()} ticked. A marker that says nothing about this population still contributes noise to the distance, so fewer and more relevant beats more."
                        }
                    }

                    label { "Then" }
                    select {
                        value: "{fit}",
                        onchange: move |e| fit.set(e.value()),
                        for choice in ShapeFit::ALL {
                            option { value: "{choice.key()}", "{choice.choice()}" }
                        }
                    }

                    if fit() == ShapeFit::DrawPolygon.key() {
                        p { class: "gate_rules-hint gate_rules-span",
                            "The gate becomes a polygon whatever it is now, because no other shape can follow a traced boundary. A rectangle or an ellipse will stop being one."
                        }

                        label { "Hold this much (%)" }
                        input {
                            r#type: "number",
                            step: "1",
                            value: "{keep}",
                            oninput: move |e| keep.set(e.value()),
                        }
                        p { class: "gate_rules-hint gate_rules-span",
                            "Of the matched cells. Not all of them: the last few percent are the ones the match is least sure about, and a boundary drawn to include them is drawn around the doubt."
                        }

                        label { "Boundary smoothing" }
                        input {
                            r#type: "number",
                            step: "0.1",
                            value: "{outline_smoothing}",
                            oninput: move |e| outline_smoothing.set(e.value()),
                        }
                        p { class: "gate_rules-hint gate_rules-span",
                            "Below 1 follows the cells more closely and picks up their noise; above 1 gives a smoother outline that may cut corners off a genuinely angular population."
                        }

                        label { "About this many points" }
                        input {
                            r#type: "number",
                            step: "1",
                            value: "{vertices}",
                            oninput: move |e| vertices.set(e.value()),
                        }
                        p { class: "gate_rules-hint gate_rules-span",
                            "A gate with two hundred points is a different kind of object from one drawn by hand, however well it fits."
                        }
                    } else {
                        p { class: "gate_rules-hint gate_rules-span",
                            "The gate is moved and resized onto the matched cells and keeps its shape and its kind - a rectangle stays a rectangle. Use this where the outline means something the data does not: a quadrant, a shape agreed with somebody else, a gate that has to stay comparable with how it was drawn before."
                        }
                    }
                }

                if kind() == "InTheValley" {
                    label { "Calibrate on" }
                    select {
                        value: "{calibrate_on}",
                        onchange: move |e| calibrate_on.set(e.value()),
                        option { value: "", "choose the reference sample" }
                        // Marked, not only valued: see the gate's list.
                        for (name , id) in files.read().clone() {
                            option {
                                value: "{id}",
                                selected: calibrate_on() == *id,
                                "{name}"
                            }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Finds the dip between the negative and the positive on each sample and puts the gate at its lowest point, offset by however far from the bottom the gate sits on the reference. It reads the boundary rather than pacing out from the negative's centre, so nothing is multiplied and a shallower dip still places correctly. It needs two populations: where the positives are a smear with no peak of their own, use above-the-negative instead."
                    }

                    p { class: "gate_rules-hint gate_rules-span",
                        "A shallower dip than the reference's still gets a gate - refusing hid the answer exactly where it was most wanted - but it is scored on how deep it is against the reference's, and a shallow one rises to the top for review. Only a density with no dip at all is left unplaced, because then there is nothing to place."
                    }

                    label { "Smoothing" }
                    input {
                        r#type: "number",
                        step: "0.1",
                        value: "{smoothing}",
                        oninput: move |e| smoothing.set(e.value()),
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Scales the density's bandwidth. Below 1 finds shallower dips and more noise; above 1 smooths shallow ones away."
                    }
                }

                if kind() == "AboveTheNegative" {
                    label { "Calibrate on" }
                    select {
                        value: "{calibrate_on}",
                        onchange: move |e| calibrate_on.set(e.value()),
                        option { value: "", "choose the reference sample" }
                        // Marked, not only valued: see the gate's list.
                        for (name , id) in files.read().clone() {
                            option {
                                value: "{id}",
                                selected: calibrate_on() == *id,
                                "{name}"
                            }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Reads how far above that sample's negative its gate sits, in widths of that negative, and puts every other gate the same number of widths above its own. No FMO needed - the negative is read from the sample being gated."
                    }

                    label { "Find the negative" }
                    select {
                        value: "{finder}",
                        onchange: move |e| finder.set(e.value()),
                        for option_ in NegativeFinder::ALL {
                            option { value: "{option_.key()}", "{option_.choice()}" }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Pick by what the plot looks like. A real valley between negative and positive - use the events below the gate: the gate already sits in that valley, so everything under it is the negative, and it measured about five times the sharper. Dim cells rising out of the negative with no valley - use the negative's own peak: below-the-gate swallows the smear, and its centre drifts about 0.27 of a width as the smear grows from 4% to 35% of the population, where the peak finder drifts 0.09. Neither can tell a spillover shoulder from another channel apart from real dim expression, so place those by hand."
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
                } else if kind() == "TailFraction" {
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
                    // Marked, not only valued: see the gate's list.
                    for (name , id) in files.read().clone() {
                        option { value: "{id}", selected: gated_file() == *id, "{name}" }
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
                        option { value: "{id}", selected: reference_file() == *id, "{name}" }
                    }
                }

                button {
                    class: "gate_rules-add",
                    onclick: move |_| {
                        let (gated, reference) = (gated_file(), reference_file());
                        if gated.is_empty() || reference.is_empty() {
                            warn(&toasts, "Choose both files");
                            return;
                        }
                        rules
                            .write()
                            .set_reference(
                                Arc::from(gated.as_str()),
                                Arc::from(reference_type().as_str()),
                                Arc::from(reference.as_str()),
                            );
                        say(&toasts, "Reference set");
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

                p { class: "gate_rules-hint gate_rules-span",
                    match filehandler.read().as_ref().map(|f| f.sample_count()) {
                        Some(n) if n > 0 => format!("Runs over the {n} FCS files in the workspace."),
                        _ => "No FCS files are loaded - open a workspace on the first tab.".to_string(),
                    }
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
                        progress.set(Some(Progress::Measuring { done: 0, total: 0 }));

                        // Everything the run reads, as it stands now. Read
                        // again before anything is written: see `RunInputs`.
                        //
                        // The axis settings are compared whole, not only the
                        // cofactors the run reads: any new scaling is a new
                        // workspace as far as the answers are concerned.
                        let inputs_now = move || {
                            let axes = axis_store.settings().read().clone();
                            let inputs = RunInputs {
                                // Each file with the name the metadata knows it by.
                                files: filehandler
                                    .read()
                                    .as_ref()
                                    .map(|f| {
                                        f.file_list()
                                            .iter()
                                            .map(|stub| (stub.name.clone(), stub.get_filepath().to_owned()))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                compensation: compensation.read().clone(),
                                names: metadata_store.file_name_to_gating_id().read().clone(),
                                cofactors: RunInputs::cofactors_of(&axes),
                                metadata: metadata_store.metadata().read().clone(),
                                rules: rules.read().clone(),
                            };
                            (inputs, axes)
                        };
                        let started = inputs_now();
                        if started.0.files.is_empty() {
                            warn(&toasts, "No FCS files are loaded - open a workspace on the first tab");
                            running.set(false);
                            progress.set(None);
                            return;
                        }

                        // A snapshot, not a lock. Every gate is behind an Arc,
                        // so this is a refcount bump rather than a copy, and
                        // the store is free for the rest of the editor the
                        // moment it is taken.
                        let snapshot = gate_store.read().clone();
                        let started_gates = snapshot.clone();

                        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        cancel.set(Some(flag.clone()));
                        let stopped = flag.clone();
                        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();

                        let worker = {
                            let inputs = started.0.clone();
                            tokio::task::spawn_blocking(move || {
                                run_rules(
                                    &snapshot,
                                    &inputs,
                                    |step| {
                                        let _ = tx.send(step);
                                    },
                                    &flag,
                                )
                            })
                        };

                        // The worker's sender drops when it returns, which ends
                        // this loop - no sentinel message to get wrong.
                        while let Some(step) = rx.recv().await {
                            progress.set(Some(step));
                        }

                        let outcome = worker.await;
                        progress.set(None);
                        cancel.set(None);
                        running.set(false);

                        let outcome = match outcome {
                            Ok(o) => o,
                            Err(e) => {
                                warn(&toasts, format!("The run did not finish: {e}"));
                                return;
                            }
                        };
                        // Answers measured on one workspace mean nothing in
                        // another, whatever stopped or did not stop the run.
                        // Checked here rather than trusted to the stop flag:
                        // the flag is raised by an effect, which runs after the
                        // change that fires it, and the run can finish first.
                        if !gate_store.peek().unchanged_since(&started_gates) || inputs_now() != started {
                            warn(
                                &toasts,
                                "The workspace changed while the rules ran, so the run was stopped and no gates were moved - run it again on the workspace as it is now",
                            );
                            return;
                        }
                        if outcome.cancelled || stopped.load(std::sync::atomic::Ordering::Relaxed) {
                            note(&toasts, "Stopped - no gates were moved");
                            return;
                        }

                        // Writing happens here, on the one thread that owns the
                        // store, and only once the document is known to be the
                        // one the run measured.
                        // The whole run is one step of the working copy.
                        let before = edits.before();
                        clingate_core::gate_rules::autogate::apply_placements(
                            &mut gate_store.write(),
                            &outcome.placements,
                        );
                        edits.after(before);

                        let run = outcome.report;
                        say(
                            &toasts,
                            format!(
                                "Moved {} gates, left {} already in band and {} reference; {} need review",
                                run.positioned.len(),
                                run.unchanged.len(),
                                run.reference.len(),
                                run.needs_review(REVIEW_FLOOR).count()
                            ),
                        );
                        report.set(Some(run));
                    },
                    if running() { "Working..." } else { "Solve and apply" }
                }

                if let Some(step) = progress() {
                    div { class: "gate_rules-progress gate_rules-span",
                        div { class: "gate_rules-bar",
                            div {
                                class: "gate_rules-bar_fill",
                                style: "width: {step.fraction() * 100.0}%",
                            }
                        }
                        span { class: "gate_rules-progress_text", "{step.describe()}" }
                        button {
                            class: "gate_rules-cancel",
                            onclick: move |_| {
                                if let Some(flag) = cancel() {
                                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                    note(&toasts, "Stopping after this file...");
                                }
                            },
                            "Stop"
                        }
                    }
                }
            }

            // ── what it did ───────────────────────────────────────────────
            if let Some(run) = report.read().as_ref() {
                div { class: "gate_rules-report",
                    // Two kinds of placement, two tables. A phenotype rule's
                    // "from" and "to" are fractions of the parent and every
                    // other rule's are coordinates on an axis, so one table
                    // would put an axis value in the same column as a
                    // percentage and label them both From.
                    if run.positioned.iter().any(|p| p.phenotype.is_none()) {
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
                                    th { "Line only" }
                                    th { "Confidence" }
                                    th { "Weakest" }
                                }
                            }
                            tbody {
                                for placed in run.positioned.iter().filter(|p| p.phenotype.is_none()) {
                                    tr {
                                        class: if placed.confidence < REVIEW_FLOOR || !placed.in_band { "gate_rules-weak" } else { "" },
                                        td { "{placed.specimen}" }
                                        td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                        td { "{name_of(&files.read(), &placed.measured_on)}" }
                                        td { "{placed.from:.3}" }
                                        td { "{placed.to:.3}" }
                                        td {
                                            title: "of {placed.reference_events} events on {name_of(&files.read(), &placed.captured_on)}",
                                            "{placed.achieved * 100.0:.3}%"
                                            if !placed.in_band {
                                                " (outside the band - nearest achievable)"
                                            }
                                        }
                                        td {
                                            class: if placed.above_the_line > 0.0
                                                && placed.achieved < placed.above_the_line * 0.5 { "gate_rules-weak" } else { "" },
                                            title: "what a bare threshold on this parameter would take, the gate's other sides ignored - far above the captured figure means the gate's other axis is discarding the events",
                                            "{placed.above_the_line * 100.0:.3}%"
                                        }
                                        td { "{placed.confidence:.2}" }
                                        td { "{placed.weakest.unwrap_or(\"-\")}" }
                                    }
                                }
                            }
                        }
                    }
                    if run.positioned.iter().any(|p| p.phenotype.is_some()) {
                        h3 { "Matched by phenotype" }
                        p { class: "gate_rules-hint",
                            "A gate drawn round the wrong cells looks exactly like one drawn round the right cells until these are read. Each marker shows where the matched cells sat on the reference and where they sit here, both in spreads of their own parent - the two should agree, because they are supposed to be the same cells."
                        }
                        div { class: "gate_rules-verify",
                            table { class: "gate_rules-table",
                                thead {
                                    tr {
                                        th { "Specimen" }
                                        th { "Gate" }
                                        th { "Matched" }
                                        th { "On the reference" }
                                        th { "Only this population" }
                                        th { "Of the population" }
                                        th { "Clouds" }
                                        th { "Fitted" }
                                        th { "Marker, reference to here" }
                                        th { "Confidence" }
                                        th { "Weakest" }
                                    }
                                }
                                tbody {
                                    for placed in run.positioned.iter() {
                                        if let Some(read) = placed.phenotype.as_ref() {
                                            tr {
                                                class: if placed.confidence < REVIEW_FLOOR { "gate_rules-weak" } else { "" },
                                                td { "{placed.specimen}" }
                                                td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                                td {
                                                    title: "of {read.parent} events in the parent population",
                                                    "{read.matched} ({fraction(read.matched, read.parent)})"
                                                }
                                                td {
                                                    title: "what the hand-drawn gate held on {name_of(&files.read(), &placed.measured_on)}",
                                                    "{read.reference_matched} ({fraction(read.reference_matched, read.reference_parent)})"
                                                }
                                                td {
                                                    class: if read.purity < PURE_ENOUGH { "gate_rules-doubt" } else { "" },
                                                    title: "of what the fitted gate holds. Low means the population is not separated on these two axes, so no boundary round it can exclude its neighbours - which is a fact about the plot, not a fault in the fit",
                                                    "{read.purity * 100.0:.0}%"
                                                }
                                                td {
                                                    title: "how much of the matched population the fitted gate holds",
                                                    "{read.caught * 100.0:.0}%"
                                                }
                                                td {
                                                    class: if read.pieces > 1 { "gate_rules-doubt" } else { "" },
                                                    title: if read.pieces > 1 { "the matched cells sit in more than one place on this plot, so one outline is not the whole story" } else { "the matched cells form a single cloud" },
                                                    "{read.pieces}"
                                                }
                                                td {
                                                    match read.reshaped {
                                                        Some((dx, dy)) => format!(
                                                            "moved {dx:+.0}, {dy:+.0}{}",
                                                            if read.clamped { " (stretch clamped)" } else { "" },
                                                        ),
                                                        None => "new polygon".to_string(),
                                                    }
                                                }
                                                td {
                                                    for (marker , there , here) in read.centres.iter() {
                                                        span {
                                                            class: if (there - here).abs() > MARKER_DISAGREEMENT { "gate_rules-doubt" } else { "" },
                                                            title: "{marker}: {there:+.1} spreads on the reference, {here:+.1} here",
                                                            // The marker as it was ticked, not the
                                                            // column it was read from.
                                                            "{marker_label(marker, &panel.read())} {there:+.1}→{here:+.1}  "
                                                        }
                                                    }
                                                }
                                                td { "{placed.confidence:.2}" }
                                                td { "{placed.weakest.unwrap_or(\"-\")}" }
                                            }
                                        }
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
                                    th { "Line only" }
                                }
                            }
                            tbody {
                                for kept in run.unchanged.iter() {
                                    tr {
                                        td { "{kept.specimen}" }
                                        td { "{describe(&kept.gate, kept.parent_gate.as_deref())}" }
                                        td { "{kept.achieved * 100.0:.3}%" }
                                        td { "{kept.above_the_line * 100.0:.3}%" }
                                    }
                                }
                            }
                        }
                    }
                    if run.positioned.iter().any(|p| p.valley.is_some()) {
                        h3 { "How the valley was read" }
                        p { class: "gate_rules-note",
                            "The gate sits at the lowest point of the dip, offset by however far from the bottom it sits on the reference. Depth is how far the dip falls below the lower of the two peaks either side: a shallower dip than the reference's still places correctly, but one near zero means the two populations have merged."
                        }
                        table { class: "gate_rules-table",
                            thead {
                                tr {
                                    th { "Specimen" }
                                    th { "Gate" }
                                    th { "Ref peak" }
                                    th { "Ref bottom" }
                                    th { "Ref depth" }
                                    th { "Peak" }
                                    th { "Bottom" }
                                    th { "Depth" }
                                    th { "Offset" }
                                    th { "Placed at" }
                                }
                            }
                            tbody {
                                for (placed , (reference , here)) in run
                                    .positioned
                                    .iter()
                                    .filter_map(|p| p.valley.map(|v| (p, v)))
                                {
                                    tr {
                                        td { "{placed.specimen}" }
                                        td { "{describe(&placed.gate, placed.parent_gate.as_deref())}" }
                                        td { "{reference.peak:.3}" }
                                        td { "{reference.bottom:.3}" }
                                        td { "{reference.depth:.3}" }
                                        td { "{here.peak:.3}" }
                                        td { "{here.bottom:.3}" }
                                        td {
                                            class: if here.depth < reference.depth * 0.5 { "gate_rules-weak" } else { "" },
                                            title: "against the reference's dip",
                                            "{here.depth:.3}"
                                        }
                                        td { "{here.offset:+.3}" }
                                        td { "{here.at:.3}" }
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
                                    th { "Line only" }
                                }
                            }
                            tbody {
                                for kept in run.reference.iter() {
                                    tr {
                                        td { "{kept.specimen}" }
                                        td { "{describe(&kept.gate, kept.parent_gate.as_deref())}" }
                                        td { "{kept.achieved * 100.0:.3}%" }
                                        td {
                                            class: if kept.above_the_line > 0.0
                                                && kept.achieved < kept.above_the_line * 0.5 { "gate_rules-weak" } else { "" },
                                            "{kept.above_the_line * 100.0:.3}%"
                                        }
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
                                    // The file's own name: an opaque gating id
                                    // names a file nobody can look up.
                                    if missed.file.is_empty() {
                                        "{describe(&missed.gate, missed.parent_gate.as_deref())}: {missed.reason}"
                                    } else {
                                        "{describe(&missed.gate, missed.parent_gate.as_deref())} on {name_of(&files.read(), &missed.file)}: {missed.reason}"
                                    }
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
                div { class: "gate_rules-path",
                    input {
                        value: "{sidecar}",
                        oninput: move |e| sidecar.set(e.value()),
                    }
                    // Choosing one that exists, for Load. Naming one to write
                    // is the button beside Save; they are different dialogs,
                    // and an open dialog cannot name a file that is not there.
                    PickPath {
                        path: sidecar,
                        mode: Pick::OpenFile,
                        label: "Rules",
                        extensions: vec!["json".to_string()],
                    }
                }
                div { class: "gate_rules-band gate_rules-actions_row",
                    button {
                        onclick: move |_| {
                            let path = PathBuf::from(sidecar());
                            match rules.read().save(&path) {
                                Ok(()) => say(&toasts, format!("Saved to {}", path.display())),
                                Err(e) => warn(&toasts, format!("Could not save: {e}")),
                            }
                        },
                        "Save"
                    }
                    // Joined to Save, not floating between the two actions:
                    // this dialog names where to write, which is Save's
                    // question and not Load's.
                    PickPath {
                        path: sidecar,
                        mode: Pick::SaveFile,
                        label: "Rules",
                        extensions: vec!["json".to_string()],
                    }
                    span { class: "gate_rules-gap" }
                    button {
                        onclick: move |_| {
                            let path = PathBuf::from(sidecar());
                            match RuleStore::load(&path) {
                                Ok(loaded) => {
                                    let n = loaded.len();
                                    rules.set(loaded);
                                    say(&toasts, format!("Loaded {n} rules"));
                                }
                                Err(e) => warn(&toasts, format!("Could not load: {e}")),
                            }
                        },
                        "Load"
                    }
                }
            }

            if let Some(text) = editing_note() {
                p { class: "gate_rules-message", "{text}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B-RUN-1: a run must stop when anything it read changes. Driven through
    /// the real hook, in a headless `VirtualDom` holding the same stores and
    /// signals the app provides.
    mod a_run_stops_when_anything_it_read_changes {
        use super::*;
        use crate::gate_editor::workspace_window::{AxesStore, GateStore, MetadataStore};
        use clingate_core::axis_store::PlotMapper;
        use clingate_core::gates::gate_store::{GateSource, GateStateStoreExt};
        use clingate_core::gates::gate_types::PrimaryGateType;
        use dioxus::stores::use_store_sync;
        use dioxus_core::{NoOpMutations, generation};
        use std::sync::atomic::{AtomicBool, Ordering};

        /// Everything the hook watches, handed to the change under test.
        #[derive(Clone, Copy)]
        struct Held {
            gates: GateStore,
            metadata: MetadataStore,
            axes: AxesStore,
            rules: Signal<RuleStore>,
            files: Signal<Option<clingate_core::file_load::FcsFiles>>,
        }

        #[derive(Clone)]
        struct Setup {
            change: Rc<dyn Fn(Held)>,
            flag: Arc<AtomicBool>,
        }
        impl PartialEq for Setup {
            fn eq(&self, _: &Self) -> bool {
                true
            }
        }
        use std::rc::Rc;

        fn mapper() -> PlotMapper {
            PlotMapper::new(
                600.0,
                600.0,
                0.0..=1000.0,
                0.0..=1000.0,
                0.0..=1000.0,
                0.0..=1000.0,
                flow_fcs::TransformType::Linear,
                flow_fcs::TransformType::Linear,
            )
        }

        /// A document with one rectangle at the root, as the editor draws it.
        fn document() -> GateState {
            let mut state = GateState::default();
            state
                .add_gate(
                    &mapper(),
                    300.0,
                    300.0,
                    Arc::from("FSC-A"),
                    Arc::from("SSC-A"),
                    None,
                    None,
                    PrimaryGateType::Rectangle,
                    Some("r".to_string()),
                )
                .unwrap();
            state
        }

        /// Mount the hook with nothing running, start a run, make `change`,
        /// and say whether the run's stop flag went up.
        fn stops_for(change: impl Fn(Held) + 'static) -> bool {
            let flag = Arc::new(AtomicBool::new(false));
            let mut dom = VirtualDom::new_with_props(
                |setup: Setup| {
                    let gates = use_store_sync(document);
                    let metadata = use_store_sync(MetaDataStore::default);
                    let axes = use_store_sync(AxisStore::default);
                    let rules = use_signal(RuleStore::default);
                    let files = use_signal(|| None::<clingate_core::file_load::FcsFiles>);
                    use_context_provider(|| gates);
                    use_context_provider(|| metadata);
                    use_context_provider(|| axes);
                    use_context_provider(|| rules);
                    use_context_provider(|| files);
                    let compensation =
                        use_signal(clingate_core::compensation::groups::Compensation::default);
                    use_context_provider(|| compensation);
                    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);
                    use_stop_run_on_change(cancel);
                    // Pass 1 starts the run; pass 2 makes the change.
                    if generation() == 1 {
                        cancel.set(Some(setup.flag.clone()));
                    }
                    if generation() == 2 {
                        (setup.change)(Held {
                            gates,
                            metadata,
                            axes,
                            rules,
                            files,
                        });
                    }
                    rsx! {}
                },
                Setup {
                    change: Rc::new(change),
                    flag: flag.clone(),
                },
            );
            // Effects run from `wait_for_work`, not from a render, so each pass
            // renders and then lets the queued effects run.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            let settle = |dom: &mut VirtualDom| {
                runtime.block_on(async {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_millis(20),
                        dom.wait_for_work(),
                    )
                    .await;
                });
                dom.render_immediate(&mut NoOpMutations);
            };
            dom.rebuild_in_place();
            settle(&mut dom);
            for _ in 0..3 {
                dom.mark_dirty(ScopeId::APP);
                dom.render_immediate(&mut NoOpMutations);
                settle(&mut dom);
            }
            flag.load(Ordering::Relaxed)
        }

        #[test]
        fn nothing_changing_does_not_stop_it() {
            // Guard for every test below: mounting, starting the run and
            // re-rendering are not changes, or they would all pass for free.
            assert!(!stops_for(|_| {}));
        }

        #[test]
        fn a_gate_moved_in_the_editor_stops_it() {
            assert!(stops_for(|held| {
                let mut gates = held.gates;
                let id = gates.peek().registered_ids()[0].clone();
                let moved = gates.peek().registered_gate(&id).unwrap();
                gates.write().place_gate(
                    std::slice::from_ref(&id),
                    &moved,
                    &GateSource::Sample((id.clone(), Arc::from("s1"))),
                );
            }));
        }

        #[test]
        fn a_gating_file_loaded_over_the_document_stops_it() {
            assert!(stops_for(|held| {
                let mut gates = held.gates;
                *gates.write() = GateState::default();
            }));
        }

        #[test]
        fn new_metadata_stops_it() {
            assert!(stops_for(|held| {
                let metadata = held.metadata;
                metadata
                    .metadata()
                    .write()
                    .insert(Arc::from("f1"), Default::default());
            }));
        }

        #[test]
        fn a_new_scaling_stops_it() {
            assert!(stops_for(|held| {
                let mut axes = held.axes;
                axes.with_mut(|a| a.replace_axis_configs(vec![clingate_core::AxisInfo::default()]));
            }));
        }

        #[test]
        fn an_edited_rule_stops_it() {
            assert!(stops_for(|held| {
                let mut rules = held.rules;
                rules.write().pairing.sample_id_column = Arc::from("Donor");
            }));
        }

        #[test]
        fn a_changed_file_list_stops_it() {
            assert!(stops_for(|held| {
                let mut files = held.files;
                files.set(Some(clingate_core::file_load::FcsFiles::default()));
            }));
        }

        #[test]
        fn selecting_a_gate_does_not_stop_it() {
            assert!(!stops_for(|held| {
                let gates = held.gates;
                let id = gates.peek().registered_ids()[0].clone();
                *gates.selected_gate().write() = Some(id);
            }));
        }

        #[test]
        fn showing_another_file_in_the_editor_does_not_stop_it() {
            // The editor re-matches its gates to the plot whenever the file
            // changes. With nothing to transpose that must not write.
            assert!(!stops_for(|held| {
                use clingate_core::gates::gate_store::GateStateImplExt as _;
                let mut gates = held.gates;
                let resolver = gates
                    .peek()
                    .get_current_sample(Arc::from("f2"), &Default::default());
                let parent =
                    gates
                        .peek()
                        .parent_node(&clingate_core::gates::gate_store::NodeId::from(
                            gates.peek().registered_ids()[0].clone(),
                        ));
                gates
                    .match_gates_to_plot(
                        Arc::<str>::from("FSC-A"),
                        Arc::<str>::from("SSC-A"),
                        parent.map(|p| p.as_arc().clone()),
                        &resolver,
                    )
                    .expect("the gate is on this plot");
            }));
        }
    }
}
