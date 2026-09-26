use std::sync::Arc;

use anyhow::anyhow;
use flow_fcs::TransformType;
use flow_gates::{GateGeometry, create_rectangle_geometry};

use crate::gate_editor::{
    gates::{
        gate_drag::{GateDragData, PointDragData},
        gate_single::rescale_helper,
        gate_traits::DrawableGate,
        gate_types::{DEFAULT_LINE, GateRenderShape, GateStats, SELECTED_LINE, ShapeType},
    },
    plots::axis_store::PlotMapper,
};

#[derive(PartialEq, Clone)]
pub struct LineGate {
    pub inner: flow_gates::Gate,
    pub points: Vec<(f32, f32)>,
    pub height: f32,
    pub axis_matched: bool,
    is_primary: bool,
}

impl LineGate {
    pub fn try_new(gate: flow_gates::Gate, height: f32, is_primary: bool) -> anyhow::Result<Self> {
        let p = {
            if let GateGeometry::Rectangle { min, max } = &gate.geometry {
                let (x1, y1) = (
                    min.get_coordinate(&gate.parameters.0),
                    min.get_coordinate(&gate.parameters.1),
                );
                let (x2, y2) = (
                    max.get_coordinate(&gate.parameters.0),
                    max.get_coordinate(&gate.parameters.1),
                );
                if let (Some(x1), Some(y1), Some(x2), Some(y2)) = (x1, y1, x2, y2) {
                    vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)]
                } else {
                    return Err(anyhow!("Invalid points for Line Gate"));
                }
            } else {
                return Err(anyhow!("Invalid points for Line Gate"));
            }
        };

        Ok(Self {
            inner: gate,
            points: p,
            height,
            axis_matched: true,
            is_primary,
        })
    }

    pub fn clone_line_for_rescaled_axis(
        &self,
        param: Arc<str>,
        old: &TransformType,
        new: &TransformType,
    ) -> anyhow::Result<Self> {
        let points = rescale_helper(
            &self.get_points(),
            &param,
            &self.inner.parameters.0,
            old,
            new,
        )?;

        let new_geometry =
            create_rectangle_geometry(points, &self.inner.parameters.0, &self.inner.parameters.1)?;

        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        let mut line = LineGate::try_new(new_gate, self.height, self.is_primary)?;
        line.axis_matched = self.axis_matched;
        Ok(line)
    }

    pub fn clone_line_for_axis_swap(
        &self,
        plot_x: &str,
        plot_y: &str,
    ) -> anyhow::Result<Option<Self>> {
        let (x, y) = (&self.inner.parameters.0, &self.inner.parameters.1);
        if plot_x == x.as_ref() && *plot_y == *y.as_ref() {
            return Ok(None);
        }

        if plot_x == y.as_ref() && plot_y == x.as_ref() {
            let pts: Vec<_> = self.get_points().into_iter().map(|(x, y)| (y, x)).collect();
            let new_geometry = create_rectangle_geometry(pts, y, x)?;
            let new_parameters = (y.clone(), x.clone());
            let new_axis_matched = !self.axis_matched;
            let new_gate = flow_gates::Gate {
                id: self.inner.id.clone(),
                parameters: new_parameters,
                geometry: new_geometry,
                label_position: self.inner.label_position.clone(),
                name: self.inner.name.clone(),
                mode: self.inner.mode.clone(),
            };
            let mut new_line = LineGate::try_new(new_gate, self.height, self.is_primary)?;
            new_line.axis_matched = new_axis_matched;
            return Ok(Some(new_line));
        }
        Err(anyhow!("Axis mismatch for Line Gate"))
    }

    pub fn clone_line_for_new_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
    ) -> anyhow::Result<Self> {
        self.clone_line_for_new_anchored_point(new_point, point_index, None)
    }

    pub fn clone_line_for_new_anchored_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        anchor: Option<(f32, f32)>,
    ) -> anyhow::Result<Self> {
        let p = self.get_points();
        // Anchored, the line is two edges on the dragged axis - the anchor and
        // the pointer - and the span across the other axis is untouched by the
        // drag, so it is read straight off the current geometry. Feeding both to
        // `create_rectangle_geometry` lets the dragged edge pass the anchor and
        // come out the other side.
        let new_geometry = match (anchor, p.first(), p.get(2)) {
            (Some(anchor), Some(&(min_x, min_y)), Some(&(max_x, max_y))) => {
                let pts = if self.axis_matched {
                    vec![(anchor.0, min_y), (new_point.0, max_y)]
                } else {
                    vec![(min_x, anchor.1), (max_x, new_point.1)]
                };
                create_rectangle_geometry(pts, &self.inner.parameters.0, &self.inner.parameters.1)?
            }
            _ => update_line_geometry(
                p,
                new_point,
                point_index,
                &self.inner.parameters.0,
                &self.inner.parameters.1,
                self.axis_matched,
            )?,
        };
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.get_params(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        let mut new_line = LineGate::try_new(new_gate, self.height, self.is_primary)?;
        new_line.axis_matched = self.axis_matched;
        Ok(new_line)
    }

    fn get_points(&self) -> Vec<(f32, f32)> {
        if let GateGeometry::Rectangle { min, max } = &self.inner.geometry {
            let (x1, y1) = (
                min.get_coordinate(&self.inner.parameters.0),
                min.get_coordinate(&self.inner.parameters.1),
            );
            let (x2, y2) = (
                max.get_coordinate(&self.inner.parameters.0),
                max.get_coordinate(&self.inner.parameters.1),
            );
            if let (Some(x1), Some(y1), Some(x2), Some(y2)) = (x1, y1, x2, y2) {
                return vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)];
            }
        }
        vec![]
    }
}

