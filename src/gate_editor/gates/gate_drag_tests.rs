//! Tests for the drag/rotate interaction maths.
//!
//! These types translate raw mouse movement into gate edits. They are pure, and
//! a sign error here is the difference between a gate following the cursor and
//! running away from it.
//!
//! cargo test gate_drag -- --nocapture

#![cfg(test)]

use crate::gate_editor::gates::gate_drag::{
    GateDragData, GateDragType, PointDragData, RotationData,
};
use std::sync::Arc;

fn id() -> Arc<str> {
    Arc::from("gate-1")
}

// ─── GateDragData ─────────────────────────────────────────────────────────────

#[test]
fn a_fresh_drag_starts_and_ends_at_the_same_point() {
    let d = GateDragData::new(id(), (5.0, 5.0), (5.0, 5.0));

    assert_eq!(d.start_loc(), (5.0, 5.0));
    assert_eq!(d.current_loc(), (5.0, 5.0));
    assert_eq!(d.offset(), (0.0, 0.0), "no movement means no offset");
}

/// The offset is start minus current, so dragging right gives a negative x.
/// Every caller of `clone_with_offset` depends on that sign.
#[test]
fn the_offset_is_start_minus_current() {
    let d = GateDragData::new(id(), (10.0, 20.0), (13.0, 24.0));
    assert_eq!(d.offset(), (-3.0, -4.0));
}

/// Each mouse move rebases: the previous current becomes the new start, so the
/// offset is always the *incremental* movement rather than the total.
#[test]
fn a_continued_drag_reports_the_incremental_movement() {
    let first = GateDragData::new(id(), (0.0, 0.0), (2.0, 2.0));
    let second = GateDragData::clone_from_data((5.0, 6.0), first);

    assert_eq!(
        second.start_loc(),
        (2.0, 2.0),
        "the old current becomes the new start"
    );
    assert_eq!(second.current_loc(), (5.0, 6.0));
    assert_eq!(second.offset(), (-3.0, -4.0), "increment, not total");
}

#[test]
fn a_drag_keeps_its_gate_id_across_moves() {
    let first = GateDragData::new(id(), (0.0, 0.0), (1.0, 1.0));
    let second = GateDragData::clone_from_data((2.0, 2.0), first);

    assert_eq!(&*second.gate_id(), "gate-1");
}

// ─── PointDragData ────────────────────────────────────────────────────────────

#[test]
fn a_point_drag_tracks_its_index_and_location() {
    let p = PointDragData::new(3, (7.0, 8.0));

    assert_eq!(p.point_index(), 3);
    assert_eq!(p.loc(), (7.0, 8.0));
}

#[test]
fn a_point_drag_keeps_its_index_as_the_location_moves() {
    let first = PointDragData::new(2, (1.0, 1.0));
    let moved = PointDragData::clone_from_data((9.0, 9.0), first);

    assert_eq!(
        moved.point_index(),
        2,
        "the vertex being dragged does not change"
    );
    assert_eq!(moved.loc(), (9.0, 9.0));
}

// ─── RotationData ─────────────────────────────────────────────────────────────

fn rotation(start: (f32, f32), current: (f32, f32)) -> RotationData {
    RotationData::new(id(), (0.0, 0.0), start, current)
}

#[test]
fn no_movement_is_no_rotation() {
    let r = rotation((1.0, 0.0), (1.0, 0.0));
    assert!(r.rotation_rad().abs() < 1e-6);
}

/// Screen y grows downward while the maths is done in data space, so the result
/// is negated. A quarter turn anticlockwise in data space reports -90 degrees.
#[test]
fn a_quarter_turn_reports_ninety_degrees() {
    let r = rotation((1.0, 0.0), (0.0, 1.0));
    assert!(
        (r.rotation_deg() - -90.0).abs() < 1e-3,
        "got {}",
        r.rotation_deg()
    );
}

#[test]
fn rotating_the_other_way_flips_the_sign() {
    let anticlockwise = rotation((1.0, 0.0), (0.0, 1.0)).rotation_deg();
    let clockwise = rotation((0.0, 1.0), (1.0, 0.0)).rotation_deg();

    assert!(
        (anticlockwise + clockwise).abs() < 1e-3,
        "opposite rotations should cancel: {anticlockwise} and {clockwise}"
    );
}

#[test]
fn a_half_turn_is_a_hundred_and_eighty_degrees() {
    let r = rotation((1.0, 0.0), (-1.0, 0.0));
    assert!(
        (r.rotation_deg().abs() - 180.0).abs() < 1e-3,
        "got {}",
        r.rotation_deg()
    );
}

/// Rotation is about the pivot, so the distance from it is irrelevant - only the
/// angle matters.
#[test]
fn rotation_ignores_the_distance_from_the_pivot() {
    let near = rotation((1.0, 0.0), (0.0, 1.0)).rotation_deg();
    let far = rotation((50.0, 0.0), (0.0, 50.0)).rotation_deg();

    assert!((near - far).abs() < 1e-3);
}

#[test]
fn rotation_is_measured_about_the_given_pivot() {
    // Pivot at (10, 10); start due east of it, end due north of it.
    let r = RotationData::new(id(), (10.0, 10.0), (11.0, 10.0), (10.0, 11.0));

    assert!(
        (r.rotation_deg() - -90.0).abs() < 1e-3,
        "got {}",
        r.rotation_deg()
    );
    assert_eq!(r.pivot_point(), (10.0, 10.0));
}

#[test]
fn a_continued_rotation_rebases_onto_the_previous_position() {
    let first = rotation((1.0, 0.0), (0.0, 1.0));
    let second = RotationData::clone_from_data((-1.0, 0.0), first);

    assert_eq!(second.start_loc(), (0.0, 1.0));
    assert_eq!(second.current_loc(), (-1.0, 0.0));
    // A further quarter turn from north to west.
    assert!((second.rotation_deg() - -90.0).abs() < 1e-3);
}

// ─── GateDragType dispatch ────────────────────────────────────────────────────

#[test]
fn cloning_with_a_point_preserves_the_drag_variant() {
    let point = GateDragType::Point(PointDragData::new(1, (0.0, 0.0)));
    let gate = GateDragType::Gate(GateDragData::new(id(), (0.0, 0.0), (0.0, 0.0)));
    let rotate =
        GateDragType::Rotation(RotationData::new(id(), (0.0, 0.0), (1.0, 0.0), (1.0, 0.0)));

    assert!(matches!(
        point.clone_with_point((5.0, 5.0)),
        GateDragType::Point(_)
    ));
    assert!(matches!(
        gate.clone_with_point((5.0, 5.0)),
        GateDragType::Gate(_)
    ));
    assert!(matches!(
        rotate.clone_with_point((5.0, 5.0)),
        GateDragType::Rotation(_)
    ));
}

#[test]
fn cloning_a_point_drag_moves_it_to_the_new_location() {
    let drag = GateDragType::Point(PointDragData::new(4, (0.0, 0.0)));

    match drag.clone_with_point((3.0, 7.0)) {
        GateDragType::Point(p) => {
            assert_eq!(p.loc(), (3.0, 7.0));
            assert_eq!(p.point_index(), 4);
        }
        _ => panic!("variant changed"),
    }
}
