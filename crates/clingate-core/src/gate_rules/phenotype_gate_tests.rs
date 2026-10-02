//! Tests for turning a fitted population into a gate's geometry.

#![cfg(test)]

use super::phenotype::Open;
use super::phenotype_gate::*;
use super::rule::Side;
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

/// The edges at 100 and 300 carried to 250 and 650: the gate twice as wide.
fn doubling(open: Open) -> Carry {
    Carry {
        extent: (100.0, 300.0),
        open,
        to: (250.0, 650.0),
    }
}

/// The edges at 100 and 300 carried to 50 and 150: the gate half as wide.
fn halving(open: Open) -> Carry {
    Carry {
        extent: (100.0, 300.0),
        open,
        to: (50.0, 150.0),
    }
}

/// An axis whose edges stay where they are.
fn standing(extent: (f64, f64)) -> Carry {
    Carry {
        extent,
        open: Open::default(),
        to: extent,
    }
}

/// An axis on which the gate slides `by`, its size kept.
fn shifting(by: f64) -> Carry {
    Carry {
        extent: (0.0, 0.0),
        open: Open::default(),
        to: (by, by),
    }
}

const OPEN_ABOVE: Open = Open {
    low: false,
    high: true,
};

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

fn vertices(geometry: &GateGeometry) -> Vec<(f32, f32)> {
    let GateGeometry::Polygon { nodes, .. } = geometry else {
        panic!("a polygon must stay a polygon");
    };
    nodes
        .iter()
        .map(|n| (n.get_coordinate(X).unwrap(), n.get_coordinate(Y).unwrap()))
        .collect()
}

fn triangle() -> GateGeometry {
    GateGeometry::Polygon {
        nodes: vec![
            node("a", 100.0, 100.0),
            node("b", 300.0, 100.0),
            node("c", 200.0, 300.0),
        ],
        closed: true,
    }
}

/// 0 to 100, one apart: its 5th percentile is 5 and its 95th 95.
fn population() -> Vec<f64> {
    (0..=100).map(f64::from).collect()
}

/// `count` cells one apart, ending at `last`.
fn cells_ending_at(last: f64, count: usize) -> Vec<f64> {
    (0..count).map(|at| last - at as f64).collect()
}

/// 20,000 cells, N(`centre`, 50).
fn normal_cells(centre: f64, seed: u64) -> Vec<f64> {
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal};
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let cells = Normal::new(centre, 50.0).unwrap();
    (0..20_000).map(|_| cells.sample(&mut rng)).collect()
}

#[test]
fn a_gap_runs_from_the_population_s_boundary_to_the_near_boundary_of_the_cells_beyond() {
    // Below, N(-300, 50): their near boundary is 1.645 widths above their
    // peak, -300 + 82.2 = -217.8 - give or take the 10 or so that reading a
    // peak and a width off 20,000 cells wanders by.
    let below = Gap::of(&population(), &normal_cells(-300.0, 1), Side::Lower).unwrap();
    assert_eq!(below.inside, 5.0);
    let beyond = below.beyond.unwrap();
    assert!((beyond + 217.8).abs() < 12.0, "{beyond}");
    // Mirrored above, N(400, 50): 400 - 82.2 = 317.8; the population's own
    // boundary there is its 95th percentile, 95.
    let above = Gap::of(&population(), &normal_cells(400.0, 2), Side::Upper).unwrap();
    assert_eq!(above.inside, 95.0);
    let beyond = above.beyond.unwrap();
    assert!((beyond - 317.8).abs() < 12.0, "{beyond}");
}

