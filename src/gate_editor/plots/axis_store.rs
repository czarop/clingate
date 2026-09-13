use anyhow::anyhow;
use polars::{frame::DataFrame, prelude::{CsvReadOptions, DataType, Field, Schema}};
use core::f32;
use dioxus::prelude::*;
use flow_fcs::{TransformType, Transformable};
use flow_gates::transforms::{
    Axis, get_plotting_area, pixel_to_raw, pixel_to_raw_y, raw_to_pixel, raw_to_pixel_y
};
use rustc_hash::FxBuildHasher;
use std::{ops::RangeInclusive, path::PathBuf, sync::Arc};

use polars::prelude::*;
use itertools::izip;
use crate::gate_editor::{AxisInfo, gates::GateId};

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
            return
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

                AxisInfo::new_from_raw(
                    p.clone(),
                    lower,
                    4194304.0,
                    transform,
                )
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

    fn set_axes_from_file(&mut self, path: PathBuf, source: ScalingInfoSource) -> anyhow::Result<()> {
        let configs = read_axis_configs(path, source)?;
        self.with_mut(|s| s.apply_axis_configs(configs));
        Ok(())
    }
}

/// The plain-data half of the axis store, so the scaling import can be tested
/// without a Dioxus runtime.
impl AxisStore {
    /// Register a batch of axis settings, replacing any existing entry for the
    /// same channel and recording the display order.
    pub fn apply_axis_configs(&mut self, configs: Vec<AxisInfo>) {
        for ai in configs {
            self.sorted_settings.insert(ai.param.clone());
            self.settings.insert(ai.param.fluoro.clone(), ai);
        }
    }
}

/// Parse a scaling export into axis settings.
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

    let primary_col = df.column("Feature Name (Primary)")?.str()?;
    let secondary_col = df.column("Feature Name (Secondary)")?.str()?;
    let scaling_col = df.column("Scaling Type")?.str()?;
    let cofactor_col = df.column("Cofactor")?.i64()?;
    let min_col = df.column("Min")?.i64()?;
    let max_col = df.column("Max")?.i64()?;

    let configs: Vec<AxisInfo> = izip!(
        primary_col,
        secondary_col,
        scaling_col,
        cofactor_col,
        min_col,
        max_col
    )
    .filter_map(
        |(prim_opt, sec_opt, scale_opt, cof_opt, min_opt, max_opt)| {
            // A blank secondary column reads back as null, not as "". Using `?`
            // on it dropped the whole row, which silently lost every channel
            // with no separate marker name - that is every scatter parameter
            // (FSC, SSC, Time), leaving them on default axis settings rather
            // than the ones Omiq exported.
            let primary = prim_opt?;
            let marker_name = match sec_opt {
                Some(marker) if !marker.is_empty() => marker,
                _ => primary,
            };

            let param = Param {
                marker: Arc::from(marker_name),
                fluoro: Arc::from(primary),
            };
            let transform = match scale_opt? {
                "Arcsinh" => TransformType::Arcsinh {
                    cofactor: cof_opt? as f32,
                },
                "None (linear)" => TransformType::Linear,
                other => {
                    println!(
                        "skipping axis {}: unsupported scaling type {other:?}",
                        param.fluoro
                    );
                    return None;
                }
            };

            let lower = transform.transform(&(min_opt? as f32));
            let upper = transform.transform(&(max_opt? as f32));

            Some(AxisInfo {
                param,
                axis_lower: lower,
                axis_upper: upper,
                transform,
            })
        },
    )
    .collect();

    Ok(configs)
}

pub enum ScalingInfoSource{
    Omiq
}

fn fetch_axes_from_omiq_csv(path: PathBuf,) -> anyhow::Result<DataFrame> {

    let schema = Schema::from_iter(vec![
        Field::new("Feature Name (Primary)".into(), DataType::String),
        Field::new("Feature Name (Secondary)".into(), DataType::String),
        Field::new("Scaling Type".into(), DataType::String),
        Field::new("Cofactor".into(), DataType::Int64),
        Field::new("Min".into(), DataType::Int64),
        Field::new("Max".into(), DataType::Int64),
        Field::new("Min Z".into(), DataType::Int64),
        Field::new("Max Z".into(), DataType::Int64),
    ]);

    let csv = CsvReadOptions::default()
        .with_has_header(true)
        .with_schema(Some(Arc::new(schema)))
        .try_into_reader_with_file_path(Some(path))?
        .finish()?;

    Ok(csv)
}
