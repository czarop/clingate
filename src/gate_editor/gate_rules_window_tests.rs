//! Tests for what the Gate Rules tab offers.

#![cfg(test)]

use crate::gate_editor::gate_rules_window::choices;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{GateStateImplExt, ROOTGATE};
use crate::gate_editor::gates::gate_types::PrimaryGateType;
use crate::gate_editor::plots::axis_store::PlotMapper;
use flow_fcs::TransformType;
use std::sync::Arc;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

fn mapper() -> PlotMapper {
    PlotMapper::new(
        600.0,
        600.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    )
}

fn add(
    state: &mut GateState,
    name: &str,
    kind: PrimaryGateType,
    parent: Option<Arc<str>>,
) -> Arc<str> {
    let before: Vec<Arc<str>> = state
        .placements()
        .map(|(n, _)| n.as_arc().clone())
        .collect();
    state
        .add_gate(
            &mapper(),
            300.0,
            300.0,
            Arc::from(X),
            Arc::from(Y),
            // Polygons need their points; the other kinds derive theirs.
            Some(vec![(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)]),
            parent,
            kind,
            Some(name.to_string()),
        )
        .expect("a gate can be added");
    state
        .placements()
        .map(|(n, p)| (n.as_arc().clone(), p.gate_id.clone()))
        .find(|(n, _)| !before.contains(n))
        .map(|(_, id)| id)
        .expect("the new placement")
}

fn everything_offered(state: &GateState) -> Vec<String> {
    let c = choices(state);
    c.children
        .iter()
        .flat_map(|(_, kids)| kids.iter())
        .map(|k| k.to_string())
        .collect()
}

#[test]
fn a_quadrants_container_is_not_offered_but_its_corners_are() {
    // The container carries the composite group's id as its name, so it shows
    // up in a list as something like `Mzk4` - not a gate anyone drew, and not
    // one a rule positioning a single line could act on.
    let mut state = GateState::default();
    let parent = add(
        &mut state,
        "CD4+CD8-",
        PrimaryGateType::Polygon,
        Some(ROOTGATE.clone()),
    );
    let quad = add(
        &mut state,
        "Q",
        PrimaryGateType::Quadrant,
        Some(parent.clone()),
    );

    let composite = state.registered_gate(&quad).expect("the quadrant");
    assert!(
        composite.is_composite(),
        "the fixture should be a composite"
    );

    let offered = everything_offered(&state);
    assert!(
        !offered.contains(&composite.get_name().to_string()),
        "the composite container should not be offered, got {offered:?}"
    );
}

#[test]
fn an_ordinary_gate_is_still_offered() {
    // The exclusion has to be narrow: a plain gate under a parent stays.
    let mut state = GateState::default();
    let parent = add(
        &mut state,
        "CD4+",
        PrimaryGateType::Polygon,
        Some(ROOTGATE.clone()),
    );
    let _ = add(
        &mut state,
        "Ki67+",
        PrimaryGateType::Rectangle,
        Some(parent),
    );

    assert!(everything_offered(&state).contains(&"Ki67+".to_string()));
}

#[test]
fn a_gate_at_the_root_is_not_offered() {
    // Nothing to be a fraction of.
    let mut state = GateState::default();
    let _ = add(
        &mut state,
        "Cells",
        PrimaryGateType::Polygon,
        Some(ROOTGATE.clone()),
    );
    assert!(everything_offered(&state).is_empty());
}

// ── carrying a rule's gate across a change of population ──────────────────
//
// The Edit button exists so one rule can be moved onto a second population
// without retyping it, which only pays off if the gate survives the move.

use crate::gate_editor::gate_rules_window::{GateChoices, carry_over};

fn two_populations() -> GateChoices {
    GateChoices {
        parents: vec![Arc::from("CD4+"), Arc::from("CD8+")],
        children: vec![
            (
                Arc::from("CD4+"),
                vec![Arc::from("Ki67+"), Arc::from("TIGIT+")],
            ),
            (Arc::from("CD8+"), vec![Arc::from("Ki67+")]),
        ],
        parameters: vec![
            (
                Arc::from("Ki67+"),
                vec![Arc::from("Ki-67"), Arc::from("CD3")],
            ),
            (Arc::from("TIGIT+"), vec![Arc::from("TIGIT")]),
        ],
    }
}

#[test]
fn a_gate_the_new_population_also_holds_is_kept() {
    let (gate, parameter) = carry_over(&two_populations(), "CD8+", "Ki67+", "Ki-67");
    assert_eq!(gate, "Ki67+");
    assert_eq!(parameter, "Ki-67");
}

#[test]
fn a_gate_the_new_population_does_not_hold_is_cleared() {
    // TIGIT+ is drawn on CD4+ only. Carrying the name over would let the form
    // name a population that does not exist.
    let (gate, parameter) = carry_over(&two_populations(), "CD8+", "TIGIT+", "TIGIT");
    assert_eq!(gate, "");
    assert_eq!(parameter, "");
}

#[test]
fn a_parameter_that_gate_is_not_drawn_on_is_cleared_but_the_gate_stays() {
    // The gate survives the move; the parameter did not come with it, so the
    // form asks for that one field rather than both.
    let (gate, parameter) = carry_over(&two_populations(), "CD8+", "Ki67+", "TIGIT");
    assert_eq!(gate, "Ki67+");
    assert_eq!(parameter, "");
}

#[test]
fn clearing_the_population_clears_the_gate() {
    let (gate, parameter) = carry_over(&two_populations(), "", "Ki67+", "Ki-67");
    assert_eq!(gate, "");
    assert_eq!(parameter, "");
}

// ── how a phenotype rule reads back ──────────────────────────────────────

use crate::gate_editor::plots::axis_store::Param;

fn param(marker: &str, fluoro: &str) -> Param {
    Param {
        marker: Arc::from(marker),
        fluoro: Arc::from(fluoro),
    }
}

#[test]
fn a_phenotype_rule_names_its_markers_as_they_were_ticked() {
    use crate::gate_editor::gate_rules_window::describe_phenotype;
    use crate::gate_rules::rule::{PhenotypeRule, ShapeFit};
    // The rule stores the column it reads, because that is what a DataFrame is
    // indexed by. Nobody ticks a column called BV421-A.
    let panel = vec![
        param("CD279", "BV421-A"),
        param("Va7_2", "BV711-A"),
        param("CD161", "BB700-A"),
    ];
    let rule = PhenotypeRule {
        markers: vec![Arc::from("BV421-A"), Arc::from("BB700-A")],
        fit: ShapeFit::DrawPolygon,
        ..Default::default()
    };
    let described = describe_phenotype(&rule, &panel);
    assert!(described.contains("CD279"), "got: {described}");
    assert!(described.contains("CD161"), "got: {described}");
    assert!(
        !described.contains("BV421-A"),
        "the column leaked: {described}"
    );
}

#[test]
fn a_marker_the_panel_does_not_carry_is_shown_as_the_rule_stores_it() {
    use crate::gate_editor::gate_rules_window::marker_label;
    // The honest answer: that is the column the rule will look for, and saying
    // so is how a person finds out the panel has changed under them.
    assert_eq!(marker_label("PE-A", &[param("CD279", "BV421-A")]), "PE-A");
}

#[test]
fn a_phenotype_rule_with_nothing_ticked_says_it_uses_every_marker() {
    use crate::gate_editor::gate_rules_window::describe_phenotype;
    use crate::gate_rules::rule::PhenotypeRule;
    let described = describe_phenotype(&PhenotypeRule::default(), &[]);
    assert!(described.contains("every marker"), "got: {described}");
}
