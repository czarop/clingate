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
    // A corner is registered under its quadrant, so it is told from the
    // container by whether the quadrant has a shape under that id.
    let container = gate.is_composite() && gate.get_gate_ref(Some(gate_id.as_ref())).is_none();
    !state.is_ghost(gate_id) && !container && gate.as_any().downcast_ref::<BooleanGate>().is_none()
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
        let name: Arc<str> = state
            .population_name(&placement.gate_id)
            .unwrap_or_else(|| Arc::from(gate.get_name()));

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
        "find the cells that match on {named}, then {}{}",
        rule.fit.label(),
        crate::gate_rules::rule::pinned_said(&rule.pinned, |m| marker_label(m, panel))
    )
}

// ── the form for a rule from another gate ─────────────────────────────────

/// Every gate a rule can name, as it names them: the gate and its parent.
/// What a rule from another gate picks its anchor from.
pub fn every_target(choices: &GateChoices) -> Vec<crate::gate_rules::rule_store::RuleTarget> {
    use crate::gate_rules::rule_store::RuleTarget;
    let mut out: Vec<RuleTarget> = choices
        .children
        .iter()
        .flat_map(|(parent, children)| {
            children
                .iter()
                .map(move |child| RuleTarget::under(child.clone(), parent.clone()))
        })
        .collect();
    out.sort_by_key(|t| t.describe());
    out
}

/// The gates `gate` under `parent` can be placed next to: beside it under
/// the same parent, drawn on the same two parameters.
pub fn beside(
    choices: &GateChoices,
    gate: &str,
    parent: &str,
) -> Vec<crate::gate_rules::rule_store::RuleTarget> {
    let mut own: Vec<&Arc<str>> = choices.parameters_of(gate).iter().collect();
    own.sort();
    choices
        .children_of(parent)
        .iter()
        .filter(|other| ***other != *gate)
        .filter(|other| {
            let mut theirs: Vec<&Arc<str>> = choices.parameters_of(other).iter().collect();
            theirs.sort();
            theirs == own
        })
        .map(|other| crate::gate_rules::rule_store::RuleTarget::under(other.clone(), parent))
        .collect()
}

/// The gates a valley-or-smear rule for `gate` under `parent` can fall back to: the
/// same gate under every other parent.
pub fn fallback_targets(
    choices: &GateChoices,
    gate: &str,
    parent: &str,
) -> Vec<crate::gate_rules::rule_store::RuleTarget> {
    every_target(choices)
        .into_iter()
        .filter(|t| *t.gate == *gate && t.parent.as_deref() != Some(parent))
        .collect()
}

/// One edge as the form holds it: text, until it is saved.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct EdgeForm {
    /// The anchor, as [`RuleTarget::describe`](crate::gate_rules::rule_store::RuleTarget::describe) writes it.
    pub anchor: String,
    pub parameter: String,
    /// "Lower" or "Upper".
    pub side: String,
    pub anchor_side: String,
    /// A number, or empty for 0.
    pub gap: String,
}

/// An edge as the form writes it, "Lower" or "Upper".
pub fn side_from(text: &str) -> Result<crate::gate_rules::rule::Side, String> {
    use crate::gate_rules::rule::Side;
    match text {
        "Lower" => Ok(Side::Lower),
        "Upper" => Ok(Side::Upper),
        other => Err(format!(
            "\"{other}\" is not an edge - choose lower or upper"
        )),
    }
}

/// An edge as the form writes it.
pub fn side_to(side: crate::gate_rules::rule::Side) -> String {
    match side {
        crate::gate_rules::rule::Side::Lower => "Lower".into(),
        crate::gate_rules::rule::Side::Upper => "Upper".into(),
    }
}

/// The rule the form describes: the whole shape of `same_shape_as` when it
/// names a gate, otherwise the edges.
pub fn follow_from_form(
    same_shape_as: Option<&str>,
    edges: &[EdgeForm],
    targets: &[crate::gate_rules::rule_store::RuleTarget],
) -> Result<crate::gate_rules::rule::FromGateRule, String> {
    use crate::gate_rules::rule::{EdgeFrom, FromGateRule};
    let target = |named: &str| {
        targets
            .iter()
            .find(|t| t.describe() == named)
            .cloned()
            .ok_or_else(|| {
                if named.is_empty() {
                    "Choose the gate it follows".to_string()
                } else {
                    format!("There is no gate {named} to follow")
                }
            })
    };
    if let Some(named) = same_shape_as {
        return Ok(FromGateRule {
            same_shape_as: Some(target(named)?),
            edges: Vec::new(),
        });
    }
    if edges.is_empty() {
        return Err("Add an edge to set".to_string());
    }
    let edges = edges
        .iter()
        .map(|e| {
            if e.parameter.is_empty() {
                return Err("Choose the parameter each edge is on".to_string());
            }
            let gap = match e.gap.trim() {
                "" => 0.0,
                text => text
                    .parse::<f64>()
                    .map_err(|_| format!("The gap \"{text}\" is not a number"))?,
            };
            Ok(EdgeFrom {
                anchor: target(&e.anchor)?,
                parameter: Arc::from(e.parameter.as_str()),
                side: side_from(&e.side)?,
                anchor_side: side_from(&e.anchor_side)?,
                gap,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(FromGateRule {
        same_shape_as: None,
        edges,
    })
}

/// A saved rule back into the form, for Edit.
pub fn follow_to_form(
    rule: &crate::gate_rules::rule::FromGateRule,
) -> (Option<String>, Vec<EdgeForm>) {
    (
        rule.same_shape_as.as_ref().map(|t| t.describe()),
        rule.edges
            .iter()
            .map(|e| EdgeForm {
                anchor: e.anchor.describe(),
                parameter: e.parameter.to_string(),
                side: side_to(e.side),
                anchor_side: side_to(e.anchor_side),
                gap: if e.gap == 0.0 {
                    String::new()
                } else {
                    format!("{}", e.gap)
                },
            })
            .collect(),
    )
}
