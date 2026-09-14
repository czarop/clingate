use flow_fcs::TransformType;

use flow_gates::{Gate, GateGeometry};
use std::{ops::Index, sync::Arc};

use crate::{
    FxIndexMap,
    gate_editor::{
        gates::{
            gate_drag::PointDragData,
            gate_single::{line_gate::LineGate, rescale_helper_point},
            gate_traits::DrawableGate,
            gate_types::{
                self, DEFAULT_LINE, GREY_LINE_DASHED, GateRenderShape, GateStats, SELECTED_LINE,
                ShapeType,
            },
        },
        plots::axis_store::PlotMapper,
    },
};

#[derive(PartialEq, Clone)]
pub struct BisectorGate {
    gates: FxIndexMap<Arc<str>, LineGate>,
    id: Arc<str>,
    name: String,
    points: (f32, f32),
    axis_matched: bool,
    parameters: (Arc<str>, Arc<str>),
}

impl BisectorGate {
    pub fn try_new(
        plot_map: &PlotMapper,
        id: Arc<str>,
        name: String,
        click_loc: (f32, f32),
        x_axis_param: Arc<str>,
        y_axis_param: Arc<str>,
    ) -> anyhow::Result<Self> {
        let mut gate_map = FxIndexMap::default();
        let parameters = (x_axis_param.clone(), y_axis_param.clone());
        let click_data = plot_map.pixel_to_data(click_loc.0, click_loc.1, None, None);

        let geos = create_default_bisector(plot_map, click_loc.0, &x_axis_param, &y_axis_param)?;
        let id_left = format!("{id}_L");
        let id_right = format!("{id}_R");
        let id_left_arc: Arc<str> = Arc::from(id_left.as_str());
        let id_right_arc: Arc<str> = Arc::from(id_right.as_str());
        let gate_left = Gate {
            id: id_left_arc.clone(),
            name: id_left,
            geometry: geos.0,
            mode: flow_gates::GateMode::Global,
            parameters: parameters.clone(),
            label_position: None,
        };
        let gate_right = Gate {
            id: id_right_arc.clone(),
            name: id_right,
            geometry: geos.1,
            mode: flow_gates::GateMode::Global,
            parameters,
            label_position: None,
        };
        let lg_l = LineGate::try_new(gate_left, click_data.1, false)?;
        let lg_r = LineGate::try_new(gate_right, click_data.1, false)?;

        gate_map.insert(id_left_arc, lg_l);
        gate_map.insert(id_right_arc, lg_r);

        let points = click_data;

        Ok(Self {
            gates: gate_map,
            id,
            name,
            points,
            axis_matched: true,
            parameters: (x_axis_param, y_axis_param),
        })
    }

    pub fn try_new_from_data_center(
        id: Arc<str>,
        name: String,
        center: f32,
        x_axis_param: Arc<str>,
        y_axis_param: Arc<str>,
        subgate_ids: (Arc<str>, Arc<str>),
        subgate_names: Option<(String, String)>,
    ) -> anyhow::Result<Self> {
        let mut gate_map = FxIndexMap::default();
        let parameters = (x_axis_param.clone(), y_axis_param.clone());

        let geos = create_default_bisector_from_data(center, &x_axis_param, &y_axis_param)?;

        let id_left_arc = subgate_ids.0.clone();
        let id_right_arc = subgate_ids.1.clone();

        let (name_left, name_right) = {
            match subgate_names {
                Some((l, r)) => (l, r),
                None => (id_left_arc.to_string(), id_right_arc.to_string()),
            }
        };

        let gate_left = Gate {
            id: id_left_arc.clone(),
            name: name_left,
            geometry: geos.0,
            mode: flow_gates::GateMode::Global,
            parameters: parameters.clone(),
            label_position: None,
        };
        let gate_right = Gate {
            id: id_right_arc.clone(),
            name: name_right,
            geometry: geos.1,
            mode: flow_gates::GateMode::Global,
            parameters,
            label_position: None,
        };
        let lg_l = LineGate::try_new(gate_left, 0f32, false)?;
        let lg_r = LineGate::try_new(gate_right, 0f32, false)?;

        gate_map.insert(id_left_arc, lg_l);
        gate_map.insert(id_right_arc, lg_r);

        Ok(Self {
            gates: gate_map,
            id,
            name,
            points: (center, 0f32),
            axis_matched: true,
            parameters: (x_axis_param, y_axis_param),
        })
    }