#[test]
fn a_tail_trailing_into_the_cells_beyond_does_not_move_their_boundary() {
    // 2,000 more cells spread from -200 to 0, between the cells beyond and the
    // population: their 95th percentile would rise from about -218 to about
    // -110, but their peak and far side do not move.
    let beyond = |cells: &[f64]| {
        Gap::of(&population(), cells, Side::Lower)
            .unwrap()
            .beyond
            .unwrap()
    };
    let alone = normal_cells(-300.0, 1);
    let mut tailed = alone.clone();
    tailed.extend((0..2_000).map(|at| -200.0 + at as f64 / 10.0));
    let (without, with) = (beyond(&alone), beyond(&tailed));
    assert!((with - without).abs() < 5.0, "{without} then {with}");
}

#[test]
fn cells_on_the_other_side_of_the_population_are_not_beyond_it() {
    let above: Vec<f64> = (200..300).map(f64::from).collect();
    assert_eq!(
        Gap::of(&population(), &above, Side::Lower).unwrap().beyond,
        None
    );
}

#[test]
fn dust_beyond_a_population_leaves_no_gap() {
    let beyond = |count| {
        Gap::of(&population(), &cells_ending_at(-101.0, count), Side::Lower)
            .unwrap()
            .beyond
    };
    assert_eq!(beyond(20), None, "20 cells are dust");
    assert!(beyond(21).is_some(), "21 of 101 are more than a hundredth");
    let thousands: Vec<f64> = (0..3000).map(f64::from).collect();
    assert_eq!(
        Gap::of(&thousands, &cells_ending_at(-101.0, 29), Side::Lower)
            .unwrap()
            .beyond,
        None,
        "29 against 3000 are under a hundredth"
    );
}

#[test]
fn a_population_with_no_cells_has_no_gap() {
    assert_eq!(Gap::of(&[], &[1.0, 2.0], Side::Lower), None);
}

#[test]
fn an_edge_keeps_its_place_in_the_gap() {
    // Halfway from the cells beyond, at 500, to the population, at 700; on
    // the sample the gap runs from 600 to 1000, and halfway is 800.
    let there = Gap {
        inside: 700.0,
        beyond: Some(500.0),
    };
    let here = Gap {
        inside: 1000.0,
        beyond: Some(600.0),
    };
    assert_eq!(edge_in_gap(600.0, there, here), 800.0);
}

#[test]
fn an_edge_drawn_into_the_cells_beyond_moves_with_them() {
    // 450 is a quarter of the gap past the cells beyond, at 500, towards
    // them. They move to 600, the population to 1000: the edge goes to 550,
    // not a quarter of the wider gap past them, 500.
    let there = Gap {
        inside: 700.0,
        beyond: Some(500.0),
    };
    let here = Gap {
        inside: 1000.0,
        beyond: Some(600.0),
    };
    assert_eq!(edge_in_gap(450.0, there, here), 550.0);
}

#[test]
fn an_edge_drawn_into_the_population_moves_with_it() {
    // 750 is past the population's boundary, at 700; it moves to 1000, and
    // the edge to 1050 - not to 1100, a quarter of the wider gap past it.
    let there = Gap {
        inside: 700.0,
        beyond: Some(500.0),
    };
    let here = Gap {
        inside: 1000.0,
        beyond: Some(600.0),
    };
    assert_eq!(edge_in_gap(750.0, there, here), 1050.0);
}

#[test]
fn with_nothing_beyond_an_edge_moves_as_far_as_the_population_s_boundary() {
    let crowded = |inside, beyond| Gap {
        inside,
        beyond: Some(beyond),
    };
    let alone = |inside| Gap {
        inside,
        beyond: None,
    };
    // The boundary moves from 700 to 750: the edge, 50 further.
    assert_eq!(
        edge_in_gap(600.0, alone(700.0), crowded(750.0, 100.0)),
        650.0
    );
    assert_eq!(
        edge_in_gap(600.0, crowded(700.0, 500.0), alone(750.0)),
        650.0
    );
    assert_eq!(
        edge_in_gap(600.0, crowded(700.0, 700.0), crowded(750.0, 100.0)),
        650.0,
        "a gap of nothing has no place in it to keep"
    );
}

