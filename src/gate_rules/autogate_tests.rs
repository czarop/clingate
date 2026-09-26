//! Tests for writing a solved threshold onto a gate.
//!
//! The invariant that matters most is that the gate *moves*. A rule yields one
//! number for one edge, and the obvious implementation - set that edge, leave
//! the rest - silently resizes the gate, which is not what the hand gating
//! does and not what the person drew.

#![cfg(test)]

use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_rules::autogate::{ApplyError, boundary_at, specimen_of, translate_edge_to};
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

/// A triangle spanning x 100..300, as a `DrawableGate`.
fn triangle() -> Arc<dyn DrawableGate> {
    let geometry =
        create_polygon_geometry(vec![(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)], X, Y)
            .unwrap();
    Arc::new(
        crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate::try_new(
            gate("p", geometry),
            true,
        )
        .unwrap(),
    )
}

/// Every vertex's coordinate on `param`, in order.
fn polygon_points(gate: &Arc<dyn DrawableGate>, param: &str) -> Vec<f32> {
    let inner = gate.get_gate_ref(None).unwrap();
    let GateGeometry::Polygon { nodes, .. } = &inner.geometry else {
        panic!("not a polygon");
    };
    nodes
        .iter()
        .map(|n| n.get_coordinate(param).unwrap())
        .collect()
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
fn a_polygon_slides_along_the_parameter_keeping_its_shape() {
    // Real workflows are full of polygons - 41 of 211 gates in one export - and
    // refusing them meant the autogater did nothing at all, silently, for the
    // gates a person most wanted positioned.
    let before = triangle();
    let moved = translate_edge_to(&before, X, Bound::Above, 150.0)
        .expect("a polygon bounding x can be positioned");

    assert_eq!(polygon_points(&moved, X), vec![150.0, 350.0, 250.0]);
    assert_eq!(
        polygon_points(&moved, Y),
        polygon_points(&before, Y),
        "the other parameter should not move"
    );
}

#[test]
fn a_polygon_keeps_every_vertex_when_it_moves() {
    // Sliding by a delta, not rebuilding from a bounding box: a polygon squashed
    // into its own bounding rectangle would gate a different population.
    let moved = translate_edge_to(&triangle(), X, Bound::Above, 150.0).unwrap();
    assert_eq!(polygon_points(&moved, X).len(), 3);
}

#[test]
fn a_negative_polygon_moves_its_upper_extent() {
    let moved = translate_edge_to(&triangle(), X, Bound::Below, 250.0).unwrap();
    let xs = polygon_points(&moved, X);
    assert_eq!(xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max), 250.0);
}

