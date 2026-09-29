//! Where a gate's label goes: its name, with its percentage beneath.
//!
//! ## What is stored
//!
//! A gate's `label_position` holds what Omiq's `labelLoc` holds, unchanged,
//! so a label that is never moved here goes back to Omiq exactly as it came.
//! Omiq does not document it; it was worked out from three exports of one
//! gate, its label left where Omiq put it and then dragged into the plot's
//! top-right and bottom-left corners. It is an offset in screen pixels, `x`
//! to the right and `y` down, on an Omiq plot [`OMIQ_PLOT_PX`] across:
//!
//! - from the centre of the gate's bounding box - Omiq's own default is
//!   (0, 0);
//! - to the bottom centre of the label - dragged into a corner, the label's
//!   bottom edge sat on the x axis, and its top edge on the plot's top.
//!
//! Converting through a fraction of each axis's range makes it independent
//! of how big the plot here is drawn, and of a change of cofactor: the label
//! keeps its place relative to the gate on screen.
//!
//! The offset is held in the orientation the gate is held in. A gate turned
//! to a plot whose axes are the other way round turns its label with it -
//! [`swap_offset`] - and the export turns both back, so the file's numbers
//! come through a turned view unchanged.
//!
//! ## Which gate it is measured from
//!
//! The gate as drawn - its global position - never a sample's or a group's.
//! So a label sits in the same place on every sample's plot, moves when the
//! drawn gate is moved, and stays put when a rule positions the gate for one
//! sample. Where no label has been placed it sits just above the gate.
//!
//! Composite gates - quadrants, skewed quadrants, bisectors - label their
//! parts in the plot's corners and are not placed from here: Omiq keeps no
//! position for them (see the gate types).

use flow_gates::types::LabelPosition;

use crate::axis_store::PlotMapper;

/// How wide Omiq's plot is, in the pixels `labelLoc` is written in. From the
/// exports the module comment describes: between the two corners the label
/// travelled 240 by 269 pixels, the plot's size less the label's own, which
/// puts the plot at about 300 either way.
pub const OMIQ_PLOT_PX: f32 = 300.0;

/// The size labels are drawn at, in plot pixels.
pub const FONT_SIZE: f32 = 11.0;

/// The distance from one line's baseline to the next.
pub const LINE_HEIGHT: f32 = FONT_SIZE * 1.2;

/// Clearance between a label left in its default place and the gate below it.
const DEFAULT_GAP: f32 = 3.0;

/// The extent a gate's label is placed around, in the gate's own two
/// parameters: its bounding box, or for a line gate, the line itself.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelBox {
    pub params: (std::sync::Arc<str>, std::sync::Arc<str>),
    pub lo: (f32, f32),
    pub hi: (f32, f32),
}

impl LabelBox {
    /// The bounding box of some points in the gate's two parameters.
    pub fn around(
        params: (std::sync::Arc<str>, std::sync::Arc<str>),
        points: &[(f32, f32)],
    ) -> Option<Self> {
        let finite: Vec<_> = points
            .iter()
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .collect();
        if finite.is_empty() {
            return None;
        }
        let fold = |f: fn(f32, f32) -> f32, pick: fn(&(f32, f32)) -> f32, start: f32| {
            finite.iter().map(|p| pick(p)).fold(start, f)
        };
        Some(Self {
            params,
            lo: (
                fold(f32::min, |p| p.0, f32::INFINITY),
                fold(f32::min, |p| p.1, f32::INFINITY),
            ),
            hi: (
                fold(f32::max, |p| p.0, f32::NEG_INFINITY),
                fold(f32::max, |p| p.1, f32::NEG_INFINITY),
            ),
        })
    }
}

/// A plot's two axes: the parameter drawn along each and its range, in the
/// units the plot is drawn in.
#[derive(Debug, Clone, PartialEq)]
pub struct PlotAxes {
    pub x: std::sync::Arc<str>,
    pub y: std::sync::Arc<str>,
    pub x_range: (f32, f32),
    pub y_range: (f32, f32),
}

impl PlotAxes {
    pub fn of(mapper: &PlotMapper, x: std::sync::Arc<str>, y: std::sync::Arc<str>) -> Self {
        let (xr, yr) = (mapper.x_axis_min_max(), mapper.y_axis_min_max());
        Self {
            x,
            y,
            x_range: (*xr.start(), *xr.end()),
            y_range: (*yr.start(), *yr.end()),
        }
    }
}

/// How a block of label lines sits against its point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VAlign {
    /// The block's bottom edge a little above the point: a label left in its
    /// default place, clear of the gate's top edge.
    Above,
    /// Its top edge on the point.
    Below,
    /// Its bottom edge on the point: where Omiq measures a placed label from.
    Bottom,
}

