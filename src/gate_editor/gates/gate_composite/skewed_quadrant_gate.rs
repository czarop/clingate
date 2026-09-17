use flow_fcs::TransformType;

use flow_gates::transforms::{raw_to_transformed, transformed_to_raw};
use flow_gates::{Gate, GateGeometry};
use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;
use std::ops::Index;
use std::{ops::RangeInclusive, sync::Arc};

use crate::gate_editor::gates::gate_types::{self, GateStats};
use crate::gate_editor::{
    gates::{
        gate_drag::PointDragData,
        gate_single::{polygon_gate::PolygonGate, rescale_helper_point},
        gate_traits::DrawableGate,
        gate_types::{DEFAULT_LINE, GateRenderShape, SELECTED_LINE, ShapeType},
    },
    plots::axis_store::PlotMapper,
};

type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;

#[derive(PartialEq, Clone, Debug)]
pub struct DataPoints {
    pub center: (f32, f32),
    pub left: (f32, f32),
    pub bottom: (f32, f32),
    pub right: (f32, f32),
    pub top: (f32, f32),
    // pub x_data_range: RangeInclusive<f32>,
    // pub y_data_range: RangeInclusive<f32>,
}

impl DataPoints {
    pub fn new_from_click(cx: f32, cy: f32, plot_map: &PlotMapper) -> Self {
        let (xmin, xmax) = {
            let axis = plot_map.x_axis_min_max();
            (*axis.start(), *axis.end())
        };
        let (ymin, ymax) = {
            let axis = plot_map.y_axis_min_max();
            (*axis.start(), *axis.end())
        };

        // 1. Calculate the same 10% visual buffers used in your drag/resize logic
        let x_span = (xmax - xmin).abs();
        let y_span = (ymax - ymin).abs();
        let x_buffer = x_span * 0.1;
        let y_buffer = y_span * 0.1;

        let x_min_safe = xmin + x_buffer;
        let x_max_safe = xmax - x_buffer;
        let y_min_safe = ymin + y_buffer;
        let y_max_safe = ymax - y_buffer;

        // 2. Clamp the initial click coordinates to the safe zone
        let safe_cx = cx.clamp(x_min_safe, x_max_safe);
        let safe_cy = cy.clamp(y_min_safe, y_max_safe);

        // 3. Derive handles from the safe center
        // Left/Right snap to X-axis edges but use the safe Y
        let left = (xmin, safe_cy);
        let right = (xmax, safe_cy);

        // Bottom/Top snap to Y-axis edges but use the safe X
        let bottom = (safe_cx, ymin);
        let top = (safe_cx, ymax);

        Self {
            center: (safe_cx, safe_cy),
            left,
            bottom,
            right,
            top,
            // x_data_range: plot_map.x_data_min_max(),
            // y_data_range: plot_map.y_data_min_max(),
        }
    }

    pub fn clone_for_swap_axis(&self) -> Self {
        Self {
            center: (self.center.1, self.center.0),
            left: (self.bottom.1, self.bottom.0),
            right: (self.top.1, self.top.0),
            bottom: (self.left.1, self.left.0),
            top: (self.right.1, self.right.0),
            // x_data_range: self.y_data_range.clone(),
            // y_data_range: self.x_data_range.clone(),
        }
    }

    pub fn new_from_data_center(
        cx: f32,
        cy: f32,
        x_axis_range: RangeInclusive<f32>,
        y_axis_range: RangeInclusive<f32>,
        // x_data_range: RangeInclusive<f32>,
        // y_data_range: RangeInclusive<f32>,
    ) -> Self {
        let (xmin, xmax) = (*x_axis_range.start(), *x_axis_range.end());
        let (ymin, ymax) = (*y_axis_range.start(), *y_axis_range.end());

        // 1. Clamp the center to the VISUAL axis boundaries
        // This prevents the center handle from being lost if Omiq data is off-plot
        let safe_cx = cx.clamp(xmin, xmax);
        let safe_cy = cy.clamp(ymin, ymax);

        // 2. Derive handles based on Axis limits to ensure lines hit the plot edges
        let left = (xmin, safe_cy);
        let right = (xmax, safe_cy);
        let bottom = (safe_cx, ymin);
        let top = (safe_cx, ymax);

        Self {
            center: (safe_cx, safe_cy),
            left,
            bottom,
            right,
            top,
        }
    }
}

#[derive(PartialEq, Clone)]
pub struct SkewedQuadrantGate {
    gates: FxIndexMap<Arc<str>, PolygonGate>,
    id: Arc<str>,
    name: String,
    points: DataPoints,
    axis_matched: bool,
    parameters: (Arc<str>, Arc<str>),
    infs: (f32, f32),
}

