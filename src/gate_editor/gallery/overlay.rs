//! Gate outlines as a picture rather than as a control.
//!
//! The editor's `GateLayer` is a drawing *and* an interface: every vertex is a
//! drag target, every shape has a context menu, and the whole layer subscribes
//! to the drag signals so it can follow the mouse. None of that belongs in a
//! gallery. A page of twenty plots with twenty live gate layers would be a
//! thousand event handlers hanging off pictures nobody is allowed to edit, and
//! one stray click would move a gate on a sample the person was only looking
//! at.
//!
//! So the gallery flattens the same [`GateRenderShape`]s the editor draws into
//! plain pixel-space primitives and renders those: no handlers, no handles, no
//! vertex dots.
//!
//! Flattening is a separate step from rendering because it has two consumers.
//! The screen draws the primitives as SVG; the PDF draws the same primitives as
//! page operators. Sharing the flattened form is what makes the exported page
//! and the page on screen the same picture, rather than two drawings of the
//! same gate that drift apart one fix at a time.

use std::sync::Arc;

use dioxus::prelude::*;

use crate::gate_editor::gates::gate_single::rectangle_gate;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::gates::gate_types::{GateRenderShape, GateStats};
use crate::gate_editor::plots::axis_store::PlotMapper;

/// A drawn thing, in pixels, with nothing left to resolve.
#[derive(Clone, Debug, PartialEq)]
pub enum Flat {
    /// An open or closed run of points. Rectangles and lines arrive here too -
    /// a rectangle is four points closed, a line is two points open - because
    /// one path primitive is all either consumer needs.
    Path {
        points: Vec<(f32, f32)>,
        closed: bool,
        stroke: &'static str,
        fill: &'static str,
        width: f32,
        dashed: bool,
    },
    Ellipse {
        centre: (f32, f32),
        radius: (f32, f32),
        rotation: f32,
        stroke: &'static str,
        fill: &'static str,
        width: f32,
        dashed: bool,
    },
    Text {
        at: (f32, f32),
        size: f32,
        text: String,
        anchor: Option<String>,
    },
}

/// Flatten one gate's shapes into pixel space.
///
/// `Handle` and `Circle` are dropped. Both exist only to be grabbed - the
/// rotation handle and the polygon's vertex dots - and drawing them where
/// nothing can be grabbed would promise an interaction the gallery does not
/// have.
pub fn flatten(shapes: Vec<GateRenderShape>, mapper: &PlotMapper) -> Vec<Flat> {
    let point = |(x, y): (f32, f32)| mapper.data_to_pixel(x, y, None, None);
    let mut out = Vec::with_capacity(shapes.len());
    for shape in shapes {
        match shape {
            GateRenderShape::PolyLine { points, style, .. } => out.push(Flat::Path {
                points: points.into_iter().map(point).collect(),
                closed: false,
                stroke: style.stroke,
                fill: style.fill,
                width: style.stroke_width,
                dashed: style.dashed,
            }),
            GateRenderShape::Polygon { points, style, .. } => out.push(Flat::Path {
                points: points.iter().copied().map(point).collect(),
                closed: true,
                stroke: style.stroke,
                fill: style.fill,
                width: style.stroke_width,
                dashed: style.dashed,
            }),
            GateRenderShape::Rectangle {
                x,
                y,
                width,
                height,
                style,
                ..
            } => {
                let (px, py, pw, ph) =
                    rectangle_gate::map_rect_to_pixels(x, y, width, height, mapper);
                out.push(Flat::Path {
                    points: vec![(px, py), (px + pw, py), (px + pw, py + ph), (px, py + ph)],
                    closed: true,
                    stroke: style.stroke,
                    fill: style.fill,
                    width: style.stroke_width,
                    dashed: style.dashed,
                });
            }
            GateRenderShape::Line {
                x1,
                y1,
                x2,
                y2,
                style,
                ..
            } => out.push(Flat::Path {
                points: vec![point((x1, y1)), point((x2, y2))],
                closed: false,
                stroke: style.stroke,
                // A line has no inside. The editor's `line` element ignores
                // `fill` for the same reason; saying so here keeps the PDF from
                // filling the triangle between a line's ends.
                fill: "none",
                width: style.stroke_width,
                dashed: style.dashed,
            }),
            GateRenderShape::Ellipse {
                center,
                radius_x,
                radius_y,
                degrees_rotation,
                style,
                ..
            } => {
                // Radii are measured in pixels by stepping one radius out along
                // each axis and taking the distance, rather than scaling the
                // data radius: on an arcsinh axis a radius is not a constant
                // number of pixels, and this is what the editor does.
                let centre = point(center);
                let x_edge = point((center.0 + radius_x, center.1));
                let y_edge = point((center.0, center.1 + radius_y));
                out.push(Flat::Ellipse {
                    centre,
                    radius: ((x_edge.0 - centre.0).abs(), (y_edge.1 - centre.1).abs()),
                    rotation: degrees_rotation,
                    stroke: style.stroke,
                    fill: style.fill,
                    width: style.stroke_width,
                    dashed: style.dashed,
                });
            }
            GateRenderShape::Text {
                origin,
                offset,
                fontsize,
                text,
                text_anchor,
                ..
            } => out.push(Flat::Text {
                at: point((origin.0 + offset.0, origin.1 + offset.1)),
                size: fontsize,
                text,
                anchor: text_anchor,
            }),
            // Grab targets, not drawing. See the function comment.
            GateRenderShape::Handle { .. } | GateRenderShape::Circle { .. } => {}
        }
    }
    out
}

