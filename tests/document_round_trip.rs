//! A gating document written by the program and read back by it.
//!
//! The import (`omiq::deserialise`), the three-tier gate store
//! (`gate_editor::gates::gate_store`), the metadata grouping and the export
//! (`omiq::serialise`) each have their own tests. What none of them can show
//! alone is the loop a person actually runs: gates adjusted per sample in
//! this session, the file saved, and the file opened again. A position that
//! comes back on the wrong sample - or on every sample - is the failure that
//! matters, and it only exists across the whole loop.

mod common;

use clingate::gate_editor::gates::GateState;
use clingate::gate_editor::gates::gate_store::{GateId, GateSource};
use clingate::gate_editor::gates::gate_traits::DrawableGate;
use clingate::gate_rules::autogate::{extent_on, place_for_specimen, translate_edge_to};
use clingate::gate_rules::rule_store::Bound;
use clingate::omiq::metadata::MetaDataKey;
use clingate::omiq::serialise::to_omiq_document;
use common::*;
use std::sync::Arc;

const FIXTURE: &str = "quadrant_with_boolean_child.omiqgt";

fn import(path: &std::path::Path) -> GateState {
    let mut state = GateState::default();
    state
        .upload_gates_from_file(path.to_path_buf(), &fixture_metadata(), fixture_axes())
        .unwrap_or_else(|e| panic!("{} imports: {e}", path.display()));
    state
}

/// Save the state and open what was saved.
fn saved_and_reopened(state: &GateState, name: &str) -> GateState {
    let written = to_omiq_document(state, &fixture_metadata(), &fixture_axes())
        .expect("an imported document can be written");
    let path = scratch(name).join("saved.omiqgt");
    std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();
    import(&path)
}

/// A gate with a finite lower edge on its x parameter, to move.
fn movable(state: &GateState) -> (GateId, Arc<dyn DrawableGate>, Arc<str>, f32) {
    state
        .registered_ids()
        .into_iter()
        .find_map(|id| {
            let gate = state.registered_gate(&id)?;
            if gate.is_composite() {
                return None;
            }
            let (x, _) = gate.get_params();
            let (low, _) = extent_on(&gate.get_gate_ref(None)?.geometry, &x)?;
            (low.is_finite() && low.abs() < 1e9).then_some((id, gate, x, low))
        })
        .expect("the fixture holds a gate with a finite x edge")
}

fn low_edge_for(state: &GateState, gate: &GateId, file: &str, x: &str) -> f32 {
    let resolved = state
        .gate_for_file(gate, &Arc::from(file), &fixture_metadata())
        .unwrap_or_else(|| panic!("{gate} resolves for {file}"));
    extent_on(&resolved.get_gate_ref(None).unwrap().geometry, x)
        .unwrap()
        .0
}

/// Equal to f32 precision. The moves below are a tenth of the edge's value,
/// so a position that did not move cannot pass for one that did - the edge
/// the fixture offers is on FSC-A, around a million, where a fixed nudge of
/// half a unit would vanish inside any tolerance a float needs.
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(1.0)
}

/// A move big enough to see: a tenth of where the edge is.
fn nudge(from: f32) -> f32 {
    (from.abs() * 0.1).max(1.0)
}

#[test]
fn a_position_set_for_one_sample_comes_back_on_that_sample_only() {
    let mut state = import(&fixture(FIXTURE));
    let (id, gate, x, from) = movable(&state);
    let to = from + nudge(from);
    let moved = translate_edge_to(&gate, &x, Bound::Above, to as f64).unwrap();
    state.place_gate(
        &[id.clone()],
        &moved,
        &GateSource::Sample((id.clone(), Arc::from("sample1"))),
    );

    let back = saved_and_reopened(&state, "persample");
    let (one, two) = (
        low_edge_for(&back, &id, "sample1", &x),
        low_edge_for(&back, &id, "sample2", &x),
    );
    assert!(close(one, to), "sample1 came back at {one}, set to {to}");
    assert!(
        close(two, from),
        "sample2 came back at {two}, left at {from}"
    );
}

#[test]
fn a_position_set_for_a_specimen_comes_back_on_its_samples_only() {
    // What the autogater writes: one position per metadata group.
    let mut state = import(&fixture(FIXTURE));
    let (id, gate, x, from) = movable(&state);
    let to = from + nudge(from);
    let moved = translate_edge_to(&gate, &x, Bound::Above, to as f64).unwrap();
    place_for_specimen(
        &mut state,
        &id,
        &MetaDataKey {
            parameter: Arc::from("test"),
            group: Arc::from("two"),
        },
        &moved,
    );

    let back = saved_and_reopened(&state, "perspecimen");
    assert!(close(low_edge_for(&back, &id, "sample2", &x), to));
    assert!(close(low_edge_for(&back, &id, "sample1", &x), from));
}

#[test]
fn saving_twice_changes_nothing_the_second_time() {
    // A document that drifts each time it is saved - rounding, a sentinel
    // re-read as a coordinate, a group re-keyed - is corrupted a little more
    // by every save.
    let mut state = import(&fixture(FIXTURE));
    let (id, gate, x, from) = movable(&state);
    let moved = translate_edge_to(&gate, &x, Bound::Above, (from + nudge(from)) as f64).unwrap();
    state.place_gate(
        &[id.clone()],
        &moved,
        &GateSource::Sample((id, Arc::from("sample1"))),
    );

    let once = saved_and_reopened(&state, "twice-a");
    let twice = saved_and_reopened(&once, "twice-b");
    let write = |s: &GateState| to_omiq_document(s, &fixture_metadata(), &fixture_axes()).unwrap();
    assert_eq!(write(&once)["tree"], write(&twice)["tree"]);
}

#[test]
fn every_gate_and_position_survives_a_save() {
    let state = import(&fixture(FIXTURE));
    let back = saved_and_reopened(&state, "everything");

    let mut before = state.registered_ids();
    let mut after = back.registered_ids();
    before.sort();
    after.sort();
    assert_eq!(before, after, "the same gates");
    for id in &before {
        assert_eq!(
            state.placement_count(id),
            back.placement_count(id),
            "{id} is on the same number of plots"
        );
        assert_eq!(
            state.hierarchy_parent(id),
            back.hierarchy_parent(id),
            "{id} keeps its parent"
        );
    }
}
