use std::sync::Arc;

use anyhow::anyhow;
use flow_fcs::TransformType;
use flow_gates::{GateGeometry, create_polygon_geometry};

use crate::gate_editor::{
    gates::{
        gate_drag::{GateDragData, PointDragData},
        gate_single::{draw_circles_for_selected_gate, rescale_helper},
        gate_traits::DrawableGate,
        gate_types::{DEFAULT_LINE, GateRenderShape, GateStats, SELECTED_LINE, ShapeType},
    },
    plots::axis_store::PlotMapper,
};

#[derive(PartialEq, Clone)]
pub struct PolygonGate {
    inner: flow_gates::Gate,
    points: Vec<(f32, f32)>,
    is_primary: bool,
}

impl PolygonGate {
    pub fn try_new(gate: flow_gates::Gate, is_primary: bool) -> anyhow::Result<Self> {
        let p;
        if let GateGeometry::Polygon { nodes, .. } = &gate.geometry {
            p = nodes
                .iter()
                .filter_map(|n| {
                    Some((
                        n.get_coordinate(&gate.parameters.0)?,
                        n.get_coordinate(&gate.parameters.1)?,
                    ))
                })
                .collect();
        } else {
            return Err(anyhow!("Invalid geometry for Polygon Gate"));
        }
        Ok(Self {
            inner: gate,
            points: p,
            is_primary,
        })
    }

    pub fn clone_polygon_for_axis_swap(
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
            let new_geometry = create_polygon_geometry(pts, y, x)?;
            let new_parameters = (y.clone(), x.clone());
            let new_gate = flow_gates::Gate {
                id: self.inner.id.clone(),
                parameters: new_parameters,
                geometry: new_geometry,
                label_position: self.inner.label_position.clone(),
                name: self.inner.name.clone(),
                mode: self.inner.mode.clone(),
            };
            return Ok(Some(PolygonGate::try_new(new_gate, self.is_primary)?));
        }
        Err(anyhow!("Axis mismatch for Polygon Gate"))
    }

    pub fn clone_polygon_for_rescaled_axis(
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
            create_polygon_geometry(points, &self.inner.parameters.0, &self.inner.parameters.1)?;
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        PolygonGate::try_new(new_gate, self.is_primary)
    }

    pub fn clone_polygon_for_new_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        _mapper: &PlotMapper,
    ) -> anyhow::Result<Self> {
        let mut p = self.get_points();
        if point_index < p.len() {
            p[point_index] = new_point;
        }
        let new_geometry =
            create_polygon_geometry(p, &self.inner.parameters.0, &self.inner.parameters.1)?;
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        PolygonGate::try_new(new_gate, self.is_primary)
    }

    fn get_points(&self) -> Vec<(f32, f32)> {
        if let GateGeometry::Polygon { nodes, .. } = &self.inner.geometry {
            return nodes
                .iter()
                .filter_map(|n| {
                    Some((
                        n.get_coordinate(&self.inner.parameters.0)?,
                        n.get_coordinate(&self.inner.parameters.1)?,
                    ))
                })
                .collect();
        }
        vec![]
    }

    pub fn get_label_offset(&self) -> (f32, f32) {
        match &self.inner.label_position {
            Some(o) => (o.offset_x, o.offset_y),
            None => (0f32, 0f32),
        }
    }
}

