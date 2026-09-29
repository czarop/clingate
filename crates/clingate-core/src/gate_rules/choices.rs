//! What a rule can be written against: the populations, the gates drawn on
//! them, and the parameters they are drawn on - read from the gating document,
//! so a rule can only name something the document has.

use std::sync::Arc;

use crate::axis_store::Param;
use crate::gate_rules::rule::PhenotypeRule;
use crate::gates::GateState;
use crate::gates::gate_single::boolean_gates::BooleanGate;
use crate::gates::gate_store::GateId;
use crate::gates::gate_traits::DrawableGate;

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
    let names = crate::gates::gate_paths::unique_names(state);
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
pub fn marker_label(column: &str, panel: &[Param]) -> String {
    panel
        .iter()
        .find(|param| &*param.fluoro == column)
        .map(|param| param.to_string())
        .unwrap_or_else(|| column.to_string())
}

/// The panel's channels, one each, for the phenotype rule's marker picker.
///
/// A rule stores a marker by its channel, so two entries for one channel
/// would be two boxes ticking the same thing.
pub fn marker_panel(panel: &[Param]) -> Vec<Param> {
    let mut seen: Vec<&str> = Vec::new();
    panel
        .iter()
        .filter(|param| {
            let fresh = !seen.contains(&&*param.fluoro);
            seen.push(&param.fluoro);
            fresh
        })
        .cloned()
        .collect()
}

/// The markers a new phenotype rule starts with ticked: the two its gate is
/// drawn on, as the channels the rule stores.
///
/// A population is nearly always described by at least the two markers it is
/// plotted on, and nothing adds them behind the scenes - only what is ticked
/// is read - so they start ticked, in the one list, rather than being implied.
/// A gate parameter is matched by channel or by marker name, whichever the
/// gating document used.
pub fn plot_markers(gate_parameters: &[Arc<str>], panel: &[Param]) -> Vec<String> {
    let mut chosen: Vec<String> = Vec::new();
    for parameter in gate_parameters {
        let column = panel
            .iter()
            .find(|p| p.fluoro == *parameter)
            .or_else(|| panel.iter().find(|p| p.marker == *parameter))
            .map(|p| p.fluoro.to_string())
            .unwrap_or_else(|| parameter.to_string());
        if !chosen.contains(&column) {
            chosen.push(column);
        }
    }
    chosen
}

pub fn describe_phenotype(rule: &PhenotypeRule, panel: &[Param]) -> String {
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
