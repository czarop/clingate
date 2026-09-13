//! Tests for the Omiq interchange layer.
//!
//! This is the highest-value surface in the codebase to pin down: everything the
//! editor and the eventual autogater work on arrives through here, and the
//! export path will have to reproduce it exactly in reverse.
//!
//! cargo test omiq -- --nocapture

#![cfg(test)]

use crate::omiq::deserialise::*;
use std::collections::HashMap;
use std::sync::Arc;

// ─── JSON fixtures ────────────────────────────────────────────────────────────

/// A rectangle gate as Omiq serialises it.
fn rectangle_json(id: &str, name: &str) -> String {
    format!(
        r#"{{
            "containerType": "AtomicFilterContainer",
            "id": "{id}",
            "name": "{name}",
            "defaultFilter": {{
                "type": "RectangleGate",
                "f1": "FSC-A",
                "f2": "SSC-A",
                "min": {{ "f1Val": 100.0, "f2Val": 200.0 }},
                "max": {{ "f1Val": 900.0, "f2Val": 800.0 }}
            }}
        }}"#
    )
}

/// Two atomic gates under a root node, one nested below the other.
fn simple_experiment() -> String {
    format!(
        r#"{{
            "tree": {{
                "nodes": {{
                    "n1": {{
                        "id": "n1",
                        "parentId": "",
                        "filterContainerId": "g1",
                        "ord": 0,
                        "collapsed": false
                    }},
                    "n2": {{
                        "id": "n2",
                        "parentId": "n1",
                        "filterContainerId": "g2",
                        "ord": 1,
                        "collapsed": true
                    }}
                }},
                "filterContainers": {{
                    "g1": {},
                    "g2": {}
                }}
            }}
        }}"#,
        rectangle_json("g1", "Cells"),
        rectangle_json("g2", "Singlets")
    )
}

fn parse(json: &str) -> ExperimentJson {
    serde_json::from_str(json).expect("fixture should deserialise")
}

fn atomic(container: &FilterContainer) -> &AtomicContainer {
    match container {
        FilterContainer::Atomic(a) => a,
        FilterContainer::Compound(_) => panic!("expected an atomic container"),
    }
}

// ─── Tree parsing ─────────────────────────────────────────────────────────────

#[test]
fn an_experiment_parses_its_nodes_and_containers() {
    let exp = parse(&simple_experiment());

    assert_eq!(exp.tree.nodes.len(), 2);
    assert_eq!(exp.tree.filter_containers.len(), 2);
}

#[test]
fn node_fields_map_from_camel_case() {
    let exp = parse(&simple_experiment());
    let n2 = exp.tree.nodes.get("n2").expect("n2 present");

    assert_eq!(&*n2.id, "n2");
    assert_eq!(&*n2.parent_id, "n1");
    assert_eq!(&*n2.filter_container_id, "g2");
    assert_eq!(n2.ord, 1);
    assert!(n2.collapsed);
}

/// A root node is marked by an empty parentId, not by a missing key - the
/// importer tests for `""` when deciding what hangs off ROOTGATE.
#[test]
fn a_root_node_is_signalled_by_an_empty_parent_id() {
    let exp = parse(&simple_experiment());
    let n1 = exp.tree.nodes.get("n1").expect("n1 present");

    assert_eq!(&*n1.parent_id, "");
}

#[test]
fn a_gate_name_survives_the_round_trip() {
    let exp = parse(&simple_experiment());
    let g1 = atomic(exp.tree.filter_containers.get("g1").unwrap());

    assert_eq!(&*g1.name, "Cells");
    assert_eq!(&*g1.id, "g1");
}

// ─── Gate geometry parsing ────────────────────────────────────────────────────

#[test]
fn a_rectangle_gate_parses_its_corners_and_parameters() {
    let exp = parse(&simple_experiment());
    let g1 = atomic(exp.tree.filter_containers.get("g1").unwrap());

    match &g1.default_filter {
        GateSerialized::Rectangle {
            x_param,
            y_param,
            min,
            max,
            ..
        } => {
            assert_eq!(&**x_param, "FSC-A");
            assert_eq!(&**y_param, "SSC-A");
            assert_eq!((min.x, min.y), (100.0, 200.0));
            assert_eq!((max.x, max.y), (900.0, 800.0));
        }
        _ => panic!("expected a rectangle"),
    }
}

#[test]
fn a_polygon_gate_parses_its_vertices() {
    let json = r#"{
        "type": "PolygonGate",
        "f1": "CD3",
        "f2": "CD4",
        "vertices": [
            { "f1Val": 0.0, "f2Val": 0.0 },
            { "f1Val": 1.0, "f2Val": 0.0 },
            { "f1Val": 0.5, "f2Val": 1.0 }
        ]
    }"#;
    let gate: GateSerialized = serde_json::from_str(json).unwrap();

    match gate {
        GateSerialized::Polygon { points, .. } => {
            assert_eq!(points.len(), 3);
            assert_eq!((points[2].x, points[2].y), (0.5, 1.0));
        }
        _ => panic!("expected a polygon"),
    }
}

#[test]
fn an_ellipse_gate_parses_its_four_control_nodes() {
    let json = r#"{
        "type": "EllipseGate",
        "f1": "CD3",
        "f2": "CD4",
        "left":   { "f1Val": -1.0, "f2Val": 0.0 },
        "right":  { "f1Val":  1.0, "f2Val": 0.0 },
        "top":    { "f1Val":  0.0, "f2Val": 2.0 },
        "bottom": { "f1Val":  0.0, "f2Val": -2.0 }
    }"#;
    let gate: GateSerialized = serde_json::from_str(json).unwrap();

    match gate {
        GateSerialized::Ellipse { left, right, top, .. } => {
            assert_eq!((left.x, left.y), (-1.0, 0.0));
            assert_eq!((right.x, right.y), (1.0, 0.0));
            assert_eq!((top.x, top.y), (0.0, 2.0));
        }
        _ => panic!("expected an ellipse"),
    }
}

#[test]
fn a_range_gate_parses_its_one_dimensional_bounds() {
    let json = r#"{
        "type": "RangeGate",
        "f1": "CD3",
        "f2": "CD4",
        "f1Min": 1.5,
        "f1Max": 3.5
    }"#;
    let gate: GateSerialized = serde_json::from_str(json).unwrap();

    match gate {
        GateSerialized::Line { f1min, f1max, .. } => {
            assert_eq!(f1min, 1.5);
            assert_eq!(f1max, 3.5);
        }
        _ => panic!("expected a range gate"),
    }
}

#[test]
fn an_angle_gate_parses_its_centre_and_arms() {
    let json = r#"{
        "type": "AngleGate",
        "f1": "CD3",
        "f2": "CD4",
        "c":  { "f1Val": 1.0, "f2Val": 1.0 },
        "v1": { "f1Val": 0.0, "f2Val": 1.2 },
        "v2": { "f1Val": 1.2, "f2Val": 0.0 }
    }"#;
    let gate: GateSerialized = serde_json::from_str(json).unwrap();

    match gate {
        GateSerialized::Angle { center, v1, v2, .. } => {
            assert_eq!((center.x, center.y), (1.0, 1.0));
            assert_eq!((v1.x, v1.y), (0.0, 1.2));
            assert_eq!((v2.x, v2.y), (1.2, 0.0));
        }
        _ => panic!("expected an angle gate"),
    }
}

#[test]
fn a_label_position_is_optional() {
    let exp = parse(&simple_experiment());
    let g1 = atomic(exp.tree.filter_containers.get("g1").unwrap());

    match &g1.default_filter {
        GateSerialized::Rectangle { label_position, .. } => assert!(label_position.is_none()),
        _ => panic!("expected a rectangle"),
    }
}