#[test]
fn an_ellipse_is_refused_rather_than_moved_wrongly() {
    // An ellipse carries a rotation, so its extent on a parameter is not simply
    // its radius. Better to say so than to slide it by the wrong amount.
    let geometry = crate::omiq::deserialise::create_omiq_ellipse_geometry(
        (100.0, 200.0),
        (300.0, 200.0),
        (200.0, 250.0),
        X,
        Y,
    )
    .unwrap();
    let ellipse: Arc<dyn DrawableGate> = Arc::new(
        crate::gate_editor::gates::gate_single::ellipse_gate::EllipseGate::try_new(
            gate("e", geometry),
            true,
        )
        .unwrap(),
    );
    assert!(matches!(
        translate_edge_to(&ellipse, X, Bound::Above, 150.0),
        Err(ApplyError::UnsupportedShape(_))
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

    sweep_over(state, store, map, &["fs_a", "fmx_a"])
}

/// The same, over whichever files the test has metadata for.
fn sweep_over(
    state: &mut crate::gate_editor::gates::GateState,
    store: &crate::gate_rules::rule_store::RuleStore,
    map: &crate::omiq::metadata::MetaDataFileMap,
    files: &[&str],
) -> crate::gate_rules::autogate::Report {
    use crate::gate_rules::autogate::{measure_file, position_all};

    let frame = ramp(1000);
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for file in files {
        let (m, u) = measure_file(state, &Arc::from(*file), &frame, map, store).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    position_all(state, store, &measured, &unmeasured, map)
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
    let (measured, unmeasured) =
        measure_file(&state, &Arc::from("fs_a"), &frame, &map, &fmx_rule()).unwrap();
    let report = position_all(&mut state, &fmx_rule(), &measured, &unmeasured, &map);

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

#[test]
fn a_gate_a_rule_names_but_cannot_measure_says_so() {
    // The silent case: a shape that yields no measurement never reaches the
    // rule check, so nothing was positioned, nothing was skipped, and the run
    // reported "0 gates" with no clue why. An unmeasurable gate a rule names
    // has to account for itself.
    use crate::gate_rules::autogate::{Unmeasured, position_all};

    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let unmeasured = vec![Unmeasured {
        gate_id: Arc::from("whatever"),
        gate: Arc::from("CD134+"),
        parent_gate: Some(Arc::from("CD4+")),
        reason: "drawn as a shape a rule cannot slide".to_string(),
    }];
    let report = position_all(&mut state, &fmx_rule(), &[], &unmeasured, &map);

    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].reason.contains("cannot slide"));
}

#[test]
fn a_gate_no_rule_names_is_never_measured_at_all() {
    // Most gates in a workflow have no rule. Measuring them would cost a
    // filtered pass over the frame each and bury the handful that matter, so
    // they are passed over before any of that - and never reported.
    use crate::gate_rules::autogate::measure_file;
    use crate::gate_rules::rule_store::RuleStore;

    let (state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let frame = ramp(1000);

    let (measured, unmeasured) = measure_file(
        &state,
        &Arc::from("fs_a"),
        &frame,
        &map,
        &RuleStore::default(),
    )
    .unwrap();
    assert!(measured.is_empty());
    assert!(
        unmeasured.is_empty(),
        "a gate with no rule is not a problem"
    );
}

#[test]
fn a_band_too_narrow_for_the_population_is_flagged_not_hidden() {
    // A gate admits a whole number of events. Over a small population a
    // 0.2-0.5% band can contain no achievable fraction at all, and ties in the
    // data can block the ones it does contain - so the solver returns the
    // nearest achievable instead. That is worth having and not worth trusting
    // silently, which is what "0.19% when the rule said 0.2" looks like.
    use crate::gate_rules::autogate::{measure_file, position_all};

    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();

    // 600 events: the band allows 2 to 3, and every value is distinct, so the
    // midpoint is reachable.
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    let frame = ramp(600);
    for file in ["fs_a", "fmx_a"] {
        let (m, u) = measure_file(&state, &Arc::from(file), &frame, &map, &fmx_rule()).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let report = position_all(&mut state, &fmx_rule(), &measured, &unmeasured, &map);
    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.file == "fs_a")
        .expect("positioned");
    assert!(placed.in_band, "2 or 3 of 600 is inside 0.2-0.5%");
    assert_eq!(placed.reference_events, 600);

    // 200 events: 0.2-0.5% is 0.4 to 1.0 events, so only one whole count is
    // allowed and the nearest achievable may miss the band entirely.
    let (mut state, _) = one_positive_gate();
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    let frame = ramp(200);
    for file in ["fs_a", "fmx_a"] {
        let (m, u) = measure_file(&state, &Arc::from(file), &frame, &map, &fmx_rule()).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let report = position_all(&mut state, &fmx_rule(), &measured, &unmeasured, &map);
    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.file == "fs_a")
        .expect("positioned");
    assert_eq!(placed.achieved, 0.005, "1 of 200");
    // Whatever it achieved, a miss must reach needs_review even at full
    // confidence - the count is the problem, not the confidence.
    if !placed.in_band {
        assert_eq!(report.needs_review(0.0).count(), 1);
    }
}

#[test]
fn a_gate_already_in_band_is_left_exactly_where_it_is() {
    // Re-running must be safe. A gate capturing what the rule asks for is
    // already right, and moving it is at best wasted work - at worst it lands
    // outside the band the gate was already inside.
    use crate::gate_rules::autogate::{measure_file, position_all};

    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    let frame = ramp(1000);

    // Solve once, then solve again from the result.
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for file in ["fs_a", "fmx_a"] {
        let (m, u) = measure_file(&state, &Arc::from(file), &frame, &map, &fmx_rule()).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let first = position_all(&mut state, &fmx_rule(), &measured, &unmeasured, &map);
    assert_eq!(
        first.positioned.len(),
        1,
        "one answer per specimen, however many of its files were measured"
    );
    let after_first = edges(
        &state
            .gate_for_file(&gate_id, &Arc::from("fs_a"), &map)
            .unwrap(),
        X,
    );

    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for file in ["fs_a", "fmx_a"] {
        let (m, u) = measure_file(&state, &Arc::from(file), &frame, &map, &fmx_rule()).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let second = position_all(&mut state, &fmx_rule(), &measured, &unmeasured, &map);

    assert!(
        second.positioned.is_empty(),
        "nothing should move on a second run"
    );
    assert_eq!(second.unchanged.len(), 1, "reported once as already right");
    assert_eq!(
        edges(
            &state
                .gate_for_file(&gate_id, &Arc::from("fs_a"), &map)
                .unwrap(),
            X
        ),
        after_first,
        "the gate must not drift"
    );
}

#[test]
fn a_rule_with_no_band_always_re_solves() {
    // A percentile-offset rule names a position rather than a range, so there
    // is no "already correct" to test - it is solved every time.
    use crate::gate_rules::rule::{PercentileOffsetRule, Rule};

    assert!(
        Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.5))
            .accepted_band()
            .is_none()
    );
    assert_eq!(
        Rule::TailFraction(crate::gate_rules::rule::TailFractionRule::new((
            0.002, 0.005
        )))
        .accepted_band(),
        Some((0.002, 0.005))
    );
}

/// A gate that also bounds y, so its sides exclude events its x line admits.
fn two_dimensional_gate() -> (crate::gate_editor::gates::GateState, Arc<str>) {
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
        .unwrap();
    let gate_id = state
        .placements()
        .next()
        .map(|(_, p)| p.gate_id.clone())
        .unwrap();

    // Open above 500 on x, but capped at y <= 0 - and the ramp's y is all 0.0,
    // so the cap admits everything. Narrowing it is what the test varies.
    let geometry = create_rectangle_geometry(
        vec![(500.0, -1.0), (1e16, -1.0), (1e16, 1.0), (500.0, 1.0)],
        X,
        Y,
    )
    .unwrap();
    let mut inner = gate("r2", geometry);
    inner.name = "CD134+".to_string();
    let g: Arc<dyn DrawableGate> = Arc::new(RectangleGate::try_new(inner, true).unwrap());
    state.place_gate(&[gate_id.clone()], &g, &GateSource::Global);
    (state, gate_id)
}

#[test]
fn a_gate_that_bounds_both_axes_is_still_positioned() {
    // The old rule was "exactly one edge may lie inside the data", which read a
    // gate bounding both axes as a window with no line to solve and refused it
    // outright. The rule names its parameter; there is nothing to infer.
    use crate::gate_rules::autogate::measure_file;

    let (state, _) = two_dimensional_gate();
    let map = fs_and_fmx();
    let frame = ramp(1000);

    let (measured, unmeasured) =
        measure_file(&state, &Arc::from("fmx_a"), &frame, &map, &fmx_rule()).unwrap();
    assert_eq!(measured.len(), 1, "it should be measured, not refused");
    assert!(unmeasured.is_empty());
    let line = measured[0]
        .line
        .as_ref()
        .expect("a rule that positions an axis is measured along one");
    assert_eq!(&*line.parameter, X);
    assert_eq!(line.current, 500.0);
}

#[test]
fn what_a_gate_captures_is_asked_through_the_screens_own_statistic() {
    // The number a run reports has to be the number a person reads off the
    // plot. Rather than count events past the line - which ignores the gate's
    // other sides - this delegates to the same function that draws the
    // percentage beside the gate, so the two agree by construction.
    use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
    use crate::gate_editor::plots::plot_store::EventIndexMapped;
    use crate::gate_rules::autogate::admitted_by;
    use polars::prelude::*;

    let xs: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
    let ys: Vec<f32> = vec![0.0; 1000];
    let frame = Arc::new(df![X => xs, Y => ys].unwrap());
    let index = EventIndexMapped {
        event_index: get_event_mask_from_scaled_df(frame.clone(), Arc::from(X), Arc::from(Y))
            .unwrap(),
        index_map: Arc::new((0..1000).collect()),
    };

    // Open above 995 on x, spanning y: the gate includes its boundary, so
    // 995..=1000 is six events.
    let open = rect(995.0, -1e16, 1e16, 1e16);
    // Compared loosely: the on-screen statistic is computed in f32, and taking
    // it as it is - rounding included - is the whole point.
    assert!((admitted_by(&open, &index).unwrap() - 0.006).abs() < 1e-6);

    // The same line, capped on y above the data. Counting past the x line
    // alone would still claim 0.6%; the gate admits nothing, and that is what
    // the plot shows.
    let capped = rect(995.0, 10.0, 1e16, 20.0);
    assert_eq!(admitted_by(&capped, &index), Some(0.0));
}

#[test]
fn a_slanted_gate_is_positioned_by_what_it_holds_not_by_its_corner() {
    // The CD185 case. The gate's boundary slopes, so the population crosses it
    // nowhere near its leftmost vertex. Anchoring that vertex to a
    // one-dimensional threshold moves the gate far too far - a gate meant to
    // hold 0.35% held 0.010%. Sliding it until it holds the band instead works
    // whatever the boundary does.
    use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
    use crate::gate_editor::plots::plot_store::EventIndexMapped;
    use crate::gate_rules::autogate::admitted_by;
    use polars::prelude::*;

    // A population spread over y as well as x, so a slope actually matters.
    let mut xs: Vec<f32> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    for i in 0..1000 {
        xs.push((i % 100) as f32);
        ys.push((i / 100) as f32 * 10.0);
    }
    let frame = Arc::new(df![X => xs, Y => ys].unwrap());
    let index = EventIndexMapped {
        event_index: get_event_mask_from_scaled_df(frame.clone(), Arc::from(X), Arc::from(Y))
            .unwrap(),
        index_map: Arc::new((0..1000).collect()),
    };

    // Open to the right, with a left edge leaning from x=40 at the bottom to
    // x=90 at the top - its extreme vertex is nowhere near its typical boundary.
    let geometry = create_polygon_geometry(
        vec![(40.0, -10.0), (1e6, -10.0), (1e6, 200.0), (90.0, 200.0)],
        X,
        Y,
    )
    .unwrap();
    let mut inner = gate("slanted", geometry);
    inner.name = "CD185+".to_string();
    let slanted: Arc<dyn DrawableGate> = Arc::new(
        crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate::try_new(inner, true)
            .unwrap(),
    );

    let before = admitted_by(&slanted, &index).unwrap();
    assert!(
        before > 0.05,
        "the fixture should start well outside the band, got {before}"
    );

    // Slide it until it holds 0.2-0.5%.
    let values: Vec<f64> = (0..1000).map(|i| (i % 100) as f64).collect();
    let moved = slide_into_band(&slanted, &index, &values, 40.0);
    let after = admitted_by(&moved, &index).unwrap();
    assert!(
        (0.002..=0.005).contains(&after),
        "sliding should land it in the band, got {after}"
    );
}

/// Drive the same search the autogater uses, for a gate keeping the bright side.
fn slide_into_band(
    gate: &Arc<dyn DrawableGate>,
    index: &crate::gate_editor::plots::plot_store::EventIndexMapped,
    values: &[f64],
    current: f64,
) -> Arc<dyn DrawableGate> {
    use crate::gate_rules::autogate::position_by_capture;
    position_by_capture(
        gate,
        X,
        Bound::Above,
        index,
        (0.002, 0.005),
        values,
        current,
    )
    .expect("a position holding the band exists")
    .0
}

#[test]
fn a_gate_keeping_the_dim_side_slides_the_other_way() {
    // The same search drives a negative gate - Live, CD19-, Ki67- - and the
    // direction reverses: sliding such a gate *up* the parameter admits more,
    // not fewer, so a gate holding too much has to come down.
    use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
    use crate::gate_editor::plots::plot_store::EventIndexMapped;
    use crate::gate_rules::autogate::{admitted_by, position_by_capture};
    use polars::prelude::*;

    let xs: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
    let frame = Arc::new(df![X => xs, Y => vec![0.0f32; 1000]].unwrap());
    let index = EventIndexMapped {
        event_index: get_event_mask_from_scaled_df(frame.clone(), Arc::from(X), Arc::from(Y))
            .unwrap(),
        index_map: Arc::new((0..1000).collect()),
    };

    // Open to the left, capped at 800: holds 80% of the population, far too
    // much for a 0.2-0.5% band.
    let gate = rect(-1e16, -1e16, 800.0, 1e16);
    assert!(admitted_by(&gate, &index).unwrap() > 0.5);

    let values: Vec<f64> = (1..=1000).map(|i| i as f64).collect();
    let (moved, to, held) = position_by_capture(
        &gate,
        X,
        Bound::Below,
        &index,
        (0.002, 0.005),
        &values,
        800.0,
    )
    .expect("a position holding the band exists");

    assert!(
        (0.002..=0.005).contains(&held),
        "a Below gate should land in the band too, got {held}"
    );
    assert!(to < 800.0, "it has to come down, not go up - ended at {to}");
    assert_eq!(admitted_by(&moved, &index).unwrap(), held);
}

#[test]
fn a_gate_holding_too_little_moves_the_other_way_again() {
    // The search is not one-directional. A gate holding less than the band has
    // to open up, whichever side it keeps.
    use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
    use crate::gate_editor::plots::plot_store::EventIndexMapped;
    use crate::gate_rules::autogate::{admitted_by, position_by_capture};
    use polars::prelude::*;

    let xs: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
    let frame = Arc::new(df![X => xs, Y => vec![0.0f32; 1000]].unwrap());
    let index = EventIndexMapped {
        event_index: get_event_mask_from_scaled_df(frame.clone(), Arc::from(X), Arc::from(Y))
            .unwrap(),
        index_map: Arc::new((0..1000).collect()),
    };
    let values: Vec<f64> = (1..=1000).map(|i| i as f64).collect();

    // An Above gate starting past the data: holds nothing, so it must come down.
    let empty = rect(2000.0, -1e16, 1e16, 1e16);
    assert_eq!(admitted_by(&empty, &index).unwrap(), 0.0);
    let (_, to, held) = position_by_capture(
        &empty,
        X,
        Bound::Above,
        &index,
        (0.002, 0.005),
        &values,
        2000.0,
    )
    .expect("reachable");
    assert!((0.002..=0.005).contains(&held), "got {held}");
    assert!(to < 2000.0, "it has to come down to reach the data");
}

#[test]
fn a_line_gate_stays_a_line_when_it_moves() {
    // A line holds a rectangle - it is a threshold drawn as one edge with a
    // height - so rebuilding from the geometry alone turned every line into a
    // box, which draws differently and exports as a different Omiq type.
    use crate::gate_editor::gates::gate_single::line_gate::LineGate;

    let geometry = create_rectangle_geometry(
        vec![(100.0, 0.0), (300.0, 0.0), (300.0, 50.0), (100.0, 50.0)],
        X,
        Y,
    )
    .unwrap();
    let line: Arc<dyn DrawableGate> =
        Arc::new(LineGate::try_new(gate("l", geometry), 50.0, true).unwrap());

    let moved = translate_edge_to(&line, X, Bound::Above, 150.0).expect("a line can be moved");

    assert!(
        moved.as_any().downcast_ref::<LineGate>().is_some(),
        "it came back as something other than a line"
    );
    assert_eq!(edges(&moved, X).0, 150.0, "and it moved");
    assert_eq!(
        moved.as_any().downcast_ref::<LineGate>().unwrap().height,
        50.0,
        "keeping its height"
    );
}

#[test]
fn a_rectangle_is_still_rebuilt_as_a_rectangle() {
    use crate::gate_editor::gates::gate_single::line_gate::LineGate;
    let moved =
        translate_edge_to(&rect(100.0, 100.0, 300.0, 300.0), X, Bound::Above, 150.0).unwrap();
    assert!(moved.as_any().downcast_ref::<LineGate>().is_none());
}

#[test]
fn positioning_a_linked_gate_moves_it_everywhere_it_is_applied() {
    // A linked gate is one gate at two points in the tree, and an override is
    // keyed by gate - so positioning it under one parent moves it under the
    // other too. That is what linking means, and the rule cannot break it:
    // there is only ever one position to write.
    use crate::gate_editor::gates::gate_store::GateSource;
    use crate::gate_rules::autogate::place_for_specimen;
    use crate::omiq::metadata::MetaDataKey;

    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    let specimen = MetaDataKey {
        parameter: Arc::from("SampleID"),
        group: Arc::from("QC-A"),
    };

    let global = state.registered_gate(&gate_id).unwrap();
    let moved = translate_edge_to(&global, X, Bound::Above, 750.0).unwrap();
    place_for_specimen(&mut state, &gate_id, &specimen, &moved);

    // Whichever placement asks, the gate resolves to the moved position: the
    // override is on the gate, and a linked gate is the same gate.
    for file in ["fs_a", "fmx_a"] {
        assert_eq!(
            edges(
                &state
                    .gate_for_file(&gate_id, &Arc::from(file), &map)
                    .unwrap(),
                X
            )
            .0,
            750.0
        );
    }
    assert!(matches!(
        state
            .get_current_sample(Arc::from("fs_a"), &map[&Arc::from("fs_a") as &Arc<str>])
            .gate_origins
            .get(&gate_id),
        Some(GateSource::Group(_))
    ));
}

#[test]
fn a_specimen_is_positioned_once_however_many_files_it_has() {
    // The solve is keyed on (specimen, gate), so a linked gate measured at two
    // placements, or a specimen with several files, still yields one answer -
    // not one per file that then overwrite each other.
    use crate::gate_rules::autogate::{measure_file, position_all};

    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let frame = ramp(1000);
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for file in ["fs_a", "fmx_a"] {
        let (m, u) = measure_file(&state, &Arc::from(file), &frame, &map, &fmx_rule()).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    assert_eq!(measured.len(), 2, "both files are measured");

    let report = position_all(&mut state, &fmx_rule(), &measured, &unmeasured, &map);
    assert_eq!(
        report.positioned.len(),
        1,
        "but the specimen is positioned once"
    );
}

// ─── the boundary at a height ────────────────────────────────────────────────

/// A gate's geometry, for the boundary tests.
fn geometry_of(gate: &Arc<dyn DrawableGate>) -> GateGeometry {
    gate.get_gate_ref(None).unwrap().geometry.clone()
}

#[test]
fn a_full_height_rectangle_has_the_same_boundary_everywhere() {
    let g = geometry_of(&rect(200.0, -1e16, 400.0, 1e16));
    for height in [-5000.0, 0.0, 250.0, 5000.0] {
        assert_eq!(
            boundary_at(&g, X, Y, Bound::Above, height),
            Some(200.0),
            "at height {height}"
        );
    }
}

#[test]
fn a_rectangle_says_nothing_outside_its_own_span() {
    // A gate boxed in its other axis makes no statement about events above or
    // below it, and inventing one would read the negative from events the gate
    // never reached.
    let g = geometry_of(&rect(200.0, 100.0, 400.0, 300.0));
    assert_eq!(boundary_at(&g, X, Y, Bound::Above, 200.0), Some(200.0));
    assert_eq!(boundary_at(&g, X, Y, Bound::Below, 200.0), Some(400.0));
    assert_eq!(boundary_at(&g, X, Y, Bound::Above, 50.0), None);
    assert_eq!(boundary_at(&g, X, Y, Bound::Above, 400.0), None);
}

#[test]
fn a_slanted_polygon_has_a_different_boundary_at_every_height() {
    // The triangle spans x 100..300 at its base and narrows to a point at
    // (200, 300). Half way up, its left edge has moved in to 150 - so an event
    // at that height is 50 units from the boundary, not from the leftmost
    // vertex at 100. Reading the extreme vertex instead put the cut well below
    // the real edge and read the negative from a sliver.
    let g = geometry_of(&triangle());
    assert_eq!(boundary_at(&g, X, Y, Bound::Above, 200.0), Some(150.0));
    assert_eq!(boundary_at(&g, X, Y, Bound::Below, 200.0), Some(250.0));
    // At the base it is the full width.
    let low = boundary_at(&g, X, Y, Bound::Above, 101.0).unwrap();
    assert!(
        (low - 100.5).abs() < 1.0,
        "at the base it opens out, got {low}"
    );
}

#[test]
fn a_polygon_says_nothing_below_itself() {
    let g = geometry_of(&triangle());
    assert_eq!(boundary_at(&g, X, Y, Bound::Above, 50.0), None);
    assert_eq!(boundary_at(&g, X, Y, Bound::Above, 400.0), None);
}

// ─── the reference is left as it was drawn ───────────────────────────────────

fn two_specimens() -> crate::omiq::metadata::MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, id) in [("fs_qc", "QC-A"), ("fs_b", "DONOR-B")] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(id));
        columns.insert(Arc::from("SampleType"), Arc::from("FS"));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    map
}

/// A rule calibrated from one named file, which is how the reference-sample
/// rules are written.
fn rule_measured_on(file: &str) -> crate::gate_rules::rule_store::RuleStore {
    use crate::gate_rules::rule::{Rule, TailFractionRule};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};

    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("CD134+"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from(file)),
            rule: Rule::TailFraction(TailFractionRule::new((0.002, 0.005))),
        },
    );
    store
}

