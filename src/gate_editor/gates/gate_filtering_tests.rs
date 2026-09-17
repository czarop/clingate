//! Tests for event filtering.
//!
//! This is where gates stop being geometry and start selecting events, so it is
//! the layer that decides every statistic the UI shows and every subset the
//! autogater will eventually measure a shift against.
//!
//! `GateOverrideResolver` has public fields, so a resolver can be assembled by
//! hand here without standing up a Dioxus store.
//!
//! cargo test gate_filtering -- --nocapture

#![cfg(test)]

use crate::gate_editor::gates::gate_filtering::filter_events_to_mask;
use crate::gate_editor::gates::gate_single::boolean_gates::BooleanGate;
use crate::gate_editor::gates::gate_single::ellipse_gate::EllipseGate;
use crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate;
use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_store::{ComparableGate, GateOverrideResolver, GateSource};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use flow_gates::{BooleanOperation, create_polygon_geometry, create_rectangle_geometry};
use polars::prelude::*;
use rustc_hash::FxBuildHasher;
use std::sync::Arc;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

// ─── Fixtures ─────────────────────────────────────────────────────────────────

/// A 5-event frame laid out so each gate below selects a known subset:
///
///   index:  0        1        2        3        4
///   x:      1.0      5.0      5.0      9.0      5.0
///   y:      1.0      5.0      1.0      9.0      9.0
fn events() -> DataFrame {
    df![
        X => [1.0f32, 5.0, 5.0, 9.0, 5.0],
        Y => [1.0f32, 5.0, 1.0, 9.0, 9.0],
    ]
    .unwrap()
}

fn gate(id: &str, geometry: flow_gates::GateGeometry) -> flow_gates::Gate {
    flow_gates::Gate {
        id: Arc::from(id),
        name: id.to_string(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    }
}

/// A rectangle spanning x and y in 4.0..=6.0 - selects only the centre event.
fn centre_rectangle(id: &str) -> Arc<dyn DrawableGate> {
    let geometry =
        create_rectangle_geometry(vec![(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)], X, Y)
            .unwrap();
    Arc::new(RectangleGate::try_new(gate(id, geometry), true).unwrap())
}

/// A tall rectangle spanning the full y range at x in 4.0..=6.0 - selects the
/// three events on the centre column.
fn centre_column(id: &str) -> Arc<dyn DrawableGate> {
    let geometry =
        create_rectangle_geometry(vec![(4.0, 0.0), (6.0, 0.0), (6.0, 10.0), (4.0, 10.0)], X, Y)
            .unwrap();
    Arc::new(RectangleGate::try_new(gate(id, geometry), true).unwrap())
}

/// A wide rectangle spanning the full x range at y in 4.0..=6.0 - selects only
/// the centre event of the three on the centre row.
fn centre_row(id: &str) -> Arc<dyn DrawableGate> {
    let geometry =
        create_rectangle_geometry(vec![(0.0, 4.0), (10.0, 4.0), (10.0, 6.0), (0.0, 6.0)], X, Y)
            .unwrap();
    Arc::new(RectangleGate::try_new(gate(id, geometry), true).unwrap())
}

fn resolver(gates: Vec<Arc<dyn DrawableGate>>) -> GateOverrideResolver {
    let mut active_gates = im::HashMap::with_hasher(FxBuildHasher);
    let mut gate_origins = im::HashMap::with_hasher(FxBuildHasher);

    for g in gates {
        let id = g.get_id();
        active_gates.insert(id.clone(), ComparableGate(g));
        gate_origins.insert(id, GateSource::Global);
    }

    GateOverrideResolver {
        active_gates,
        gate_origins,
    }
}

/// The indices the mask selects, for readable assertions.
fn selected(mask: &BooleanChunked) -> Vec<usize> {
    mask.into_iter()
        .enumerate()
        .filter_map(|(i, v)| if v == Some(true) { Some(i) } else { None })
        .collect()
}

fn mask_for(id: &str, r: &GateOverrideResolver) -> BooleanChunked {
    filter_events_to_mask(&events(), Arc::from(id), r).expect("gate resolves")
}

// ─── Rectangle ────────────────────────────────────────────────────────────────

#[test]
fn a_rectangle_selects_the_events_inside_it() {
    let r = resolver(vec![centre_rectangle("rect")]);
    assert_eq!(selected(&mask_for("rect", &r)), vec![1]);
}

#[test]
fn a_rectangle_spanning_an_axis_selects_the_whole_column() {
    let r = resolver(vec![centre_column("col")]);
    assert_eq!(selected(&mask_for("col", &r)), vec![1, 2, 4]);
}

/// The rectangle mask uses strict comparisons, so an event exactly on the edge
/// falls outside. Worth pinning: it decides whether counts match Omiq's.
#[test]
fn a_rectangle_excludes_events_exactly_on_its_boundary() {
    let geometry =
        create_rectangle_geometry(vec![(1.0, 1.0), (5.0, 1.0), (5.0, 5.0), (1.0, 5.0)], X, Y)
            .unwrap();
    let g: Arc<dyn DrawableGate> =
        Arc::new(RectangleGate::try_new(gate("edge", geometry), true).unwrap());
    let r = resolver(vec![g]);

    // Events 0 (1,1) and 1 (5,5) sit exactly on opposite corners.
    assert!(
        selected(&mask_for("edge", &r)).is_empty(),
        "boundary events are excluded"
    );
}

#[test]
fn a_rectangle_selecting_nothing_yields_an_empty_mask() {
    let geometry = create_rectangle_geometry(
        vec![
            (100.0, 100.0),
            (200.0, 100.0),
            (200.0, 200.0),
            (100.0, 200.0),
        ],
        X,
        Y,
    )
    .unwrap();
    let g: Arc<dyn DrawableGate> =
        Arc::new(RectangleGate::try_new(gate("far", geometry), true).unwrap());
    let r = resolver(vec![g]);

    assert!(selected(&mask_for("far", &r)).is_empty());
}

// ─── Polygon ──────────────────────────────────────────────────────────────────

#[test]
fn a_polygon_selects_the_events_inside_it() {
    // A diamond around the centre event.
    let geometry =
        create_polygon_geometry(vec![(5.0, 3.0), (7.0, 5.0), (5.0, 7.0), (3.0, 5.0)], X, Y)
            .unwrap();
    let g: Arc<dyn DrawableGate> =
        Arc::new(PolygonGate::try_new(gate("diamond", geometry), true).unwrap());
    let r = resolver(vec![g]);

    assert_eq!(selected(&mask_for("diamond", &r)), vec![1]);
}

#[test]
fn a_polygon_enclosing_everything_selects_every_event() {
    let geometry = create_polygon_geometry(
        vec![(-1.0, -1.0), (20.0, -1.0), (20.0, 20.0), (-1.0, 20.0)],
        X,
        Y,
    )
    .unwrap();
    let g: Arc<dyn DrawableGate> =
        Arc::new(PolygonGate::try_new(gate("all", geometry), true).unwrap());
    let r = resolver(vec![g]);

    assert_eq!(selected(&mask_for("all", &r)), vec![0, 1, 2, 3, 4]);
}

/// A concave polygon exercises the ray-casting crossing count rather than the
/// bounding-box fast path.
#[test]
fn a_concave_polygon_excludes_its_notch() {
    // An arrowhead whose notch swallows the centre point.
    let geometry = create_polygon_geometry(
        vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (5.0, 2.0),
            (0.0, 10.0),
        ],
        X,
        Y,
    )
    .unwrap();
    let g: Arc<dyn DrawableGate> =
        Arc::new(PolygonGate::try_new(gate("notch", geometry), true).unwrap());
    let r = resolver(vec![g]);

    let hits = selected(&mask_for("notch", &r));
    assert!(hits.contains(&0), "(1,1) is inside the body");
    assert!(!hits.contains(&1), "(5,5) sits in the notch");
    assert!(!hits.contains(&4), "(5,9) sits in the notch");
}