impl SkewedQuadrantGate {
    pub fn try_new_from_raw_coord(
        plot_map: &PlotMapper,
        id: Arc<str>,
        name: String,
        click_loc_raw: (f32, f32),
        x_axis_param: Arc<str>,
        y_axis_param: Arc<str>,
    ) -> anyhow::Result<Self> {
        let (cx, cy) = plot_map.pixel_to_data(click_loc_raw.0, click_loc_raw.1, None, None);
        let points = DataPoints::new_from_click(cx, cy, plot_map);

        let x_inf = get_infinite_bounds(&plot_map.get_x_transform());

        let y_inf = get_infinite_bounds(&plot_map.get_y_transform());

        let infs = (x_inf, y_inf);

        SkewedQuadrantGate::try_new_from_data_points(
            id,
            name,
            points,
            x_axis_param,
            y_axis_param,
            true,
            None,
            None,
            infs,
        )
    }

    pub fn try_new_from_data_points(
        id: Arc<str>,
        name: String,
        data_points: DataPoints,
        x_axis_param: Arc<str>,
        y_axis_param: Arc<str>,
        axis_matched: bool,
        subgate_ids: Option<Vec<Arc<str>>>,
        subgate_names: Option<(String, String, String, String)>,
        infs: (f32, f32),
    ) -> anyhow::Result<Self> {
        let mut gate_map = FxIndexMap::default();
        let parameters = (x_axis_param.clone(), y_axis_param.clone());
        let geos = create_skewed_quadrant_geos(
            data_points.clone(),
            &x_axis_param,
            &y_axis_param,
            infs.0,
            infs.1,
        )?;

        let sub_ids = if let Some(ids) = subgate_ids {
            ids
        } else {
            vec![
                Arc::from(format!("{id}_BL")),
                Arc::from(format!("{id}_BR")),
                Arc::from(format!("{id}_TR")),
                Arc::from(format!("{id}_TL")),
            ]
        };

        let names = {
            match subgate_names {
                Some((a, b, c, d)) => [a, b, c, d],
                None => [
                    sub_ids[0].to_string(),
                    sub_ids[1].to_string(),
                    sub_ids[2].to_string(),
                    sub_ids[3].to_string(),
                ],
            }
        };

        for (i, (id_arc, name)) in sub_ids.into_iter().zip(names.into_iter()).enumerate() {
            let geo = match i {
                0 => geos.0.clone(),
                1 => geos.1.clone(),
                2 => geos.2.clone(),
                _ => geos.3.clone(),
            };
            let g = Gate {
                id: id_arc.clone(),
                name,
                geometry: geo,
                mode: flow_gates::GateMode::Global,
                parameters: parameters.clone(),
                label_position: None,
            };
            gate_map.insert(id_arc, PolygonGate::try_new(g, false)?);
        }

        Ok(Self {
            gates: gate_map,
            id,
            name,
            points: data_points,
            axis_matched,
            parameters,
            infs,
        })
    }

    fn clone_with_gates(
        &self,
        gates: FxIndexMap<Arc<str>, PolygonGate>,
        swap_axis: bool,
    ) -> Box<dyn DrawableGate> {
        if swap_axis {
            let new_parameters = (self.parameters.1.clone(), self.parameters.0.clone());
            let new_points = self.points.clone_for_swap_axis();
            let infs = (self.infs.1, self.infs.0);
            Box::new(Self {
                gates,
                id: self.id.clone(),
                name: self.name.clone(),
                points: new_points,
                axis_matched: !self.axis_matched,
                parameters: new_parameters,
                infs,
            })
        } else {
            Box::new(Self {
                gates,
                id: self.id.clone(),
                name: self.name.clone(),
                points: self.points.clone(),
                axis_matched: self.axis_matched,
                parameters: self.parameters.clone(),
                infs: self.infs,
            })
        }
    }

    fn clone_with_point(
        &self,
        data_points: DataPoints,
        infs: Option<(f32, f32)>,
    ) -> anyhow::Result<Self> {
        let (x_axis_param, y_axis_param) = self.parameters.clone();
        let subgate_bl_id = self.gates.index(0).get_id();
        let subgate_br_id = self.gates.index(1).get_id();
        let subgate_tr_id = self.gates.index(2).get_id();
        let subgate_tl_id = self.gates.index(3).get_id();
        let mut it = self.gates.iter().map(|(_, v)| v.get_name().to_string());

        let gate_names = (
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 1"))?,
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 2"))?,
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 3"))?,
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 4"))?,
        );
        let gate_ids = vec![subgate_bl_id, subgate_br_id, subgate_tr_id, subgate_tl_id];
        SkewedQuadrantGate::try_new_from_data_points(
            self.id.clone(),
            self.name.clone(),
            data_points,
            x_axis_param,
            y_axis_param,
            self.axis_matched,
            Some(gate_ids),
            Some(gate_names),
            if infs.is_some() {
                infs.unwrap()
            } else {
                self.infs
            },
        )
    }

    pub fn get_subgate_map(&self) -> &FxIndexMap<Arc<str>, PolygonGate> {
        &self.gates
    }
}

