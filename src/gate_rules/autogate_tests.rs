//! Tests for writing a solved threshold onto a gate.
//!
//! The invariant that matters most is that the gate *moves*. A rule yields one
//! number for one edge, and the obvious implementation - set that edge, leave
//! the rest - silently resizes the gate, which is not what the hand gating
//! does and not what the person drew.

#![cfg(test)]

use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_rules::autogate::{ApplyError, specimen_of, translate_edge_to};
use crate::gate_rules::rule_store::{Bound, SamplePairing};
use flow_gates::{GateGeometry, create_polygon_geometry, create_rectangle_geometry};
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::sync::Arc;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

fn gate(id: &str, geometry: GateGeometry) -> flow_gates::Gate {
    flow_gates::Gate {
        id: Arc::from(id),
        name: format!("{id} name"),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    }
}

/// A rectangle from its corners, as a `DrawableGate`.
fn rect(x1: f32, y1: f32, x2: f32, y2: f32) -> Arc<dyn DrawableGate> {
    let geometry =
        create_rectangle_geometry(vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)], X, Y).unwrap();
    Arc::new(RectangleGate::try_new(gate("r", geometry), true).unwrap())
}

/// The two edges bounding `param`, read back out of a gate.
fn edges(gate: &Arc<dyn DrawableGate>, param: &str) -> (f32, f32) {
    let inner = gate.get_gate_ref(None).unwrap();
    let GateGeometry::Rectangle { min, max } = &inner.geometry else {
        panic!("not a rectangle");
    };
    (
        min.get_coordinate(param).unwrap(),
        max.get_coordinate(param).unwrap(),
    )
}

#[test]
fn a_positive_gate_moves_its_lower_edge_to_the_solved_value() {
    let moved = translate_edge_to(&rect(100.0, 100.0, 300.0, 300.0), X, Bound::Above, 150.0)
        .expect("a rectangle bounding x can be positioned");
    assert_eq!(edges(&moved, X).0, 150.0);
}

#[test]
fn a_negative_gate_moves_its_upper_edge_instead() {
    // The edge a rule positions is the one the gate keeps events from, which is
    // the other one entirely when the gate keeps the negatives.
    let moved = translate_edge_to(&rect(100.0, 100.0, 300.0, 300.0), X, Bound::Below, 250.0)
        .expect("a rectangle bounding x can be positioned");
    assert_eq!(edges(&moved, X).1, 250.0);
}

#[test]
fn the_gate_moves_rather_than_resizes() {
    // The whole point. Real hand gating translates: across 41 gates over 117
    // files the width held to seven figures while the edge moved.
    let before = rect(100.0, 100.0, 300.0, 300.0);
    let (low, high) = edges(&before, X);
    let moved = translate_edge_to(&before, X, Bound::Above, 150.0).unwrap();
    let (new_low, new_high) = edges(&moved, X);
    assert_eq!(new_low, 150.0);
    assert_eq!(
        new_high - new_low,
        high - low,
        "the width should not change"
    );
}

#[test]
fn moving_on_one_parameter_leaves_the_other_alone() {
    let before = rect(100.0, 100.0, 300.0, 300.0);
    let moved = translate_edge_to(&before, X, Bound::Above, 150.0).unwrap();
    assert_eq!(edges(&moved, Y), edges(&before, Y));
}

#[test]
fn an_unbounded_opposite_edge_stays_unbounded() {
    // A positive gate is usually open at the top, and Omiq writes that openness
    // as 1e16. Shifting it by the delta would be arithmetic on a flag, and
    // rounding it into a real number would close a side the person left open.
    let moved =
        translate_edge_to(&rect(100.0, 100.0, 1e16, 300.0), X, Bound::Above, 150.0).unwrap();
    let (low, high) = edges(&moved, X);
    assert_eq!(low, 150.0);
    assert_eq!(high, 1e16, "the sentinel should survive untouched");
}

