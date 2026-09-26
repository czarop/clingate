use std::sync::Arc;

use anyhow::anyhow;
use flow_fcs::TransformType;
use flow_gates::{GateGeometry, create_rectangle_geometry};

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
pub struct RectangleGate {
    inner: flow_gates::Gate,
    points: Vec<(f32, f32)>,
    is_primary: bool,
}

impl RectangleGate {
    pub fn try_new(gate: flow_gates::Gate, is_primary: bool) -> anyhow::Result<Self> {
        let p;
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
                p = vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)];
            } else {
                return Err(anyhow!(
                    "Invalid geometry for Rectangle Gate: invalid parameters"
                ));
            }
        } else {
            return Err(anyhow!(
                "Invalid geometry for Rectangle Gate: missing coordinates"
            ));
        }
        Ok(Self {
            inner: gate,
            points: p,
            is_primary,
        })
    }

    pub fn clone_rectangle_for_axis_swap(
        &self,
        plot_x: &str,
        plot_y: &str,
    ) -> anyhow::Result<Option<Self>> {
        let (x, y) = (&self.inner.parameters.0, &self.inner.parameters.1);
        if plot_x == x.as_ref() && plot_y == y.as_ref() {
            return Ok(None);
        }
        if plot_x == y.as_ref() && plot_y == x.as_ref() {
            let pts: Vec<_> = self.points.iter().map(|&(x, y)| (y, x)).collect();
            let new_geometry = create_rectangle_geometry(pts, y, x)?;
            let new_parameters = (y.clone(), x.clone());
            let new_gate = flow_gates::Gate {
                id: self.inner.id.clone(),
                parameters: new_parameters,
                geometry: new_geometry,
                label_position: self.inner.label_position.clone(),
                name: self.inner.name.clone(),
                mode: self.inner.mode.clone(),
            };
            return Ok(Some(RectangleGate::try_new(new_gate, self.is_primary)?));
        }
        Err(anyhow!("Axis mismatch for Rectangle Gate"))
    }

    pub fn clone_rectangle_for_rescaled_axis(
        &self,
        param: Arc<str>,
        old: &TransformType,
        new: &TransformType,
    ) -> anyhow::Result<RectangleGate> {
        let points = rescale_helper(
            &self.points.to_vec(),
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
        RectangleGate::try_new(new_gate, self.is_primary)
    }

    pub fn clone_rectangle_for_new_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        anchor: Option<(f32, f32)>,
        _mapper: &PlotMapper,
    ) -> anyhow::Result<Self> {
        let p = &self.points;
        // With an anchor the rectangle is fully determined by two opposite
        // corners, and `create_rectangle_geometry` takes the min and max of
        // whatever it is given - so the pointer may cross the anchor freely and
        // the gate simply inverts about it.
        let new_geometry = match anchor {
            Some(anchor) => create_rectangle_geometry(
                vec![anchor, new_point],
                &self.inner.parameters.0,
                &self.inner.parameters.1,
            )?,
            None => update_rectangle_geometry(
                p.to_vec(),
                new_point,
                point_index,
                &self.inner.parameters.0,
                &self.inner.parameters.1,
            )?,
        };
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        RectangleGate::try_new(new_gate, self.is_primary)
    }
}

