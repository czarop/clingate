//! Pictures of a gate on several samples, tiled into one image - for Claude,
//! which reads plots the way a person does.
//!
//! Each tile is drawn the way the gallery draws a plot: the file read,
//! compensated and scaled as the editor reads it, filtered by the gate's
//! parent chain, and rendered as a density on the plot's own axes. The gate's
//! outline is drawn into the picture - the app draws it as an overlay, and a
//! picture handed on has none - through the gate's own drawing, so it lands
//! where it does on screen. What the gate holds is counted by the statistic
//! the plot shows.

use std::path::PathBuf;
use std::sync::Arc;

use flow_plots::{
    BasePlotOptions, ColorMaps, DensityPlot, DensityPlotOptions, Plot, ScatterPlotData,
    render::RenderConfig,
};
use image::{Rgba, RgbaImage};

use crate::AxisInfo;
use crate::axis_store::PlotMapper;
use crate::gates::gate_store::{GateId, GateOverrideResolver};
use crate::gates::gate_traits::DrawableGate;
use crate::gates::gate_types::{GateRenderShape, GateStatValue};

/// A tile's side, in pixels.
pub const TILE: u32 = 300;
/// The most tiles in one picture.
pub const MOST_TILES: usize = 9;

/// One plot to draw.
pub struct PlotRequest {
    pub path: PathBuf,
    pub compensation: crate::compensation::Choice,
    pub cofactors: Vec<(Arc<str>, f32)>,
    /// The gates above the one drawn, outermost first.
    pub chain: Vec<GateId>,
    pub resolver: GateOverrideResolver,
    pub x: Arc<str>,
    pub y: Arc<str>,
    pub x_axis: AxisInfo,
    pub y_axis: AxisInfo,
    pub gate: Option<Arc<dyn DrawableGate>>,
}

/// A drawn tile, with what the gate holds on it.
pub struct Drawn {
    pub image: RgbaImage,
    pub parent_events: usize,
    /// The fraction of the parent inside the gate.
    pub holds: Option<f64>,
}

/// Read, filter, count and draw one plot. Blocking.
pub fn render(request: &PlotRequest) -> anyhow::Result<Drawn> {
    let scaled =
        crate::events::read_scaled(&request.path, &request.compensation, &request.cofactors)?;
    let frame = crate::events::under_chain(&scaled, &request.chain, &request.resolver)?;
    drop(scaled);
    let parent_events = frame.height();
    let points = crate::events::points(&frame, &request.x, &request.y)?;
    let holds = match &request.gate {
        Some(gate) => {
            let mapped = crate::events::index_mapped(&frame, &request.x, &request.y)?;
            crate::gates::gate_stats::get_percent_and_counts_gate(
                gate.clone(),
                &mapped,
                parent_events as f32,
            )
            .ok()
            .and_then(|s| match s.percent_parent {
                GateStatValue::Single(p) => Some(p as f64 / 100.0),
                GateStatValue::Composite(_) => None,
            })
        }
        None => None,
    };
    drop(frame);
    let (png, mapper) = density(points, request)?;
    let mut image = image::load_from_memory(&png)?.to_rgba8();
    if let Some(gate) = &request.gate {
        for shape in gate.draw_self(false, None, &mapper, &None) {
            outline(&mut image, &shape, &mapper);
        }
    }
    Ok(Drawn {
        image,
        parent_events,
        holds,
    })
}

/// The density, as the gallery draws it, and the mapper that agrees with it.
fn density(
    points: Vec<(f32, f32)>,
    request: &PlotRequest,
) -> anyhow::Result<(Vec<u8>, PlotMapper)> {
    let bounds = bounds_of(&points);
    let base = BasePlotOptions::new()
        .width(TILE)
        .height(TILE)
        .title("")
        .show_colorbar(false)
        .build()?;
    let x_options = flow_plots::AxisOptions::new()
        .range(request.x_axis.axis_lower..=request.x_axis.axis_upper)
        .transform(request.x_axis.transform.clone())
        .label(request.x.to_string())
        .build()?;
    let y_options = flow_plots::AxisOptions::new()
        .range(request.y_axis.axis_lower..=request.y_axis.axis_upper)
        .transform(request.y_axis.transform.clone())
        .label(request.y.to_string())
        .build()?;
    let mapper = PlotMapper::for_plot(
        &base,
        *x_options.range.start()..=*x_options.range.end(),
        *y_options.range.start()..=*y_options.range.end(),
        bounds.0,
        bounds.1,
        request.x_axis.transform.clone(),
        request.y_axis.transform.clone(),
    );
    let options = DensityPlotOptions::new()
        .base(base.clone())
        .plot_type(flow_plots::PlotType::Density)
        .colormap(ColorMaps::Jet)
        .x_axis(x_options)
        .y_axis(y_options)
        .point_size(0.5_f32)
        .build()?;
    let data = ScatterPlotData {
        points,
        gate_ids: None,
        z_values: None,
    };
    let mut config = RenderConfig::default();
    let bytes = DensityPlot::new().render(data, &options, &mut config)?;
    Ok((bytes, mapper))
}