#[test]
fn positioning_the_unbounded_edge_itself_is_an_error() {
    // Nothing to move: the rule would be positioning a side that is not there.
    let err = translate_edge_to(&rect(100.0, 100.0, 1e16, 300.0), X, Bound::Below, 150.0);
    assert!(matches!(err, Err(ApplyError::UnboundedEdge(_))));
}

#[test]
fn a_parameter_the_gate_does_not_bound_is_an_error_not_a_guess() {
    let err = translate_edge_to(
        &rect(100.0, 100.0, 300.0, 300.0),
        "BV421-A",
        Bound::Above,
        5.0,
    );
    assert!(matches!(err, Err(ApplyError::NoSuchParameter { .. })));
}

#[test]
fn a_shape_that_is_not_a_rectangle_is_refused() {
    // A polygon has no single edge to translate, and guessing one would move a
    // gate in a way the person could not predict.
    let geometry =
        create_polygon_geometry(vec![(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)], X, Y)
            .unwrap();
    let poly: Arc<dyn DrawableGate> = Arc::new(
        crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate::try_new(
            gate("p", geometry),
            true,
        )
        .unwrap(),
    );
    assert!(matches!(
        translate_edge_to(&poly, X, Bound::Above, 150.0),
        Err(ApplyError::NotARectangle(_))
    ));
}

#[test]
fn moving_a_gate_that_is_already_right_changes_nothing() {
    let before = rect(100.0, 100.0, 300.0, 300.0);
    let moved = translate_edge_to(&before, X, Bound::Above, 100.0).unwrap();
    assert_eq!(edges(&moved, X), edges(&before, X));
}

#[test]
fn the_gates_identity_survives_the_move() {
    // The override is keyed by gate id, and filtering looks the gate up by it.
    // A rebuilt gate that minted a new id would write an override nothing reads.
    let before = rect(100.0, 100.0, 300.0, 300.0);
    let moved = translate_edge_to(&before, X, Bound::Above, 150.0).unwrap();
    assert_eq!(moved.get_id(), before.get_id());
    assert_eq!(moved.get_name(), before.get_name());
    assert_eq!(moved.get_params(), before.get_params());
}

// ── which specimen a file belongs to ──────────────────────────────────────

fn metadata(file: &str, pairs: &[(&str, &str)]) -> crate::omiq::metadata::MetaDataFileMap {
    let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
    for (k, v) in pairs {
        columns.insert(Arc::from(*k), Arc::from(*v));
    }
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    map.insert(Arc::from(file) as Arc<str>, columns);
    map
}

#[test]
fn a_files_specimen_comes_from_the_column_the_pairing_names() {
    let pairing = SamplePairing::default();
    let map = metadata("f1", &[("SampleID", "QC-A"), ("SampleType", "FS")]);
    let key = specimen_of(&pairing, &Arc::from("f1"), &map).expect("f1 has a sample id");
    assert_eq!(&*key.parameter, "SampleID");
    assert_eq!(&*key.group, "QC-A");
}

#[test]
fn renaming_the_column_in_the_pairing_follows_through() {
    // The column that groups a specimen is configuration, not a constant - this
    // is what makes the same rules work on the next dataset.
    let pairing = SamplePairing {
        sample_id_column: Arc::from("Donor"),
        ..SamplePairing::default()
    };
    let map = metadata("f1", &[("Donor", "D7"), ("SampleID", "QC-A")]);
    let key = specimen_of(&pairing, &Arc::from("f1"), &map).unwrap();
    assert_eq!(&*key.group, "D7");
}

#[test]
fn a_file_with_no_sample_id_has_no_specimen() {
    // Better to place nothing than to place every unlabelled file together.
    let pairing = SamplePairing::default();
    let map = metadata("f1", &[("SampleType", "FS")]);
    assert!(specimen_of(&pairing, &Arc::from("f1"), &map).is_none());
}