impl DrawableGate for RectangleGate {
    fn get_gate_ref(&self, _id: Option<&str>) -> Option<&flow_gates::Gate> {
        Some(&self.inner)
    }
    fn get_inner_gate_ids(&self) -> Vec<Arc<str>> {
        vec![self.inner.id.clone()]
    }
    fn get_name(&self) -> &str {
        &self.inner.name
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

    fn is_point_on_perimeter(
        &self,
        point: (f32, f32),
        tolerance: (f32, f32),
        _mapper: &PlotMapper,
    ) -> Option<f32> {
        is_point_on_rectangle_perimeter(self, point, tolerance)
    }

    fn match_to_plot_axis(
        &self,
        plot_x: &str,
        plot_y: &str,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        match self.clone_rectangle_for_axis_swap(plot_x, plot_y)? {
            Some(l) => Ok(Some(Box::new(l))),
            None => Ok(None),
        }
    }

    /// The corner diagonally opposite the one being dragged. The constructor
    /// winds the corners `[(min_x, min_y), (max_x, min_y), (max_x, max_y),
    /// (min_x, max_y)]`, so the opposite of any corner is two steps round.
    fn drag_anchor(&self, point_index: usize) -> Option<(f32, f32)> {
        self.points
            .get((point_index + 2) % self.points.len())
            .copied()
    }

    fn replace_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        anchor: Option<(f32, f32)>,
        mapper: &PlotMapper,
    ) -> anyhow::Result<Box<dyn DrawableGate>> {
        Ok(Box::new(self.clone_rectangle_for_new_point(
            new_point,
            point_index,
            anchor,
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
            .points
            .iter()
            .map(|(x, y)| (x - x_offset, y - y_offset))
            .collect();
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
        Ok(Some(Box::new(RectangleGate::try_new(
            new_gate,
            self.is_primary,
        )?)))
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
            self.clone_rectangle_for_rescaled_axis(param, old, new)?,
        ))
    }

    fn is_finalised(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn draw_self(
        &self,
        is_selected: bool,
        drag_point: Option<PointDragData>,
        _plot_map: &PlotMapper,
        gate_stats: &Option<GateStats>,
    ) -> Vec<GateRenderShape> {
        let style = if is_selected {
            &SELECTED_LINE
        } else {
            &DEFAULT_LINE
        };
        let pts = &self.points;
        let main = draw_rectangle(
            pts[0],
            pts[2],
            style,
            ShapeType::Gate(self.inner.id.clone()),
        );
        let selected = if is_selected {
            Some(draw_circles_for_selected_gate(pts, 0))
        } else {
            None
        };
        let ghost = drag_point
            .as_ref()
            .and_then(|d| draw_ghost_point_for_rectangle(d, pts));

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
            //         // yrange * 0.02
            //         0f32
            //     }
            // };
            // let offset = (x_offset, y_offset);
            let offset = (0f32, 0f32);
            if let Some(percent) = gate_stats.get_percent_for_id(self.inner.id.clone()) {
                let center = {
                    let bl = self.points[0];
                    let tr = self.points[2];
                    ((bl.0 + tr.0) / 2.0, (bl.1 + tr.1) / 2.0)
                };
                let shape = GateRenderShape::Text {
                    origin: center,
                    offset,
                    fontsize: 10f32,
                    text: format!("{:.2}%", percent),
                    text_anchor: None,
                    shape_type: ShapeType::Text,
                };
                labels.push(shape)
            }
        }

        let labels = if labels.is_empty() {
            None
        } else {
            Some(labels)
        };

        crate::collate_vecs!(main, selected, ghost, labels)
    }

    fn is_primary(&self) -> bool {
        self.is_primary
    }
}

use crate::gate_editor::gates::gate_types::{DRAGGED_LINE, DrawingStyle};

pub fn create_default_rectangle(
    plot_map: &PlotMapper,
    cx_raw: f32,
    cy_raw: f32,
    width_raw: f32,
    height_raw: f32,
    x_channel: &str,
    y_channel: &str,
) -> anyhow::Result<GateGeometry> {
    let half_width = width_raw / 2f32;
    let half_height = height_raw / 2f32;

    let max = plot_map.pixel_to_data(cx_raw + half_width, cy_raw + half_height, None, None)?;
    let min = plot_map.pixel_to_data(cx_raw - half_width, cy_raw - half_height, None, None)?;
    let coords = vec![min, max];

    flow_gates::geometry::create_rectangle_geometry(coords, x_channel, y_channel)
        .map_err(|_| anyhow::anyhow!("failed to create rectangle geometry"))
}

pub fn bounds_to_svg_rect(min: (f32, f32), max: (f32, f32)) -> (f32, f32, f32, f32) {
    let width = (max.0 - min.0).abs();
    let height = (max.1 - min.1).abs();
    let x = min.0;
    let y = max.1;
    (x, y, width, height)
}

pub fn map_rect_to_pixels(
    data_x: f32,
    data_y: f32,
    data_width: f32,
    data_height: f32,
    mapper: &PlotMapper,
) -> (f32, f32, f32, f32) {
    // 1. Identify the two data-space corners
    // (Assuming data_y is the "top" and data_x is the "left")
    let x_min = data_x;
    let x_max = data_x + data_width;
    let y_max = data_y;
    let y_min = data_y - data_height;

    // 2. Map both points to pixel space
    let (p1_x, p1_y) = mapper.data_to_pixel(x_min, y_max, None, None);
    let (p2_x, p2_y) = mapper.data_to_pixel(x_max, y_min, None, None);

    // 3. Calculate SVG attributes from the mapped pixels
    // We use .min() and .abs() because screen Y is inverted
    let rect_x = p1_x.min(p2_x);
    let rect_y = p1_y.min(p2_y);
    let rect_width = (p1_x - p2_x).abs();
    let rect_height = (p1_y - p2_y).abs();

    (rect_x, rect_y, rect_width, rect_height)
}

pub fn draw_rectangle(
    min: (f32, f32),
    max: (f32, f32),
    style: &'static DrawingStyle,
    shape_type: ShapeType,
) -> Vec<GateRenderShape> {
    let (x, y, width, height) = bounds_to_svg_rect(min, max);
    vec![GateRenderShape::Rectangle {
        x,
        y,
        width,
        height,
        style,
        shape_type,
    }]
}

pub fn is_point_on_rectangle_perimeter(
    shape: &RectangleGate,
    point: (f32, f32),
    tolerance: (f32, f32),
) -> Option<f32> {
    let points = &shape.points;
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

pub fn draw_ghost_point_for_rectangle(
    drag_data: &PointDragData,
    main_points: &[(f32, f32)],
) -> Option<Vec<GateRenderShape>> {
    // [bottom-left, bottom-right, top-right, top-left]
    let current = drag_data.loc();

    // The preview must be built from the same anchor the geometry is, or the
    // two disagree the moment a drag crosses it: the gate inverts about the
    // anchor while the ghost, recomputing the anchor from the index it captured
    // on mousedown, would follow whichever corner that index now names.
    //
    // Falling back to the diagonal keeps a preview for a drag that never
    // registered an anchor.
    let anchor = match drag_data.anchor() {
        Some(anchor) => anchor,
        None => *main_points.get((drag_data.point_index() + 2) % main_points.len())?,
    };

    let x = current.0.min(anchor.0);
    let y = current.1.max(anchor.1); // In data space, Top is Max Y
    let width = (current.0 - anchor.0).abs();
    let height = (current.1 - anchor.1).abs();

    let new_rect = GateRenderShape::Rectangle {
        x,
        y,
        width,
        height,
        style: &DRAGGED_LINE,
        shape_type: ShapeType::GhostPoint,
    };

    let point_curr = GateRenderShape::Circle {
        center: current,
        radius: 5.0,
        fill: "yellow",
        shape_type: ShapeType::GhostPoint,
    };

    Some(vec![new_rect, point_curr])
}

pub fn update_rectangle_geometry(
    mut current_points: Vec<(f32, f32)>,
    new_point: (f32, f32),
    point_index: usize,
    x_param: &str,
    y_param: &str,
) -> anyhow::Result<GateGeometry> {
    let n = current_points.len();

    if point_index >= n {
        return Err(anyhow::anyhow!(
            "invalid point index for rectangle geometry"
        ));
    }

    let idx_before = (point_index + n - 1) % n;
    let idx_after = (point_index + 1) % n;

    let p_prev = current_points[idx_before];
    let p_next = current_points[idx_after];

    let prev;
    let current = new_point;
    let next;

    match point_index {
        0 => {
            //top-left, bottom-left, bottom-right
            prev = (current.0, p_prev.1);
            next = (p_next.0, current.1);
        }
        1 => {
            //bottom-left, bottom-right, top-right
            prev = (p_prev.0, current.1);
            next = (current.0, p_next.1);
        }
        2 => {
            //bottom-right, top-right, top-left
            prev = (current.0, p_prev.1);
            next = (p_next.0, current.1);
        }
        3 => {
            //top-right, top-left, bottom-left
            prev = (p_prev.0, current.1);
            next = (current.0, p_next.1);
        }
        _ => {
            return Err(anyhow::anyhow!(
                "invalid point index for rectangle geometry"
            ));
        }
    }

    current_points[point_index] = new_point;
    current_points[idx_before] = prev;
    current_points[idx_after] = next;

    flow_gates::geometry::create_rectangle_geometry(current_points, x_param, y_param)
        .map_err(|_| anyhow::anyhow!("failed to update rectangle geometry"))
}

// use dioxus::prelude::*;

// #[component]
// pub fn RectangleGateComponent(
//     gate: Arc<RectangleGate>,
//     is_selected: ReadSignal<bool>,
//     drag_point: ReadSignal<Option<PointDragData>>,
//     drag_gate: ReadSignal<Option<GateDragData>>,
//     plot_map: ReadSignal<PlotMapper>,
//     gate_stats: ReadSignal<Option<GateStats>>
// ) -> Element {

//     let style = if is_selected() {
//             &SELECTED_LINE
//         } else {
//             &DEFAULT_LINE
//         };

//     let pts = &gate.points;

//     let main = use_memo(move || {
//         let (min, max) = (gate.points[0], gate.points[2]);
//         let (x, y, width, height) = bounds_to_svg_rect(min, max);
//         let gate_id = gate.get_id();
//         let (mx, my, m_width, m_height) = map_rect_to_pixels(x, y, width, height, &*plot_map.read());
//         rsx! {
//             g { transform,
//                 rect {
//                     x: mx,
//                     y: my,
//                     width: m_width,
//                     height: m_height,
//                     stroke: style.stroke,
//                     stroke_width: style.stroke_width,
//                     stroke_dasharray: if style.dashed { "4" } else { "none" },
//                     fill: style.fill,
//                 }
//             }

//         }
//     });

//     let selected = if is_selected() {
//         Some(draw_circles_for_selected_gate(&pts, 0))
//     } else {
//         None
//     };
//     let ghost = drag_point.read()
//         .as_ref()
//         .and_then(|d| draw_ghost_point_for_rectangle(d, &pts));

//     let mut labels = vec![];

//     if let Some(gate_stats) = gate_stats() {
//         let x_offset = {
//             let axis = plot_map.read().x_axis_min_max();
//             let xrange = *axis.end() - *axis.start();
//             if let Some(label_pos) = &gate.inner.label_position{
//                 xrange * label_pos.offset_x
//             } else {
//                 0f32
//             }
//         };
//         let y_offset = {
//             let axis = plot_map.read().y_axis_min_max();
//             let yrange = *axis.end() - *axis.start();
//             if let Some(label_pos) = &gate.inner.label_position{
//                 yrange * label_pos.offset_y
//             } else {
//                 yrange * 0.02
//             }
//         };
//         let offset = (x_offset, y_offset);
//         match gate_stats.get_percent_for_id(gate.inner.id.clone()){
//             Some(percent) => {
//                 let shape = GateRenderShape::Text { origin: gate.points[3], offset: offset, fontsize: 10f32, text: format!("{:.2}%", percent), text_anchor: None, shape_type: ShapeType::Text };
//                 labels.push(shape)
//         },
//             None => {},
//         }
//     }

//     let labels = Some(labels);

//     rsx!()
// }