fn bounds_of(
    points: &[(f32, f32)],
) -> (std::ops::RangeInclusive<f32>, std::ops::RangeInclusive<f32>) {
    let Some(first) = points.first() else {
        return (0.0..=1.0, 0.0..=1.0);
    };
    let mut x = (first.0, first.0);
    let mut y = (first.1, first.1);
    for (px, py) in points.iter().skip(1) {
        x.0 = x.0.min(*px);
        x.1 = x.1.max(*px);
        y.0 = y.0.min(*py);
        y.1 = y.1.max(*py);
    }
    (x.0..=x.1, y.0..=y.1)
}

const INK: Rgba<u8> = Rgba([0, 0, 0, 255]);

/// Where a line may be drawn: columns and rows, ends excluded.
pub(crate) type Clip = (std::ops::Range<u32>, std::ops::Range<u32>);

/// A line two pixels wide, drawn only inside `clip` - the plotting area, so a
/// gate running off the plot stops at its edge rather than over the axes.
pub(crate) fn line(image: &mut RgbaImage, (x0, y0): (f32, f32), (x1, y1): (f32, f32), clip: &Clip) {
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return;
    }
    let (w, h) = (image.width() as f32, image.height() as f32);
    // Clamp far-off ends - an unbounded side is a coordinate of 1e16 - so the
    // walk along the line stays short.
    let clamp = |v: f32, max: f32| v.clamp(-max, 2.0 * max);
    let (x0, y0, x1, y1) = (clamp(x0, w), clamp(y0, h), clamp(x1, w), clamp(y1, h));
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            let (px, py) = ((x + dx - 0.5).floor(), (y + dy - 0.5).floor());
            if px >= 0.0 && py >= 0.0 && px < w && py < h {
                let (px, py) = (px as u32, py as u32);
                if clip.0.contains(&px) && clip.1.contains(&py) {
                    image.put_pixel(px, py, INK);
                }
            }
        }
    }
}

/// Draw one of a gate's shapes as an outline; handles and labels are left
/// out - the caption says what the gate holds. Shapes come in data units and
/// are put on the plot the way the gallery puts them, through `mapper`.
fn outline(image: &mut RgbaImage, shape: &GateRenderShape, mapper: &PlotMapper) {
    let point = |(x, y): (f32, f32)| mapper.data_to_pixel(x, y, None, None);
    let clip = mapper.plotting_area();
    let ring = |image: &mut RgbaImage, points: &[(f32, f32)], closed: bool| {
        for pair in points.windows(2) {
            line(image, pair[0], pair[1], &clip);
        }
        if closed && points.len() > 2 {
            line(image, points[points.len() - 1], points[0], &clip);
        }
    };
    match shape {
        GateRenderShape::PolyLine { points, .. } => {
            let points: Vec<_> = points.iter().copied().map(point).collect();
            ring(image, &points, false)
        }
        GateRenderShape::Polygon { points, .. } => {
            let points: Vec<_> = points.iter().copied().map(point).collect();
            ring(image, &points, true)
        }
        GateRenderShape::Rectangle {
            x,
            y,
            width,
            height,
            ..
        } => {
            let (px, py, pw, ph) = crate::gates::gate_single::rectangle_gate::map_rect_to_pixels(
                *x, *y, *width, *height, mapper,
            );
            ring(
                image,
                &[(px, py), (px + pw, py), (px + pw, py + ph), (px, py + ph)],
                true,
            )
        }
        GateRenderShape::Line { x1, y1, x2, y2, .. } => {
            line(image, point((*x1, *y1)), point((*x2, *y2)), &clip)
        }
        GateRenderShape::Ellipse {
            center,
            radius_x,
            radius_y,
            degrees_rotation,
            ..
        } => {
            // Radii in pixels by stepping one radius out along each axis, as
            // the gallery does: on an arcsinh axis a radius is not a constant
            // number of pixels.
            let centre = point(*center);
            let x_edge = point((center.0 + radius_x, center.1));
            let y_edge = point((center.0, center.1 + radius_y));
            let (rx, ry) = ((x_edge.0 - centre.0).abs(), (y_edge.1 - centre.1).abs());
            let (s, c) = degrees_rotation.to_radians().sin_cos();
            let points: Vec<(f32, f32)> = (0..64)
                .map(|i| {
                    let a = i as f32 / 64.0 * std::f32::consts::TAU;
                    let (ex, ey) = (rx * a.cos(), ry * a.sin());
                    (centre.0 + ex * c - ey * s, centre.1 + ex * s + ey * c)
                })
                .collect();
            ring(image, &points, true);
        }
        _ => {}
    }
}