    fn clone_with_gates(
        &self,
        gates: FxIndexMap<Arc<str>, LineGate>,
        swap_axis: bool,
    ) -> Box<dyn DrawableGate> {
        if swap_axis {
            let new_parameters = (self.parameters.1.clone(), self.parameters.0.clone());
            let new_points = (self.points.1, self.points.0);
            Box::new(Self {
                gates,
                id: self.id.clone(),
                name: self.name.clone(),
                points: new_points,
                axis_matched: !self.axis_matched,
                parameters: new_parameters,
            })
        } else {
            Box::new(Self {
                gates,
                id: self.id.clone(),
                name: self.name.clone(),
                points: self.points,
                axis_matched: self.axis_matched,
                parameters: self.parameters.clone(),
            })
        }
    }

    fn clone_with_gates_and_loc(
        &self,
        gates: FxIndexMap<Arc<str>, LineGate>,
        cx: f32,
        cy: f32,
    ) -> Box<dyn DrawableGate> {
        let new_points = (cx, cy);
        Box::new(Self {
            gates,
            id: self.id.clone(),
            name: self.name.clone(),
            points: new_points,
            axis_matched: self.axis_matched,
            parameters: self.parameters.clone(),
        })
    }

    fn clone_with_point(&self, cx: f32, cy: f32) -> anyhow::Result<Self> {
        let (x_channel, y_channel) = &self.parameters;
        let mut gate_map = FxIndexMap::default();
        let max_left = (cx, f32::MAX);
        let min_left = (f32::MIN, f32::MIN);
        let coords = vec![min_left, max_left];
        let g1 = flow_gates::geometry::create_rectangle_geometry(coords, x_channel, y_channel)
            .map_err(|_| anyhow::anyhow!("failed to create rectangle geometry"))?;

        let max_right = (f32::MAX, f32::MAX);
        let min_right = (cx, f32::MIN);
        let coords = vec![min_right, max_right];
        let g2 = flow_gates::geometry::create_rectangle_geometry(coords, x_channel, y_channel)
            .map_err(|_| anyhow::anyhow!("failed to create rectangle geometry"))?;

        let curr_gate_1 = self.gates.index(0);
        let curr_gate_2 = self.gates.index(1);
        let new_gate_1 = Gate {
            id: curr_gate_1.get_id(),
            name: curr_gate_1.inner.name.clone(),
            geometry: g1,
            mode: curr_gate_1.inner.mode.clone(),
            parameters: curr_gate_1.inner.parameters.clone(),
            label_position: curr_gate_1.inner.label_position.clone(),
        };
        let new_gate_2 = Gate {
            id: curr_gate_2.get_id(),
            name: curr_gate_2.inner.name.clone(),
            geometry: g2,
            mode: curr_gate_2.inner.mode.clone(),
            parameters: curr_gate_2.inner.parameters.clone(),
            label_position: curr_gate_2.inner.label_position.clone(),
        };
        let mut lg_l = LineGate::try_new(new_gate_1, cy, false)?;
        let mut lg_r = LineGate::try_new(new_gate_2, cy, false)?;

        lg_l.axis_matched = self.axis_matched;
        lg_r.axis_matched = self.axis_matched;

        gate_map.insert(curr_gate_1.get_id(), lg_l);
        gate_map.insert(curr_gate_2.get_id(), lg_r);

        let points = (cx, cy);

        Ok(Self {
            gates: gate_map,
            id: self.id.clone(),
            name: self.name.clone(),
            points,
            axis_matched: self.axis_matched,
            parameters: self.parameters.clone(),
        })
    }

    pub fn get_subgate_map(&self) -> &FxIndexMap<Arc<str>, LineGate> {
        &self.gates
    }
}

impl super::super::gate_traits::DrawableGate for BisectorGate {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn is_finalised(&self) -> bool {
        true
    }