// ── the override reaching the right files ─────────────────────────────────

/// A gate registered globally, plus metadata placing three files in two
/// specimens: the full stain and FMO of QC-A, and an unrelated QC-B.
fn workflow() -> (
    crate::gate_editor::gates::GateState,
    Arc<dyn DrawableGate>,
    crate::omiq::metadata::MetaDataFileMap,
) {
    use crate::gate_editor::gates::GateState;
    use crate::gate_editor::gates::gate_store::GateSource;

    let mut state = GateState::default();
    let global = rect(100.0, 100.0, 300.0, 300.0);
    state.place_gate(&[global.get_id()], &global, &GateSource::Global);

    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, specimen, kind) in [
        ("fs_a", "QC-A", "FS"),
        ("fmx_a", "QC-A", "FMX"),
        ("fs_b", "QC-B", "FS"),
    ] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(specimen));
        columns.insert(Arc::from("SampleType"), Arc::from(kind));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    (state, global, map)
}

fn edge_for(
    state: &crate::gate_editor::gates::GateState,
    gate_id: &Arc<str>,
    file: &str,
    map: &crate::omiq::metadata::MetaDataFileMap,
) -> f32 {
    let resolved = state
        .gate_for_file(gate_id, &Arc::from(file), map)
        .expect("every file resolves to some position");
    edges(&resolved, X).0
}

#[test]
fn a_solved_position_reaches_every_file_of_that_specimen() {
    // The 0.2-0.5% rule measures the FMO and gates the full stain, and both
    // want the same line - the FMO to show it captures the band, the full stain
    // to read the positives off.
    use crate::gate_rules::autogate::place_for_specimen;

    let (mut state, global, map) = workflow();
    let pairing = SamplePairing::default();
    let specimen = specimen_of(&pairing, &Arc::from("fs_a"), &map).unwrap();
    let moved = translate_edge_to(&global, X, Bound::Above, 150.0).unwrap();
    place_for_specimen(&mut state, &global.get_id(), &specimen, &moved);

    assert_eq!(edge_for(&state, &global.get_id(), "fs_a", &map), 150.0);
    assert_eq!(edge_for(&state, &global.get_id(), "fmx_a", &map), 150.0);
}

#[test]
fn another_specimen_keeps_the_position_that_was_drawn() {
    // Positioning one specimen must not touch the rest, or one solve would
    // quietly restate the whole experiment.
    use crate::gate_rules::autogate::place_for_specimen;

    let (mut state, global, map) = workflow();
    let pairing = SamplePairing::default();
    let specimen = specimen_of(&pairing, &Arc::from("fs_a"), &map).unwrap();
    let moved = translate_edge_to(&global, X, Bound::Above, 150.0).unwrap();
    place_for_specimen(&mut state, &global.get_id(), &specimen, &moved);

    assert_eq!(edge_for(&state, &global.get_id(), "fs_b", &map), 100.0);
}

#[test]
fn the_global_position_survives_being_overridden() {
    // The gate a person drew is the fallback for every specimen that has not
    // been solved, and the thing to fall back to if the rules are wrong.
    use crate::gate_rules::autogate::place_for_specimen;

    let (mut state, global, map) = workflow();
    let pairing = SamplePairing::default();
    let specimen = specimen_of(&pairing, &Arc::from("fs_a"), &map).unwrap();
    let moved = translate_edge_to(&global, X, Bound::Above, 150.0).unwrap();
    place_for_specimen(&mut state, &global.get_id(), &specimen, &moved);

    assert_eq!(
        edges(&state.registered_gate(&global.get_id()).unwrap(), X).0,
        100.0
    );
}