#[test]
fn the_reference_sample_keeps_the_gate_a_person_drew() {
    // Everything else is calibrated from where this gate sits, so moving it
    // overwrites the hand placement - and the next run would calibrate from the
    // moved gate, so the drift would compound every time it was run.
    let (mut state, gate_id) = one_positive_gate();
    let map = two_specimens();
    let report = sweep_over(
        &mut state,
        &rule_measured_on("fs_qc"),
        &map,
        &["fs_qc", "fs_b"],
    );

    assert!(
        report.positioned.iter().all(|p| &*p.specimen != "QC-A"),
        "the specimen the rule calibrates from should not be positioned"
    );
    assert_eq!(
        report.reference.len(),
        1,
        "and it should be reported as the reference rather than silently dropped"
    );
    assert_eq!(&*report.reference[0].specimen, "QC-A");

    let on_qc = state
        .gate_for_file(&gate_id, &Arc::from("fs_qc"), &map)
        .unwrap();
    assert_eq!(
        edges(&on_qc, X).0,
        500.0,
        "the reference gate should still be where it was drawn"
    );
}

#[test]
fn the_other_specimens_are_still_positioned_from_it() {
    // The other half of the same rule: leaving the reference alone must not
    // stop anything else being placed against it.
    let (mut state, gate_id) = one_positive_gate();
    let map = two_specimens();
    let report = sweep_over(
        &mut state,
        &rule_measured_on("fs_qc"),
        &map,
        &["fs_qc", "fs_b"],
    );

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("the other specimen should be positioned");
    assert_eq!(&*placed.measured_on, "fs_qc");

    let on_b = state
        .gate_for_file(&gate_id, &Arc::from("fs_b"), &map)
        .unwrap();
    assert!(
        edges(&on_b, X).0 > 994.0,
        "it should have moved into the band, got {}",
        edges(&on_b, X).0
    );
}

#[test]
fn a_rule_measured_on_the_partner_still_positions_its_own_specimen() {
    // The skip is only for a rule naming one sample. A partner rule's reference
    // sits inside the specimen it is positioning - the FMO measures, the full
    // stain is gated - so skipping that would skip everything.
    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &fmx_rule(), &map);

    assert!(report.reference.is_empty());
    assert!(
        report.positioned.iter().any(|p| &*p.specimen == "QC-A"),
        "a partner rule positions the specimen it measured"
    );
}

// ─── the reading reaches the report ──────────────────────────────────────────

/// An above-the-negative rule calibrated on one named file.
fn above_the_negative_rule(file: &str) -> crate::gate_rules::rule_store::RuleStore {
    use crate::gate_rules::rule::{AboveTheNegativeRule, Rule};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};

    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("CD134+"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from(file)),
            rule: Rule::AboveTheNegative(AboveTheNegativeRule::default()),
        },
    );
    store
}

#[test]
fn an_above_the_negative_placement_reports_what_it_read() {
    // Without this the report says a gate moved and gives no way to tell
    // whether the negative moved or was simply measured differently.
    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let report = sweep_over(
        &mut state,
        &above_the_negative_rule("fs_qc"),
        &map,
        &["fs_qc", "fs_b"],
    );

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("the other specimen is positioned");
    let (reference, here) = placed.negative.expect("the reading is reported");

    assert!(reference.spread > 0.0 && here.spread > 0.0);
    assert_eq!(
        reference.at, 500.0,
        "the reference's reading is anchored to the gate as drawn"
    );
    // The figures shown have to be the ones the placement came from.
    assert!(
        (here.centre + here.widths * here.spread - here.at).abs() < 1e-6,
        "the reported reading does not reproduce the gate it placed"
    );
    assert!((here.at - placed.to).abs() < 1e-6);
}

#[test]
fn a_band_rule_reports_no_negative_because_it_reads_none() {
    // Only the above-the-negative rule has a negative to report. A band rule
    // slides the gate until it holds the fraction and never looks at one, so an
    // empty column there is the truth rather than a gap.
    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &fmx_rule(), &map);

    assert!(report.positioned.iter().all(|p| p.negative.is_none()));
}

// ─── which population a placement is judged against ──────────────────────────

#[test]
fn an_above_the_negative_gate_reports_what_it_holds_on_its_own_sample() {
    // It placed the gate from this sample's negative, so what it would capture
    // on the reference describes a sample nobody is looking at. Reported that
    // way it read 5.1% beside a plot showing 16.1%.
    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let report = sweep_over(
        &mut state,
        &above_the_negative_rule("fs_qc"),
        &map,
        &["fs_qc", "fs_b"],
    );

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("positioned");
    assert_eq!(
        &*placed.captured_on, "fs_b",
        "the capture belongs to the sample being gated, not the one it calibrated from"
    );
    assert_eq!(
        &*placed.measured_on, "fs_qc",
        "which is not the file it read"
    );
}

#[test]
fn a_band_rule_still_reports_what_it_holds_on_the_file_it_read() {
    // The other half: a band names a fraction *of the file it reads* - "0.2 to
    // 0.5% of the FMO" - so counting it anywhere else would not answer the
    // question the rule asked.
    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &fmx_rule(), &map);

    let placed = report.positioned.first().expect("positioned");
    assert_eq!(&*placed.measured_on, "fmx_a");
    assert_eq!(
        placed.captured_on, placed.measured_on,
        "a band rule is judged on the population whose fraction it named"
    );
}

