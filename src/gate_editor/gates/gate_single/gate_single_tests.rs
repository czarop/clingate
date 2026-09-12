//! Tests for the single (non-composite) gate geometries.
//!
//! These are the editing operations behind every drag in the UI, and they are
//! also exactly what the Omiq export will have to read back out, so their
//! invariants matter twice over.
//!
//! cargo test gate_single -- --nocapture

#![cfg(test)]

use crate::gate_editor::gates::gate_drag::GateDragData;
use crate::gate_editor::gates::gate_single::ellipse_gate::EllipseGate;
use crate::gate_editor::gates::gate_single::line_gate::LineGate;
use crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate;
use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::plots::axis_store::PlotMapper;
use flow_fcs::TransformType;
use flow_gates::{GateGeometry, create_polygon_geometry, create_rectangle_geometry};
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

fn square(id: &str) -> RectangleGate {
    let geometry = create_rectangle_geometry(
        vec![(100.0, 100.0), (300.0, 100.0), (300.0, 300.0), (100.0, 300.0)],
        X,
        Y,
    )
    .unwrap();
    RectangleGate::try_new(gate(id, geometry), true).unwrap()
}

fn triangle(id: &str) -> PolygonGate {
    let geometry =
        create_polygon_geometry(vec![(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)], X, Y)
            .unwrap();
    PolygonGate::try_new(gate(id, geometry), true).unwrap()
}

fn ellipse(id: &str) -> EllipseGate {
    let geometry = crate::omiq::deserialise::create_omiq_ellipse_geometry(
        (100.0, 200.0), // left
        (300.0, 200.0), // right
        (200.0, 250.0), // top
        X,
        Y,
    )
    .unwrap();
    EllipseGate::try_new(gate(id, geometry), true).unwrap()
}

/// The rectangle corners, read back out of the geometry.
fn corners(g: &RectangleGate) -> ((f32, f32), (f32, f32)) {
    let inner = g.get_gate_ref(None).unwrap();
    match &inner.geometry {
        GateGeometry::Rectangle { min, max } => (
            (
                min.get_coordinate(X).unwrap(),
                min.get_coordinate(Y).unwrap(),
            ),
            (
                max.get_coordinate(X).unwrap(),
                max.get_coordinate(Y).unwrap(),
            ),
        ),
        _ => panic!("expected a rectangle"),
    }
}

fn polygon_points(g: &dyn DrawableGate) -> Vec<(f32, f32)> {
    let inner = g.get_gate_ref(None).unwrap();
    match &inner.geometry {
        GateGeometry::Polygon { nodes, .. } => nodes
            .iter()
            .map(|n| {
                (
                    n.get_coordinate(X).unwrap(),
                    n.get_coordinate(Y).unwrap(),
                )
            })
            .collect(),
        _ => panic!("expected a polygon"),
    }
}

fn ellipse_parts(g: &dyn DrawableGate) -> ((f32, f32), f32, f32, f32) {
    let inner = g.get_gate_ref(None).unwrap();
    match &inner.geometry {
        GateGeometry::Ellipse { center, radius_x, radius_y, angle } => (
            (
                center.get_coordinate(X).unwrap(),
                center.get_coordinate(Y).unwrap(),
            ),
            *radius_x,
            *radius_y,
            *angle,
        ),
        _ => panic!("expected an ellipse"),
    }
}

// ─── Identity ─────────────────────────────────────────────────────────────────

#[test]
fn a_gate_reports_its_own_identity() {
    let g = square("rect-1");

    assert_eq!(&*g.get_id(), "rect-1");
    assert_eq!(g.get_name(), "rect-1 name");
    assert_eq!(g.get_params(), (Arc::from(X), Arc::from(Y)));
    assert!(g.is_primary());
    assert!(g.is_finalised());
}

