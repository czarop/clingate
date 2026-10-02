//! Where a gate next to another goes, and the outline helpers it is worked
//! out with - every place and point worked by hand.

use std::sync::Arc;

use super::*;
use crate::gate_rules::rule_store::RuleTarget;
use crate::gates::gate_single::polygon_gate::PolygonGate;
use crate::gates::gate_single::rectangle_gate::RectangleGate;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

fn flow_gate(geometry: GateGeometry) -> flow_gates::Gate {
    flow_gates::Gate {
        id: Arc::from("g"),
        name: "G".into(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    }
}

fn rectangle((x0, x1): (f32, f32), (y0, y1): (f32, f32)) -> Arc<dyn DrawableGate> {
    let corners = vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let geometry = flow_gates::create_rectangle_geometry(corners, X, Y).unwrap();
    Arc::new(RectangleGate::try_new(flow_gate(geometry), true).unwrap())
}

fn polygon(points: &[(f32, f32)]) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_polygon_geometry(points.to_vec(), X, Y).unwrap();
    Arc::new(PolygonGate::try_new(flow_gate(geometry), true).unwrap())
}

/// The outline of a rectangle, as `outline` gives it.
fn boxed((x0, x1): (f64, f64), (y0, y1): (f64, f64)) -> Vec<Point> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

fn rule(parameter: &str, side: Side, meet: Meet, gap: f64) -> NextToRule {
    NextToRule {
        anchor: RuleTarget::named("A"),
        parameter: Arc::from(parameter),
        side,
        meet,
        gap,
    }
}

fn placed(gate: &Arc<dyn DrawableGate>, against: &[Point], rule: &NextToRule) -> Vec<Point> {
    let moved = placed_next_to(gate, &Arc::from("g"), against, rule).unwrap();
    outline(&moved, &Arc::from("g"), X, Y).unwrap()
}

fn assert_near(got: &[Point], expected: &[Point]) {
    assert_eq!(
        got.len(),
        expected.len(),
        "expected {expected:?}, got {got:?}"
    );
    for (g, e) in got.iter().zip(expected) {
        assert!(
            (g.0 - e.0).abs() < 1e-3 && (g.1 - e.1).abs() < 1e-3,
            "expected {expected:?}, got {got:?}"
        );
    }
}

// ── the outline helpers ──────────────────────────────────────────────────

/// The triangle (10, 0), (20, 0), (20, 10): its slanted side is at u 15
/// halfway up, its bottom from 10 to 20.
#[test]
fn the_nearest_point_across_is_read_off_every_side_slanted_or_level() {
    let triangle = [(10.0, 0.0), (20.0, 0.0), (20.0, 10.0)];
    assert_eq!(nearest_at(&triangle, 5.0), Some(15.0));
    assert_eq!(nearest_at(&triangle, 0.0), Some(10.0));
    assert_eq!(nearest_at(&triangle, 11.0), None);
}

/// The chain (0, 0), (10, 10), (4, 20): at 5 on its first piece, at 15 on
/// its second, 10 + (5 / 10) x (4 - 10) = 7.
#[test]
fn the_furthest_point_of_a_chain_is_read_off_the_piece_at_that_level() {
    let chain = [(0.0, 0.0), (10.0, 10.0), (4.0, 20.0)];
    assert_eq!(furthest_at(&chain, 5.0), Some(5.0));
    assert_eq!(furthest_at(&chain, 15.0), Some(7.0));
    assert_eq!(furthest_at(&chain, 10.0), Some(10.0));
    assert_eq!(furthest_at(&chain, 21.0), None);
    let with_a_level_piece = [(0.0, 0.0), (3.0, 5.0), (8.0, 5.0), (0.0, 10.0)];
    assert_eq!(furthest_at(&with_a_level_piece, 5.0), Some(8.0));
}