#[test]
fn two_specimens_at_the_same_position_can_report_different_captures() {
    // The symptom that gave the bug away: every row's capture was one curve in
    // the position, because all of them were counted on the same population. A
    // capture that varies with the sample cannot do that.
    use crate::gate_rules::autogate::{measure_file, position_all};
    use polars::prelude::*;

    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let store = above_the_negative_rule("fs_qc");

    // Same gate, deliberately different populations.
    let dense = ramp(1000);
    let sparse = {
        let xs: Vec<f32> = (1..=1000).map(|i| (i as f32) * 0.5).collect();
        df![X => xs, Y => vec![0.0f32; 1000]].unwrap()
    };

    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in [("fs_qc", &dense), ("fs_b", &sparse)] {
        let (m, u) = measure_file(&state, &Arc::from(file), frame, &map, &store).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let report = position_all(&mut state, &store, &measured, &unmeasured, &map);

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("positioned");
    // Counted on the sparse frame, which has half the dense one's values, so a
    // gate at the same place holds a different fraction of it.
    assert_eq!(&*placed.captured_on, "fs_b");
    assert_eq!(
        placed.reference_events,
        sparse.height(),
        "and the event count is that population's, not the reference's"
    );
}

// ─── what the confidence score is entitled to complain about ─────────────────

/// A population with a real negative peak at `centre`, plus a scatter of
/// positives above it - the shape the rule is written for. A uniform ramp has
/// no peak to find, so it cannot exercise this at all.
fn with_negative_at(centre: f32) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rand_distr::{Distribution, Normal, Uniform};

    let mut rng = StdRng::seed_from_u64(7);
    let neg = Normal::new(centre, 40.0).unwrap();
    let pos = Uniform::new(centre + 250.0, centre + 400.0).unwrap();
    let mut xs: Vec<f32> = (0..9000).map(|_| neg.sample(&mut rng)).collect();
    xs.extend((0..1000).map(|_| pos.sample(&mut rng)));
    let n = xs.len();
    df![X => xs, Y => vec![0.0f32; n]].unwrap()
}

/// The two specimens measured on populations whose negatives sit far apart, so
/// the gate has to travel a long way to follow the second one's.
fn sweep_with_a_long_move(
    state: &mut crate::gate_editor::gates::GateState,
    store: &crate::gate_rules::rule_store::RuleStore,
    map: &crate::omiq::metadata::MetaDataFileMap,
) -> crate::gate_rules::autogate::Report {
    use crate::gate_rules::autogate::{measure_file, position_all};

    let here = with_negative_at(300.0);
    let far = with_negative_at(600.0);
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in [("fs_qc", &here), ("fs_b", &far)] {
        let (m, u) = measure_file(state, &Arc::from(file), frame, map, store).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    position_all(state, store, &measured, &unmeasured, map)
}

#[test]
fn moving_off_the_reference_is_not_held_against_an_above_the_negative_gate() {
    // Moving the gate to wherever this sample's negative is *is* the rule. A
    // displacement penalty marked 15 of 32 correct placements as zero
    // confidence on a real run, so the flag stopped meaning anything.
    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let mut store = above_the_negative_rule("fs_qc");
    {
        use crate::gate_rules::rule::{AboveTheNegativeRule, NegativeFinder, Rule};
        use crate::gate_rules::rule_store::RuleTarget;
        let mut entry = store.rule_for(&Arc::from("CD134+"), None).unwrap().clone();
        entry.rule = Rule::AboveTheNegative(AboveTheNegativeRule {
            find: NegativeFinder::NegativePeak,
            ..AboveTheNegativeRule::default()
        });
        store.insert(RuleTarget::named("CD134+"), entry);
    }
    let report = sweep_with_a_long_move(&mut state, &store, &map);

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("positioned");
    assert!(
        (placed.to - placed.from).abs() > 150.0,
        "the gate should have travelled a long way here, or this proves nothing - moved {}",
        placed.to - placed.from
    );
    assert_ne!(
        placed.weakest,
        Some(crate::gate_rules::confidence::DISPLACEMENT),
        "distance from the reference is the intended behaviour here, not a fault"
    );
    assert!(
        placed.confidence > 0.0,
        "and it should not be scored to zero for doing what it was asked"
    );
}

#[test]
fn a_band_rule_is_still_judged_on_how_far_it_moved() {
    // The check is meaningful there: a band rule should land near where the
    // equivalent gate was drawn, so a long trip is evidence something is wrong.
    // Only above-the-negative is exempt.
    use crate::gate_rules::confidence::DISPLACEMENT;
    use crate::gate_rules::rule::{AboveTheNegativeRule, Rule, TailFractionRule};
    use crate::gate_rules::threshold::{Status, Threshold};

    let t = Threshold {
        x: 900.0,
        events_admitted: 30,
        fraction_admitted: 0.003,
        parent_events: 10_000,
        count_swing: 0.1,
        parent_spread: 100.0,
        status: Status::InBand,
    };

    let band = Rule::TailFraction(TailFractionRule::new((0.002, 0.005)));
    assert!(
        band.assess(&t, Some(500.0))
            .expect("a band rule is judged on a threshold")
            .get(DISPLACEMENT)
            .is_some(),
        "a band rule keeps the displacement check"
    );
    let above = Rule::AboveTheNegative(AboveTheNegativeRule::default());
    assert!(
        above
            .assess(&t, None)
            .expect("above-the-negative is judged on a threshold")
            .get(DISPLACEMENT)
            .is_none(),
        "above-the-negative is assessed without it"
    );
}

#[test]
fn an_old_sidecar_still_names_a_finder_after_the_rename() {
    // The variants were renamed to say when to use them; rule files written
    // before that must still load, or a person's saved rules silently revert to
    // the default finder.
    use crate::gate_rules::rule::NegativeFinder;
    let old: NegativeFinder = serde_json::from_str("\"DensityPeak\"").unwrap();
    assert_eq!(old, NegativeFinder::NegativePeak);
    let old: NegativeFinder = serde_json::from_str("\"RefineFromGate\"").unwrap();
    assert_eq!(old, NegativeFinder::BelowTheGate);
}

// ─── solving off the store ───────────────────────────────────────────────────

#[test]
fn a_snapshot_solve_matches_an_in_place_one() {
    // Both entry points agree end to end, store included. The invariant itself
    // - that a solve cannot depend on its own writes - is held by `solve_all`
    // taking `&GateState`, which is stronger than anything asserted here; this
    // guards the wiring around it, that `position_all` still applies what
    // `solve_all` returns and both reach the same gates.
    use crate::gate_rules::autogate::{measure_file, position_all, solve_all};

    // Three specimens, so two of them are placed: with only one placement a
    // store comparison passes however badly the placements are applied.
    let map = {
        let mut map = im::HashMap::with_hasher(FxBuildHasher);
        for (file, id) in [("fs_qc", "QC-A"), ("fs_b", "DONOR-B"), ("fs_c", "DONOR-C")] {
            let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
            columns.insert(Arc::from("SampleID"), Arc::from(id));
            columns.insert(Arc::from("SampleType"), Arc::from("FS"));
            map.insert(Arc::from(file) as Arc<str>, columns);
        }
        map
    };
    let store = above_the_negative_rule("fs_qc");
    let here = with_negative_at(300.0);
    let far = with_negative_at(600.0);
    let further = with_negative_at(450.0);

    let measure = |state: &crate::gate_editor::gates::GateState| {
        let mut measured = Vec::new();
        let mut unmeasured = Vec::new();
        for (file, frame) in [("fs_qc", &here), ("fs_b", &far), ("fs_c", &further)] {
            let (m, u) = measure_file(state, &Arc::from(file), frame, &map, &store).unwrap();
            measured.extend(m);
            unmeasured.extend(u);
        }
        (measured, unmeasured)
    };

    // In place, as the editor used to do it.
    let (mut in_place, gate_id) = one_positive_gate();
    let (measured, unmeasured) = measure(&in_place);
    let live = position_all(&mut in_place, &store, &measured, &unmeasured, &map);

    // Against a snapshot, as the worker does it.
    let (mut owner, offline_gate_id) = one_positive_gate();
    let snapshot = owner.clone();
    let (measured, unmeasured) = measure(&snapshot);
    let (offline, placements) = solve_all(&snapshot, &store, &measured, &unmeasured, &map);
    crate::gate_rules::autogate::apply_placements(&mut owner, &placements);

    assert_eq!(
        live.positioned.len(),
        2,
        "two specimens should be placed, or the comparison below proves little"
    );
    assert_eq!(live.positioned.len(), offline.positioned.len());
    assert_eq!(live.reference.len(), offline.reference.len());
    assert_eq!(live.skipped.len(), offline.skipped.len());
    for (a, b) in live.positioned.iter().zip(offline.positioned.iter()) {
        assert_eq!(a.specimen, b.specimen);
        assert_eq!(
            a.to, b.to,
            "the two paths placed {} differently",
            a.specimen
        );
        assert_eq!(a.achieved, b.achieved);
    }

    // And the stores end up holding the same gates, not just the same report.
    for file in ["fs_qc", "fs_b", "fs_c"] {
        let a = in_place
            .gate_for_file(&gate_id, &Arc::from(file), &map)
            .unwrap();
        // Each store minted its own id for the gate, so each is asked with its
        // own; what has to match is where the gate ended up.
        let b = owner
            .gate_for_file(&offline_gate_id, &Arc::from(file), &map)
            .unwrap();
        assert_eq!(
            edges(&a, X),
            edges(&b, X),
            "{file} differs between the paths"
        );
    }
}

#[test]
fn a_snapshot_does_not_see_later_edits_to_the_store() {
    // The reason only the placements come back rather than the whole state: a
    // gate moved while a run is in flight has to survive it.
    use crate::gate_editor::gates::gate_store::GateSource;

    let (mut state, gate_id) = one_positive_gate();
    let snapshot = state.clone();

    // The person drags the gate while the worker is busy.
    let moved = translate_edge_to(
        &state
            .gate_for_file(&gate_id, &Arc::from("fs_qc"), &two_specimens())
            .unwrap(),
        X,
        Bound::Above,
        750.0,
    )
    .unwrap();
    state.place_gate(&[gate_id.clone()], &moved, &GateSource::Global);

    let from_snapshot = snapshot
        .gate_for_file(&gate_id, &Arc::from("fs_qc"), &two_specimens())
        .unwrap();
    assert_eq!(
        edges(&from_snapshot, X).0,
        500.0,
        "the snapshot is the state as it was when the run started"
    );
    let from_store = state
        .gate_for_file(&gate_id, &Arc::from("fs_qc"), &two_specimens())
        .unwrap();
    assert_eq!(edges(&from_store, X).0, 750.0, "and the edit stands");
}

// ─── the line against the gate ───────────────────────────────────────────────

#[test]
fn the_line_and_the_gate_agree_when_the_shape_does_not_interfere() {
    // A gate open on every other side takes exactly what the line takes, so the
    // two columns sitting side by side in the report mean nothing is wrong.
    use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
    use crate::gate_editor::plots::plot_store::EventIndexMapped;
    use crate::gate_rules::autogate::{admitted_by, beyond_the_line};
    use polars::prelude::*;

    let xs: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
    let frame = Arc::new(df![X => xs.clone(), Y => vec![500.0f32; 1000]].unwrap());
    let index = EventIndexMapped {
        event_index: get_event_mask_from_scaled_df(frame, Arc::from(X), Arc::from(Y)).unwrap(),
        index_map: Arc::new((0..1000).collect()),
    };
    let values: Vec<f64> = xs.iter().map(|v| *v as f64).collect();

    // Open above 900 and unbounded on every other side.
    let geometry = create_rectangle_geometry(
        vec![(900.0, -1e16), (1e16, -1e16), (1e16, 1e16), (900.0, 1e16)],
        X,
        Y,
    )
    .unwrap();
    let gate: Arc<dyn DrawableGate> =
        Arc::new(RectangleGate::try_new(gate("open", geometry), true).unwrap());

    let by_gate = admitted_by(&gate, &index).unwrap();
    let by_line = beyond_the_line(&values, Bound::Above, 900.0);
    // Within one event of a thousand, not exact: the gate holds the event
    // sitting exactly on its edge - as the filter below it does - and the
    // naive `beyond_the_line` does not. Worth knowing when reading the two
    // columns against each other - a one-event disagreement is the floor, not
    // a signal.
    assert!(
        (by_gate - by_line).abs() <= 0.0011,
        "gate {by_gate}, line {by_line}"
    );
    assert!(
        by_line > 0.09 && by_line < 0.11,
        "about a tenth, got {by_line}"
    );
}

#[test]
fn a_gate_boxed_on_the_other_axis_takes_less_than_the_line() {
    // The case the two columns exist to expose: the line finds the events, the
    // gate throws them away because its other axis does not reach them. In the
    // report that shows up as a capture far below the line's figure, which is
    // the only signal that separates a misplaced gate from a mis-read axis.
    use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
    use crate::gate_editor::plots::plot_store::EventIndexMapped;
    use crate::gate_rules::autogate::{admitted_by, beyond_the_line};
    use polars::prelude::*;

    let xs: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
    // Every event sits at y = 500.
    let frame = Arc::new(df![X => xs.clone(), Y => vec![500.0f32; 1000]].unwrap());
    let index = EventIndexMapped {
        event_index: get_event_mask_from_scaled_df(frame, Arc::from(X), Arc::from(Y)).unwrap(),
        index_map: Arc::new((0..1000).collect()),
    };
    let values: Vec<f64> = xs.iter().map(|v| *v as f64).collect();

    // Same threshold, but the gate only spans y 0..100 - well below the data.
    let geometry = create_rectangle_geometry(
        vec![(900.0, 0.0), (1e16, 0.0), (1e16, 100.0), (900.0, 100.0)],
        X,
        Y,
    )
    .unwrap();
    let gate: Arc<dyn DrawableGate> =
        Arc::new(RectangleGate::try_new(gate("boxed", geometry), true).unwrap());

    let by_gate = admitted_by(&gate, &index).unwrap();
    let by_line = beyond_the_line(&values, Bound::Above, 900.0);
    assert_eq!(by_gate, 0.0, "the gate reaches none of them");
    assert!(by_line > 0.09, "but the line finds them all: {by_line}");
}

#[test]
fn a_below_gate_counts_the_other_side_of_the_line() {
    use crate::gate_rules::autogate::beyond_the_line;
    let values: Vec<f64> = (1..=100).map(|i| i as f64).collect();
    assert!((beyond_the_line(&values, Bound::Below, 10.0) - 0.09).abs() < 1e-9);
    assert!((beyond_the_line(&values, Bound::Above, 10.0) - 0.90).abs() < 1e-9);
    assert_eq!(beyond_the_line(&[], Bound::Above, 10.0), 0.0);
}

// ─── which of a specimen's files is the one gated ────────────────────────────

/// The same two files as `fs_and_fmx`, measured with the FMO first - which is
/// what path order gives when the control's well sorts ahead of the stain's.
fn sweep_fmo_first(
    state: &mut crate::gate_editor::gates::GateState,
    store: &crate::gate_rules::rule_store::RuleStore,
    map: &crate::omiq::metadata::MetaDataFileMap,
    fmx: &polars::prelude::DataFrame,
    fs: &polars::prelude::DataFrame,
) -> crate::gate_rules::autogate::Report {
    use crate::gate_rules::autogate::{measure_file, position_all};

    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in [("fmx_a", fmx), ("fs_a", fs)] {
        let (m, u) = measure_file(state, &Arc::from(file), frame, map, store).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    position_all(state, store, &measured, &unmeasured, map)
}

#[test]
fn a_specimen_is_read_from_its_full_stain_not_whichever_file_sorted_first() {
    // The control and the stain share a gate position but not a population.
    // Reading the FMO's meant a gate holding 6.24% of the stain was reported as
    // 0.016%, and - worse than the report being wrong - the negative was read
    // and the gate placed off the control too.
    use crate::gate_rules::rule_store::MeasuredOn;
    use polars::prelude::*;

    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    // The FMO has nothing above the gate; the full stain has a tenth of its
    // events there. Whichever file the run reads is unmistakable in the result.
    let fmx = {
        let xs: Vec<f32> = (1..=1000).map(|i| i as f32 * 0.1).collect();
        df![X => xs, Y => vec![0.0f32; 1000]].unwrap()
    };
    let fs = ramp(1000);

    let mut store = crate::gate_rules::rule_store::RuleStore::default();
    store.insert(
        crate::gate_rules::rule_store::RuleTarget::named("CD134+"),
        crate::gate_rules::rule_store::GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: crate::gate_rules::rule::Rule::AboveTheNegative(
                crate::gate_rules::rule::AboveTheNegativeRule::default(),
            ),
        },
    );

    let report = sweep_fmo_first(&mut state, &store, &map, &fmx, &fs);
    let placed = report.positioned.first().expect("positioned");
    assert_eq!(
        &*placed.file, "fs_a",
        "the answer belongs to the full stain, not the control that sorted first"
    );
    assert_eq!(&*placed.captured_on, "fs_a");
}

#[test]
fn the_display_order_says_which_file_is_the_stained_one() {
    // The FMO on the left where the line is set, the full stain on the right
    // where the positives are read off - so the later type is the gated one.
    use crate::gate_rules::autogate::gated_rank;
    use crate::gate_rules::rule_store::SamplePairing;

    let pairing = SamplePairing::default();
    let map = fs_and_fmx();
    assert!(
        gated_rank(&pairing, &Arc::from("fs_a"), &map)
            > gated_rank(&pairing, &Arc::from("fmx_a"), &map),
        "FS is last in the default order, so it outranks FMX"
    );
    // A file the metadata says nothing about ranks below both.
    assert_eq!(gated_rank(&pairing, &Arc::from("nobody"), &map), 0);
}

#[test]
fn reversing_the_display_order_reverses_which_file_is_gated() {
    // It is configuration, not a guess about the strings "FS" and "FMX" - a
    // dataset naming them the other way round has to work.
    use crate::gate_rules::autogate::gated_rank;
    use crate::gate_rules::rule_store::SamplePairing;

    let pairing = SamplePairing {
        display_order: vec![Arc::from("FS"), Arc::from("FMX")],
        ..SamplePairing::default()
    };
    let map = fs_and_fmx();
    assert!(
        gated_rank(&pairing, &Arc::from("fmx_a"), &map)
            > gated_rank(&pairing, &Arc::from("fs_a"), &map)
    );
}

// ─── placing in the valley ───────────────────────────────────────────────────

/// Two populations: a negative at `negative`, a positive 3.0 above it, with a
/// real dip between. `merge` moves the positive down onto the negative until
/// the dip fills in.
fn two_populations(negative: f32, merge: f32) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rand_distr::{Distribution, Normal};

    let mut rng = StdRng::seed_from_u64(9);
    let neg = Normal::new(negative, 40.0).unwrap();
    let pos = Normal::new(negative + 300.0 - merge, 45.0).unwrap();
    let mut xs: Vec<f32> = (0..14_000).map(|_| neg.sample(&mut rng)).collect();
    xs.extend((0..6_000).map(|_| pos.sample(&mut rng)));
    let n = xs.len();
    df![X => xs, Y => vec![0.0f32; n]].unwrap()
}

