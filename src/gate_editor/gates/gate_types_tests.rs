//! Tests for the render-shape vocabulary and gate statistics.
//!
//! `GateRenderShape` is the boundary between a gate and the SVG layer, and its
//! `ShapeType` tag decides what the mouse handlers will let the user grab.
//!
//! cargo test gate_types -- --nocapture

#![cfg(test)]

use crate::gate_editor::gates::gate_types::*;
use rustc_hash::FxHashMap;
use std::sync::Arc;

fn gate_id(s: &str) -> Arc<str> {
    Arc::from(s)
}

// ─── PrimaryGateType ──────────────────────────────────────────────────────────

#[test]
fn the_three_multi_part_gates_are_composite() {
    for t in [
        PrimaryGateType::Bisector,
        PrimaryGateType::Quadrant,
        PrimaryGateType::SkewedQuadrant,
    ] {
        assert!(t.is_composite(), "should be composite");
        assert!(!t.is_single());
    }
}

#[test]
fn every_other_gate_type_is_single() {
    for t in [
        PrimaryGateType::Polygon,
        PrimaryGateType::Ellipse,
        PrimaryGateType::Rectangle,
        PrimaryGateType::Line(None),
        PrimaryGateType::Not,
        PrimaryGateType::And,
        PrimaryGateType::Or,
    ] {
        assert!(t.is_single(), "should be single");
        assert!(!t.is_composite());
    }
}

/// A line gate carries the y coordinate it was created at, and that must not
/// change whether it counts as composite.
#[test]
fn a_line_gate_is_single_whether_or_not_it_has_a_height() {
    assert!(PrimaryGateType::Line(None).is_single());
    assert!(PrimaryGateType::Line(Some(3.5)).is_single());
}

// ─── Shape classification ─────────────────────────────────────────────────────

fn circle(shape_type: ShapeType) -> GateRenderShape {
    GateRenderShape::Circle {
        center: (1.0, 2.0),
        radius: 3.0,
        fill: "red",
        shape_type,
    }
}

#[test]
fn composite_shapes_are_recognised() {
    assert!(circle(ShapeType::CompositeGate(gate_id("g"), true)).is_composite());
    assert!(circle(ShapeType::CompositePoint(0, false)).is_composite());
    assert!(circle(ShapeType::UndraggableLine).is_composite());
    assert!(circle(ShapeType::UndraggablePoint(2)).is_composite());
}

#[test]
fn plain_shapes_are_not_composite() {
    assert!(!circle(ShapeType::Gate(gate_id("g"))).is_composite());
    assert!(!circle(ShapeType::Point(0)).is_composite());
    assert!(!circle(ShapeType::DraftGate).is_composite());
}

/// Undraggable parts of a composite are drawn but must not offer a drag handle -
/// the quadrant's arms move only via its centre.
#[test]
fn only_the_undraggable_shape_types_report_undraggable() {
    assert!(circle(ShapeType::UndraggableLine).is_undraggable());
    assert!(circle(ShapeType::UndraggablePoint(1)).is_undraggable());

    assert!(!circle(ShapeType::Gate(gate_id("g"))).is_undraggable());
    assert!(!circle(ShapeType::CompositeGate(gate_id("g"), true)).is_undraggable());
    assert!(!circle(ShapeType::Point(0)).is_undraggable());
}

/// The flag on the composite variants records whether the gate's own axes match
/// the plot's. A mismatched composite still draws, but not as an editable gate.
#[test]
fn axis_matching_is_read_from_the_composite_flag() {
    assert!(circle(ShapeType::CompositeGate(gate_id("g"), true)).is_axis_matched());
    assert!(circle(ShapeType::CompositePoint(0, true)).is_axis_matched());

    assert!(!circle(ShapeType::CompositeGate(gate_id("g"), false)).is_axis_matched());
    assert!(!circle(ShapeType::CompositePoint(0, false)).is_axis_matched());
}