impl super::super::gate_traits::DrawableGate for SkewedQuadrantGate {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn is_finalised(&self) -> bool {
        true
    }
    fn get_name(&self) -> &str {
        &self.name
    }
    fn draw_self(
        &self,
        is_selected: bool,
        drag_point: Option<PointDragData>,
        plot_map: &PlotMapper,
        gate_stats: &Option<GateStats>,
    ) -> Vec<GateRenderShape> {
        let (xmin, xmax) = {
            let axis = plot_map.x_axis_min_max();
            (*axis.start(), *axis.end())
        };
        let (ymin, ymax) = {
            let axis = plot_map.y_axis_min_max();
            (*axis.start(), *axis.end())
        };

        let (mut left, mut right, mut top, mut bottom, mut center) = (
            self.points.left,
            self.points.right,
            self.points.top,
            self.points.bottom,
            self.points.center,
        );

        if let Some(dd) = drag_point {
            let x_span = (xmax - xmin).abs();
            let y_span = (ymax - ymin).abs();
            let x_buffer = x_span * 0.1;
            let y_buffer = y_span * 0.1;

            let x_min_safe = xmin + x_buffer;
            let x_max_safe = xmax - x_buffer;
            let y_min_safe = ymin + y_buffer;
            let y_max_safe = ymax - y_buffer;
            match dd.point_index() {
                0 => {
                    center = (
                        dd.loc().0.clamp(x_min_safe, x_max_safe),
                        dd.loc().1.clamp(y_min_safe, y_max_safe),
                    );
                }
                // Left/Right: X is fixed to axis edge, clamp Y skew
                1 => left.1 = dd.loc().1.clamp(y_min_safe, y_max_safe),
                3 => right.1 = dd.loc().1.clamp(y_min_safe, y_max_safe),

                // Bottom/Top: Y is fixed to axis edge, clamp X skew
                2 => bottom.0 = dd.loc().0.clamp(x_min_safe, x_max_safe),
                4 => top.0 = dd.loc().0.clamp(x_min_safe, x_max_safe),
                _ => unreachable!(),
            }
        };

        let style = if is_selected {
            &SELECTED_LINE
        } else {
            &DEFAULT_LINE
        };

        let main = {
            let left = GateRenderShape::Line {
                x1: xmin,
                y1: left.1,
                x2: center.0,
                y2: center.1,
                style,
                shape_type: ShapeType::UndraggableLine,
            };
            let right = GateRenderShape::Line {
                x1: center.0,
                y1: center.1,
                x2: xmax,
                y2: right.1,
                style,
                shape_type: ShapeType::UndraggableLine,
            };

            let bottom = GateRenderShape::Line {
                x1: bottom.0,
                y1: ymin,
                x2: center.0,
                y2: center.1,
                style,
                shape_type: ShapeType::UndraggableLine,
            };

            let top = GateRenderShape::Line {
                x1: center.0,
                y1: center.1,
                x2: top.0,
                y2: ymax,
                style,
                shape_type: ShapeType::UndraggableLine,
            };

            Some(vec![left, right, top, bottom])
        };

        let selected = if is_selected {
            let c = GateRenderShape::Circle {
                center,
                radius: 3.0,
                fill: "red",
                shape_type: ShapeType::UndraggablePoint(0),
            };
            let l = GateRenderShape::Circle {
                center: (xmin, left.1),
                radius: 3.0,
                fill: "red",
                shape_type: ShapeType::UndraggablePoint(1),
            };
            let b = GateRenderShape::Circle {
                center: (bottom.0, ymin),
                radius: 3.0,
                fill: "red",
                shape_type: ShapeType::UndraggablePoint(2),
            };
            let r = GateRenderShape::Circle {
                center: (xmax, right.1),
                radius: 3.0,
                fill: "red",
                shape_type: ShapeType::UndraggablePoint(3),
            };

            let t = GateRenderShape::Circle {
                center: (top.0, ymax),
                radius: 3.0,
                fill: "red",
                shape_type: ShapeType::UndraggablePoint(4),
            };

            Some(vec![c, l, b, r, t])
        } else {
            None
        };

        let mut labels = vec![];
        if let Some(gate_stats) = gate_stats {
            let x_axis_min_max = plot_map.x_axis_min_max();
            let y_axis_min_max = plot_map.y_axis_min_max();
            let x_axis_offset = ((x_axis_min_max.end() - x_axis_min_max.start()) / 100f32) * 1f32;
            let y_axis_offset = ((y_axis_min_max.end() - y_axis_min_max.start()) / 100f32) * 1f32;
            for (i, (id, _)) in self.gates.iter().enumerate() {
                // order: bl, br, tr, tl
                if let Some(percent) = gate_stats.get_percent_for_id(id.clone()) {
                    let text = format!("{:.2}%", percent);
                    let (origin, offset, text_anchor) = match i {
                        0 => {
                            // ALWAYS BOTTOM LEFT
                            (
                                (
                                    *x_axis_min_max.start() + x_axis_offset,
                                    *y_axis_min_max.start() + y_axis_offset,
                                ),
                                self.gates.get(id).expect("").get_label_offset(),
                                Some(String::from("start")),
                            )
                        }
                        1 => {
                            // BOTTOM RIGHT LABEL
                            if self.axis_matched {
                                (
                                    (
                                        *x_axis_min_max.end() - x_axis_offset,
                                        *y_axis_min_max.start() + y_axis_offset,
                                    ),
                                    self.gates.get(id).expect("").get_label_offset(),
                                    Some(String::from("end")),
                                )
                            } else {
                                // TOP LEFT LABEL
                                (
                                    (
                                        *x_axis_min_max.start() + x_axis_offset,
                                        *y_axis_min_max.end() - 2f32 * y_axis_offset,
                                    ),
                                    self.gates.get(id).expect("").get_label_offset(),
                                    Some(String::from("start")),
                                )
                            }
                        }
                        2 => {
                            // ALWAYS TOP RIGHT
                            (
                                (
                                    *x_axis_min_max.end() - x_axis_offset,
                                    *y_axis_min_max.end() - 2f32 * y_axis_offset,
                                ),
                                self.gates.get(id).expect("").get_label_offset(),
                                Some(String::from("end")),
                            )
                        }
                        3 => {
                            // TOP LEFT LABEL
                            if self.axis_matched {
                                (
                                    (
                                        *x_axis_min_max.start() + x_axis_offset,
                                        *y_axis_min_max.end() - 2f32 * y_axis_offset,
                                    ),
                                    self.gates.get(id).expect("").get_label_offset(),
                                    Some(String::from("start")),
                                )
                            } else {
                                // BOTTOM RIGHT LABEL
                                (
                                    (
                                        *x_axis_min_max.end() - x_axis_offset,
                                        *y_axis_min_max.start() + y_axis_offset,
                                    ),
                                    self.gates.get(id).expect("").get_label_offset(),
                                    Some(String::from("end")),
                                )
                            }
                        }
                        _ => unreachable!(),
                    };
                    let shape = GateRenderShape::Text {
                        origin,
                        offset,
                        fontsize: 10f32,
                        text,
                        text_anchor,
                        shape_type: ShapeType::UndraggableText(gate_types::Direction::Both),
                    };
                    labels.push(shape)
                }
            }
        }

        let labels = if labels.is_empty() {
            None
        } else {
            Some(labels)
        };

        crate::collate_vecs!(main, selected, labels)
    }

