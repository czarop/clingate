use std::sync::Arc;

use anyhow::anyhow;
use flow_fcs::{TransformType, Transformable};
use flow_gates::{GateGeometry, create_ellipse_geometry};

use crate::gate_editor::{
    gates::{
        gate_drag::{GateDragData, PointDragData},
        gate_single::draw_circles_for_selected_gate,
        gate_traits::DrawableGate,
        gate_types::{DEFAULT_LINE, GateRenderShape, GateStats, SELECTED_LINE, ShapeType},
    },
    plots::axis_store::PlotMapper,
};

/// The four control points Omiq stores for an ellipse.
///
/// An ellipse as a *locus* has five degrees of freedom, and the canonical
/// centre/radii/angle form captures all five - the import maths is exact. What
/// the canonical form cannot carry is *which* pair of conjugate diameters Omiq
/// chose to put its handles on: the eigen-decomposition normalises any pair onto
/// the principal axes, which can swap the major and minor assignment, flip a
/// direction by 180 degrees, or resolve to an arbitrary angle for a circle.
///
/// Keeping the original four points means an imported gate that was never
/// edited exports byte-identically instead of "the same ellipse, drawn with
/// different handles".
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EllipseHandles {
    pub left: (f32, f32),
    pub top: (f32, f32),
    pub right: (f32, f32),
    pub bottom: (f32, f32),
}

impl EllipseHandles {
    /// Derive handles from the canonical form, for a gate with no stored pair.
    ///
    /// `right` lies along the rotation angle and `top` a quarter turn from it,
    /// with the opposite handles reflected through the centre - so the result is
    /// always a consistent, principal-axis pair.
    pub fn from_canonical(centre: (f32, f32), radius_x: f32, radius_y: f32, angle: f32) -> Self {
        let (sin_a, cos_a) = angle.sin_cos();
        let (cx, cy) = centre;

        let right = (cx + radius_x * cos_a, cy + radius_x * sin_a);
        let top = (cx - radius_y * sin_a, cy + radius_y * cos_a);

        Self {
            right,
            top,
            left: (2.0 * cx - right.0, 2.0 * cy - right.1),
            bottom: (2.0 * cx - top.0, 2.0 * cy - top.1),
        }
    }

    /// Move every handle by the same offset. A translation cannot change which
    /// conjugate pair the handles sit on, so the convention survives a drag.
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        let shift = |p: (f32, f32)| (p.0 + dx, p.1 + dy);
        Self {
            left: shift(self.left),
            top: shift(self.top),
            right: shift(self.right),
            bottom: shift(self.bottom),
        }
    }

    /// The centre implied by the handles: the midpoint of either diameter.
    pub fn centre(&self) -> (f32, f32) {
        (
            (self.left.0 + self.right.0) / 2.0,
            (self.left.1 + self.right.1) / 2.0,
        )
    }
}

#[derive(PartialEq, Clone)]
pub struct EllipseGate {
    pub inner: flow_gates::Gate,
    points: Vec<(f32, f32)>,
    is_primary: bool,
    /// The handles this gate was imported with, if any. `None` once the gate has
    /// been edited in a way that invalidates them, in which case they are
    /// re-derived from the canonical form.
    source_handles: Option<EllipseHandles>,
}

impl EllipseGate {
    /// The four points to write back to Omiq: the imported pair when the gate
    /// still carries one, otherwise a principal-axis pair derived from the
    /// current geometry.
    pub fn omiq_handles(&self) -> anyhow::Result<EllipseHandles> {
        if let Some(handles) = self.source_handles {
            return Ok(handles);
        }

        let GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } = &self.inner.geometry
        else {
            return Err(anyhow!("Invalid geometry for Ellipse Gate"));
        };

        let (Some(cx), Some(cy)) = (
            center.get_coordinate(&self.inner.parameters.0),
            center.get_coordinate(&self.inner.parameters.1),
        ) else {
            return Err(anyhow!("Invalid points for Ellipse Gate"));
        };