/// A square either way round: the side towards higher u from its lowest
/// point up, and the side away from its highest point down.
#[test]
fn a_square_s_two_sides_are_the_same_whichever_way_round_it_runs() {
    let near = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let far = vec![(0.0, 10.0), (0.0, 0.0)];
    let anticlockwise = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let clockwise = [(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)];
    assert_eq!(sides(&anticlockwise), (near.clone(), far.clone()));
    assert_eq!(sides(&clockwise), (near, far));
}

// ── sliding ──────────────────────────────────────────────────────────────

/// G is 0 to 100 across and up; A is 300 to 400 across, 50 to 150 up.
#[test]
fn a_gate_slides_up_to_the_other_from_wherever_it_starts() {
    let against = boxed((300.0, 400.0), (50.0, 150.0));
    let slide = rule(X, Side::Lower, Meet::Slide, 0.0);
    for start in [(0.0, 100.0), (250.0, 350.0), (500.0, 600.0)] {
        let got = placed(&rectangle(start, (0.0, 100.0)), &against, &slide);
        assert_near(&got, &boxed((200.0, 300.0), (0.0, 100.0)));
    }
    let gapped = rule(X, Side::Lower, Meet::Slide, 10.0);
    let got = placed(&rectangle((0.0, 100.0), (0.0, 100.0)), &against, &gapped);
    assert_near(&got, &boxed((190.0, 290.0), (0.0, 100.0)));
}

#[test]
fn a_gate_on_the_upper_side_slides_down_to_the_other_s_far_edge() {
    let against = boxed((300.0, 400.0), (50.0, 150.0));
    let slide = rule(X, Side::Upper, Meet::Slide, 0.0);
    for start in [(0.0, 100.0), (500.0, 600.0)] {
        let got = placed(&rectangle(start, (0.0, 100.0)), &against, &slide);
        assert_near(&got, &boxed((400.0, 500.0), (0.0, 100.0)));
    }
}

#[test]
fn a_gate_slides_up_and_down_the_other_axis_too() {
    let against = boxed((50.0, 150.0), (300.0, 400.0));
    let gate = rectangle((0.0, 100.0), (0.0, 100.0));
    let below = placed(&gate, &against, &rule(Y, Side::Lower, Meet::Slide, 0.0));
    assert_near(&below, &boxed((0.0, 100.0), (200.0, 300.0)));
    let above = placed(&gate, &against, &rule(Y, Side::Upper, Meet::Slide, 0.0));
    assert_near(&above, &boxed((0.0, 100.0), (400.0, 500.0)));
}

/// A's left side slants from (300, 0) up to (200, 100): G's top right
/// corner meets it first, at 200.
#[test]
fn a_gate_slides_until_its_nearest_corner_meets_a_slanted_side() {
    let against = vec![(300.0, 0.0), (400.0, 0.0), (400.0, 100.0), (200.0, 100.0)];
    let gate = rectangle((0.0, 100.0), (0.0, 100.0));
    let got = placed(&gate, &against, &rule(X, Side::Lower, Meet::Slide, 0.0));
    assert_near(&got, &boxed((100.0, 200.0), (0.0, 100.0)));
}

#[test]
fn a_gate_never_level_with_the_other_is_refused() {
    let against = boxed((300.0, 400.0), (500.0, 600.0));
    let gate = rectangle((0.0, 100.0), (0.0, 100.0));
    let said = placed_next_to(
        &gate,
        &Arc::from("g"),
        &against,
        &rule(X, Side::Lower, Meet::Slide, 0.0),
    )
    .err()
    .unwrap();
    assert!(said.contains("never comes level with A"), "{said}");
}

// ── an edge open to the end of an axis ───────────────────────────────────