#[test]
fn a_label_position_is_read_when_present() {
    let json = r#"{
        "type": "RectangleGate",
        "f1": "A", "f2": "B",
        "min": { "f1Val": 0.0, "f2Val": 0.0 },
        "max": { "f1Val": 1.0, "f2Val": 1.0 },
        "labelLoc": { "f1Val": 0.25, "f2Val": 0.75 }
    }"#;
    let gate: GateSerialized = serde_json::from_str(json).unwrap();

    match gate {
        GateSerialized::Rectangle { label_position, .. } => {
            let loc = label_position.expect("label position present");
            assert_eq!((loc.x, loc.y), (0.25, 0.75));
        }
        _ => panic!("expected a rectangle"),
    }
}

/// Omiq omits a coordinate rather than writing zero in some exports.
#[test]
fn a_missing_point_coordinate_defaults_to_zero() {
    let point: Point = serde_json::from_str(r#"{ "f1Val": 4.0 }"#).unwrap();
    assert_eq!((point.x, point.y), (4.0, 0.0));
}

#[test]
fn points_convert_to_and_from_tuples() {
    let point = Point { x: 1.5, y: -2.5 };
    let as_f32: (f32, f32) = point.into();
    let as_f64: (f64, f64) = point.into();

    assert_eq!(as_f32, (1.5f32, -2.5f32));
    assert_eq!(as_f64, (1.5f64, -2.5f64));
    assert_eq!(Point::from((1.5f32, -2.5f32)), point);
}

// ─── The Unknown catch-all ────────────────────────────────────────────────────

#[test]
fn an_unrecognised_gate_type_falls_into_the_unknown_variant() {
    let json = r#"{ "type": "SomeFutureGate", "f1": "A", "f2": "B" }"#;
    let gate: GateSerialized = serde_json::from_str(json).unwrap();

    assert!(matches!(gate, GateSerialized::Unknown));
}

/// Regression: `get_params` used to `panic!` on Unknown, so a single gate type
/// this build doesn't model took the whole import down.
#[test]
fn get_params_returns_none_for_an_unknown_gate_rather_than_panicking() {
    let gate: GateSerialized =
        serde_json::from_str(r#"{ "type": "SomeFutureGate" }"#).unwrap();

    assert!(gate.get_params().is_none());
}

#[test]
fn get_params_reports_the_axes_for_every_modelled_gate() {
    let exp = parse(&simple_experiment());
    let g1 = atomic(exp.tree.filter_containers.get("g1").unwrap());

    let (x, y) = g1.default_filter.get_params().expect("rectangle has params");
    assert_eq!((&*x, &*y), ("FSC-A", "SSC-A"));
}

/// Regression: `to_drawable` used to `todo!()` on Unknown.
#[test]
fn to_drawable_errors_on_an_unknown_gate_rather_than_panicking() {
    let gate: GateSerialized =
        serde_json::from_str(r#"{ "type": "SomeFutureGate" }"#).unwrap();

    let result = gate.to_drawable(
        Arc::from("id"),
        Arc::from("name"),
        flow_gates::GateMode::Global,
        false,
    );
    assert!(result.is_err());
}

/// Angle gates only ever appear inside a skewed quadrant, so converting one
/// directly is a programming error - but it must not abort the process.
#[test]
fn to_drawable_errors_on_a_bare_angle_gate() {
    let json = r#"{
        "type": "AngleGate", "f1": "A", "f2": "B",
        "c":  { "f1Val": 0.0, "f2Val": 0.0 },
        "v1": { "f1Val": 1.0, "f2Val": 0.0 },
        "v2": { "f1Val": 0.0, "f2Val": 1.0 }
    }"#;
    let gate: GateSerialized = serde_json::from_str(json).unwrap();

    let result = gate.to_drawable(
        Arc::from("id"),
        Arc::from("name"),
        flow_gates::GateMode::Global,
        false,
    );
    assert!(result.is_err());
}

// ─── Composite detection ──────────────────────────────────────────────────────

#[test]
fn a_container_without_a_group_id_is_not_composite() {
    let exp = parse(&simple_experiment());
    let g1 = atomic(exp.tree.filter_containers.get("g1").unwrap());

    assert!(!g1.is_composite());
    assert!(g1.group_id.is_none());
}

#[test]
fn a_container_with_a_group_id_is_composite() {
    let json = r#"{
        "containerType": "AtomicFilterContainer",
        "id": "q0",
        "name": "Q1",
        "groupId": "ABC_QUAD0",
        "defaultFilter": {
            "type": "RectangleGate",
            "f1": "CD3", "f2": "CD4",
            "min": { "f1Val": 1.0, "f2Val": 1.0 },
            "max": { "f1Val": 9.0, "f2Val": 9.0 }
        }
    }"#;
    let container: FilterContainer = serde_json::from_str(json).unwrap();

    assert!(atomic(&container).is_composite());
}

/// The importer classifies a composite purely by substring, and the order of
/// the checks matters: SKEWEDQUAD contains QUAD, so it must be tested first.
#[test]
fn composite_group_ids_carry_their_type_and_position() {
    for (group_id, expect_skewed) in [
        ("ABC_QUAD0", false),
        ("ABC_SKEWEDQUAD3", true),
        ("ABC_SPLIT1", false),
    ] {
        assert_eq!(group_id.contains("SKEWEDQUAD"), expect_skewed);
        // The trailing digit is the quadrant/half index.
        let position = group_id.chars().last().and_then(|c| c.to_digit(10));
        assert!(position.is_some(), "{group_id} must end in its position index");
    }
}

#[test]
fn per_file_filters_default_to_empty_when_absent() {
    let exp = parse(&simple_experiment());
    let g1 = atomic(exp.tree.filter_containers.get("g1").unwrap());

    assert!(g1.per_file_filters.is_empty());
    assert!(g1.md.is_none());
}

#[test]
fn per_file_filters_are_keyed_by_file_id() {
    let json = r#"{
        "containerType": "AtomicFilterContainer",
        "id": "g1",
        "name": "Cells",
        "md": "$VOL",
        "defaultFilter": {
            "type": "RectangleGate",
            "f1": "FSC-A", "f2": "SSC-A",
            "min": { "f1Val": 0.0, "f2Val": 0.0 },
            "max": { "f1Val": 1.0, "f2Val": 1.0 }
        },
        "perFileFilters": {
            "12345": {
                "type": "RectangleGate",
                "f1": "FSC-A", "f2": "SSC-A",
                "min": { "f1Val": 5.0, "f2Val": 5.0 },
                "max": { "f1Val": 9.0, "f2Val": 9.0 }
            }
        }
    }"#;
    let container: FilterContainer = serde_json::from_str(json).unwrap();
    let a = atomic(&container);

    assert_eq!(a.md.as_deref(), Some("$VOL"));
    assert_eq!(a.per_file_filters.len(), 1);

    let override_gate = a.per_file_filters.get("12345").expect("file override present");
    match override_gate {
        GateSerialized::Rectangle { min, .. } => assert_eq!((min.x, min.y), (5.0, 5.0)),
        _ => panic!("expected a rectangle"),
    }
}

// ─── Boolean (compound) containers ────────────────────────────────────────────

fn compound_experiment() -> String {
    format!(
        r#"{{
            "tree": {{
                "nodes": {{}},
                "filterContainers": {{
                    "g1": {},
                    "b1": {{
                        "containerType": "CompoundFilterContainer",
                        "id": "b1",
                        "name": "Not Cells",
                        "type": "NOT",
                        "filterContainerIds": ["g1"]
                    }}
                }}
            }}
        }}"#,
        rectangle_json("g1", "Cells")
    )
}

