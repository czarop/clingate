use flow_fcs::TransformType;
use std::any::Any;
use std::sync::Arc;

use crate::gate_editor::{
    gates::{
        gate_drag::{GateDragData, PointDragData},
        gate_types::{GateRenderShape, GateStats},
    },
    plots::axis_store::PlotMapper,
};

pub trait DrawableGate: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn get_gate_ref(&self, id: Option<&str>) -> Option<&flow_gates::Gate>;
    fn get_name(&self) -> &str;
    fn get_inner_gate_ids(&self) -> Vec<Arc<str>>;
    fn is_primary(&self) -> bool;
    fn is_finalised(&self) -> bool;

    fn draw_self(
        &self,
        is_selected: bool,
        drag_point: Option<PointDragData>,
        plot_map: &PlotMapper,
        gate_stats: &Option<GateStats>,
    ) -> Vec<GateRenderShape>;

    fn is_near_segment(
        &self,
        m: (f32, f32),
        a: (f32, f32),
        b: (f32, f32),
        tolerance: (f32, f32),
    ) -> Option<f32> {
        let (tol_x, tol_y) = tolerance;
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        let length_sq = dx * dx + dy * dy;

        // 1. Find the nearest point on the segment
        let t_clamped = if length_sq == 0.0 {
            0.0
        } else {
            (((m.0 - a.0) * dx + (m.1 - a.1) * dy) / length_sq).clamp(0.0, 1.0)
        };

        let nearest_x = a.0 + t_clamped * dx;
        let nearest_y = a.1 + t_clamped * dy;

        // 2. Check the rectangular tolerance box
        let diff_x = (m.0 - nearest_x).abs();
        let diff_y = (m.1 - nearest_y).abs();

        if diff_x <= tol_x && diff_y <= tol_y {
            // 3. Return the actual Euclidean distance in data space
            let actual_dist = (diff_x.powi(2) + diff_y.powi(2)).sqrt();
            Some(actual_dist)
        } else {
            None
        }
    }

    fn is_composite(&self) -> bool;

    fn get_id(&self) -> Arc<str>;

    fn get_params(&self) -> (Arc<str>, Arc<str>);

    fn is_point_on_perimeter(
        &self,
        point: (f32, f32),
        tolerance: (f32, f32),
        mapper: &PlotMapper,
    ) -> Option<f32>;

    fn match_to_plot_axis(
        &self,
        plot_x_param: &str,
        plot_y_param: &str,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>>;

    fn recalculate_gate_for_rescaled_axis(
        &self,
        param: std::sync::Arc<str>,
        old_transform: &TransformType,
        new_transform: &TransformType,
        // data_range: (f32, f32),
        axis_range: (f32, f32),
    ) -> anyhow::Result<Box<dyn DrawableGate>>;

    fn recalculate_gate_for_new_axis_limits(
        &self,
        _param: std::sync::Arc<str>,
        _lower: f32,
        _upper: f32,
        _transform: &TransformType,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>> {
        Ok(None)
    }

    fn rotate_gate(
        &self,
        mouse_position: (f32, f32),
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>>;

    /// The point a point-drag must hold still, read off this gate when the drag
    /// starts. See [`PointDragData::anchor`].
    ///
    /// `None` by default, which is right for every geometry whose point indices
    /// survive a rebuild: a polygon vertex keeps its place in the ring, an
    /// ellipse handle is derived from the centre, and a composite is positioned
    /// by its centre rather than by a corner. Only the geometries stored as a
    /// normalised `min`/`max` rectangle need one.
    fn drag_anchor(&self, _point_index: usize) -> Option<(f32, f32)> {
        None
    }

    /// Move one point of this gate to `new_point`.
    ///
    /// `anchor` is the point that must not move, where this gate supplied one
    /// from [`drag_anchor`](Self::drag_anchor). When it is `Some`, build the new
    /// geometry from it rather than from `point_index`'s neighbours - that is
    /// what lets a drag carry a corner through its opposite. `None` means either
    /// a geometry that does not need one or a single call outside a drag, and
    /// the index-based path is used.
    fn replace_point(
        &self,
        new_point: (f32, f32),
        point_index: usize,
        anchor: Option<(f32, f32)>,
        plot_map: &PlotMapper,
    ) -> anyhow::Result<Box<dyn DrawableGate>>;

    fn replace_points(
        &self,
        gate_drag_data: GateDragData,
    ) -> anyhow::Result<Option<Box<dyn DrawableGate>>>;

    fn clone_box(&self) -> Box<dyn DrawableGate>;

    /// A copy of this gate under a new id, for unlinking one placement of a
    /// linked gate: the node needs a gate of its own with the same geometry.
    ///
    /// `None` by default, which composites keep. A composite is registered under
    /// its own id *and* each corner's, and Omiq treats it as all-or-nothing, so
    /// a copy would have to mint a fresh id for every corner and rewrite the
    /// group id that ties them together. Linking is refused for composites
    /// rather than half-supported.
    fn with_new_id(&self, _new_id: Arc<str>) -> Option<Box<dyn DrawableGate>> {
        None
    }

    /// A copy of a composite under a new id, with a fresh id for every corner.
    ///
    /// Separate from [`with_new_id`](Self::with_new_id) because a composite is
    /// not one gate: unlinking it has to mint an id for the group *and* one per
    /// corner, since the corners are what the tree and the file hold. `None`
    /// for everything that is not a composite.
    fn with_new_group_id(&self, _new_id: Arc<str>) -> Option<Box<dyn DrawableGate>> {
        None
    }
}

impl Clone for Box<dyn DrawableGate> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}