#[test]
fn a_gate_open_to_the_end_of_one_axis_is_refused_and_one_reaching_1e9_is_not() {
    let against = boxed((300.0, 400.0), (0.0, 100.0));
    let slide = rule(X, Side::Lower, Meet::Slide, 0.0);
    let open = rectangle((0.0, 100.0), (0.0, 1e16));
    let said = placed_next_to(&open, &Arc::from("g"), &against, &slide)
        .err()
        .unwrap();
    assert!(said.contains("runs to the end of an axis"), "{said}");

    let tall = rectangle((0.0, 100.0), (0.0, 1e9));
    let got = placed(&tall, &against, &slide);
    assert_near(&got, &boxed((200.0, 300.0), (0.0, 1e9)));
}

// ── growing ──────────────────────────────────────────────────────────────

/// G as a polygon to the right of A: its left side comes down to A's right
/// edge at 200, its right side stays at 600.
#[test]
fn a_polygon_on_the_upper_side_grows_its_low_side_down_to_the_other() {
    let gate = polygon(&[(500.0, 0.0), (600.0, 0.0), (600.0, 100.0), (500.0, 100.0)]);
    let against = boxed((100.0, 200.0), (0.0, 100.0));
    let got = placed(&gate, &against, &rule(X, Side::Upper, Meet::GrowSide, 0.0));
    assert_near(
        &got,
        &[(200.0, 0.0), (600.0, 0.0), (600.0, 100.0), (200.0, 100.0)],
    );
}

/// The peak at (50, 150) sits at the middle of G across, so it is not on
/// the facing side and stays where it is.
#[test]
fn a_point_at_the_middle_of_the_gate_is_not_on_its_facing_side() {
    let gate = polygon(&[
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (50.0, 150.0),
        (0.0, 100.0),
    ]);
    let against = boxed((300.0, 400.0), (-50.0, 200.0));
    let got = placed(&gate, &against, &rule(X, Side::Lower, Meet::GrowSide, 0.0));
    assert_near(
        &got,
        &[
            (0.0, 0.0),
            (300.0, 0.0),
            (300.0, 100.0),
            (50.0, 150.0),
            (0.0, 100.0),
        ],
    );
}

/// G runs 0 to 300 and A starts at 250: G's right side comes back to 250.
#[test]
fn a_polygon_over_the_other_draws_its_facing_side_back_to_it() {
    let gate = polygon(&[(0.0, 0.0), (300.0, 0.0), (300.0, 100.0), (0.0, 100.0)]);
    let against = boxed((250.0, 400.0), (0.0, 100.0));
    let got = placed(&gate, &against, &rule(X, Side::Lower, Meet::GrowSide, 0.0));
    assert_near(
        &got,
        &[(0.0, 0.0), (250.0, 0.0), (250.0, 100.0), (0.0, 100.0)],
    );
}

// ── following the outline ────────────────────────────────────────────────

/// G to the right of A, whose right side slants from (200, 0) up to
/// (300, 100): G's left side takes that slant, its right side stays.
#[test]
fn a_polygon_on_the_upper_side_follows_the_other_s_slanted_side() {
    let gate = polygon(&[(500.0, 0.0), (600.0, 0.0), (600.0, 100.0), (500.0, 100.0)]);
    let against = vec![(100.0, 0.0), (200.0, 0.0), (300.0, 100.0), (100.0, 100.0)];
    let got = placed(
        &gate,
        &against,
        &rule(X, Side::Upper, Meet::FollowOutline, 0.0),
    );
    assert_near(
        &got,
        &[
            (500.0, 0.0),
            (200.0, 0.0),
            (300.0, 100.0),
            (500.0, 100.0),
            (600.0, 100.0),
            (600.0, 0.0),
        ],
    );
}