        Ok(EllipseHandles::from_canonical(
            (cx, cy),
            *radius_x,
            *radius_y,
            *angle,
        ))
    }

    /// True while the gate still carries the exact handles it was imported with.
    pub fn has_source_handles(&self) -> bool {
        self.source_handles.is_some()
    }

    /// Build a gate that remembers the handles Omiq stored for it.
    pub fn try_new_with_handles(
        gate: flow_gates::Gate,
        is_primary: bool,
        handles: EllipseHandles,
    ) -> anyhow::Result<Self> {
        let mut built = Self::try_new(gate, is_primary)?;
        built.source_handles = Some(handles);
        Ok(built)
    }

    pub fn try_new(gate: flow_gates::Gate, is_primary: bool) -> anyhow::Result<Self> {
        let p = {
            if let GateGeometry::Ellipse {
                center,
                radius_x,
                radius_y,
                angle,
            } = &gate.geometry
            {
                let cx = center.get_coordinate(&gate.parameters.0);
                let cy = center.get_coordinate(&gate.parameters.1);
                if let (Some(cx), Some(cy)) = (cx, cy) {
                    calculate_ellipse_nodes(cx, cy, *radius_x, *radius_y, *angle)
                } else {
                    return Err(anyhow!("Invalid points for Ellipse Gate"));
                }
            } else {
                return Err(anyhow!("Invalid geometry for Ellipse Gate"));
            }
        };
        Ok(Self {
            inner: gate,
            points: p,
            is_primary,
            // Any edit routes back through here, so handles are dropped unless
            // the caller re-attaches them. Only a move can keep them, because
            // only a move leaves the conjugate pair intact.
            source_handles: None,
        })
    }

    fn get_points(&self) -> Vec<(f32, f32)> {
        if let GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } = &self.inner.geometry
        {
            let cx = center.get_coordinate(&self.inner.parameters.0);
            let cy = center.get_coordinate(&self.inner.parameters.1);
            if let (Some(cx), Some(cy)) = (cx, cy) {
                return calculate_ellipse_nodes(cx, cy, *radius_x, *radius_y, *angle);
            }
        }
        vec![]
    }
}

