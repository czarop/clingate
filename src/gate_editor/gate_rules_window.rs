//! The Gate Rules tab: say where each gate belongs, once.
//!
//! A rule names a *population* - "Ki67+ of CD4+" - rather than a container, so
//! one rule covers every place that gate appears. In a real export "CD279+"
//! occupied twenty-five containers; four rules covered the whole panel.

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_single::boolean_gates::BooleanGate;
use crate::gate_editor::gates::gate_store::GateId;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::pairing_controls::PairingColumns;
use crate::gate_editor::path_picker::{Pick, PickPath};
use crate::gate_editor::plots::axis_store::{AxisStore, AxisStoreStoreExt};
use crate::gate_rules::autogate::{Report, describe, measure_file};
use crate::gate_rules::rule::{
    AboveTheNegativeRule, NegativeFinder, PercentileOffsetRule, PhenotypeRule, Rule, ShapeFit,
    TailFractionRule, ValleyRule,
};
use crate::gate_rules::rule_store::{
    Bound, GateRule, MeasuredOn, RuleEntry, RuleStore, RuleTarget,
};
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

/// What the gate and parameter fields should hold once the population changes.
///
/// Editing a rule is nearly always "the same gate, a different population":
/// that is what the Edit button is for, and clearing both fields on every
/// change of parent made it three picks instead of one - the slowest of them
/// being to find the gate again by name in a list that had just been rebuilt.
///
/// So a gate the new parent also holds is kept, and the parameter with it where
/// that gate is still drawn on it. A name the new parent does not hold is
/// cleared, because carrying it over would let the form name a combination the
/// document does not have, which is the one thing these narrowing lists exist
/// to prevent.
pub fn carry_over(
    choices: &GateChoices,
    parent: &str,
    gate: &str,
    parameter: &str,
) -> (String, String) {
    if gate.is_empty() || !choices.children_of(parent).iter().any(|g| &**g == gate) {
        return (String::new(), String::new());
    }
    let keep = !parameter.is_empty()
        && choices
            .parameters_of(gate)
            .iter()
            .any(|p| &**p == parameter);
    let parameter = if keep {
        parameter.to_string()
    } else {
        String::new()
    };
    (gate.to_string(), parameter)
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

/// A phenotype rule in the words a person used to write it.
///
/// [`Rule::describe`] names the markers by the column the rule reads, because
/// that is what it stores and what it has to store - a signature is built by
/// looking those columns up in a DataFrame. Nobody ticks a column called
/// `BV421-A`, though; they tick CD279. The panel is the only place the two are
/// connected and it is loaded per file, so the translation belongs here rather
/// than in the rule.
///
/// A marker the panel does not carry is shown as the rule stores it, which is
/// the honest answer: that is what the rule will look for.
/// One marker's name as a person would recognise it, from the column it is
/// read out of.
pub fn marker_label(
    column: &str,
    panel: &[crate::gate_editor::plots::axis_store::Param],
) -> String {
    panel
        .iter()
        .find(|param| &*param.fluoro == column)
        .map(|param| param.to_string())
        .unwrap_or_else(|| column.to_string())
}

pub fn describe_phenotype(
    rule: &PhenotypeRule,
    panel: &[crate::gate_editor::plots::axis_store::Param],
) -> String {
    let named = if rule.markers.is_empty() {
        "every marker".to_string()
    } else {
        rule.markers
            .iter()
            .map(|column| marker_label(column, panel))
            .collect::<Vec<String>>()
            .join(", ")
    };
    format!(
        "find the cells that match on {named}, then {}",
        rule.fit.label()
    )
}

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
    let mut finder = use_signal(|| NegativeFinder::default().key().to_string());
    let mut scale = use_signal(|| "1.0".to_string());
    let mut min_depth = use_signal(|| "0.25".to_string());
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
    let filehandler = use_context::<Signal<Option<crate::file_load::FcsFiles>>>();
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
                min_depth.set(format!("{}", r.min_depth_fraction));
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
                let (Ok(d), Ok(sm)) = (min_depth().parse::<f64>(), smoothing().parse::<f64>())
                else {
                    warn(&toasts, "The depth and smoothing must be numbers");
                    return;
                };
                Rule::InTheValley(ValleyRule {
                    min_depth_fraction: d,
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
                        for (name , id) in files.read().clone() {
                            option { value: "{id}", "{name}" }
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
                        for (name , id) in files.read().clone() {
                            option { value: "{id}", "{name}" }
                        }
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "Finds the dip between the negative and the positive on each sample and puts the gate at its lowest point, offset by however far from the bottom the gate sits on the reference. It reads the boundary rather than pacing out from the negative's centre, so nothing is multiplied and a shallower dip still places correctly. It needs two populations: where the positives are a smear with no peak of their own, use above-the-negative instead."
                    }

                    label { "Flag below" }
                    input {
                        r#type: "number",
                        step: "0.05",
                        value: "{min_depth}",
                        oninput: move |e| min_depth.set(e.value()),
                    }
                    p { class: "gate_rules-hint gate_rules-span",
                        "As a fraction of the reference's valley depth. A shallower dip below this still gets a gate - refusing hid the answer exactly where it was most wanted - but it is scored low and rises to the top for review. Only a density with no dip at all is left unplaced, because then there is nothing to place."
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

                        // Each file with the name the metadata knows it by.
                        let files: Vec<(Arc<str>, PathBuf)> = filehandler
                            .read()
                            .as_ref()
                            .map(|f| {
                                f.file_list()
                                    .iter()
                                    .map(|stub| (stub.name.clone(), stub.get_filepath().to_owned()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if files.is_empty() {
                            warn(&toasts, "No FCS files are loaded - open a workspace on the first tab");
                            running.set(false);
                            progress.set(None);
                            return;
                        }
                        let names = metadata_store.file_name_to_gating_id().read().clone();
                        let mut arcsinh: Vec<(Arc<str>, f32)> = Vec::new();
                        for (param, info) in axis_store.settings().read().iter() {
                            if info.is_arcsinh()
                                && let Some(cofactor) = info.get_cofactor()
                            {
                                arcsinh.push((param.clone(), cofactor));
                            }
                        }
                        let metadata = metadata_store.metadata().read().clone();
                        let rules_now = rules.read().clone();

                        // A snapshot, not a lock. Every gate is behind an Arc,
                        // so this is a refcount bump rather than a copy, and
                        // the store is free for the rest of the editor the
                        // moment it is taken.
                        let snapshot = gate_store.read().clone();

                        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        cancel.set(Some(flag.clone()));
                        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();

                        let worker = tokio::task::spawn_blocking(move || {
                            run_solve(
                                snapshot, files, names, arcsinh, metadata, rules_now, tx, flag,
                            )
                        });

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
                        if outcome.cancelled {
                            note(&toasts, "Stopped - no gates were moved");
                            return;
                        }

                        // Writing happens here, on the one thread that owns the
                        // store, and only the answers are written: a gate moved
                        // by hand while this ran keeps its position.
                        crate::gate_rules::autogate::apply_placements(
                            &mut gate_store.write(),
                            &outcome.placements,
                        );

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

/// What a run is doing, for the progress line.
#[derive(Clone, Copy, PartialEq)]
pub enum Progress {
    /// Reading and measuring go together: one file is read, measured, and
    /// dropped before the next is opened.
    Measuring { done: usize, total: usize },
    /// Every file is in; the rules are being solved.
    Solving { done: usize, total: usize },
}

impl Progress {
    fn fraction(self) -> f64 {
        match self {
            // Solving is the tail after the reading, so the bar carries on
            // through it rather than stopping dead on one message.
            Progress::Measuring { done, total } if total > 0 => 0.95 * (done as f64 / total as f64),
            Progress::Measuring { .. } => 0.0,
            Progress::Solving { done, total } if total > 0 => {
                0.95 + 0.05 * (done as f64 / total as f64)
            }
            Progress::Solving { .. } => 0.95,
        }
    }

    fn describe(self) -> String {
        match self {
            Progress::Measuring { done, total } => format!("Measuring file {done} of {total}"),
            Progress::Solving { done, total } if total > 0 => {
                format!("Solving rule {done} of {total}")
            }
            Progress::Solving { .. } => "Solving the rules...".to_string(),
        }
    }
}

/// The workspace's FCS files, each paired with the gating id the metadata
/// gives it.
///
/// Takes the files the workspace holds rather than reading a folder. The rules
/// tab used to walk its own folder field, which could point somewhere other
/// than the files the editor had open - two tabs measuring two different
/// experiments with nothing to say so.
///
/// Each file is looked up by its name in the program, not its name on disk:
/// for a file from a sub-folder those differ, and it is the program name the
/// metadata is written against.
fn files_to_read(
    files: &[(Arc<str>, PathBuf)],
    names: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
) -> (Vec<(PathBuf, Arc<str>)>, Vec<String>) {
    let mut files = files.to_vec();
    // Measuring runs in parallel and the results are flattened in this order,
    // so it has to be fixed - see `measure_all`.
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut problems = Vec::new();
    let mut found = Vec::new();
    for (name, path) in files {
        // The metadata export's name, exactly. Matching on a stem instead is
        // how one donor's gates came to be scored against another's population.
        match names.get(&name) {
            Some(id) => found.push((path, id.clone())),
            None => problems.push(format!("{name}: no metadata row with this name")),
        }
    }
    (found, problems)
}

/// Read and measure every file, several at a time.
///
/// Each file is independent: reading is I/O and parsing, measuring builds an
/// R-tree over that file's events alone, and the gate state is only read. The
/// frame is dropped as soon as its measurements are taken, so what is alive at
/// once is one frame per worker rather than the whole experiment.
///
/// Results are collected **in path order**, not completion order: `collect` on
/// an indexed parallel iterator returns elements in the iterator's order, and
/// the flattening below preserves it. That is what keeps the answer the same
/// whichever thread finishes first - `position_all` answers once per specimen
/// and takes the first file of each, so an order that varied with scheduling
/// would make two identical runs disagree. `files_to_read` sorts, so the
/// iterator's order is the sorted one.
#[allow(clippy::too_many_arguments)]
fn measure_all(
    snapshot: &GateState,
    files: &[(Arc<str>, PathBuf)],
    names: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    arcsinh: &[(Arc<str>, f32)],
    metadata: &crate::omiq::metadata::MetaDataFileMap,
    rules: &RuleStore,
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> (
    Vec<crate::gate_rules::autogate::Measurement>,
    Vec<crate::gate_rules::autogate::Unmeasured>,
    Vec<String>,
) {
    use polars::prelude::*;
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (files, mut problems) = files_to_read(files, names);
    let total = files.len();
    let done = AtomicUsize::new(0);

    type Measured = (
        Vec<crate::gate_rules::autogate::Measurement>,
        Vec<crate::gate_rules::autogate::Unmeasured>,
        Vec<String>,
    );

    let per_file: Vec<Measured> = files
        .par_iter()
        .map(|(path, id)| {
            let mut out: Measured = (Vec::new(), Vec::new(), Vec::new());
            if cancel.load(Ordering::Relaxed) {
                return out;
            }
            let frame = (|| -> anyhow::Result<DataFrame> {
                let fcs = flow_fcs::Fcs::open(path.to_str().unwrap_or_default())?;
                // Only the cofactors this file actually carries.
                // `apply_arcsinh_transforms` errors on the first parameter it
                // cannot find, and the cofactors describe the whole panel as
                // the scaling file defines it - so one channel absent from one
                // file failed every rule on every file, with the run reporting
                // "Parameter AF P1-A not found" six times and nothing placed.
                // The same filter the two plotting paths use.
                let carried =
                    crate::gate_editor::plots::data_helpers::cofactors_carried_by(&fcs, arcsinh);
                let params: Vec<(&str, f32)> =
                    carried.iter().map(|(k, v)| (k.as_ref(), *v)).collect();
                let scaled = (*fcs.apply_arcsinh_transforms(&params)?).clone();
                Ok(scaled.with_row_index("original_index".into(), None)?)
            })();
            match frame {
                Ok(df) => match measure_file(snapshot, id, &df, metadata, rules) {
                    Ok((m, u)) => {
                        out.0 = m;
                        out.1 = u;
                    }
                    Err(e) => out.2.push(format!("{id}: {e}")),
                },
                Err(e) => out.2.push(format!("{}: {e}", path.display())),
            }
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
            out
        })
        .collect();

    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (m, u, p) in per_file {
        measured.extend(m);
        unmeasured.extend(u);
        problems.extend(p);
    }
    (measured, unmeasured, problems)
}

/// Everything a run produces, handed back from the worker in one piece.
struct RunOutcome {
    report: Report,
    placements: Vec<crate::gate_rules::autogate::Placement>,
    cancelled: bool,
}

/// The whole solve, off the UI thread.
///
/// It works against a snapshot of the gate store and returns the placements
/// rather than writing them, so the editor stays live and usable throughout and
/// a gate moved by hand while this runs is not silently overwritten.
#[allow(clippy::too_many_arguments)]
fn run_solve(
    snapshot: GateState,
    files: Vec<(Arc<str>, PathBuf)>,
    names: HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    arcsinh: Vec<(Arc<str>, f32)>,
    metadata: crate::omiq::metadata::MetaDataFileMap,
    rules: RuleStore,
    progress: tokio::sync::mpsc::UnboundedSender<Progress>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> RunOutcome {
    use std::sync::atomic::Ordering;

    let (measured, unmeasured, mut problems) = measure_all(
        &snapshot,
        &files,
        &names,
        &arcsinh,
        &metadata,
        &rules,
        &cancel,
        |done, total| {
            let _ = progress.send(Progress::Measuring { done, total });
        },
    );

    if cancel.load(Ordering::Relaxed) {
        return RunOutcome {
            report: Report::default(),
            placements: Vec::new(),
            cancelled: true,
        };
    }

    let _ = progress.send(Progress::Solving { done: 0, total: 0 });
    let (mut report, placements) = crate::gate_rules::autogate::solve_all_reporting(
        &snapshot,
        &rules,
        &measured,
        &unmeasured,
        &metadata,
        |done, total| {
            let _ = progress.send(Progress::Solving { done, total });
        },
    );

    for problem in problems.drain(..) {
        report.skipped.push(crate::gate_rules::autogate::Skipped {
            file: Arc::from(""),
            gate: Arc::from(""),
            parent_gate: None,
            reason: problem,
        });
    }

    RunOutcome {
        report,
        placements,
        cancelled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(pairs: &[(&str, &str)]) -> HashMap<Arc<str>, Arc<str>, FxBuildHasher> {
        let mut map = HashMap::with_hasher(FxBuildHasher);
        for (file, id) in pairs {
            map.insert(Arc::from(*file), Arc::from(*id));
        }
        map
    }

    fn workspace(pairs: &[(&str, &str)]) -> Vec<(Arc<str>, PathBuf)> {
        pairs
            .iter()
            .map(|(name, path)| (Arc::from(*name), PathBuf::from(path)))
            .collect()
    }

    #[test]
    fn files_are_read_in_sorted_order() {
        // Measuring runs in parallel and the results are flattened in this
        // order, so two identical runs agree only if this order is fixed.
        let files = workspace(&[
            ("c.fcs", "/w/c.fcs"),
            ("a.fcs", "/w/a.fcs"),
            ("b.fcs", "/w/b.fcs"),
        ]);
        let names = named(&[("a.fcs", "A"), ("b.fcs", "B"), ("c.fcs", "C")]);
        let (found, problems) = files_to_read(&files, &names);

        let ids: Vec<&str> = found.iter().map(|(_, id)| id.as_ref()).collect();
        assert_eq!(ids, ["A", "B", "C"]);
        assert!(problems.is_empty());
    }

    #[test]
    fn a_file_with_no_metadata_row_is_reported_not_guessed() {
        // Matching on a stem rather than the exact name is how one donor's
        // gates came to be scored against another's population.
        let files = workspace(&[
            ("known.fcs", "/w/known.fcs"),
            ("stranger.fcs", "/w/stranger.fcs"),
        ]);
        let names = named(&[("known.fcs", "A")]);
        let (found, problems) = files_to_read(&files, &names);

        assert_eq!(found.len(), 1);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("stranger.fcs"), "{:?}", problems);
    }

    #[test]
    fn a_file_from_a_sub_folder_is_looked_up_by_its_program_name() {
        // Plate_10/A1.fcs is known as Plate_10_A1.fcs, and that is the name
        // the metadata has to carry. Looking it up by the name on disk would
        // find another plate's A1.
        let files = workspace(&[("Plate_10_A1.fcs", "/w/Plate_10/A1.fcs")]);
        let found_by_program_name = files_to_read(&files, &named(&[("Plate_10_A1.fcs", "P10")]));
        assert_eq!(found_by_program_name.0.len(), 1);
        assert_eq!(&*found_by_program_name.0[0].1, "P10");

        let by_disk_name = files_to_read(&files, &named(&[("A1.fcs", "WRONG")]));
        assert!(by_disk_name.0.is_empty(), "matched on the name on disk");
        assert_eq!(by_disk_name.1.len(), 1);
    }

    #[test]
    fn the_path_that_is_read_is_the_real_one() {
        // The name is for the metadata; opening the file needs its path.
        let files = workspace(&[("Plate_10_A1.fcs", "/w/Plate_10/A1.fcs")]);
        let (found, _) = files_to_read(&files, &named(&[("Plate_10_A1.fcs", "P10")]));
        assert_eq!(found[0].0, PathBuf::from("/w/Plate_10/A1.fcs"));
    }

    #[test]
    fn no_files_is_nothing_to_do_rather_than_a_problem() {
        let (found, problems) = files_to_read(&[], &named(&[]));
        assert!(found.is_empty());
        assert!(problems.is_empty());
    }

    // ── a whole run, from files on disk ──────────────────────────────────────
    //
    // What the Run button does: read every workspace file, measure the gates
    // the rules name, solve against each file's reference, and hand back the
    // placements. Everything below `run_solve` is tested piece by piece on
    // frames built in memory; this is the only test that starts from FCS
    // files, so it is the one that sees the reading, the metadata join and
    // the solve agree about which file is which.

    mod a_run_from_files_on_disk {
        use super::*;
        use crate::file_load_tests::{scratch, write_fcs_rows};
        use crate::gate_editor::gates::gate_store::GateSource;
        use crate::gate_editor::gates::gate_traits::DrawableGate;
        use crate::gate_rules::rule::{AboveTheNegativeRule, Rule};
        use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleTarget};
        use rand::SeedableRng;
        use rand_distr::{Distribution, Normal, Uniform};
        use std::sync::atomic::AtomicBool;

        const X: &str = "FSC-A";
        const Y: &str = "SSC-A";

        /// A negative centred at `centre` and a positive well above it.
        fn population(centre: f32, seed: u64) -> Vec<Vec<f32>> {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let neg = Normal::new(centre, 40.0).unwrap();
            let pos = Uniform::new(centre + 250.0, centre + 400.0).unwrap();
            let mut rows: Vec<Vec<f32>> = (0..9_000)
                .map(|_| vec![neg.sample(&mut rng), 100.0])
                .collect();
            rows.extend((0..1_000).map(|_| vec![pos.sample(&mut rng), 100.0]));
            rows
        }

        /// A positive gate at the root whose left edge sits at 500 - right
        /// for a negative at 300.
        fn positive_gate() -> (GateState, Arc<str>) {
            let mut state = GateState::default();
            let geometry = flow_gates::create_rectangle_geometry(
                vec![(500.0, -1e16), (1e16, -1e16), (1e16, 1e16), (500.0, 1e16)],
                X,
                Y,
            )
            .unwrap();
            let id: Arc<str> = Arc::from("CD134+");
            let gate: Arc<dyn DrawableGate> = Arc::new(
                crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate::try_new(
                    flow_gates::Gate {
                        id: id.clone(),
                        name: "CD134+".into(),
                        geometry,
                        mode: flow_gates::GateMode::Global,
                        parameters: (Arc::from(X), Arc::from(Y)),
                        label_position: None,
                    },
                    true,
                )
                .unwrap(),
            );
            state.place_gate(&[id.clone()], &gate, &GateSource::Global);
            state.place_new_gate(None, id.clone()).unwrap();
            (state, id)
        }

        fn specimens() -> crate::omiq::metadata::MetaDataFileMap {
            let mut map = im::HashMap::with_hasher(FxBuildHasher);
            for (file, id) in [("fs_qc", "QC-A"), ("fs_b", "DONOR-B")] {
                let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
                columns.insert(Arc::from("SampleID"), Arc::from(id));
                columns.insert(Arc::from("SampleType"), Arc::from("FS"));
                map.insert(Arc::from(file) as Arc<str>, columns);
            }
            map
        }

        fn rules() -> RuleStore {
            let mut store = RuleStore::default();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule::default()),
                },
            );
            store
        }

        /// The QC's negative at 300; the donor's has drifted to 600.
        fn workspace(name: &str) -> Vec<(Arc<str>, PathBuf)> {
            let dir = scratch(name);
            let channels = [(X, None), (Y, None)];
            write_fcs_rows(
                &dir.join("fs_qc.fcs"),
                &channels,
                &population(300.0, 1),
                &[],
            );
            write_fcs_rows(&dir.join("fs_b.fcs"), &channels, &population(600.0, 2), &[]);
            vec![
                (Arc::from("fs_qc.fcs"), dir.join("fs_qc.fcs")),
                (Arc::from("fs_b.fcs"), dir.join("fs_b.fcs")),
            ]
        }

        fn run(state: &GateState, files: Vec<(Arc<str>, PathBuf)>, cancel: bool) -> RunOutcome {
            let (progress, _) = tokio::sync::mpsc::unbounded_channel();
            run_solve(
                state.clone(),
                files,
                named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                Vec::new(),
                specimens(),
                rules(),
                progress,
                Arc::new(AtomicBool::new(cancel)),
            )
        }

        fn left_edge(state: &GateState, gate: &Arc<str>, file: &str) -> f32 {
            let g = state
                .gate_for_file(gate, &Arc::from(file), &specimens())
                .unwrap();
            crate::gate_rules::autogate::extent_on(&g.get_gate_ref(None).unwrap().geometry, X)
                .unwrap()
                .0
        }

        /// BUG (docs/test-audit.md, B-AUTO-1): the default finder,
        /// `BelowTheGate`, refines from where the gate already sits on the
        /// sample - the reference's position. The donor's negative has
        /// drifted from 300 to 600, past that position, so the gate sees only
        /// the bottom of it, reads it low, and settles at 584: inside the
        /// negative, admitting 69% of the sample where the reference admits
        /// 10%. And it is scored 0.87, so a run ranks it among the
        /// placements least in need of review.
        #[test]
        #[ignore = "known bug B-AUTO-1: a negative that drifts past the gate puts it inside the negative, confidently"]
        fn the_gate_follows_the_donor_s_negative_and_leaves_the_qc_alone() {
            let (mut state, id) = positive_gate();
            let outcome = run(&state, workspace("run-follow"), false);
            assert!(!outcome.cancelled);
            assert!(
                outcome.report.skipped.iter().all(|s| &*s.file != "fs_b"),
                "{:?}",
                outcome
                    .report
                    .skipped
                    .iter()
                    .map(|s| &s.reason)
                    .collect::<Vec<_>>()
            );

            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
            // The negative moved 300 up, so the gate should too - to within
            // what reading a noisy negative allows.
            let moved = left_edge(&state, &id, "fs_b");
            assert!(
                (moved - 800.0).abs() < 20.0,
                "the donor's gate is at {moved}"
            );
            assert!((left_edge(&state, &id, "fs_qc") - 500.0).abs() < 20.0);
        }

        /// The same run with the density finder, which ignores where the
        /// gate starts: it follows the drift.
        #[test]
        fn the_density_finder_follows_a_negative_that_drifted_past_the_gate() {
            use crate::gate_rules::rule::NegativeFinder;
            let (mut state, id) = positive_gate();
            let mut store = rules();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule {
                        find: NegativeFinder::NegativePeak,
                        ..AboveTheNegativeRule::default()
                    }),
                },
            );
            let (progress, _) = tokio::sync::mpsc::unbounded_channel();
            let outcome = run_solve(
                state.clone(),
                workspace("run-density"),
                named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                Vec::new(),
                specimens(),
                store,
                progress,
                Arc::new(AtomicBool::new(false)),
            );
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
            let moved = left_edge(&state, &id, "fs_b");
            assert!(
                (moved - 800.0).abs() < 30.0,
                "the donor's gate is at {moved}"
            );
        }

        #[test]
        fn an_unreadable_file_is_reported_and_the_rest_still_run() {
            let (state, _) = positive_gate();
            let mut files = workspace("run-bad");
            let bad = files[1].1.with_file_name("broken.fcs");
            std::fs::write(&bad, b"not an fcs file").unwrap();
            files[1].1 = bad;

            let outcome = run(&state, files, false);
            let said: Vec<&str> = outcome.report.skipped.iter().map(|s| &*s.reason).collect();
            assert!(
                said.iter().any(|r| r.contains("broken.fcs")),
                "the broken file should be reported by name: {said:?}"
            );
            assert!(
                outcome.report.positioned.iter().all(|p| &*p.file != "fs_b"),
                "nothing is placed for a file that could not be read"
            );
        }

        /// A marker on arcsinh: the events are transformed with the axis
        /// store's cofactor before they are measured, so the gate - drawn and
        /// stored in that transformed space - is positioned in it too.
        #[test]
        fn a_run_measures_in_the_arcsinh_space_the_gate_is_drawn_in() {
            use crate::gate_rules::rule::NegativeFinder;
            use flow_fcs::Transformable;
            const M: &str = "BV421-A";
            let t = flow_fcs::TransformType::Arcsinh { cofactor: 150.0 };
            let shown = |raw: f32| t.transform(&raw);

            // Raw negatives at 1,000 on the QC and 3,000 on the donor; the
            // same fraction of positives far above both.
            let rows = |centre: f32, seed: u64| -> Vec<Vec<f32>> {
                let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
                let neg = Normal::new(centre, centre * 0.1).unwrap();
                let pos = Uniform::new(60_000.0f32, 90_000.0).unwrap();
                let mut out: Vec<Vec<f32>> = (0..9_000)
                    .map(|_| vec![neg.sample(&mut rng), 100.0])
                    .collect();
                out.extend((0..1_000).map(|_| vec![pos.sample(&mut rng), 100.0]));
                out
            };
            let dir = scratch("run-arcsinh");
            let channels = [(M, Some("CD3")), (Y, None)];
            write_fcs_rows(&dir.join("fs_qc.fcs"), &channels, &rows(1_000.0, 3), &[]);
            write_fcs_rows(&dir.join("fs_b.fcs"), &channels, &rows(3_000.0, 4), &[]);
            let files = vec![
                (Arc::from("fs_qc.fcs"), dir.join("fs_qc.fcs")),
                (Arc::from("fs_b.fcs"), dir.join("fs_b.fcs")),
            ];

            // The gate's edge a little above the QC's negative, in display
            // units - where a person would have drawn it on that plot.
            let edge = shown(2_500.0);
            let mut state = GateState::default();
            let id: Arc<str> = Arc::from("CD3+");
            let gate: Arc<dyn DrawableGate> = Arc::new(
                crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate::try_new(
                    flow_gates::Gate {
                        id: id.clone(),
                        name: "CD3+".into(),
                        geometry: flow_gates::create_rectangle_geometry(
                            vec![(edge, -1e16), (1e16, -1e16), (1e16, 1e16), (edge, 1e16)],
                            M,
                            Y,
                        )
                        .unwrap(),
                        mode: flow_gates::GateMode::Global,
                        parameters: (Arc::from(M), Arc::from(Y)),
                        label_position: None,
                    },
                    true,
                )
                .unwrap(),
            );
            state.place_gate(&[id.clone()], &gate, &GateSource::Global);
            state.place_new_gate(None, id.clone()).unwrap();

            let mut store = RuleStore::default();
            store.insert(
                RuleTarget::named("CD3+"),
                GateRule {
                    parameter: Arc::from(M),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule {
                        find: NegativeFinder::NegativePeak,
                        ..AboveTheNegativeRule::default()
                    }),
                },
            );
            let (progress, _) = tokio::sync::mpsc::unbounded_channel();
            let outcome = run_solve(
                state.clone(),
                files,
                named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                vec![(Arc::from(M), 150.0)],
                specimens(),
                store,
                progress,
                Arc::new(AtomicBool::new(false)),
            );
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);

            let placed = state
                .gate_for_file(&id, &Arc::from("fs_b"), &specimens())
                .unwrap();
            let at = crate::gate_rules::autogate::extent_on(
                &placed.get_gate_ref(None).unwrap().geometry,
                M,
            )
            .unwrap()
            .0;
            // In display space the donor's negative sits shown(3000) -
            // shown(1000) higher, so the gate should have moved by about as
            // much, and still be a display coordinate - single digits, not
            // thousands.
            let expected = edge + (shown(3_000.0) - shown(1_000.0));
            assert!(at < 10.0, "the gate was placed in raw units: {at}");
            assert!(
                (at - expected).abs() < 0.3,
                "the gate is at {at}, expected about {expected}"
            );
        }

        /// BUG (docs/test-audit.md, B-FCS-2): a file whose header's data
        /// offset is damaged passes the workspace's check, and reading its
        /// events trips an assertion inside flow_fcs. Files are read in
        /// parallel, so that panic takes the whole run with it - the donor's
        /// gate is never placed because of a different file.
        #[test]
        #[ignore = "known bug B-FCS-2: one file with a damaged data offset ends the whole run"]
        fn a_file_with_a_damaged_data_offset_is_reported_and_the_rest_still_run() {
            let (state, _) = positive_gate();
            let mut files = workspace("run-damaged");
            let damaged = files[0].1.with_file_name("damaged.fcs");
            crate::file_load_tests::with_data_start_damaged(&damaged);
            files.push((Arc::from("damaged.fcs"), damaged));

            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (progress, _) = tokio::sync::mpsc::unbounded_channel();
                let mut names = named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]);
                names.insert(Arc::from("damaged.fcs"), Arc::from("damaged"));
                run_solve(
                    state.clone(),
                    files,
                    names,
                    Vec::new(),
                    specimens(),
                    rules(),
                    progress,
                    Arc::new(AtomicBool::new(false)),
                )
            }));
            let outcome = outcome.expect("the run finished");
            assert!(
                outcome.report.positioned.iter().any(|p| &*p.file == "fs_b"),
                "the donor was still placed"
            );
        }

        #[test]
        fn a_cancelled_run_places_nothing() {
            let (state, _) = positive_gate();
            let outcome = run(&state, workspace("run-cancel"), true);
            assert!(outcome.cancelled);
            assert!(outcome.placements.is_empty());
        }
    }
}
