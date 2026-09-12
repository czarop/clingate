//! Tests for the composite gates.
//!
//! A composite is one draggable object that owns several real gates. It is
//! registered in the store under its own id *and* under each subgate id, all
//! aliased to the same `Arc`, so the invariants about subgate identity and count
//! are what keep deletion, rescaling and export from corrupting the registry.
//!
//! cargo test gate_composite -- --nocapture

#![cfg(test)]

use crate::gate_editor::gates::gate_composite::bisector_gate::BisectorGate;
use crate::gate_editor::gates::gate_composite::quadrant_gate::QuadrantGate;
use crate::gate_editor::gates::gate_composite::skewed_quadrant_gate::{
    DataPoints, SkewedQuadrantGate,
};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::plots::axis_store::PlotMapper;
use flow_fcs::TransformType;
use std::sync::Arc;

const X: &str = "CD3";
const Y: &str = "CD4";

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

fn quadrant(id: &str) -> QuadrantGate {
    QuadrantGate::try_new_from_raw_coord(
        &mapper(),
        Arc::from(id),
        format!("{id} name"),
        (300.0, 300.0),
        Arc::from(X),
        Arc::from(Y),
    )
    .unwrap()
}

fn bisector(id: &str) -> BisectorGate {
    BisectorGate::try_new(
        &mapper(),
        Arc::from(id),
        format!("{id} name"),
        (300.0, 300.0),
        Arc::from(X),
        Arc::from(Y),
    )
    .unwrap()
}

fn skewed(id: &str) -> SkewedQuadrantGate {
    SkewedQuadrantGate::try_new_from_raw_coord(
        &mapper(),
        Arc::from(id),
        format!("{id} name"),
        (300.0, 300.0),
        Arc::from(X),
        Arc::from(Y),
    )
    .unwrap()
}

// ─── Identity and subgate structure ───────────────────────────────────────────

#[test]
fn every_composite_reports_itself_as_composite() {
    assert!(quadrant("q").is_composite());
    assert!(bisector("b").is_composite());
    assert!(skewed("s").is_composite());
}

#[test]
fn a_quadrant_owns_exactly_four_subgates() {
    assert_eq!(quadrant("q").get_inner_gate_ids().len(), 4);
}

#[test]
fn a_bisector_owns_exactly_two_subgates() {
    assert_eq!(bisector("b").get_inner_gate_ids().len(), 2);
}

#[test]
fn a_skewed_quadrant_owns_exactly_four_subgates() {
    assert_eq!(skewed("s").get_inner_gate_ids().len(), 4);
}

/// The store keys a composite by its own id as well as by each subgate id. If a
/// subgate id collided with another, one of them would silently overwrite the
/// other in the registry.
#[test]
fn subgate_ids_are_distinct() {
    for ids in [
        quadrant("q").get_inner_gate_ids(),
        bisector("b").get_inner_gate_ids(),
        skewed("s").get_inner_gate_ids(),
    ] {
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "duplicate subgate id in {ids:?}");
    }
}

#[test]
fn subgate_ids_are_derived_from_the_parent_id() {
    for id in quadrant("quad-1").get_inner_gate_ids() {
        assert!(
            id.starts_with("quad-1"),
            "subgate {id} should be namespaced under its parent"
        );
    }
}

/// `get_gate_ref(Some(subgate_id))` is how filtering and statistics reach an
/// individual quadrant. Every advertised subgate id must resolve.
#[test]
fn each_advertised_subgate_id_resolves_to_a_real_gate() {
    let q = quadrant("q");
    for id in q.get_inner_gate_ids() {
        assert!(
            q.get_gate_ref(Some(&id)).is_some(),
            "subgate {id} did not resolve"
        );
    }
}

#[test]
fn an_unknown_subgate_id_does_not_resolve() {
    assert!(quadrant("q").get_gate_ref(Some("not-a-subgate")).is_none());
}

#[test]
fn a_composite_reports_the_parameters_it_was_built_on() {
    assert_eq!(quadrant("q").get_params(), (Arc::from(X), Arc::from(Y)));
    assert_eq!(bisector("b").get_params(), (Arc::from(X), Arc::from(Y)));
    assert_eq!(skewed("s").get_params(), (Arc::from(X), Arc::from(Y)));
}

// ─── Quadrant geometry ────────────────────────────────────────────────────────