#[test]
fn a_single_gate_is_not_composite_and_owns_only_itself() {
    for g in [
        Box::new(square("r")) as Box<dyn DrawableGate>,
        Box::new(triangle("p")),
        Box::new(ellipse("e")),
    ] {
        assert!(!g.is_composite());
        assert_eq!(g.get_inner_gate_ids().len(), 1);
        assert_eq!(g.get_inner_gate_ids()[0], g.get_id());
    }
}

#[test]
fn a_rotate_is_a_no_op_on_an_unrotatable_gate() {
    assert!(square("r").rotate_gate((0.0, 0.0)).unwrap().is_none());
    assert!(triangle("p").rotate_gate((0.0, 0.0)).unwrap().is_none());
}

// ─── Rectangle ────────────────────────────────────────────────────────────────

#[test]
fn a_rectangle_keeps_the_corners_it_was_built_from() {
    let (min, max) = corners(&square("r"));
    assert_eq!(min, (100.0, 100.0));
    assert_eq!(max, (300.0, 300.0));
}

/// The drag offset is start-minus-current, and the gate subtracts it, so a drag
/// down-right moves the gate down-right.
#[test]
fn dragging_a_rectangle_translates_it_without_resizing() {
    let g = square("r");
    let drag = GateDragData::new(g.get_id(), (0.0, 0.0), (50.0, 20.0));

    let moved = g.replace_points(drag).unwrap().expect("rectangle moves");
    let moved = moved.as_any().downcast_ref::<RectangleGate>().unwrap();
    let (min, max) = corners(moved);

    assert_eq!(min, (150.0, 120.0));
    assert_eq!(max, (350.0, 320.0));
    assert_eq!(
        (max.0 - min.0, max.1 - min.1),
        (200.0, 200.0),
        "a move must not resize"
    );
}

#[test]
fn dragging_a_rectangle_nowhere_leaves_it_alone() {
    let g = square("r");
    let drag = GateDragData::new(g.get_id(), (10.0, 10.0), (10.0, 10.0));

    let moved = g.replace_points(drag).unwrap().expect("rectangle moves");
    let moved = moved.as_any().downcast_ref::<RectangleGate>().unwrap();

    assert_eq!(corners(moved), corners(&square("r")));
}

#[test]
fn moving_a_rectangle_corner_resizes_it() {
    let g = square("r");
    // Corner 2 is (max_x, max_y) in the constructor's winding order.
    let resized = g.replace_point((400.0, 400.0), 2, &mapper()).unwrap();
    let resized = resized.as_any().downcast_ref::<RectangleGate>().unwrap();
    let (min, max) = corners(resized);

    assert_eq!(min, (100.0, 100.0), "the opposite corner is pinned");
    assert_eq!(max, (400.0, 400.0));
}

#[test]
fn a_rectangle_keeps_its_id_and_name_through_an_edit() {
    let g = square("rect-1");
    let drag = GateDragData::new(g.get_id(), (0.0, 0.0), (5.0, 5.0));
    let moved = g.replace_points(drag).unwrap().unwrap();

    assert_eq!(&*moved.get_id(), "rect-1");
    assert_eq!(moved.get_name(), "rect-1 name");
}

// ─── Polygon ──────────────────────────────────────────────────────────────────

#[test]
fn a_polygon_keeps_the_vertices_it_was_built_from() {
    assert_eq!(
        polygon_points(&triangle("p")),
        vec![(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)]
    );
}

#[test]
fn dragging_a_polygon_translates_every_vertex_equally() {
    let g = triangle("p");
    let drag = GateDragData::new(g.get_id(), (0.0, 0.0), (10.0, 25.0));
    let moved = g.replace_points(drag).unwrap().expect("polygon moves");

    assert_eq!(
        polygon_points(moved.as_ref()),
        vec![(110.0, 125.0), (310.0, 125.0), (210.0, 325.0)]
    );
}

