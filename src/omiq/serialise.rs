//! Writing an Omiq gating file.
//!
//! The document is built from scratch rather than patched, so everything Omiq
//! needs has to come from the editor: the geometry from the gates themselves,
//! and the document identity - node ids, tree position, group ids, metadata
//! column - from [`crate::omiq::rebuild`].
//!
//! Coordinates are written in each axis's own transformed space, which is what
//! Omiq stores: arcsinh for fluorescence channels, raw for linear ones. Gates
//! are already held that way, so no conversion happens here.

use anyhow::anyhow;
use flow_gates::{GateGeometry, types::LabelPosition};
use std::sync::Arc;

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::gate_composite::skewed_quadrant_gate::get_infinite_bounds;
use crate::gate_editor::gates::gate_single::ellipse_gate::EllipseGate;
use crate::gate_editor::gates::gate_store::GateId;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::omiq::deserialise::{GateSerialized, Point};
use crate::omiq::rebuild::OmiqGateType;
use rustc_hash::FxBuildHasher;

/// Per-channel axis settings, as the axis store holds them.
pub type AxisSettings = im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>;

/// The value Omiq writes for an unbounded gate edge.
///
/// Seen as exactly this in every file inspected so far, on both `RangeGate`
/// bounds and quadrant corners, across separate workflows.
pub const OMIQ_UNBOUNDED: f64 = 1e16;

/// Anything past this came in as Omiq's sentinel and was merely blunted by
/// `f32`, or is `f32::MAX`. Real data tops out around 4.3e6 on a raw scatter
/// axis, so the gap is eight orders of magnitude.
const BLUNTED_SENTINEL: f32 = 1e15;

/// How far a stored coordinate may sit below the axis's own infinite bound and
/// still count as unbounded.
const BOUND_TOLERANCE: f32 = 1e-3;

/// The value this axis uses for an unbounded edge, if it is configured.
///
/// Import does not keep Omiq's sentinel for a composite: it rebuilds each corner
/// against `get_infinite_bounds`, which on an arcsinh axis at cofactor 6000 is
/// about 15.41, not 1e16. Export has to recognise that convention as well as the
/// blunted sentinel, or an unbounded corner would come back as a real
/// coordinate at 15.41 and crop the gate.
fn axis_unbounded(param: &str, axes: &AxisSettings) -> Option<f32> {
    axes.get(param).map(|a| get_infinite_bounds(&a.transform))
}

/// Widen a stored coordinate back to the file's precision, restoring Omiq's
/// sentinel for an unbounded edge.
fn coord(value: f32, unbounded: Option<f32>) -> f64 {
    let is_unbounded = value.abs() >= BLUNTED_SENTINEL
        || unbounded.is_some_and(|b| value.abs() >= b * (1.0 - BOUND_TOLERANCE));

    if is_unbounded {
        if value.is_sign_negative() {
            -OMIQ_UNBOUNDED
        } else {
            OMIQ_UNBOUNDED
        }
    } else {
        value as f64
    }
}

fn point(p: (f32, f32), bounds: (Option<f32>, Option<f32>)) -> Point {
    Point {
        x: coord(p.0, bounds.0),
        y: coord(p.1, bounds.1),
    }
}

fn label(position: &Option<LabelPosition>) -> Option<Point> {
    position.as_ref().map(|l| Point {
        x: l.offset_x as f64,
        y: l.offset_y as f64,
    })
}

/// The coordinates of a geometry's nodes, in the gate's own two parameters.
fn polygon_points(
    geometry: &GateGeometry,
    x_param: &str,
    y_param: &str,
) -> anyhow::Result<Vec<(f32, f32)>> {
    match geometry {
        GateGeometry::Polygon { nodes, .. } => nodes
            .iter()
            .map(|n| {
                let x = n
                    .get_coordinate(x_param)
                    .ok_or_else(|| anyhow!("polygon node missing {x_param}"))?;
                let y = n
                    .get_coordinate(y_param)
                    .ok_or_else(|| anyhow!("polygon node missing {y_param}"))?;
                Ok((x, y))
            })
            .collect(),
        _ => Err(anyhow!("expected a polygon geometry")),
    }
}

fn rectangle_corners(
    geometry: &GateGeometry,
    x_param: &str,
    y_param: &str,
) -> anyhow::Result<((f32, f32), (f32, f32))> {
    match geometry {
        GateGeometry::Rectangle { min, max } => {
            let read = |n: &flow_gates::GateNode, p: &str| {
                n.get_coordinate(p)
                    .ok_or_else(|| anyhow!("rectangle node missing {p}"))
            };
            Ok((
                (read(min, x_param)?, read(min, y_param)?),
                (read(max, x_param)?, read(max, y_param)?),
            ))
        }
        // A quadrant corner is held as a polygon but was a rectangle in the
        // file; its bounding box is exact because the corner is orthogonal.
        GateGeometry::Polygon { .. } => {
            let points = polygon_points(geometry, x_param, y_param)?;
            if points.is_empty() {
                return Err(anyhow!("cannot take the bounds of an empty polygon"));
            }
            let mut min = points[0];
            let mut max = points[0];
            for (x, y) in points {
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
            }
            Ok((min, max))
        }
        _ => Err(anyhow!("expected a rectangle or polygon geometry")),
    }
}

