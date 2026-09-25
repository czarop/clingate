use anyhow::anyhow;
use core::f32;
use dioxus::prelude::*;
use flow_fcs::{TransformType, Transformable};
use flow_gates::transforms::{
    Axis, get_plotting_area, pixel_to_raw, pixel_to_raw_y, raw_to_pixel, raw_to_pixel_y,
};
use polars::{
    frame::DataFrame,
    prelude::{CsvReadOptions, DataType, Field, Schema},
};
use rustc_hash::FxBuildHasher;
use std::{ops::RangeInclusive, path::PathBuf, sync::Arc};

use crate::gate_editor::{AxisInfo, gates::GateId};
use itertools::izip;
use polars::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub struct PlotMapper {
    view_width: f32,
    view_height: f32,
    x_data_axis_range: RangeInclusive<f32>,
    y_data_axis_range: RangeInclusive<f32>,
    x_data_range: RangeInclusive<f32>,
    y_data_range: RangeInclusive<f32>,
    x_transform: TransformType,
    y_transform: TransformType,
    x_pix_range: std::ops::Range<u32>,
    y_pix_range: std::ops::Range<u32>,
}

impl PlotMapper {
    pub fn new(
        width: f32,
        height: f32,
        x_data_axis_range: RangeInclusive<f32>,
        y_data_axis_range: RangeInclusive<f32>,
        x_data_range: RangeInclusive<f32>,
        y_data_range: RangeInclusive<f32>,
        x_transform: TransformType,
        y_transform: TransformType,
    ) -> Self {
        let (x_pix_range, y_pix_range) = get_plotting_area(width as u32, height as u32);

        Self {
            view_width: width,
            view_height: height,
            x_data_axis_range,
            y_data_axis_range,
            x_transform,
            y_transform,
            x_data_range,
            y_data_range,
            x_pix_range,
            y_pix_range,
        }
    }

    pub fn get_data_tolerance(&self, pixel_slop: f32) -> (f32, f32) {
        let x_span = self.x_data_axis_range.end() - self.x_data_axis_range.start();
        let y_span = self.y_data_axis_range.end() - self.y_data_axis_range.start();

        let plot_w = (self.x_pix_range.end - self.x_pix_range.start) as f32;
        let plot_h = (self.y_pix_range.end - self.y_pix_range.start) as f32;

        (
            (pixel_slop / plot_w) * x_span.abs(),
            (pixel_slop / plot_h) * y_span.abs(),
        )
    }

    pub fn pixel_to_data(
        &self,
        px: f32,
        py: f32,
        x_t: Option<TransformType>,
        y_t: Option<TransformType>,
    ) -> (f32, f32) {
        let xt = x_t.unwrap_or(TransformType::Linear);
        let yt = y_t.unwrap_or(TransformType::Linear);
        let dx_raw = pixel_to_raw(px, &self.x_data_axis_range, &self.x_pix_range, &xt);
        let dy_raw = pixel_to_raw_y(py, &self.y_data_axis_range, &self.y_pix_range, &yt);
        (dx_raw, dy_raw)
    }

    pub fn pixel_x_to_data(&self, x: f32, t: Option<TransformType>) -> f32 {
        let xt = t.unwrap_or(TransformType::Linear);
        pixel_to_raw(x, &self.x_data_axis_range, &self.x_pix_range, &xt)
    }

    pub fn pixel_y_to_data(&self, y: f32, t: Option<TransformType>) -> f32 {
        let yt = t.unwrap_or(TransformType::Linear);
        pixel_to_raw_y(y, &self.y_data_axis_range, &self.y_pix_range, &yt)
    }

    pub fn data_to_pixel(
        &self,
        dx: f32,
        dy: f32,
        x_t: Option<TransformType>,
        y_t: Option<TransformType>,
    ) -> (f32, f32) {
        let xt = x_t.unwrap_or(TransformType::Linear);
        let yt = y_t.unwrap_or(TransformType::Linear);

        let px = raw_to_pixel(dx, &self.x_data_axis_range, &self.x_pix_range, &xt);
        let py = raw_to_pixel_y(dy, &self.y_data_axis_range, &self.y_pix_range, &yt);

        (px, py)
    }

    pub fn width(&self) -> f32 {
        self.view_width
    }
    pub fn height(&self) -> f32 {
        self.view_height
    }

    pub fn x_axis_min_max(&self) -> RangeInclusive<f32> {
        self.x_data_axis_range.clone()
    }

    pub fn y_axis_min_max(&self) -> RangeInclusive<f32> {
        self.y_data_axis_range.clone()
    }