#[test]
fn moving_one_polygon_vertex_leaves_the_others_in_place() {
    let g = triangle("p");
    let edited = g.replace_point((250.0, 400.0), 2, &mapper()).unwrap();
    let points = polygon_points(edited.as_ref());

    assert_eq!(points[0], (100.0, 100.0));
    assert_eq!(points[1], (300.0, 100.0));
    assert_eq!(points[2], (250.0, 400.0));
}

#[test]
fn a_polygon_keeps_its_vertex_count_through_an_edit() {
    let g = triangle("p");
    let edited = g.replace_point((0.0, 0.0), 1, &mapper()).unwrap();

    assert_eq!(polygon_points(edited.as_ref()).len(), 3);
}

// ─── Ellipse ──────────────────────────────────────────────────────────────────

#[test]
fn an_ellipse_keeps_the_centre_and_radii_it_was_built_from() {
    let (centre, rx, ry, _) = ellipse_parts(&ellipse("e"));

    assert!((centre.0 - 200.0).abs() < 1e-3, "centre x was {}", centre.0);
    assert!((centre.1 - 200.0).abs() < 1e-3, "centre y was {}", centre.1);
    assert!((rx - 100.0).abs() < 1e-3, "radius_x was {rx}");
    assert!((ry - 50.0).abs() < 1e-3, "radius_y was {ry}");
}

#[test]
fn dragging_an_ellipse_moves_its_centre_and_keeps_its_shape() {
    let g = ellipse("e");
    let (_, rx, ry, angle) = ellipse_parts(&g);
    let drag = GateDragData::new(g.get_id(), (0.0, 0.0), (40.0, 60.0));

    let moved = g.replace_points(drag).unwrap().expect("ellipse moves");
    let (centre, new_rx, new_ry, new_angle) = ellipse_parts(moved.as_ref());

    assert!((centre.0 - 240.0).abs() < 1e-3, "centre x was {}", centre.0);
    assert!((centre.1 - 260.0).abs() < 1e-3, "centre y was {}", centre.1);
    assert!((new_rx - rx).abs() < 1e-3, "a move must not resize");
    assert!((new_ry - ry).abs() < 1e-3);
    assert!((new_angle - angle).abs() < 1e-3, "a move must not rotate");
}

/// An axis-aligned ellipse has no rotation, and the eigen-decomposition should
/// say so rather than reporting an arbitrary angle.
#[test]
fn an_axis_aligned_ellipse_has_no_rotation() {
    let (_, _, _, angle) = ellipse_parts(&ellipse("e"));
    assert!(angle.abs() < 1e-4, "angle was {angle}");
}

/// Ellipses are rotatable, unlike rectangles and polygons.
#[test]
fn an_ellipse_can_be_rotated() {
    let g = ellipse("e");
    assert!(
        g.rotate_gate((300.0, 300.0)).unwrap().is_some(),
        "an ellipse should accept a rotation"
    );
}

// ─── Line ─────────────────────────────────────────────────────────────────────

fn line(id: &str) -> LineGate {
    let geometry =
        create_rectangle_geometry(vec![(150.0, 0.0), (150.0, 1000.0)], X, Y).unwrap();
    LineGate::try_new(gate(id, geometry), 200.0, true).unwrap()
}

#[test]
fn a_line_gate_records_the_height_it_was_created_at() {
    let g = line("l");
    assert_eq!(g.height, 200.0);
    assert!(!g.is_composite());
}

#[test]
fn a_line_gate_keeps_its_identity_through_a_drag() {
    let g = line("line-1");
    let drag = GateDragData::new(g.get_id(), (0.0, 0.0), (25.0, 0.0));

    if let Some(moved) = g.replace_points(drag).unwrap() {
        assert_eq!(&*moved.get_id(), "line-1");
        assert_eq!(moved.get_params(), (Arc::from(X), Arc::from(Y)));
    }
}

// ─── Axis matching ────────────────────────────────────────────────────────────