fn valley_rule(file: &str, min_depth: f64) -> crate::gate_rules::rule_store::RuleStore {
    use crate::gate_rules::rule::{Rule, ValleyRule};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};

    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("CD134+"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from(file)),
            rule: Rule::InTheValley(ValleyRule {
                min_depth_fraction: min_depth,
                ..ValleyRule::default()
            }),
        },
    );
    store
}

fn sweep_frames(
    state: &mut crate::gate_editor::gates::GateState,
    store: &crate::gate_rules::rule_store::RuleStore,
    map: &crate::omiq::metadata::MetaDataFileMap,
    frames: &[(&str, &polars::prelude::DataFrame)],
) -> crate::gate_rules::autogate::Report {
    use crate::gate_rules::autogate::{measure_file, position_all};

    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in frames {
        let (m, u) = measure_file(state, &Arc::from(*file), frame, map, store).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    position_all(state, store, &measured, &unmeasured, map)
}

#[test]
fn the_gate_follows_the_valley_when_the_populations_move() {
    // Both populations shifted up by 150. The dip moves with them, and so
    // should the gate - by the same amount, since nothing else changed.
    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let here = two_populations(400.0, 0.0);
    let shifted = two_populations(550.0, 0.0);

    let report = sweep_frames(
        &mut state,
        &valley_rule("fs_qc", 0.25),
        &map,
        &[("fs_qc", &here), ("fs_b", &shifted)],
    );

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("positioned");
    let (reference, sample) = placed.valley.expect("the reading is reported");
    assert!(
        (sample.bottom - reference.bottom - 150.0).abs() < 25.0,
        "the dip moved 150; read {} against {}",
        sample.bottom,
        reference.bottom
    );
    assert!(
        (placed.to - 150.0 - reference.at).abs() < 25.0,
        "so should the gate: {} against {}",
        placed.to,
        reference.at
    );
}

#[test]
fn a_wider_positive_population_does_not_carry_the_gate_away() {
    // The EOMES failure, as a test. Pacing out a fixed number of the negative's
    // widths let a negative measured 2.47x too wide throw the gate six widths
    // past where it belonged. Reading the dip cannot do that, because there is
    // no width being multiplied.
    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let here = two_populations(400.0, 0.0);
    // Populations closer together: the dip is shallower and sits lower, but it
    // is still a dip and the gate should follow it - not fly outwards.
    let closer = two_populations(400.0, 120.0);

    let report = sweep_frames(
        &mut state,
        &valley_rule("fs_qc", 0.05),
        &map,
        &[("fs_qc", &here), ("fs_b", &closer)],
    );

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("positioned");
    let (reference, sample) = placed.valley.expect("reported");
    assert!(
        sample.depth < reference.depth,
        "the dip should be shallower: {} against {}",
        sample.depth,
        reference.depth
    );
    assert!(
        placed.to < reference.at,
        "and the gate should move inwards with it, not outwards: {} against {}",
        placed.to,
        reference.at
    );
}

#[test]
fn merged_populations_are_refused_rather_than_guessed() {
    // When the two have run together there is no boundary, and a rule that
    // reads boundaries should say so. Placing something plausible-looking is
    // how a gate holding 35% came back holding 0.07%.
    let (mut state, gate_id) = one_positive_gate();
    let map = two_specimens();
    let here = two_populations(400.0, 0.0);
    let merged = two_populations(400.0, 300.0);

    let report = sweep_frames(
        &mut state,
        &valley_rule("fs_qc", 0.25),
        &map,
        &[("fs_qc", &here), ("fs_b", &merged)],
    );

    assert!(
        report.positioned.iter().all(|p| &*p.specimen != "DONOR-B"),
        "nothing should be placed where there is no boundary"
    );
    let said = report
        .skipped
        .iter()
        .find(|s| &*s.file == "fs_b")
        .expect("and it should say why");
    assert!(
        said.reason.contains("not a boundary")
            || said.reason.contains("merged")
            || said.reason.contains("no population"),
        "and say what it saw, not just that it failed: {}",
        said.reason
    );
    // The gate is left exactly where it was rather than moved somewhere wrong.
    let untouched = state
        .gate_for_file(&gate_id, &Arc::from("fs_b"), &map)
        .unwrap();
    assert_eq!(edges(&untouched, X).0, 500.0);
}

#[test]
fn a_shallower_valley_within_tolerance_is_still_placed() {
    // The other half: shallower is not the same as absent. A dip a third as
    // deep still has a lowest point and the gate still belongs there.
    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let here = two_populations(400.0, 0.0);
    let shallower = two_populations(400.0, 130.0);

    let report = sweep_frames(
        &mut state,
        &valley_rule("fs_qc", 0.05),
        &map,
        &[("fs_qc", &here), ("fs_b", &shallower)],
    );
    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("a shallower dip should still be gated");
    let (_, sample) = placed.valley.unwrap();
    assert!(sample.depth > 0.0);
}

#[test]
fn the_offset_from_the_bottom_is_carried_across() {
    // If a person gates a little to one side of the true bottom, that is their
    // judgement and it should be reproduced, not corrected to the centre.
    use crate::gate_rules::rule::ValleyRule;
    let rule = ValleyRule::default();
    let mut v: Vec<f64> = Vec::new();
    {
        use rand::SeedableRng;
        use rand::rngs::StdRng;
        use rand_distr::{Distribution, Normal};
        let mut rng = StdRng::seed_from_u64(11);
        let neg = Normal::new(0.0, 0.4).unwrap();
        let pos = Normal::new(3.0, 0.5).unwrap();
        v.extend((0..12_000).map(|_| neg.sample(&mut rng)));
        v.extend((0..5_000).map(|_| pos.sample(&mut rng)));
    }
    let bottom = rule.calibrate(&v, 0.0).unwrap().bottom;
    // A gate drawn 0.4 to the right of the bottom.
    let cal = rule.calibrate(&v, bottom + 0.4).unwrap();
    assert!((cal.offset - 0.4).abs() < 1e-9, "offset {}", cal.offset);
    let back = rule.place(&v, cal.offset).unwrap();
    assert!(
        (back.at - (bottom + 0.4)).abs() < 1e-9,
        "applied back to its own sample it reproduces the gate: {}",
        back.at
    );
}

#[test]
fn a_shallow_valley_is_placed_and_flagged_rather_than_refused() {
    // Refusing hid the answer exactly where a person most wanted it: a run came
    // back with seven samples unplaced at depths of 5% to 24% against a
    // threshold of 25%, one short by a single point, and the only way to see
    // where the gate would have gone was to change the setting and run again.
    use crate::gate_rules::confidence::VALLEY;

    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let here = two_populations(400.0, 0.0);
    let shallow = two_populations(400.0, 130.0);

    // A threshold the sample's dip does not meet.
    let report = sweep_frames(
        &mut state,
        &valley_rule("fs_qc", 0.9),
        &map,
        &[("fs_qc", &here), ("fs_b", &shallow)],
    );

    let placed = report
        .positioned
        .iter()
        .find(|p| &*p.specimen == "DONOR-B")
        .expect("a shallow dip is still placed");
    let (reference, sample) = placed.valley.unwrap();
    assert!(
        sample.depth < reference.depth * 0.9,
        "shallower than the bar"
    );
    assert_eq!(
        placed.weakest,
        Some(VALLEY),
        "and the shallow dip is what holds its score down"
    );
    assert!(
        placed.confidence < 1.0,
        "scored on how deep the dip was: {}",
        placed.confidence
    );
}

/// BUG (docs/test-audit.md, B-RULE-1): `ValleyRule::min_depth_fraction` is
/// documented as how shallow a sample's dip may be, against the reference's,
/// before the placement is flagged - and nothing reads it. The Gate Rules tab
/// edits it and the rules file saves it, but the depth component is added
/// the same way whatever it says, so a dip well inside a lenient bar is
/// flagged exactly as hard as one far outside a strict one.
#[test]
#[ignore = "known bug B-RULE-1: min_depth_fraction has no effect"]
fn a_dip_inside_the_bar_is_judged_more_kindly_than_one_outside_it() {
    let map = two_specimens();
    let here = two_populations(400.0, 0.0);
    let shallow = two_populations(400.0, 130.0);
    let confidence = |bar: f64| {
        let (mut state, _) = one_positive_gate();
        let report = sweep_frames(
            &mut state,
            &valley_rule("fs_qc", bar),
            &map,
            &[("fs_qc", &here), ("fs_b", &shallow)],
        );
        let placed = report
            .positioned
            .iter()
            .find(|p| &*p.specimen == "DONOR-B")
            .expect("placed");
        (placed.confidence, placed.weakest)
    };
    let (strict, strict_weakest) = confidence(0.9);
    let (lenient, lenient_weakest) = confidence(0.05);
    assert_eq!(strict_weakest, Some(crate::gate_rules::confidence::VALLEY));
    assert!(
        lenient > strict || lenient_weakest != strict_weakest,
        "the bar changed nothing: {strict} at 0.9, {lenient} at 0.05"
    );
}

#[test]
fn a_density_with_no_dip_at_all_is_still_refused() {
    // The other side: flagging is for a dip that is shallow, not for one that
    // is absent. With nothing to place against there is nothing to place.
    let (mut state, _) = one_positive_gate();
    let map = two_specimens();
    let here = two_populations(400.0, 0.0);
    let merged = two_populations(400.0, 300.0);

    let report = sweep_frames(
        &mut state,
        &valley_rule("fs_qc", 0.0),
        &map,
        &[("fs_qc", &here), ("fs_b", &merged)],
    );
    assert!(report.positioned.iter().all(|p| &*p.specimen != "DONOR-B"));
    assert!(report.skipped.iter().any(|s| &*s.file == "fs_b"));
}

#[test]
fn the_report_follows_the_pairing_s_sample_order() {
    // Two tabs disagreeing about the order of the same specimens is worse than
    // either order, so the report follows the column the plots are sorted by.
    use crate::gate_rules::autogate::{measure_file, position_all};
    use crate::gate_rules::rule_store::SamplePairing;

    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    // Deliberately measured in an order that is not the sorted one.
    for (file, id, day) in [
        ("fs_qc", "QC-A", "D1"),
        ("fs_c", "DONOR-C", "D85"),
        ("fs_b", "DONOR-B", "D4"),
    ] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(id));
        columns.insert(Arc::from("SampleType"), Arc::from("FS"));
        columns.insert(Arc::from("Day"), Arc::from(day));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }

    let mut store = above_the_negative_rule("fs_qc");
    store.pairing = SamplePairing {
        sort_column: Some(Arc::from("Day")),
        ..SamplePairing::default()
    };

    let (mut state, _) = one_positive_gate();
    let frames = [
        ("fs_qc", with_negative_at(300.0)),
        ("fs_c", with_negative_at(500.0)),
        ("fs_b", with_negative_at(400.0)),
    ];
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in &frames {
        let (m, u) = measure_file(&state, &Arc::from(*file), frame, &map, &store).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let report = position_all(&mut state, &store, &measured, &unmeasured, &map);

    let order: Vec<&str> = report.positioned.iter().map(|p| &*p.specimen).collect();
    assert_eq!(
        order,
        ["DONOR-B", "DONOR-C"],
        "D4 before D85, not the order they were measured in"
    );
}