#[test]
fn a_compound_container_parses_its_operation_and_operands() {
    let exp = parse(&compound_experiment());

    match exp.tree.filter_containers.get("b1").unwrap() {
        FilterContainer::Compound(c) => {
            assert_eq!(&*c.id, "b1");
            assert_eq!(&*c.name, "Not Cells");
            assert!(matches!(c.operation, BooleanOpType::Not));
            assert_eq!(c.filter_container_ids.len(), 1);
        }
        FilterContainer::Atomic(_) => panic!("expected a compound container"),
    }
}

#[test]
fn boolean_operations_parse_from_upper_case() {
    for (text, matches_and) in [("AND", true), ("OR", false), ("NOT", false)] {
        let json = format!(
            r#"{{
                "containerType": "CompoundFilterContainer",
                "id": "b", "name": "b",
                "type": "{text}",
                "filterContainerIds": ["x"]
            }}"#
        );
        let container: FilterContainer = serde_json::from_str(&json).unwrap();
        match container {
            FilterContainer::Compound(c) => {
                assert_eq!(matches!(c.operation, BooleanOpType::And), matches_and);
            }
            FilterContainer::Atomic(_) => panic!("expected a compound container"),
        }
    }
}

/// A boolean gate has no geometry of its own, so it borrows the axes of the
/// first atomic gate it can reach. That lookup recurses through nested booleans.
#[test]
fn find_atomic_params_resolves_through_a_boolean() {
    let exp = parse(&compound_experiment());
    let containers: HashMap<Arc<str>, FilterContainer> = exp.tree.filter_containers;

    let id: Arc<str> = Arc::from("b1");
    let (x, y) = find_atomic_params(&id, &containers).expect("resolves to g1");

    assert_eq!((&*x, &*y), ("FSC-A", "SSC-A"));
}

#[test]
fn find_atomic_params_resolves_through_nested_booleans() {
    let json = format!(
        r#"{{
            "tree": {{
                "nodes": {{}},
                "filterContainers": {{
                    "g1": {},
                    "b1": {{
                        "containerType": "CompoundFilterContainer",
                        "id": "b1", "name": "inner", "type": "NOT",
                        "filterContainerIds": ["g1"]
                    }},
                    "b2": {{
                        "containerType": "CompoundFilterContainer",
                        "id": "b2", "name": "outer", "type": "AND",
                        "filterContainerIds": ["b1"]
                    }}
                }}
            }}
        }}"#,
        rectangle_json("g1", "Cells")
    );
    let exp = parse(&json);
    let id: Arc<str> = Arc::from("b2");

    let (x, _) = find_atomic_params(&id, &exp.tree.filter_containers).expect("resolves");
    assert_eq!(&*x, "FSC-A");
}

#[test]
fn find_atomic_params_gives_up_on_a_dangling_reference() {
    let exp = parse(&compound_experiment());
    let missing: Arc<str> = Arc::from("does-not-exist");

    assert!(find_atomic_params(&missing, &exp.tree.filter_containers).is_none());
}

// ─── Subgate ordering ─────────────────────────────────────────────────────────

#[test]
fn subgates_are_sorted_by_descending_position() {
    let containers: Vec<(u32, AtomicContainer)> = (0..4)
        .map(|i| {
            let json = rectangle_json(&format!("q{i}"), &format!("Q{i}"));
            let c: FilterContainer = serde_json::from_str(&json).unwrap();
            (i, atomic(&c).clone())
        })
        .collect();

    let (ids, names) = get_sorted_subgate_ids_and_names(&containers);

    assert_eq!(ids.iter().map(|i| &**i).collect::<Vec<_>>(), vec!["q3", "q2", "q1", "q0"]);
    assert_eq!(names, vec!["Q3", "Q2", "Q1", "Q0"]);
}

// ─── Skewed quadrant geometry ─────────────────────────────────────────────────

fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}

#[test]
fn a_skewed_quadrant_is_classified_by_the_midpoint_of_its_arms() {
    let x_axis = 0.0f32..=10.0f32;
    let y_axis = 0.0f32..=10.0f32;
    let centre = point(5.0, 5.0);

    // Arms pointing up and left.
    let (quad, _) = calculate_skewed_quadrant_polygon(
        centre,
        point(4.0, 8.0),
        point(2.0, 6.0),
        &x_axis,
        &y_axis,
    );
    assert_eq!(quad, QuadrantPosition::TopLeft);

    // Arms pointing up and right.
    let (quad, _) = calculate_skewed_quadrant_polygon(
        centre,
        point(6.0, 8.0),
        point(8.0, 6.0),
        &x_axis,
        &y_axis,
    );
    assert_eq!(quad, QuadrantPosition::TopRight);

    // Arms pointing down and right.
    let (quad, _) = calculate_skewed_quadrant_polygon(
        centre,
        point(6.0, 2.0),
        point(8.0, 4.0),
        &x_axis,
        &y_axis,
    );
    assert_eq!(quad, QuadrantPosition::BottomRight);

    // Arms pointing down and left.
    let (quad, _) = calculate_skewed_quadrant_polygon(
        centre,
        point(4.0, 2.0),
        point(2.0, 4.0),
        &x_axis,
        &y_axis,
    );
    assert_eq!(quad, QuadrantPosition::BottomLeft);
}

#[test]
fn a_skewed_quadrant_polygon_closes_on_its_axis_corner() {
    let x_axis = 0.0f32..=10.0f32;
    let y_axis = 0.0f32..=10.0f32;

    let (quad, points) = calculate_skewed_quadrant_polygon(
        point(5.0, 5.0),
        point(6.0, 8.0),
        point(8.0, 6.0),
        &x_axis,
        &y_axis,
    );

    assert_eq!(quad, QuadrantPosition::TopRight);
    assert_eq!(points.len(), 4, "centre, arm, corner, arm");
    assert_eq!(points[0], (5.0, 5.0), "the centre comes first");
    assert_eq!(points[2], (10.0, 10.0), "the top-right corner of the axes");
}

/// The renderer needs a consistent winding order, so the arms are swapped when
/// the cross product says they arrived clockwise.
#[test]
fn a_skewed_quadrant_polygon_is_wound_consistently() {
    let x_axis = 0.0f32..=10.0f32;
    let y_axis = 0.0f32..=10.0f32;
    let centre = point(5.0, 5.0);
    let arm_a = point(6.0, 8.0);
    let arm_b = point(8.0, 6.0);

    let (_, one_way) = calculate_skewed_quadrant_polygon(centre, arm_a, arm_b, &x_axis, &y_axis);
    let (_, other_way) = calculate_skewed_quadrant_polygon(centre, arm_b, arm_a, &x_axis, &y_axis);

    assert_eq!(
        one_way, other_way,
        "the same wedge must produce the same polygon whichever arm is given first"
    );
}

// ─── Ellipse reconstruction ───────────────────────────────────────────────────

/// Omiq stores an ellipse as four control points; the importer recovers the
/// centre, radii and rotation by eigen-decomposition. The export path will need
/// to invert exactly this, so the forward direction is worth nailing down.
#[test]
fn an_axis_aligned_ellipse_recovers_its_centre_and_radii() {
    let geometry = create_omiq_ellipse_geometry(
        (-2.0, 0.0), // left
        (2.0, 0.0),  // right
        (0.0, 1.0),  // top
        "CD3",
        "CD4",
    )
    .expect("valid ellipse");

    match geometry {
        flow_gates::GateGeometry::Ellipse {
            center,
            radius_x,
            radius_y,
            angle,
        } => {
            assert!((center.get_coordinate("CD3").unwrap() - 0.0).abs() < 1e-5);
            assert!((center.get_coordinate("CD4").unwrap() - 0.0).abs() < 1e-5);
            assert!((radius_x - 2.0).abs() < 1e-4, "radius_x was {radius_x}");
            assert!((radius_y - 1.0).abs() < 1e-4, "radius_y was {radius_y}");
            assert!(angle.abs() < 1e-4, "an axis-aligned ellipse has no rotation");
        }
        _ => panic!("expected an ellipse geometry"),
    }
}

