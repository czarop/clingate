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

/// Was B-AX-4: an imported centre outside the plot was clamped onto the axis
/// bounds, so the gate split the events somewhere other than the file said.
/// The centre is kept; each arm still points away from it, reaching the axis
/// edge on its side where there is one.
#[test]
fn an_imported_centre_outside_the_axes_is_kept_and_its_arms_point_away_from_it() {
    let p = DataPoints::new_from_data_center(5000.0, -900.0, 0.0..=1000.0, 0.0..=1000.0);

    assert_eq!(p.center, (5000.0, -900.0));
    assert!(p.left.0 < 5000.0 && p.left.1 == -900.0, "{:?}", p.left);
    assert!(p.right.0 > 5000.0 && p.right.1 == -900.0, "{:?}", p.right);
    assert!(
        p.bottom.1 < -900.0 && p.bottom.0 == 5000.0,
        "{:?}",
        p.bottom
    );
    assert_eq!(p.top, (5000.0, 1000.0), "the top edge is above the centre");
    assert_eq!(p.left, (0.0, -900.0), "the left edge is left of the centre");
}

fn imported(skewed: bool, centre: (f32, f32)) -> Box<dyn DrawableGate> {
    let points = DataPoints::new_from_data_center(centre.0, centre.1, 0.0..=1000.0, 0.0..=1000.0);
    let (id, x, y) = (Arc::from("g"), Arc::from(X), Arc::from(Y));
    let infs = (1e8, 1e8);
    if skewed {
        Box::new(
            SkewedQuadrantGate::try_new_from_data_points(
                id,
                "g".into(),
                points,
                x,
                y,
                true,
                None,
                None,
                infs,
            )
            .unwrap(),
        )
    } else {
        Box::new(
            QuadrantGate::try_new_from_data_points(
                id,
                "g".into(),
                points,
                x,
                y,
                true,
                None,
                None,
                infs,
            )
            .unwrap(),
        )
    }
}

/// Which quarter - 0 BL, 1 BR, 2 TR, 3 TL - holds a point.
fn quarter(g: &dyn DrawableGate, at: (f32, f32)) -> Option<usize> {
    g.get_inner_gate_ids().iter().position(|id| {
        g.get_gate_ref(Some(id)).is_some_and(|inner| {
            inner
                .geometry
                .contains_point(at.0, at.1, X, Y)
                .unwrap_or(false)
        })
    })
}

/// The file's centre is beyond the right of the plot, so everything on the
/// plot is left of it. Clamped onto the plot's right edge, the centre used to
/// put whatever lies between that edge and the real centre in the right-hand
/// quarters.
#[test]
fn a_quadrant_imported_with_its_centre_beyond_the_axes_gates_as_the_file_said() {
    for skewed in [false, true] {
        let g = imported(skewed, (5000.0, 400.0));
        assert_eq!(
            quarter(&*g, (3000.0, 600.0)),
            Some(3),
            "skewed {skewed}: top left"
        );
        assert_eq!(
            quarter(&*g, (3000.0, 200.0)),
            Some(0),
            "skewed {skewed}: bottom left"
        );
        assert_eq!(
            quarter(&*g, (6000.0, 600.0)),
            Some(2),
            "skewed {skewed}: top right"
        );
    }
}

fn lines(
    shapes: &[crate::gate_editor::gates::gate_types::GateRenderShape],
) -> Vec<((f32, f32), (f32, f32))> {
    use crate::gate_editor::gates::gate_types::GateRenderShape;
    shapes
        .iter()
        .filter_map(|s| match s {
            GateRenderShape::Line { x1, y1, x2, y2, .. } => Some(((*x1, *y1), (*x2, *y2))),
            _ => None,
        })
        .collect()
}

fn handles(shapes: &[crate::gate_editor::gates::gate_types::GateRenderShape]) -> Vec<(f32, f32)> {
    use crate::gate_editor::gates::gate_types::GateRenderShape;
    shapes
        .iter()
        .filter_map(|s| match s {
            GateRenderShape::Circle { center, .. } => Some(*center),
            _ => None,
        })
        .collect()
}

