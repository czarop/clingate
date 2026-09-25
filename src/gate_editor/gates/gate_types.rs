use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::gate_editor::gates::gate_store::GateId;

// #[derive(Clone)]
// pub enum GateType{
//     Drawable(Arc<dyn super::gate_traits::DrawableGate>),
//     Boolean(Arc<super::gate_single::boolean_gates::BooleanGate>)
// }

// impl PartialEq for GateType {
//     fn eq(&self, other: &Self) -> bool {
//         match (self, other) {
//             (Self::Drawable(l0), Self::Drawable(r0)) => Arc::ptr_eq(l0, r0),
//             (Self::Boolean(l0), Self::Boolean(r0)) => Arc::ptr_eq(l0, r0),
//             _ => false,
//         }
//     }
// }

// impl GateType {
//     pub fn get_id(&self) -> Arc<str> {
//         match self {
//             GateType::Drawable(drawable_gate) => drawable_gate.get_id(),
//             GateType::Boolean(boolean_gate) => boolean_gate.get_id(),
//         }
//     }
// }

//     pub fn is_composite(&self) -> bool {
//         match self {
//             GateType::Drawable(drawable_gate) => drawable_gate.is_composite(),
//             GateType::DrawableSub(_drawablesub_gate, _parent_id) => true,
//             GateType::Boolean(_boolean_gate) => false,
//         }
//     }

//     pub fn get_subgate_parent_id(&self) -> Option<Arc<str>> {
//         match self {
//             GateType::Drawable(..) => None,
//             GateType::DrawableSub(_drawable_gate, parent_id) => Some(parent_id.clone()),
//             GateType::Boolean(..) => None,
//         }
//     }
// }

#[derive(Clone, PartialEq, Debug)]
pub enum GateText {
    Name(String),
    Percent(String),
    Count(String),
}

#[derive(Clone, PartialEq, Copy, Debug)]
pub enum PrimaryGateType {
    Polygon,
    Ellipse,
    Rectangle,
    Line(Option<f32>),
    Bisector,
    Quadrant,
    SkewedQuadrant,
    Not,
    And,
    Or,
}

impl PrimaryGateType {
    pub fn is_composite(&self) -> bool {
        matches!(
            self,
            PrimaryGateType::Bisector | PrimaryGateType::Quadrant | PrimaryGateType::SkewedQuadrant
        )
    }

    pub fn is_single(&self) -> bool {
        !self.is_composite()
    }
}

#[derive(PartialEq, Clone, Debug)]
pub enum Direction {
    X,
    Y,
    Both,
}

#[derive(PartialEq, Clone, Debug)]
pub enum ShapeType {
    Gate(GateId),
    CompositeGate(GateId, bool),
    Point(usize),
    CompositePoint(usize, bool),
    GhostGate((f32, f32)),
    GhostPoint,
    DraftGate,
    Rotation(f32),
    UndraggableLine,
    UndraggablePoint(usize),
    Text,
    UndraggableText(Direction),
}

#[derive(PartialEq, Clone, Debug)]
pub enum GateRenderShape {
    PolyLine {
        points: Vec<(f32, f32)>,
        style: &'static DrawingStyle,
        shape_type: ShapeType,
    },
    Circle {
        center: (f32, f32),
        radius: f32,
        fill: &'static str,
        shape_type: ShapeType,
    },
    Polygon {
        points: Arc<Vec<(f32, f32)>>,
        style: &'static DrawingStyle,
        shape_type: ShapeType,
    },
    Ellipse {
        center: (f32, f32),
        radius_x: f32,
        radius_y: f32,
        degrees_rotation: f32,
        style: &'static DrawingStyle,
        shape_type: ShapeType,
    },
    Handle {
        center: (f32, f32),
        size: f32,
        shape_center: (f32, f32),
        shape_type: ShapeType,
    },
    Rectangle {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        style: &'static DrawingStyle,
        shape_type: ShapeType,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        style: &'static DrawingStyle,
        shape_type: ShapeType,
    },
    Text {
        origin: (f32, f32),
        offset: (f32, f32),
        fontsize: f32,
        text: String,
        text_anchor: Option<String>,
        shape_type: ShapeType,
    },
}