/// Where a label goes on a plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// In the plot's data units, x along the plot's x axis.
    pub at: (f32, f32),
    pub valign: VAlign,
    /// Whether the label has been put somewhere, as opposed to sitting in
    /// its default place above the gate.
    pub placed: bool,
}

/// A label offset for the same gate turned onto the other axes.
///
/// Its own inverse, so turning twice gives back exactly the numbers the
/// file held. Moving right along the old x is moving up the new y; moving
/// down the old y is moving left along the new x.
pub fn swap_offset(label: Option<LabelPosition>) -> Option<LabelPosition> {
    label.map(|l| LabelPosition {
        offset_x: -l.offset_y,
        offset_y: -l.offset_x,
    })
}

/// The box on the plot's axes: `None` when the gate is not on this plot's
/// two parameters.
fn on_plot(b: &LabelBox, axes: &PlotAxes) -> Option<(bool, (f32, f32), (f32, f32))> {
    let (p0, p1) = (&b.params.0, &b.params.1);
    let reversed = if *p0 == axes.x && *p1 == axes.y {
        false
    } else if *p0 == axes.y && *p1 == axes.x {
        true
    } else {
        return None;
    };
    let (lo, hi) = if reversed {
        ((b.lo.1, b.lo.0), (b.hi.1, b.hi.0))
    } else {
        (b.lo, b.hi)
    };
    // Clamped to what the plot shows, so a gate open to one side - a range
    // gate, a rectangle running off the axis - has its label over the part
    // that can be seen.
    let clamp = |v: f32, (a, b): (f32, f32)| v.clamp(a.min(b), a.max(b));
    Some((
        reversed,
        (clamp(lo.0, axes.x_range), clamp(lo.1, axes.y_range)),
        (clamp(hi.0, axes.x_range), clamp(hi.1, axes.y_range)),
    ))
}

fn span((a, b): (f32, f32)) -> f32 {
    b - a
}

/// Where a gate's label goes on a plot, from its box and stored offset.
pub fn placement(
    b: &LabelBox,
    label: Option<&LabelPosition>,
    axes: &PlotAxes,
) -> Option<Placement> {
    let (reversed, lo, hi) = on_plot(b, axes)?;
    let centre = ((lo.0 + hi.0) / 2.0, (lo.1 + hi.1) / 2.0);
    let Some(label) = label else {
        return Some(Placement {
            at: (centre.0, hi.1),
            valign: VAlign::Above,
            placed: false,
        });
    };
    let on_plot = if reversed {
        swap_offset(Some(label.clone()))?
    } else {
        label.clone()
    };
    Some(Placement {
        at: (
            centre.0 + on_plot.offset_x / OMIQ_PLOT_PX * span(axes.x_range),
            centre.1 - on_plot.offset_y / OMIQ_PLOT_PX * span(axes.y_range),
        ),
        valign: VAlign::Bottom,
        placed: true,
    })
}

/// The offset that puts the bottom centre of a gate's label at `at`, a point
/// on the plot - the inverse of [`placement`], in the gate's own orientation.
pub fn offset_for(b: &LabelBox, at: (f32, f32), axes: &PlotAxes) -> Option<LabelPosition> {
    let (reversed, lo, hi) = on_plot(b, axes)?;
    let (xs, ys) = (span(axes.x_range), span(axes.y_range));
    if xs == 0.0 || ys == 0.0 {
        return None;
    }
    let centre = ((lo.0 + hi.0) / 2.0, (lo.1 + hi.1) / 2.0);
    let on_plot = LabelPosition {
        offset_x: (at.0 - centre.0) / xs * OMIQ_PLOT_PX,
        offset_y: -(at.1 - centre.1) / ys * OMIQ_PLOT_PX,
    };
    if reversed {
        swap_offset(Some(on_plot))
    } else {
        Some(on_plot)
    }
}

/// The baseline of each of `lines` lines, in plot pixels, for a label whose
/// point is at `at_px`.
pub fn line_baselines(at_px: (f32, f32), lines: usize, valign: VAlign) -> Vec<f32> {
    let height = lines as f32 * LINE_HEIGHT;
    let top = match valign {
        VAlign::Above => at_px.1 - DEFAULT_GAP - height,
        VAlign::Below => at_px.1,
        VAlign::Bottom => at_px.1 - height,
    };
    // The baseline sits about four-fifths of the font down its line.
    (0..lines)
        .map(|i| top + i as f32 * LINE_HEIGHT + FONT_SIZE * 0.85 + (LINE_HEIGHT - FONT_SIZE) / 2.0)
        .collect()
}