#[test]
fn an_offset_ellipse_recovers_its_centre() {
    let geometry =
        create_omiq_ellipse_geometry((3.0, 5.0), (7.0, 5.0), (5.0, 6.0), "CD3", "CD4")
            .expect("valid ellipse");

    match geometry {
        flow_gates::GateGeometry::Ellipse { center, .. } => {
            assert!((center.get_coordinate("CD3").unwrap() - 5.0).abs() < 1e-5);
            assert!((center.get_coordinate("CD4").unwrap() - 5.0).abs() < 1e-5);
        }
        _ => panic!("expected an ellipse geometry"),
    }
}

#[test]
fn the_major_radius_is_never_smaller_than_the_minor_one() {
    // A tall ellipse: the eigen-decomposition should still report the larger
    // eigenvalue as radius_x, with the rotation carrying the orientation.
    let geometry =
        create_omiq_ellipse_geometry((-1.0, 0.0), (1.0, 0.0), (0.0, 4.0), "A", "B")
            .expect("valid ellipse");

    match geometry {
        flow_gates::GateGeometry::Ellipse { radius_x, radius_y, .. } => {
            assert!(radius_x >= radius_y, "{radius_x} should be the major axis");
        }
        _ => panic!("expected an ellipse geometry"),
    }
}

// ─── Axis settings lookup ─────────────────────────────────────────────────────
//
// This feeds the infinite bounds of every quadrant and skewed-quadrant gate on
// import, so getting the per-axis transform right matters.

use crate::gate_editor::AxisInfo;
use crate::gate_editor::plots::axis_store::Param;
use flow_fcs::TransformType;
use rustc_hash::FxBuildHasher;

fn axis(name: &str, lower: f32, upper: f32, transform: TransformType) -> AxisInfo {
    AxisInfo {
        param: Param {
            marker: Arc::from(name),
            fluoro: Arc::from(name),
        },
        axis_lower: lower,
        axis_upper: upper,
        transform,
    }
}

fn settings(entries: Vec<AxisInfo>) -> im::HashMap<Arc<str>, AxisInfo, FxBuildHasher> {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for a in entries {
        map.insert(a.param.fluoro.clone(), a);
    }
    map
}

#[test]
fn axis_ranges_are_read_from_the_matching_parameter() {
    let x: Arc<str> = Arc::from("FSC-A");
    let y: Arc<str> = Arc::from("CD3");
    let s = settings(vec![
        axis("FSC-A", 0.0, 100.0, TransformType::Linear),
        axis("CD3", -1.0, 4.5, TransformType::Arcsinh { cofactor: 6000.0 }),
    ]);

    let (x_range, y_range, _, _) =
        extract_axis_range_from_axis_settings(&(&x, &y), &s).expect("both axes present");

    assert_eq!((*x_range.start(), *x_range.end()), (0.0, 100.0));
    assert_eq!((*y_range.start(), *y_range.end()), (-1.0, 4.5));
}

/// Regression: the Y transform was cloned from the X axis, so any plot whose two
/// axes were scaled differently - a linear scatter channel against an arcsinh
/// fluorescence channel, which is most of them - imported its composite gates
/// with the wrong infinite bounds on Y.
#[test]
fn each_axis_reports_its_own_transform() {
    let x: Arc<str> = Arc::from("FSC-A");
    let y: Arc<str> = Arc::from("CD3");
    let s = settings(vec![
        axis("FSC-A", 0.0, 100.0, TransformType::Linear),
        axis("CD3", -1.0, 4.5, TransformType::Arcsinh { cofactor: 6000.0 }),
    ]);

    let (_, _, x_transform, y_transform) =
        extract_axis_range_from_axis_settings(&(&x, &y), &s).unwrap();

    assert!(
        matches!(x_transform, TransformType::Linear),
        "x should keep its own linear transform"
    );
    assert!(
        matches!(y_transform, TransformType::Arcsinh { cofactor } if cofactor == 6000.0),
        "y must report its own transform, not a copy of x's"
    );
}

#[test]
fn the_two_axes_keep_distinct_cofactors() {
    let x: Arc<str> = Arc::from("CD4");
    let y: Arc<str> = Arc::from("CD8");
    let s = settings(vec![
        axis("CD4", -1.0, 4.0, TransformType::Arcsinh { cofactor: 500.0 }),
        axis("CD8", -1.0, 4.0, TransformType::Arcsinh { cofactor: 9000.0 }),
    ]);

    let (_, _, x_transform, y_transform) =
        extract_axis_range_from_axis_settings(&(&x, &y), &s).unwrap();

    assert!(matches!(x_transform, TransformType::Arcsinh { cofactor } if cofactor == 500.0));
    assert!(matches!(y_transform, TransformType::Arcsinh { cofactor } if cofactor == 9000.0));
}

#[test]
fn a_missing_axis_setting_is_an_error() {
    let x: Arc<str> = Arc::from("FSC-A");
    let missing: Arc<str> = Arc::from("not-configured");
    let s = settings(vec![axis("FSC-A", 0.0, 100.0, TransformType::Linear)]);

    assert!(extract_axis_range_from_axis_settings(&(&x, &missing), &s).is_err());
    assert!(extract_axis_range_from_axis_settings(&(&missing, &x), &s).is_err());
}

// ─── Ellipse handle preservation ──────────────────────────────────────────────
//
// The canonical centre/radii/angle form describes the same ellipse as Omiq's
// four control points, but it cannot say which conjugate pair those points sat
// on. An imported gate therefore carries its original handles so that a
// round trip returns them unchanged rather than a canonicalised equivalent.

use crate::gate_editor::gates::gate_single::ellipse_gate::{EllipseGate, EllipseHandles};
use crate::gate_editor::gates::gate_drag::GateDragData;
use crate::gate_editor::gates::gate_traits::DrawableGate;

fn ellipse_json(left: (f64, f64), top: (f64, f64), right: (f64, f64), bottom: (f64, f64)) -> String {
    format!(
        r#"{{
            "type": "EllipseGate",
            "f1": "CD3",
            "f2": "CD4",
            "left":   {{ "f1Val": {}, "f2Val": {} }},
            "top":    {{ "f1Val": {}, "f2Val": {} }},
            "right":  {{ "f1Val": {}, "f2Val": {} }},
            "bottom": {{ "f1Val": {}, "f2Val": {} }}
        }}"#,
        left.0, left.1, top.0, top.1, right.0, right.1, bottom.0, bottom.1
    )
}

fn import_ellipse(json: &str) -> EllipseGate {
    let spec: GateSerialized = serde_json::from_str(json).unwrap();
    let drawable = spec
        .to_drawable(
            Arc::from("e1"),
            Arc::from("an ellipse"),
            flow_gates::GateMode::Global,
            false,
        )
        .expect("ellipse imports");
    drawable
        .as_any()
        .downcast_ref::<EllipseGate>()
        .expect("an ellipse gate")
        .clone()
}

#[test]
fn an_imported_ellipse_keeps_the_handles_omiq_wrote() {
    let gate = import_ellipse(&ellipse_json(
        (-2.0, 0.0),
        (0.0, 1.0),
        (2.0, 0.0),
        (0.0, -1.0),
    ));

    assert!(gate.has_source_handles());
    let handles = gate.omiq_handles().unwrap();

    assert_eq!(handles.left, (-2.0, 0.0));
    assert_eq!(handles.top, (0.0, 1.0));
    assert_eq!(handles.right, (2.0, 0.0));
    assert_eq!(handles.bottom, (0.0, -1.0));
}