    fn is_composite(&self) -> bool {
        true
    }

    fn get_id(&self) -> Arc<str> {
        self.id.clone()
    }

    fn get_params(&self) -> (Arc<str>, Arc<str>) {
        self.parameters.clone()
    }

    fn is_point_on_perimeter(
        &self,
        point: (f32, f32),
        tolerance: (f32, f32),
        plot_map: &PlotMapper,
    ) -> Option<f32> {
        let (xmin, xmax) = {
            let axis = plot_map.x_axis_min_max();
            (*axis.start(), *axis.end())
        };
        let (ymin, ymax) = {
            let axis = plot_map.y_axis_min_max();
            (*axis.start(), *axis.end())
        };

        let (left, bottom, right, top, center) = {
            (
                (xmin, self.points.left),
                (self.points.bottom, ymin),
                (xmax, self.points.right),
                (self.points.top, ymax),
                self.points.center,
            )
        };

        let mut closest = f32::INFINITY;

        if let Some(dis) = self.is_near_segment(point, left.1, center, tolerance) {
            closest = closest.min(dis);
        }
        if let Some(dis) = self.is_near_segment(point, center, right.1, tolerance) {
            closest = closest.min(dis);
        }
        if let Some(dis) = self.is_near_segment(point, center, bottom.0, tolerance) {
            closest = closest.min(dis);
        }
        if let Some(dis) = self.is_near_segment(point, center, top.0, tolerance) {
            closest = closest.min(dis);
        }

        if closest == f32::INFINITY {
            None
        } else {
            Some(closest)
        }
    }

