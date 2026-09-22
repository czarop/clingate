//! Tests for turning a fitted population into a gate's geometry.

#![cfg(test)]

use super::phenotype_gate::*;
use super::shape_fit::{Extent, Outline, Reshape};
use flow_gates::{GateGeometry, GateNode};
use std::sync::Arc;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

fn params() -> (Arc<str>, Arc<str>) {
    (Arc::from(X), Arc::from(Y))
}

fn node(id: &str, x: f32, y: f32) -> GateNode {
    GateNode::new(id)
        .with_coordinate(Arc::from(X) as Arc<str>, x)
        .with_coordinate(Arc::from(Y) as Arc<str>, y)
}

/// A reshape that moves a population from one centre to another and scales it.
fn moving(from: (f64, f64), to: (f64, f64), scale: (f64, f64)) -> Reshape {
    Reshape {
        from: Extent {
            centre: from,
            spread: (1.0, 1.0),
        },
        to: Extent {
            centre: to,
            spread: (scale.0, scale.1),
        },
        scale,
        clamped: false,
    }
}

#[test]
fn a_rectangle_is_moved_and_stays_a_rectangle() {
    let was = GateGeometry::Rectangle {
        min: node("a", 100.0, 100.0),
        max: node("b", 300.0, 300.0),
    };
    let moved = reshaped(
        &was,
        &params(),
        &moving((200.0, 200.0), (600.0, 500.0), (1.0, 1.0)),
    )
    .expect("a rectangle can be reshaped");
    let GateGeometry::Rectangle { min, max } = moved else {
        panic!("a rectangle must stay a rectangle");
    };
    assert_eq!(min.get_coordinate(X), Some(500.0));
    assert_eq!(max.get_coordinate(X), Some(700.0));
    assert_eq!(min.get_coordinate(Y), Some(400.0));
    assert_eq!(max.get_coordinate(Y), Some(600.0));
}

#[test]
fn a_rectangle_is_resized_about_the_population_not_the_origin() {
    // Scaling about the origin would send a gate drawn far from zero a long way
    // off when its population turned out wider.
    let was = GateGeometry::Rectangle {
        min: node("a", 900.0, 100.0),
        max: node("b", 1100.0, 300.0),
    };
    let moved = reshaped(
        &was,
        &params(),
        &moving((1000.0, 200.0), (1000.0, 200.0), (2.0, 1.0)),
    )
    .expect("reshaped");
    let GateGeometry::Rectangle { min, max } = moved else {
        panic!("not a rectangle");
    };
    // Twice as wide, still centred on 1000.
    assert_eq!(min.get_coordinate(X), Some(800.0));
    assert_eq!(max.get_coordinate(X), Some(1200.0));
}

#[test]
fn an_unbounded_edge_stays_unbounded() {
    // 1e16 is Omiq's "this side does not close". Scaling it would turn a
    // half-open gate into one with an arbitrary far edge that exports as a
    // real coordinate.
    let was = GateGeometry::Rectangle {
        min: node("a", 500.0, -1e16),
        max: node("b", 1e16, 1e16),
    };
    let moved = reshaped(
        &was,
        &params(),
        &moving((600.0, 0.0), (900.0, 0.0), (2.0, 2.0)),
    )
    .expect("reshaped");
    let GateGeometry::Rectangle { min, max } = moved else {
        panic!("not a rectangle");
    };
    assert_eq!(
        max.get_coordinate(X),
        Some(1e16),
        "the open side must stay open"
    );
    assert_eq!(min.get_coordinate(Y), Some(-1e16));
    assert_eq!(max.get_coordinate(Y), Some(1e16));
    // The closed side moved.
    assert!((min.get_coordinate(X).unwrap() - 700.0).abs() < 1e-3);
}

#[test]
fn a_polygon_keeps_its_shape_when_it_is_moved() {
    let was = GateGeometry::Polygon {
        nodes: vec![
            node("a", 100.0, 100.0),
            node("b", 300.0, 100.0),
            node("c", 200.0, 300.0),
        ],
        closed: true,
    };
    let moved = reshaped(
        &was,
        &params(),
        &moving((200.0, 166.0), (700.0, 166.0), (1.0, 1.0)),
    )
    .expect("reshaped");
    let GateGeometry::Polygon { nodes, .. } = moved else {
        panic!("not a polygon");
    };
    // Still a triangle with the same proportions, 500 to the right.
    let xs: Vec<f32> = nodes.iter().map(|n| n.get_coordinate(X).unwrap()).collect();
    assert_eq!(xs, vec![600.0, 800.0, 700.0]);
    let ys: Vec<f32> = nodes.iter().map(|n| n.get_coordinate(Y).unwrap()).collect();
    assert_eq!(ys[0], ys[1], "the base must stay level");
}

