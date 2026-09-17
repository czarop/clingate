use anyhow::anyhow;
use flow_fcs::TransformType;
use flow_gates::types::LabelPosition;
use flow_gates::{
    GateError, GateGeometry, GateNode, create_polygon_geometry, create_rectangle_geometry,
};
use itertools::Itertools;
use rustc_hash::{FxBuildHasher, FxHashMap};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::RangeInclusive;
use std::sync::Arc;

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::GateId;
use crate::gate_editor::gates::gate_composite::bisector_gate::BisectorGate;
use crate::gate_editor::gates::gate_composite::quadrant_gate::QuadrantGate;
use crate::gate_editor::gates::gate_composite::skewed_quadrant_gate::{
    DataPoints, SkewedQuadrantGate, get_infinite_bounds,
};
use crate::gate_editor::gates::gate_single::ellipse_gate::{EllipseGate, EllipseHandles};
use crate::gate_editor::gates::gate_single::line_gate::LineGate;
use crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate;
use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_store::{FileId, GateSource};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::{MetaDataFileMap, MetaDataKey, MetaDataParameter};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentJson {
    pub tree: GatingTree,
}

// Gating Tree and Node will be made into the Gating Hierarchy
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GatingTree {
    pub nodes: HashMap<Arc<str>, GatingNode>,
    pub filter_containers: HashMap<Arc<str>, FilterContainer>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GatingNode {
    pub id: Arc<str>,
    pub parent_id: Arc<str>,
    pub filter_container_id: Arc<str>,
    pub ord: u64,
    pub collapsed: bool,
}

//FilterContainer is the actual gate info
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "containerType")]
pub enum FilterContainer {
    #[serde(rename = "AtomicFilterContainer")]
    Atomic(AtomicContainer),

    #[serde(rename = "CompoundFilterContainer")]
    Compound(CompoundContainer),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompoundContainer {
    pub id: GateId,
    pub name: Arc<str>,
    #[serde(rename = "type")]
    pub operation: BooleanOpType,
    pub filter_container_ids: Vec<GateId>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "UPPERCASE")]
pub enum BooleanOpType {
    And,
    Or,
    Not,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AtomicContainer {
    pub id: GateId,
    pub name: Arc<str>,

    /// Omiq's own label for the container - "DEFAULT" for an ordinary gate.
    /// Modelled only so it survives the round trip: it was read and discarded
    /// before, so an exported file was missing it on every container the editor
    /// rebuilt, and Omiq hung rather than say so.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub filter_type: Option<Arc<str>>,

    // The "Global" fallback
    #[serde(rename = "defaultFilter")]
    pub default_filter: GateSerialized,

    // for composite gates - this ties the individual gates together.
    #[serde(rename = "groupId", skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,

    // The metadata parameter controlling the grouping (e.g., "$VOL")
    // Use Option because not all gates are grouped by metadata.
    // this will be SOME if there is a metadata param governing this
    // ie group-specific - each file still gets an entry in file-specific
    // even if its group-specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md: Option<MetaDataParameter>,

    // The "File-Specific" variants
    // JSON: "perFileFilters": { "628415...": { ... } }
    #[serde(default)]
    pub per_file_filters: FxHashMap<FileId, GateSerialized>,
}

impl AtomicContainer {
    pub fn is_composite(&self) -> bool {
        self.group_id.is_some()
    }