    fn match_to_plot_axis(
        &self,
        plot_x_param: &str,
        plot_y_param: &str,
    ) -> anyhow::Result<Option<Box<dyn super::super::gate_traits::DrawableGate>>> {
        let mut new_gate_map = FxIndexMap::default();
        let mut swap_axis = false;
        for gate in self.gates.values() {
            match gate.clone_polygon_for_axis_swap(plot_x_param, plot_y_param) {
                Ok(Some(g)) => {
                    swap_axis = true;
                    new_gate_map.insert(gate.get_id(), g);
                }
                Ok(None) => {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Some(self.clone_with_gates(new_gate_map, swap_axis)))
    }

    fn recalculate_gate_for_rescaled_axis(
        &self,
        param: std::sync::Arc<str>,
        old_transform: &TransformType,
        new_transform: &TransformType,
        // data_range: (f32, f32),
        axis_range: (f32, f32),
    ) -> anyhow::Result<Box<dyn super::super::gate_traits::DrawableGate>> {
        let (x_param, _) = &self.parameters;
        let is_x = x_param == &param;
        let mut c = crate::gate_editor::gates::gate_single::rescale_helper_point(
            self.points.center,
            &param,
            x_param,
            old_transform,
            new_transform,
        )?;

        let (mut l, mut b, mut r, mut t) = {
            (
                rescale_helper_point(
                    self.points.left,
                    &param,
                    x_param,
                    old_transform,
                    new_transform,
                )?,
                rescale_helper_point(
                    self.points.bottom,
                    &param,
                    x_param,
                    old_transform,
                    new_transform,
                )?,
                rescale_helper_point(
                    self.points.right,
                    &param,
                    x_param,
                    old_transform,
                    new_transform,
                )?,
                rescale_helper_point(
                    self.points.top,
                    &param,
                    x_param,
                    old_transform,
                    new_transform,
                )?,
            )
        };

        let x_spec = match new_transform {
            TransformType::Linear => {
                let min = axis_range.0;
                let max = axis_range.1;
                let (nice_min, nice_max) = nice_bounds(min, max);
                nice_min..nice_max
            }
            TransformType::Arcsinh { cofactor: _ } | TransformType::Biexponential { .. } => {
                axis_range.0..axis_range.1
            }
        };

        let new_lower = x_spec.start;
        let new_upper = x_spec.end;

        let span = (new_upper - new_lower).abs();
        let buffer = span * 0.1;
        let min_safe = new_lower + buffer;
        let max_safe = new_upper - buffer;

        // 3. Apply the Clamping Logic
        if is_x {
            // Center and Skew handles must stay in the middle 80% of the NEW X-scale
            c.0 = c.0.clamp(min_safe, max_safe);
            t.0 = t.0.clamp(min_safe, max_safe);
            b.0 = b.0.clamp(min_safe, max_safe);

            // Snap edge handles strictly to the new axis limits
            l.0 = new_lower;
            r.0 = new_upper;
        } else {
            // Center and Skew handles must stay in the middle 80% of the NEW Y-scale
            c.1 = c.1.clamp(min_safe, max_safe);
            l.1 = l.1.clamp(min_safe, max_safe);
            r.1 = r.1.clamp(min_safe, max_safe);

            // Snap edge handles strictly to the new axis limits
            t.1 = new_upper;
            b.1 = new_lower;
        }

        let new = DataPoints {
            center: c,
            left: l,
            bottom: b,
            right: r,
            top: t,
        };
        let infs = {
            let new_inf = get_infinite_bounds(&new_transform);
            if is_x {
                (new_inf, self.infs.1)
            } else {
                (self.infs.0, new_inf)
            }
        };

        Ok(Box::new(self.clone_with_point(new, Some(infs))?))
    }

    fn rotate_gate(
        &self,
        _mouse_position: (f32, f32),
    ) -> anyhow::Result<Option<Box<dyn super::super::gate_traits::DrawableGate>>> {
        Ok(None)
    }

    fn replace_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        _anchor: Option<(f32, f32)>,
        mapper: &PlotMapper,
    ) -> anyhow::Result<Box<dyn super::super::gate_traits::DrawableGate>> {
        let (xmin, xmax) = {
            let axis = mapper.x_axis_min_max();
            (*axis.start(), *axis.end())
        };
        let (ymin, ymax) = {
            let axis = mapper.y_axis_min_max();
            (*axis.start(), *axis.end())
        };

        let (c, l, r, t, b) = (
            self.points.center,
            self.points.left,
            self.points.right,
            self.points.top,
            self.points.bottom,
        );

        let x_span = (xmax - xmin).abs();
        let y_span = (ymax - ymin).abs();
        let x_buffer = x_span * 0.1;
        let y_buffer = y_span * 0.1;

        let x_min_safe = xmin + x_buffer;
        let x_max_safe = xmax - x_buffer;
        let y_min_safe = ymin + y_buffer;
        let y_max_safe = ymax - y_buffer;

        let new = match point_index {
            0 => {
                // CENTER: Clamp both X and Y to the inner 80%
                let clamped_c = (
                    new_point.0.clamp(x_min_safe, x_max_safe),
                    new_point.1.clamp(y_min_safe, y_max_safe),
                );
                DataPoints {
                    center: clamped_c,
                    left: l,
                    bottom: b,
                    right: r,
                    top: t,
                    // x_data_range: self.points.x_data_range.clone(),
                    // y_data_range: self.points.y_data_range.clone(),
                }
            }
            1 => {
                // LEFT handle: Fixed to xmin, clamp Y skew
                DataPoints {
                    center: c,
                    left: (xmin, new_point.1.clamp(y_min_safe, y_max_safe)),
                    bottom: b,
                    right: r,
                    top: t,
                    // x_data_range: self.points.x_data_range.clone(),
                    // y_data_range: self.points.y_data_range.clone(),
                }
            }
            2 => {
                // BOTTOM handle: Fixed to ymin, clamp X skew
                DataPoints {
                    center: c,
                    left: l,
                    bottom: (new_point.0.clamp(x_min_safe, x_max_safe), ymin),
                    right: r,
                    top: t,
                    // x_data_range: self.points.x_data_range.clone(),
                    // y_data_range: self.points.y_data_range.clone(),
                }
            }
            3 => {
                // RIGHT handle: Fixed to xmax, clamp Y skew
                DataPoints {
                    center: c,
                    left: l,
                    bottom: b,
                    right: (xmax, new_point.1.clamp(y_min_safe, y_max_safe)),
                    top: t,
                    // x_data_range: self.points.x_data_range.clone(),
                    // y_data_range: self.points.y_data_range.clone(),
                }
            }
            4 => {
                // TOP handle: Fixed to ymax, clamp X skew
                DataPoints {
                    center: c,
                    left: l,
                    bottom: b,
                    right: r,
                    top: (new_point.0.clamp(x_min_safe, x_max_safe), ymax),
                    // x_data_range: self.points.x_data_range.clone(),
                    // y_data_range: self.points.y_data_range.clone(),
                }
            }
            _ => unreachable!(),
        };

        Ok(Box::new(self.clone_with_point(new, None)?))
    }

    fn replace_points(
        &self,
        _gate_drag_data: super::super::gate_drag::GateDragData,
    ) -> anyhow::Result<Option<Box<dyn super::super::gate_traits::DrawableGate>>> {
        Ok(None)
    }

    fn clone_box(&self) -> Box<dyn super::super::gate_traits::DrawableGate> {
        Box::new(self.clone())
    }

    /// Corner ids are positional, not derived from the old ones: an imported
    /// composite's corners carry Omiq's container ids, which say nothing about
    /// which corner they are. The map is built in a fixed geometric order, so
    /// index i is the corner these suffixes name.
    fn with_new_group_id(&self, new_id: Arc<str>) -> Option<Box<dyn DrawableGate>> {
        const SUFFIXES: [&str; 4] = ["_BL", "_BR", "_TR", "_TL"];
        if self.gates.len() != SUFFIXES.len() {
            return None;
        }
        let mut gates = FxIndexMap::default();
        for (corner, suffix) in self.gates.values().zip(SUFFIXES.iter()) {
            let id: Arc<str> = Arc::from(format!("{new_id}{suffix}").as_str());
            gates.insert(id.clone(), corner.with_id(id));
        }
        Some(Box::new(Self {
            gates,
            id: new_id,
            ..self.clone()
        }))
    }

    fn get_gate_ref(&self, id: Option<&str>) -> Option<&Gate> {
        if let Some(id) = id {
            if let Some(g) = self.gates.get(id) {
                g.get_gate_ref(None)
            } else {
                None
            }
        } else {
            None
        }
    }
    fn get_inner_gate_ids(&self) -> Vec<Arc<str>> {
        self.gates.keys().cloned().collect()
    }

    fn recalculate_gate_for_new_axis_limits(
        &self,
        param: std::sync::Arc<str>,
        lower: f32,
        upper: f32,
        _transform: &TransformType,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        let is_x = param == self.parameters.0;
        let mut new_points = self.points.clone();

        // Calculate the 10% safety buffers
        let span = (upper - lower).abs();
        let buffer = span * 0.1;
        let min_safe = lower + buffer;
        let max_safe = upper - buffer;

        if is_x {
            // --- X-AXIS UPDATED ---

            // 1. Clamp Center X (Keep it in the middle 80%)
            new_points.center.0 = new_points.center.0.clamp(min_safe, max_safe);

            // 2. Clamp Top and Bottom handles so they don't drift into the X-axis margins
            // These handles live at the top/bottom Y edges, but their X position
            // determines the vertical skew.
            new_points.top.0 = new_points.top.0.clamp(min_safe, max_safe);
            new_points.bottom.0 = new_points.bottom.0.clamp(min_safe, max_safe);

            // 3. Fix Left/Right handles to the new axis edges
            new_points.left.0 = lower;
            new_points.right.0 = upper;
        } else {
            // --- Y-AXIS UPDATED ---

            // 1. Clamp Center Y (Keep it in the middle 80%)
            new_points.center.1 = new_points.center.1.clamp(min_safe, max_safe);

            // 2. Clamp Left and Right handles so they don't drift into the Y-axis margins
            // These handles live at the left/right X edges, but their Y position
            // determines the horizontal skew.
            new_points.left.1 = new_points.left.1.clamp(min_safe, max_safe);
            new_points.right.1 = new_points.right.1.clamp(min_safe, max_safe);

            // 3. Fix Top/Bottom handles to the new axis edges
            new_points.top.1 = upper;
            new_points.bottom.1 = lower;
        }

        let new_self = self.clone_with_point(new_points, None)?;
        Ok(Some(Box::new(new_self)))
    }

    fn is_primary(&self) -> bool {
        true
    }
}

// pub fn create_skewed_quadrant_geos(
//     data_points: DataPoints,
//     x_channel: &str,
//     y_channel: &str,
// ) -> anyhow::Result<(GateGeometry, GateGeometry, GateGeometry, GateGeometry)> {
//     let c = data_points.center;
//     let b_x = data_points.bottom.0;
//     let l_y = data_points.left.1;
//     let t_x = data_points.top.0;
//     let r_y = data_points.right.1;

//     let x_min = data_points.left.0;
//     let x_max = data_points.right.0;
//     let y_min = data_points.bottom.1;
//     let y_max = data_points.top.1;

//     let x_limit_min = data_points.x_data_range.start().min(x_min).min(c.0);
//     let x_limit_max = data_points.x_data_range.end().max(x_max).max(c.0);
//     let y_limit_min = data_points.y_data_range.start().min(y_min).min(c.1);
//     let y_limit_max = data_points.y_data_range.end().max(y_max).max(c.1);

//     // Projected points (Spoke Ends)
//     let p_t = project_to_boundary(
//         c,
//         (t_x, y_max),
//         (x_limit_min, y_limit_min),
//         (x_limit_max, y_limit_max),
//     );
//     let p_b = project_to_boundary(
//         c,
//         (b_x, y_min),
//         (x_limit_min, y_limit_min),
//         (x_limit_max, y_limit_max),
//     );
//     let p_l = project_to_boundary(
//         c,
//         (x_min, l_y),
//         (x_limit_min, y_limit_min),
//         (x_limit_max, y_limit_max),
//     );
//     let p_r = project_to_boundary(
//         c,
//         (x_max, r_y),
//         (x_limit_min, y_limit_min),
//         (x_limit_max, y_limit_max),
//     );

//     // Universe Corners
//     let tl_c = (x_limit_min, y_limit_max);
//     let tr_c = (x_limit_max, y_limit_max);
//     let bl_c = (x_limit_min, y_limit_min);
//     let br_c = (x_limit_max, y_limit_min);

//     let tl = vec![c, p_l, tl_c, p_t];

//     let tr = vec![c, p_t, tr_c, p_r];

//     let br = vec![c, p_r, br_c, p_b];

//     let bl = vec![c, p_b, bl_c, p_l];

//     // Bottom-Left (BL)
//     let bl = flow_gates::geometry::create_polygon_geometry(bl, x_channel, y_channel)
//         .map_err(|_| anyhow::anyhow!("failed bl"))?;

//     // Bottom-Right (BR)
//     let br = flow_gates::geometry::create_polygon_geometry(br, x_channel, y_channel)
//         .map_err(|_| anyhow::anyhow!("failed br"))?;

//     // Top-Right (TR)
//     let tr = flow_gates::geometry::create_polygon_geometry(tr, x_channel, y_channel)
//         .map_err(|_| anyhow::anyhow!("failed tr"))?;

//     // Top-Left (TL)
//     let tl = flow_gates::geometry::create_polygon_geometry(tl, x_channel, y_channel)
//         .map_err(|_| anyhow::anyhow!("failed tl"))?;

//     println!("gates created");
//     Ok((bl, br, tr, tl))
// }

// const INF: f32 = 1e10;
// const NEG_INF: f32 = -1e10;
// pub fn create_skewed_quadrant_geos(
//     datapoints: DataPoints,
//     x_channel: &str,
//     y_channel: &str,
// ) -> anyhow::Result<(GateGeometry, GateGeometry, GateGeometry, GateGeometry)> {
//     let center = datapoints.center;
//     let top_handle = datapoints.top;
//     let bottom_handle = datapoints.bottom;
//     let left_handle = datapoints.left;
//     let right_handle = datapoints.right;
//     // Define the "Universe" - ignore the data range entirely.
//     // This ensures the gate works for every file in the experiment.
//     let min_x = NEG_INF;
//     let max_x = INF;
//     let min_y = NEG_INF;
//     let max_y = INF;

//     // 1. Project the 4 handles to the "Infinite" boundaries
//     let p_t = project_to_boundary(center, top_handle, (min_x, min_y), (max_x, max_y));
//     let p_b = project_to_boundary(center, bottom_handle, (min_x, min_y), (max_x, max_y));
//     let p_l = project_to_boundary(center, left_handle, (min_x, min_y), (max_x, max_y));
//     let p_r = project_to_boundary(center, right_handle, (min_x, min_y), (max_x, max_y));

//     // 2. Define the corners of our infinite universe
//     let tl_corner = (min_x, max_y);
//     let tr_corner = (max_x, max_y);
//     let bl_corner = (min_x, min_y);
//     let br_corner = (max_x, min_y);

//     // 3. Construct the 4 Polygons
//     // Note: The order of points matters (Clockwise or Counter-Clockwise)
//     let tl_poly = vec![center, p_l, tl_corner, p_t];
//     let tr_poly = vec![center, p_t, tr_corner, p_r];
//     let br_poly = vec![center, p_r, br_corner, p_b];
//     let bl_poly = vec![center, p_b, bl_corner, p_l];

//     Ok((
//         flow_gates::geometry::create_polygon_geometry(bl_poly, x_channel, y_channel)?,
//         flow_gates::geometry::create_polygon_geometry(br_poly, x_channel, y_channel)?,
//         flow_gates::geometry::create_polygon_geometry(tr_poly, x_channel, y_channel)?,
//         flow_gates::geometry::create_polygon_geometry(tl_poly, x_channel, y_channel)?,
//     ))
// }

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(usize)]
enum Wall {
    Top = 0,
    Right = 1,
    Bottom = 2,
    Left = 3,
}