impl DrawableGate for EllipseGate {
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
        if let GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } = &self.inner.geometry
        {
            let cx = center
                .get_coordinate(&self.inner.parameters.0)
                .unwrap_or_default();
            let cy = center
                .get_coordinate(&self.inner.parameters.1)
                .unwrap_or_default();
            return is_point_on_ellipse_perimeter(
                point,
                (cx, cy),
                *radius_x,
                *radius_y,
                *angle,
                tolerance,
            );
        }
        None
    }

    fn match_to_plot_axis(
        &self,
        plot_x: &str,
        plot_y: &str,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        let (x, y) = (&self.inner.parameters.0, &self.inner.parameters.1);
        if plot_x == x.as_ref() && *plot_y == *y.as_ref() {
            return Ok(None);
        }
        if plot_x == y.as_ref() && plot_y == x.as_ref() {
            let p = self.get_points();
            let mirrored = vec![
                (p[0].1, p[0].0),
                (p[3].1, p[3].0),
                (p[4].1, p[4].0),
                (p[1].1, p[1].0),
                (p[2].1, p[2].0),
            ];
            let new_geometry = create_ellipse_geometry(mirrored, y, x)?;
            let new_parameters = (y.clone(), x.clone());
            let new_gate = flow_gates::Gate {
                id: self.inner.id.clone(),
                parameters: new_parameters,
                geometry: new_geometry,
                label_position: self.inner.label_position.clone(),
                name: self.inner.name.clone(),
                mode: self.inner.mode.clone(),
            };
            return Ok(Some(Box::new(EllipseGate::try_new(
                new_gate,
                self.is_primary,
            )?)));
        }

        Err(anyhow!("Axis mismatch for Ellipse Gate"))
    }

    fn replace_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        _anchor: Option<(f32, f32)>,
        mapper: &PlotMapper,
    ) -> anyhow::Result<Box<dyn DrawableGate>> {
        let new_geometry;
        if let GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } = &self.inner.geometry
        {
            new_geometry = update_ellipse_geometry(
                center,
                *radius_x,
                *radius_y,
                *angle,
                new_point,
                point_index,
                &self.inner.parameters.0,
                &self.inner.parameters.1,
                Some(mapper),
            )?;
        } else {
            return Err(anyhow!("Error replacing point in Ellipse"));
        }
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        Ok(Box::new(EllipseGate::try_new(new_gate, self.is_primary)?))
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
            create_ellipse_geometry(points, &self.inner.parameters.0, &self.inner.parameters.1)?;
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        // A translation moves every handle by the same offset, so the imported
        // conjugate pair is still the right one - carry it across rather than
        // re-deriving and silently canonicalising the gate.
        let mut moved = EllipseGate::try_new(new_gate, self.is_primary)?;
        moved.source_handles = self
            .source_handles
            .map(|handles| handles.translated(-x_offset, -y_offset));

        Ok(Some(Box::new(moved)))
    }

    fn rotate_gate(&self, mouse_pos: (f32, f32)) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        let new_geometry;
        if let GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } = &self.inner.geometry
        {
            new_geometry = update_ellipse_geometry(
                center,
                *radius_x,
                *radius_y,
                *angle,
                mouse_pos,
                5,
                &self.inner.parameters.0,
                &self.inner.parameters.1,
                None,
            )?;
        } else {
            return Err(anyhow!("Error rotating Ellipse"));
        }
        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };
        Ok(Some(Box::new(EllipseGate::try_new(
            new_gate,
            self.is_primary,
        )?)))
    }

    fn recalculate_gate_for_rescaled_axis(
        &self,
        param: Arc<str>,
        old: &TransformType,
        new: &TransformType,
        // _data_range: (f32, f32),
        _axis_range: (f32, f32),
    ) -> anyhow::Result<Box<dyn DrawableGate>> {
        let (x_param, y_param) = self.get_params();
        let GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } = &self.inner.geometry
        else {
            return Err(anyhow!(
                "Ellipse gate {} has no ellipse geometry",
                self.get_id()
            ));
        };
        let (Some(cx), Some(cy)) = (
            center.get_coordinate(&x_param),
            center.get_coordinate(&y_param),
        ) else {
            return Err(anyhow!("Ellipse gate {} has no centre", self.get_id()));
        };
        let is_x = x_param == param;

        // An ellipse on one scale is not an ellipse on another, so this keeps
        // three points exactly - the centre and one end of each principal
        // axis - and fits the ellipse they imply on the new scale.
        //
        // The axis ends are carried as points, not their radii as lengths. A
        // radius lies along its own principal axis, which is the rescaled
        // channel only when the ellipse is unrotated; on a marker against a
        // linear scatter channel the long axis is almost always the scatter
        // one, and pushing a scatter-sized length through the marker's
        // transform overflows.
        //
        // Of each axis's two ends, the one on the positive side of the
        // rescaled channel is kept: the canonical form's angle can come back
        // either way round, and this makes the result not depend on which.
        let along = |p: (f32, f32)| if is_x { p.0 } else { p.1 };
        let (sin_a, cos_a) = angle.sin_cos();
        let towards_positive = |v: (f32, f32)| if along(v) < 0.0 { (-v.0, -v.1) } else { v };
        let u = towards_positive((radius_x * cos_a, radius_x * sin_a));
        let v = towards_positive((-radius_y * sin_a, radius_y * cos_a));

        let carry = |p: (f32, f32)| {
            let moved = new.transform(&old.inverse_transform(&along(p)));
            if is_x { (moved, p.1) } else { (p.0, moved) }
        };
        let c_new = carry((cx, cy));
        let u_end = carry((cx + u.0, cy + u.1));
        let v_end = carry((cx + v.0, cy + v.1));
        if [c_new, u_end, v_end]
            .iter()
            .any(|p| !p.0.is_finite() || !p.1.is_finite())
        {
            return Err(anyhow!(
                "Ellipse gate {} cannot be carried to the new scaling of {param}",
                self.get_id()
            ));
        }

        // The carried axis ends are no longer perpendicular unless the gate
        // was unrotated, so they are a conjugate pair of the new ellipse
        // rather than its principal axes - the importer's reconstruction
        // turns such a pair into centre, radii and angle.
        let wide = |p: (f32, f32)| (f64::from(p.0), f64::from(p.1));
        let left = (2.0 * c_new.0 - u_end.0, 2.0 * c_new.1 - u_end.1);
        let new_geometry = crate::omiq::deserialise::create_omiq_ellipse_geometry(
            wide(left),
            wide(u_end),
            wide(v_end),
            &x_param,
            &y_param,
        )?;

        let new_gate = flow_gates::Gate {
            id: self.inner.id.clone(),
            parameters: self.inner.parameters.clone(),
            geometry: new_geometry,
            label_position: self.inner.label_position.clone(),
            name: self.inner.name.clone(),
            mode: self.inner.mode.clone(),
        };

        Ok(Box::new(EllipseGate::try_new(new_gate, self.is_primary)?))
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
        plot_map: &PlotMapper,
        gate_stats: &Option<GateStats>,
    ) -> Vec<GateRenderShape> {
        let style = if is_selected {
            &SELECTED_LINE
        } else {
            &DEFAULT_LINE
        };

        // Get the 5 control points (Center, R, T, L, B)
        let pts = self.get_points();

        if let GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
            ..
        } = &self.inner.geometry
        {
            let cx = center
                .get_coordinate(&self.inner.parameters.0)
                .unwrap_or(0.0);
            let cy = center
                .get_coordinate(&self.inner.parameters.1)
                .unwrap_or(0.0);

            // --- 1. GENERATE ELLIPSE PATH ---
            // We calculate the points in Data Space.
            // The PlotMapper will handle the visual stretching when these are rendered.
            let mut path_points = Vec::with_capacity(65);
            let segments = 64;
            let (sin_a, cos_a) = angle.sin_cos();

            for i in 0..=segments {
                let theta = (i as f32) * 2.0 * std::f32::consts::PI / (segments as f32);
                let (sin_t, cos_t) = theta.sin_cos();

                // Parametric ellipse equation with rotation
                let x_local = radius_x * cos_t;
                let y_local = radius_y * sin_t;

                let x = cx + x_local * cos_a - y_local * sin_a;
                let y = cy + x_local * sin_a + y_local * cos_a;

                path_points.push((x, y));
            }

            let main = Some(vec![GateRenderShape::Polygon {
                points: path_points.into(),
                style,
                shape_type: ShapeType::Gate(self.inner.id.clone()),
            }]);

            let selected = if is_selected {
                let mut c = draw_circles_for_selected_gate(&pts[1..], 1);
                c.push(GateRenderShape::Handle {
                    center: (pts[0].0, pts[0].1 + *radius_y),
                    size: 5.0,
                    shape_center: pts[0],
                    shape_type: ShapeType::Rotation(*angle),
                });
                Some(c)
            } else {
                None
            };
            let ghost = drag_point.as_ref().and_then(|d| {
                draw_ghost_point_for_ellipse(
                    &self.inner.geometry,
                    d,
                    &self.inner.parameters.0,
                    &self.inner.parameters.1,
                )
            });
            let mut labels = vec![];

            if let Some(gate_stats) = gate_stats {
                let x_offset = {
                    let axis = plot_map.x_axis_min_max();
                    let xrange = *axis.end() - *axis.start();
                    if let Some(label_pos) = &self.inner.label_position {
                        xrange * label_pos.offset_x
                    } else {
                        xrange * 0.02
                    }
                };
                let y_offset = {
                    let axis = plot_map.y_axis_min_max();
                    let yrange = *axis.end() - *axis.start();
                    if let Some(label_pos) = &self.inner.label_position {
                        yrange * label_pos.offset_y
                    } else {
                        yrange * 0.00
                    }
                };
                let offset = (x_offset, y_offset);
                if let Some(percent) = gate_stats.get_percent_for_id(self.inner.id.clone()) {
                    let shape = GateRenderShape::Text {
                        origin: self.points[1],
                        offset,
                        fontsize: 10f32,
                        text: format!("{:.2}%", percent),
                        text_anchor: None,
                        shape_type: ShapeType::Text,
                    };
                    labels.push(shape);
                }
            }

            let labels = Some(labels);

            return crate::collate_vecs!(main, selected, ghost, labels);
        }
        vec![]
    }

    fn get_gate_ref(&self, _id: Option<&str>) -> Option<&flow_gates::Gate> {
        Some(&self.inner)
    }
    fn get_inner_gate_ids(&self) -> Vec<Arc<str>> {
        vec![self.inner.id.clone()]
    }

    fn is_primary(&self) -> bool {
        self.is_primary
    }
}

