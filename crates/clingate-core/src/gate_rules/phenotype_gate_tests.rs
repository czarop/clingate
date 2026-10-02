//! Tests for turning a fitted population into a gate's geometry.

#![cfg(test)]

use super::phenotype::{Frame, Open};
use super::phenotype_gate::*;
use super::shape_fit::Outline;
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

fn frame(origin: f64, unit: f64) -> Frame {
    Frame {
        origin,
        unit,
        by_landmarks: true,
    }
}

/// An axis read from origin 0 in units of 100 on the reference, and from 50 in
/// units of 200 on the sample: 100 on the reference is 250 on the sample, 300
/// is 650.
fn doubling(extent: (f64, f64), open: Open) -> Carry {
    Carry {
        from: frame(0.0, 100.0),
        to: frame(50.0, 200.0),
        extent,
        open,
    }
}

/// An axis that reads the same on both.
fn standing(extent: (f64, f64)) -> Carry {
    Carry {
        from: frame(0.0, 1.0),
        to: frame(0.0, 1.0),
        extent,
        open: Open::default(),
    }
}

/// An axis on which the gate slides `by`, its size kept.
fn shifting(by: f64) -> Carry {
    Carry {
        from: frame(0.0, 1.0),
        to: frame(by, 1.0),
        extent: (0.0, 0.0),
        open: Open::default(),
    }
}

fn rectangle(x: (f32, f32), y: (f32, f32)) -> GateGeometry {
    GateGeometry::Rectangle {
        min: node("a", x.0, y.0),
        max: node("b", x.1, y.1),
    }
}

fn x_span(geometry: &GateGeometry) -> (f32, f32) {
    let GateGeometry::Rectangle { min, max } = geometry else {
        panic!("a rectangle must stay a rectangle");
    };
    (
        min.get_coordinate(X).unwrap(),
        max.get_coordinate(X).unwrap(),
    )
}

#[test]
fn a_rectangle_is_carried_edge_by_edge_and_stays_a_rectangle() {
    let x = doubling((100.0, 300.0), Open::default());
    let moved = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &x,
        &standing((10.0, 20.0)),
    )
    .expect("a rectangle can be carried");
    assert_eq!(x_span(&moved), (250.0, 650.0));
    let GateGeometry::Rectangle { min, max } = moved else {
        unreachable!()
    };
    assert_eq!(
        (min.get_coordinate(Y), max.get_coordinate(Y)),
        (Some(10.0), Some(20.0))
    );
}

/// An axis read from 0 in units of 100 on the reference and of 50 on the
/// sample: 100 goes to 50, 300 to 150.
fn halving(extent: (f64, f64), open: Open) -> Carry {
    Carry {
        from: frame(0.0, 100.0),
        to: frame(0.0, 50.0),
        extent,
        open,
    }
}

const OPEN_ABOVE: Open = Open {
    low: false,
    high: true,
};

#[test]
fn a_side_the_gate_leaves_open_is_never_pulled_in() {
    let moved = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &halving((100.0, 300.0), OPEN_ABOVE),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (50.0, 300.0));
    let closed = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &halving((100.0, 300.0), Open::default()),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&closed), (50.0, 150.0));
}

#[test]
fn a_side_the_gate_leaves_open_moves_out_with_the_frame() {
    let moved = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &doubling((100.0, 300.0), OPEN_ABOVE),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (250.0, 650.0));
}

#[test]
fn a_gate_slides_by_how_far_its_closed_edges_move() {
    // Both closed: 100 moves 150 and 300 moves 350, 250 on average.
    assert_eq!(doubling((100.0, 300.0), Open::default()).shift(), 250.0);
    // The far side open: only the near edge counts.
    assert_eq!(doubling((100.0, 300.0), OPEN_ABOVE).shift(), 150.0);
    let both = Open {
        low: true,
        high: true,
    };
    assert_eq!(doubling((100.0, 300.0), both).shift(), 0.0);
}

#[test]
fn a_slid_gate_never_pulls_in_a_side_it_leaves_open() {
    // The near edge moves -50, as carried; the far one would follow it in.
    let moved = slid(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &halving((100.0, 300.0), OPEN_ABOVE),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (50.0, 300.0));
}

#[test]
fn a_gate_stretches_by_the_units_only_where_both_its_edges_are_carried() {
    assert_eq!(doubling((100.0, 300.0), Open::default()).stretch(), 2.0);
    assert_eq!(doubling((100.0, 300.0), OPEN_ABOVE).stretch(), 1.0);
}

#[test]
fn an_unbounded_edge_stays_unbounded() {
    // 1e16 is Omiq's "this side does not close". Moving it would turn a
    // half-open gate into one with an arbitrary far edge that exports as a
    // real coordinate.
    let was = rectangle((100.0, 1e16), (-1e16, 1e16));
    let moved = carried(
        &was,
        &params(),
        &doubling((100.0, 1e16), Open::default()),
        &standing((-1e16, 1e16)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (250.0, 1e16));
    let slid = slid(&was, &params(), &shifting(500.0), &shifting(500.0)).unwrap();
    assert_eq!(x_span(&slid), (600.0, 1e16));
}

#[test]
fn a_slid_gate_keeps_its_size_and_shape() {
    let triangle = GateGeometry::Polygon {
        nodes: vec![
            node("a", 100.0, 100.0),
            node("b", 300.0, 100.0),
            node("c", 200.0, 300.0),
        ],
        closed: true,
    };
    let GateGeometry::Polygon { nodes, .. } =
        slid(&triangle, &params(), &shifting(500.0), &shifting(-50.0)).unwrap()
    else {
        panic!("not a polygon");
    };
    let at = |n: &GateNode| (n.get_coordinate(X).unwrap(), n.get_coordinate(Y).unwrap());
    assert_eq!(
        nodes.iter().map(at).collect::<Vec<_>>(),
        vec![(600.0, 50.0), (800.0, 50.0), (700.0, 250.0)]
    );
}

#[test]
fn an_ellipse_stays_an_ellipse_its_radii_scaled_by_each_axis_s_units() {
    let was = GateGeometry::Ellipse {
        center: node("c", 100.0, 200.0),
        radius_x: 50.0,
        radius_y: 20.0,
        angle: 0.0,
    };
    let GateGeometry::Ellipse {
        center,
        radius_x,
        radius_y,
        ..
    } = carried(
        &was,
        &params(),
        &doubling((100.0, 100.0), Open::default()),
        &standing((200.0, 200.0)),
    )
    .unwrap()
    else {
        panic!("an ellipse must stay an ellipse");
    };
    assert_eq!(center.get_coordinate(X), Some(250.0));
    assert_eq!(center.get_coordinate(Y), Some(200.0));
    assert_eq!((radius_x, radius_y), (100.0, 20.0));
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
    let GateGeometry::Rectangle { min, .. } =
        slid(&was, &params(), &shifting(900.0), &shifting(900.0)).unwrap()
    else {
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
        slid(&was, &params(), &shifting(1.0), &shifting(1.0)),
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