/// The whole point of storing them: an unedited gate exports exactly what came
/// in, with no canonicalisation in between.
#[test]
fn an_unedited_ellipse_round_trips_its_handles_exactly() {
    for (left, top, right, bottom) in [
        ((-2.0, 0.0), (0.0, 1.0), (2.0, 0.0), (0.0, -1.0)),
        ((1.0, 5.0), (5.0, 9.0), (9.0, 5.0), (5.0, 1.0)),
        // A tall ellipse: the minor axis is the left/right pair, which the
        // canonical form would swap onto the major axis.
        ((-1.0, 0.0), (0.0, 4.0), (1.0, 0.0), (0.0, -4.0)),
    ] {
        let gate = import_ellipse(&ellipse_json(left, top, right, bottom));
        let handles = gate.omiq_handles().unwrap();

        assert_eq!(
            (handles.left, handles.top, handles.right, handles.bottom),
            (
                (left.0 as f32, left.1 as f32),
                (top.0 as f32, top.1 as f32),
                (right.0 as f32, right.1 as f32),
                (bottom.0 as f32, bottom.1 as f32)
            ),
            "handles were canonicalised"
        );
    }
}

/// A tall ellipse is the case the canonical form cannot represent faithfully:
/// radius_x always takes the larger eigenvalue, so re-deriving would put
/// left/right on the major axis. The stored handles must not do that.
#[test]
fn a_tall_ellipse_keeps_its_handles_on_the_minor_axis() {
    let gate = import_ellipse(&ellipse_json(
        (-1.0, 0.0),
        (0.0, 4.0),
        (1.0, 0.0),
        (0.0, -4.0),
    ));
    let handles = gate.omiq_handles().unwrap();

    let horizontal_span = (handles.right.0 - handles.left.0).abs();
    let vertical_span = (handles.top.1 - handles.bottom.1).abs();

    assert!(
        horizontal_span < vertical_span,
        "left/right should still be the short pair: {horizontal_span} vs {vertical_span}"
    );
}

#[test]
fn moving_an_ellipse_carries_its_handles_along() {
    let gate = import_ellipse(&ellipse_json(
        (-2.0, 0.0),
        (0.0, 1.0),
        (2.0, 0.0),
        (0.0, -1.0),
    ));

    let drag = GateDragData::new(gate.get_id(), (0.0, 0.0), (10.0, 5.0));
    let moved = gate.replace_points(drag).unwrap().expect("ellipse moves");
    let moved = moved.as_any().downcast_ref::<EllipseGate>().unwrap();

    assert!(moved.has_source_handles(), "a move must not discard them");
    let handles = moved.omiq_handles().unwrap();

    assert_eq!(handles.left, (8.0, 5.0));
    assert_eq!(handles.right, (12.0, 5.0));
    assert_eq!(handles.top, (10.0, 6.0));
    assert_eq!(handles.bottom, (10.0, 4.0));
}

/// A rotation genuinely invalidates the imported pair, so the gate falls back to
/// a derived principal-axis pair rather than reporting stale handles.
#[test]
fn rotating_an_ellipse_drops_the_imported_handles() {
    let gate = import_ellipse(&ellipse_json(
        (-2.0, 0.0),
        (0.0, 1.0),
        (2.0, 0.0),
        (0.0, -1.0),
    ));

    let rotated = gate
        .rotate_gate((5.0, 5.0))
        .unwrap()
        .expect("ellipse rotates");
    let rotated = rotated.as_any().downcast_ref::<EllipseGate>().unwrap();

    assert!(!rotated.has_source_handles());
    // Derived handles are still well formed.
    assert!(rotated.omiq_handles().is_ok());
}

// ─── Derived handles ──────────────────────────────────────────────────────────

#[test]
fn derived_handles_are_opposite_through_the_centre() {
    let handles = EllipseHandles::from_canonical((5.0, 5.0), 3.0, 1.0, 0.7);

    assert!((handles.centre().0 - 5.0).abs() < 1e-5);
    assert!((handles.centre().1 - 5.0).abs() < 1e-5);
    // bottom is top reflected through the centre, and likewise left/right.
    assert!((handles.top.0 + handles.bottom.0 - 10.0).abs() < 1e-5);
    assert!((handles.top.1 + handles.bottom.1 - 10.0).abs() < 1e-5);
    assert!((handles.left.0 + handles.right.0 - 10.0).abs() < 1e-5);
}

#[test]
fn derived_handles_sit_on_the_requested_radii() {
    let handles = EllipseHandles::from_canonical((0.0, 0.0), 4.0, 2.0, 0.0);

    assert_eq!(handles.right, (4.0, 0.0));
    assert_eq!(handles.left, (-4.0, 0.0));
    assert_eq!(handles.top, (0.0, 2.0));
    assert_eq!(handles.bottom, (0.0, -2.0));
}

#[test]
fn derived_handles_follow_the_rotation_angle() {
    let quarter = std::f32::consts::FRAC_PI_2;
    let handles = EllipseHandles::from_canonical((0.0, 0.0), 4.0, 2.0, quarter);

    // A quarter turn puts the major axis on y.
    assert!((handles.right.0).abs() < 1e-5, "right x was {}", handles.right.0);
    assert!((handles.right.1 - 4.0).abs() < 1e-5, "right y was {}", handles.right.1);
}

#[test]
fn translating_handles_moves_all_four_together() {
    let handles = EllipseHandles::from_canonical((0.0, 0.0), 2.0, 1.0, 0.0);
    let moved = handles.translated(3.0, -4.0);

    assert_eq!(moved.right, (5.0, -4.0));
    assert_eq!(moved.left, (1.0, -4.0));
    assert_eq!(moved.top, (3.0, -3.0));
    assert_eq!(moved.bottom, (3.0, -5.0));
}

// ─── Degenerate reconstruction ────────────────────────────────────────────────

/// A circle has equal eigenvalues, so its principal axes - and any angle read
/// off them - are arbitrary. It is pinned to zero rather than left to float
/// noise.
#[test]
fn a_circle_reconstructs_with_no_rotation() {
    let geometry =
        create_omiq_ellipse_geometry((-2.0, 0.0), (2.0, 0.0), (0.0, 2.0), "A", "B").unwrap();

    match geometry {
        flow_gates::GateGeometry::Ellipse { radius_x, radius_y, angle, .. } => {
            assert!((radius_x - radius_y).abs() < 1e-4, "should be a circle");
            assert_eq!(angle, 0.0, "a circle has no meaningful rotation");
        }
        _ => panic!("expected an ellipse"),
    }
}

/// Regression: the angle branch tested `f == 0.0` exactly, so an ellipse that
/// is axis-aligned to within float noise took the atan2 branch with two
/// near-zero arguments.
#[test]
fn a_near_axis_aligned_ellipse_is_treated_as_aligned() {
    let nudge = 1e-9;
    let geometry =
        create_omiq_ellipse_geometry((-4.0, 0.0), (4.0, nudge), (0.0, 2.0), "A", "B").unwrap();

    match geometry {
        flow_gates::GateGeometry::Ellipse { angle, .. } => {
            assert!(
                angle.abs() < 1e-3,
                "a near-aligned ellipse reported {angle} radians"
            );
        }
        _ => panic!("expected an ellipse"),
    }
}