/// Flatten every gate on one plot, in drawing order.
pub fn flatten_gates(
    gates: &[Arc<dyn DrawableGate>],
    stats: &rustc_hash::FxHashMap<Arc<str>, GateStats>,
    selected: Option<&Arc<str>>,
    mapper: &PlotMapper,
) -> Vec<Flat> {
    let mut out = Vec::new();
    for gate in gates {
        let id = gate.get_id();
        let is_selected = selected.is_some_and(|s| *s == id);
        let stat = stats.get(&id).cloned();
        out.extend(flatten(
            gate.draw_self(is_selected, None, mapper, &stat),
            mapper,
        ));
    }
    out
}

/// The flattened primitives as SVG, over the plot image.
#[component]
pub fn StaticGates(shapes: ReadSignal<Vec<Flat>>, size: ReadSignal<u32>) -> Element {
    let side = size();
    rsx! {
        svg {
            width: "100%",
            height: "100%",
            view_box: "0 0 {side} {side}",
            // `pointer_events: none` is the whole difference between this and
            // the editor's layer: clicks fall through to the page rather than
            // landing on a gate that must not move.
            style: "position: absolute; top: 0; left: 0; z-index: 2; pointer-events: none; user-select: none;",
            for (at , shape) in shapes.read().iter().enumerate() {
                match shape {
                    Flat::Path { points, closed, stroke, fill, width, dashed } => {
                        let listed = points
                            .iter()
                            .map(|(x, y)| format!("{x},{y}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let dash = if *dashed { "4" } else { "none" };
                        if *closed {
                            rsx! {
                                polygon {
                                    key: "{at}",
                                    points: "{listed}",
                                    stroke: "{stroke}",
                                    stroke_width: *width,
                                    stroke_dasharray: dash,
                                    fill: "{fill}",
                                }
                            }
                        } else {
                            rsx! {
                                polyline {
                                    key: "{at}",
                                    points: "{listed}",
                                    stroke: "{stroke}",
                                    stroke_width: *width,
                                    stroke_dasharray: dash,
                                    fill: "{fill}",
                                }
                            }
                        }
                    }
                    Flat::Ellipse { centre, radius, rotation, stroke, fill, width, dashed } => {
                        rsx! {
                            ellipse {
                                key: "{at}",
                                cx: centre.0,
                                cy: centre.1,
                                rx: radius.0,
                                ry: radius.1,
                                stroke: "{stroke}",
                                stroke_width: *width,
                                stroke_dasharray: if *dashed { "4" } else { "none" },
                                fill: "{fill}",
                                transform: "rotate({rotation} {centre.0} {centre.1})",
                            }
                        }
                    }
                    Flat::Text { at: loc, size, text, anchor } => {
                        rsx! {
                            text {
                                key: "{at}",
                                x: loc.0,
                                y: loc.1,
                                font_size: *size,
                                text_anchor: anchor.clone(),
                                fill: "black",
                                "{text}"
                            }
                        }
                    }
                }
            }
        }
    }
}