/// Convert one gate back into the filter Omiq stores for it.
///
/// `container_id` selects which gate to write: for a composite it names one
/// subgate, since each of a composite's parts is its own container in the file.
///
/// `source_type` says which Omiq type the gate arrived as, because the held
/// shape is not always the written one - a quadrant's corners are rectangles in
/// the file but polygons once imported, and a skewed quadrant's are angle gates.
/// Without it a round trip would silently change the gate's type.
pub fn gate_to_serialized(
    gate: &Arc<dyn DrawableGate>,
    container_id: &GateId,
    source_type: Option<OmiqGateType>,
    axes: &AxisSettings,
) -> anyhow::Result<GateSerialized> {
    let inner = gate
        .get_gate_ref(Some(container_id))
        .ok_or_else(|| anyhow!("gate {container_id} has no geometry to write"))?;

    let (x_param, y_param) = inner.parameters.clone();
    let label_position = label(&inner.label_position);
    let bounds = (
        axis_unbounded(&x_param, axes),
        axis_unbounded(&y_param, axes),
    );

    // Fall back to whatever the held geometry naturally is, for a gate created
    // in the editor that never came from a file.
    let target = source_type.unwrap_or(match &inner.geometry {
        GateGeometry::Rectangle { .. } => OmiqGateType::Rectangle,
        GateGeometry::Ellipse { .. } => OmiqGateType::Ellipse,
        GateGeometry::Polygon { .. } => OmiqGateType::Polygon,
        GateGeometry::Boolean { .. } => {
            return Err(anyhow!(
                "boolean gate {container_id} is written as a compound container, not a filter"
            ));
        }
    });

    Ok(match target {
        OmiqGateType::Rectangle => {
            let (min, max) = rectangle_corners(&inner.geometry, &x_param, &y_param)?;
            GateSerialized::Rectangle {
                x_param,
                y_param,
                min: point(min, bounds),
                max: point(max, bounds),
                label_position,
            }
        }

        OmiqGateType::Polygon => GateSerialized::Polygon {
            x_param: x_param.clone(),
            y_param: y_param.clone(),
            points: polygon_points(&inner.geometry, &x_param, &y_param)?
                .into_iter()
                .map(|p| point(p, bounds))
                .collect(),
            label_position,
        },

        OmiqGateType::Ellipse => {
            // Prefer the control points the file was imported with: the
            // canonical centre/radii/angle describes the same ellipse but
            // normalises which conjugate pair the handles sit on.
            let handles = gate
                .as_any()
                .downcast_ref::<EllipseGate>()
                .ok_or_else(|| anyhow!("gate {container_id} is not an ellipse"))?
                .omiq_handles()?;

            GateSerialized::Ellipse {
                x_param,
                y_param,
                left: point(handles.left, bounds),
                top: point(handles.top, bounds),
                right: point(handles.right, bounds),
                bottom: point(handles.bottom, bounds),
                label_position,
            }
        }

        OmiqGateType::Range => {
            // A bisector half is held as a rectangle spanning the whole of the
            // other axis; only the split axis is written.
            let (min, max) = rectangle_corners(&inner.geometry, &x_param, &y_param)?;
            GateSerialized::Line {
                x_param,
                y_param,
                f1min: coord(min.0, bounds.0),
                f1max: coord(max.0, bounds.0),
                label_position,
            }
        }

        OmiqGateType::Angle => {
            // A skewed quadrant's corner is held as the polygon
            // [centre, arm, axis corner, arm] - see calculate_skewed_quadrant_polygon.
            let points = polygon_points(&inner.geometry, &x_param, &y_param)?;
            if points.len() < 4 {
                return Err(anyhow!(
                    "skewed quadrant corner {container_id} has {} points, expected 4",
                    points.len()
                ));
            }
            GateSerialized::Angle {
                x_param,
                y_param,
                center: point(points[0], bounds),
                v1: point(points[1], bounds),
                v2: point(points[3], bounds),
                label_position,
            }
        }
    })
}

// ─── Assembling the document ──────────────────────────────────────────────────

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_single::boolean_gates::BooleanGate;
use crate::omiq::deserialise::{
    AtomicContainer, BooleanOpType, CompoundContainer, FilterContainer, GatingNode,
};
use crate::omiq::metadata::MetaDataFileMap;
use crate::omiq::rebuild::OmiqRebuildData;
use std::collections::HashMap;

fn boolean_op(op: flow_gates::BooleanOperation) -> BooleanOpType {
    match op {
        flow_gates::BooleanOperation::And => BooleanOpType::And,
        flow_gates::BooleanOperation::Or => BooleanOpType::Or,
        flow_gates::BooleanOperation::Not => BooleanOpType::Not,
    }
}