use crate::gate_editor::gates::gate_types::DRAGGED_LINE;
use flow_gates::GateNode;

pub fn is_point_on_ellipse_perimeter(
    point: (f32, f32),
    center: (f32, f32),
    rx: f32,
    ry: f32,
    angle_rad: f32,
    tolerance: (f32, f32),
) -> Option<f32> {
    // 1. Pre-calculate rotation once
    let (sin_a, cos_a) = (-angle_rad).sin_cos();

    // 2. Translate and Rotate point into local ellipse space
    let dx = point.0 - center.0;
    let dy = point.1 - center.1;
    let local_x = dx * cos_a - dy * sin_a;
    let local_y = dx * sin_a + dy * cos_a;

    // 3. Normalized distance check (The Ellipse Equation: (x/rx)^2 + (y/ry)^2 = 1)
    // We use a "fat" perimeter check by comparing the normalized distance to 1.0
    let norm_x = local_x / rx;
    let norm_y = local_y / ry;
    let dist_sq = norm_x * norm_x + norm_y * norm_y;

    // Estimate thickness based on tolerance
    // This is much faster than finding the exact nearest coordinate
    let norm_tol = (tolerance.0 / rx).max(tolerance.1 / ry);

    if (dist_sq.sqrt() - 1.0).abs() <= norm_tol {
        // Only do the expensive math if we are actually near the edge
        let theta = local_y.atan2(local_x);
        let nearest_world_x = center.0 + (rx * theta.cos() * -cos_a - ry * theta.sin() * sin_a);
        let nearest_world_y = center.1 + (rx * theta.cos() * sin_a + ry * theta.sin() * -cos_a);

        let actual_dist = f32::hypot(point.0 - nearest_world_x, point.1 - nearest_world_y);
        Some(actual_dist)
    } else {
        None
    }
}

