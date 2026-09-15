use flow_fcs::TransformType;

use crate::gate_editor::{
    axis_info::{asinh_reverse_f32, asinh_transform_f32},
    gates::gate_types::{GateRenderShape, ShapeType},
};

pub mod boolean_gates;
pub mod ellipse_gate;
pub mod line_gate;
pub mod polygon_gate;
pub mod rectangle_gate;

pub fn draw_circles_for_selected_gate(
    points: &[(f32, f32)],
    index_offset: usize,
) -> Vec<GateRenderShape> {
    points
        .iter()
        .enumerate()
        .map(|(idx, p)| GateRenderShape::Circle {
            center: *p,
            radius: 3.0,
            fill: "red",
            shape_type: ShapeType::Point(idx + index_offset),
        })
        .collect()
}

pub fn rescale_helper(
    pts: &[(f32, f32)],
    param: &str,
    x_param: &str,
    old: &TransformType,
    new: &TransformType,
) -> anyhow::Result<Vec<(f32, f32)>> {
    let is_x = x_param == param;
    let mut new_pts = pts.to_vec();
    println!("{:?}", new_pts);
    for p in new_pts.iter_mut() {
        let val = if is_x { &mut p.0 } else { &mut p.1 };
        let raw = match old {
            TransformType::Arcsinh { cofactor } => {
                asinh_reverse_f32(*val, *cofactor).unwrap_or(*val)
            }
            _ => *val,
        };
        *val = match new {
            TransformType::Arcsinh { cofactor } => {
                asinh_transform_f32(raw, *cofactor).unwrap_or(raw)
            }
            _ => raw,
        };
    }
    Ok(new_pts)
}

pub fn rescale_helper_point(
    pt: (f32, f32),
    param: &str,
    x_param: &str,
    old: &TransformType,
    new: &TransformType,
) -> anyhow::Result<(f32, f32)> {
    let is_x = x_param == param;

    let mut val = if is_x { pt.0 } else { pt.1 };
    let raw = match old {
        TransformType::Arcsinh { cofactor } => asinh_reverse_f32(val, *cofactor).unwrap_or(val),
        _ => val,
    };
    val = match new {
        TransformType::Arcsinh { cofactor } => asinh_transform_f32(raw, *cofactor).unwrap_or(raw),
        _ => raw,
    };

    let new_point = if is_x { (val, pt.1) } else { (pt.0, val) };
    Ok(new_point)
}

pub fn rescale_helper_single(
    pt: f32,
    old: &TransformType,
    new: &TransformType,
) -> anyhow::Result<f32> {
    let mut val = pt;
    let raw = match old {
        TransformType::Arcsinh { cofactor } => asinh_reverse_f32(val, *cofactor).unwrap_or(val),
        _ => val,
    };
    val = match new {
        TransformType::Arcsinh { cofactor } => asinh_transform_f32(raw, *cofactor).unwrap_or(raw),
        _ => raw,
    };

    Ok(val)
}

#[cfg(test)]
mod gate_single_tests;