#[test]
fn a_text_shape_is_never_composite_and_always_axis_matched() {
    let text = GateRenderShape::Text {
        origin: (0.0, 0.0),
        offset: (1.0, 1.0),
        fontsize: 12.0,
        text: "20.5%".to_string(),
        text_anchor: None,
        shape_type: ShapeType::Text,
    };

    assert!(!text.is_composite());
    assert!(!text.is_undraggable());
    assert!(
        text.is_axis_matched(),
        "labels are drawn regardless of axes"
    );
}

// ─── clone_with_type ──────────────────────────────────────────────────────────

#[test]
fn cloning_with_a_new_type_keeps_the_geometry() {
    let original = circle(ShapeType::Point(0));
    let restyled = original.clone_with_type(&SELECTED_LINE, ShapeType::Point(7));

    match restyled {
        GateRenderShape::Circle {
            center,
            radius,
            shape_type,
            ..
        } => {
            assert_eq!(center, (1.0, 2.0));
            assert_eq!(radius, 3.0);
            assert!(
                matches!(shape_type, ShapeType::Point(7)),
                "the tag is replaced"
            );
        }
        _ => panic!("variant changed"),
    }
}

#[test]
fn cloning_a_polyline_with_a_new_style_keeps_its_points() {
    let line = GateRenderShape::PolyLine {
        points: vec![(0.0, 0.0), (1.0, 1.0)],
        style: &DEFAULT_LINE,
        shape_type: ShapeType::DraftGate,
    };

    match line.clone_with_type(&SELECTED_LINE, ShapeType::Gate(gate_id("g"))) {
        GateRenderShape::PolyLine { points, style, .. } => {
            assert_eq!(points, vec![(0.0, 0.0), (1.0, 1.0)]);
            assert_eq!(style.stroke, SELECTED_LINE.stroke);
        }
        _ => panic!("variant changed"),
    }
}

#[test]
fn cloning_a_text_shape_with_a_new_type_leaves_it_untouched() {
    let text = GateRenderShape::Text {
        origin: (0.0, 0.0),
        offset: (0.0, 0.0),
        fontsize: 10.0,
        text: "x".to_string(),
        text_anchor: None,
        shape_type: ShapeType::Text,
    };

    assert_eq!(
        text.clone_with_type(&SELECTED_LINE, ShapeType::Point(1)),
        text
    );
}

// ─── clone_with_offset ────────────────────────────────────────────────────────

/// Circles, polylines, handles, rectangles and lines add the offset; polygons
/// and ellipses subtract it. That asymmetry is load-bearing - the drag offset is
/// reported as start-minus-current - so it is pinned here rather than assumed.
#[test]
fn a_circle_adds_the_offset() {
    match circle(ShapeType::Point(0)).clone_with_offset((10.0, 20.0), &DEFAULT_LINE) {
        GateRenderShape::Circle { center, .. } => assert_eq!(center, (11.0, 22.0)),
        _ => panic!("variant changed"),
    }
}

#[test]
fn a_polygon_subtracts_the_offset() {
    let polygon = GateRenderShape::Polygon {
        points: Arc::new(vec![(10.0, 10.0), (20.0, 20.0)]),
        style: &DEFAULT_LINE,
        shape_type: ShapeType::Gate(gate_id("g")),
    };

    match polygon.clone_with_offset((1.0, 2.0), &DEFAULT_LINE) {
        GateRenderShape::Polygon { points, .. } => {
            assert_eq!(*points, vec![(9.0, 8.0), (19.0, 18.0)]);
        }
        _ => panic!("variant changed"),
    }
}

#[test]
fn an_ellipse_subtracts_the_offset_and_keeps_its_radii() {
    let ellipse = GateRenderShape::Ellipse {
        center: (10.0, 10.0),
        radius_x: 4.0,
        radius_y: 2.0,
        degrees_rotation: 30.0,
        style: &DEFAULT_LINE,
        shape_type: ShapeType::Gate(gate_id("g")),
    };

    match ellipse.clone_with_offset((1.0, 2.0), &DEFAULT_LINE) {
        GateRenderShape::Ellipse {
            center,
            radius_x,
            radius_y,
            degrees_rotation,
            ..
        } => {
            assert_eq!(center, (9.0, 8.0));
            assert_eq!((radius_x, radius_y), (4.0, 2.0), "a move must not resize");
            assert_eq!(degrees_rotation, 30.0, "a move must not rotate");
        }
        _ => panic!("variant changed"),
    }
}

