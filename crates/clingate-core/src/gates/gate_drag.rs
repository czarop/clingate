use std::sync::Arc;

#[derive(Clone, PartialEq)]
pub struct GateDragData {
    gate_id: Arc<str>,
    start_loc: (f32, f32),
    current_loc: (f32, f32),
}

impl GateDragData {
    pub fn new(gate_id: Arc<str>, start_loc: (f32, f32), current_loc: (f32, f32)) -> Self {
        Self {
            gate_id,
            start_loc,
            current_loc,
        }
    }

    pub fn clone_from_data(new_loc: (f32, f32), old_data: Self) -> Self {
        Self {
            gate_id: old_data.gate_id.clone(),
            start_loc: old_data.current_loc,
            current_loc: new_loc,
        }
    }

    pub fn start_loc(&self) -> (f32, f32) {
        self.start_loc
    }
    pub fn current_loc(&self) -> (f32, f32) {
        self.current_loc
    }
    pub fn offset(&self) -> (f32, f32) {
        let x_offset = self.start_loc.0 - self.current_loc.0;
        let y_offset = self.start_loc.1 - self.current_loc.1;
        (x_offset, y_offset)
    }
    pub fn gate_id(&self) -> Arc<str> {
        self.gate_id.clone()
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct PointDragData {
    point_index: usize,
    loc: (f32, f32),
    anchor: Option<(f32, f32)>,
}

impl PointDragData {
    pub fn new(point_index: usize, loc: (f32, f32)) -> Self {
        Self {
            point_index,
            loc,
            anchor: None,
        }
    }
    pub fn clone_from_data(new_loc: (f32, f32), old_data: Self) -> Self {
        Self {
            point_index: old_data.point_index,
            loc: new_loc,
            anchor: old_data.anchor,
        }
    }

    pub fn point_index(&self) -> usize {
        self.point_index
    }
    pub fn loc(&self) -> (f32, f32) {
        self.loc
    }

    /// The point this drag must hold still, captured once when the drag starts
    /// and carried unchanged until it ends.
    ///
    /// A rectangle's geometry is stored as `min`/`max`, so rebuilding it
    /// renormalises the corners. The moment a drag carries one corner past its
    /// opposite, that renormalisation reassigns the indices and `point_index`
    /// stops naming the corner under the pointer - from there the drag resizes
    /// the wrong corner and the gate collapses into a sliver trailing the
    /// pointer. Rebuilding from a fixed anchor instead of from the neighbouring
    /// corners makes the operation independent of the indices altogether.
    pub fn anchor(&self) -> Option<(f32, f32)> {
        self.anchor
    }

    /// Record the anchor the first time the drag resolves a gate. Later calls
    /// are ignored: the whole point is that it does not move.
    pub fn set_anchor_once(&mut self, anchor: (f32, f32)) {
        self.anchor.get_or_insert(anchor);
    }
}

#[derive(Clone, PartialEq)]
pub struct RotationData {
    gate_id: Arc<str>,
    gate_center_loc: (f32, f32),
    start_loc: (f32, f32),
    current_loc: (f32, f32),
}

impl RotationData {
    pub fn new(
        gate_id: Arc<str>,
        gate_center_loc: (f32, f32),
        start_loc: (f32, f32),
        current_loc: (f32, f32),
    ) -> Self {
        Self {
            gate_id,
            gate_center_loc,
            start_loc,
            current_loc,
        }
    }

    pub fn clone_from_data(new_loc: (f32, f32), old_data: Self) -> Self {
        Self {
            gate_id: old_data.gate_id.clone(),
            gate_center_loc: old_data.gate_center_loc,
            start_loc: old_data.current_loc,
            current_loc: new_loc,
        }
    }
    pub fn gate_id(&self) -> Arc<str> {
        self.gate_id.clone()
    }
    pub fn start_loc(&self) -> (f32, f32) {
        self.start_loc
    }
    pub fn current_loc(&self) -> (f32, f32) {
        self.current_loc
    }
    pub fn pivot_point(&self) -> (f32, f32) {
        self.gate_center_loc
    }

    pub fn rotation_rad(&self) -> f32 {
        let (cx, cy) = self.gate_center_loc;
        let (sx, sy) = self.start_loc;
        let (tx, ty) = self.current_loc;

        // Calculate the angle of the vector from center to the initial click
        let angle_start = (sy - cy).atan2(sx - cx);

        // Calculate the angle of the vector from center to the current mouse position
        let angle_now = (ty - cy).atan2(tx - cx);

        // The change in rotation in radians
        -(angle_now - angle_start)
    }

    pub fn rotation_deg(&self) -> f32 {
        let delta_rad = self.rotation_rad();

        // Convert to degrees for your Ellipse geometry
        delta_rad.to_degrees()
    }
}

#[derive(Clone, PartialEq)]
pub enum GateDragType {
    Point(PointDragData),
    Gate(GateDragData),
    Rotation(RotationData),
}

impl GateDragType {
    pub fn clone_with_point(self, point: (f32, f32)) -> Self {
        match self {
            GateDragType::Point(point_drag_data) => {
                GateDragType::Point(PointDragData::clone_from_data(point, point_drag_data))
            }
            GateDragType::Gate(gate_drag_data) => {
                GateDragType::Gate(GateDragData::clone_from_data(point, gate_drag_data))
            }
            GateDragType::Rotation(rotation_data) => {
                GateDragType::Rotation(RotationData::clone_from_data(point, rotation_data))
            }
        }
    }
}