fn get_wall(p: (f32, f32), x_inf: f32, y_inf: f32) -> Wall {
    let epsilon = 1.0;
    // Check Y boundaries first for Top/Bottom, then X for Right/Left
    if (p.1 - y_inf).abs() < epsilon {
        Wall::Top
    } else if (p.0 - x_inf).abs() < epsilon {
        Wall::Right
    } else if (p.1 + y_inf).abs() < epsilon {
        Wall::Bottom
    } else {
        Wall::Left
    }
}

// Corners ordered Clockwise: TR, BR, BL, TL
fn get_corners(x_inf: f32, y_inf: f32) -> [(f32, f32); 4] {
    [
        (x_inf, y_inf),   // Top-Right
        (x_inf, -y_inf),  // Bottom-Right
        (-x_inf, -y_inf), // Bottom-Left
        (-x_inf, y_inf),  // Top-Left
    ]
}

fn build_poly(
    center: (f32, f32),
    start: (f32, f32),
    end: (f32, f32),
    x_inf: f32,
    y_inf: f32,
) -> Vec<(f32, f32)> {
    let mut poly = vec![center, start];

    let corners = get_corners(x_inf, y_inf);
    let start_wall_idx = get_wall(start, x_inf, y_inf) as usize;
    let end_wall_idx = get_wall(end, x_inf, y_inf) as usize;

    let mut current_wall = start_wall_idx;

    // Walk corners clockwise until we reach the wall of the 'end' spoke
    while current_wall != end_wall_idx {
        poly.push(corners[current_wall]);
        current_wall = (current_wall + 1) % 4;
    }

    poly.push(end);
    poly.push(center);
    poly
}