impl LineGate {
    /// A copy under a different id, for rebuilding a composite whose corners
    /// all need fresh ids.
    pub fn with_id(&self, id: Arc<str>) -> Self {
        let mut copy = self.clone();
        copy.inner.id = id;
        copy
    }
}

impl DrawableGate for LineGate {
    fn get_gate_ref(&self, _id: Option<&str>) -> Option<&flow_gates::Gate> {
        Some(&self.inner)
    }
    fn get_inner_gate_ids(&self) -> Vec<Arc<str>> {
        vec![self.inner.id.clone()]
    }
    fn clone_box(&self) -> Box<dyn DrawableGate> {
        Box::new(self.clone())
    }

    fn with_new_id(&self, new_id: Arc<str>) -> Option<Box<dyn DrawableGate>> {
        let mut copy = self.clone();
        copy.inner.id = new_id;
        Some(Box::new(copy))
    }

    fn get_id(&self) -> Arc<str> {
        self.inner.id.clone()
    }
    fn is_composite(&self) -> bool {
        false
    }
    fn get_params(&self) -> (Arc<str>, Arc<str>) {
        self.inner.parameters.clone()
    }
    fn get_name(&self) -> &str {
        &self.inner.name
    }
    fn is_point_on_perimeter(
        &self,
        point: (f32, f32),
        tolerance: (f32, f32),
        _mapper: &PlotMapper,
    ) -> Option<f32> {
        is_point_on_line(self, point, tolerance, self.axis_matched)
    }

    fn match_to_plot_axis(
        &self,
        plot_x: &str,
        plot_y: &str,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        let line = self.clone_line_for_axis_swap(plot_x, plot_y)?;
        match line {
            Some(l) => Ok(Some(Box::new(l))),
            None => Ok(None),
        }
    }

    /// The edge the drag must hold still. A line gate's point 0 is its `min`
    /// edge on the dragged axis and point 1 its `max`, so the anchor is the
    /// far corner in each case.
    fn drag_anchor(&self, point_index: usize) -> Option<(f32, f32)> {
        let points = self.get_points();
        match point_index {
            0 => points.get(2).copied(),
            1 => points.first().copied(),
            _ => None,
        }
    }