impl GateRenderShape {
    pub fn clone_with_type(&self, style: &'static DrawingStyle, shape_type: ShapeType) -> Self {
        match self {
            GateRenderShape::PolyLine {
                points,
                style: _,
                shape_type: _,
            } => Self::PolyLine {
                points: points.clone(),
                style,
                shape_type,
            },
            GateRenderShape::Circle {
                center,
                radius,
                fill,
                shape_type: _,
            } => Self::Circle {
                center: *center,
                radius: *radius,
                fill,
                shape_type,
            },
            GateRenderShape::Polygon {
                points,
                style: _,
                shape_type: _,
            } => Self::Polygon {
                points: points.clone(),
                style,
                shape_type,
            },
            GateRenderShape::Ellipse {
                center,
                radius_x,
                radius_y,
                degrees_rotation,
                style: _,
                shape_type: _,
            } => GateRenderShape::Ellipse {
                center: *center,
                radius_x: *radius_x,
                radius_y: *radius_y,
                degrees_rotation: *degrees_rotation,
                style,
                shape_type,
            },
            GateRenderShape::Handle {
                center,
                size,
                shape_center,
                shape_type: _,
            } => Self::Handle {
                center: *center,
                size: *size,
                shape_center: *shape_center,
                shape_type,
            },
            GateRenderShape::Rectangle {
                x,
                y,
                width,
                height,
                style,
                shape_type: _,
            } => Self::Rectangle {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                style,
                shape_type,
            },
            GateRenderShape::Line {
                x1,
                y1,
                x2,
                y2,
                style,
                shape_type: _,
            } => Self::Line {
                x1: *x1,
                y1: *y1,
                x2: *x2,
                y2: *y2,
                style,
                shape_type,
            },
            GateRenderShape::Text { .. } => self.clone(),
        }
    }

    pub fn clone_with_offset(&self, offset: (f32, f32), style: &'static DrawingStyle) -> Self {
        match self {
            GateRenderShape::PolyLine {
                points,
                style: _,
                shape_type,
            } => {
                let p = points
                    .iter()
                    .map(|(x, y)| (x + offset.0, y + offset.1))
                    .collect();
                Self::PolyLine {
                    points: p,
                    style,
                    shape_type: shape_type.clone(),
                }
            }
            GateRenderShape::Circle {
                center,
                radius,
                fill,
                shape_type,
            } => Self::Circle {
                center: (center.0 + offset.0, center.1 + offset.1),
                radius: *radius,
                fill,
                shape_type: shape_type.clone(),
            },
            GateRenderShape::Polygon {
                points,
                style: _,
                shape_type,
            } => {
                let p = points
                    .iter()
                    .map(|(x, y)| (x - offset.0, y - offset.1))
                    .collect();
                Self::Polygon {
                    points: Arc::new(p),
                    style,
                    shape_type: shape_type.clone(),
                }
            }
            GateRenderShape::Ellipse {
                center,
                radius_x,
                radius_y,
                degrees_rotation,
                style,
                shape_type,
            } => {
                let c = (center.0 - offset.0, center.1 - offset.1);

                Self::Ellipse {
                    center: c,
                    radius_x: *radius_x,
                    radius_y: *radius_y,
                    degrees_rotation: *degrees_rotation,
                    style,
                    shape_type: shape_type.clone(),
                }
            }
            GateRenderShape::Handle {
                center,
                size,
                shape_center,
                shape_type,
            } => Self::Handle {
                center: (center.0 + offset.0, center.1 + offset.1),
                shape_center: *shape_center,
                size: *size,
                shape_type: shape_type.clone(),
            },
            GateRenderShape::Rectangle {
                x,
                y,
                width,
                height,
                style,
                shape_type,
            } => {
                let new_x = x + offset.0;
                let new_y = y + offset.1;
                Self::Rectangle {
                    x: new_x,
                    y: new_y,
                    width: *width,
                    height: *height,
                    style,
                    shape_type: shape_type.clone(),
                }
            }
            GateRenderShape::Line {
                x1,
                y1,
                x2,
                y2,
                style,
                shape_type,
            } => Self::Line {
                x1: *x1 + offset.0,
                y1: *y1 + offset.1,
                x2: *x2 + offset.0,
                y2: *y2 + offset.1,
                style,
                shape_type: shape_type.clone(),
            },
            GateRenderShape::Text { .. } => self.clone(),
        }
    }