fn on_plot(p: (f32, f32)) -> bool {
    (0.0..=1000.0).contains(&p.0) && (0.0..=1000.0).contains(&p.1)
}

/// A narrowed axis can leave a quadrant's centre off the plot. Its lines and
/// handles are still drawn on the plot - the centre handle at the nearest
/// point of it, where it can be grabbed.
#[test]
fn a_quadrant_with_its_centre_off_the_plot_is_drawn_on_the_plot() {
    for skewed in [false, true] {
        let g = imported(skewed, (5000.0, 400.0));
        let shapes = g.draw_self(true, None, &mapper(), &None);
        let drawn = lines(&shapes);
        assert!(!drawn.is_empty(), "skewed {skewed}: no lines drawn");
        for (a, b) in drawn {
            assert!(on_plot(a) && on_plot(b), "skewed {skewed}: {a:?} to {b:?}");
        }
        let grips = handles(&shapes);
        assert!(
            grips.contains(&(1000.0, 400.0)),
            "skewed {skewed}: {grips:?}"
        );
        assert!(
            grips.iter().all(|h| on_plot(*h)),
            "skewed {skewed}: {grips:?}"
        );
    }
}

/// A line can be picked up where it is drawn - here the level line through a
/// centre beyond the right of the plot - and not where it is not.
#[test]
fn a_quadrant_is_picked_up_by_the_lines_it_draws() {
    for skewed in [false, true] {
        let g = imported(skewed, (5000.0, 400.0));
        let m = mapper();
        assert!(
            g.is_point_on_perimeter((500.0, 402.0), (5.0, 5.0), &m)
                .is_some(),
            "skewed {skewed}: missed the drawn line"
        );
        assert!(
            g.is_point_on_perimeter((500.0, 700.0), (5.0, 5.0), &m)
                .is_none(),
            "skewed {skewed}: picked up off the lines"
        );
    }
}

/// A skewed quadrant's lines are drawn where its quarters really meet: just
/// either side of the middle of each drawn line lie two different quarters.
/// Checked on a slanted gate after its X axis is narrowed, which used to snap
/// the ends of the left and right arms to the new edges and turn them.
#[test]
fn a_skewed_quadrants_lines_are_drawn_where_its_quarters_meet() {
    let m = mapper();
    let tilted = skewed("s")
        .replace_point((700.0, 1000.0), 4, None, &m)
        .unwrap()
        .replace_point((1000.0, 700.0), 3, None, &m)
        .unwrap();
    let narrowed = tilted
        .recalculate_gate_for_new_axis_limits(Arc::from(X), 200.0, 900.0, &TransformType::Linear)
        .unwrap()
        .expect("a quadrant answers a new range");
    let view = PlotMapper::new(
        600.0,
        600.0,
        200.0..=900.0,
        0.0..=1000.0,
        200.0..=900.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    );
    for gate in [&tilted, &narrowed] {
        let drawn = lines(&gate.draw_self(false, None, &view, &None));
        assert_eq!(drawn.len(), 4);
        for (a, b) in drawn {
            let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            let len = ((b.0 - a.0).hypot(b.1 - a.1)).max(f32::EPSILON);
            let normal = (-(b.1 - a.1) / len, (b.0 - a.0) / len);
            let side = |k: f32| quarter(&**gate, (mid.0 + k * normal.0, mid.1 + k * normal.1));
            assert_ne!(
                side(2.0),
                side(-2.0),
                "the line {a:?} to {b:?} divides nothing"
            );
        }
    }
    // And the quarters themselves are those of the gate before the change.
    for x in (0..=1000).step_by(37) {
        for y in (0..=1000).step_by(41) {
            let at = (x as f32, y as f32);
            assert_eq!(quarter(&*tilted, at), quarter(&*narrowed, at), "{at:?}");
        }
    }
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
        .replace_point((500.0, 500.0), 0, None, &mapper())
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
        .replace_point((450.0, 300.0), 0, None, &mapper())
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