#[test]
fn an_ellipse_keeps_being_an_ellipse_and_its_radii_scale() {
    let was = GateGeometry::Ellipse {
        center: node("c", 200.0, 200.0),
        radius_x: 50.0,
        radius_y: 20.0,
        angle: 0.0,
    };
    let moved = reshaped(
        &was,
        &params(),
        &moving((200.0, 200.0), (500.0, 400.0), (2.0, 0.5)),
    )
    .expect("reshaped");
    let GateGeometry::Ellipse {
        center,
        radius_x,
        radius_y,
        ..
    } = moved
    else {
        panic!("an ellipse must stay an ellipse");
    };
    assert_eq!(center.get_coordinate(X), Some(500.0));
    assert_eq!(center.get_coordinate(Y), Some(400.0));
    assert_eq!(radius_x, 100.0);
    assert_eq!(radius_y, 10.0);
}

#[test]
fn a_coordinate_on_a_third_channel_is_left_alone() {
    // A gate can carry coordinates for channels it is not drawn on - one
    // imported from a plot with more axes than this one. Rewriting those would
    // move the gate on a plot nobody asked about.
    let mut min = node("a", 100.0, 100.0);
    min.set_coordinate(Arc::from("CD4") as Arc<str>, 42.0);
    let was = GateGeometry::Rectangle {
        min,
        max: node("b", 300.0, 300.0),
    };
    let moved = reshaped(
        &was,
        &params(),
        &moving((200.0, 200.0), (900.0, 900.0), (3.0, 3.0)),
    )
    .expect("reshaped");
    let GateGeometry::Rectangle { min, .. } = moved else {
        panic!("not a rectangle");
    };
    assert_eq!(min.get_coordinate("CD4"), Some(42.0));
}

#[test]
fn a_boolean_gate_has_no_shape_to_fit() {
    let was = GateGeometry::Boolean {
        operation: flow_gates::BooleanOperation::And,
        operands: vec![Arc::from("one"), Arc::from("two")],
    };
    assert_eq!(
        reshaped(&was, &params(), &moving((0.0, 0.0), (1.0, 1.0), (1.0, 1.0))),
        Err(NoGeometry::NotAShape)
    );
}

#[test]
fn an_outline_becomes_a_closed_polygon_on_the_plots_axes() {
    let outline = Outline(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    let GateGeometry::Polygon { nodes, closed } =
        polygon(&outline, &params(), "mait").expect("a polygon can be built")
    else {
        panic!("not a polygon");
    };
    assert!(closed, "a gate's boundary has to close");
    assert_eq!(nodes.len(), 4);
    assert_eq!(nodes[1].get_coordinate(X), Some(10.0));
    assert_eq!(nodes[2].get_coordinate(Y), Some(10.0));
}

#[test]
fn a_polygons_node_names_come_from_the_gate_so_they_do_not_change_every_run() {
    // An export that renamed every vertex on every run would show a diff for a
    // gate that had not moved.
    let outline = Outline(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let first = polygon(&outline, &params(), "mait").expect("built");
    let again = polygon(&outline, &params(), "mait").expect("built");
    let ids = |geometry: &GateGeometry| -> Vec<Arc<str>> {
        let GateGeometry::Polygon { nodes, .. } = geometry else {
            panic!("not a polygon");
        };
        nodes.iter().map(|n| n.id.clone()).collect()
    };
    assert_eq!(ids(&first), ids(&again));
    assert!(ids(&first)[0].contains("mait"));
}

#[test]
fn an_outline_with_too_few_points_is_refused() {
    let outline = Outline(vec![(0.0, 0.0), (1.0, 1.0)]);
    assert_eq!(
        polygon(&outline, &params(), "mait"),
        Err(NoGeometry::TooFewPoints(2))
    );
}