pub fn create_skewed_quadrant_geos(
    datapoints: DataPoints,
    x_channel: &str,
    y_channel: &str,
    x_inf: f32,
    y_inf: f32,
) -> anyhow::Result<(GateGeometry, GateGeometry, GateGeometry, GateGeometry)> {
    let c = datapoints.center;

    // 1. Project spokes to the rectangular boundary
    let p_t = project_to_boundary(c, datapoints.top, (-x_inf, -y_inf), (x_inf, y_inf));
    let p_r = project_to_boundary(c, datapoints.right, (-x_inf, -y_inf), (x_inf, y_inf));
    let p_b = project_to_boundary(c, datapoints.bottom, (-x_inf, -y_inf), (x_inf, y_inf));
    let p_l = project_to_boundary(c, datapoints.left, (-x_inf, -y_inf), (x_inf, y_inf));

    // 2. Build each quadrant with the rectangular limits
    // Order: TR (T to R), BR (R to B), BL (B to L), TL (L to T)
    let tr_poly = build_poly(c, p_t, p_r, x_inf, y_inf);
    let br_poly = build_poly(c, p_r, p_b, x_inf, y_inf);
    let bl_poly = build_poly(c, p_b, p_l, x_inf, y_inf);
    let tl_poly = build_poly(c, p_l, p_t, x_inf, y_inf);

    Ok((
        flow_gates::geometry::create_polygon_geometry(bl_poly, x_channel, y_channel)?,
        flow_gates::geometry::create_polygon_geometry(br_poly, x_channel, y_channel)?,
        flow_gates::geometry::create_polygon_geometry(tr_poly, x_channel, y_channel)?,
        flow_gates::geometry::create_polygon_geometry(tl_poly, x_channel, y_channel)?,
    ))
}