/// A label's lines: the gate's name, then its percentage when it has been
/// counted.
pub fn label_lines(name: &str, percent: Option<f32>) -> Vec<String> {
    let mut lines = vec![name.to_string()];
    if let Some(percent) = percent {
        lines.push(format!("{percent:.2}%"));
    }
    lines
}

/// A gate's label on a plot, measured from `drawn` - the gate as drawn, not
/// the position a sample shows - with the percentage from `stats`, which are
/// the shown position's. `None` for a gate labelled by its own parts, or one
/// not on this plot's parameters.
pub fn label_shape(
    drawn: &dyn crate::gates::gate_traits::DrawableGate,
    stats: Option<&crate::gates::gate_types::GateStats>,
    axes: &PlotAxes,
) -> Option<crate::gates::gate_types::GateRenderShape> {
    let b = drawn.label_box()?;
    let id = drawn.get_id();
    let label = drawn
        .get_gate_ref(None)
        .and_then(|g| g.label_position.as_ref());
    let placed = placement(&b, label, axes)?;
    Some(crate::gates::gate_types::GateRenderShape::Label {
        at: placed.at,
        lines: label_lines(
            drawn.get_name(),
            stats.and_then(|s| s.get_percent_for_id(id.clone())),
        ),
        valign: placed.valign,
        anchor: "middle",
        movable: Some(id),
    })
}

/// A composite part's label in a corner of the plot: its name over its
/// percentage, as two lines of text at `origin`.
///
/// Near the bottom of the plot the percentage keeps `origin` and the name
/// sits a line above; near the top the name takes `origin` and the
/// percentage goes a line below, so neither runs off the plot. Plain text
/// shapes, as the corners have always been drawn, so the editor holds them
/// still while the composite is dragged exactly as before.
pub fn corner_label(
    origin: (f32, f32),
    text_anchor: Option<String>,
    near_top: bool,
    name: &str,
    percent: String,
    shape_type: crate::gates::gate_types::ShapeType,
    mapper: &PlotMapper,
) -> [crate::gates::gate_types::GateRenderShape; 2] {
    let line = mapper.get_data_tolerance(LINE_HEIGHT).1;
    let text = |offset_y: f32, text: String| crate::gates::gate_types::GateRenderShape::Text {
        origin,
        offset: (0.0, offset_y),
        fontsize: 10f32,
        text,
        text_anchor: text_anchor.clone(),
        shape_type: shape_type.clone(),
    };
    if near_top {
        [text(0.0, name.to_string()), text(-line, percent)]
    } else {
        [text(line, name.to_string()), text(0.0, percent)]
    }
}

/// Where to draw a label so the whole of it is on the plot's data area, in
/// plot pixels: `at_px` itself when it already is, otherwise moved just far
/// enough in.
///
/// Omiq keeps labels on the plot the same way - dragged into a corner, its
/// label stopped at the edges - and a stored offset can point off the plot
/// here, where the plot is not the size Omiq's was. Only the drawing moves;
/// the stored offset, and so the file, is unchanged. A centred label's width
/// is estimated from its longest line.
pub fn keep_on_plot(
    at_px: (f32, f32),
    lines: &[String],
    valign: VAlign,
    mapper: &PlotMapper,
) -> (f32, f32) {
    let (xr, yr) = (mapper.x_axis_min_max(), mapper.y_axis_min_max());
    let a = mapper.data_to_pixel(*xr.start(), *yr.start(), None, None);
    let b = mapper.data_to_pixel(*xr.end(), *yr.end(), None, None);
    let (left, right) = (a.0.min(b.0), a.0.max(b.0));
    let (top, bottom) = (a.1.min(b.1), a.1.max(b.1));

    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
    let half_width = longest * FONT_SIZE * 0.3;
    let height = lines.len() as f32 * LINE_HEIGHT;
    let block_top = match valign {
        VAlign::Above => at_px.1 - DEFAULT_GAP - height,
        VAlign::Below => at_px.1,
        VAlign::Bottom => at_px.1 - height,
    };

    let shift = |lo: f32, hi: f32, min: f32, max: f32| {
        if hi - lo > max - min {
            // Larger than the plot: its start on the plot's start.
            min - lo
        } else if lo < min {
            min - lo
        } else if hi > max {
            max - hi
        } else {
            0.0
        }
    };
    (
        at_px.0 + shift(at_px.0 - half_width, at_px.0 + half_width, left, right),
        at_px.1 + shift(block_top, block_top + height, top, bottom),
    )
}