/// The name Omiq shows for this container. A composite's parts each carry their
/// own name, so ask the gate for the one belonging to this id.
fn container_name(gate: &Arc<dyn DrawableGate>, container_id: &GateId) -> Arc<str> {
    gate.get_gate_ref(Some(container_id))
        .map(|inner| Arc::from(inner.name.as_str()))
        .unwrap_or_else(|| Arc::from(gate.get_name()))
}

/// Build the filter container for one gate.
fn container_for(
    state: &GateState,
    gate: &Arc<dyn DrawableGate>,
    container_id: &GateId,
    rebuild: Option<&OmiqRebuildData>,
    metadata: &MetaDataFileMap,
    axes: &AxisSettings,
) -> anyhow::Result<FilterContainer> {
    // A boolean has no geometry; it is a compound container naming its operands.
    if let Some(boolean) = gate.as_any().downcast_ref::<BooleanGate>() {
        return Ok(FilterContainer::Compound(CompoundContainer {
            id: container_id.clone(),
            name: Arc::from(boolean.get_name()),
            operation: boolean_op(boolean.get_operation()),
            filter_container_ids: boolean.get_operands().to_vec(),
        }));
    }

    let source_type = rebuild.and_then(|r| r.source_type);
    let default_filter = gate_to_serialized(gate, container_id, source_type, axes)?;

    // Omiq stores one entry per file, even when a metadata column is what
    // actually drives the position - so fan a group override back out over
    // exactly the files the original listed, rather than a set derived from the
    // metadata, which could differ.
    let mut per_file_filters = rustc_hash::FxHashMap::default();
    if let Some(rebuild) = rebuild {
        for file_id in &rebuild.per_file_ids {
            let Some(for_file) = state.gate_for_file(container_id, file_id, metadata) else {
                continue;
            };
            let filter = gate_to_serialized(&for_file, container_id, source_type, axes)?;
            per_file_filters.insert(file_id.clone(), filter);
        }
    }

    Ok(FilterContainer::Atomic(AtomicContainer {
        id: container_id.clone(),
        name: container_name(gate, container_id),
        default_filter,
        group_id: rebuild.and_then(|r| r.group_id.clone()),
        md: rebuild.and_then(|r| r.md.clone()),
        per_file_filters,
    }))
}

/// Which registry ids are containers in the file.
///
/// A composite occupies its own key as well as one per subgate, but only the
/// subgates are containers - the composite itself is a grouping the editor
/// invents, tied together by `groupId`.
fn container_ids(state: &GateState) -> Vec<GateId> {
    state
        .registered_ids()
        .into_iter()
        .filter(|id| {
            let Some(gate) = state.registered_gate(id) else {
                return false;
            };
            !(gate.is_composite() && gate.get_id() == *id)
        })
        .collect()
}

/// Write the whole gating document.
///
/// Built from scratch: the geometry comes from the gates, the document identity
/// from what was captured on import, and anything unreachable is passed through
/// verbatim.
pub fn to_omiq_document(
    state: &GateState,
    metadata: &MetaDataFileMap,
    axes: &AxisSettings,
) -> anyhow::Result<serde_json::Value> {
    let rebuild = state.omiq_rebuild();

    let mut nodes: HashMap<Arc<str>, GatingNode> = HashMap::new();
    let mut containers: HashMap<Arc<str>, FilterContainer> = HashMap::new();

    for container_id in container_ids(state) {
        let Some(gate) = state.registered_gate(&container_id) else {
            continue;
        };
        let entry = rebuild.get(&container_id);

        containers.insert(
            container_id.clone(),
            container_for(state, &gate, &container_id, entry, metadata, axes)?,
        );

        // One node per placement: the same gate can be applied at several
        // points in the tree, and each of those is its own node. A container
        // with no placements is a ghost kept alive by a boolean that references
        // it - it belongs in the file but not in the tree.
        if let Some(entry) = entry {
            for placement in &entry.nodes {
                nodes.insert(
                    placement.node_id.clone(),
                    GatingNode {
                        id: placement.node_id.clone(),
                        parent_id: placement.parent_node_id.clone(),
                        filter_container_id: container_id.clone(),
                        ord: placement.ord,
                        collapsed: placement.collapsed,
                    },
                );
            }
        }
    }

    let mut tree = serde_json::json!({
        "nodes": serde_json::to_value(&nodes)?,
        "filterContainers": serde_json::to_value(&containers)?,
    });

    // Containers no live gate reaches are written back untouched. One that a
    // live boolean references is indistinguishable from these until the
    // reachability walk says otherwise, and dropping it would break that
    // boolean in Omiq.
    if let Some(map) = tree
        .get_mut("filterContainers")
        .and_then(|c| c.as_object_mut())
    {
        for (id, raw) in &rebuild.ghost_containers {
            map.entry(id.to_string()).or_insert_with(|| raw.clone());
        }
    }

    let mut document = serde_json::to_value(&rebuild.header)?;
    let Some(object) = document.as_object_mut() else {
        return Err(anyhow!("document header did not serialise to an object"));
    };
    object.insert("tree".to_string(), tree);

    Ok(document)
}