#[test]
fn a_rectangle_moves_without_resizing() {
    let rect = GateRenderShape::Rectangle {
        x: 1.0,
        y: 2.0,
        width: 10.0,
        height: 20.0,
        style: &DEFAULT_LINE,
        shape_type: ShapeType::Gate(gate_id("g")),
    };

    match rect.clone_with_offset((5.0, 5.0), &DEFAULT_LINE) {
        GateRenderShape::Rectangle {
            x,
            y,
            width,
            height,
            ..
        } => {
            assert_eq!((x, y), (6.0, 7.0));
            assert_eq!((width, height), (10.0, 20.0));
        }
        _ => panic!("variant changed"),
    }
}

#[test]
fn a_line_moves_both_of_its_ends_together() {
    let line = GateRenderShape::Line {
        x1: 0.0,
        y1: 0.0,
        x2: 10.0,
        y2: 10.0,
        style: &DEFAULT_LINE,
        shape_type: ShapeType::Gate(gate_id("g")),
    };

    match line.clone_with_offset((2.0, 3.0), &DEFAULT_LINE) {
        GateRenderShape::Line { x1, y1, x2, y2, .. } => {
            assert_eq!((x1, y1), (2.0, 3.0));
            assert_eq!((x2, y2), (12.0, 13.0));
        }
        _ => panic!("variant changed"),
    }
}

#[test]
fn a_handle_moves_but_keeps_the_shape_centre_it_orbits() {
    let handle = GateRenderShape::Handle {
        center: (5.0, 5.0),
        size: 4.0,
        shape_center: (0.0, 0.0),
        shape_type: ShapeType::Rotation(0.0),
    };

    match handle.clone_with_offset((1.0, 1.0), &DEFAULT_LINE) {
        GateRenderShape::Handle {
            center,
            shape_center,
            size,
            ..
        } => {
            assert_eq!(center, (6.0, 6.0));
            assert_eq!(shape_center, (0.0, 0.0), "the pivot is not offset");
            assert_eq!(size, 4.0);
        }
        _ => panic!("variant changed"),
    }
}

#[test]
fn a_zero_offset_leaves_a_shape_where_it_was() {
    let original = circle(ShapeType::Point(0));
    assert_eq!(
        original.clone_with_offset((0.0, 0.0), &DEFAULT_LINE),
        original
    );
}

// ─── GateStats ────────────────────────────────────────────────────────────────

#[test]
fn a_single_gate_reports_the_same_figure_for_any_id() {
    let stats = GateStats {
        count: GateStatValue::Single(250.0),
        percent_parent: GateStatValue::Single(25.0),
    };

    assert!(!stats.is_composite());
    // A single gate ignores the id it is asked about.
    assert_eq!(stats.get_count_for_id(gate_id("anything")), Some(250.0));
    assert_eq!(stats.get_percent_for_id(gate_id("anything")), Some(25.0));
}

#[test]
fn a_composite_reports_per_subgate_figures() {
    let mut counts = FxHashMap::default();
    counts.insert(gate_id("q1"), 10.0);
    counts.insert(gate_id("q2"), 90.0);
    let mut percents = FxHashMap::default();
    percents.insert(gate_id("q1"), 10.0);
    percents.insert(gate_id("q2"), 90.0);

    let stats = GateStats {
        count: GateStatValue::Composite(counts),
        percent_parent: GateStatValue::Composite(percents),
    };

    assert!(stats.is_composite());
    assert_eq!(stats.get_count_for_id(gate_id("q1")), Some(10.0));
    assert_eq!(stats.get_percent_for_id(gate_id("q2")), Some(90.0));
}

#[test]
fn a_composite_reports_nothing_for_an_id_it_does_not_hold() {
    let stats = GateStats {
        count: GateStatValue::Composite(FxHashMap::default()),
        percent_parent: GateStatValue::Composite(FxHashMap::default()),
    };

    assert_eq!(stats.get_count_for_id(gate_id("missing")), None);
    assert_eq!(stats.get_percent_for_id(gate_id("missing")), None);
}
