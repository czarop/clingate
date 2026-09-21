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
//! function for function - `apply_arcsinh_transforms`,
//! `filter_events_by_hierarchy_to_mask`, `EventIndex::build`,
//! `get_percent_and_counts_gate`, `DensityPlot`. A gallery whose percentages
//! disagreed with the editor's by a rounding step would be worse than no
//! gallery, because the whole point of it is to be trusted at a glance.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use flow_fcs::Fcs;
use flow_gates::EventIndex;
use flow_plots::{
    BasePlotOptions, ColorMaps, DensityPlot, DensityPlotOptions, Plot, ScatterPlotData,
    render::RenderConfig,
};
use polars::prelude::*;
use rustc_hash::FxHashMap;

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::gate_filtering::filter_events_by_hierarchy_to_mask;
use crate::gate_editor::gates::gate_stats::get_percent_and_counts_gate;
use crate::gate_editor::gates::gate_store::{GateId, GateOverrideResolver};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::gates::gate_types::GateStats;
use crate::gate_editor::plots::axis_store::PlotMapper;
use crate::gate_editor::plots::plot_store::EventIndexMapped;

/// One plot's worth of work.
///
/// Everything here is owned, because the job crosses onto a blocking thread.
/// The gates arrive already resolved for this file and already matched to the
/// plot's axes - that is the gate store's business, not this module's, and
/// doing it here would mean holding a store lock on a worker thread.
pub struct PlotJob {
    pub path: PathBuf,
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
    /// The JPEG, as an `<img src>` can take it.
    pub src: String,
    /// The same JPEG unencoded, for the PDF - which embeds the bytes exactly as
    /// they are rather than re-compressing them.
    pub jpeg: Arc<Vec<u8>>,
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
        Arc::ptr_eq(&self.jpeg, &other.jpeg)
    }
}

/// Open, scale, gate, index, measure and draw. Blocking; call it on a pool.
pub fn render_plot(job: &PlotJob) -> anyhow::Result<PlotImage> {
    let fcs = Fcs::open(
        job.path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("file path is not valid UTF-8"))?,
    )?;

    // Only the channels this file actually carries.
    //
    // The cofactors come from the axis settings, which describe the whole panel
    // as the scaling file defines it. `apply_arcsinh_transforms` errors on the
    // first name it cannot find, so one channel missing from one file - a
    // shorter panel, a renamed detector - loses the entire plot rather than one
    // axis of it. Nothing measured changes by skipping it: a transform for a
    // column that is not there could not have affected this plot's x, y or
    // chain, all of which are columns that are. If a *gate* needs the missing
    // channel the filter below still fails, and says so, which is the right
    // answer to gating on a parameter the file does not have.
    let present: rustc_hash::FxHashSet<&str> = fcs
        .parameters
        .values()
        .map(|p| p.channel_name.as_ref())
        .collect();
    let refs: Vec<(&str, f32)> = job
        .cofactors
        .iter()
        .filter(|(k, _)| present.contains(k.as_ref()))
        .map(|(k, v)| (k.as_ref(), *v))
        .collect();
    let scaled = fcs.apply_arcsinh_transforms(refs.as_slice())?;
    // The editor carries a row index through so a filtered event can be traced
    // back to its row in the whole file. Nothing here needs that mapping - the
    // gallery reads counts, never individual events - but the index is built
    // anyway, because `get_percent_and_counts_gate` takes the mapped pair and
    // sharing that function verbatim is what keeps the numbers identical.
    let scaled = scaled.with_row_index("original_index".into(), None)?;

    let frame = if job.chain.is_empty() {
        scaled
    } else {
        let mask = filter_events_by_hierarchy_to_mask(&scaled, &job.chain, &job.resolver)?;
        scaled.filter(&mask)?
    };
    let parent_events = frame.height();

    let points = zip_columns(&frame, &job.x, &job.y)?;

    let index_map: Vec<usize> = frame
        .column("original_index")?
        .u32()?
        .into_iter()
        .flatten()
        .map(|v| v as usize)
        .collect();
    let mapped = build_index(&frame, &job.x, &job.y, index_map)?;

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

    let (jpeg, mapper) = draw(points, job)?;
    Ok(PlotImage {
        src: format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(&jpeg)),
        jpeg: Arc::new(jpeg),
        mapper: Arc::new(mapper),
        parent_events,
        stats,
    })
}

fn zip_columns(frame: &DataFrame, x: &str, y: &str) -> anyhow::Result<Vec<(f32, f32)>> {
    let xs = frame.column(x)?.f32()?;
    let ys = frame.column(y)?.f32()?;
    Ok(xs
        .into_iter()
        .zip(ys.into_iter())
        .filter_map(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        })
        .collect())
}

fn build_index(
    frame: &DataFrame,
    x: &str,
    y: &str,
    index_map: Vec<usize>,
) -> anyhow::Result<EventIndexMapped> {
    let xr = frame.column(x)?.f32()?.rechunk();
    let yr = frame.column(y)?.f32()?.rechunk();
    let xs = xr
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("x column is not contiguous"))?;
    let ys = yr
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("y column is not contiguous"))?;
    Ok(EventIndexMapped {
        event_index: Arc::new(EventIndex::build(xs, ys).map_err(|e| anyhow::anyhow!("{e}"))?),
        index_map: Arc::new(index_map),
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

    let mapper = PlotMapper::new(
        size as f32,
        size as f32,
        *x_options.range.start()..=*x_options.range.end(),
        *y_options.range.start()..=*y_options.range.end(),
        bounds.0,
        bounds.1,
        job.x_axis.transform.clone(),
        job.y_axis.transform.clone(),
    );

    let options = DensityPlotOptions::new()
        .base(base)
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
