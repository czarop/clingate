use flow_fcs::TransformType;

use crate::gate_editor::{
    gates::{
        gate_composite::skewed_quadrant_gate::{
            DataPoints, create_skewed_quadrant_geos, get_infinite_bounds,
        },
        gate_drag::{GateDragData, PointDragData},
        gate_single::{polygon_gate::PolygonGate, rescale_helper_point},
        gate_traits::DrawableGate,
        gate_types::{self, DEFAULT_LINE, GateRenderShape, GateStats, SELECTED_LINE, ShapeType},
    },
    plots::axis_store::PlotMapper,
};
use anyhow::Result;
use flow_gates::Gate;

use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;
use std::sync::Arc;
type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;

#[derive(PartialEq, Clone)]
pub struct QuadrantGate {
    gates: FxIndexMap<Arc<str>, PolygonGate>,
    id: Arc<str>,
    name: String,
    points: DataPoints,
    axis_matched: bool,
    parameters: (Arc<str>, Arc<str>),
    infs: (f32, f32),
}

impl QuadrantGate {
    pub fn try_new_from_raw_coord(
        plot_map: &PlotMapper,
        id: Arc<str>,
        name: String,
        click_loc_raw: (f32, f32),
        x_axis_param: Arc<str>,
        y_axis_param: Arc<str>,
    ) -> Result<Self> {
        let (cx, cy) = plot_map.pixel_to_data(click_loc_raw.0, click_loc_raw.1, None, None);
        let points = DataPoints::new_from_click(cx, cy, plot_map);

        let x_inf = get_infinite_bounds(&plot_map.get_x_transform());

        let y_inf = get_infinite_bounds(&plot_map.get_y_transform());

        let infs = (x_inf, y_inf);

        Self::try_new_from_data_points(
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
        mut data_points: DataPoints,
        x_axis_param: Arc<str>,
        y_axis_param: Arc<str>,
        axis_matched: bool,
        subgate_ids: Option<Vec<Arc<str>>>,
        subgate_names: Option<(String, String, String, String)>,
        infs: (f32, f32),
    ) -> Result<Self> {
        // FORCE ORTHOGONALITY: Overwrite any skew with center alignment
        data_points.left.1 = data_points.center.1;
        data_points.right.1 = data_points.center.1;
        data_points.top.0 = data_points.center.0;
        data_points.bottom.0 = data_points.center.0;

        let mut gate_map = FxIndexMap::default();
        let parameters = (x_axis_param.clone(), y_axis_param.clone());

        // Reuse the skewed geometry generator (orthogonal is just 0 skew)
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

    fn clone_with_point(&self, data_points: DataPoints, infs: Option<(f32, f32)>) -> Result<Self> {
        let gate_ids = self.gates.keys().cloned().collect();
        let mut it = self.gates.iter().map(|(_, v)| v.get_name().to_string());

        let gate_names = (
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 1"))?,
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 2"))?,
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 3"))?,
            it.next().ok_or_else(|| anyhow::anyhow!("Missing gate 4"))?,
        );
        Self::try_new_from_data_points(
            self.id.clone(),
            self.name.clone(),
            data_points,
            self.parameters.0.clone(),
            self.parameters.1.clone(),
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
                infs: self.infs.clone(),
            })
        }
    }
}