    pub fn is_composite(&self) -> bool {
        let st = match self {
            GateRenderShape::PolyLine { shape_type, .. }
            | GateRenderShape::Circle { shape_type, .. }
            | GateRenderShape::Polygon { shape_type, .. }
            | GateRenderShape::Ellipse { shape_type, .. }
            | GateRenderShape::Handle { shape_type, .. }
            | GateRenderShape::Rectangle { shape_type, .. }
            | GateRenderShape::Line { shape_type, .. } => shape_type,
            GateRenderShape::Text { .. } => return false,
        };

        matches!(
            st,
            ShapeType::CompositeGate { .. }
                | ShapeType::CompositePoint(..)
                | ShapeType::UndraggableLine
                | ShapeType::UndraggablePoint(..)
        )
    }

    pub fn is_undraggable(&self) -> bool {
        let st = match self {
            GateRenderShape::PolyLine { shape_type, .. }
            | GateRenderShape::Circle { shape_type, .. }
            | GateRenderShape::Polygon { shape_type, .. }
            | GateRenderShape::Ellipse { shape_type, .. }
            | GateRenderShape::Handle { shape_type, .. }
            | GateRenderShape::Rectangle { shape_type, .. }
            | GateRenderShape::Line { shape_type, .. } => shape_type,
            GateRenderShape::Text { .. } => return false,
        };

        matches!(
            st,
            ShapeType::UndraggableLine | ShapeType::UndraggablePoint(..)
        )
    }

    pub fn is_axis_matched(&self) -> bool {
        let st = match self {
            GateRenderShape::PolyLine { shape_type, .. }
            | GateRenderShape::Circle { shape_type, .. }
            | GateRenderShape::Polygon { shape_type, .. }
            | GateRenderShape::Ellipse { shape_type, .. }
            | GateRenderShape::Handle { shape_type, .. }
            | GateRenderShape::Rectangle { shape_type, .. }
            | GateRenderShape::Line { shape_type, .. } => shape_type,
            GateRenderShape::Text { .. } => return true,
        };

        matches!(
            st,
            ShapeType::CompositeGate(.., true) | ShapeType::CompositePoint(.., true)
        )
    }
}

#[derive(PartialEq, Clone, Debug)]
pub struct DrawingStyle {
    pub stroke: &'static str,
    pub fill: &'static str,
    pub stroke_width: f32,
    pub dashed: bool,
}

pub static DRAFT_LINE: DrawingStyle = DrawingStyle {
    stroke: "red",
    fill: "rgba(0, 255, 255, 0.2)",
    stroke_width: 2.0,
    dashed: false,
};

pub static DEFAULT_LINE: DrawingStyle = DrawingStyle {
    stroke: "cyan",
    fill: "rgba(0, 255, 255, 0.2)",
    stroke_width: 2.0,
    dashed: false,
};

pub static SELECTED_LINE: DrawingStyle = DrawingStyle {
    stroke: "orange",
    fill: "rgba(0, 255, 255, 0.2)",
    stroke_width: 2.0,
    dashed: false,
};

pub static DRAGGED_LINE: DrawingStyle = DrawingStyle {
    stroke: "yellow",
    fill: "none",
    stroke_width: 2.0,
    dashed: true,
};

pub static GREY_LINE: DrawingStyle = DrawingStyle {
    stroke: "grey",
    fill: "none",
    stroke_width: 2.0,
    dashed: false,
};

pub static GREY_LINE_DASHED: DrawingStyle = DrawingStyle {
    stroke: "grey",
    fill: "none",
    stroke_width: 2.0,
    dashed: true,
};

#[derive(Clone, Debug, PartialEq)]
pub enum GateStatValue {
    Single(f32),
    Composite(FxHashMap<Arc<str>, f32>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GateStats {
    pub count: GateStatValue,
    pub percent_parent: GateStatValue,
}

impl GateStats {
    pub fn is_composite(&self) -> bool {
        match self.percent_parent {
            GateStatValue::Single(_) => false,
            GateStatValue::Composite(..) => true,
        }
    }

    pub fn get_percent_for_id(&self, id: Arc<str>) -> Option<f32> {
        match &self.percent_parent {
            GateStatValue::Single(val) => Some(*val),
            GateStatValue::Composite(val_map) => val_map.get(&id).copied(),
        }
    }

    pub fn get_count_for_id(&self, id: Arc<str>) -> Option<f32> {
        match &self.count {
            GateStatValue::Single(val) => Some(*val),
            GateStatValue::Composite(val_map) => val_map.get(&id).copied(),
        }
    }
}
