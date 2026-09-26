use dioxus::prelude::*;
use flow_gates::{EventIndex, Gate, GateGeometry};
use polars::prelude::*;

use crate::gate_editor::gates::{GateId, gate_store::GateOverrideResolver};

/// Which events of `df` the gate `gate_id` admits.
///
/// This is the population a gate's children are drawn on and gated from, and
/// the percentage printed on a gate is counted by `flow_gates`'s `EventIndex`
/// (`gate_stats`). The two must agree event for event, so every shape here
/// decides membership exactly as the index does - the same comparisons, the
/// same formula, in the same order - including for an event exactly on a
/// gate's edge:
///
/// - a rectangle holds its edges and corners (`>=` and `<=` on both axes);
/// - a polygon - and so every quadrant, skewed quadrant and bisector piece -
///   is `flow_gates::polygon::point_in_polygon`, called directly; like any
///   ray cast it holds an event on a left or bottom side and not one on a
///   right or top side;
/// - an ellipse holds its boundary (`<= 1`), by the index's own formula.
///
/// The rectangle used to admit only events strictly inside, so an event on its
/// edge was counted in the percentage but missing from the population below
/// it; and the ellipse used an equivalent formula that rounded differently,
/// so an event on or next to its boundary could fall either way (B-CNT-1).
pub fn filter_events_to_mask(
    df: &DataFrame,
    gate_id: GateId,
    resolver: &super::gate_store::GateOverrideResolver,
) -> anyhow::Result<BooleanChunked> {
    // let gate = resolver.resolve(&gate_id)?;
    let gate_drawable = resolver
        .active_gates
        .get(&gate_id)
        .ok_or_else(|| anyhow::anyhow!("error fetching gate from resolver"))?;
    let gate = gate_drawable
        .get_gate_ref(Some(&gate_id))
        .ok_or_else(|| anyhow::anyhow!("error fetching gate from resolver"))?;

    let (x_param, y_param) = gate.parameters.clone();

    match &gate.geometry {
        GateGeometry::Rectangle { min, max } => {
            // Polars native SIMD comparison - Extremely Fast
            let x_series = df.column(&x_param)?.f32()?;
            let y_series = df.column(&y_param)?.f32()?;

            let (minx, miny, maxx, maxy) = {
                (
                    min.get_coordinate(&x_param)
                        .ok_or(anyhow::anyhow!("x_coord not found"))?,
                    min.get_coordinate(&y_param)
                        .ok_or(anyhow::anyhow!("y_coord not found"))?,
                    max.get_coordinate(&x_param)
                        .ok_or(anyhow::anyhow!("x_coord not found"))?,
                    max.get_coordinate(&y_param)
                        .ok_or(anyhow::anyhow!("y_coord not found"))?,
                )
            };

            // Edges and corners are inside, as in `flow_gates`'s
            // `filter_by_rectangle_batch` (`x >= min_x && x <= max_x && ...`).
            // A NaN value is outside either way.
            let mask = x_series.gt_eq(minx)
                & x_series.lt_eq(maxx)
                & y_series.gt_eq(miny)
                & y_series.lt_eq(maxy);

            Ok(mask)
        }
        GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } => {
            // 1. EXTRACTION: Get coordinates from the HashMap once
            let h = center
                .get_coordinate(&x_param)
                .ok_or_else(|| anyhow::anyhow!("Missing X"))?;
            let k = center
                .get_coordinate(&y_param)
                .ok_or_else(|| anyhow::anyhow!("Missing Y"))?;

            // 2. PRE-CALCULATION: Trig and Bounding Box
            let cos_a = angle.cos();
            let sin_a = angle.sin();

            // Calculate Bounding Box (AABB)
            let x_extent = ((radius_x * cos_a).powi(2) + (radius_y * sin_a).powi(2)).sqrt();
            let y_extent = ((radius_x * sin_a).powi(2) + (radius_y * cos_a).powi(2)).sqrt();

            let min_x = h - x_extent;
            let max_x = h + x_extent;
            let min_y = k - y_extent;
            let max_y = k + y_extent;

            // 3. SCAN: 10 Million Rows
            let x_series = df.column(&x_param)?.f32()?;
            let y_series = df.column(&y_param)?.f32()?;
            let xs = x_series.cont_slice()?;
            let ys = y_series.cont_slice()?;

            let mask: BooleanChunked = xs
                .iter()
                .zip(ys.iter())
                .map(|(&px, &py)| {
                    // STEP A: Cheap Bounding Box Pre-Check
                    if px < min_x || px > max_x || py < min_y || py > max_y {
                        return false;
                    }

                    // STEP B: the index's own test (`flow_gates`'s
                    // `filter_by_ellipse_batch`), operation for operation, so an
                    // event on the boundary rounds the same way in both.
                    let dx = px - h;
                    let dy = py - k;
                    let rotated_x = dx * cos_a + dy * sin_a;
                    let rotated_y = -dx * sin_a + dy * cos_a;
                    let normalized_x = rotated_x / radius_x;
                    let normalized_y = rotated_y / radius_y;
                    normalized_x * normalized_x + normalized_y * normalized_y <= 1.0
                })
                .collect();

            Ok(mask.with_name("mask".into()))
        }
        GateGeometry::Polygon { nodes, closed } => {
            // As the index: an open polygon, or one with fewer than three
            // corners, holds nothing.
            if !closed || nodes.len() < 3 {
                return Ok(BooleanChunked::full("mask".into(), false, df.height()));
            }
            let coords: Vec<(f32, f32)> = nodes
                .iter()
                .filter_map(|node| {
                    // let x = x_transform.inverse_transform(&node.get_coordinate(&x_param)?);
                    // let y = y_transform.inverse_transform(&node.get_coordinate(&y_param)?);
                    let x = node.get_coordinate(&x_param)?;
                    let y = node.get_coordinate(&y_param)?;
                    Some((x, y))
                })
                .collect();

            if coords.len() < 3 {
                return Ok(BooleanChunked::full("mask".into(), false, df.height()));
            }

            // 2. Pre-calculate the Bounding Box (AABB) from our flat coords
            let mut min_x = f32::MAX;
            let mut max_x = f32::MIN;
            let mut min_y = f32::MAX;
            let mut max_y = f32::MIN;

            for (x, y) in &coords {
                if *x < min_x {
                    min_x = *x;
                }
                if *x > max_x {
                    max_x = *x;
                }
                if *y < min_y {
                    min_y = *y;
                }
                if *y > max_y {
                    max_y = *y;
                }
            }

            // 3. Get raw data slices
            let x_series = df.column(&x_param)?.f32()?;
            let y_series = df.column(&y_param)?.f32()?;
            let xs = x_series.cont_slice()?;
            let ys = y_series.cont_slice()?;

            // 4. Vectorized Scan
            let mask: BooleanChunked = xs
                .iter()
                .zip(ys.iter())
                .map(|(&px, &py)| {
                    // Fast Bounding Box Reject
                    if px < min_x || px > max_x || py < min_y || py > max_y {
                        return false;
                    }

                    // The index's own test, called rather than copied, so the
                    // two cannot drift apart. The copy that was here measured
                    // each edge from its other end: the same test, and it
                    // agreed in every case tried, but nothing held it to
                    // rounding the same way. The box above rejects only events
                    // this test would reject too.
                    flow_gates::polygon::point_in_polygon(px, py, &coords)
                })
                .collect();

            Ok(mask.with_name("mask".into()))
        }
        GateGeometry::Boolean {
            operation,
            operands,
        } => match operation {
            flow_gates::BooleanOperation::And => {
                // A single operand folds to that operand's own mask. Gates are created
                // with one operand and gain the rest later, so this must not be fatal.
                if operands.is_empty() {
                    return Err(anyhow::anyhow!("AND gates must have at least 1 operand"));
                }
                let mut final_mask: Option<BooleanChunked> = None;
                for gate in operands {
                    let current_mask = filter_events_to_mask(df, gate.clone(), resolver)?;
                    match final_mask {
                        None => final_mask = Some(current_mask),
                        Some(ref mut acc) => {
                            *acc = &*acc & &current_mask;

                            if !acc.any() {
                                break;
                            }
                        }
                    }
                }
                match final_mask {
                    Some(m) => Ok(m),
                    None => Err(anyhow::anyhow!("AND gate operand could not be resolved")),
                }
            }
            flow_gates::BooleanOperation::Or => {
                if operands.is_empty() {
                    return Err(anyhow::anyhow!("OR gates must have at least 1 operand"));
                }
                let mut final_mask: Option<BooleanChunked> = None;
                for gate in operands {
                    let current_mask = filter_events_to_mask(df, gate.clone(), resolver)?;
                    match final_mask {
                        None => final_mask = Some(current_mask),
                        Some(ref mut acc) => *acc = &*acc | &current_mask,
                    }
                }
                match final_mask {
                    Some(m) => Ok(m),
                    None => Err(anyhow::anyhow!("OR gate operand could not be resolved")),
                }
            }
            flow_gates::BooleanOperation::Not => {
                if operands.len() != 1 {
                    return Err(anyhow::anyhow!("Not gates can only have 1 operand"));
                }
                let other_id = operands[0].clone();
                let mask = !filter_events_to_mask(df, other_id, resolver)?;
                Ok(mask)
            }
        },
    }
}