    fn replace_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        anchor: Option<(f32, f32)>,
        _mapper: &PlotMapper,
    ) -> anyhow::Result<Box<dyn DrawableGate>> {
        let line = self.clone_line_for_new_anchored_point(new_point, point_index, anchor)?;
        Ok(Box::new(line))
    }

    fn replace_points(
        &self,
        gate_drag_data: GateDragData,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        let x_offset = gate_drag_data.offset().0;
        let y_offset = gate_drag_data.offset().1;
        let height;
        let points: Vec<(f32, f32)> = match self.axis_matched {
            true => {
                height = gate_drag_data.current_loc().1;
                self.get_points()
                    .into_iter()
                    .map(|(x, y)| (x - x_offset, y))
                    .collect()
            }
            false => {
                height = gate_drag_data.current_loc().0;
                self.get_points()
                    .into_iter()
                    .map(|(x, y)| (x, y - y_offset))
                    .collect()
            }
        };

        if points.len() != 4 {
            return Err(anyhow!("Line gate geometry must have exactly 4 points"));
        }
        let new_geometry =
            create_rectangle_geometry(points, &self.inner.parameters.0, &self.inner.parameters.1)?;
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.get_params(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        let mut new_line = LineGate::try_new(new_gate, height, self.is_primary)?;
        new_line.axis_matched = self.axis_matched;
        Ok(Some(Box::new(new_line)))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn rotate_gate(&self, _mouse_pos: (f32, f32)) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        Ok(None)
    }

    fn recalculate_gate_for_rescaled_axis(
        &self,
        param: Arc<str>,
        old: &TransformType,
        new: &TransformType,
        // _data_range: (f32, f32),
        _axis_range: (f32, f32),
    ) -> anyhow::Result<Box<dyn DrawableGate>> {
        let line = self.clone_line_for_rescaled_axis(param, old, new)?;
        Ok(Box::new(line))
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

        let style = if is_selected {
            &SELECTED_LINE
        } else {
            &DEFAULT_LINE
        };

        let tab_height = {
            if self.axis_matched {
                (max.1 - min.1) * 0.02
            } else {
                (max.0 - min.0) * 0.02
            }
        };

        let pts = self.get_points();
        let main = draw_line(
            pts[0],
            pts[2],
            self.height,
            style,
            ShapeType::Gate(self.inner.id.clone()),
            &drag_point,
            self.axis_matched,
            tab_height,
        );
        let selected = if is_selected {
            let p = draw_circles_for_line(
                pts[0],
                pts[2],
                self.height,
                if self.axis_matched {
                    (min.1, max.1)
                } else {
                    (min.0, max.0)
                },
                &drag_point,
                self.axis_matched,
            );
            Some(p)
        } else {
            None
        };
        let mut labels = vec![];

        if let Some(gate_stats) = gate_stats {
            let x_offset = {
                let axis = plot_map.x_axis_min_max();
                let xrange = *axis.end() - *axis.start();
                if let Some(label_pos) = &self.inner.label_position {
                    xrange * label_pos.offset_x
                } else if self.axis_matched {
                    0f32
                } else {
                    xrange * 0.02
                }
            };
            let y_offset = {
                let axis = plot_map.y_axis_min_max();
                let yrange = *axis.end() - *axis.start();
                if let Some(label_pos) = &self.inner.label_position {
                    yrange * label_pos.offset_y
                } else if self.axis_matched {
                    yrange * 0.02
                } else {
                    0f32
                }
            };
            let offset = (x_offset, y_offset);
            if let Some(percent) = gate_stats.get_percent_for_id(self.inner.id.clone()) {
                let origin = if self.axis_matched {
                    (self.points[0].0, self.height)
                } else {
                    (self.height, self.points[0].1)
                };
                let shape = GateRenderShape::Text {
                    origin,
                    offset,
                    fontsize: 10f32,
                    text: format!("{:.2}%", percent),
                    text_anchor: None,
                    shape_type: ShapeType::Text,
                };
                labels.push(shape)
            }
        }

        let labels = Some(labels);
        crate::collate_vecs!(main, selected, labels)
    }

    fn is_primary(&self) -> bool {
        self.is_primary
    }
}

use crate::gate_editor::gates::gate_types::{DRAGGED_LINE, DrawingStyle, GREY_LINE_DASHED};

pub fn create_default_line(
    plot_map: &PlotMapper,
    cx_raw: f32,
    width_raw: f32,
    x_channel: &str,
    y_channel: &str,
) -> anyhow::Result<GateGeometry> {
    let xmax = plot_map.pixel_x_to_data(cx_raw + (width_raw / 2f32), None)?;
    let xmin = plot_map.pixel_x_to_data(cx_raw - (width_raw / 2f32), None)?;
    let max = (xmax, f32::MAX);
    let min = (xmin, f32::MIN);
    let coords = vec![min, max];
    flow_gates::geometry::create_rectangle_geometry(coords, x_channel, y_channel)
        .map_err(|_| anyhow::anyhow!("failed to create rectangle geometry"))
}