    pub fn process_gates_to_drawable(
        &self,
        metadata_file_to_group_map: &MetaDataFileMap,
    ) -> anyhow::Result<Vec<(GateSource, Arc<dyn DrawableGate>)>> {
        let mut collections = Vec::new();
        let gate_id = self.id.clone();

        let is_composite = self.group_id.is_some();

        let rect_to_polygon = {
            if is_composite {
                self.group_id.as_ref().unwrap().contains("QUAD")
            } else {
                false
            }
        };

        // 1. Handle Global (Default)
        let global_gate = self.default_filter.to_drawable(
            gate_id.clone(),
            self.name.clone(),
            flow_gates::GateMode::Global,
            rect_to_polygon,
        )?;
        println!(
            "CREATED GLOBAL GATE! ID: {}, Name: {}",
            global_gate.get_id(),
            global_gate.get_name()
        );
        collections.push((GateSource::Global, global_gate));

        if let Some(ref md_parameter) = self.md {
            // --- GROUP SPECIFIC MODE ---
            // We track unique MetaDataKeys (Param + Group) to avoid duplicate gates
            let mut group_cache: HashMap<MetaDataKey, Arc<dyn DrawableGate>> = HashMap::new();

            for (file_id, gate_spec) in &self.per_file_filters {
                // 1. Look up this file in our new File-First map
                if let Some(file_metadata) = metadata_file_to_group_map.get(file_id) {
                    // 2. Check if this file has the parameter Omiq is asking for
                    if let Some(group_id) = file_metadata.get(md_parameter) {
                        let key = MetaDataKey {
                            parameter: md_parameter.clone(),
                            group: group_id.clone(),
                        };

                        // 3. Only create the DrawableGate if we haven't handled this specific Group yet
                        if let Entry::Vacant(e) = group_cache.entry(key) {
                            let gate = gate_spec.to_drawable(
                                gate_id.clone(),
                                self.name.clone(),
                                flow_gates::GateMode::Global,
                                rect_to_polygon,
                            )?;

                            e.insert(gate);
                        }
                    }
                }
            }

            // 4. Move the unique group gates into the final collection
            for (metadata_key, gate_instance) in group_cache {
                collections.push((
                    GateSource::Group((gate_instance.get_id(), metadata_key)), // Matches your new struct
                    gate_instance,
                ));
            }
        } else {
            // --- FILE SPECIFIC MODE ---
            for (file_id, gate_spec) in &self.per_file_filters {
                let file_gate = gate_spec.to_drawable(
                    gate_id.clone(),
                    self.name.clone(),
                    flow_gates::GateMode::Global,
                    rect_to_polygon,
                )?;
                println!(
                    "CREATED FILE-SPECIFIC GATE! ID: {}, Name: {}, File: {}",
                    file_gate.get_id(),
                    file_gate.get_name(),
                    file_id
                );

                collections.push((
                    GateSource::Sample((gate_id.clone(), file_id.clone())),
                    file_gate,
                ));
            }
        }

        Ok(collections)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum GateSerialized {
    #[serde(rename = "RectangleGate")]
    Rectangle {
        #[serde(rename = "f1")]
        x_param: Arc<str>,
        #[serde(rename = "f2")]
        y_param: Arc<str>,
        min: Point,
        max: Point,
        #[serde(rename = "labelLoc", skip_serializing_if = "Option::is_none")]
        label_position: Option<Point>,
    },
    #[serde(rename = "PolygonGate")]
    Polygon {
        #[serde(rename = "f1")]
        x_param: Arc<str>,
        #[serde(rename = "f2")]
        y_param: Arc<str>,
        #[serde(rename = "vertices")]
        points: Vec<Point>,
        #[serde(rename = "labelLoc", skip_serializing_if = "Option::is_none")]
        label_position: Option<Point>,
    },
    #[serde(rename = "EllipseGate")]
    Ellipse {
        #[serde(rename = "f1")]
        x_param: Arc<str>,
        #[serde(rename = "f2")]
        y_param: Arc<str>,
        left: Point,
        top: Point,
        right: Point,
        bottom: Point,
        #[serde(rename = "labelLoc", skip_serializing_if = "Option::is_none")]
        label_position: Option<Point>,
    },
    #[serde(rename = "RangeGate")]
    Line {
        #[serde(rename = "f1")]
        x_param: Arc<str>,
        #[serde(rename = "f2")]
        y_param: Arc<str>,
        #[serde(rename = "f1Min")]
        f1min: f64,
        #[serde(rename = "f1Max")]
        f1max: f64,
        #[serde(rename = "labelLoc", skip_serializing_if = "Option::is_none")]
        label_position: Option<Point>,
    },
    #[serde(rename = "AngleGate")]
    Angle {
        #[serde(rename = "f1")]
        x_param: Arc<str>,
        #[serde(rename = "f2")]
        y_param: Arc<str>,
        #[serde(rename = "c")]
        center: Point,
        #[serde(rename = "v1")]
        v1: Point,
        #[serde(rename = "v2")]
        v2: Point,
        #[serde(rename = "labelLoc", skip_serializing_if = "Option::is_none")]
        label_position: Option<Point>,
    },
    // Future-proofing for other gate types
    #[serde(other)]
    Unknown,
}

impl GateSerialized {
    /// `None` for an Omiq gate type this build doesn't model (the `Unknown`
    /// catch-all), so an unrecognised gate is skipped rather than taking the
    /// whole import down.
    pub fn get_params(&self) -> Option<(Arc<str>, Arc<str>)> {
        match self {
            GateSerialized::Rectangle {
                x_param, y_param, ..
            } => Some((x_param.clone(), y_param.clone())),
            GateSerialized::Polygon {
                x_param, y_param, ..
            } => Some((x_param.clone(), y_param.clone())),
            GateSerialized::Ellipse {
                x_param, y_param, ..
            } => Some((x_param.clone(), y_param.clone())),
            GateSerialized::Line {
                x_param, y_param, ..
            } => Some((x_param.clone(), y_param.clone())),
            GateSerialized::Angle {
                x_param, y_param, ..
            } => Some((x_param.clone(), y_param.clone())),
            GateSerialized::Unknown => None,
        }
    }
    pub fn to_drawable(
        &self,
        id: Arc<str>,
        name: Arc<str>,
        gate_mode: flow_gates::GateMode,
        rect_to_polygon: bool,
    ) -> anyhow::Result<Arc<dyn DrawableGate>> {
        match self {
            GateSerialized::Rectangle {
                x_param,
                y_param,
                min,
                max,
                label_position,
            } => {
                let raw_coords = vec![
                    (min.x as f32, min.y as f32),
                    (max.x as f32, min.y as f32),
                    (max.x as f32, max.y as f32),
                    (min.x as f32, max.y as f32),
                ];
                let parameters = (x_param.clone(), y_param.clone());
                let geom = {
                    if !rect_to_polygon {
                        create_rectangle_geometry(raw_coords, x_param, y_param)?
                    } else {
                        create_polygon_geometry(raw_coords, x_param, y_param)?
                    }
                };
                let label_position = label_position.map(|p| LabelPosition {
                    offset_x: p.x as f32,
                    offset_y: p.y as f32,
                });
                let gate = flow_gates::Gate {
                    id: id.clone(),
                    name: name.to_string(),
                    geometry: geom,
                    mode: gate_mode,
                    parameters,
                    label_position,
                };

                Ok(Arc::new(RectangleGate::try_new(gate, true)?))
            }

            GateSerialized::Polygon {
                x_param,
                y_param,
                points,
                label_position,
            } => {
                let parameters = (x_param.clone(), y_param.clone());
                let points: Vec<(f32, f32)> = points.iter().map(|&p| p.into()).collect();
                let geom = create_polygon_geometry(points, x_param, y_param)?;
                let label_position = label_position.map(|p| LabelPosition {
                    offset_x: p.x as f32,
                    offset_y: p.y as f32,
                });
                let gate = flow_gates::Gate {
                    id: id.clone(),
                    name: name.to_string(),
                    geometry: geom,
                    mode: gate_mode,
                    parameters,
                    label_position,
                };

                Ok(Arc::new(PolygonGate::try_new(gate, true)?))
            }
            GateSerialized::Ellipse {
                x_param,
                y_param,
                left,
                top,
                right,
                bottom,
                label_position,
            } => {
                let parameters = (x_param.clone(), y_param.clone());
                let geom = create_omiq_ellipse_geometry(
                    (*left).into(),
                    (*right).into(),
                    (*top).into(),
                    x_param,
                    y_param,
                )?;
                // Keep the control points exactly as Omiq wrote them. The
                // canonical centre/radii/angle describes the same locus, but
                // normalises which conjugate pair the handles sit on, so
                // re-deriving them on export would move the handles.
                let handles = EllipseHandles {
                    left: (*left).into(),
                    top: (*top).into(),
                    right: (*right).into(),
                    bottom: (*bottom).into(),
                };
                let label_position = label_position.map(|p| LabelPosition {
                    offset_x: p.x as f32,
                    offset_y: p.y as f32,
                });
                let gate = flow_gates::Gate {
                    id: id.clone(),
                    name: name.to_string(),
                    geometry: geom,
                    mode: gate_mode,
                    parameters,
                    label_position,
                };
                Ok(Arc::new(EllipseGate::try_new_with_handles(
                    gate, true, handles,
                )?))
            }
            GateSerialized::Line {
                x_param,
                y_param,
                f1min,
                f1max,
                label_position,
            } => {
                let parameters = (x_param.clone(), y_param.clone());
                let max = (*f1max as f32, f32::MAX);
                let min = (*f1min as f32, f32::MIN);
                let coords = vec![min, max];
                let geom =
                    flow_gates::geometry::create_rectangle_geometry(coords, x_param, y_param)?;
                let label_position = label_position.map(|p| LabelPosition {
                    offset_x: p.x as f32,
                    offset_y: p.y as f32,
                });
                let gate = flow_gates::Gate {
                    id: id.clone(),
                    name: name.to_string(),
                    geometry: geom,
                    mode: gate_mode,
                    parameters,
                    label_position,
                };
                Ok(Arc::new(LineGate::try_new(gate, 0f32, true)?))
            }
            GateSerialized::Angle { .. } => Err(anyhow::anyhow!(
                "Angle gates are only part of composites and should not be directly deserialized into DrawableGates. They are handled separately in the composite gate logic."
            )),
            GateSerialized::Unknown => Err(anyhow::anyhow!(
                "Unsupported Omiq gate type for gate '{}' ({})",
                name,
                id
            )),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Point {
    #[serde(rename = "f1Val", default)]
    pub x: f64,
    #[serde(rename = "f2Val", default)]
    pub y: f64,
}

impl From<(f32, f32)> for Point {
    fn from(coords: (f32, f32)) -> Self {
        Point {
            x: coords.0 as f64,
            y: coords.1 as f64,
        }
    }
}

impl From<Point> for (f32, f32) {
    fn from(p: Point) -> Self {
        (p.x as f32, p.y as f32)
    }
}

impl From<Point> for (f64, f64) {
    fn from(p: Point) -> Self {
        (p.x, p.y)
    }
}

pub fn create_omiq_ellipse_geometry(
    left: (f64, f64),
    right: (f64, f64),
    top: (f64, f64),
    x_param: &str,
    y_param: &str,
) -> Result<GateGeometry, GateError> {
    // 1. Center is the average of opposite nodes
    // Using left/right is safe even if they are dragged across each other
    let cx = (left.0 + right.0) / 2.0;
    let cy = (left.1 + right.1) / 2.0;

    // 2. Define the Conjugate Vectors
    // These vectors represent the affine skew from the center to the nodes
    let dx1 = right.0 - cx;
    let dy1 = right.1 - cy;

    let dx2 = top.0 - cx;
    let dy2 = top.1 - cy;

    // 3. Matrix Multiplication (MM^T)
    // We construct the covariance matrix of these vectors to find the true axes
    let e = dx1 * dx1 + dx2 * dx2;
    let f = dx1 * dy1 + dx2 * dy2;
    let g = dy1 * dy1 + dy2 * dy2;

    // 4. Eigenvalue Decomposition
    // The eigenvalues of this matrix give us the squared lengths of the major and minor axes
    let trace = e + g;
    let diff = ((e - g).powi(2) + 4.0 * f * f).sqrt();

    let lambda1 = (trace + diff) / 2.0;
    let lambda2 = (trace - diff) / 2.0;

    // We take the square root to get the actual radii
    let radius_x = lambda1.max(0.0).sqrt();
    let radius_y = lambda2.max(0.0).sqrt();

    // 5. Calculate the True Rotation Angle
    // The angle is derived from the eigenvector corresponding to lambda1
    // A circle has lambda1 == lambda2 and no distinguishable axes, so the
    // eigenvector - and any angle read off it - is arbitrary. Pin it to zero
    // rather than letting float noise pick a rotation.
    //
    // `f` is compared against a tolerance scaled to the matrix, not against
    // exact zero: a near-axis-aligned ellipse leaves f at around 1e-18, where
    // atan2 of two near-zero arguments is numerically meaningless.
    let scale = trace.abs().max(1.0);
    let angle = if diff <= f64::EPSILON * scale {
        0.0
    } else if f.abs() <= f64::EPSILON * scale {
        if e >= g {
            0.0
        } else {
            std::f64::consts::PI / 2.0
        }
    } else {
        // Rust's atan2 takes (y, x)
        (lambda1 - e).atan2(f)
    };

    // 6. Final Node Setup for your Library
    let mut center_node = GateNode::new("ellipse_center");
    center_node.set_coordinate(Arc::from(x_param), cx as f32);
    center_node.set_coordinate(Arc::from(y_param), cy as f32);

    Ok(GateGeometry::Ellipse {
        center: center_node,
        radius_x: radius_x as f32,
        radius_y: radius_y as f32,
        angle: angle as f32,
    })
}

// recursive fn to get the params of a boolean gate
pub fn find_atomic_params(
    current_id: &GateId,
    all_containers: &std::collections::HashMap<GateId, FilterContainer>,
) -> Option<(Arc<str>, Arc<str>)> {
    match all_containers.get(current_id)? {
        FilterContainer::Atomic(atomic) => atomic.default_filter.get_params(),
        FilterContainer::Compound(compound) => {
            let first_child_id = compound.filter_container_ids.first()?;
            find_atomic_params(first_child_id, all_containers)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompositeType {
    Bisector(String),
    Quadrant(String),
    SkewedQuadrant(String),
}

pub fn get_composite_gates_from_filter_container(
    composite_type: CompositeType,
    subgates: &[(u32, AtomicContainer)],
    axis_settings: &im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
    metadata_file_to_group_map: &MetaDataFileMap,
) -> anyhow::Result<FxHashMap<(GateId, GateSource), Arc<dyn DrawableGate>>> {
    if matches!(composite_type, CompositeType::Bisector(_)) && subgates.len() != 2 {
        return Err(anyhow::anyhow!(
            "Bisector must have exactly 2 subgates, found {}",
            subgates.len()
        ));
    }
    if matches!(
        composite_type,
        CompositeType::Quadrant(_) | CompositeType::SkewedQuadrant(_)
    ) && subgates.len() != 4
    {
        return Err(anyhow::anyhow!(
            "Quadrant composite gate must have exactly 4 subgates, found {}",
            subgates.len()
        ));
    }
    let mut map = FxHashMap::default();
    let params = if let GateSerialized::Rectangle {
        x_param, y_param, ..
    }
    | GateSerialized::Line {
        x_param, y_param, ..
    }
    | GateSerialized::Angle {
        x_param, y_param, ..
    } = &subgates[0].1.default_filter
    {
        (x_param, y_param)
    } else {
        return Err(anyhow::anyhow!(
            "Unexpected gate type for composite subgate"
        ));
    };

    let (x_axis_range, y_axis_range, x_transform, y_transform) =
        extract_axis_range_from_axis_settings(&params, axis_settings)?;

    let (subgate_ids, subgate_names, gate_id): (_, _, Arc<str>) = match &composite_type {
        CompositeType::Bisector(id)
        | CompositeType::Quadrant(id)
        | CompositeType::SkewedQuadrant(id) => {
            let (ids, names) = get_sorted_subgate_ids_and_names(subgates);
            (ids, names, Arc::from(id.as_str()))
        }
    };

    // make the default gate
    match &composite_type {
        CompositeType::Bisector(composite_group_id) => {
            let default_subgate = subgates[0].1.default_filter.clone(); // we only need one of the subgates - we only need a center point for this
            let default_gate_arc = get_bisector_gate_for_filter_container(
                &default_subgate,
                gate_id.clone(),
                composite_group_id.clone(),
                params,
                &subgate_ids,
                &subgate_names,
            )?;
            map.insert(
                (default_gate_arc.get_id(), GateSource::Global),
                default_gate_arc,
            );
        }
        CompositeType::Quadrant(composite_group_id) => {
            let default_subgate = subgates[1].1.default_filter.clone(); // we only need one of the subgates - we only need a center point for this
            let default_gate_arc = get_quadrant_gate_for_filter_container(
                &default_subgate,
                gate_id.clone(),
                composite_group_id.clone(),
                params,
                &x_axis_range,
                &y_axis_range,
                &subgate_ids,
                &subgate_names,
                &x_transform,
                &y_transform,
            )?;
            map.insert(
                (default_gate_arc.get_id(), GateSource::Global),
                default_gate_arc,
            );
        }
        CompositeType::SkewedQuadrant(composite_group_id) => {
            let default_subgates = subgates
                .iter()
                .map(|(_quadrant_number, fc)| {
                    if let GateSerialized::Angle { .. } = &fc.default_filter {
                        Ok(fc.default_filter.clone())
                    } else {
                        Err(anyhow::anyhow!(
                            "Unexpected gate type for skewed quadrant composite subgate"
                        ))
                    }
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let default_gate_arc = get_skewed_quadrant_gate(
                gate_id.clone(),
                composite_group_id.clone(),
                &default_subgates,
                params,
                // &x_data_range,
                // &y_data_range,
                &x_axis_range,
                &y_axis_range,
                subgate_ids.clone(),
                &subgate_names,
                &x_transform,
                &y_transform,
            )?;
            map.insert(
                (default_gate_arc.get_id(), GateSource::Global),
                default_gate_arc,
            );
        }
    };

    if subgates[0].1.md.is_some() {
        // we have metadata-specific overrides
        let md_param = subgates[0].1.md.as_ref().unwrap().clone();
        // Cache to ensure we only build the geometry once per metadata group
        let mut group_cache: FxHashMap<Arc<str>, Arc<dyn DrawableGate>> = FxHashMap::default();

        match composite_type {
            CompositeType::Bisector(composite_group_id) => {
                // just look at the first subgate - we just need the center point
                for (file_id, gate_spec) in &subgates[0].1.per_file_filters {
                    let file_metadata = metadata_file_to_group_map
                        .get(file_id)
                        .ok_or_else(|| anyhow!("Missing metadata for file {}", file_id))?;
                    let group_id = file_metadata
                        .get(&md_param)
                        .ok_or_else(|| anyhow!("Missing group value for param {}", md_param))?;
                    // Only build the gate if we haven't seen this group yet
                    if !group_cache.contains_key(group_id) {
                        let new_gate = get_bisector_gate_for_filter_container(
                            gate_spec,
                            gate_id.clone(),
                            composite_group_id.clone(),
                            params,
                            &subgate_ids,
                            &subgate_names,
                        )?;
                        group_cache.insert(group_id.clone(), new_gate.clone());
                        // Insert into the final map with the Group key
                        let meta_key = MetaDataKey {
                            parameter: md_param.clone(),
                            group: group_id.clone(),
                        };

                        map.insert(
                            (
                                new_gate.get_id(),
                                GateSource::Group((new_gate.get_id(), meta_key)),
                            ),
                            new_gate,
                        );
                    };
                }
            }
            CompositeType::Quadrant(composite_group_id) => {
                // we only need to look at one of the subgates for quadrants, as we only need the center!
                for (file_id, gate_spec) in &subgates[1].1.per_file_filters {
                    let file_metadata = metadata_file_to_group_map
                        .get(file_id)
                        .ok_or_else(|| anyhow!("Missing metadata for file {}", file_id))?;

                    let group_id = file_metadata
                        .get(&md_param)
                        .ok_or_else(|| anyhow!("Missing group value for param {}", md_param))?;

                    // Only build the gate if we haven't seen this group yet
                    if !group_cache.contains_key(group_id) {
                        let new_gate = get_quadrant_gate_for_filter_container(
                            gate_spec,
                            gate_id.clone(),
                            composite_group_id.clone(),
                            params,
                            &x_axis_range,
                            &y_axis_range,
                            &subgate_ids,
                            &subgate_names,
                            &x_transform,
                            &y_transform,
                        )?;
                        group_cache.insert(group_id.clone(), new_gate.clone());
                        // Insert into the final map with the Group key
                        let meta_key = MetaDataKey {
                            parameter: md_param.clone(),
                            group: group_id.clone(),
                        };

                        map.insert(
                            (
                                new_gate.get_id(),
                                GateSource::Group((new_gate.get_id(), meta_key)),
                            ),
                            new_gate,
                        );
                    };
                }
            }
            CompositeType::SkewedQuadrant(composite_group_id) => {
                // build the initial map collating subgate overrides by file
                let mut file_to_specs: FxHashMap<Arc<str>, Vec<GateSerialized>> =
                    FxHashMap::default();

                for (_quad_idx, fc) in subgates {
                    for (file_id, gate_spec) in &fc.per_file_filters {
                        file_to_specs
                            .entry(file_id.clone())
                            .or_default()
                            .push(gate_spec.clone());
                    }
                }

                for (file_id, specs) in file_to_specs {
                    if specs.len() != 4 {
                        return Err(anyhow::anyhow!(
                            "Skewed quadrant composite gate requires 4 subgates per file, found {} for file {}",
                            specs.len(),
                            file_id
                        ));
                    }

                    let file_metadata = metadata_file_to_group_map
                        .get(&file_id)
                        .ok_or_else(|| anyhow!("Missing metadata for file {}", file_id))?;

                    let group_id = file_metadata
                        .get(&md_param)
                        .ok_or_else(|| anyhow!("Missing group value for param {}", md_param))?;

                    // Only build the gate if we haven't seen this metadata group yet
                    if !group_cache.contains_key(group_id) {
                        let new_gate = get_skewed_quadrant_gate(
                            gate_id.clone(),
                            composite_group_id.clone(),
                            &specs, // Pass the 4 collected Angle gates
                            params,
                            &x_axis_range,
                            &y_axis_range,
                            // &x_data_range,
                            // &y_data_range,
                            subgate_ids.clone(),
                            &subgate_names,
                            &x_transform,
                            &y_transform,
                        )?;

                        group_cache.insert(group_id.clone(), new_gate.clone());

                        let meta_key = MetaDataKey {
                            parameter: md_param.clone(),
                            group: group_id.clone(),
                        };

                        map.insert(
                            (
                                new_gate.get_id(),
                                GateSource::Group((new_gate.get_id(), meta_key)),
                            ),
                            new_gate,
                        );
                    }
                }
            }
        }
    } else if !subgates[0].1.per_file_filters.is_empty() {
        // we have file-specific overrides
        match composite_type {
            CompositeType::Bisector(composite_group_id) => {
                for (file_id, gate_spec) in &subgates[0].1.per_file_filters {
                    let file_gate_arc = get_bisector_gate_for_filter_container(
                        gate_spec,
                        gate_id.clone(),
                        composite_group_id.clone(),
                        params,
                        &subgate_ids,
                        &subgate_names,
                    )?;

                    map.insert(
                        (
                            file_gate_arc.get_id(),
                            GateSource::Sample((file_gate_arc.get_id(), file_id.clone())),
                        ),
                        file_gate_arc,
                    );
                }
            }
            CompositeType::Quadrant(composite_group_id) => {
                for (file_id, gate_spec) in &subgates[1].1.per_file_filters {
                    let file_gate_arc = get_quadrant_gate_for_filter_container(
                        gate_spec,
                        gate_id.clone(),
                        composite_group_id.clone(),
                        params,
                        // &x_data_range,
                        // &y_data_range,
                        &x_axis_range,
                        &y_axis_range,
                        &subgate_ids,
                        &subgate_names,
                        &x_transform,
                        &y_transform,
                    )?;

                    map.insert(
                        (
                            file_gate_arc.get_id(),
                            GateSource::Sample((file_gate_arc.get_id(), file_id.clone())),
                        ),
                        file_gate_arc,
                    );
                }
            }
            CompositeType::SkewedQuadrant(composite_group_id) => {
                let mut file_to_specs: FxHashMap<Arc<str>, Vec<GateSerialized>> =
                    FxHashMap::default();

                for (_quad_idx, fc) in subgates {
                    for (file_id, gate_spec) in &fc.per_file_filters {
                        file_to_specs
                            .entry(file_id.clone())
                            .or_default()
                            .push(gate_spec.clone());
                    }
                }

                for (file_id, specs) in file_to_specs {
                    if specs.len() != 4 {
                        return Err(anyhow::anyhow!(
                            "Skewed quadrant composite gate requires 4 subgates per file, found {} for file {}",
                            specs.len(),
                            file_id
                        ));
                    }

                    let new_gate = get_skewed_quadrant_gate(
                        gate_id.clone(),
                        composite_group_id.clone(),
                        &specs, // Pass the 4 collected Angle gates
                        params,
                        &x_axis_range,
                        &y_axis_range,
                        subgate_ids.clone(),
                        &subgate_names,
                        &x_transform,
                        &y_transform,
                    )?;

                    map.insert(
                        (
                            new_gate.get_id(),
                            GateSource::Sample((new_gate.get_id(), file_id.clone())),
                        ),
                        new_gate,
                    );
                }
            }
        }
    }
    Ok(map)
}

pub fn get_sorted_subgate_ids_and_names(
    subgates: &[(u32, AtomicContainer)],
) -> (Vec<Arc<str>>, Vec<String>) {
    let mut sorted = subgates.to_vec();
    // Sort by position descending to match your mapping (0->3, 1->2, 2->1, 3->0)
    sorted.sort_by(|a, b| b.0.cmp(&a.0));

    let ids: Vec<Arc<str>> = sorted.iter().map(|(_, fc)| fc.id.clone()).collect();
    let names: Vec<String> = sorted
        .into_iter()
        .map(|(_, fc)| fc.name.to_string())
        .collect();

    (ids, names)
}

// pub fn extract_data_range_from_axis_settings(
//     params: &(&Arc<str>, &Arc<str>),
//     axis_settings: &im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
// ) -> anyhow::Result<(RangeInclusive<f32>, RangeInclusive<f32>)> {
//     let x_axis = axis_settings.get(params.0).ok_or_else(|| {
//         anyhow::anyhow!("Could not find axis settings for parameter {}", params.0)
//     })?;
//     let y_axis = axis_settings.get(params.1).ok_or_else(|| {
//         anyhow::anyhow!("Could not find axis settings for parameter {}", params.1)
//     })?;
//     let x_data_range = x_axis.data_lower..=x_axis.data_upper;
//     let y_data_range = y_axis.data_lower..=y_axis.data_upper;
//     Ok((x_data_range, y_data_range))
// }

pub fn extract_axis_range_from_axis_settings(
    params: &(&Arc<str>, &Arc<str>),
    axis_settings: &im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>,
) -> anyhow::Result<(
    RangeInclusive<f32>,
    RangeInclusive<f32>,
    TransformType,
    TransformType,
)> {
    let x_axis = axis_settings.get(params.0).ok_or_else(|| {
        anyhow::anyhow!("Could not find axis settings for parameter {}", params.0)
    })?;
    let y_axis = axis_settings.get(params.1).ok_or_else(|| {
        anyhow::anyhow!("Could not find axis settings for parameter {}", params.1)
    })?;
    let x_axis_range = x_axis.axis_lower..=x_axis.axis_upper;
    let y_axis_range = y_axis.axis_lower..=y_axis.axis_upper;

    let x_trans = x_axis.transform.clone();
    let y_trans = y_axis.transform.clone();

    Ok((x_axis_range, y_axis_range, x_trans, y_trans))
}

pub fn make_data_points_for_quadrant_filter(
    gate: &GateSerialized,
    // x_data_range: &RangeInclusive<f32>,
    // y_data_range: &RangeInclusive<f32>,
    x_axis_range: &RangeInclusive<f32>,
    y_axis_range: &RangeInclusive<f32>,
) -> anyhow::Result<DataPoints> {
    let center: (f32, f32) = if let GateSerialized::Rectangle { min, .. } = gate {
        (*min).into()
    } else {
        return Err(anyhow::anyhow!("Unexpected gate type for quadrant subgate"));
    };

    let data_points = DataPoints::new_from_data_center(
        center.0,
        center.1,
        x_axis_range.clone(),
        y_axis_range.clone(),
    );

    Ok(data_points)
}

pub fn get_quadrant_gate_for_filter_container(
    filter: &GateSerialized,
    id: Arc<str>,
    name: String,
    params: (&Arc<str>, &Arc<str>),
    x_axis_range: &RangeInclusive<f32>,
    y_axis_range: &RangeInclusive<f32>,
    subgate_ids: &[Arc<str>],
    subgate_names: &[String],
    x_transform: &TransformType,
    y_transform: &TransformType,
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let default_data_points =
        make_data_points_for_quadrant_filter(filter, x_axis_range, y_axis_range)?;
    let subgate_names: (String, String, String, String) = subgate_names
        .iter()
        .cloned()
        .collect_tuple()
        .ok_or_else(|| anyhow::anyhow!("Expected 4 items"))?;

    let x_inf = get_infinite_bounds(x_transform);

    let y_inf = get_infinite_bounds(y_transform);

    let infs = (x_inf, y_inf);

    let default_gate = QuadrantGate::try_new_from_data_points(
        id,
        name,
        default_data_points,
        params.0.clone(),
        params.1.clone(),
        true,
        Some(subgate_ids.to_vec()),
        Some(subgate_names),
        infs,
    )?;
    let default_gate_arc: Arc<dyn DrawableGate> = Arc::new(default_gate);
    Ok(default_gate_arc)
}

pub fn get_bisector_gate_for_filter_container(
    filter: &GateSerialized,
    id: Arc<str>,
    name: String,
    params: (&Arc<str>, &Arc<str>),
    subgate_ids: &[Arc<str>],
    subgate_names: &[String],
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let center = if let GateSerialized::Line { f1max, .. } = filter {
        *f1max as f32
    } else {
        return Err(anyhow::anyhow!("Unexpected gate type for quadrant subgate"));
    };
    let subgate_ids: (Arc<str>, Arc<str>) = subgate_ids
        .iter()
        .cloned()
        .collect_tuple()
        .ok_or_else(|| anyhow::anyhow!("Expected 2 items"))?;
    let subgate_names: (String, String) = subgate_names
        .iter()
        .cloned()
        .collect_tuple()
        .ok_or_else(|| anyhow::anyhow!("Expected 2 items"))?;
    let default_gate = BisectorGate::try_new_from_data_center(
        id,
        name,
        center,
        params.0.clone(),
        params.1.clone(),
        subgate_ids.clone(),
        Some(subgate_names),
    )?;
    let default_gate_arc: Arc<dyn DrawableGate> = Arc::new(default_gate);
    Ok(default_gate_arc)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuadrantPosition {
    TopLeft,     // Q1 / QUAD0
    TopRight,    // Q2 / QUAD1
    BottomRight, // Q3 / QUAD2
    BottomLeft,  // Q4 / QUAD3
}

pub fn calculate_skewed_quadrant_polygon(
    c: Point,
    v1: Point,
    v2: Point,
    x_axis: &RangeInclusive<f32>,
    y_axis: &RangeInclusive<f32>,
) -> (QuadrantPosition, Vec<(f32, f32)>) {
    // 1. Determine the "Direction" of the quadrant using the midpoint of the arms
    let mid_x = ((v1.x + v2.x) / 2.0) as f32;
    let mid_y = ((v1.y + v2.y) / 2.0) as f32;

    let is_right = mid_x > c.x as f32;
    let is_top = mid_y > c.y as f32;

    let quad = match (is_right, is_top) {
        (false, true) => QuadrantPosition::TopLeft, // mid is Left and Up
        (true, true) => QuadrantPosition::TopRight, // mid is Right and Up
        (true, false) => QuadrantPosition::BottomRight, // mid is Right and Down
        (false, false) => QuadrantPosition::BottomLeft, // mid is Left and Down
    };

    // 2. Identify the Axis Corner belonging to this quadrant
    let corner = (
        if is_right {
            *x_axis.end()
        } else {
            *x_axis.start()
        },
        if is_top {
            *y_axis.end()
        } else {
            *y_axis.start()
        },
    );

    // 3. Ensure consistent winding order (Counter-Clockwise)
    // We use the cross product of (v1-c) and (v2-c) to see if they are
    // already in CCW order.
    let cross_product = (v1.x - c.x) * (v2.y - c.y) - (v1.y - c.y) * (v2.x - c.x);

    if cross_product > 0.0 {
        // Already Counter-Clockwise
        (quad, vec![c.into(), v1.into(), corner, v2.into()])
    } else {
        // Clockwise, so swap v1 and v2 to maintain CCW for the renderer
        (quad, vec![c.into(), v2.into(), corner, v1.into()])
    }
}

pub fn angle_gates_to_skewed_data_points(
    angle_gate_map: FxHashMap<QuadrantPosition, Vec<(f32, f32)>>,
    // x_data_range: RangeInclusive<f32>,
    // y_data_range: RangeInclusive<f32>,
) -> anyhow::Result<DataPoints> {
    // we need two opposite quadrants to construct the DataPoints

    let Some(top_left) = angle_gate_map.get(&QuadrantPosition::TopLeft) else {
        return Err(anyhow::anyhow!("Missing top-left quadrant"));
    };
    let Some(bottom_right) = angle_gate_map.get(&QuadrantPosition::BottomRight) else {
        return Err(anyhow::anyhow!("Missing bottom-right quadrant"));
    };
    let center = top_left[0];

    let left = top_left[3];
    let right = bottom_right[3];
    let bottom = bottom_right[1];
    let top = top_left[1];

    Ok(DataPoints {
        center,
        left,
        bottom,
        right,
        top,
        // x_data_range,
        // y_data_range,
    })
}

pub fn get_skewed_quadrant_gate(
    gate_id: Arc<str>,
    composite_group_id: String,
    subgates: &[GateSerialized],
    params: (&Arc<str>, &Arc<str>),
    x_axis_range: &RangeInclusive<f32>,
    y_axis_range: &RangeInclusive<f32>,
    subgate_ids: Vec<Arc<str>>,
    subgate_names: &[String],
    x_transform: &TransformType,
    y_transform: &TransformType,
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let default_gate_map: FxHashMap<_, _> = subgates
        .iter()
        .map(|gs| {
            if let GateSerialized::Angle { center, v1, v2, .. } = &gs {
                Ok(calculate_skewed_quadrant_polygon(
                    *center,
                    *v1,
                    *v2,
                    x_axis_range,
                    y_axis_range,
                ))
            } else {
                Err(anyhow::anyhow!(
                    "Unexpected gate type for skewed quadrant composite subgate"
                ))
            }
        })
        .collect::<anyhow::Result<FxHashMap<_, _>>>()?;

    let subgate_names: (String, String, String, String) = subgate_names
        .iter()
        .cloned()
        .collect_tuple()
        .ok_or_else(|| anyhow::anyhow!("Expected 4 items"))?;

    let data_points = angle_gates_to_skewed_data_points(
        default_gate_map,
        // x_data_range.clone(),
        // y_data_range.clone(),
    )?;

    let x_inf = get_infinite_bounds(x_transform);

    let y_inf = get_infinite_bounds(y_transform);

    let infs = (x_inf, y_inf);

    let gate = SkewedQuadrantGate::try_new_from_data_points(
        gate_id.clone(),
        composite_group_id.clone(),
        data_points,
        params.0.clone(),
        params.1.clone(),
        true,
        Some(subgate_ids.clone()),
        Some(subgate_names),
        infs,
    )?;

    Ok(Arc::new(gate))
}

pub fn validate_metadata_requirements(
    containers: &FxHashMap<GateId, FilterContainer>,
    metadata_headers: &std::collections::HashSet<Arc<str>>,
) {
    for container in containers.values() {
        if let FilterContainer::Atomic(atomic) = container
            && let Some(required_md) = &atomic.md
        {
            // Check if the metadata CSV actually has this column
            if !metadata_headers.contains(required_md) {
                println!(
                    "Gate '{}' ({}) requires metadata column '{}', but it's missing from the CSV!",
                    atomic.name, atomic.id, required_md
                );
            }
        }
    }
}
