//! Tests for `PlotMapper`, the pixel-to-data mapping.
//!
//! Every click, drag and gate render passes through this. It is the only place
//! screen coordinates meet transformed data space, so a mistake here misplaces
//! every gate on the plot while still looking plausible.
//!
//! cargo test axis_store -- --nocapture

#![cfg(test)]

use crate::gate_editor::plots::axis_store::{Param, PlotMapper};
use flow_fcs::TransformType;
use std::sync::Arc;

const W: f32 = 600.0;
const H: f32 = 600.0;

/// A linear mapper over 0..1000 on both axes.
fn linear() -> PlotMapper {
    PlotMapper::new(
        W,
        H,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    )
}

/// An arcsinh mapper over the range a typical fluorescence channel occupies.
fn arcsinh() -> PlotMapper {
    let t = TransformType::Arcsinh { cofactor: 6000.0 };
    PlotMapper::new(
        W,
        H,
        -1.0..=4.5,
        -1.0..=4.5,
        -1.0..=4.5,
        -1.0..=4.5,
        t.clone(),
        t,
    )
}

// ─── Construction ─────────────────────────────────────────────────────────────

#[test]
fn a_mapper_reports_the_dimensions_it_was_built_with() {
    let m = linear();
    assert_eq!(m.width(), W);
    assert_eq!(m.height(), H);
}

#[test]
fn a_mapper_reports_its_axis_and_data_ranges() {
    let m = PlotMapper::new(
        W,
        H,
        0.0..=100.0,
        -5.0..=5.0,
        10.0..=90.0,
        -1.0..=1.0,
        TransformType::Linear,
        TransformType::Linear,
    );

    assert_eq!(m.x_axis_min_max(), 0.0..=100.0);
    assert_eq!(m.y_axis_min_max(), -5.0..=5.0);
    assert_eq!(m.x_data_min_max(), 10.0..=90.0);
    assert_eq!(m.y_data_min_max(), -1.0..=1.0);
}

#[test]
fn a_mapper_reports_its_transforms() {
    let m = arcsinh();
    assert!(matches!(
        m.get_x_transform(),
        TransformType::Arcsinh { cofactor } if cofactor == 6000.0
    ));
    assert!(matches!(m.get_y_transform(), TransformType::Arcsinh { .. }));
}

// ─── Round trips ──────────────────────────────────────────────────────────────

/// The core invariant: a pixel converted to data and back must land where it
/// started. Everything the mouse handlers do assumes this.
#[test]
fn pixel_to_data_and_back_is_a_round_trip() {
    let m = linear();

    for (px, py) in [
        (100.0, 100.0),
        (300.0, 300.0),
        (450.0, 200.0),
        (250.0, 500.0),
    ] {
        let (dx, dy) = m.pixel_to_data(px, py, None, None).unwrap();
        let (rx, ry) = m.data_to_pixel(dx, dy, None, None);

        assert!((rx - px).abs() < 0.5, "x: {px} -> {dx} -> {rx}");
        assert!((ry - py).abs() < 0.5, "y: {py} -> {dy} -> {ry}");
    }
}

#[test]
fn the_round_trip_holds_on_an_arcsinh_axis() {
    let m = arcsinh();

    for (px, py) in [(120.0, 120.0), (300.0, 400.0), (500.0, 250.0)] {
        let (dx, dy) = m.pixel_to_data(px, py, None, None).unwrap();
        let (rx, ry) = m.data_to_pixel(dx, dy, None, None);

        assert!((rx - px).abs() < 0.5, "x: {px} -> {dx} -> {rx}");
        assert!((ry - py).abs() < 0.5, "y: {py} -> {dy} -> {ry}");
    }
}

#[test]
fn the_single_axis_helpers_agree_with_the_pair() {
    let m = linear();
    let (dx, dy) = m.pixel_to_data(200.0, 400.0, None, None).unwrap();

    assert!((m.pixel_x_to_data(200.0, None).unwrap() - dx).abs() < 1e-4);
    assert!((m.pixel_y_to_data(400.0, None).unwrap() - dy).abs() < 1e-4);
}

// ─── Direction and monotonicity ───────────────────────────────────────────────

#[test]
fn x_increases_to_the_right() {
    let m = linear();
    let left = m.pixel_x_to_data(150.0, None).unwrap();
    let right = m.pixel_x_to_data(450.0, None).unwrap();

    assert!(left < right, "data x should grow with pixel x");
}

/// Screen y grows downward, so a larger pixel y is a smaller data value. Getting
/// this backwards would flip every gate vertically.
#[test]
fn y_increases_upward() {
    let m = linear();
    let top = m.pixel_y_to_data(150.0, None).unwrap();
    let bottom = m.pixel_y_to_data(450.0, None).unwrap();

    assert!(top > bottom, "data y should shrink as pixel y grows");
}