    fn draw_self(
        &self,
        is_selected: bool,
        drag_point: Option<PointDragData>,
        plot_map: &PlotMapper,
        gate_stats: &Option<GateStats>,
    ) -> Vec<GateRenderShape> {
        let (min, max) = {
            let (xmin, xmax) = {
                let axis = plot_map.x_axis_min_max();
                (*axis.start(), *axis.end())
            };
            let (ymin, ymax) = {
                let axis = plot_map.y_axis_min_max();
                (*axis.start(), *axis.end())
            };
            ((xmin, ymin), (xmax, ymax))
        };
        let mut center = self.points;

        if let Some(dd) = drag_point {
            if self.axis_matched {
                center = (dd.loc().0, center.1);
            } else {
                center = (center.0, dd.loc().1);
            }
        };

        let center_tab_height = {
            if self.axis_matched {
                (max.1 - min.1) * 0.02
            } else {
                (max.0 - min.0) * 0.02
            }
        };
        let style = if is_selected {
            &SELECTED_LINE
        } else {
            &DEFAULT_LINE
        };

        let main = {
            let l;
            let center_tab;
            if self.axis_matched {
                l = GateRenderShape::Line {
                    x1: min.0,
                    y1: center.1,
                    x2: max.0,
                    y2: center.1,
                    style,
                    shape_type: ShapeType::CompositeGate(self.id.clone(), self.axis_matched),
                };

                center_tab = GateRenderShape::Line {
                    x1: center.0,
                    y1: center.1 - center_tab_height,
                    x2: center.0,
                    y2: center.1 + center_tab_height,
                    style,
                    shape_type: ShapeType::CompositeGate(self.id.clone(), self.axis_matched),
                };
            } else {
                l = GateRenderShape::Line {
                    x1: center.0,
                    y1: min.1,
                    x2: center.0,
                    y2: max.1,
                    style,
                    shape_type: ShapeType::CompositeGate(self.id.clone(), self.axis_matched),
                };

                center_tab = GateRenderShape::Line {
                    x1: center.0 - center_tab_height,
                    y1: center.1,
                    x2: center.0 + center_tab_height,
                    y2: center.1,
                    style,
                    shape_type: ShapeType::CompositeGate(self.id.clone(), self.axis_matched),
                };
            }

            Some(vec![l, center_tab])
        };

        let selected = if is_selected {
            let p = GateRenderShape::Circle {
                center,
                radius: 3.0,
                fill: "red",
                shape_type: ShapeType::CompositePoint(0, self.axis_matched),
            };
            let line = if self.axis_matched {
                GateRenderShape::Line {
                    x1: center.0,
                    y1: min.1,
                    x2: center.0,
                    y2: max.1,
                    style: &GREY_LINE_DASHED,
                    shape_type: ShapeType::UndraggableLine,
                }
            } else {
                GateRenderShape::Line {
                    x1: min.0,
                    y1: center.1,
                    x2: max.0,
                    y2: center.1,
                    style: &GREY_LINE_DASHED,
                    shape_type: ShapeType::UndraggableLine,
                }
            };

            Some(vec![line, p])
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
                if let Some(percent) = gate_stats.get_percent_for_id(id.clone()) {
                    let text = format!("{:.2}%", percent);
                    let (origin, offset, text_anchor) = if i == 0 {
                        // LEFT LABEL
                        if self.axis_matched {
                            (
                                (
                                    *x_axis_min_max.start() + x_axis_offset,
                                    self.points.1 + y_axis_offset,
                                ),
                                (0.0, 0.0),
                                Some(String::from("start")),
                            )
                        } else {
                            // BOTTOM LABEL
                            (
                                (
                                    self.points.0 + x_axis_offset,
                                    *y_axis_min_max.start() + y_axis_offset,
                                ),
                                (0.0, 0.0),
                                None,
                            )
                        }
                    } else {
                        // RIGHT LABEL
                        if self.axis_matched {
                            (
                                (
                                    *x_axis_min_max.end() - y_axis_offset,
                                    self.points.1 + y_axis_offset,
                                ),
                                (0.0, 0.0),
                                Some(String::from("end")),
                            )
                        } else {
                            // TOP LABEL
                            (
                                (
                                    self.points.0 + x_axis_offset,
                                    *y_axis_min_max.end() - 2f32 * y_axis_offset,
                                ),
                                (0.0, 0.0),
                                None,
                            )
                        }
                    };
                    let shape = GateRenderShape::Text {
                        origin,
                        offset,
                        fontsize: 10f32,
                        text,
                        text_anchor,
                        shape_type: if self.axis_matched {
                            ShapeType::UndraggableText(gate_types::Direction::X)
                        } else {
                            ShapeType::UndraggableText(gate_types::Direction::Y)
                        },
                    };
                    labels.push(shape)
                }
            }
        }

