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

    let frame = ramp(1000);
    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for file in ["fs_a", "fmx_a"] {
        let (m, u) = measure_file(state, &Arc::from(file), &frame, map, store).unwrap();
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
    assert_eq!(&*measured[0].parameter, X);
    assert_eq!(measured[0].current, 500.0);
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
