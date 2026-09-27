//! Drawing one plot with no component around it.
//!
//! The editor builds a plot out of a chain of `use_resource`s: open the file,
//! scale it, filter it, index it, render it. That is right for a tab showing
//! two plots, where each step's result is worth keeping - the frame survives a
//! gate drag, the index survives a re-render.
//!
//! The gallery wants the opposite. It draws twenty plots of twenty different
//! files, and the only thing it keeps is the picture: a few tens of kilobytes
//! against the tens of megabytes of DataFrame that produced it. So the whole
//! pipeline is one blocking function that returns the image and drops
//! everything else, and the caller runs it on a pool.
//!
//! It is deliberately the *same* pipeline the editor uses, step for step and
//! function for function - [`crate::events`] to read, filter and index,
//! `get_percent_and_counts_gate`, `DensityPlot`. A gallery whose percentages
//! disagreed with the editor's by a rounding step would be worse than no
//! gallery, because the whole point of it is to be trusted at a glance.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use flow_plots::{
    BasePlotOptions, ColorMaps, DensityPlot, DensityPlotOptions, Plot, ScatterPlotData,
    render::RenderConfig,
};
use rustc_hash::FxHashMap;

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::gate_stats::get_percent_and_counts_gate;
use crate::gate_editor::gates::gate_store::{GateId, GateOverrideResolver};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::gates::gate_types::GateStats;
use crate::gate_editor::plots::axis_store::PlotMapper;

/// One plot's worth of work.
///
/// Everything here is owned, because the job crosses onto a blocking thread.
/// The gates arrive already resolved for this file and already matched to the
/// plot's axes - that is the gate store's business, not this module's, and
/// doing it here would mean holding a store lock on a worker thread.
pub struct PlotJob {
    pub path: PathBuf,
    /// What the file is compensated with.
    pub compensation: crate::compensation::Choice,
    /// Channel and cofactor for every arcsinh axis, as the axis store has them.
    pub cofactors: Vec<(Arc<str>, f32)>,
    /// The gates that filter this plot, outermost first. Empty draws every
    /// event, which is what the root shows.
    pub chain: Vec<GateId>,
    pub resolver: GateOverrideResolver,
    pub x: Arc<str>,
    pub y: Arc<str>,
    pub x_axis: AxisInfo,
    pub y_axis: AxisInfo,
    /// The gates to draw, in the order they should be drawn.
    pub gates: Vec<Arc<dyn DrawableGate>>,
    pub size: u32,
}

/// A drawn plot, and nothing that made it.
#[derive(Clone)]
pub struct PlotImage {
    /// The PNG, as an `<img src>` can take it.
    pub src: String,
    /// The same PNG unencoded, for the PDF - which embeds its compressed
    /// pixels exactly as they are rather than decoding and re-compressing.
    pub png: Arc<Vec<u8>>,
    /// Data coordinates to pixels, for drawing the gates on top. The same
    /// mapper the editor's overlay uses, so an outline lands identically.
    pub mapper: Arc<PlotMapper>,
    /// How many events the parent chain let through - the denominator every
    /// percentage on this plot is taken against.
    pub parent_events: usize,
    pub stats: FxHashMap<Arc<str>, GateStats>,
}

impl PartialEq for PlotImage {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.png, &other.png)
    }
}

/// Open, scale, gate, index, measure and draw. Blocking; call it on a pool.
pub fn render_plot(job: &PlotJob) -> anyhow::Result<PlotImage> {
    // Read exactly as the editor reads it: see [`crate::events`].
    let scaled = crate::events::read_scaled(&job.path, &job.compensation, &job.cofactors)?;
    let frame = crate::events::under_chain(&scaled, &job.chain, &job.resolver)?;
    drop(scaled);
    let parent_events = frame.height();

    let points = crate::events::points(&frame, &job.x, &job.y)?;
    // The row index is carried because `get_percent_and_counts_gate` takes
    // the mapped pair, and sharing that function verbatim is what keeps the
    // numbers identical to the editor's; nothing here reads individual events.
    let mapped = crate::events::index_mapped(&frame, &job.x, &job.y)?;

    let mut stats = FxHashMap::default();
    for gate in &job.gates {
        // A gate that cannot be measured is not a reason to lose the plot: the
        // picture is the point, the number is an annotation on it. The plot
        // draws with that one gate's label missing rather than as an error
        // message where a plot should be.
        if let Ok(s) = get_percent_and_counts_gate(gate.clone(), &mapped, parent_events as f32) {
            stats.insert(gate.get_id(), s);
        }
    }
    drop(mapped);
    drop(frame);

    let (png, mapper) = draw(points, job)?;
    Ok(PlotImage {
        src: format!("data:image/png;base64,{}", BASE64_STANDARD.encode(&png)),
        png: Arc::new(png),
        mapper: Arc::new(mapper),
        parent_events,
        stats,
    })
}

/// The bitmap, and the mapper that agrees with it.
fn draw(points: Vec<(f32, f32)>, job: &PlotJob) -> anyhow::Result<(Vec<u8>, PlotMapper)> {
    let bounds = bounds_of(&points);
    let size = job.size;

    let base = BasePlotOptions::new()
        .width(size)
        .height(size)
        .title("")
        .show_colorbar(false)
        .build()?;
    let x_options = flow_plots::AxisOptions::new()
        .range(job.x_axis.axis_lower..=job.x_axis.axis_upper)
        .transform(job.x_axis.transform.clone())
        .label(job.x_axis.param.to_string())
        .build()?;
    let y_options = flow_plots::AxisOptions::new()
        .range(job.y_axis.axis_lower..=job.y_axis.axis_upper)
        .transform(job.y_axis.transform.clone())
        .label(job.y_axis.param.to_string())
        .build()?;

    // Built from the options the image is drawn with, so the gates map to
    // the plotting area the events are in.
    let mapper = PlotMapper::for_plot(
        &base,
        *x_options.range.start()..=*x_options.range.end(),
        *y_options.range.start()..=*y_options.range.end(),
        bounds.0,
        bounds.1,
        job.x_axis.transform.clone(),
        job.y_axis.transform.clone(),
    );

    let options = DensityPlotOptions::new()
        .base(base.clone())
        .plot_type(flow_plots::PlotType::Density)
        .colormap(ColorMaps::Jet)
        .x_axis(x_options)
        .y_axis(y_options)
        .point_size(0.5)
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

/// The data's own extent, which the mapper needs alongside the axis range.
///
/// An empty frame still has to produce a mapper, because a gate drawn on a
/// population that selects nothing is exactly the case worth seeing. The axis
/// range is what positions the outline, so a degenerate data range is harmless
/// here; it only has to exist.
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
