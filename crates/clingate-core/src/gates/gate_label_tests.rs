use std::sync::Arc;

use flow_gates::types::LabelPosition;

use super::gate_label::*;

fn axes(x: &str, y: &str) -> PlotAxes {
    PlotAxes {
        x: Arc::from(x),
        y: Arc::from(y),
        x_range: (0.0, 1000.0),
        y_range: (-2.0, 8.0),
    }
}

fn gate_box(p0: &str, p1: &str, lo: (f32, f32), hi: (f32, f32)) -> LabelBox {
    LabelBox {
        params: (Arc::from(p0), Arc::from(p1)),
        lo,
        hi,
    }
}

fn offset(x: f32, y: f32) -> LabelPosition {
    LabelPosition {
        offset_x: x,
        offset_y: y,
    }
}

fn close(a: (f32, f32), b: (f32, f32)) {
    assert!(
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3,
        "{a:?} != {b:?}"
    );
}

#[test]
fn an_unplaced_label_sits_above_the_middle_of_the_gate() {
    let b = gate_box("FSC", "SSC", (100.0, 1.0), (300.0, 3.0));
    let p = placement(&b, None, &axes("FSC", "SSC")).unwrap();
    assert!(!p.placed);
    assert_eq!(p.valign, VAlign::Above);
    close(p.at, (200.0, 3.0));
}

#[test]
fn an_offset_is_pixels_on_omiqs_plot_right_and_down_from_the_middle() {
    let b = gate_box("FSC", "SSC", (100.0, 1.0), (300.0, 3.0));
    // A tenth of the plot right and a fifth up.
    let o = offset(OMIQ_PLOT_PX * 0.1, -OMIQ_PLOT_PX * 0.2);
    let p = placement(&b, Some(&o), &axes("FSC", "SSC")).unwrap();
    assert!(p.placed);
    assert_eq!(p.valign, VAlign::Bottom);
    close(p.at, (200.0 + 100.0, 2.0 + 2.0));
}

#[test]
fn a_point_turned_into_an_offset_comes_back_as_the_same_point() {
    let b = gate_box("FSC", "SSC", (100.0, 1.0), (300.0, 3.0));
    for plot in [axes("FSC", "SSC"), axes("SSC", "FSC")] {
        let plot = PlotAxes {
            x_range: if plot.x.as_ref() == "FSC" {
                (0.0, 1000.0)
            } else {
                (-2.0, 8.0)
            },
            y_range: if plot.y.as_ref() == "FSC" {
                (0.0, 1000.0)
            } else {
                (-2.0, 8.0)
            },
            ..plot
        };
        let at = if plot.x.as_ref() == "FSC" {
            (640.0, 6.5)
        } else {
            (6.5, 640.0)
        };
        let o = offset_for(&b, at, &plot).unwrap();
        close(placement(&b, Some(&o), &plot).unwrap().at, at);
    }
}

#[test]
fn turning_an_offset_twice_gives_back_the_files_numbers_exactly() {
    for (x, y) in [
        (219.5, -60.65164),
        (-83.765625, -83.0),
        (0.0, 0.0),
        (1.0, -1.0),
    ] {
        let o = offset(x, y);
        assert_eq!(swap_offset(swap_offset(Some(o.clone()))), Some(o));
    }
    assert_eq!(swap_offset(None), None);
}

#[test]
fn a_gate_and_its_label_turned_together_land_on_the_same_spot() {
    // The same gate held on FSC by SSC, and turned to SSC by FSC with its
    // offset turned too, puts its label at the same place on either plot.
    let held = gate_box("FSC", "SSC", (100.0, 1.0), (300.0, 3.0));
    let turned = gate_box("SSC", "FSC", (1.0, 100.0), (3.0, 300.0));
    let o = offset(40.0, -75.0);
    let o_turned = swap_offset(Some(o.clone())).unwrap();
    for plot in [
        axes("FSC", "SSC"),
        PlotAxes {
            x: Arc::from("SSC"),
            y: Arc::from("FSC"),
            x_range: (-2.0, 8.0),
            y_range: (0.0, 1000.0),
        },
    ] {
        let a = placement(&held, Some(&o), &plot).unwrap().at;
        let b = placement(&turned, Some(&o_turned), &plot).unwrap().at;
        close(a, b);
    }
}

#[test]
fn a_gate_open_to_one_side_is_labelled_over_the_part_the_plot_shows() {
    let range = gate_box("FSC", "SSC", (200.0, -1e16), (400.0, 1e16));
    let p = placement(&range, None, &axes("FSC", "SSC")).unwrap();
    close(p.at, (300.0, 8.0));
}