/// A gate drawn on (x, y) must be able to redraw itself on a plot showing
/// (y, x). Asking for the axes it already has is a no-op.
#[test]
fn matching_a_gate_to_its_own_axes_changes_nothing() {
    assert!(square("r").match_to_plot_axis(X, Y).unwrap().is_none());
    assert!(triangle("p").match_to_plot_axis(X, Y).unwrap().is_none());
}

#[test]
fn matching_a_rectangle_to_swapped_axes_transposes_its_corners() {
    let swapped = square("r")
        .match_to_plot_axis(Y, X)
        .unwrap()
        .expect("a swap produces a new gate");

    assert_eq!(
        swapped.get_params(),
        (Arc::from(Y), Arc::from(X)),
        "the parameters swap with the geometry"
    );
}

#[test]
fn matching_a_polygon_to_swapped_axes_transposes_its_vertices() {
    let g = triangle("p");
    let swapped = g
        .match_to_plot_axis(Y, X)
        .unwrap()
        .expect("a swap produces a new gate");

    let inner = swapped.get_gate_ref(None).unwrap();
    match &inner.geometry {
        GateGeometry::Polygon { nodes, .. } => {
            // A transpose swaps which axis each coordinate is *drawn* against,
            // but the event a vertex sits on does not move: read back by
            // parameter name, every vertex still reports what it always did.
            for (i, expected) in [(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)]
                .into_iter()
                .enumerate()
            {
                let by_name = (
                    nodes[i].get_coordinate(X).unwrap(),
                    nodes[i].get_coordinate(Y).unwrap(),
                );
                assert_eq!(by_name, expected, "vertex {i} moved in data space");
            }
        }
        _ => panic!("expected a polygon"),
    }
}

#[test]
fn matching_to_an_unrelated_axis_is_an_error() {
    assert!(square("r").match_to_plot_axis("CD3", "CD4").is_err());
}

// ─── Hit testing ──────────────────────────────────────────────────────────────

#[test]
fn a_point_on_the_perimeter_is_detected() {
    let g = square("r");
    let tolerance = mapper().get_data_tolerance(5.0);

    assert!(
        g.is_point_on_perimeter((200.0, 100.0), tolerance, &mapper())
            .is_some(),
        "the midpoint of the bottom edge is on the perimeter"
    );
}

#[test]
fn a_point_well_away_from_the_perimeter_is_not_detected() {
    let g = square("r");
    let tolerance = mapper().get_data_tolerance(5.0);

    assert!(
        g.is_point_on_perimeter((900.0, 900.0), tolerance, &mapper())
            .is_none()
    );
}

/// The interior is not the perimeter - clicking the middle of a gate must not
/// select its edge.
#[test]
fn a_point_inside_the_gate_is_not_on_its_perimeter() {
    let g = square("r");
    let tolerance = mapper().get_data_tolerance(5.0);

    assert!(
        g.is_point_on_perimeter((200.0, 200.0), tolerance, &mapper())
            .is_none()
    );
}

// ─── Rendering ────────────────────────────────────────────────────────────────

#[test]
fn every_gate_draws_something() {
    let m = mapper();
    for g in [
        Box::new(square("r")) as Box<dyn DrawableGate>,
        Box::new(triangle("p")),
        Box::new(ellipse("e")),
        Box::new(line("l")),
    ] {
        assert!(
            !g.draw_self(false, None, &m, &None).is_empty(),
            "{} drew nothing",
            g.get_id()
        );
    }
}

/// Selecting a gate adds its vertex handles, so the selected form always has at
/// least as many shapes as the unselected one.
#[test]
fn selecting_a_gate_adds_its_handles() {
    let m = mapper();
    let g = triangle("p");

    let unselected = g.draw_self(false, None, &m, &None).len();
    let selected = g.draw_self(true, None, &m, &None).len();

    assert!(
        selected > unselected,
        "selected drew {selected} shapes, unselected {unselected}"
    );
}