#[test]
fn the_valley_rule_gives_each_specimen_its_own_position_and_badge() {
    // Two things reported broken together after a run: every gate at the same
    // position, and no group badge in the hierarchy. Both would follow from the
    // placements never reaching the store, so both are checked here in one go.
    let (mut state, gate_id) = one_positive_gate();
    let map = {
        let mut map = im::HashMap::with_hasher(FxBuildHasher);
        for (file, id) in [("fs_qc", "QC-A"), ("fs_b", "DONOR-B"), ("fs_c", "DONOR-C")] {
            let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
            columns.insert(Arc::from("SampleID"), Arc::from(id));
            columns.insert(Arc::from("SampleType"), Arc::from("FS"));
            map.insert(Arc::from(file) as Arc<str>, columns);
        }
        map
    };
    let qc = two_populations(400.0, 0.0);
    let b = two_populations(480.0, 0.0);
    let c = two_populations(330.0, 0.0);

    let report = sweep_frames(
        &mut state,
        &valley_rule("fs_qc", 0.25),
        &map,
        &[("fs_qc", &qc), ("fs_b", &b), ("fs_c", &c)],
    );
    assert_eq!(report.positioned.len(), 2, "{:?}", report.skipped.len());

    // Each specimen's gate resolves to its own place, not one shared position.
    let at = |file: &str| {
        edges(
            &state
                .gate_for_file(&gate_id, &Arc::from(file), &map)
                .unwrap(),
            X,
        )
        .0
    };
    assert_ne!(at("fs_b"), at("fs_c"), "two specimens, two positions");
    assert_ne!(at("fs_b"), 500.0, "and not the global one either");

    // And the store reports the gate as group-overridden, which is what the
    // hierarchy's G badge reads.
    let (groups, samples) = state.overridden_ids();
    assert!(groups.contains(&gate_id), "the badge's source says group");
    assert!(!samples.contains(&gate_id), "not per sample");
}