// ─── Ellipse ──────────────────────────────────────────────────────────────────

#[test]
fn an_ellipse_selects_the_events_inside_it() {
    let geometry = crate::omiq::deserialise::create_omiq_ellipse_geometry(
        (3.0, 5.0), // left
        (7.0, 5.0), // right
        (5.0, 7.0), // top
        X,
        Y,
    )
    .unwrap();
    let g: Arc<dyn DrawableGate> =
        Arc::new(EllipseGate::try_new(gate("ellipse", geometry), true).unwrap());
    let r = resolver(vec![g]);

    assert_eq!(selected(&mask_for("ellipse", &r)), vec![1]);
}

// ─── Boolean gates ────────────────────────────────────────────────────────────

fn boolean(id: &str, op: BooleanOperation, operands: Vec<&str>) -> Arc<dyn DrawableGate> {
    Arc::new(
        BooleanGate::new(
            Arc::from(id),
            id.to_string(),
            operands.into_iter().map(Arc::from).collect(),
            op,
            Arc::from(X),
            Arc::from(Y),
        )
        .unwrap(),
    )
}

#[test]
fn a_not_gate_inverts_its_operand() {
    let r = resolver(vec![
        centre_rectangle("rect"),
        boolean("not", BooleanOperation::Not, vec!["rect"]),
    ]);

    assert_eq!(selected(&mask_for("not", &r)), vec![0, 2, 3, 4]);
}

#[test]
fn an_and_gate_intersects_its_operands() {
    let r = resolver(vec![
        centre_column("col"),
        centre_row("row"),
        boolean("and", BooleanOperation::And, vec!["col", "row"]),
    ]);

    // The column holds 1, 2, 4; the row holds only 1.
    assert_eq!(selected(&mask_for("and", &r)), vec![1]);
}