pub fn filter_events_by_hierarchy_to_mask(
    scaled_data: &DataFrame,
    gate_chain: &[GateId],
    resolver: &GateOverrideResolver,
) -> Result<BooleanChunked, anyhow::Error> {
    let event_count = scaled_data.height();
    let mut final_mask = BooleanChunked::full("mask".into(), true, event_count);
    println!("called with gate chain length {}", gate_chain.len());
    for gate_id in gate_chain {
        let gate_mask = filter_events_to_mask(scaled_data, gate_id.clone(), resolver)?;
        final_mask = final_mask & gate_mask;
    }

    Ok(final_mask)
}

// pub fn filter_events_by_gate(
//     x_ca: &Float32Chunked,
//     y_ca: &Float32Chunked,
//     gate: &Gate,
// ) -> Result<Vec<usize>> {
//     // Build index from slices (zero-copy)
//     let index = build_event_index_from_polars(x_ca, y_ca)?;
//     let indices = index.filter_by_gate(gate)?;

//     Ok(indices)
// }

pub fn filter_events_by_gate_with_index(
    gate: &Gate,
    spatial_index: &EventIndex,
) -> Result<Vec<usize>> {
    // Use provided index or build one
    let indices = spatial_index.filter_by_gate(gate)?;

    Ok(indices)
}

pub fn build_event_index_from_polars(
    x_ca: &Float32Chunked,
    y_ca: &Float32Chunked,
) -> anyhow::Result<EventIndex> {
    let x_rechunked = x_ca.rechunk();
    let y_rechunked = y_ca.rechunk();
    let x_slice = x_rechunked
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("Failed to get contiguous slice for X"))?;
    let y_slice = y_rechunked
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("Failed to get contiguous slice for Y"))?;
    EventIndex::build(x_slice, y_slice).map_err(|e| anyhow::anyhow!("{e}"))
}