/// The five handle points of an ellipse, with the minor-axis pair ordered
/// screen-top first.
///
/// Note the minor-axis pair is the other way round from
/// [`calculate_ellipse_nodes_y_up`], which orders it data-top first: index 2
/// here is index 4 there. The two are mirror images in the minor axis, and
/// since that pair is symmetric about the centre both describe the same
/// ellipse - only the labels differ. Nothing depends on the order: the four
/// handles are drawn at all four points either way, and
/// `calculate_projected_radii` takes `abs()` of the projection, so a resize
/// gives the same radius whichever of the pair is grabbed.
///
/// Used for the drawn points. The geometry is built from the `_y_up` form.
pub fn calculate_ellipse_nodes(
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    angle_rad: f32,
) -> Vec<(f32, f32)> {
    let (sin_a, cos_a) = angle_rad.sin_cos();

    vec![
        (cx, cy),                           // 0. Center
        (cx + rx * cos_a, cy + rx * sin_a), // 1. Right (Local X+)
        (cx + ry * sin_a, cy - ry * cos_a), // 2. Local Y-, screen top
        (cx - rx * cos_a, cy - rx * sin_a), // 3. Left (Local X-)
        (cx - ry * sin_a, cy + ry * cos_a), // 4. Local Y+, screen bottom
    ]
}