#[test]
fn a_run_that_cannot_tell_a_stain_from_its_fmo_says_so() {
    // It fails quietly and totally: with no sample type every rule reads
    // whichever file of a specimen sorts first, and an FMO has no positive
    // population to gate against. A whole run came back unpositioned that way
    // with not one message explaining it.
    use crate::gate_rules::rule_store::SamplePairing;

    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for file in ["fs_qc", "fs_b"] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(file));
        // A type the display order does not name - which is the same as none.
        columns.insert(Arc::from("SampleType"), Arc::from("Unstained"));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    let mut store = above_the_negative_rule("fs_qc");
    store.pairing = SamplePairing::default();

    let (mut state, _) = one_positive_gate();
    let report = sweep_over(&mut state, &store, &map, &["fs_qc", "fs_b"]);

    let warned = report
        .skipped
        .iter()
        .find(|s| s.reason.contains("full stain cannot be told from its FMO"))
        .expect("the run should say it could not tell them apart");
    assert!(warned.reason.contains("SampleType"), "{}", warned.reason);
    assert!(warned.reason.contains("FMX"), "{}", warned.reason);
}

#[test]
fn a_run_that_can_tell_them_apart_stays_quiet_about_it() {
    let (mut state, _) = one_positive_gate();
    let map = fs_and_fmx();
    let report = sweep(&mut state, &fmx_rule(), &map);
    assert!(
        !report
            .skipped
            .iter()
            .any(|s| s.reason.contains("cannot be told")),
        "no warning when the types resolve"
    );
}

// ─── how a positioned gate is written back ───────────────────────────────────

#[test]
fn every_file_of_a_specimen_gets_the_one_position() {
    // The FMO and the full stain must never hold different positions: the FMO
    // is where the line is set and the stain is where it is read off, and a
    // line set on one and read on the other is the entire point.
    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    sweep(&mut state, &fmx_rule(), &map);

    let fmx = state
        .gate_for_file(&gate_id, &Arc::from("fmx_a"), &map)
        .unwrap();
    let fs = state
        .gate_for_file(&gate_id, &Arc::from("fs_a"), &map)
        .unwrap();
    assert_eq!(
        edges(&fmx, X),
        edges(&fs, X),
        "one specimen, one position, whichever of its files is asked"
    );
}

#[test]
fn a_positioned_gate_names_the_column_its_grouping_follows() {
    // Omiq stores one filter per file whatever drives it, and names the
    // grouping column separately. Without that name the positions read as
    // per-sample even though every file of a specimen holds the same one - so
    // the same run came back group-specific for containers that happened to
    // carry the name already and sample-specific for the rest.
    let (mut state, gate_id) = one_positive_gate();
    let map = fs_and_fmx();
    assert_eq!(
        state.group_override_column(&gate_id),
        None,
        "nothing to name before the run"
    );

    sweep(&mut state, &fmx_rule(), &map);

    assert_eq!(
        state.group_override_column(&gate_id).as_deref(),
        Some("SampleID"),
        "the column the pairing groups by"
    );
}

#[test]
fn renaming_the_grouping_column_follows_through_to_the_export() {
    // It reports the column actually used, not a default - a dataset grouping
    // on its own column has to export as grouped on that column.
    use crate::gate_rules::rule_store::SamplePairing;

    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, kind) in [("fs_a", "FS"), ("fmx_a", "FMX")] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("Donor_Day"), Arc::from("000602_D4"));
        columns.insert(Arc::from("Type"), Arc::from(kind));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    let mut store = fmx_rule();
    store.pairing = SamplePairing {
        sample_id_column: Arc::from("Donor_Day"),
        sample_type_column: Arc::from("Type"),
        ..SamplePairing::default()
    };

    let (mut state, gate_id) = one_positive_gate();
    sweep(&mut state, &store, &map);
    assert_eq!(
        state.group_override_column(&gate_id).as_deref(),
        Some("Donor_Day")
    );
}

// ── matching a phenotype ─────────────────────────────────────────────────

/// Deterministic scatter for the end-to-end phenotype tests.
struct Spread(u64);

impl Spread {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((self.0 >> 32) as u32) as f32 / u32::MAX as f32
    }
    fn about(&mut self, centre: f32, spread: f32) -> f32 {
        centre + (self.unit() + self.unit() - 1.0) * spread
    }
}

/// A frame with a background and one population, on two axes plus a marker.
///
/// `population` says where those cells sit: their x, y and marker value. The
/// background sits at the origin and is dim for the marker, so the population
/// is identifiable by the marker whatever the axes do.
fn panel(
    seed: u64,
    background: usize,
    members: usize,
    population: (f32, f32, f32),
) -> polars::prelude::DataFrame {
    use polars::prelude::*;
    let mut rng = Spread(seed);
    let mut xs = Vec::with_capacity(background + members);
    let mut ys = Vec::with_capacity(background + members);
    let mut marker = Vec::with_capacity(background + members);
    for _ in 0..background {
        xs.push(rng.about(200.0, 60.0));
        ys.push(rng.about(200.0, 60.0));
        marker.push(rng.about(100.0, 30.0));
    }
    for _ in 0..members {
        xs.push(rng.about(population.0, 25.0));
        ys.push(rng.about(population.1, 25.0));
        marker.push(rng.about(population.2, 20.0));
    }
    df![X => xs, Y => ys, "CD161" => marker].unwrap()
}

/// A state with one rectangle drawn round `(cx, cy)`, named so a rule can
/// find it.
fn gate_around(cx: f32, cy: f32, half: f32) -> (crate::gate_editor::gates::GateState, Arc<str>) {
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
            Some(crate::gate_editor::gates::gate_store::ROOTGATE.clone()),
            PrimaryGateType::Rectangle,
            Some("MAIT".to_string()),
        )
        .expect("a rectangle can be added at the root");
    let gate_id = state
        .placements()
        .next()
        .map(|(_, p)| p.gate_id.clone())
        .expect("adding a gate leaves a placement");

    let geometry = create_rectangle_geometry(
        vec![
            (cx - half, cy - half),
            (cx + half, cy - half),
            (cx + half, cy + half),
            (cx - half, cy + half),
        ],
        X,
        Y,
    )
    .unwrap();
    let mut inner = gate(&gate_id, geometry);
    inner.name = "MAIT".to_string();
    let drawn: Arc<dyn DrawableGate> = Arc::new(RectangleGate::try_new(inner, true).unwrap());
    state.place_gate(&[gate_id.clone()], &drawn, &GateSource::Global);
    (state, gate_id)
}

fn phenotype_rule(
    fit: crate::gate_rules::rule::ShapeFit,
    markers: &[&str],
) -> crate::gate_rules::rule_store::RuleStore {
    use crate::gate_rules::rule::{PhenotypeRule, Rule};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};

    let mut store = RuleStore::default();
    store.insert(
        RuleTarget::named("MAIT"),
        GateRule {
            // Neither is read by this rule; they are on every rule, and a
            // phenotype rule ignores them.
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from("fs_qc")),
            rule: Rule::MatchThePhenotype(PhenotypeRule {
                markers: markers.iter().map(|m| Arc::from(*m)).collect(),
                fit,
                ..Default::default()
            }),
        },
    );
    store
}

/// Run a phenotype rule over a reference and one sample whose population sits
/// at `moved_to`, and give back the report and the state.
fn match_run(
    fit: crate::gate_rules::rule::ShapeFit,
    moved_to: (f32, f32, f32),
) -> (
    crate::gate_rules::autogate::Report,
    crate::gate_editor::gates::GateState,
    Arc<str>,
) {
    use crate::gate_rules::autogate::{measure_file, position_all};

    // The reference: the population sits at (700, 700) and is bright.
    let (mut state, gate_id) = gate_around(700.0, 700.0, 80.0);
    let map = two_specimens();
    let rules = phenotype_rule(fit, &["CD161"]);

    let reference = panel(1, 1800, 200, (700.0, 700.0, 800.0));
    let sample = panel(2, 1800, 200, moved_to);

    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (file, frame) in [("fs_qc", &reference), ("fs_b", &sample)] {
        let (m, u) = measure_file(&state, &Arc::from(file), frame, &map, &rules).unwrap();
        measured.extend(m);
        unmeasured.extend(u);
    }
    let report = position_all(&mut state, &rules, &measured, &unmeasured, &map);
    (report, state, gate_id)
}

/// The centre of a gate on the two axes, whatever shape it is.
fn centre_of(gate: &Arc<dyn DrawableGate>) -> (f32, f32) {
    let inner = gate.get_gate_ref(None).unwrap();
    match &inner.geometry {
        GateGeometry::Rectangle { min, max } => (
            (min.get_coordinate(X).unwrap() + max.get_coordinate(X).unwrap()) / 2.0,
            (min.get_coordinate(Y).unwrap() + max.get_coordinate(Y).unwrap()) / 2.0,
        ),
        GateGeometry::Polygon { nodes, .. } => {
            let n = nodes.len() as f32;
            (
                nodes
                    .iter()
                    .map(|v| v.get_coordinate(X).unwrap())
                    .sum::<f32>()
                    / n,
                nodes
                    .iter()
                    .map(|v| v.get_coordinate(Y).unwrap())
                    .sum::<f32>()
                    / n,
            )
        }
        other => panic!("unexpected geometry {other:?}"),
    }
}

#[test]
fn keeping_the_shape_carries_the_gate_onto_the_moved_population() {
    use crate::gate_rules::rule::ShapeFit;
    // The same cells, bright for CD161 as before, but sitting somewhere else
    // on the two axes the gate is drawn on. Nothing about the axes identifies
    // them - only the marker does.
    let (report, state, gate_id) = match_run(ShapeFit::KeepShape, (300.0, 650.0, 800.0));

    assert_eq!(
        report.positioned.len(),
        1,
        "{:?}",
        report.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>()
    );
    let placed = state
        .gate_for_file(&gate_id, &Arc::from("fs_b"), &two_specimens())
        .expect("the sample has a gate");
    let (cx, cy) = centre_of(&placed);
    assert!(
        (cx - 300.0).abs() < 60.0 && (cy - 650.0).abs() < 60.0,
        "the gate centred on ({cx:.0}, {cy:.0}), the cells are at (300, 650)"
    );
}

#[test]
fn keeping_the_shape_keeps_the_gate_a_rectangle() {
    use crate::gate_rules::rule::ShapeFit;
    let (_, state, gate_id) = match_run(ShapeFit::KeepShape, (300.0, 650.0, 800.0));
    let placed = state
        .gate_for_file(&gate_id, &Arc::from("fs_b"), &two_specimens())
        .expect("the sample has a gate");
    assert!(
        matches!(
            placed.get_gate_ref(None).unwrap().geometry,
            GateGeometry::Rectangle { .. }
        ),
        "a rectangle must stay a rectangle"
    );
}