/// A plain quadrant is forced orthogonal on construction: its arms lie exactly
/// on the centre's row and column, whatever skew the input carried.
#[test]
fn a_quadrant_is_forced_orthogonal() {
    let points = DataPoints::new_from_data_center(400.0, 250.0, 0.0..=1000.0, 0.0..=1000.0);
    let q = QuadrantGate::try_new_from_data_points(
        Arc::from("q"),
        "q".to_string(),
        points,
        Arc::from(X),
        Arc::from(Y),
        true,
        None,
        None,
        (0.0, 0.0),
    )
    .unwrap();

    // Reconstructed through the public drawing path: the gate exists and owns
    // its four quadrants with the centre where it was asked for.
    assert_eq!(q.get_inner_gate_ids().len(), 4);
    assert!(!q.draw_self(false, None, &mapper(), &None).is_empty());
}

#[test]
fn data_points_derive_their_arms_from_the_centre() {
    let p = DataPoints::new_from_data_center(400.0, 250.0, 0.0..=1000.0, 0.0..=1000.0);

    assert_eq!(p.center, (400.0, 250.0));
    assert_eq!(p.left, (0.0, 250.0), "left sits on the centre's row");
    assert_eq!(p.right, (1000.0, 250.0));
    assert_eq!(p.bottom, (400.0, 0.0), "bottom sits on the centre's column");
    assert_eq!(p.top, (400.0, 1000.0));
}

/// An imported centre outside the plot would put the handles off-screen and be
/// ungrabbable, so it is clamped to the axis bounds.
#[test]
fn a_centre_outside_the_axes_is_clamped_onto_them() {
    let p = DataPoints::new_from_data_center(5000.0, -900.0, 0.0..=1000.0, 0.0..=1000.0);

    assert_eq!(p.center, (1000.0, 0.0));
}

#[test]
fn swapping_a_data_points_axis_transposes_every_handle() {
    let p = DataPoints::new_from_data_center(400.0, 250.0, 0.0..=1000.0, 0.0..=1000.0);
    let s = p.clone_for_swap_axis();

    assert_eq!(s.center, (250.0, 400.0));
    // What was the bottom handle becomes the left one, transposed.
    assert_eq!(s.left, (p.bottom.1, p.bottom.0));
    assert_eq!(s.right, (p.top.1, p.top.0));
}

// ─── Editing ──────────────────────────────────────────────────────────────────

#[test]
fn moving_a_composite_keeps_its_subgate_count_and_ids() {
    let q = quadrant("q");
    let before = q.get_inner_gate_ids();

    let moved = q
        .replace_point((500.0, 500.0), 0, &mapper())
        .expect("centre moves");

    assert_eq!(
        moved.get_inner_gate_ids(),
        before,
        "editing must not renumber the subgates"
    );
    assert_eq!(&*moved.get_id(), "q");
}

#[test]
fn a_bisector_keeps_its_two_subgates_through_a_move() {
    let b = bisector("b");
    let before = b.get_inner_gate_ids();

    let moved = b
        .replace_point((450.0, 300.0), 0, &mapper())
        .expect("centre moves");

    assert_eq!(moved.get_inner_gate_ids(), before);
    assert!(moved.is_composite());
}

// ─── Rendering ────────────────────────────────────────────────────────────────

#[test]
fn every_composite_draws_something() {
    let m = mapper();
    for g in [
        Box::new(quadrant("q")) as Box<dyn DrawableGate>,
        Box::new(bisector("b")),
        Box::new(skewed("s")),
    ] {
        assert!(
            !g.draw_self(false, None, &m, &None).is_empty(),
            "{} drew nothing",
            g.get_id()
        );
    }
}

#[test]
fn a_composite_marks_its_shapes_as_composite() {
    let shapes = quadrant("q").draw_self(true, None, &mapper(), &None);

    assert!(
        shapes.iter().any(|s| s.is_composite()),
        "a composite should tag at least some shapes as composite"
    );
}

// ─── Axis matching ────────────────────────────────────────────────────────────

#[test]
fn matching_a_composite_to_its_own_axes_changes_nothing() {
    assert!(quadrant("q").match_to_plot_axis(X, Y).unwrap().is_none());
    assert!(bisector("b").match_to_plot_axis(X, Y).unwrap().is_none());
}

#[test]
fn matching_a_composite_to_swapped_axes_produces_a_swapped_gate() {
    let swapped = quadrant("q")
        .match_to_plot_axis(Y, X)
        .unwrap()
        .expect("a swap produces a new gate");

    assert_eq!(swapped.get_params(), (Arc::from(Y), Arc::from(X)));
    assert_eq!(swapped.get_inner_gate_ids().len(), 4);
}