#[test]
fn a_gate_on_other_parameters_has_no_place_on_the_plot() {
    let b = gate_box("CD4", "CD8", (1.0, 1.0), (2.0, 2.0));
    assert!(placement(&b, None, &axes("FSC", "SSC")).is_none());
    assert!(offset_for(&b, (1.0, 1.0), &axes("FSC", "SSC")).is_none());
}

#[test]
fn lines_stack_name_over_percent_against_their_point() {
    let at = (100.0, 200.0);
    let above = line_baselines(at, 2, VAlign::Above);
    assert!(above[0] < above[1] && above[1] < at.1, "{above:?}");
    let below = line_baselines(at, 2, VAlign::Below);
    assert!(below[0] > at.1 && below[1] > below[0], "{below:?}");
    // A placed label's bottom edge is its point: the last line's baseline
    // sits just above it, less than a line from it.
    let bottom = line_baselines(at, 2, VAlign::Bottom);
    assert!(
        bottom[1] < at.1 && at.1 - bottom[1] < LINE_HEIGHT,
        "{bottom:?}"
    );
    assert_eq!(label_lines("CD4+", Some(61.714)), ["CD4+", "61.71%"]);
    assert_eq!(label_lines("CD4+", None), ["CD4+"]);
}

/// The three exports the conversion was worked out from: a Time gate's label
/// left where Omiq put it, then dragged to the plot's top right, then to its
/// bottom left. Scaling Time 0-956063, FSC-A 0-4194304, both linear.
#[test]
fn omiqs_own_labels_land_where_omiq_showed_them() {
    let time = gate_box(
        "Time",
        "FSC-A",
        (27750.127, 258992.66),
        (936655.7, 4021869.2),
    );
    let axes = PlotAxes {
        x: Arc::from("Time"),
        y: Arc::from("FSC-A"),
        x_range: (0.0, 956063.0),
        y_range: (0.0, 4194304.0),
    };
    let fraction = |label: LabelPosition| {
        let at = placement(&time, Some(&label), &axes).unwrap().at;
        (at.0 / axes.x_range.1, at.1 / axes.y_range.1)
    };
    // Omiq's default: the middle of the gate.
    let (x, y) = fraction(offset(0.0, 0.0));
    assert!(
        (x - 0.504).abs() < 0.01 && (y - 0.510).abs() < 0.01,
        "{x} {y}"
    );
    // Top right: the label's bottom a label's height below the plot's top.
    let (x, y) = fraction(offset(121.69534, -119.352_86));
    assert!(x > 0.85 && x < 0.95, "{x}");
    let label_height = 2.0 * LINE_HEIGHT / OMIQ_PLOT_PX;
    assert!((y + label_height - 1.0).abs() < 0.03, "{y}");
    // Bottom left: the label's bottom on the x axis.
    let (x, y) = fraction(offset(-118.60153, 149.64714));
    assert!(x > 0.05 && x < 0.15, "{x}");
    assert!(y.abs() < 0.03, "{y}");
}

#[test]
fn a_box_is_the_extent_of_the_finite_points() {
    let b = LabelBox::around(
        (Arc::from("a"), Arc::from("b")),
        &[(1.0, 5.0), (3.0, 2.0), (f32::NAN, 9.0), (2.0, 4.0)],
    )
    .unwrap();
    assert_eq!((b.lo, b.hi), ((1.0, 2.0), (3.0, 5.0)));
    assert!(LabelBox::around((Arc::from("a"), Arc::from("b")), &[]).is_none());
}

#[test]
fn a_label_that_would_fall_off_the_plot_is_drawn_just_inside_it() {
    use crate::axis_store::PlotMapper;
    use flow_fcs::TransformType;
    let m = PlotMapper::new(
        600.0,
        600.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    );
    let lines = label_lines("Cells", Some(96.16));
    let corner = |x: f32, y: f32| m.data_to_pixel(x, y, None, None);
    let (left, top) = corner(0.0, 1000.0);
    let (right, bottom) = corner(1000.0, 0.0);

    // Inside already: not moved.
    let middle = corner(500.0, 500.0);
    assert_eq!(keep_on_plot(middle, &lines, VAlign::Bottom, &m), middle);

    // Off the top left: brought in until it touches both edges.
    let off = (left - 80.0, top - 60.0);
    let (x, y) = keep_on_plot(off, &lines, VAlign::Bottom, &m);
    let half = 6.0 * FONT_SIZE * 0.3;
    assert!((x - half - left).abs() < 1e-3, "{x} vs {left}");
    assert!((y - 2.0 * LINE_HEIGHT - top).abs() < 1e-3, "{y} vs {top}");

    // Off the bottom right likewise.
    let (x, y) = keep_on_plot((right + 50.0, bottom + 50.0), &lines, VAlign::Bottom, &m);
    assert!((x + half - right).abs() < 1e-3 && (y - bottom).abs() < 1e-3);
}