    pub fn x_data_min_max(&self) -> RangeInclusive<f32> {
        self.x_data_range.clone()
    }

    pub fn y_data_min_max(&self) -> RangeInclusive<f32> {
        self.y_data_range.clone()
    }

    pub fn get_x_transform(&self) -> TransformType {
        self.x_transform.clone()
    }

    pub fn get_y_transform(&self) -> TransformType {
        self.y_transform.clone()
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Param {
    pub marker: Arc<str>,
    pub fluoro: Arc<str>,
}

impl std::fmt::Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.marker == self.fluoro {
            write!(f, "{}", self.marker)
        } else {
            let trimmed = if self.fluoro.ends_with("-A") {
                &self.fluoro[..self.fluoro.len().saturating_sub(2)]
            } else {
                &self.fluoro
            };
            write!(f, "{}-{}", self.marker, trimmed)
        }
    }
}

#[derive(Default, Clone, Store)]
pub struct AxisStore {
    // all settings
    pub settings: im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    //current file's param names listed by file's internal order
    pub sorted_settings: indexmap::IndexSet<Param, FxBuildHasher>,
}

#[store(pub name = AxisStoreImplExt)]
impl<Lens> Store<AxisStore, Lens> {
    fn add_new_default_axis_settings(&mut self, p: &Param, fcs_file: &flow_fcs::Fcs) {
        if self.settings().peek().contains_key(&p.fluoro) {
            return;
        }
        self.settings()
            .write()
            .entry(p.fluoro.clone())
            .or_insert_with(|| {
                // Determine transform based on channel metadata
                let transform = fcs_file
                    .parameters
                    .get(p.fluoro.as_ref())
                    .map(|t| {
                        if t.is_fluorescence() {
                            TransformType::Arcsinh { cofactor: 6000.0 }
                        } else {
                            TransformType::Linear
                        }
                    })
                    .unwrap_or(TransformType::Linear);

                // Set logical lower bounds based on transform type
                let lower = if matches!(transform, TransformType::Linear) {
                    0.0
                } else {
                    -10000.0
                };

                AxisInfo::new_from_raw(p.clone(), lower, 4194304.0, transform)
            });
    }

    fn update_cofactor(
        &mut self,
        id: &Arc<str>,
        cofactor: f32,
    ) -> anyhow::Result<(AxisInfo, AxisInfo)> {
        let mut old = None;
        let mut new = None;

        self.settings()
            .write()
            .entry(id.clone())
            .and_modify(|axis| {
                if let TransformType::Arcsinh { .. } = axis.transform {
                    let old_axis = std::mem::take(axis);
                    let new_axis = (old_axis)
                        .into_archsinh(cofactor)
                        .unwrap_or(old_axis.clone());
                    new = Some(new_axis.clone());
                    old = Some(old_axis);
                    *axis = new_axis;
                }
            });

        if let (Some(new), Some(old)) = (new, old) {
            return Ok((old, new));
        }

        Err(anyhow!("Could not find axis"))
    }

    fn update_lower(
        &mut self,
        id: &GateId,
        lower: f32,
    ) -> anyhow::Result<(f32, f32, TransformType)> {
        let mut old_upper = None;
        let mut new_lower = None;
        let mut transform = None;
        self.settings()
            .write()
            .entry(id.clone())
            .and_modify(|axis_arc| {
                old_upper = Some(axis_arc.axis_upper);
                let new_axis_data = axis_arc.into_new_lower(lower);
                new_lower = Some(new_axis_data.axis_lower);
                transform = Some(new_axis_data.transform.clone());
                *axis_arc = new_axis_data;
            });

        if let (Some(upper), Some(lower), Some(transform)) = (old_upper, new_lower, transform) {
            Ok((lower, upper, transform))
        } else {
            Err(anyhow!("error modifying axis for {}", id.clone()))
        }
    }
    fn update_upper(
        &mut self,
        id: &GateId,
        upper: f32,
    ) -> anyhow::Result<(f32, f32, TransformType)> {
        let mut new_upper = None;
        let mut old_lower = None;
        let mut transform = None;

        self.settings()
            .write()
            .entry(id.clone())
            .and_modify(|axis_arc| {
                old_lower = Some(axis_arc.axis_lower);
                let new_axis_data = axis_arc.into_new_upper(upper);
                new_upper = Some(new_axis_data.axis_upper);
                transform = Some(new_axis_data.transform.clone());
                *axis_arc = new_axis_data;
            });

        if let (Some(upper), Some(lower), Some(transform)) = (new_upper, old_lower, transform) {
            Ok((lower, upper, transform))
        } else {
            Err(anyhow!("error modifying axis for {}", id.clone()))
        }
    }