pub fn draw_ghost_point_for_ellipse(
    curr_geo: &GateGeometry,
    drag_data: &PointDragData,
    x_param: &str,
    y_param: &str,
) -> Option<Vec<GateRenderShape>> {
    let (cursor_x, cursor_y) = drag_data.loc();

    if let GateGeometry::Ellipse {
        center,
        radius_x,
        radius_y,
        angle,
    } = curr_geo
    {
        let cx = center.get_coordinate(x_param).unwrap_or_default();
        let cy = center.get_coordinate(y_param).unwrap_or_default();
        let index = drag_data.point_index();

        let (new_rx, new_ry, new_angle) = calculate_projected_radii(
            (cursor_x, cursor_y),
            (cx, cy),
            *radius_x,
            *radius_y,
            *angle,
            index,
        );

        let (sin_n, cos_n) = new_angle.sin_cos();

        let ghost_circle_pos = match index {
            0 => (cursor_x, cursor_y),
            1 | 3 => {
                let proj = (cursor_x - cx) * cos_n + (cursor_y - cy) * sin_n;
                (cx + proj * cos_n, cy + proj * sin_n)
            }
            2 | 4 => {
                let proj = (cursor_x - cx) * sin_n - (cursor_y - cy) * cos_n;
                (cx + proj * sin_n, cy - proj * cos_n)
            }
            5 => {
                // let dist = new_ry + 20.0;
                // (cx - dist * sin_n, cy + dist * cos_n)
                let handle_distance = new_ry + 20.0; // Distance from center to the handle

                // Position the ghost dot relative to the rotated top of the ellipse
                // (cx - dist * sin, cy + dist * cos)
                let gx = cx - handle_distance * sin_n;
                let gy = cy + handle_distance * cos_n;

                (gx, gy)
            }
            _ => (cursor_x, cursor_y),
        };

        return Some(vec![
            GateRenderShape::Circle {
                center: ghost_circle_pos,
                radius: 5.0,
                fill: "yellow",
                shape_type: ShapeType::GhostPoint,
            },
            GateRenderShape::Ellipse {
                center: (cx, cy),
                radius_x: new_rx,
                radius_y: new_ry,
                degrees_rotation: (-new_angle).to_degrees(), // Standard SVG degrees
                style: &DRAGGED_LINE,
                shape_type: ShapeType::GhostPoint,
            },
        ]);
    }
    None
}

pub fn calculate_projected_radii(
    cursor: (f32, f32),
    center: (f32, f32),
    current_rx: f32,
    current_ry: f32,
    current_angle_rad: f32,
    point_index: usize,
) -> (f32, f32, f32) {
    let dx = cursor.0 - center.0;
    let dy = cursor.1 - center.1;
    let (sin_a, cos_a) = current_angle_rad.sin_cos();

    // Use a very small epsilon to prevent 0.0 radii
    // let eps = 0.1;

    match point_index {
        1 | 3 => {
            // Project onto Major Axis: (cos, sin)
            let rx_raw = dx * cos_a + dy * sin_a;
            let rx = rx_raw.abs();
            (rx, current_ry, current_angle_rad)
        }
        2 | 4 => {
            // Project onto Minor Axis: (-sin, cos)
            // Note: swap the signs here to properly project perpendicular to the major axis
            let ry_raw = dy * cos_a - dx * sin_a;
            let ry = ry_raw.abs();
            (current_rx, ry, current_angle_rad)
        }
        5 => {
            let mouse_angle = dy.atan2(dx);
            let new_angle = mouse_angle - std::f32::consts::FRAC_PI_2;
            (current_rx, current_ry, new_angle)
        }
        _ => (current_rx, current_ry, current_angle_rad),
    }
}

