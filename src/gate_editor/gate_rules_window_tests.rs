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
