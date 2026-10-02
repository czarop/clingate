//! Keeping a gate apart from the others on its plot as it is edited, each
//! place worked by hand.

use std::sync::Arc;

use flow_gates::{GateGeometry, create_polygon_geometry, create_rectangle_geometry};

use crate::gate_rules::clearance::kept_apart;
use crate::gates::gate_contact::{Point, outline};
use crate::gates::gate_single::ellipse_gate::EllipseGate;
use crate::gates::gate_single::polygon_gate::PolygonGate;
use crate::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gates::gate_traits::DrawableGate;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

fn gate(id: &str, geometry: GateGeometry) -> flow_gates::Gate {
    flow_gates::Gate {
        id: Arc::from(id),
        name: id.to_string(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    }
}

fn rectangle(id: &str, (x0, y0): Point, (x1, y1): Point) -> Arc<dyn DrawableGate> {
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let points = corners.map(|(x, y)| (x as f32, y as f32)).to_vec();
    let geometry = create_rectangle_geometry(points, X, Y).unwrap();
    Arc::new(RectangleGate::try_new(gate(id, geometry), true).unwrap())
}

fn polygon(id: &str, points: &[Point]) -> Arc<dyn DrawableGate> {
    let points = points.iter().map(|(x, y)| (*x as f32, *y as f32)).collect();
    let geometry = create_polygon_geometry(points, X, Y).unwrap();
    Arc::new(PolygonGate::try_new(gate(id, geometry), true).unwrap())
}

fn ellipse(id: &str, centre: Point, radius: f64) -> Arc<dyn DrawableGate> {
    let (x, y, r) = (centre.0, centre.1, radius);
    let geometry = crate::omiq::deserialise::create_omiq_ellipse_geometry(
        (x - r, y),
        (x + r, y),
        (x, y + r),
        X,
        Y,
    )
    .unwrap();
    Arc::new(EllipseGate::try_new(gate(id, geometry), true).unwrap())
}

fn points_of(gate: &Arc<dyn DrawableGate>) -> Vec<Point> {
    outline(gate, &gate.get_id(), X, Y).unwrap()
}

fn assert_near(gate: &Arc<dyn DrawableGate>, expected: &[Point]) {
    let got = points_of(gate);
    assert_eq!(got.len(), expected.len(), "{got:?}");
    for (g, e) in got.iter().zip(expected) {
        assert!(
            (g.0 - e.0).abs() < 1e-2 && (g.1 - e.1).abs() < 1e-2,
            "expected {expected:?}, got {got:?}"
        );
    }
}

fn square_at(x: f64) -> [Point; 4] {
    [(x, 0.0), (x + 100.0, 0.0), (x + 100.0, 100.0), (x, 100.0)]
}

#[test]
fn a_gate_moved_into_another_stops_touching_it() {
    let from = rectangle("a", (0.0, 0.0), (100.0, 100.0));
    let moved = rectangle("a", (150.0, 0.0), (250.0, 100.0));
    let other = rectangle("b", (200.0, 0.0), (300.0, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[from.clone(), other]);

    assert_near(&kept, &square_at(100.0));
}

#[test]
fn a_quick_move_cannot_jump_over_another_gate() {
    let from = rectangle("a", (0.0, 0.0), (100.0, 100.0));
    let moved = rectangle("a", (400.0, 0.0), (500.0, 100.0));
    let other = rectangle("b", (200.0, 0.0), (300.0, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[other]);

    assert_near(&kept, &square_at(100.0));
}

#[test]
fn a_move_clear_of_every_gate_goes_all_the_way() {
    let from = rectangle("a", (0.0, 0.0), (100.0, 100.0));
    let moved = rectangle("a", (-50.0, 0.0), (50.0, 100.0));
    let other = rectangle("b", (200.0, 0.0), (300.0, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[other]);

    assert!(Arc::ptr_eq(&kept, &moved));
}

#[test]
fn with_no_gates_to_keep_apart_from_a_move_goes_all_the_way() {
    let from = rectangle("a", (0.0, 0.0), (100.0, 100.0));
    let moved = rectangle("a", (250.0, 0.0), (350.0, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[]);

    assert!(Arc::ptr_eq(&kept, &moved));
}

/// One drawn over another can still be moved, and moved off it.
#[test]
fn a_gate_already_over_another_is_not_held_by_it() {
    let from = rectangle("a", (0.0, 0.0), (100.0, 100.0));
    let moved = rectangle("a", (30.0, 0.0), (130.0, 100.0));
    let under = rectangle("c", (50.0, 0.0), (150.0, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[under]);

    assert!(Arc::ptr_eq(&kept, &moved));
}

/// Touching is not overlapping: a gate held against another slides along it.
#[test]
fn a_gate_touching_another_slides_along_it() {
    let from = rectangle("a", (100.0, 0.0), (200.0, 100.0));
    let moved = rectangle("a", (100.0, 50.0), (200.0, 150.0));
    let beside = rectangle("b", (200.0, 0.0), (300.0, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[beside]);

    assert!(Arc::ptr_eq(&kept, &moved));
}

/// The dragged point (100, 0) heads for (250, 50) and meets the other
/// gate's left side, x = 200, two thirds of the way: at (200, 33.3).
#[test]
fn a_point_dragged_into_another_gate_stops_on_its_edge() {
    let from = polygon("p", &[(0.0, 0.0), (100.0, 0.0), (50.0, 100.0)]);
    let moved = polygon("p", &[(0.0, 0.0), (250.0, 50.0), (50.0, 100.0)]);
    let other = rectangle("b", (200.0, 0.0), (300.0, 100.0));

    let kept = kept_apart(&from, &moved, &"p".into(), &[other]);

    assert_near(&kept, &[(0.0, 0.0), (200.0, 100.0 / 3.0), (50.0, 100.0)]);
}

/// An ellipse is not moved part way: it stays put or goes all the way.
#[test]
fn an_ellipse_moved_into_another_gate_stays_where_it_was() {
    let from = ellipse("e", (50.0, 50.0), 50.0);
    let into = ellipse("e", (200.0, 50.0), 50.0);
    let clear = ellipse("e", (40.0, 50.0), 50.0);
    let other = rectangle("b", (200.0, 0.0), (300.0, 100.0));

    let others = [other];
    assert!(Arc::ptr_eq(
        &kept_apart(&from, &into, &"e".into(), &others),
        &from
    ));
    assert!(Arc::ptr_eq(
        &kept_apart(&from, &clear, &"e".into(), &others),
        &clear
    ));
}

/// A neighbour 2 wide, a long way short of a sixteenth of a move of 10,000:
/// still not stepped over.
#[test]
fn a_long_move_cannot_jump_over_a_thin_gate() {
    let from = rectangle("a", (0.0, 0.0), (100.0, 100.0));
    let moved = rectangle("a", (10_000.0, 0.0), (10_100.0, 100.0));
    let thin = rectangle("b", (530.0, 0.0), (532.0, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[thin]);

    assert_near(&kept, &square_at(430.0));
}

/// Both gates 2 wide: the moving one, from 10 to 1,010, has a window of
/// only 4 across which it overlaps the other - stepped a 1 at a time, as
/// the narrowest gate asks, it cannot miss it.
#[test]
fn a_thin_gate_moved_far_cannot_jump_another_thin_gate() {
    let from = rectangle("a", (10.0, 0.0), (12.0, 100.0));
    let moved = rectangle("a", (1010.0, 0.0), (1012.0, 100.0));
    let thin = rectangle("b", (530.5, 0.0), (532.5, 100.0));

    let kept = kept_apart(&from, &moved, &"a".into(), &[thin]);

    assert_near(
        &kept,
        &[(528.5, 0.0), (530.5, 0.0), (530.5, 100.0), (528.5, 100.0)],
    );
}

/// The notch at (50, 40) filled by deleting its point would cover the gate
/// sitting in it. A change of how many points the gate has cannot be made
/// part way, so the gate stays as it was.
#[test]
fn a_point_deleted_into_a_gate_kept_apart_leaves_the_gate_as_it_was() {
    let notched = [
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (60.0, 100.0),
        (50.0, 40.0),
        (40.0, 100.0),
        (0.0, 100.0),
    ];
    let from = polygon("p", &notched);
    let filled = polygon(
        "p",
        &[
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 100.0),
            (60.0, 100.0),
            (40.0, 100.0),
            (0.0, 100.0),
        ],
    );
    let in_the_notch = rectangle("n", (48.0, 70.0), (52.0, 95.0));

    let kept = kept_apart(&from, &filled, &"p".into(), &[in_the_notch]);

    assert!(Arc::ptr_eq(&kept, &from));
}