impl DrawableGate for QuadrantGate {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn is_finalised(&self) -> bool {
        true
    }
    fn get_name(&self) -> &str {
        &self.name
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
    fn clone_box(&self) -> Box<dyn DrawableGate> {
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

    fn draw_self(
        &self,
        is_selected: bool,
        drag_point: Option<PointDragData>,
        plot_map: &PlotMapper,
        gate_stats: &Option<GateStats>,
    ) -> Vec<GateRenderShape> {
        let (xmin, xmax) = {
            let a = plot_map.x_axis_min_max();
            (*a.start(), *a.end())
        };
        let (ymin, ymax) = {
            let a = plot_map.y_axis_min_max();
            (*a.start(), *a.end())
        };

        let mut center = self.points.center;

        if let Some(dd) = drag_point {
            let x_span = (xmax - xmin).abs();
            let y_span = (ymax - ymin).abs();
            // Center-only clamp for QuadrantGate
            if dd.point_index() == 0 {
                center = (
                    dd.loc().0.clamp(xmin + x_span * 0.1, xmax - x_span * 0.1),
                    dd.loc().1.clamp(ymin + y_span * 0.1, ymax - y_span * 0.1),
                );
            }
        }

        let style = if is_selected {
            &SELECTED_LINE
        } else {
            &DEFAULT_LINE
        };

        // Draw perfectly horizontal and vertical lines crossing at center
        let mut shapes = vec![
            GateRenderShape::Line {
                x1: xmin,
                y1: center.1,
                x2: center.0,
                y2: center.1,
                style,
                shape_type: ShapeType::UndraggableLine,
            },
            GateRenderShape::Line {
                x1: center.0,
                y1: center.1,
                x2: xmax,
                y2: center.1,
                style,
                shape_type: ShapeType::UndraggableLine,
            },
            GateRenderShape::Line {
                x1: center.0,
                y1: ymin,
                x2: center.0,
                y2: center.1,
                style,
                shape_type: ShapeType::UndraggableLine,
            },
            GateRenderShape::Line {
                x1: center.0,
                y1: center.1,
                x2: center.0,
                y2: ymax,
                style,
                shape_type: ShapeType::UndraggableLine,
            },
        ];

        if is_selected {
            shapes.push(GateRenderShape::Circle {
                center,
                radius: 4.0,
                fill: "red",
                shape_type: ShapeType::UndraggablePoint(0),
            });
        }

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

        shapes.extend_from_slice(&labels);

        shapes
    }

    fn replace_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        _anchor: Option<(f32, f32)>,
        mapper: &PlotMapper,
    ) -> Result<Box<dyn DrawableGate>> {
        if point_index != 0 {
            return Ok(Box::new(self.clone()));
        }

        let (xmin, xmax) = {
            let a = mapper.x_axis_min_max();
            (*a.start(), *a.end())
        };
        let (ymin, ymax) = {
            let a = mapper.y_axis_min_max();
            (*a.start(), *a.end())
        };

        let clamped_c = (
            new_point
                .0
                .clamp(xmin + (xmax - xmin) * 0.1, xmax - (xmax - xmin) * 0.1),
            new_point
                .1
                .clamp(ymin + (ymax - ymin) * 0.1, ymax - (ymax - ymin) * 0.1),
        );

        let new_pts = DataPoints {
            center: clamped_c,
            left: (xmin, clamped_c.1),
            right: (xmax, clamped_c.1),
            bottom: (clamped_c.0, ymin),
            top: (clamped_c.0, ymax),
            // x_data_range: self.points.x_data_range.clone(),
            // y_data_range: self.points.y_data_range.clone(),
        };

        Ok(Box::new(self.clone_with_point(new_pts, None)?))
    }

    fn recalculate_gate_for_new_axis_limits(
        &self,
        param: Arc<str>,
        lower: f32,
        upper: f32,
        _transform: &TransformType,
    ) -> Result<Option<Box<dyn DrawableGate>>> {
        super::usable_range(lower, upper)?;
        let is_x = param == self.parameters.0;
        let mut new_points = self.points.clone();
        let buffer = (upper - lower).abs() * 0.1;

        if is_x {
            new_points.center.0 = new_points.center.0.clamp(lower + buffer, upper - buffer);
            new_points.left.0 = lower;
            new_points.right.0 = upper;
            new_points.top.0 = new_points.center.0;
            new_points.bottom.0 = new_points.center.0;
        } else {
            new_points.center.1 = new_points.center.1.clamp(lower + buffer, upper - buffer);
            new_points.top.1 = upper;
            new_points.bottom.1 = lower;
            new_points.left.1 = new_points.center.1;
            new_points.right.1 = new_points.center.1;
        }

        Ok(Some(Box::new(self.clone_with_point(new_points, None)?)))
    }

    fn recalculate_gate_for_rescaled_axis(
        &self,
        param: Arc<str>,
        old_transform: &TransformType,
        new_transform: &TransformType,
        // data_range: (f32, f32),
        axis_range: (f32, f32),
    ) -> Result<Box<dyn DrawableGate>> {
        let (x_param, _) = &self.parameters;
        let is_x = x_param == &param;

        let mut c = rescale_helper_point(
            self.points.center,
            &param,
            x_param,
            old_transform,
            new_transform,
        )?;

        // Orthogonal quadrants only care about the center and the edges
        let new_lower = axis_range.0;
        let new_upper = axis_range.1;
        super::usable_range(new_lower, new_upper)?;
        let buffer = (new_upper - new_lower).abs() * 0.1;

        if is_x {
            c.0 = c.0.clamp(new_lower + buffer, new_upper - buffer);
        } else {
            c.1 = c.1.clamp(new_lower + buffer, new_upper - buffer);
        }

        let new_pts = DataPoints {
            center: c,
            left: (if is_x { new_lower } else { self.points.left.0 }, c.1),
            right: (if is_x { new_upper } else { self.points.right.0 }, c.1),
            bottom: (
                c.0,
                if !is_x {
                    new_lower
                } else {
                    self.points.bottom.1
                },
            ),
            top: (c.0, if !is_x { new_upper } else { self.points.top.1 }),
        };
        let infs = {
            let new_inf = get_infinite_bounds(&new_transform);
            if is_x {
                (new_inf, self.infs.1)
            } else {
                (self.infs.0, new_inf)
            }
        };
        Ok(Box::new(self.clone_with_point(new_pts, Some(infs))?))
    }

    fn match_to_plot_axis(
        &self,
        plot_x_param: &str,
        plot_y_param: &str,
    ) -> Result<Option<Box<dyn DrawableGate>>> {
        let mut new_gate_map = FxIndexMap::default();
        let mut swap_axis = false;
        for gate in self.gates.values() {
            if let Some(g) = gate.clone_polygon_for_axis_swap(plot_x_param, plot_y_param)? {
                swap_axis = true;
                new_gate_map.insert(gate.get_id(), g);
            } else {
                return Ok(None);
            }
        }
        Ok(Some(self.clone_with_gates(new_gate_map, swap_axis)))
    }

    // Boilerplate trait passthroughs
    fn get_gate_ref(&self, id: Option<&str>) -> Option<&Gate> {
        id.and_then(|id| self.gates.get(id))
            .and_then(|g| g.get_gate_ref(None))
    }
    fn get_inner_gate_ids(&self) -> Vec<Arc<str>> {
        self.gates.keys().cloned().collect()
    }
    fn rotate_gate(&self, _: (f32, f32)) -> Result<Option<Box<dyn DrawableGate>>> {
        Ok(None)
    }
    fn replace_points(&self, _: GateDragData) -> Result<Option<Box<dyn DrawableGate>>> {
        Ok(None)
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

    fn is_primary(&self) -> bool {
        true
    }
}