pub fn update_ellipse_geometry(
    center: &GateNode,
    old_rx: f32,
    old_ry: f32,
    old_angle: f32,
    new_point: (f32, f32),
    point_index: usize,
    x_param: &str,
    y_param: &str,
    plot_map: Option<&PlotMapper>,
) -> anyhow::Result<GateGeometry> {
    let cx = center.get_coordinate(x_param).unwrap_or(0.0);
    let cy = center.get_coordinate(y_param).unwrap_or(0.0);

    let (final_cx, final_cy, final_rx, final_ry, final_angle) = if point_index == 0 {
        (new_point.0, new_point.1, old_rx, old_ry, old_angle)
    } else {
        let (rx, ry, angle) =
            calculate_projected_radii(new_point, (cx, cy), old_rx, old_ry, old_angle, point_index);
        (cx, cy, rx, ry, angle)
    };

    // CALL THE HELPER HERE
    let sanitized_points = calculate_ellipse_nodes_y_up(
        final_cx,
        final_cy,
        final_rx,
        final_ry,
        final_angle,
        plot_map,
    );

    Ok(flow_gates::create_ellipse_geometry(
        sanitized_points,
        x_param,
        y_param,
    )?)
}

pub fn calculate_ellipse_nodes_y_up(
    cx: f32,
    cy: f32,
    mut rx: f32, // Make these mutable
    mut ry: f32,
    angle_rad: f32,
    plot_map_op: Option<&PlotMapper>,
) -> Vec<(f32, f32)> {
    // 1. Safety Floor:

    if let Some(plot_map) = plot_map_op {
        let x_range = plot_map.x_axis_min_max();
        let y_range = plot_map.y_axis_min_max();
        let x_span = (*x_range.end() - *x_range.start()).abs();
        let y_span = (*y_range.end() - *y_range.start()).abs();
        let x_min_safe = x_span * 0.000001;
        let y_min_safe = y_span * 0.000001;
        if rx < x_min_safe {
            rx = x_min_safe;
        }
        if ry < y_min_safe {
            ry = y_min_safe;
        }
    }

    let (sin_a, cos_a) = angle_rad.sin_cos();

    // Minor-axis pair ordered data-top first, the opposite way round from
    // `calculate_ellipse_nodes` - see the note there. `create_ellipse_geometry`
    // reads index 1 for the angle and radius_x and index 2 for radius_y, so
    // this is the order the canonical form round-trips through.
    vec![
        (cx, cy),                           // 0. Center
        (cx + rx * cos_a, cy + rx * sin_a), // 1. Local X+, sets the angle
        (cx - ry * sin_a, cy + ry * cos_a), // 2. Local Y+, sets radius_y
        (cx - rx * cos_a, cy - rx * sin_a), // 3. Local X-
        (cx + ry * sin_a, cy - ry * cos_a), // 4. Local Y-
    ]
}

pub fn create_default_ellipse(
    plot_map: &PlotMapper,
    cx_raw: f32,
    cy_raw: f32,
    rx_raw: f32,
    ry_raw: f32,
    x_channel: &str,
    y_channel: &str,
) -> anyhow::Result<GateGeometry> {
    let data_coords = plot_map.pixel_to_data(cx_raw, cy_raw, None, None);
    let (click_x, click_y) = data_coords;

    let edge_x_data = plot_map.pixel_to_data(cx_raw + rx_raw, cy_raw, None, None);
    let edge_y_data = plot_map.pixel_to_data(cx_raw, cy_raw + ry_raw, None, None);
    let rx = (edge_x_data.0 - click_x).abs();
    let ry = (edge_y_data.1 - click_y).abs();
    let coords = vec![
        (click_x, click_y),
        (click_x + rx, click_y),
        (click_x, click_y + ry),
        (click_x - rx, click_y),
        (click_x, click_y - ry),
    ];
    flow_gates::geometry::create_ellipse_geometry(coords, x_channel, y_channel)
        .map_err(|_| anyhow::anyhow!("failed to create ellipse geometry"))
}