#[test]
fn the_mapping_is_monotonic_across_the_plot() {
    let m = linear();
    let mut previous = f32::NEG_INFINITY;

    for step in 0..20 {
        let px = 100.0 + step as f32 * 20.0;
        let dx = m.pixel_x_to_data(px, None).unwrap();
        assert!(dx > previous, "not monotonic at {px}");
        previous = dx;
    }
}

// ─── Tolerance ────────────────────────────────────────────────────────────────

/// Hit testing works in data space, so the pixel slop the mouse allows has to be
/// converted into a data-space box first.
#[test]
fn the_data_tolerance_scales_with_the_pixel_slop() {
    let m = linear();
    let (small_x, small_y) = m.get_data_tolerance(5.0);
    let (big_x, big_y) = m.get_data_tolerance(10.0);

    assert!(small_x > 0.0 && small_y > 0.0);
    assert!(
        (big_x / small_x - 2.0).abs() < 1e-3,
        "double the slop should double the tolerance"
    );
    assert!((big_y / small_y - 2.0).abs() < 1e-3);
}

#[test]
fn a_wider_axis_range_gives_a_coarser_tolerance() {
    let narrow = PlotMapper::new(
        W,
        H,
        0.0..=10.0,
        0.0..=10.0,
        0.0..=10.0,
        0.0..=10.0,
        TransformType::Linear,
        TransformType::Linear,
    );
    let wide = PlotMapper::new(
        W,
        H,
        0.0..=10_000.0,
        0.0..=10_000.0,
        0.0..=10_000.0,
        0.0..=10_000.0,
        TransformType::Linear,
        TransformType::Linear,
    );

    assert!(wide.get_data_tolerance(5.0).0 > narrow.get_data_tolerance(5.0).0);
}

#[test]
fn zero_slop_gives_zero_tolerance() {
    assert_eq!(linear().get_data_tolerance(0.0), (0.0, 0.0));
}

// ─── Param display ────────────────────────────────────────────────────────────

#[test]
fn params_compare_by_both_marker_and_channel() {
    let a = Param {
        marker: Arc::from("CD3"),
        fluoro: Arc::from("BV421-A"),
    };
    let b = Param {
        marker: Arc::from("CD3"),
        fluoro: Arc::from("BV421-A"),
    };
    let c = Param {
        marker: Arc::from("CD3"),
        fluoro: Arc::from("APC-A"),
    };

    assert_eq!(a, b);
    assert_ne!(
        a, c,
        "the same marker on a different channel is a different param"
    );
}

// ─── No plotting area ─────────────────────────────────────────────────────────

/// A plot too small to hold its margins and labels has no plotting area, so
/// no pixel on it has data under it: that is an error to handle, not a
/// panic on the UI thread, which is what it used to be.
#[test]
fn a_plot_with_no_plotting_area_maps_no_pixel() {
    let m = PlotMapper::new(
        60.0,
        60.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    );
    assert!(m.pixel_to_data(30.0, 30.0, None, None).is_err());
    assert!(m.pixel_x_to_data(30.0, None).is_err());
    assert!(m.pixel_y_to_data(30.0, None).is_err());
}

/// A pixel that is not a number has no data under it either.
#[test]
fn a_pixel_that_is_not_a_number_maps_to_nothing() {
    let m = linear();
    assert!(m.pixel_to_data(f32::NAN, 100.0, None, None).is_err());
    assert!(m.pixel_to_data(100.0, f32::INFINITY, None, None).is_err());
}

/// The mapper built for a plot uses the plot's own layout: with other
/// margins or label areas than the default, the same pixel is other data.
#[test]
fn a_mapper_for_a_plot_uses_its_layout() {
    let wide_labels = flow_plots::BasePlotOptions {
        width: 600,
        height: 600,
        y_label_area_size: 120,
        ..Default::default()
    };
    let for_plot = PlotMapper::for_plot(
        &wide_labels,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    );
    let (x_area, _) = flow_plots::plotting_area(&wide_labels);
    // The left edge of the plot's own area is the bottom of the x axis.
    assert_eq!(
        for_plot.pixel_x_to_data(x_area.start as f32, None).unwrap(),
        0.0
    );
    assert_eq!(
        for_plot.data_to_pixel(0.0, 0.0, None, None).0,
        x_area.start as f32
    );
    let default = linear();
    assert_ne!(
        default.pixel_x_to_data(x_area.start as f32, None).unwrap(),
        0.0,
        "the premise: the default layout puts that pixel elsewhere"
    );
}