#[test]
fn a_rectangle_is_carried_edge_by_edge_and_stays_a_rectangle() {
    let moved = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &doubling(Open::default()),
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

#[test]
fn a_carried_polygon_keeps_its_shape_stretched_between_its_new_edges() {
    // x from 100..300 to 250..650: each vertex as far between the new edges
    // as it was between the old - 100 to 250, 300 to 650, 200 halfway, 450.
    let moved = carried(
        &triangle(),
        &params(),
        &doubling(Open::default()),
        &standing((100.0, 300.0)),
    )
    .unwrap();
    assert_eq!(
        vertices(&moved),
        vec![(250.0, 100.0), (650.0, 100.0), (450.0, 300.0)]
    );
}

#[test]
fn a_side_the_gate_leaves_open_is_never_pulled_in() {
    let moved = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &halving(OPEN_ABOVE),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (50.0, 300.0));
    let closed = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &halving(Open::default()),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&closed), (50.0, 150.0));
}

#[test]
fn a_side_the_gate_leaves_open_moves_out() {
    let moved = carried(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &doubling(OPEN_ABOVE),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (250.0, 650.0));
}

#[test]
fn a_gate_slides_by_how_far_its_closed_edges_move() {
    // Both closed: 100 moves 150 and 300 moves 350, 250 on average.
    assert_eq!(doubling(Open::default()).shift(), 250.0);
    // The far side open: only the near edge counts.
    assert_eq!(doubling(OPEN_ABOVE).shift(), 150.0);
    let both = Open {
        low: true,
        high: true,
    };
    assert_eq!(doubling(both).shift(), 0.0);
}

#[test]
fn a_slid_gate_keeps_its_size_even_where_a_side_is_open() {
    // The near edge moves -50, as carried, and the far one follows it in.
    let moved = slid(
        &rectangle((100.0, 300.0), (10.0, 20.0)),
        &params(),
        &halving(OPEN_ABOVE),
        &standing((10.0, 20.0)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (50.0, 250.0));
}

#[test]
fn a_gate_stretches_only_where_both_its_edges_are_carried() {
    assert_eq!(doubling(Open::default()).stretch(), 2.0);
    assert_eq!(doubling(OPEN_ABOVE).stretch(), 1.0);
}

#[test]
fn an_unbounded_edge_stays_unbounded() {
    // 1e16 is Omiq's "this side does not close". Moving it would turn a
    // half-open gate into one with an arbitrary far edge that exports as a
    // real coordinate.
    let was = rectangle((100.0, 1e16), (-1e16, 1e16));
    let x = Carry {
        extent: (100.0, 1e16),
        open: Open::default(),
        to: (250.0, 2e16),
    };
    let moved = carried(&was, &params(), &x, &standing((-1e16, 1e16))).unwrap();
    assert_eq!(x_span(&moved), (250.0, 1e16));
    let slid = slid(&was, &params(), &shifting(500.0), &shifting(500.0)).unwrap();
    assert_eq!(x_span(&slid), (600.0, 1e16));
}

#[test]
fn a_slid_gate_keeps_its_size_and_shape() {
    let moved = slid(&triangle(), &params(), &shifting(500.0), &shifting(-50.0)).unwrap();
    assert_eq!(
        vertices(&moved),
        vec![(600.0, 50.0), (800.0, 50.0), (700.0, 250.0)]
    );
}

#[test]
fn an_ellipse_stays_an_ellipse_its_radii_scaled_by_each_axis_s_stretch() {
    let was = GateGeometry::Ellipse {
        center: node("c", 200.0, 200.0),
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
        &doubling(Open::default()),
        &standing((180.0, 220.0)),
    )
    .unwrap()
    else {
        panic!("an ellipse must stay an ellipse");
    };
    // 200, halfway between 100 and 300, goes halfway between 250 and 650.
    assert_eq!(center.get_coordinate(X), Some(450.0));
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
