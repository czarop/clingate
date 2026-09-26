//! Random edits to single gates: dragging them about and pulling their
//! handles.
//!
//! Each gate type has hand-written tests for particular drags. These ask two
//! things of every type at once, over many random edits: a drag and the
//! opposite drag leave the gate holding exactly the cells it held before,
//! and no sequence of handle moves panics or leaves a coordinate that is not
//! a number.

#![cfg(test)]

use super::gate_drag::GateDragData;
use super::gate_traits::DrawableGate;
use super::rescale_tests::{
    OLD, bisector, ellipse, line, mapper, membership, quadrant, rectangle, skewed, tilted_ellipse,
    triangle,
};
use flow_gates::GateGeometry;
use rand::prelude::*;
use std::sync::Arc;

fn every_kind() -> Vec<Arc<dyn DrawableGate>> {
    vec![
        rectangle("r", &OLD),
        triangle("p", &OLD),
        ellipse("e", &OLD),
        tilted_ellipse("t", &OLD),
        line("l", &OLD),
        quadrant("q", &OLD),
        bisector("b", &OLD),
        skewed("s", &OLD),
    ]
}

fn dragged(gate: &Arc<dyn DrawableGate>, by: (f32, f32)) -> Option<Arc<dyn DrawableGate>> {
    let data = GateDragData::new(gate.get_id(), (0.0, 0.0), by);
    gate.replace_points(data)
        .unwrap_or_else(|e| panic!("{} cannot be dragged: {e}", gate.get_id()))
        .map(Arc::from)
}

/// Every coordinate every piece of the gate holds.
fn coordinates(gate: &Arc<dyn DrawableGate>) -> Vec<f32> {
    let pieces: Vec<Option<Arc<str>>> = if gate.is_composite() {
        gate.get_inner_gate_ids().into_iter().map(Some).collect()
    } else {
        vec![None]
    };
    let (x, y) = gate.get_params();
    let mut out = Vec::new();
    for piece in pieces {
        let Some(inner) = gate.get_gate_ref(piece.as_deref()) else {
            continue;
        };
        match &inner.geometry {
            GateGeometry::Rectangle { min, max } => {
                for node in [min, max] {
                    out.extend(node.get_coordinate(&x));
                    out.extend(node.get_coordinate(&y));
                }
            }
            GateGeometry::Polygon { nodes, .. } => {
                for node in nodes {
                    out.extend(node.get_coordinate(&x));
                    out.extend(node.get_coordinate(&y));
                }
            }
            GateGeometry::Ellipse {
                center,
                radius_x,
                radius_y,
                angle,
            } => {
                out.extend(center.get_coordinate(&x));
                out.extend(center.get_coordinate(&y));
                out.extend([*radius_x, *radius_y, *angle]);
            }
            GateGeometry::Boolean { .. } => {}
        }
    }
    out
}

/// How many grab handles the gate draws when selected - the indices a
/// person can actually pull.
fn handles_of(
    gate: &Arc<dyn DrawableGate>,
    map: &crate::gate_editor::plots::axis_store::PlotMapper,
) -> usize {
    gate.draw_self(true, None, map, &None)
        .iter()
        .filter(|s| matches!(s, super::gate_types::GateRenderShape::Handle { .. }))
        .count()
}

#[test]
fn a_drag_and_the_opposite_drag_hold_the_same_cells() {
    let mut rng = StdRng::seed_from_u64(11);
    for gate in every_kind() {
        for _ in 0..25 {
            let by = (rng.random_range(-1.5..1.5), rng.random_range(-150.0..150.0));
            let Some(there) = dragged(&gate, by) else {
                // This kind is not dragged whole.
                break;
            };
            let back = dragged(&there, (-by.0, -by.1)).expect("draggable once, draggable again");
            assert_eq!(
                membership(&back, &OLD),
                membership(&gate, &OLD),
                "{} changed after a drag of {by:?} and back",
                gate.get_id()
            );
        }
    }
}

#[test]
fn a_drag_moves_the_cells_a_gate_holds() {
    // Guard for the test above: a drag that did nothing would pass it.
    for gate in every_kind() {
        // Dragging a bisector whole slides its handle along the split, which
        // is where its label sits - it moves no cells, by design; its split
        // moves by its handle. So it has nothing to show here.
        if &*gate.get_id() == "b" {
            continue;
        }
        if let Some(moved) = dragged(&gate, (1.0, 120.0)) {
            assert_ne!(
                membership(&moved, &OLD),
                membership(&gate, &OLD),
                "{} did not move",
                gate.get_id()
            );
        }
    }
}

#[test]
fn random_handle_moves_never_leave_a_coordinate_that_is_not_a_number() {
    let map = mapper(&OLD);
    let (x_range, y_range) = (map.x_axis_min_max(), map.y_axis_min_max());
    let mut rng = StdRng::seed_from_u64(12);
    for original in every_kind() {
        let mut gate = original.clone();
        for step in 0..200 {
            let at = (
                rng.random_range(*x_range.start()..*x_range.end()),
                rng.random_range(*y_range.start()..*y_range.end()),
            );
            // Only the handles the gate draws can be grabbed.
            let handles = handles_of(&gate, &map);
            if handles == 0 {
                break;
            }
            let handle = rng.random_range(0..handles);
            // A refused move is fine; a panic or a NaN is not.
            if let Ok(next) = gate.replace_point(at, handle, None, &map) {
                gate = Arc::from(next);
            }
            let bad: Vec<f32> = coordinates(&gate)
                .into_iter()
                .filter(|v| !v.is_finite())
                .collect();
            assert!(
                bad.is_empty(),
                "{} after {step} handle moves holds {bad:?}",
                original.get_id()
            );
        }
    }
}