/// Tiles in rows of `columns`, as one PNG.
pub fn grid(tiles: &[RgbaImage], columns: usize) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(!tiles.is_empty(), "nothing to draw");
    let columns = columns.clamp(1, tiles.len());
    let rows = tiles.len().div_ceil(columns);
    let (w, h) = tiles
        .iter()
        .fold((0, 0), |(w, h), t| (w.max(t.width()), h.max(t.height())));
    let mut sheet = RgbaImage::from_pixel(w * columns as u32, h * rows as u32, Rgba([255; 4]));
    for (at, tile) in tiles.iter().enumerate() {
        let (col, row) = ((at % columns) as i64, (at / columns) as i64);
        image::imageops::overlay(&mut sheet, tile, col * w as i64, row * h as i64);
    }
    let mut out = Vec::new();
    sheet.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: u32, h: u32, shade: u8) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([shade, shade, shade, 255]))
    }

    #[test]
    fn tiles_are_laid_in_rows_and_come_back_as_a_png() {
        let tiles = vec![blank(10, 10, 10), blank(10, 10, 20), blank(10, 10, 30)];
        let png = grid(&tiles, 2).unwrap();
        let back = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height()), (20, 20));
        assert_eq!(back.get_pixel(5, 5).0[0], 10);
        assert_eq!(back.get_pixel(15, 5).0[0], 20);
        assert_eq!(back.get_pixel(5, 15).0[0], 30);
        // The empty place is left white.
        assert_eq!(back.get_pixel(15, 15).0, [255; 4]);
        // One row when there are fewer tiles than columns.
        let one = image::load_from_memory(&grid(&tiles, 5).unwrap()).unwrap();
        assert_eq!((one.width(), one.height()), (30, 10));
        assert!(grid(&[], 3).is_err());
    }

    #[test]
    fn a_line_is_drawn_where_it_runs_and_clipped_to_the_plot() {
        let mut image = blank(20, 20, 255);
        let all = (0..20, 0..20);
        line(&mut image, (2.0, 10.0), (17.0, 10.0), &all);
        assert_eq!(*image.get_pixel(10, 10), INK);
        assert_eq!(*image.get_pixel(10, 3), Rgba([255; 4]));
        // An unbounded side: ends far off the image, drawn only where it is on it.
        line(&mut image, (5.0, -1e16), (5.0, 1e16), &all);
        assert_eq!(*image.get_pixel(5, 0), INK);
        assert_eq!(*image.get_pixel(5, 19), INK);
        // Not a number: nothing.
        let before = image.clone();
        line(&mut image, (f32::NAN, 1.0), (3.0, 3.0), &all);
        assert_eq!(image, before);
        // Outside the plotting area: nothing.
        let mut image = blank(20, 20, 255);
        line(&mut image, (2.0, 3.0), (17.0, 3.0), &(0..20, 5..15));
        assert!(image.pixels().all(|p| *p != INK));
        line(&mut image, (12.0, 0.0), (12.0, 19.0), &(0..20, 5..15));
        assert_eq!(*image.get_pixel(12, 4), Rgba([255; 4]));
        assert_eq!(*image.get_pixel(12, 10), INK);
    }

    #[test]
    fn every_outline_shape_is_drawn_where_the_mapper_puts_it() {
        use crate::gates::gate_types::{DEFAULT_LINE as STYLE, ShapeType};
        let gate = || ShapeType::Gate(Arc::from("g"));
        // Data 0 to 100 on each axis over a plot the size of a tile.
        let mapper = PlotMapper::new(
            TILE as f32,
            TILE as f32,
            0.0..=100.0,
            0.0..=100.0,
            0.0..=100.0,
            0.0..=100.0,
            flow_fcs::TransformType::Linear,
            flow_fcs::TransformType::Linear,
        );
        let inked = |shape: GateRenderShape| {
            let mut image = blank(TILE, TILE, 255);
            outline(&mut image, &shape, &mapper);
            image
        };
        let count = |image: &RgbaImage| image.pixels().filter(|p| **p == INK).count();
        let square = vec![(20.0, 20.0), (80.0, 20.0), (80.0, 80.0), (20.0, 80.0)];
        let closed = inked(GateRenderShape::Polygon {
            points: Arc::new(square.clone()),
            style: &STYLE,
            shape_type: gate(),
        });
        let open = inked(GateRenderShape::PolyLine {
            points: square,
            style: &STYLE,
            shape_type: gate(),
        });
        assert!(count(&closed) > count(&open), "the polygon closes");
        // Drawn in the middle of the plot, not at the data's own numbers.
        let (cx, cy) = mapper.data_to_pixel(20.0, 50.0, None, None);
        assert!(
            (-2..=2).any(|d| *closed.get_pixel((cx as i32 + d) as u32, cy as u32) == INK),
            "the left side at pixel ({cx}, {cy})"
        );
        assert!(
            count(&inked(GateRenderShape::Ellipse {
                center: (50.0, 50.0),
                radius_x: 25.0,
                radius_y: 12.0,
                degrees_rotation: 30.0,
                style: &STYLE,
                shape_type: gate(),
            })) > 40
        );
        assert!(
            count(&inked(GateRenderShape::Rectangle {
                x: 20.0,
                y: 20.0,
                width: 40.0,
                height: 40.0,
                style: &STYLE,
                shape_type: gate(),
            })) > 40
        );
    }
}