#[test]
fn solving_a_specimen_twice_replaces_rather_than_stacks() {
    // Re-running the rules after loading more files is ordinary, and the second
    // answer is the one that counts.
    use crate::gate_rules::autogate::place_for_specimen;

    let (mut state, global, map) = workflow();
    let pairing = SamplePairing::default();
    let specimen = specimen_of(&pairing, &Arc::from("fs_a"), &map).unwrap();

    for to in [150.0, 175.0] {
        let moved = translate_edge_to(&global, X, Bound::Above, to).unwrap();
        place_for_specimen(&mut state, &global.get_id(), &specimen, &moved);
    }

    assert_eq!(edge_for(&state, &global.get_id(), "fs_a", &map), 175.0);
}

// ── the whole sweep ──────────────────────────────────────────────────────

/// A frame whose X values run 1..=n, so the position of any tail fraction is
/// arithmetic rather than a guess.
fn ramp(n: usize) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    let xs: Vec<f32> = (1..=n).map(|i| i as f32).collect();
    let ys: Vec<f32> = vec![0.0; n];
    df![X => xs, Y => ys].unwrap()
}

/// A state holding one root-level gate named `CD134+`, open above 500 on X and
/// unbounded on every other side - the shape a positive gate actually has.
fn one_positive_gate() -> (crate::gate_editor::gates::GateState, Arc<str>) {
    use crate::gate_editor::gates::GateState;
    use crate::gate_editor::gates::gate_store::GateSource;
    use crate::gate_editor::gates::gate_types::PrimaryGateType;
    use crate::gate_editor::plots::axis_store::PlotMapper;
    use flow_fcs::TransformType;

    let mapper = PlotMapper::new(
        600.0,
        600.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    );
    let mut state = GateState::default();
    state
        .add_gate(
            &mapper,
            300.0,
            300.0,
            Arc::from(X),
            Arc::from(Y),
            None,
            None,
            PrimaryGateType::Rectangle,
            Some("CD134+".to_string()),
        )
        .expect("a rectangle can be added at the root");

    let gate_id = state
        .placements()
        .next()
        .map(|(_, p)| p.gate_id.clone())
        .expect("adding a gate leaves a placement");

    // Replace the default geometry with the shape under test, keeping the id
    // the placement points at.
    let geometry = create_rectangle_geometry(
        vec![(500.0, -1e16), (1e16, -1e16), (1e16, 1e16), (500.0, 1e16)],
        X,
        Y,
    )
    .unwrap();
    let mut inner = gate(&gate_id, geometry);
    inner.name = "CD134+".to_string();
    let positive: Arc<dyn DrawableGate> = Arc::new(RectangleGate::try_new(inner, true).unwrap());
    state.place_gate(&[gate_id.clone()], &positive, &GateSource::Global);

    (state, gate_id)
}

fn fs_and_fmx() -> crate::omiq::metadata::MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, kind) in [("fs_a", "FS"), ("fmx_a", "FMX")] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from("QC-A"));
        columns.insert(Arc::from("SampleType"), Arc::from(kind));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    map
}

fn fmx_rule() -> crate::gate_rules::rule_store::RuleStore {
    use crate::gate_rules::rule::{Rule, TailFractionRule};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};

    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("CD134+"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Partner(Arc::from("FMX")),
            rule: Rule::TailFraction(TailFractionRule::new((0.002, 0.005))),
        },
    );
    store
}

/// Measure both files, then position everything the rules cover.
fn sweep(
    state: &mut crate::gate_editor::gates::GateState,
    store: &crate::gate_rules::rule_store::RuleStore,
    map: &crate::omiq::metadata::MetaDataFileMap,
) -> crate::gate_rules::autogate::Report {
    use crate::gate_rules::autogate::{measure_file, position_all};

    let frame = ramp(1000);
    let mut measured = Vec::new();
    for file in ["fs_a", "fmx_a"] {
        measured.extend(measure_file(state, &Arc::from(file), &frame, map).unwrap());
    }
    position_all(state, store, &measured, map)
}