        let labels = Some(labels);

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
        let min = if self.axis_matched {
            (xmin, self.points.1)
        } else {
            (self.points.0, ymin)
        };
        let max = if self.axis_matched {
            (xmax, self.points.1)
        } else {
            (self.points.0, ymax)
        };
        if let Some(dis) = self.is_near_segment(point, min, max, tolerance) {
            return Some(dis);
        }
        None
    }

    fn match_to_plot_axis(
        &self,
        plot_x_param: &str,
        plot_y_param: &str,
    ) -> anyhow::Result<Option<Box<dyn super::super::gate_traits::DrawableGate>>> {
        let mut new_gate_map = FxIndexMap::default();
        let mut axis_matched = self.axis_matched;
        for gate in self.gates.values() {
            match gate.clone_line_for_axis_swap(plot_x_param, plot_y_param) {
                Ok(Some(g)) => {
                    axis_matched = g.axis_matched;
                    new_gate_map.insert(gate.get_id(), g);
                }
                Ok(None) => {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Some(self.clone_with_gates(
            new_gate_map,
            axis_matched != self.axis_matched,
        )))
    }

    fn recalculate_gate_for_rescaled_axis(
        &self,
        param: std::sync::Arc<str>,
        old_transform: &TransformType,
        new_transform: &TransformType,
        // _data_range: (f32, f32),
        _axis_range: (f32, f32),
    ) -> anyhow::Result<Box<dyn super::super::gate_traits::DrawableGate>> {
        let (x_param, _) = &self.parameters;
        let (cx, cy) =
            rescale_helper_point(self.points, &param, x_param, old_transform, new_transform)?;
        Ok(Box::new(self.clone_with_point(cx, cy)?))
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
        _point_index: usize,
        _mapper: &PlotMapper,
    ) -> anyhow::Result<Box<dyn super::super::gate_traits::DrawableGate>> {
        let mut new_gate_map = FxIndexMap::default();
        let new_cx = new_point.0;
        let new_cy = new_point.1;
        for (i, (id, gate)) in self.gates.iter().enumerate() {
            let p_index = if i == 0 { 1 } else { 0 };
            let old_p = gate.points[p_index];
            let target_p = if self.axis_matched {
                (new_cx, old_p.1)
            } else {
                (old_p.0, new_cy)
            };
            let new_gate = gate.clone_line_for_new_point(target_p, p_index)?;
            new_gate_map.insert(id.clone(), new_gate);
        }
        if self.axis_matched {
            Ok(self.clone_with_gates_and_loc(new_gate_map, new_point.0, self.points.1))
        } else {
            Ok(self.clone_with_gates_and_loc(new_gate_map, self.points.0, new_point.1))
        }
    }

    fn replace_points(
        &self,
        gate_drag_data: super::super::gate_drag::GateDragData,
    ) -> anyhow::Result<Option<Box<dyn super::super::gate_traits::DrawableGate>>> {
        let (x_offset, y_offset) = gate_drag_data.offset();
        let mut new_self = self.clone();
        let mut new_points = self.points;
        if self.axis_matched {
            new_points.1 = self.points.1 - y_offset;
        } else {
            new_points.0 = self.points.0 - x_offset;
        }

        new_self.points = new_points;
        Ok(Some(Box::new(new_self)))
    }

    fn clone_box(&self) -> Box<dyn super::super::gate_traits::DrawableGate> {
        Box::new(self.clone())
    }

    /// Corner ids are positional, not derived from the old ones: an imported
    /// composite's corners carry Omiq's container ids, which say nothing about
    /// which corner they are. The map is built in a fixed geometric order, so
    /// index i is the corner these suffixes name.
    fn with_new_group_id(&self, new_id: Arc<str>) -> Option<Box<dyn DrawableGate>> {
        const SUFFIXES: [&str; 2] = ["_L", "_R"];
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

    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_primary(&self) -> bool {
        true
    }
}

fn create_default_bisector(
    plot_map: &PlotMapper,
    cx_raw: f32,
    x_channel: &str,
    y_channel: &str,
) -> anyhow::Result<(GateGeometry, GateGeometry)> {
    let cx_data = plot_map.pixel_x_to_data(cx_raw, None);
    create_default_bisector_from_data(cx_data, x_channel, y_channel)
}

fn create_default_bisector_from_data(
    cx_data: f32,
    x_channel: &str,
    y_channel: &str,
) -> anyhow::Result<(GateGeometry, GateGeometry)> {
    let max_left = (cx_data, f32::MAX);
    let min_left = (f32::MIN, f32::MIN);
    let coords = vec![min_left, max_left];
    let g1 = flow_gates::geometry::create_rectangle_geometry(coords, x_channel, y_channel)
        .map_err(|_| anyhow::anyhow!("failed to create rectangle geometry"))?;

    let max_right = (f32::MAX, f32::MAX);
    let min_right = (cx_data, f32::MIN);
    let coords = vec![min_right, max_right];
    let g2 = flow_gates::geometry::create_rectangle_geometry(coords, x_channel, y_channel)
        .map_err(|_| anyhow::anyhow!("failed to create rectangle geometry"))?;

    Ok((g1, g2))
}