#[test]
fn an_or_gate_unions_its_operands() {
    let r = resolver(vec![
        centre_rectangle("rect"),
        centre_row("row"),
        boolean("or", BooleanOperation::Or, vec!["rect", "row"]),
    ]);

    assert_eq!(selected(&mask_for("or", &r)), vec![1]);
}

#[test]
fn an_or_gate_over_disjoint_operands_keeps_both() {
    let geometry = create_rectangle_geometry(
        vec![(8.0, 8.0), (10.0, 8.0), (10.0, 10.0), (8.0, 10.0)],
        X,
        Y,
    )
    .unwrap();
    let corner: Arc<dyn DrawableGate> =
        Arc::new(RectangleGate::try_new(gate("corner", geometry), true).unwrap());

    let r = resolver(vec![
        centre_rectangle("rect"),
        corner,
        boolean("or", BooleanOperation::Or, vec!["rect", "corner"]),
    ]);

    assert_eq!(selected(&mask_for("or", &r)), vec![1, 3]);
}

/// Regression: the sidebar creates AND/OR gates with exactly one operand and the
/// user adds the rest later, but the filter rejected anything with fewer than
/// two, so every freshly created boolean gate errored the moment it was used as
/// a parent. One operand folds to that operand's own mask.
#[test]
fn a_single_operand_and_gate_folds_to_that_operand() {
    let r = resolver(vec![
        centre_rectangle("rect"),
        boolean("and", BooleanOperation::And, vec!["rect"]),
    ]);

    assert_eq!(
        selected(&mask_for("and", &r)),
        selected(&mask_for("rect", &r))
    );
}

#[test]
fn a_single_operand_or_gate_folds_to_that_operand() {
    let r = resolver(vec![
        centre_column("col"),
        boolean("or", BooleanOperation::Or, vec!["col"]),
    ]);

    assert_eq!(
        selected(&mask_for("or", &r)),
        selected(&mask_for("col", &r))
    );
}

#[test]
fn a_boolean_gate_with_no_operands_is_an_error() {
    for op in [BooleanOperation::And, BooleanOperation::Or] {
        let r = resolver(vec![boolean("empty", op, vec![])]);
        assert!(
            filter_events_to_mask(&events(), Arc::from("empty"), &r).is_err(),
            "an operand-less boolean gate cannot be evaluated"
        );
    }
}

#[test]
fn a_not_gate_requires_exactly_one_operand() {
    let r = resolver(vec![
        centre_rectangle("rect"),
        centre_row("row"),
        boolean("not", BooleanOperation::Not, vec!["rect", "row"]),
    ]);

    assert!(filter_events_to_mask(&events(), Arc::from("not"), &r).is_err());
}

#[test]
fn booleans_nest() {
    // NOT (col AND row) - everything except the centre event.
    let r = resolver(vec![
        centre_column("col"),
        centre_row("row"),
        boolean("and", BooleanOperation::And, vec!["col", "row"]),
        boolean("not", BooleanOperation::Not, vec!["and"]),
    ]);

    assert_eq!(selected(&mask_for("not", &r)), vec![0, 2, 3, 4]);
}

// ─── Resolution failures ──────────────────────────────────────────────────────

#[test]
fn an_unknown_gate_id_is_an_error() {
    let r = resolver(vec![centre_rectangle("rect")]);
    assert!(filter_events_to_mask(&events(), Arc::from("ghost"), &r).is_err());
}

#[test]
fn a_boolean_referencing_a_missing_operand_is_an_error() {
    let r = resolver(vec![boolean("or", BooleanOperation::Or, vec!["ghost"])]);
    assert!(filter_events_to_mask(&events(), Arc::from("or"), &r).is_err());
}

// ─── Hierarchy chains ─────────────────────────────────────────────────────────

#[test]
fn a_hierarchy_chain_narrows_at_each_step() {
    use crate::gate_editor::gates::gate_filtering::filter_events_by_hierarchy_to_mask;

    let r = resolver(vec![centre_column("col"), centre_row("row")]);
    let chain = vec![Arc::from("col"), Arc::from("row")];

    let mask = filter_events_by_hierarchy_to_mask(&events(), &chain, &r).unwrap();

    // The column alone holds 1, 2 and 4; adding the row leaves only 1.
    assert_eq!(selected(&mask), vec![1]);
}

#[test]
fn an_empty_chain_selects_every_event() {
    use crate::gate_editor::gates::gate_filtering::filter_events_by_hierarchy_to_mask;

    let r = resolver(vec![]);
    let mask = filter_events_by_hierarchy_to_mask(&events(), &[], &r).unwrap();

    assert_eq!(selected(&mask), vec![0, 1, 2, 3, 4]);
}

#[test]
fn a_chain_containing_an_unresolvable_gate_is_an_error() {
    use crate::gate_editor::gates::gate_filtering::filter_events_by_hierarchy_to_mask;

    let r = resolver(vec![centre_column("col")]);
    let chain = vec![Arc::from("col"), Arc::from("ghost")];

    assert!(filter_events_by_hierarchy_to_mask(&events(), &chain, &r).is_err());
}