    /// Load a scaling file, replacing whatever scaling was loaded before.
    ///
    /// Replacing rather than merging: see [`AxisStore::replace_axis_configs`].
    /// Read and parsed before the store is touched, so a file that fails to
    /// parse leaves the previous scaling exactly as it was.
    fn set_axes_from_file(
        &mut self,
        path: PathBuf,
        source: ScalingInfoSource,
    ) -> anyhow::Result<()> {
        let configs = read_axis_configs(path, source)?;
        self.with_mut(|s| s.replace_axis_configs(configs));
        Ok(())
    }
}

/// How one channel's scaling differs between what is loaded and a new file.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelChange {
    pub channel: Arc<str>,
    pub old: AxisInfo,
    pub new: AxisInfo,
}

impl ChannelChange {
    /// The gates on this channel have to be carried through the new
    /// transform - `rescale_gates`, what the editor's cofactor box does.
    pub fn transform_changed(&self) -> bool {
        self.old.transform != self.new.transform
    }

    /// The channel's range moved - `set_current_axis_limits`, what the
    /// editor's lower and upper boxes do. Only the composite gates, whose
    /// extent comes from the axis range, actually change.
    pub fn range_changed(&self) -> bool {
        self.old.axis_lower != self.new.axis_lower || self.old.axis_upper != self.new.axis_upper
    }
}

/// What replacing the loaded scaling with `new` would change.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScalingDiff {
    /// Channels in both whose settings differ, in channel order.
    pub changed: Vec<ChannelChange>,
    /// Channels the loaded scaling has and the new file does not. Gates on
    /// these cannot be carried across - there is no new transform to carry
    /// them to - so they are left as they are and reported.
    pub dropped: Vec<Arc<str>>,
}

/// Compare the loaded scaling with a new file's.
///
/// Replacing the scaling is then exactly the edits a person could make by
/// hand in the editor, channel by channel: the same `rescale_gates` and
/// `set_current_axis_limits` calls, so gate positions - drawn, per specimen
/// and per sample - come across rather than being re-imported and lost.
pub fn scaling_diff(
    loaded: &im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    new: &[AxisInfo],
) -> ScalingDiff {
    let mut changed: Vec<ChannelChange> = new
        .iter()
        .filter_map(|incoming| {
            let channel = &incoming.param.fluoro;
            let current = loaded.get(channel)?;
            let change = ChannelChange {
                channel: channel.clone(),
                old: current.clone(),
                new: incoming.clone(),
            };
            (change.transform_changed() || change.range_changed()).then_some(change)
        })
        .collect();
    changed.sort_by(|a, b| a.channel.cmp(&b.channel));

    let mut dropped: Vec<Arc<str>> = loaded
        .keys()
        .filter(|channel| {
            !new.iter()
                .any(|incoming| incoming.param.fluoro == **channel)
        })
        .cloned()
        .collect();
    dropped.sort();

    ScalingDiff { changed, dropped }
}

/// The plain-data half of the axis store, so the scaling import can be tested
/// without a Dioxus runtime.
impl AxisStore {
    /// Register a batch of axis settings, replacing any existing entry for the
    /// same channel and recording the display order.
    ///
    /// Merges: a channel not in `configs` keeps whatever it had. That suits a
    /// store fed from more than one source - the original design had per-file
    /// defaults overlaid by the scaling export - and it is what loading a
    /// replacement scaling file must *not* do. Use
    /// [`AxisStore::replace_axis_configs`] for that.
    pub fn apply_axis_configs(&mut self, configs: Vec<AxisInfo>) {
        for ai in configs {
            self.sorted_settings.insert(ai.param.clone());
            self.settings.insert(ai.param.fluoro.clone(), ai);
        }
    }

    /// Discard the current scaling and take `configs` as the whole of it.
    ///
    /// What loading a scaling file means. Merging - which is what this store
    /// did from the start, when the scaling was only ever loaded once - would
    /// keep channels the new file does not mention, carrying the old file's
    /// cofactors and ranges for them into a workspace that never had them; and
    /// it would keep the old display order for every channel the two files
    /// share.
    ///
    /// Also discards any cofactor or range edited in the editor since the last
    /// load. That is what replacing the scaling asks for.
    ///
    /// Not safe on its own under loaded gates: quadrant and skewed-quadrant
    /// gates take their extent from the axis range and transform *when they
    /// are imported*, and the import fails outright for a gate on an axis the
    /// scaling does not carry. Whoever replaces the scaling has to re-import
    /// the gating file after it.
    pub fn replace_axis_configs(&mut self, configs: Vec<AxisInfo>) {
        self.settings.clear();
        self.sorted_settings.clear();
        self.apply_axis_configs(configs);
    }

