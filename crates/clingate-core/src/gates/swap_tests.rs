//! Tests for showing a gate on a plot with its axes the other way round.
//!
//! A gate drawn on CD3 against SSC-A is shown on SSC-A against CD3 by
//! transposing it. What must hold is what a person relies on: an event is
//! inside the transposed gate exactly when it was inside the original - and
//! for a composite, inside the same named piece, since a quarter is a
//! population, not a corner of the screen.

#![cfg(test)]

use super::gate_traits::DrawableGate;
use super::rescale_tests::{
    OLD, X, Y, bisector, ellipse, events, line, quadrant, rectangle, shown, skewed, tilted_ellipse,
    triangle,
};
use std::sync::Arc;

/// Which named piece of `gate` holds each event, with the event's
/// coordinates given in the order the gate's axes are.
fn pieces(gate: &Arc<dyn DrawableGate>, transposed: bool) -> Vec<Option<Arc<str>>> {
    let regions: Vec<Option<Arc<str>>> = if gate.is_composite() {
        gate.get_inner_gate_ids().into_iter().map(Some).collect()
    } else {
        vec![None]
    };
    let (first, second) = gate.get_params();
    events()
        .iter()
        .map(|(x_raw, y)| {
            let x = shown(&OLD, *x_raw);
            let (a, b) = if transposed { (*y, x) } else { (x, *y) };
            regions
                .iter()
                .find(|region| {
                    gate.get_gate_ref(region.as_deref()).is_some_and(|g| {
                        g.geometry
                            .contains_point(a, b, &first, &second)
                            .unwrap_or(false)
                    })
                })
                .map(|region| region.clone().unwrap_or_else(|| Arc::from("inside")))
        })
        .collect()
}

fn swapped(gate: &Arc<dyn DrawableGate>) -> Arc<dyn DrawableGate> {
    let turned = gate
        .match_to_plot_axis(Y, X)
        .unwrap_or_else(|e| panic!("{} cannot be transposed: {e}", gate.get_id()))
        .unwrap_or_else(|| panic!("{} transposed to itself", gate.get_id()));
    Arc::from(turned)
}

fn holds_the_same_events(gate: Arc<dyn DrawableGate>) {
    let turned = swapped(&gate);
    assert_eq!(
        turned.get_params(),
        (Arc::from(Y), Arc::from(X)),
        "{}: the axes are the other way round",
        gate.get_id()
    );
    let (before, after) = (pieces(&gate, false), pieces(&turned, true));
    assert!(
        before.iter().any(Option::is_some),
        "{}: the fixture should hold something",
        gate.get_id()
    );
    let moved = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    assert_eq!(moved, 0, "{}: {moved} events changed piece", gate.get_id());
}

#[test]
fn a_transposed_rectangle_holds_the_same_events() {
    holds_the_same_events(rectangle("r", &OLD));
}

#[test]
fn a_transposed_polygon_holds_the_same_events() {
    holds_the_same_events(triangle("p", &OLD));
}

#[test]
fn a_transposed_ellipse_holds_the_same_events() {
    holds_the_same_events(ellipse("e", &OLD));
    holds_the_same_events(tilted_ellipse("t", &OLD));
}

#[test]
fn a_transposed_line_gate_holds_the_same_events() {
    holds_the_same_events(line("l", &OLD));
}

#[test]
fn a_transposed_quadrant_keeps_each_event_in_the_same_quarter() {
    holds_the_same_events(quadrant("q", &OLD));
}

#[test]
fn a_transposed_skewed_quadrant_keeps_each_event_in_the_same_quarter() {
    holds_the_same_events(skewed("s", &OLD));
}

#[test]
fn a_transposed_bisector_keeps_each_event_on_the_same_side() {
    holds_the_same_events(bisector("b", &OLD));
}

#[test]
fn transposing_twice_gives_back_the_same_gate() {
    for gate in [
        rectangle("r", &OLD),
        triangle("p", &OLD),
        ellipse("e", &OLD),
        quadrant("q", &OLD),
    ] {
        let back = swapped(&gate)
            .match_to_plot_axis(X, Y)
            .unwrap()
            .map(Arc::from)
            .expect("transposes back");
        assert_eq!(
            pieces(&gate, false),
            pieces(&back, false),
            "{} changed on the way round",
            gate.get_id()
        );
    }
}
