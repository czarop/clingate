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
        pinned: None,
    }
}

/// The edges at 100 and 300 carried to 50 and 150: the gate half as wide.
fn halving(open: Open) -> Carry {
    Carry {
        extent: (100.0, 300.0),
        open,
        to: (50.0, 150.0),
        pinned: None,
    }
}

/// An axis whose edges stay where they are.
fn standing(extent: (f64, f64)) -> Carry {
    Carry {
        extent,
        open: Open::default(),
        to: extent,
        pinned: None,
    }
}

/// An axis on which the gate slides `by`, its size kept.
fn shifting(by: f64) -> Carry {
    Carry {
        extent: (0.0, 0.0),
        open: Open::default(),
        to: (by, by),
        pinned: None,
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

#[test]
fn a_gap_runs_from_the_population_s_boundary_to_the_near_boundary_of_the_cells_beyond() {
    // 100 cells from -200 to -101; their 95th percentile, nearest the
    // population, is the 95th of 100 sorted: index 94, -106.
    let below = cells_ending_at(-101.0, 100);
    assert_eq!(
        Gap::of(&population(), &below, Side::Lower),
        Some(Gap {
            inside: 5.0,
            beyond: Some(-106.0)
        })
    );
    // Mirrored above: 200 to 299, nearest boundary at index 5, 205.
    let above: Vec<f64> = (200..300).map(f64::from).collect();
    assert_eq!(
        Gap::of(&population(), &above, Side::Upper),
        Some(Gap {
            inside: 95.0,
            beyond: Some(205.0)
        })
    );
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
        pinned: None,
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

/// `n` events around `mean` with width `sd`, reproducibly.
fn cluster(seed: u64, n: usize, mean: f64, sd: f64) -> Vec<f64> {
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal};
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let normal = Normal::new(mean, sd).unwrap();
    (0..n).map(|_| normal.sample(&mut rng)).collect()
}

/// A parent with its negative at 0, one wide, and positives at `positives`.
fn parent(positives: f64) -> Vec<f64> {
    let mut values = cluster(1, 20_000, 0.0, 1.0);
    values.extend(cluster(2, 5_000, positives, 1.0));
    values
}

#[test]
fn a_gate_above_the_negative_is_measured_from_its_lower_side() {
    let (side, widths) = widths_from_negative(&parent(10.0), (2.0, 14.0)).unwrap();
    assert_eq!(side, Side::Lower);
    assert!((widths - 2.0).abs() < 0.2, "{widths} widths");
}

#[test]
fn a_gate_over_the_negative_is_measured_from_its_upper_side() {
    let (side, widths) = widths_from_negative(&parent(10.0), (-6.0, 1.0)).unwrap();
    assert_eq!(side, Side::Upper);
    assert!((widths - 1.0).abs() < 0.2, "{widths} widths");
}

#[test]
fn a_pinned_edge_follows_the_negative_whatever_the_positives_do() {
    for positives in [5.0, 8.0, 14.0] {
        let mut here = cluster(3, 20_000, 3.0, 0.5);
        here.extend(cluster(4, 5_000, positives, 2.0));
        let at = pinned_to_negative(2.0, &here).unwrap();
        assert!(
            (at - 4.0).abs() < 0.1,
            "positives at {positives}: pinned to {at}"
        );
    }
}

#[test]
fn pinned_on_the_reference_an_edge_stays_where_it_was_drawn() {
    let values = parent(10.0);
    for extent in [(2.0, 14.0), (-6.0, 1.0)] {
        let (side, widths) = widths_from_negative(&values, extent).unwrap();
        let drawn = if side == Side::Lower {
            extent.0
        } else {
            extent.1
        };
        let at = pinned_to_negative(widths, &values).unwrap();
        assert!((at - drawn).abs() < 1e-9, "{extent:?}: pinned to {at}");
    }
}

#[test]
fn an_edge_is_within_the_negative_up_to_its_95th_percentile() {
    assert!(within_the_negative(1.6));
    assert!(within_the_negative(-1.0));
    assert!(!within_the_negative(1.7));
}

#[test]
fn a_pinned_side_sets_how_far_the_gate_slides() {
    let carry = |pinned| Carry {
        extent: (100.0, 300.0),
        open: Open::default(),
        to: (150.0, 400.0),
        pinned,
    };
    assert_eq!(carry(Some(Side::Lower)).shift(), 50.0);
    assert_eq!(carry(Some(Side::Upper)).shift(), 100.0);
    assert_eq!(carry(None).shift(), 75.0);
    let moved = slid(
        &rectangle((100.0, 300.0), (0.0, 50.0)),
        &params(),
        &carry(Some(Side::Lower)),
        &standing((0.0, 50.0)),
    )
    .unwrap();
    assert_eq!(x_span(&moved), (150.0, 350.0));
}

#[test]
fn halves_take_alternate_events_and_renumber_their_members() {
    let points: Vec<(f64, f64)> = (0..5).map(|i| (i as f64, 0.0)).collect();
    let [even, odd] = halves(&points, &[1, 2, 4]);
    assert_eq!(even.0, [(0.0, 0.0), (2.0, 0.0), (4.0, 0.0)]);
    assert_eq!(even.1, [1, 2]);
    assert_eq!(odd.0, [(1.0, 0.0), (3.0, 0.0)]);
    assert_eq!(odd.1, [0]);
}

/// Spans of a gate on both axes, carried to `x` and `y`.
fn spans(x: (f64, f64), y: (f64, f64)) -> [Carry; 2] {
    [
        Carry {
            extent: x,
            open: Open::default(),
            to: x,
            pinned: None,
        },
        Carry {
            extent: y,
            open: Open::default(),
            to: y,
            pinned: None,
        },
    ]
}

#[test]
fn agreement_is_the_share_of_cells_either_placement_holds_that_both_hold() {
    // Cells at x = 0..=100 and y = 0: [10, 50] holds 41 of them, [30, 70]
    // 41, and both the 21 from 30 to 50, of the 61 from 10 to 70.
    let points: Vec<(f64, f64)> = (0..=100).map(|i| (i as f64, 0.0)).collect();
    let first = spans((10.0, 50.0), (-1.0, 1.0));
    let second = spans((30.0, 70.0), (-1.0, 1.0));
    assert_eq!(agreement(&first, &second, &points), 21.0 / 61.0);
    assert_eq!(agreement(&first, &first, &points), 1.0);
    // Off the plot's other axis, neither holds anything to disagree about.
    let elsewhere = spans((10.0, 50.0), (5.0, 6.0));
    assert_eq!(agreement(&elsewhere, &elsewhere, &points), 1.0);
    assert_eq!(
        agreement(&first, &spans((10.0, 50.0), (5.0, 6.0)), &points),
        0.0
    );
}