    /// Position of a channel in the display order, matched on the channel alone.
    ///
    /// `Param` compares on both fields, so a caller that only knows the channel
    /// - the hardcoded FSC-A/SSC-A defaults, say - cannot build a key that
    /// matches: the marker name comes from the scaling export and is not
    /// guessable. Matching on `fluoro` is what those callers actually mean.
    pub fn index_of_fluoro(&self, fluoro: &str) -> Option<usize> {
        index_of_fluoro(&self.sorted_settings, fluoro)
    }

    /// The full `Param` for a channel, including the marker name the scaling
    /// export gave it.
    pub fn param_for_fluoro(&self, fluoro: &str) -> Option<&Param> {
        self.sorted_settings.iter().find(|p| &*p.fluoro == fluoro)
    }

    /// The two channels a freshly loaded file should open on. See
    /// [`default_axis_params`].
    pub fn default_axis_params(&self) -> Option<(Param, Param)> {
        default_axis_params(&self.sorted_settings)
    }
}

/// Display order of a channel, matched on the channel alone.
///
/// Free-standing so a caller holding only the channel list can subscribe to
/// that rather than to the whole store, which also changes on every cofactor
/// or axis-limit edit.
pub fn index_of_fluoro(
    sorted: &indexmap::IndexSet<Param, FxBuildHasher>,
    fluoro: &str,
) -> Option<usize> {
    sorted.iter().position(|p| &*p.fluoro == fluoro)
}

/// The axes to show once a scaling has loaded, given the ones showing now.
///
/// Each keeps its channel if the scaling still has it - taking the scaling's
/// own `Param` for it, since the marker name comes from the export and the one
/// held may be a placeholder - and falls back to [`default_axis_params`] if
/// not. So replacing the scaling keeps the axes a person chose, and a channel
/// the new file dropped does not leave an axis pointing at nothing.
///
/// `None` while no scaling has loaded.
pub fn resolve_axes(
    sorted: &indexmap::IndexSet<Param, FxBuildHasher>,
    x: &Param,
    y: &Param,
) -> Option<(Param, Param)> {
    let (default_x, default_y) = default_axis_params(sorted)?;
    let keep = |current: &Param, fallback: Param| -> Param {
        sorted
            .iter()
            .find(|p| p.fluoro == current.fluoro)
            .cloned()
            .unwrap_or(fallback)
    };
    Some((keep(x, default_x), keep(y, default_y)))
}

/// The two channels a freshly loaded file should open on: the scatter pair if
/// the file has it, otherwise the first two channels in display order.
///
/// `None` while no scaling export has been read, so a caller can tell "not
/// loaded yet" from "loaded, and these are the axes" instead of committing to
/// index 0 and showing whichever channel happens to be listed first.
pub fn default_axis_params(
    sorted: &indexmap::IndexSet<Param, FxBuildHasher>,
) -> Option<(Param, Param)> {
    let pick = |preferred: &str, fallback: usize| -> Option<Param> {
        sorted
            .iter()
            .find(|p| &*p.fluoro == preferred)
            .or_else(|| sorted.get_index(fallback))
            .or_else(|| sorted.get_index(0))
            .cloned()
    };
    Some((pick("FSC-A", 0)?, pick("SSC-A", 1)?))
}

/// The columns of an Omiq scaling export this reads, by name.
const PRIMARY: &str = "Feature Name (Primary)";
const SECONDARY: &str = "Feature Name (Secondary)";
const SCALING_TYPE: &str = "Scaling Type";
const COFACTOR: &str = "Cofactor";
const MIN: &str = "Min";
const MAX: &str = "Max";
const REQUIRED: [&str; 6] = [PRIMARY, SECONDARY, SCALING_TYPE, COFACTOR, MIN, MAX];
const NUMBERS: [&str; 3] = [COFACTOR, MIN, MAX];