impl DrawableGate for PolygonGate {
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
        is_point_on_polygon_perimeter(self, point, tolerance)
    }

    fn match_to_plot_axis(
        &self,
        plot_x: &str,
        plot_y: &str,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        match self.clone_polygon_for_axis_swap(plot_x, plot_y)? {
            Some(p) => Ok(Some(Box::new(p))),
            None => Ok(None),
        }
    }

    fn replace_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        mapper: &PlotMapper,
    ) -> anyhow::Result<Box<dyn DrawableGate>> {
        Ok(Box::new(self.clone_polygon_for_new_point(
            new_point,
            point_index,
            mapper,
        )?))
    }

    fn replace_points(
        &self,
        gate_drag_data: GateDragData,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        let x_offset = gate_drag_data.offset().0;
        let y_offset = gate_drag_data.offset().1;
        let points = self
            .get_points()
            .into_iter()
            .map(|(x, y)| (x - x_offset, y - y_offset))
            .collect();
        let new_geometry =
            create_polygon_geometry(points, &self.inner.parameters.0, &self.inner.parameters.1)?;
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        Ok(Some(Box::new(PolygonGate::try_new(
            new_gate,
            self.is_primary,
        )?)))
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
        Ok(Box::new(
            self.clone_polygon_for_rescaled_axis(param, old, new)?,
        ))
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
        let style = if is_selected {
            &SELECTED_LINE
        } else {
            &DEFAULT_LINE
        };
        let pts = self.get_points();
        let main = draw_polygon(&pts, style, ShapeType::Gate(self.inner.id.clone()));
        let selected = if is_selected {
            Some(draw_circles_for_selected_gate(&pts, 0))
        } else {
            None
        };
        let ghost = drag_point
            .as_ref()
            .and_then(|d| draw_ghost_point_for_polygon(d, &pts));

        let mut labels = vec![];

        if let Some(gate_stats) = gate_stats {
            // let x_offset = {
            //     let axis = plot_map.x_axis_min_max();
            //     let xrange = *axis.end() - *axis.start();
            //     if let Some(label_pos) = &self.inner.label_position {
            //         xrange * label_pos.offset_x
            //     } else {
            //         0f32
            //     }
            // };
            // let y_offset = {
            //     let axis = plot_map.y_axis_min_max();
            //     let yrange = *axis.end() - *axis.start();
            //     if let Some(label_pos) = &self.inner.label_position {
            //         yrange * label_pos.offset_y
            //     } else {
            //         0f32
            //     }
            // };
            // let offset = (x_offset, y_offset);
            let offset = (0f32, 0f32);
            if let Some(percent) = gate_stats.get_percent_for_id(self.inner.id.clone()) {
                let params = self.get_params();
                let origin = self
                    .inner
                    .geometry
                    .calculate_center(&params.0, &params.1)
                    .expect("should not fail");
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
        crate::collate_vecs!(main, selected, ghost, labels)
    }

    fn is_primary(&self) -> bool {
        self.is_primary
    }
}

use crate::gate_editor::gates::gate_types::{DRAGGED_LINE, DrawingStyle};

pub fn draw_polygon(
    points: &[(f32, f32)],
    style: &'static DrawingStyle,
    shape_type: ShapeType,
) -> Vec<GateRenderShape> {
    vec![GateRenderShape::Polygon {
        points: Arc::new(points.to_vec()),
        style,
        shape_type,
    }]
}

pub fn draw_ghost_point_for_polygon(
    drag_data: &PointDragData,
    main_points: &[(f32, f32)],
) -> Option<Vec<GateRenderShape>> {
    let idx = drag_data.point_index();
    let n = main_points.len();

    let idx_before = (idx + n - 1) % n;
    let idx_after = (idx + 1) % n;
    let p_prev = main_points[idx_before];
    let p_next = main_points[idx_after];

    let prev = (p_prev.0, p_prev.1);
    let current = drag_data.loc();
    let next = (p_next.0, p_next.1);

    let line = GateRenderShape::PolyLine {
        points: vec![prev, current, next],
        style: &DRAGGED_LINE,
        shape_type: ShapeType::GhostPoint,
    };
    let point = GateRenderShape::Circle {
        center: current,
        radius: 5.0,
        fill: "yellow",
        shape_type: ShapeType::GhostPoint,
    };
    Some(vec![line, point])
}

pub fn is_point_on_polygon_perimeter(
    shape: &PolygonGate,
    point: (f32, f32),
    tolerance: (f32, f32),
) -> Option<f32> {
    let points = shape.get_points();
    if points.len() < 2 {
        return None;
    }
    let mut closest = f32::INFINITY;
    for segment in points.windows(2) {
        if let Some(dis) = shape.is_near_segment(point, segment[0], segment[1], tolerance) {
            closest = closest.min(dis);
        }
    }
    // close the loop if required:
    let first = points[0];
    let last = points[points.len() - 1];

    if first != last
        && let Some(dis) = shape.is_near_segment(point, last, first, tolerance)
    {
        closest = closest.min(dis);
    }
    if closest == f32::INFINITY {
        None
    } else {
        Some(closest)
    }
}