// ─── Real Omiq files: ghost containers ────────────────────────────────────────
//
// Deleting a composite in Omiq removes every node in the group and every
// container *except* any member a live boolean gate still references. That
// survivor stays as a nodeless container so the boolean can still be evaluated,
// and Omiq reparents the deleted gate's children to their grandparent.
//
// The two fixtures are the same experiment before and after: a quadrant with a
// boolean built off one corner, then the quadrant deleted.

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::ROOTGATE;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fixture_axes() -> im::HashMap<Arc<str>, AxisInfo, FxBuildHasher> {
    let mut settings = im::HashMap::with_hasher(FxBuildHasher);
    // Scatter channels are linear and fluorescence channels arcsinh, as the
    // app configures them - the two spaces have very different magnitudes, and
    // the export uses each axis's transform to tell an unbounded edge from a
    // real coordinate.
    for channel in [
        "BUV661-A", "BV785-A", "Alexa Fluor 700-A", "BUV737-A", "BUV805-A",
        "BUV563-A", "Alexa Fluor 647-A", "Vio Bright 423-A",
    ] {
        settings.insert(
            Arc::from(channel) as Arc<str>,
            AxisInfo {
                param: Param {
                    marker: Arc::from(channel),
                    fluoro: Arc::from(channel),
                },
                axis_lower: -1.0,
                axis_upper: 6.0,
                transform: TransformType::Arcsinh { cofactor: 6000.0 },
            },
        );
    }
    for channel in ["FSC-A", "SSC-A"] {
        settings.insert(
            Arc::from(channel) as Arc<str>,
            AxisInfo {
                param: Param {
                    marker: Arc::from(channel),
                    fluoro: Arc::from(channel),
                },
                axis_lower: 0.0,
                axis_upper: 4_194_304.0,
                transform: TransformType::Linear,
            },
        );
    }
    settings
}

fn import(name: &str) -> GateState {
    let mut state = GateState::default();
    let metadata = im::HashMap::with_hasher(FxBuildHasher);
    state
        .upload_gates_from_file(fixture(name), &metadata, fixture_axes())
        .unwrap_or_else(|e| panic!("{name} should import, got: {e}"));
    state
}

const BEFORE: &str = "quadrant_with_boolean_child.omiqgt";
const AFTER: &str = "quadrant_deleted_boolean_survives.omiqgt";

#[test]
fn a_file_with_an_intact_quadrant_imports() {
    let state = import(BEFORE);
    assert!(state.gate_count() > 0);
}

/// Regression: the surviving corner reached the importer through the boolean's
/// operand list, then failed the "exactly 4 subgates" check and aborted the
/// whole import. An incomplete group is no longer a composite.
#[test]
fn a_file_whose_quadrant_was_deleted_still_imports() {
    let state = import(AFTER);
    assert!(state.gate_count() > 0);
}

/// The orphaned corner has no node, so it belongs on no plot - but it must be
/// registered, or the boolean that kept it alive cannot resolve its operand.
#[test]
fn the_orphaned_quadrant_corner_is_registered_but_not_on_a_plot() {
    let state = import(AFTER);
    let orphan: Arc<str> = Arc::from("4RZa");

    assert!(
        state.is_registered(&orphan),
        "the boolean's operand must resolve"
    );
    assert!(
        state.hierarchy_parent(&orphan).is_none(),
        "a nodeless container is not in the gating tree"
    );
    assert!(
        !state.is_on_any_plot(&orphan),
        "a gate with no node must not be drawn"
    );
}

#[test]
fn the_boolean_that_kept_the_corner_alive_is_imported() {
    let state = import(AFTER);
    let boolean: Arc<str> = Arc::from("PvRn");

    assert!(state.is_registered(&boolean));
    assert!(
        state.hierarchy_parent(&boolean).is_some(),
        "the boolean has a node, so it is in the tree"
    );
}

/// Omiq reparents a deleted gate's children to their grandparent rather than
/// orphaning or deleting them - which is why a real file has no dangling
/// parentId anywhere.
#[test]
fn the_deleted_quadrants_children_are_reparented_to_its_parent() {
    let before = import(BEFORE);
    let after = import(AFTER);

    // "IL-2+" hung off the Q4 corner before the deletion, and off the
    // quadrant's own parent (the polygon "teff_naive") afterwards.
    let child: Arc<str> = Arc::from("QCVn");
    let corner: Arc<str> = Arc::from("4RZa");
    let grandparent: Arc<str> = Arc::from("hu4H");

    assert_eq!(before.hierarchy_parent(&child), Some(corner));
    assert_eq!(after.hierarchy_parent(&child), Some(grandparent));
}

#[test]
fn the_intact_quadrants_four_corners_are_all_present_before_deletion() {
    let before = import(BEFORE);
    for corner in ["uevU", "2gGu", "2y0f", "4RZa"] {
        assert!(
            before.is_registered(&Arc::from(corner)),
            "corner {corner} missing"
        );
    }
}

#[test]
fn the_three_deleted_corners_are_gone_afterwards() {
    let after = import(AFTER);
    for corner in ["uevU", "2gGu", "2y0f"] {
        assert!(
            !after.is_registered(&Arc::from(corner)),
            "corner {corner} should have been deleted with the quadrant"
        );
    }
}

#[test]
fn root_level_gates_land_under_the_root() {
    let state = import(BEFORE);
    // "1" is the top rectangle, whose node has an empty parentId.
    assert_eq!(
        state.hierarchy_parent(&Arc::from("uN8Y")),
        Some(ROOTGATE.clone())
    );
}

// ─── Rebuild capture ──────────────────────────────────────────────────────────
//
// Everything the file carries that the editor does not itself need. It is kept
// beside the registry, keyed by gate id, so it survives every edit: none of it
// changes when a gate is dragged.

use crate::omiq::rebuild::{OmiqGateType, OmiqRebuildStore};

#[test]
fn the_document_header_is_captured() {
    let state = import(BEFORE);
    let header = &state.omiq_rebuild().header;

    assert_eq!(header.workflow_id, 1);
    assert_eq!(header.dataset_id, 2);
    assert_eq!(header.task_id, 1);
    assert!(!header.inverted);
    assert!(header.url.starts_with("https://example.invalid/"));
    assert_eq!(header.date, "2020-01-01T00:00:00.000Z");
}

#[test]
fn every_container_gets_a_rebuild_entry() {
    let state = import(BEFORE);
    // 14 containers in the fixture, node-backed or not.
    assert_eq!(state.omiq_rebuild().gates.len(), 14);
}

#[test]
fn a_node_backed_gate_records_its_tree_position() {
    let state = import(BEFORE);
    // "IL-2+" sat under the Q4 corner before the deletion.
    let entry = state
        .omiq_rebuild()
        .get(&Arc::from("QCVn"))
        .expect("captured");

    assert_eq!(entry.node_id.as_deref(), Some("3JDj"));
    assert_eq!(entry.parent_node_id.as_deref(), Some("HWIv"));
    assert_eq!(entry.ord, 1775297369203);
    assert!(!entry.collapsed);
    assert_eq!(&*entry.container_type, "DEFAULT");
}

#[test]
fn the_collapsed_flag_is_captured() {
    let state = import(BEFORE);
    // The Q4 corner's node is collapsed in the fixture.
    let entry = state.omiq_rebuild().get(&Arc::from("4RZa")).unwrap();
    assert!(entry.collapsed);
}

/// A root node is marked by an empty parentId, and that has to come back as an
/// empty string rather than a missing key.
#[test]
fn a_root_gate_records_an_empty_parent() {
    let state = import(BEFORE);
    let entry = state.omiq_rebuild().get(&Arc::from("uN8Y")).unwrap();

    assert_eq!(entry.parent_node_id.as_deref(), Some(""));
}

/// The group id is what says "this is corner 3 of quadrant IinB". It must
/// survive even when the group was broken up, or the gate cannot be written
/// back as the corner it was.
#[test]
fn a_composite_member_records_its_group_id() {
    for fixture in [BEFORE, AFTER] {
        let state = import(fixture);
        let entry = state.omiq_rebuild().get(&Arc::from("4RZa")).unwrap();

        assert_eq!(
            entry.group_id.as_deref(),
            Some("IinB_QUAD3"),
            "group id lost in {fixture}"
        );
    }
}