/// Parse a scaling export into axis settings.
///
/// Columns are found by their names in the header, so their order does not
/// matter, and a file without one of them is refused by name. They used to be
/// read by position under a fixed schema: a file with its columns in another
/// order loaded without complaint and read the wrong ones, and one with a
/// column missing read every later value shifted (B-SCALE-1).
///
/// A file is refused whole if any channel in it could not be drawn on - a
/// cofactor of 0 or below, a Min not below its Max, a value missing - rather
/// than loaded and left to crash the gates laid out against it (B-AX-3).
/// Everything wrong with it is said at once.
///
/// A row whose scaling type this build does not model is skipped with a warning
/// rather than aborting: one unrecognised entry should not cost the user every
/// other axis in the file. This used to be `unreachable!()`.
pub fn read_axis_configs(
    path: PathBuf,
    source: ScalingInfoSource,
) -> anyhow::Result<Vec<AxisInfo>> {
    let df = match source {
        ScalingInfoSource::Omiq => fetch_axes_from_omiq_csv(path)?,
    };

    let primary_col = df.column(PRIMARY)?.str()?;
    let secondary_col = df.column(SECONDARY)?.str()?;
    let scaling_col = df.column(SCALING_TYPE)?.str()?;
    let cofactor_col = df.column(COFACTOR)?.f64()?;
    let min_col = df.column(MIN)?.f64()?;
    let max_col = df.column(MAX)?.f64()?;

    let mut configs = Vec::new();
    let mut problems = Vec::new();
    for (prim_opt, sec_opt, scale_opt, cof_opt, min_opt, max_opt) in izip!(
        primary_col,
        secondary_col,
        scaling_col,
        cofactor_col,
        min_col,
        max_col
    ) {
        // A row naming no channel is not a channel: a blank line at the end.
        let Some(primary) = prim_opt.filter(|p| !p.trim().is_empty()) else {
            continue;
        };
        // A blank secondary column reads back as null, not as "". Using `?`
        // on it dropped the whole row, which silently lost every channel
        // with no separate marker name - that is every scatter parameter
        // (FSC, SSC, Time), leaving them on default axis settings rather
        // than the ones Omiq exported.
        let marker_name = match sec_opt {
            Some(marker) if !marker.is_empty() => marker,
            _ => primary,
        };
        let param = Param {
            marker: Arc::from(marker_name),
            fluoro: Arc::from(primary),
        };
        let transform = match scale_opt {
            Some("Arcsinh") => match cof_opt {
                Some(cofactor) => TransformType::Arcsinh {
                    cofactor: cofactor as f32,
                },
                None => {
                    problems.push(format!("{primary} is arcsinh-scaled but has no {COFACTOR}"));
                    continue;
                }
            },
            Some("None (linear)") => TransformType::Linear,
            Some(other) => {
                println!("skipping axis {primary}: unsupported scaling type {other:?}");
                continue;
            }
            None => {
                problems.push(format!("{primary} has no {SCALING_TYPE}"));
                continue;
            }
        };
        let (Some(min), Some(max)) = (min_opt, max_opt) else {
            problems.push(format!("{primary} is missing its {MIN} or {MAX}"));
            continue;
        };

        let axis = AxisInfo {
            param,
            axis_lower: transform.transform(&(min as f32)),
            axis_upper: transform.transform(&(max as f32)),
            transform,
        };
        match axis.problem() {
            Some(problem) => problems.push(problem),
            None => configs.push(axis),
        }
    }

    if !problems.is_empty() {
        return Err(anyhow!(
            "the scaling file cannot be used: {}",
            problems.join("; ")
        ));
    }
    Ok(configs)
}

pub enum ScalingInfoSource {
    Omiq,
}

fn fetch_axes_from_omiq_csv(path: PathBuf) -> anyhow::Result<DataFrame> {
    // The header alone first, to check every column this reads is there and
    // to type each by name - a schema given by position is what read a
    // reordered file into the wrong columns.
    let header = CsvReadOptions::default()
        .with_has_header(true)
        .with_n_rows(Some(0))
        .try_into_reader_with_file_path(Some(path.clone()))?
        .finish()?;
    let names: Vec<String> = header
        .get_column_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|wanted| !names.iter().any(|name| name == wanted))
        .collect();
    if !missing.is_empty() {
        return Err(anyhow!(
            "the scaling file has no {} column{} - an Omiq scaling export has {}; this one has {}",
            missing
                .iter()
                .map(|m| format!("\"{m}\""))
                .collect::<Vec<_>>()
                .join(", "),
            if missing.len() == 1 { "" } else { "s" },
            REQUIRED.join(", "),
            names.join(", ")
        ));
    }

    // Every column typed, not only the ones read, so nothing is left to
    // inference. Numbers as decimals: an Int64 column refused a whole file
    // over one cofactor of 150.5.
    let schema = Schema::from_iter(names.iter().map(|name| {
        let dtype = if NUMBERS.contains(&name.as_str()) {
            DataType::Float64
        } else {
            DataType::String
        };
        Field::new(name.as_str().into(), dtype)
    }));
    let csv = CsvReadOptions::default()
        .with_has_header(true)
        .with_schema(Some(Arc::new(schema)))
        .try_into_reader_with_file_path(Some(path))?
        .finish()?;

    Ok(csv)
}