pub fn project_to_boundary(
    center: (f32, f32),
    handle: (f32, f32),
    min: (f32, f32),
    max: (f32, f32),
) -> (f32, f32) {
    let dx = handle.0 - center.0;
    let dy = handle.1 - center.1;

    // Avoid division by zero for perfectly vertical/horizontal spokes
    if dx.abs() < 1e-6 {
        return (center.0, if dy > 0.0 { max.1 } else { min.1 });
    }
    if dy.abs() < 1e-6 {
        return (if dx > 0.0 { max.0 } else { min.0 }, center.1);
    }

    // Calculate "time" t to hit each of the 4 boundaries
    // Line eq: P = C + t(H - C)
    let t_x = if dx > 0.0 {
        (max.0 - center.0) / dx
    } else {
        (min.0 - center.0) / dx
    };
    let t_y = if dy > 0.0 {
        (max.1 - center.1) / dy
    } else {
        (min.1 - center.1) / dy
    };

    // We want the first boundary hit (the smallest t > 0)
    let t = t_x.min(t_y);

    (center.0 + t * dx, center.1 + t * dy)
}

fn nice_bounds(min: f32, max: f32) -> (f32, f32) {
    if min.is_infinite() || max.is_infinite() || min.is_nan() || max.is_nan() {
        return (0.0, 1.0); // Fallback for invalid ranges
    }

    let range = max - min;
    if range == 0.0 {
        return (min - 0.5, min + 0.5); // Handle single-point case
    }

    // Find nice step size
    let step_size = 10_f32.powf((range.log10()).floor());
    let nice_min = (min / step_size).floor() * step_size;
    let nice_max = (max / step_size).ceil() * step_size;

    (nice_min, nice_max)
}

pub fn get_infinite_bounds(transform: &TransformType) -> f32 {
    let physical_max = 100_000_000.0; // 10^8: safely beyond any detector limit

    match transform {
        TransformType::Linear => physical_max,
        TransformType::Arcsinh { cofactor } => (physical_max / cofactor).asinh() + 5.0,
        TransformType::Biexponential { .. } => todo!(),
    }
}