#[test]
fn the_metadata_column_is_captured() {
    let state = import(BEFORE);

    assert_eq!(
        state.omiq_rebuild().get(&Arc::from("0lmI")).unwrap().md.as_deref(),
        Some("test")
    );
    assert_eq!(
        state.omiq_rebuild().get(&Arc::from("QCVn")).unwrap().md.as_deref(),
        Some("Type")
    );
    assert!(
        state.omiq_rebuild().get(&Arc::from("2PJQ")).unwrap().md.is_none(),
        "a gate with no metadata grouping records none"
    );
}

/// The shape alone cannot say what to write back - a quadrant corner is held as
/// a polygon but was a rectangle in the file.
#[test]
fn the_source_gate_type_is_captured() {
    let state = import(BEFORE);
    let r = state.omiq_rebuild();

    assert_eq!(r.get(&Arc::from("uN8Y")).unwrap().source_type, Some(OmiqGateType::Rectangle));
    assert_eq!(r.get(&Arc::from("4ECA")).unwrap().source_type, Some(OmiqGateType::Polygon));
    assert_eq!(r.get(&Arc::from("XdrW")).unwrap().source_type, Some(OmiqGateType::Ellipse));
    assert_eq!(r.get(&Arc::from("4RZa")).unwrap().source_type, Some(OmiqGateType::Rectangle));
    // A boolean has no geometry of its own.
    assert_eq!(r.get(&Arc::from("Z2Ti")).unwrap().source_type, None);
}

#[test]
fn a_boolean_records_its_operation_as_its_container_type() {
    let state = import(BEFORE);

    assert_eq!(
        &*state.omiq_rebuild().get(&Arc::from("Z2Ti")).unwrap().container_type,
        "NOT"
    );
    assert_eq!(
        &*state.omiq_rebuild().get(&Arc::from("PvRn")).unwrap().container_type,
        "AND"
    );
}

/// A group override has to fan back out to exactly the files Omiq listed, not
/// to a set derived from the metadata - which could differ.
#[test]
fn the_per_file_ids_are_captured_in_a_stable_order() {
    let state = import(BEFORE);
    let entry = state.omiq_rebuild().get(&Arc::from("0lmI")).unwrap();

    assert_eq!(entry.per_file_ids.len(), 2);
    let ids: Vec<&str> = entry.per_file_ids.iter().map(|f| &**f).collect();
    assert_eq!(ids, vec!["sample1", "sample2"], "sorted for reproducible output");
}

#[test]
fn a_gate_with_no_per_file_positions_records_none() {
    let state = import(BEFORE);
    assert!(
        state.omiq_rebuild().get(&Arc::from("4ECA")).unwrap().per_file_ids.is_empty()
    );
}

/// A nodeless container has no tree position to record.
#[test]
fn a_ghost_records_no_node() {
    let state = import(AFTER);
    let entry = state.omiq_rebuild().get(&Arc::from("4RZa")).unwrap();

    assert!(entry.node_id.is_none());
    assert!(entry.parent_node_id.is_none());
    assert_eq!(entry.ord, 0);
}

/// An unreachable container is inert for evaluation but is kept verbatim: one
/// that a live boolean references looks exactly like it until the reachability
/// walk says otherwise, and dropping it would break that boolean in Omiq.
#[test]
fn unreachable_containers_are_kept_verbatim() {
    // Both fixtures are fully reachable, so build the case directly.
    let mut store = OmiqRebuildStore::default();
    let raw: serde_json::Value = serde_json::from_str(
        r#"{"tree":{"nodes":{},"filterContainers":{
            "live":{"containerType":"AtomicFilterContainer","id":"live","name":"a"},
            "ghost":{"containerType":"AtomicFilterContainer","id":"ghost","name":"b","somethingWeDoNotModel":42}
        }}}"#,
    )
    .unwrap();
    let mut reachable = rustc_hash::FxHashSet::default();
    reachable.insert(Arc::from("live") as Arc<str>);

    store.capture_ghosts(&raw, &reachable);

    assert_eq!(store.ghost_containers.len(), 1);
    let ghost = store.ghost_containers.get(&Arc::from("ghost") as &Arc<str>).unwrap();
    assert_eq!(
        ghost.get("somethingWeDoNotModel").and_then(|v| v.as_i64()),
        Some(42),
        "fields we do not model must survive verbatim"
    );
}

/// Fields a future Omiq version adds at the document level must not be dropped.
#[test]
fn unmodelled_header_fields_are_kept() {
    let raw: serde_json::Value = serde_json::from_str(
        r#"{"date":"d","datasetId":1,"inverted":true,"taskId":2,"url":"u","workflowId":3,
            "someFutureField":{"nested":true}}"#,
    )
    .unwrap();
    let header: crate::omiq::rebuild::OmiqDocumentHeader =
        serde_json::from_value(raw).unwrap();

    assert!(header.inverted);
    assert!(
        header.extra.contains_key("someFutureField"),
        "unmodelled fields land in extra, got {:?}",
        header.extra.keys().collect::<Vec<_>>()
    );
}

/// A gate deleted in the editor must not be resurrected by the exporter.
#[test]
fn deleting_a_gate_drops_its_rebuild_entry() {
    let mut state = import(BEFORE);
    let id: Arc<str> = Arc::from("4ECA"); // the "Tmem" polygon

    assert!(state.omiq_rebuild().get(&id).is_some());
    state.remove_gate(id.clone()).unwrap();

    assert!(
        state.omiq_rebuild().get(&id).is_none(),
        "a deleted gate would be written back into the file"
    );
}

/// Deleting a gate takes its descendants' rebuild entries too.
#[test]
fn deleting_a_parent_drops_its_childrens_rebuild_entries() {
    let mut state = import(BEFORE);
    let parent: Arc<str> = Arc::from("hu4H");  // "teff_naive"
    let child: Arc<str> = Arc::from("2PJQ");   // "IFny+", nested below it

    assert!(state.omiq_rebuild().get(&child).is_some());
    state.remove_gate(parent).unwrap();

    assert!(
        state.omiq_rebuild().get(&child).is_none(),
        "a deleted descendant would be written back into the file"
    );
}

/// The entries are keyed by gate id, not by the gate itself, so anything that
/// replaces the gate under the same id leaves them alone.
#[test]
fn rebuild_entries_are_keyed_by_id_not_by_gate() {
    let state = import(BEFORE);
    let id: Arc<str> = Arc::from("uN8Y");

    let entry = state.omiq_rebuild().get(&id).expect("captured");
    assert_eq!(entry.node_id.as_deref(), Some("fn1o"));
    assert!(
        state.is_registered(&id),
        "the id in the rebuild store is the id in the registry"
    );
}

// ─── Writing gates back ───────────────────────────────────────────────────────
//
// The strongest check available: take a real Omiq file, import it, write every
// gate back out, and compare against what the file actually said.

use crate::omiq::serialise::{OMIQ_UNBOUNDED, gate_to_serialized};

/// The `defaultFilter` for each container, straight from the fixture.
fn original_filters(name: &str) -> HashMap<Arc<str>, GateSerialized> {
    let text = std::fs::read_to_string(fixture(name)).unwrap();
    let exp: ExperimentJson = serde_json::from_str(&text).unwrap();
    exp.tree
        .filter_containers
        .into_iter()
        .filter_map(|(id, c)| match c {
            FilterContainer::Atomic(a) => Some((id, a.default_filter)),
            FilterContainer::Compound(_) => None,
        })
        .collect()
}