pub fn bounds_to_svg_line(
    min: (f32, f32),
    max: (f32, f32),
    loc: f32,
    axis_matched: bool,
) -> (f32, f32, f32) {
    if axis_matched {
        let width = (max.0 - min.0).abs();
        let x = min.0;
        let y = loc;
        (x, y, width)
    } else {
        let width = (max.1 - min.1).abs();
        let x = loc;
        let y = min.1;
        (x, y, width)
    }
}

pub fn bounds_to_line_coords(
    min: (f32, f32),
    max: (f32, f32),
    loc: f32,
    axis_matched: bool,
) -> ((f32, f32), (f32, f32)) {
    if axis_matched {
        let width = (max.0 - min.0).abs();
        let x1 = min.0;
        let x2 = x1 + width;
        let y = loc;
        ((x1, y), (x2, y))
    } else {
        let width = (max.1 - min.1).abs();
        let x = loc;

        let y1 = min.1;
        let y2 = y1 + width;
        ((x, y1), (x, y2))
    }
}

pub fn draw_line(
    min: (f32, f32),
    max: (f32, f32),
    y_coord: f32,
    style: &'static DrawingStyle,
    shape_type: ShapeType,
    point_drag_data: &Option<PointDragData>,
    axis_matched: bool,
    tab_height: f32,
) -> Vec<GateRenderShape> {
    let coords = bounds_to_svg_line(min, max, y_coord, axis_matched);
    if axis_matched {
        let mut x1 = coords.0;
        let mut x2 = coords.0 + coords.2;
        let y = coords.1;

        if let Some(pdd) = point_drag_data {
            match pdd.point_index() {
                0 => {
                    x1 = pdd.loc().0;
                }
                1 => {
                    x2 = pdd.loc().0;
                }
                _ => unreachable!(),
            }
        }

        vec![
            GateRenderShape::Line {
                x1,
                y1: y,
                x2,
                y2: y,
                style,
                shape_type: shape_type.clone(),
            },
            GateRenderShape::Line {
                x1,
                y1: y - tab_height,
                x2: x1,
                y2: y + tab_height,
                style,
                shape_type: shape_type.clone(),
            },
            GateRenderShape::Line {
                x1: x2,
                y1: y - tab_height,
                x2,
                y2: y + tab_height,
                style,
                shape_type,
            },
        ]
    } else {
        let mut y1 = coords.1;
        let mut y2 = y1 + coords.2;
        let x = coords.0;

        if let Some(pdd) = point_drag_data {
            match pdd.point_index() {
                0 => {
                    y1 = pdd.loc().1;
                }
                1 => {
                    y2 = pdd.loc().1;
                }
                _ => unreachable!(),
            }
        }
        vec![
            GateRenderShape::Line {
                x1: y_coord,
                y1,
                x2: y_coord,
                y2,
                style,
                shape_type: shape_type.clone(),
            },
            GateRenderShape::Line {
                x1: x - tab_height,
                y1,
                x2: x + tab_height,
                y2: y1,
                style,
                shape_type: shape_type.clone(),
            },
            GateRenderShape::Line {
                x1: x - tab_height,
                y1: y2,
                x2: x + tab_height,
                y2,
                style,
                shape_type,
            },
        ]
    }
}

