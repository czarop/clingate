//! Overlap and contact between outlines, each case worked by hand.

use crate::gates::gate_contact::{Axis, contact_distance, overlaps, shifted};

fn square(x0: f64, y0: f64, side: f64) -> Vec<(f64, f64)> {
    vec![
        (x0, y0),
        (x0 + side, y0),
        (x0 + side, y0 + side),
        (x0, y0 + side),
    ]
}

#[test]
fn outlines_apart_or_only_touching_do_not_overlap() {
    let a = square(0.0, 0.0, 1.0);
    assert!(!overlaps(&a, &square(3.0, 0.0, 1.0)), "apart");
    assert!(!overlaps(&a, &square(1.0, 0.0, 1.0)), "sharing an edge");
    assert!(
        !overlaps(&a, &square(1.0, 1.0, 1.0)),
        "touching at a corner"
    );
    assert!(
        !overlaps(&a, &square(1.0, 0.25, 0.5)),
        "along part of an edge"
    );
}

#[test]
fn outlines_sharing_area_overlap() {
    let a = square(0.0, 0.0, 1.0);
    assert!(overlaps(&a, &square(0.5, 0.5, 1.0)), "corners crossing");
    assert!(
        overlaps(&a, &square(0.25, 0.25, 0.5)),
        "one inside the other"
    );
    assert!(overlaps(&square(0.25, 0.25, 0.5), &a), "either way round");
    assert!(overlaps(&a, &a), "the same outline");
    // A cross: neither has a corner inside the other.
    let wide = vec![(-1.0, 0.4), (2.0, 0.4), (2.0, 0.6), (-1.0, 0.6)];
    let tall = vec![(0.4, -1.0), (0.6, -1.0), (0.6, 2.0), (0.4, 2.0)];
    assert!(overlaps(&wide, &tall));
}

#[test]
fn a_square_slides_to_meet_another_level_with_it() {
    let moving = square(0.0, 0.0, 1.0);
    let fixed = square(3.0, 0.0, 1.0);
    assert_eq!(contact_distance(&moving, &fixed, Axis::X, 1.0), Some(2.0));
    assert_eq!(contact_distance(&fixed, &moving, Axis::X, -1.0), Some(2.0));
    let moved = shifted(&moving, Axis::X, 2.0);
    assert!(!overlaps(&moved, &fixed));
    assert!(overlaps(&shifted(&moving, Axis::X, 2.1), &fixed));
}

#[test]
fn a_square_meets_a_slanted_edge_where_it_comes_nearest() {
    // The fixed gate's left edge runs from (3, 0) up to (2, 1): nearest to
    // the square's right edge, x = 1, at the top, one away.
    let moving = square(0.0, 0.0, 1.0);
    let fixed = vec![(3.0, 0.0), (4.0, 0.0), (4.0, 1.0), (2.0, 1.0)];
    assert_eq!(contact_distance(&moving, &fixed, Axis::X, 1.0), Some(1.0));
}

#[test]
fn a_square_that_never_comes_level_with_another_never_meets_it() {
    let moving = square(0.0, 0.0, 1.0);
    let above = square(3.0, 5.0, 1.0);
    assert_eq!(contact_distance(&moving, &above, Axis::X, 1.0), None);
    assert_eq!(
        contact_distance(&moving, &above, Axis::Y, 1.0),
        None,
        "not in its column"
    );
    let over = square(0.5, 5.0, 1.0);
    assert_eq!(contact_distance(&moving, &over, Axis::Y, 1.0), Some(4.0));
    assert_eq!(
        contact_distance(&moving, &over, Axis::Y, -1.0),
        None,
        "the other way"
    );
}