/// Write one gate back out of an imported state.
fn round_trip(state: &GateState, id: &str) -> GateSerialized {
    let gate_id: Arc<str> = Arc::from(id);
    let gate = state
        .registered_gate(&gate_id)
        .unwrap_or_else(|| panic!("{id} should be registered"));
    let source_type = state.omiq_rebuild().get(&gate_id).and_then(|r| r.source_type);
    gate_to_serialized(&gate, &gate_id, source_type, &fixture_axes())
        .unwrap_or_else(|e| panic!("{id} should serialise, got: {e}"))
}

fn close(a: f64, b: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() <= scale * 1e-6
}

fn assert_point(actual: Point, expected: Point, what: &str) {
    assert!(
        close(actual.x, expected.x) && close(actual.y, expected.y),
        "{what}: wrote ({}, {}), file had ({}, {})",
        actual.x,
        actual.y,
        expected.x,
        expected.y
    );
}

#[test]
fn a_rectangle_writes_back_as_it_came_in() {
    let state = import(BEFORE);
    let originals = original_filters(BEFORE);

    match (round_trip(&state, "uN8Y"), &originals[&Arc::from("uN8Y") as &Arc<str>]) {
        (
            GateSerialized::Rectangle { x_param, y_param, min, max, .. },
            GateSerialized::Rectangle { x_param: ex, y_param: ey, min: emin, max: emax, .. },
        ) => {
            assert_eq!((&*x_param, &*y_param), (&**ex, &**ey));
            assert_point(min, *emin, "rectangle min");
            assert_point(max, *emax, "rectangle max");
        }
        (written, _) => panic!("expected a rectangle, wrote {written:?}"),
    }
}

#[test]
fn a_polygon_writes_back_with_every_vertex_in_order() {
    let state = import(BEFORE);
    let originals = original_filters(BEFORE);

    match (round_trip(&state, "4ECA"), &originals[&Arc::from("4ECA") as &Arc<str>]) {
        (
            GateSerialized::Polygon { points, .. },
            GateSerialized::Polygon { points: expected, .. },
        ) => {
            assert_eq!(points.len(), expected.len(), "vertex count changed");
            for (i, (got, want)) in points.iter().zip(expected).enumerate() {
                assert_point(*got, *want, &format!("polygon vertex {i}"));
            }
        }
        (written, _) => panic!("expected a polygon, wrote {written:?}"),
    }
}

/// The case the canonical form cannot represent: the handles must come back as
/// Omiq wrote them, not as a re-derived principal-axis pair.
#[test]
fn an_ellipse_writes_back_its_original_handles() {
    let state = import(BEFORE);
    let originals = original_filters(BEFORE);

    match (round_trip(&state, "XdrW"), &originals[&Arc::from("XdrW") as &Arc<str>]) {
        (
            GateSerialized::Ellipse { left, top, right, bottom, .. },
            GateSerialized::Ellipse { left: el, top: et, right: er, bottom: eb, .. },
        ) => {
            assert_point(left, *el, "ellipse left");
            assert_point(top, *et, "ellipse top");
            assert_point(right, *er, "ellipse right");
            assert_point(bottom, *eb, "ellipse bottom");
        }
        (written, _) => panic!("expected an ellipse, wrote {written:?}"),
    }
}

/// A quadrant corner is a rectangle in the file but is held as a polygon. Only
/// the captured source type can say which to write.
#[test]
fn a_quadrant_corner_writes_back_as_a_rectangle_not_a_polygon() {
    let state = import(BEFORE);
    let originals = original_filters(BEFORE);

    match (round_trip(&state, "4RZa"), &originals[&Arc::from("4RZa") as &Arc<str>]) {
        (
            GateSerialized::Rectangle { min, max, .. },
            GateSerialized::Rectangle { min: emin, max: emax, .. },
        ) => {
            assert_point(min, *emin, "corner min");
            assert_point(max, *emax, "corner max");
        }
        (written, _) => panic!("expected a rectangle, wrote {written:?}"),
    }
}

/// Gates are held as f32, which cannot represent Omiq's 1e16 exactly - it
/// drifts to 1.0000000272564224e16. Unbounded edges snap back to the constant.
#[test]
fn an_unbounded_edge_writes_back_as_omiqs_own_sentinel() {
    let state = import(BEFORE);

    match round_trip(&state, "4RZa") {
        GateSerialized::Rectangle { min, .. } => {
            assert_eq!(min.x, -OMIQ_UNBOUNDED, "lower x edge is unbounded");
            assert_eq!(min.y, -OMIQ_UNBOUNDED, "lower y edge is unbounded");
        }
        written => panic!("expected a rectangle, wrote {written:?}"),
    }
}

#[test]
fn every_corner_of_an_intact_quadrant_writes_back() {
    let state = import(BEFORE);
    let originals = original_filters(BEFORE);

    for corner in ["uevU", "2gGu", "2y0f", "4RZa"] {
        match (round_trip(&state, corner), &originals[&Arc::from(corner) as &Arc<str>]) {
            (
                GateSerialized::Rectangle { min, max, .. },
                GateSerialized::Rectangle { min: emin, max: emax, .. },
            ) => {
                assert_point(min, *emin, &format!("{corner} min"));
                assert_point(max, *emax, &format!("{corner} max"));
            }
            (written, _) => panic!("{corner}: expected a rectangle, wrote {written:?}"),
        }
    }
}

/// The orphaned corner is imported as a standalone gate, but it was corner 3 of
/// a quadrant and has to go back as one.
#[test]
fn an_orphaned_corner_still_writes_back_as_its_original_type() {
    let state = import(AFTER);
    let originals = original_filters(AFTER);

    match (round_trip(&state, "4RZa"), &originals[&Arc::from("4RZa") as &Arc<str>]) {
        (
            GateSerialized::Rectangle { min, max, .. },
            GateSerialized::Rectangle { min: emin, max: emax, .. },
        ) => {
            assert_point(min, *emin, "orphan min");
            assert_point(max, *emax, "orphan max");
        }
        (written, _) => panic!("expected a rectangle, wrote {written:?}"),
    }
    // And it still knows which group it belonged to.
    assert_eq!(
        state.omiq_rebuild().get(&Arc::from("4RZa")).unwrap().group_id.as_deref(),
        Some("IinB_QUAD3")
    );
}

/// Every atomic gate in both fixtures, written back and compared against the
/// file - the broadest check the fixtures support.
#[test]
fn every_gate_in_both_fixtures_round_trips() {
    for name in [BEFORE, AFTER] {
        let state = import(name);
        let originals = original_filters(name);
        let mut checked = 0;

        for (id, original) in &originals {
            let Some(gate) = state.registered_gate(id) else { continue };
            if gate.is_composite() {
                continue; // written per corner, covered above
            }
            let source_type = state.omiq_rebuild().get(id).and_then(|r| r.source_type);
            let written = gate_to_serialized(&gate, id, source_type, &fixture_axes())
                .unwrap_or_else(|e| panic!("{name}/{id}: {e}"));

            assert_eq!(
                std::mem::discriminant(&written),
                std::mem::discriminant(original),
                "{name}/{id}: gate type changed"
            );
            assert_eq!(
                written.get_params(),
                original.get_params(),
                "{name}/{id}: parameters changed"
            );
            checked += 1;
        }

        assert!(checked >= 8, "{name}: only checked {checked} gates");
    }
}

#[test]
fn a_boolean_gate_is_not_written_as_a_filter() {
    let state = import(BEFORE);
    let id: Arc<str> = Arc::from("Z2Ti");
    let gate = state.registered_gate(&id).unwrap();

    assert!(
        gate_to_serialized(&gate, &id, None, &fixture_axes()).is_err(),
        "a boolean is a compound container, not a filter"
    );
}