pub fn draw_circles_for_line(
    start: (f32, f32),
    end: (f32, f32),
    loc: f32,
    min_max: (f32, f32),
    point_drag_data: &Option<PointDragData>,
    axis_matched: bool,
) -> Vec<GateRenderShape> {
    let (min, max) = min_max;
    let mut coords = bounds_to_line_coords(start, end, loc, axis_matched);
    let style;
    if let Some(pdd) = point_drag_data {
        style = &DRAGGED_LINE;
        match pdd.point_index() {
            0 => match axis_matched {
                true => coords.0.0 = pdd.loc().0,
                false => coords.0.1 = pdd.loc().1,
            },
            1 => match axis_matched {
                true => coords.1.0 = pdd.loc().0,
                false => coords.1.1 = pdd.loc().1,
            },
            _ => unreachable!(),
        }
    } else {
        style = &GREY_LINE_DASHED;
    }

    let mut x1 = coords.0.0;
    let mut y1 = coords.0.1;
    let mut x2 = coords.1.0;
    let mut y2 = coords.1.1;

    let (l1, l2, c1, c2) = match axis_matched {
        true => {
            y1 = min;
            y2 = max;

            (
                GateRenderShape::Line {
                    x1,
                    y1,
                    x2: x1,
                    y2,
                    style,
                    shape_type: ShapeType::CompositeGate(Arc::from("test"), !axis_matched),
                },
                GateRenderShape::Line {
                    x1: x2,
                    y1,
                    x2,
                    y2,
                    style,
                    shape_type: ShapeType::CompositeGate(Arc::from("test"), !axis_matched),
                },
                GateRenderShape::Circle {
                    center: (coords.0.0, coords.0.1),
                    radius: 3.0,
                    fill: "red",
                    shape_type: ShapeType::Point(0),
                },
                GateRenderShape::Circle {
                    center: (coords.1.0, coords.1.1),
                    radius: 3.0,
                    fill: "red",
                    shape_type: ShapeType::Point(1),
                },
            )
        }
        false => {
            x1 = min;
            x2 = max;

            (
                GateRenderShape::Line {
                    x1,
                    y1,
                    x2,
                    y2: y1,
                    style,
                    shape_type: ShapeType::CompositeGate(Arc::from("test"), !axis_matched),
                },
                GateRenderShape::Line {
                    x1,
                    y1: y2,
                    x2,
                    y2,
                    style,
                    shape_type: ShapeType::CompositeGate(Arc::from("test"), !axis_matched),
                },
                GateRenderShape::Circle {
                    center: (coords.0.0, coords.0.1),
                    radius: 3.0,
                    fill: "red",
                    shape_type: ShapeType::Point(0),
                },
                GateRenderShape::Circle {
                    center: (coords.1.0, coords.1.1),
                    radius: 3.0,
                    fill: "red",
                    shape_type: ShapeType::Point(1),
                },
            )
        }
    };

    vec![l1, l2, c1, c2]
}

pub fn is_point_on_line(
    shape: &LineGate,
    point: (f32, f32),
    tolerance: (f32, f32),
    axis_matched: bool,
) -> Option<f32> {
    let rect_bounds = shape.get_points();
    if rect_bounds.len() != 4 {
        return None;
    }

    let (min, max) = (rect_bounds[0], rect_bounds[2]);

    let line_coords = bounds_to_line_coords(min, max, shape.height, axis_matched);

    if let Some(dis) = shape.is_near_segment(point, line_coords.0, line_coords.1, tolerance) {
        return Some(dis);
    }
    None
}

pub fn update_line_geometry(
    mut current_rect_points: Vec<(f32, f32)>,
    new_point: (f32, f32),
    point_index: usize,
    x_param: &str,
    y_param: &str,
    axis_matched: bool,
) -> anyhow::Result<GateGeometry> {
    let n = current_rect_points.len();
    if point_index >= n {
        return Err(anyhow::anyhow!(
            "invalid point index for rectangle geometry"
        ));
    }
    let current = new_point;
    match axis_matched {
        true => {
            // [bottom-left, bottom-right, top-right, top-left]
            let (idx_before, idx_after) = match point_index {
                0 => {
                    //left
                    (0, 3)
                }
                1 => {
                    //right
                    (1, 2)
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "invalid point index for rectangle geometry"
                    ));
                }
            };

            let p_prev = current_rect_points[idx_before];
            let p_next = current_rect_points[idx_after];

            let prev = (current.0, p_prev.1);
            let next = (current.0, p_next.1);

            current_rect_points[idx_before] = prev;
            current_rect_points[idx_after] = next;
        }
        false => {
            // [bottom-left, bottom-right, top-right, top-left]
            let (idx_before, idx_after) = match point_index {
                0 => {
                    //top - the rectangle is now rotated 90 degrees!
                    (1, 0)
                }
                1 => {
                    //bottom
                    (2, 3)
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "invalid point index for rectangle geometry"
                    ));
                }
            };

            let p_prev = current_rect_points[idx_before];
            let p_next = current_rect_points[idx_after];

            let prev = (p_prev.0, current.1);
            let next = (p_next.0, current.1);

            current_rect_points[idx_before] = prev;
            current_rect_points[idx_after] = next;
        }
    }

    flow_gates::geometry::create_rectangle_geometry(current_rect_points, x_param, y_param)
        .map_err(|_| anyhow::anyhow!("failed to update rectangle geometry"))
}
