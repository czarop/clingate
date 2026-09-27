use std::sync::Arc;

use crate::gates::gate_types::{DRAFT_LINE, GateRenderShape, ShapeType};

#[derive(PartialEq, Clone)]
pub enum GateDraft {
    Polygon {
        points: Vec<(f32, f32)>,
        x_param: Arc<str>,
        y_param: Arc<str>,
    },
}

impl GateDraft {
    pub fn get_points(&self) -> Vec<(f32, f32)> {
        match self {
            GateDraft::Polygon { points, .. } => points.clone(),
        }
    }
    pub fn is_finalised(&self) -> bool {
        false
    }

    pub fn draw_self(&self) -> Vec<GateRenderShape> {
        match self {
            GateDraft::Polygon { points, .. } => draw_draft_polygon(points),
        }
    }

    pub fn new_polygon(points: Vec<(f32, f32)>, x_param: Arc<str>, y_param: Arc<str>) -> Self {
        GateDraft::Polygon {
            points,
            x_param,
            y_param,
        }
    }
}

fn draw_draft_polygon(points: &[(f32, f32)]) -> Vec<GateRenderShape> {
    match points.len() {
        0 => vec![],
        1 => {
            let center = points[0];
            vec![GateRenderShape::Circle {
                center,
                radius: 3.0,
                fill: "red",
                shape_type: ShapeType::DraftGate,
            }]
        }
        2 => {
            let start = points[0];
            let end = points[1];

            vec![GateRenderShape::PolyLine {
                points: vec![start, end],
                style: &DRAFT_LINE,
                shape_type: ShapeType::DraftGate,
            }]
        }
        _ => {
            let mut points_local: Vec<(f32, f32)> = points.to_vec();
            // close the loop
            if let Some(first) = points_local.first() {
                points_local.push(*first);
            }

            vec![GateRenderShape::Polygon {
                points: Arc::new(points_local),
                style: &DRAFT_LINE,
                shape_type: ShapeType::DraftGate,
            }]
        }
    }
}
