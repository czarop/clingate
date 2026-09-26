//! Tests for naming a population unambiguously.

#![cfg(test)]

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_paths::{SEPARATOR, unique_names};
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

/// Add a gate named `name` under `parent`, returning its gate id.
fn add(state: &mut GateState, name: &str, parent: Option<Arc<str>>) -> Arc<str> {
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
            None,
            parent,
            PrimaryGateType::Rectangle,
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

/// The name given to the node holding the gate called `name`, where only one
/// such node exists.
fn named(state: &GateState, gate_id: &Arc<str>) -> String {
    let names = unique_names(state);
    state
        .placements()
        .find(|(_, p)| &p.gate_id == gate_id)
        .and_then(|(node, _)| names.get(node).cloned())
        .map(|n| n.to_string())
        .expect("every placement is named")
}

#[test]
fn a_name_nothing_else_shares_stays_short() {
    let mut state = GateState::default();
    let cells = add(&mut state, "Cells", Some(ROOTGATE.clone()));
    let _ = add(&mut state, "CD3+", Some(cells));

    let names = unique_names(&state);
    let all: Vec<String> = names.values().map(|n| n.to_string()).collect();
    assert!(all.contains(&"Cells".to_string()));
    assert!(
        all.contains(&"CD3+".to_string()),
        "an unambiguous name needs no path, got {all:?}"
    );
}

#[test]
fn a_shared_name_grows_until_it_is_its_own() {
    // The real case: the same marker pair gated under two different parents.
    let mut state = GateState::default();
    let cells = add(&mut state, "Cells", Some(ROOTGATE.clone()));
    let conventional = add(&mut state, "CD3+CD14-", Some(cells.clone()));
    let mait = add(&mut state, "MAIT", Some(cells));

    let under_conventional = add(&mut state, "CD4+CD8-", Some(conventional));
    let under_mait = add(&mut state, "CD4+CD8-", Some(mait));

    let a = named(&state, &under_conventional);
    let b = named(&state, &under_mait);

    assert_ne!(a, b, "two populations must not share a name");
    assert_eq!(a, format!("CD3+CD14-{SEPARATOR}CD4+CD8-"));
    assert_eq!(b, format!("MAIT{SEPARATOR}CD4+CD8-"));
}

#[test]
fn only_the_ambiguous_names_grow() {
    // Disambiguating one population must not lengthen the rest.
    let mut state = GateState::default();
    let cells = add(&mut state, "Cells", Some(ROOTGATE.clone()));
    let a = add(&mut state, "A", Some(cells.clone()));
    let b = add(&mut state, "B", Some(cells.clone()));
    let unique = add(&mut state, "Singlets", Some(cells));
    let _ = add(&mut state, "Shared", Some(a));
    let _ = add(&mut state, "Shared", Some(b));

    assert_eq!(named(&state, &unique), "Singlets");
}

#[test]
fn a_name_repeated_three_times_still_resolves() {
    let mut state = GateState::default();
    let cells = add(&mut state, "Cells", Some(ROOTGATE.clone()));
    let mut leaves = Vec::new();
    for parent in ["P1", "P2", "P3"] {
        let p = add(&mut state, parent, Some(cells.clone()));
        leaves.push(add(&mut state, "Ki67+", Some(p)));
    }
    let names: Vec<String> = leaves.iter().map(|g| named(&state, g)).collect();

    let unique: std::collections::BTreeSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), 3, "all three must differ, got {names:?}");
}

#[test]
fn a_linked_gate_at_two_points_is_two_populations() {
    // One gate, two places in the tree - and each place sees a different
    // population, so each needs its own name even though the gate is shared.
    let mut state = GateState::default();
    let cells = add(&mut state, "Cells", Some(ROOTGATE.clone()));
    let cd4 = add(&mut state, "CD4+", Some(cells.clone()));
    let cd8 = add(&mut state, "CD8+", Some(cells));
    let linked = add(&mut state, "CD218a+", Some(cd4));
    // A placeholder under CD8+, then linked to the CD4+ one: that is how the
    // sidebar applies one gate at a second point in the tree.
    let spare = add(&mut state, "spare", Some(cd8));
    let (spare_node, linked_node) = {
        let node_of = |gate: &Arc<str>| {
            state
                .placements()
                .find(|(_, p)| &p.gate_id == gate)
                .map(|(n, _)| n.clone())
                .expect("placement")
        };
        (node_of(&spare), node_of(&linked))
    };
    state
        .link_node_to_gate(&spare_node, &linked_node)
        .expect("a gate can be applied at a second point");

    let names = unique_names(&state);
    let for_linked: Vec<String> = state
        .placements()
        .filter(|(_, p)| p.gate_id == linked)
        .filter_map(|(node, _)| names.get(node).map(|n| n.to_string()))
        .collect();

    assert_eq!(for_linked.len(), 2, "both placements are named");
    assert_ne!(
        for_linked[0], for_linked[1],
        "the same gate in two places is two populations: {for_linked:?}"
    );
}