#[test]
fn drawing_a_polygon_replaces_the_rectangle_with_one() {
    use crate::gate_rules::rule::ShapeFit;
    let (report, state, gate_id) = match_run(ShapeFit::DrawPolygon, (300.0, 650.0, 800.0));
    assert_eq!(
        report.positioned.len(),
        1,
        "{:?}",
        report.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>()
    );
    let placed = state
        .gate_for_file(&gate_id, &Arc::from("fs_b"), &two_specimens())
        .expect("the sample has a gate");
    let GateGeometry::Polygon { nodes, .. } = &placed.get_gate_ref(None).unwrap().geometry else {
        panic!("drawing a polygon should have produced one");
    };
    assert!(nodes.len() >= 3, "a polygon needs at least three points");
    let (cx, cy) = centre_of(&placed);
    assert!(
        (cx - 300.0).abs() < 80.0 && (cy - 650.0).abs() < 80.0,
        "the polygon centred on ({cx:.0}, {cy:.0}), the cells are at (300, 650)"
    );
}

#[test]
fn the_reference_specimen_is_left_where_it_was_drawn() {
    use crate::gate_rules::rule::ShapeFit;
    let (report, _, _) = match_run(ShapeFit::KeepShape, (300.0, 650.0, 800.0));
    assert_eq!(
        report.reference.len(),
        1,
        "the sample the rule calibrates from is reported, not moved"
    );
}

#[test]
fn the_report_says_what_was_matched_and_where_it_sat() {
    use crate::gate_rules::rule::ShapeFit;
    let (report, _, _) = match_run(ShapeFit::KeepShape, (300.0, 650.0, 800.0));
    let read = report.positioned[0]
        .phenotype
        .as_ref()
        .expect("a phenotype rule reports what it matched");

    assert_eq!(read.markers.len(), 1);
    assert!(read.matched > 100, "matched only {}", read.matched);
    assert!(
        read.matched < 500,
        "matched {} - it has taken in background",
        read.matched
    );
    // The marker's centre should read about the same on both, since it is the
    // same population - that is the check that these are the same cells.
    let (_, there, here) = &read.centres[0];
    assert!(
        (there - here).abs() < 3.0,
        "CD161 read {there:.1} on the reference and {here:.1} here"
    );
    assert!(read.purity > 0.5, "purity {:.2}", read.purity);
}

#[test]
fn a_sample_missing_the_population_is_refused_rather_than_gated_on_the_nearest_cells() {
    use crate::gate_rules::rule::ShapeFit;
    // The population is simply not there: the "moved" cells are dim, like the
    // background. A gate placed round whatever was nearest would be worse than
    // no gate, because nothing downstream would say so.
    let (report, _, _) = match_run(ShapeFit::KeepShape, (300.0, 650.0, 100.0));
    assert!(
        report.positioned.is_empty(),
        "it placed a gate on a sample with no such population"
    );
    assert!(
        report.skipped.iter().any(|s| s.reason.contains("match")),
        "the report should say why: {:?}",
        report.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>()
    );
}

#[test]
fn a_rule_naming_a_marker_the_file_does_not_have_says_so() {
    use crate::gate_rules::autogate::measure_file;
    use crate::gate_rules::rule::ShapeFit;
    let (state, _) = gate_around(700.0, 700.0, 80.0);
    let map = two_specimens();
    let rules = phenotype_rule(ShapeFit::KeepShape, &["CD3", "CD4"]);
    let frame = panel(1, 500, 100, (700.0, 700.0, 800.0));
    let (measured, unmeasured) =
        measure_file(&state, &Arc::from("fs_qc"), &frame, &map, &rules).unwrap();
    assert!(measured.is_empty());
    assert_eq!(unmeasured.len(), 1);
    assert!(
        unmeasured[0].reason.contains("CD3"),
        "got: {}",
        unmeasured[0].reason
    );
}

#[test]
fn an_empty_marker_list_takes_the_whole_panel() {
    use crate::gate_rules::autogate::measure_file;
    use crate::gate_rules::rule::ShapeFit;
    let (state, _) = gate_around(700.0, 700.0, 80.0);
    let map = two_specimens();
    let rules = phenotype_rule(ShapeFit::KeepShape, &[]);
    let frame = panel(1, 500, 100, (700.0, 700.0, 800.0));
    let (measured, _) = measure_file(&state, &Arc::from("fs_qc"), &frame, &map, &rules).unwrap();
    let read = measured[0]
        .phenotype
        .as_ref()
        .expect("a phenotype rule is measured across markers");
    // Every f32 column: the two axes and the marker.
    assert_eq!(read.markers.len(), 3, "{:?}", read.markers);
}

#[test]
fn a_phenotype_rule_measured_on_a_partner_is_refused() {
    use crate::gate_rules::autogate::measure_file;
    use crate::gate_rules::rule::{PhenotypeRule, Rule, ShapeFit};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};
    // A partner resolves per specimen and can be a control. An FMO has no
    // signal in the channel it drops - usually the very marker the population
    // is defined by - so the phenotype would be described from cells that
    // cannot show it, and the answer would look like an answer.
    let (state, _) = gate_around(700.0, 700.0, 80.0);
    let map = two_specimens();
    let mut rules = RuleStore::default();
    rules.insert(
        RuleTarget::named("MAIT"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Partner(Arc::from("FMX")),
            rule: Rule::MatchThePhenotype(PhenotypeRule {
                markers: vec![Arc::from("CD161")],
                fit: ShapeFit::KeepShape,
                ..Default::default()
            }),
        },
    );
    let frame = panel(1, 500, 100, (700.0, 700.0, 800.0));
    let (measured, unmeasured) =
        measure_file(&state, &Arc::from("fs_qc"), &frame, &map, &rules).unwrap();
    assert!(measured.is_empty(), "it went ahead anyway");
    assert_eq!(unmeasured.len(), 1);
    assert!(
        unmeasured[0].reason.contains("control"),
        "the reason should say why a partner is not good enough: {}",
        unmeasured[0].reason
    );
}

#[test]
fn a_phenotype_rule_measured_on_itself_is_refused() {
    use crate::gate_rules::autogate::measure_file;
    use crate::gate_rules::rule::{PhenotypeRule, Rule, ShapeFit};
    use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};
    // Circular: it would describe the population from the gate it is about to
    // move.
    let (state, _) = gate_around(700.0, 700.0, 80.0);
    let map = two_specimens();
    let mut rules = RuleStore::default();
    rules.insert(
        RuleTarget::named("MAIT"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: Rule::MatchThePhenotype(PhenotypeRule {
                markers: vec![Arc::from("CD161")],
                fit: ShapeFit::KeepShape,
                ..Default::default()
            }),
        },
    );
    let frame = panel(1, 500, 100, (700.0, 700.0, 800.0));
    let (measured, unmeasured) =
        measure_file(&state, &Arc::from("fs_qc"), &frame, &map, &rules).unwrap();
    assert!(measured.is_empty());
    assert!(unmeasured[0].reason.contains("the sample itself"));
}

#[test]
fn the_other_rules_still_take_a_partner() {
    // The guard is about this one rule, not a new restriction on the rest:
    // every threshold rule is calibrated from a partner and must stay that way.
    use crate::gate_rules::autogate::measure_file;
    let (state, _) = one_positive_gate();
    let frame = ramp(1000);
    let (measured, unmeasured) = measure_file(
        &state,
        &Arc::from("fmx_a"),
        &frame,
        &fs_and_fmx(),
        &fmx_rule(),
    )
    .unwrap();
    assert_eq!(
        measured.len(),
        1,
        "{:?}",
        unmeasured
            .iter()
            .map(|u| u.reason.clone())
            .collect::<Vec<String>>()
    );
}

// ── the band search, over random populations ─────────────────────────────

/// Random populations - one to three clusters, sometimes rounded to whole
/// numbers so values tie - and random bands that at least one whole number
/// of events can satisfy. Sliding a gate open to one side must land it in
/// the band, whichever side it keeps, and what it reports holding must be
/// what the gate, asked afresh, holds.
#[test]
fn the_band_search_lands_in_any_band_a_population_can_satisfy() {
    use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
    use crate::gate_editor::plots::plot_store::EventIndexMapped;
    use crate::gate_rules::autogate::{admitted_by, position_by_capture};
    use polars::prelude::*;
    use rand::prelude::*;
    use rand_distr::Normal;

    for seed in 0..120u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let n = rng.random_range(200..3_000);
        let clusters = rng.random_range(1..=3);
        let whole = rng.random_bool(0.3);
        let xs: Vec<f32> = (0..n)
            .map(|i| {
                let c = (i % clusters) as f32;
                let v = Normal::new(200.0 + c * 300.0, 40.0 + c * 20.0)
                    .unwrap()
                    .sample(&mut rng);
                if whole { v.round() } else { v }
            })
            .collect();
        let values: Vec<f64> = xs.iter().map(|v| *v as f64).collect();
        let frame = Arc::new(df![X => xs.clone(), Y => vec![0.0f32; n]].unwrap());
        let index = EventIndexMapped {
            event_index: get_event_mask_from_scaled_df(frame, Arc::from(X), Arc::from(Y)).unwrap(),
            index_map: Arc::new((0..n).collect()),
        };

        // A band wide enough to hold a whole number of events - a few
        // events' worth - somewhere between 0.5% and 60%.
        let lo = rng.random_range(0.005..0.6);
        let width = (4.0 / n as f64).max(rng.random_range(0.0..0.05));
        let band = (lo, (lo + width).min(1.0));

        // With whole-number values, a band can fall inside a tie: no edge
        // can separate exactly those events. Only bands some edge can reach
        // are asked for.
        let mut sorted = values.clone();
        sorted.sort_by(f64::total_cmp);
        // `k` events below an edge and `n - k` above it: reachable when the
        // edge can fall between the k-th and (k+1)-th values.
        let reachable = |keep_above: bool| {
            (0..=n).any(|k| {
                let separable = k == 0 || k == n || sorted[k - 1] < sorted[k];
                let kept = if keep_above { n - k } else { k };
                separable && (band.0..=band.1).contains(&(kept as f64 / n as f64))
            })
        };

        for (bound, gate) in [
            (Bound::Above, rect(500.0, -1e16, 1e16, 1e16)),
            (Bound::Below, rect(-1e16, -1e16, 500.0, 1e16)),
        ] {
            if !reachable(bound == Bound::Above) {
                continue;
            }
            let (moved, _, held) = position_by_capture(
                &gate, X, bound, &index, band, &values, 500.0,
            )
            .unwrap_or_else(|| panic!("seed {seed} {bound:?}: no position found for {band:?}"));
            let asked = admitted_by(&moved, &index).unwrap();
            assert!(
                (asked - held).abs() < 1e-9,
                "seed {seed} {bound:?}: reported {held}, the gate holds {asked}"
            );
            assert!(
                (band.0..=band.1).contains(&held),
                "seed {seed} {bound:?}: held {held}, band {band:?}, whole numbers {whole}"
            );
        }
    }
}