/// CD19- left of CD19+ on CD19, as on the plot that showed the fault, in plot
/// units. CD19+'s top edge rises to its far right corner (5.2, 0.72) and its
/// bottom falls to (5.0, -0.36), so its highest and lowest points are on its
/// far side; only its left edge, (0.2, 0.36) to (0.21, -0.33), faces CD19-.
/// Its slanted top-left edge runs 1.45 along u for 0.3 across, against the
/// two gates' 5.8 by 5.3, so it is more a top than a side. CD19-'s right side
/// follows the left edge, goes up at 0.21 to CD19+'s top, and steps back to
/// where it was: at -0.36 and 0.72 its slanted side is at
/// 0.52 - 0.05 * 0.34 / 3 and 0.52 - 0.05 * 1.42 / 3. It does not wrap round
/// CD19+'s top and bottom.
#[test]
fn following_the_outline_goes_no_further_than_the_other_s_facing_side() {
    let gate = polygon(&[
        (-0.6, -0.7),
        (-0.6, 4.5),
        (-0.1, 4.6),
        (0.5, 4.6),
        (0.47, 2.3),
        (0.52, -0.7),
    ]);
    let against = vec![
        (0.2, 0.36),
        (1.65, 0.66),
        (5.2, 0.72),
        (5.0, -0.36),
        (0.21, -0.33),
    ];
    let got = placed(
        &gate,
        &against,
        &rule(X, Side::Lower, Meet::FollowOutline, 0.0),
    );
    assert_near(
        &got,
        &[
            (-0.6, -0.7),
            (0.52, -0.7),
            (0.514_333, -0.36),
            (0.21, -0.36),
            (0.21, -0.33),
            (0.2, 0.36),
            (0.21, 0.66),
            (0.21, 0.72),
            (0.496_333, 0.72),
            (0.47, 2.3),
            (0.5, 4.6),
            (-0.1, 4.6),
            (-0.6, 4.5),
        ],
    );
}

/// A long flat sliver pointing at the gate: both its edges run 880 along the
/// axis for 1 across, against the two gates' 900 by 100, so it has a top and
/// a bottom but no side to follow.
#[test]
fn following_an_outline_with_no_side_facing_the_gate_is_refused() {
    let gate = polygon(&[(0.0, 0.0), (10.0, 0.0), (10.0, 100.0), (0.0, 100.0)]);
    let against = vec![(20.0, 41.0), (900.0, 40.0), (900.0, 42.0)];
    let refused = placed_next_to(
        &gate,
        &Arc::from("g"),
        &against,
        &rule(X, Side::Lower, Meet::FollowOutline, 0.0),
    )
    .err()
    .expect("refused");
    assert!(refused.contains("no side facing it"), "{refused}");
}

/// A, from 6 to 20 and 0 to 5, has moved onto G, from 0 to 10 and 1 to 10.
/// G's right side follows A's left edge from G's bottom, 1, up to A's top, 5,
/// and steps back out to where it was above A; its bottom stops at A's edge
/// rather than running on inside A to where G's side was.
#[test]
fn following_an_outline_the_other_has_moved_onto_stays_clear_of_it() {
    let gate = polygon(&[(0.0, 1.0), (10.0, 1.0), (10.0, 10.0), (0.0, 10.0)]);
    let against = boxed((6.0, 20.0), (0.0, 5.0));
    let got = placed(
        &gate,
        &against,
        &rule(X, Side::Lower, Meet::FollowOutline, 0.0),
    );
    assert!(
        !crate::gates::gate_contact::overlaps(&got, &against),
        "{got:?}"
    );
    assert_near(
        &got,
        &[
            (6.0, 1.0),
            (6.0, 5.0),
            (10.0, 5.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 1.0),
        ],
    );
    // G from 1 to 4, within A's height at both ends: its right side is A's
    // edge all the way.
    let short = polygon(&[(0.0, 1.0), (10.0, 1.0), (10.0, 4.0), (0.0, 4.0)]);
    let got = placed(
        &short,
        &against,
        &rule(X, Side::Lower, Meet::FollowOutline, 0.0),
    );
    assert!(
        !crate::gates::gate_contact::overlaps(&got, &against),
        "{got:?}"
    );
    assert_near(&got, &[(6.0, 1.0), (6.0, 4.0), (0.0, 4.0), (0.0, 1.0)]);
}