#[test]
fn a_rule_positions_the_gate_where_it_solved() {
    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &fmx_rule(), &map);

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.file == "fs_a")
        .expect("the full stain should be positioned");

    // 0.2-0.5% of 1000 events is 2 to 5, so the line belongs between the 5th
    // and 2nd highest value - 995 and 999 on a 1..=1000 ramp.
    assert!(
        placed.to > 994.0 && placed.to < 1000.0,
        "solved at {}, which is not in the 0.2-0.5% band",
        placed.to
    );
    assert_eq!(placed.from, 500.0, "it should report where it moved from");

    let on_file = state
        .gate_for_file(&gate_id, &Arc::from("fs_a"), &map)
        .unwrap();
    assert_eq!(
        edges(&on_file, X).0 as f64,
        placed.to,
        "the gate should sit where the report says"
    );
}

#[test]
fn the_solved_gate_is_still_open_at_the_top() {
    // A positive gate keeps everything above the line. Translating it must not
    // quietly cap it.
    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    sweep(&mut state, &fmx_rule(), &map);

    let on_file = state
        .gate_for_file(&gate_id, &Arc::from("fs_a"), &map)
        .unwrap();
    assert_eq!(edges(&on_file, X).1, 1e16);
}

#[test]
fn the_rule_reads_the_fmo_and_gates_the_full_stain() {
    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &fmx_rule(), &map);

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.file == "fs_a")
        .unwrap();
    assert_eq!(&*placed.measured_on, "fmx_a");
    assert_eq!(&*placed.specimen, "QC-A");
}

#[test]
fn a_rule_naming_another_parameter_leaves_the_gate_alone() {
    // The rule is about a marker. A gate that thresholds something else is not
    // its business, whatever the gate is called.
    use crate::gate_rules::rule::{Rule, TailFractionRule};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};

    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("CD134+"),
        GateRule {
            parameter: Arc::from("BV421-A"),
            bound: Bound::Above,
            measured_on: MeasuredOn::Partner(Arc::from("FMX")),
            rule: Rule::TailFraction(TailFractionRule::new((0.002, 0.005))),
        },
    );
    let report = sweep(&mut state, &store, &map);

    assert!(report.positioned.is_empty());
    let on_file = state
        .gate_for_file(&gate_id, &Arc::from("fs_a"), &map)
        .unwrap();
    assert_eq!(
        edges(&on_file, X).0,
        500.0,
        "the gate should not have moved"
    );
}

#[test]
fn a_gate_with_no_rule_is_left_where_it_was() {
    use crate::gate_rules::rule_store::RuleStore;

    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &RuleStore::default(), &map);

    assert!(report.positioned.is_empty());
    assert!(
        report.skipped.is_empty(),
        "a gate nobody wrote a rule for is not a failure"
    );
    let on_file = state
        .gate_for_file(&gate_id, &Arc::from("fs_a"), &map)
        .unwrap();
    assert_eq!(edges(&on_file, X).0, 500.0);
}

#[test]
fn a_missing_partner_is_reported_rather_than_guessed() {
    use crate::gate_rules::autogate::{measure_file, position_all};

    let (mut state, _) = one_positive_gate();
    // Only the full stain exists - the FMO was never run.
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
    columns.insert(Arc::from("SampleID"), Arc::from("QC-A"));
    columns.insert(Arc::from("SampleType"), Arc::from("FS"));
    map.insert(Arc::from("fs_a") as Arc<str>, columns);

    let frame = ramp(1000);
    let measured = measure_file(&state, &Arc::from("fs_a"), &frame, &map).unwrap();
    let report = position_all(&mut state, &fmx_rule(), &measured, &map);

    assert!(report.positioned.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].reason.contains("reference"));
}

#[test]
fn low_confidence_placements_are_the_ones_flagged() {
    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &fmx_rule(), &map);

    // Nothing is below a floor of zero, and everything is below a floor of one.
    assert_eq!(report.needs_review(0.0).count(), 0);
    assert_eq!(report.needs_review(1.01).count(), report.positioned.len());
}
