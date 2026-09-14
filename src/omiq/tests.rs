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
use crate::gate_editor::gates::gate_store::NodeId;

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
    let header = state.omiq_rebuild().header.as_ref().expect("captured from the file");

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

    let node = entry.primary_node().expect("has a node");
    assert_eq!(&*node.node_id, "3JDj");
    assert_eq!(&*node.parent_node_id, "HWIv");
    assert_eq!(node.ord, 1775297369203);
    assert!(!node.collapsed);
    assert_eq!(&*entry.container_type, "DEFAULT");
}

#[test]
fn the_collapsed_flag_is_captured() {
    let state = import(BEFORE);
    // The Q4 corner's node is collapsed in the fixture.
    let entry = state.omiq_rebuild().get(&Arc::from("4RZa")).unwrap();
    assert!(entry.primary_node().expect("has a node").collapsed);
}

/// A root node is marked by an empty parentId, and that has to come back as an
/// empty string rather than a missing key.
#[test]
fn a_root_gate_records_an_empty_parent() {
    let state = import(BEFORE);
    let entry = state.omiq_rebuild().get(&Arc::from("uN8Y")).unwrap();

    assert_eq!(&*entry.primary_node().expect("has a node").parent_node_id, "");
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

    assert!(entry.nodes.is_empty(), "a ghost has no placement in the tree");
    assert!(entry.primary_node().is_none());
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
    assert_eq!(&*entry.primary_node().expect("has a node").node_id, "fn1o");
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

// ─── Whole-document round trip ────────────────────────────────────────────────
//
// Import a real file, write a new one from scratch, and compare against what
// Omiq actually wrote. This is the check that matters: everything else tests a
// piece of the path.

use crate::omiq::metadata::MetaDataFileMap;
use crate::omiq::serialise::to_omiq_document;

fn export(name: &str) -> serde_json::Value {
    let state = import(name);
    let metadata = im::HashMap::with_hasher(FxBuildHasher);
    to_omiq_document(&state, &metadata, &fixture_axes())
        .unwrap_or_else(|e| panic!("{name} should export, got: {e}"))
}

fn original(name: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(fixture(name)).unwrap()).unwrap()
}

fn objects<'a>(doc: &'a serde_json::Value, path: &[&str]) -> &'a serde_json::Map<String, serde_json::Value> {
    let mut cursor = doc;
    for key in path {
        cursor = cursor.get(key).unwrap_or_else(|| panic!("missing {key}"));
    }
    cursor.as_object().expect("an object")
}

#[test]
fn the_document_header_is_written_back() {
    for name in [BEFORE, AFTER] {
        let written = export(name);
        let source = original(name);

        for key in ["date", "datasetId", "inverted", "taskId", "url", "workflowId"] {
            assert_eq!(
                written.get(key),
                source.get(key),
                "{name}: header field {key} changed"
            );
        }
    }
}

#[test]
fn every_node_is_written_back_with_its_tree_position() {
    for name in [BEFORE, AFTER] {
        let written = export(name);
        let source = original(name);
        let written_nodes = objects(&written, &["tree", "nodes"]);
        let source_nodes = objects(&source, &["tree", "nodes"]);

        assert_eq!(
            written_nodes.len(),
            source_nodes.len(),
            "{name}: node count changed"
        );

        for (id, node) in source_nodes {
            let got = written_nodes
                .get(id)
                .unwrap_or_else(|| panic!("{name}: node {id} missing"));
            for key in ["id", "parentId", "filterContainerId", "ord", "collapsed"] {
                assert_eq!(got.get(key), node.get(key), "{name}: node {id} field {key}");
            }
        }
    }
}

#[test]
fn every_container_is_written_back() {
    for name in [BEFORE, AFTER] {
        let written = export(name);
        let source = original(name);

        let written_ids: std::collections::BTreeSet<&String> =
            objects(&written, &["tree", "filterContainers"]).keys().collect();
        let source_ids: std::collections::BTreeSet<&String> =
            objects(&source, &["tree", "filterContainers"]).keys().collect();

        assert_eq!(written_ids, source_ids, "{name}: container set changed");
    }
}

#[test]
fn a_container_keeps_its_identity_and_grouping() {
    let written = export(BEFORE);
    let source = original(BEFORE);
    let w = objects(&written, &["tree", "filterContainers"]);
    let s = objects(&source, &["tree", "filterContainers"]);

    for (id, container) in s {
        let got = &w[id];
        for key in ["id", "name", "containerType", "groupId", "md"] {
            assert_eq!(
                got.get(key),
                container.get(key),
                "container {id} field {key}"
            );
        }
    }
}

#[test]
fn a_boolean_is_written_as_a_compound_container_with_its_operands() {
    let written = export(BEFORE);
    let containers = objects(&written, &["tree", "filterContainers"]);

    let boolean = &containers["PvRn"];
    assert_eq!(boolean["containerType"], "CompoundFilterContainer");
    assert_eq!(boolean["type"], "AND");
    assert_eq!(boolean["name"], "IL18a AND Q4 IL-22- / IL-17A-");

    let operands: Vec<&str> = boolean["filterContainerIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(operands, vec!["WE82", "4RZa"]);
}

#[test]
fn a_not_gate_keeps_its_single_operand() {
    let written = export(BEFORE);
    let containers = objects(&written, &["tree", "filterContainers"]);

    assert_eq!(containers["Z2Ti"]["type"], "NOT");
    assert_eq!(
        containers["Z2Ti"]["filterContainerIds"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn a_gate_geometry_is_written_back_to_the_same_values() {
    let written = export(BEFORE);
    let source = original(BEFORE);
    let w = objects(&written, &["tree", "filterContainers"]);
    let s = objects(&source, &["tree", "filterContainers"]);

    for (id, container) in s {
        let Some(expected) = container.get("defaultFilter") else { continue };
        let got = w[id].get("defaultFilter").expect("a filter was written");

        assert_eq!(
            got["type"], expected["type"],
            "container {id}: gate type changed"
        );
        assert_eq!(got["f1"], expected["f1"], "container {id}: x axis changed");
        assert_eq!(got["f2"], expected["f2"], "container {id}: y axis changed");
    }
}

/// The orphaned corner has no node, so it must appear among the containers but
/// not in the tree - exactly as Omiq itself wrote it.
#[test]
fn a_nodeless_container_is_written_without_a_node() {
    let written = export(AFTER);

    assert!(
        objects(&written, &["tree", "filterContainers"]).contains_key("4RZa"),
        "the boolean's operand must be in the file"
    );
    assert!(
        objects(&written, &["tree", "nodes"])
            .values()
            .all(|n| n["filterContainerId"] != "4RZa"),
        "a nodeless container must not gain a node"
    );
}

/// Per-file positions are what Omiq reads for a sample, so they have to come
/// back for exactly the files the original listed.
#[test]
fn per_file_positions_are_written_for_the_same_files() {
    let written = export(BEFORE);
    let source = original(BEFORE);
    let w = objects(&written, &["tree", "filterContainers"]);
    let s = objects(&source, &["tree", "filterContainers"]);

    let mut checked = 0;
    for (id, container) in s {
        let Some(expected) = container.get("perFileFilters").and_then(|v| v.as_object()) else {
            continue;
        };
        if expected.is_empty() {
            continue;
        }
        let got = w[id]["perFileFilters"].as_object().expect("written");

        let want: std::collections::BTreeSet<&String> = expected.keys().collect();
        let have: std::collections::BTreeSet<&String> = got.keys().collect();
        assert_eq!(have, want, "container {id}: per-file set changed");
        checked += 1;
    }
    assert!(checked >= 3, "only checked {checked} containers");
}

/// The re-imported file has to describe the same gating as the original, which
/// is the property that actually matters for Omiq accepting it.
#[test]
fn an_exported_document_can_be_imported_again() {
    for name in [BEFORE, AFTER] {
        let first = import(name);
        let written = export(name);

        let path = std::env::temp_dir().join(format!(
            "clingate-roundtrip-{}-{name}",
            std::process::id()
        ));
        std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();

        let mut second = GateState::default();
        let metadata = im::HashMap::with_hasher(FxBuildHasher);
        second
            .upload_gates_from_file(path.clone(), &metadata, fixture_axes())
            .unwrap_or_else(|e| panic!("{name}: re-import failed: {e}"));
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            second.gate_count(),
            first.gate_count(),
            "{name}: gate count changed across a round trip"
        );

        for id in first.registered_ids() {
            assert!(
                second.is_registered(&id),
                "{name}: gate {id} lost across a round trip"
            );
            assert_eq!(
                second.hierarchy_parent(&id),
                first.hierarchy_parent(&id),
                "{name}: gate {id} moved in the tree"
            );
        }
    }
}

// ─── Per-file and per-group positions ─────────────────────────────────────────
//
// Omiq writes one entry per file even when a metadata column is what drives the
// position. Import folds those into one gate per group; export has to fan them
// back out over exactly the files the original listed.

/// sample1 and sample2 sit in different groups of both metadata columns the
/// fixture uses, so a per-group position resolves differently for each.
fn fixture_metadata() -> MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, group) in [("sample1", "one"), ("sample2", "two")] {
        let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = rustc_hash::FxHashMap::default();
        columns.insert(Arc::from("test"), Arc::from(group));
        columns.insert(Arc::from("Type"), Arc::from(group));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    map
}

fn import_with_metadata(name: &str) -> GateState {
    let mut state = GateState::default();
    state
        .upload_gates_from_file(fixture(name), &fixture_metadata(), fixture_axes())
        .unwrap_or_else(|e| panic!("{name} should import, got: {e}"));
    state
}

fn export_with_metadata(name: &str) -> serde_json::Value {
    let state = import_with_metadata(name);
    to_omiq_document(&state, &fixture_metadata(), &fixture_axes())
        .unwrap_or_else(|e| panic!("{name} should export, got: {e}"))
}

/// A metadata-grouped gate holds one position per group, not per file. Both
/// files must still get an entry on the way out.
#[test]
fn a_grouped_gate_fans_back_out_to_every_file() {
    let written = export_with_metadata(BEFORE);
    let containers = objects(&written, &["tree", "filterContainers"]);

    let per_file = containers["QCVn"]["perFileFilters"]
        .as_object()
        .expect("per-file positions written");

    let files: std::collections::BTreeSet<&String> = per_file.keys().collect();
    assert_eq!(
        files,
        ["sample1".to_string(), "sample2".to_string()].iter().collect(),
        "both files need an entry"
    );
}

/// The real check: two samples in different groups have genuinely different
/// positions in the fixture, and those have to survive the round trip rather
/// than collapsing onto the default.
#[test]
fn per_group_positions_survive_the_round_trip() {
    let written = export_with_metadata(BEFORE);
    let source = original(BEFORE);

    let written_pf = objects(&written, &["tree", "filterContainers"])["QCVn"]["perFileFilters"]
        .as_object()
        .unwrap()
        .clone();
    let source_pf = objects(&source, &["tree", "filterContainers"])["QCVn"]["perFileFilters"]
        .as_object()
        .unwrap()
        .clone();

    // f2Val is where the two samples genuinely differ in this fixture; f1Val
    // happens to be identical, which would make the comparison vacuous.
    for file in ["sample1", "sample2"] {
        for (corner, axis) in [("min", "f2Val"), ("max", "f1Val"), ("max", "f2Val")] {
            let got = written_pf[file][corner][axis].as_f64().unwrap();
            let want = source_pf[file][corner][axis].as_f64().unwrap();
            assert!(
                (got - want).abs() < 1e-6,
                "{file} {corner}.{axis}: wrote {got}, file had {want}"
            );
        }
    }

    // And the two really are different, so the test could fail.
    assert_ne!(
        source_pf["sample1"]["min"]["f2Val"].as_f64().unwrap(),
        source_pf["sample2"]["min"]["f2Val"].as_f64().unwrap(),
        "the fixture must have distinct per-group positions for this to prove anything"
    );
}

/// A per-file position must not leak onto the gate's default.
#[test]
fn the_default_position_is_unaffected_by_the_per_file_ones() {
    let written = export_with_metadata(BEFORE);
    let source = original(BEFORE);

    let got = objects(&written, &["tree", "filterContainers"])["QCVn"]["defaultFilter"]["min"]
        ["f1Val"]
        .as_f64()
        .unwrap();
    let want = objects(&source, &["tree", "filterContainers"])["QCVn"]["defaultFilter"]["min"]
        ["f1Val"]
        .as_f64()
        .unwrap();

    assert!((got - want).abs() < 1e-6, "wrote {got}, file had {want}");
}

#[test]
fn a_grouped_gate_keeps_its_metadata_column() {
    let written = export_with_metadata(BEFORE);
    let containers = objects(&written, &["tree", "filterContainers"]);

    assert_eq!(containers["QCVn"]["md"], "Type");
    assert_eq!(containers["0lmI"]["md"], "test");
}

#[test]
fn a_metadata_driven_file_still_round_trips_end_to_end() {
    let first = import_with_metadata(BEFORE);
    let written = export_with_metadata(BEFORE);

    let path = std::env::temp_dir().join(format!("clingate-md-roundtrip-{}", std::process::id()));
    std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();

    let mut second = GateState::default();
    second
        .upload_gates_from_file(path.clone(), &fixture_metadata(), fixture_axes())
        .unwrap_or_else(|e| panic!("re-import failed: {e}"));
    let _ = std::fs::remove_file(&path);

    assert_eq!(second.gate_count(), first.gate_count());
    for id in first.registered_ids() {
        assert!(second.is_registered(&id), "gate {id} lost");
    }
}

// ─── Linked gates ─────────────────────────────────────────────────────────────
//
// Omiq keys nodes separately from filter containers, so one gate can be applied
// at several points in the tree. In a real 250-node file, 38 of 146 containers
// were shared this way and one appeared at nine points. Capturing only one node
// per container silently dropped 104 of those placements.

fn linked_gate_json() -> String {
    format!(
        r#"{{
            "tree": {{
                "nodes": {{
                    "na": {{ "id": "na", "parentId": "",   "filterContainerId": "g1", "ord": 0, "collapsed": false }},
                    "nb": {{ "id": "nb", "parentId": "",   "filterContainerId": "g2", "ord": 1, "collapsed": false }},
                    "nc": {{ "id": "nc", "parentId": "na", "filterContainerId": "shared", "ord": 2, "collapsed": false }},
                    "nd": {{ "id": "nd", "parentId": "nb", "filterContainerId": "shared", "ord": 3, "collapsed": true }}
                }},
                "filterContainers": {{
                    "g1": {},
                    "g2": {},
                    "shared": {}
                }}
            }}
        }}"#,
        rectangle_json("g1", "Branch A"),
        rectangle_json("g2", "Branch B"),
        rectangle_json("shared", "Applied twice")
    )
}

fn import_json(json: &str) -> GateState {
    let path = std::env::temp_dir().join(format!(
        "clingate-linked-{}-{:?}.omiqgt",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, json).unwrap();
    let mut state = GateState::default();
    state
        .upload_gates_from_file(path.clone(), &fixture_metadata(), fixture_axes())
        .expect("imports");
    let _ = std::fs::remove_file(path);
    state
}

#[test]
fn a_gate_used_at_two_points_records_both_placements() {
    let state = import_json(&linked_gate_json());
    let entry = state
        .omiq_rebuild()
        .get(&Arc::from("shared"))
        .expect("captured");

    assert_eq!(entry.nodes.len(), 2, "both placements must be recorded");
    let ids: Vec<&str> = entry.nodes.iter().map(|n| &*n.node_id).collect();
    assert!(ids.contains(&"nc") && ids.contains(&"nd"), "got {ids:?}");
}

#[test]
fn each_placement_keeps_its_own_parent_and_flags() {
    let state = import_json(&linked_gate_json());
    let entry = state.omiq_rebuild().get(&Arc::from("shared")).unwrap();

    let nc = entry.nodes.iter().find(|n| &*n.node_id == "nc").unwrap();
    let nd = entry.nodes.iter().find(|n| &*n.node_id == "nd").unwrap();

    assert_eq!(&*nc.parent_node_id, "na");
    assert_eq!(&*nd.parent_node_id, "nb");
    assert!(!nc.collapsed);
    assert!(nd.collapsed, "the two placements differ in more than parent");
}

/// Regression: every placement has to come back, or branches of the tree
/// silently lose their gates.
#[test]
fn every_placement_is_written_back() {
    let state = import_json(&linked_gate_json());
    let written = to_omiq_document(&state, &fixture_metadata(), &fixture_axes()).unwrap();
    let nodes = objects(&written, &["tree", "nodes"]);

    assert_eq!(nodes.len(), 4, "all four nodes must be written");

    let for_shared: Vec<&String> = nodes
        .iter()
        .filter(|(_, n)| n["filterContainerId"] == "shared")
        .map(|(id, _)| id)
        .collect();
    assert_eq!(for_shared.len(), 2, "the shared gate appears twice");
}

#[test]
fn placements_are_written_in_a_stable_order() {
    let first = to_omiq_document(
        &import_json(&linked_gate_json()),
        &fixture_metadata(),
        &fixture_axes(),
    )
    .unwrap();
    let second = to_omiq_document(
        &import_json(&linked_gate_json()),
        &fixture_metadata(),
        &fixture_axes(),
    )
    .unwrap();

    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "the same input must produce byte-identical output"
    );
}

// ─── Whole-file check against a real export ───────────────────────────────────

/// Point `OMIQ_GATING_FILE` at a real `.omiqgt` to check the whole path against
/// it: import, write a new document, and confirm the structure survives.
///
/// Skipped when unset, so it costs nothing in a normal run. Real files are far
/// larger than the fixtures - one had 250 nodes over 146 containers, with 153
/// unreachable ones - and exercise shapes the fixtures do not.
#[test]
fn a_real_gating_file_survives_a_round_trip() {
    let Ok(path) = std::env::var("OMIQ_GATING_FILE") else {
        return;
    };
    let text = std::fs::read_to_string(&path).expect("readable");
    let source: serde_json::Value = serde_json::from_str(&text).expect("valid json");

    // Axis settings for every channel the file mentions: scatter linear, the
    // rest arcsinh, as the app configures them.
    let mut channels = std::collections::BTreeSet::new();
    fn walk(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for key in ["f1", "f2"] {
                    if let Some(s) = m.get(key).and_then(|x| x.as_str()) {
                        out.insert(s.to_string());
                    }
                }
                m.values().for_each(|x| walk(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(&source, &mut channels);

    let mut axes = im::HashMap::with_hasher(FxBuildHasher);
    for channel in &channels {
        let linear =
            channel.contains("FSC") || channel.contains("SSC") || channel.contains("Time");
        axes.insert(
            Arc::from(channel.as_str()) as Arc<str>,
            AxisInfo {
                param: Param {
                    marker: Arc::from(channel.as_str()),
                    fluoro: Arc::from(channel.as_str()),
                },
                axis_lower: if linear { 0.0 } else { -1.0 },
                axis_upper: if linear { 4_194_304.0 } else { 6.0 },
                transform: if linear {
                    TransformType::Linear
                } else {
                    TransformType::Arcsinh { cofactor: 6000.0 }
                },
            },
        );
    }

    // Metadata for every file the gating file names, so grouped composites can
    // resolve; the groups themselves do not matter here.
    let mut files = std::collections::BTreeSet::new();
    let mut columns = std::collections::BTreeSet::new();
    for container in source["tree"]["filterContainers"].as_object().unwrap().values() {
        if let Some(per_file) = container.get("perFileFilters").and_then(|v| v.as_object()) {
            files.extend(per_file.keys().cloned());
        }
        if let Some(md) = container.get("md").and_then(|v| v.as_str()) {
            columns.insert(md.to_string());
        }
    }
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    for (i, file) in files.iter().enumerate() {
        let mut cols: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = rustc_hash::FxHashMap::default();
        for column in &columns {
            cols.insert(
                Arc::from(column.as_str()),
                Arc::from(format!("group{}", i % 3).as_str()),
            );
        }
        metadata.insert(Arc::from(file.as_str()) as Arc<str>, cols);
    }

    let mut state = GateState::default();
    state
        .upload_gates_from_file(std::path::PathBuf::from(&path), &metadata, axes.clone())
        .expect("a real file should import");

    let written = to_omiq_document(&state, &metadata, &axes).expect("a real file should export");

    assert_eq!(
        objects(&written, &["tree", "nodes"]).len(),
        source["tree"]["nodes"].as_object().unwrap().len(),
        "node count changed - every placement of a linked gate must be written"
    );
    assert_eq!(
        objects(&written, &["tree", "filterContainers"]).len(),
        source["tree"]["filterContainers"].as_object().unwrap().len(),
        "container count changed - unreachable containers must be passed through"
    );

    let tmp = std::env::temp_dir().join(format!("clingate-real-{}.omiqgt", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string(&written).unwrap()).unwrap();
    let mut again = GateState::default();
    again
        .upload_gates_from_file(tmp.clone(), &metadata, axes)
        .expect("the written file should import again");
    let _ = std::fs::remove_file(tmp);

    assert_eq!(
        again.gate_count(),
        state.gate_count(),
        "gate count changed across a round trip"
    );
}

// ─── Gates created in the editor ──────────────────────────────────────────────
//
// A gate created here has no captured provenance, so everything the export
// normally takes from that record has to be synthesised: a node id, its place in
// the tree, and - for a composite - the grouping that ties its corners together.

use crate::gate_editor::gates::gate_store::GateId;
use crate::gate_editor::gates::gate_types::PrimaryGateType;
use crate::gate_editor::plots::axis_store::PlotMapper;
use crate::omiq::rebuild::OmiqDocumentHeader;
use crate::omiq::serialise::to_omiq_document_with_header;

fn editor_mapper() -> PlotMapper {
    PlotMapper::new(
        600.0, 600.0,
        0.0..=1000.0, 0.0..=1000.0, 0.0..=1000.0, 0.0..=1000.0,
        TransformType::Linear, TransformType::Linear,
    )
}

fn add(state: &mut GateState, kind: PrimaryGateType, name: &str, parent: Option<GateId>) -> GateId {
    state
        .add_gate(
            &editor_mapper(), 300.0, 300.0,
            Arc::from("FSC-A"), Arc::from("SSC-A"),
            None, parent, kind, Some(name.to_string()),
        )
        .unwrap();
    state
        .registered_ids()
        .into_iter()
        .find(|id| {
            state
                .registered_gate(id)
                .is_some_and(|g| g.get_name() == name && !g.is_composite())
        })
        .unwrap_or_else(|| panic!("{name} should be registered"))
}

fn test_header() -> OmiqDocumentHeader {
    OmiqDocumentHeader {
        date: "2020-01-01T00:00:00.000Z".into(),
        dataset_id: 2,
        inverted: false,
        task_id: 1,
        url: "https://example.invalid/".into(),
        workflow_id: 1,
        extra: Default::default(),
    }
}

fn export_new(state: &GateState) -> serde_json::Value {
    to_omiq_document_with_header(
        state,
        &im::HashMap::with_hasher(FxBuildHasher),
        &fixture_axes(),
        test_header(),
    )
    .expect("exports")
}

/// Regression: a gate created here landed in filterContainers but never in
/// nodes, leaving it invisible in Omiq - the same state a deleted gate is left
/// in.
#[test]
fn a_gate_created_here_gets_a_node() {
    let mut state = GateState::default();
    let id = add(&mut state, PrimaryGateType::Rectangle, "new rect", None);

    let written = export_new(&state);
    let nodes = objects(&written, &["tree", "nodes"]);

    assert_eq!(nodes.len(), 1, "the new gate needs a node");
    let node = nodes.values().next().unwrap();
    assert_eq!(node["filterContainerId"], &*id);
    assert_eq!(node["parentId"], "", "a root gate has an empty parentId");
}

#[test]
fn a_new_gate_carries_its_sibling_order() {
    let mut state = GateState::default();
    let id = add(&mut state, PrimaryGateType::Rectangle, "ordered", None);

    let written = export_new(&state);
    let node = objects(&written, &["tree", "nodes"]).values().next().unwrap().clone();

    assert_eq!(
        node["ord"].as_u64(),
        state.gate_order(&id),
        "ord comes from the hierarchy, which already uses Omiq's millisecond shape"
    );
    assert!(node["ord"].as_u64().unwrap() > 0);
}

#[test]
fn a_new_child_hangs_off_its_new_parent() {
    let mut state = GateState::default();
    let parent = add(&mut state, PrimaryGateType::Rectangle, "parent", None);
    let child = add(&mut state, PrimaryGateType::Rectangle, "child", Some(parent.clone()));

    let written = export_new(&state);
    let nodes = objects(&written, &["tree", "nodes"]);

    let parent_node = nodes.values().find(|n| n["filterContainerId"] == &*parent).unwrap();
    let child_node = nodes.values().find(|n| n["filterContainerId"] == &*child).unwrap();

    assert_eq!(
        child_node["parentId"], parent_node["id"],
        "the child must name its parent's node, not the parent's gate id"
    );
}

/// A gate added under an imported one has to attach to that gate's existing
/// node, not to a freshly minted id.
#[test]
fn a_new_child_of_an_imported_gate_attaches_to_its_existing_node() {
    let mut state = import(BEFORE);
    let imported_parent: Arc<str> = Arc::from("uN8Y");
    let child = add(
        &mut state,
        PrimaryGateType::Rectangle,
        "added below an imported gate",
        Some(imported_parent.clone()),
    );

    let written = export_new(&state);
    let nodes = objects(&written, &["tree", "nodes"]);

    let child_node = nodes.values().find(|n| n["filterContainerId"] == &*child).unwrap();
    assert_eq!(
        child_node["parentId"], "fn1o",
        "the imported parent's own node id"
    );
}

/// Regression: a new quadrant exported as four unrelated polygons, because the
/// groupId that ties a composite together comes from the captured provenance.
#[test]
fn a_new_quadrant_keeps_its_corners_grouped() {
    let mut state = GateState::default();
    state
        .add_gate(
            &editor_mapper(), 300.0, 300.0,
            Arc::from("FSC-A"), Arc::from("SSC-A"),
            None, None, PrimaryGateType::Quadrant, Some("new quad".to_string()),
        )
        .unwrap();

    let written = export_new(&state);
    let containers = objects(&written, &["tree", "filterContainers"]);

    let groups: Vec<&str> = containers
        .values()
        .filter_map(|c| c.get("groupId").and_then(|v| v.as_str()))
        .collect();

    assert_eq!(groups.len(), 4, "all four corners need a groupId");

    // One shared prefix, and all four indices present.
    let prefixes: std::collections::BTreeSet<&str> =
        groups.iter().map(|g| g.rsplit_once("_QUAD").unwrap().0).collect();
    assert_eq!(prefixes.len(), 1, "the corners must share one group");

    let indices: std::collections::BTreeSet<&str> =
        groups.iter().map(|g| g.rsplit_once("_QUAD").unwrap().1).collect();
    assert_eq!(
        indices,
        ["0", "1", "2", "3"].into_iter().collect(),
        "got {groups:?}"
    );
}

/// Omiq writes quadrant corners as rectangles; the editor holds them as
/// polygons, so without a written type they would change shape on the way out.
#[test]
fn a_new_quadrants_corners_are_written_as_rectangles() {
    let mut state = GateState::default();
    state
        .add_gate(
            &editor_mapper(), 300.0, 300.0,
            Arc::from("FSC-A"), Arc::from("SSC-A"),
            None, None, PrimaryGateType::Quadrant, Some("new quad".to_string()),
        )
        .unwrap();

    let written = export_new(&state);
    for container in objects(&written, &["tree", "filterContainers"]).values() {
        assert_eq!(
            container["defaultFilter"]["type"], "RectangleGate",
            "a quadrant corner is a rectangle in the file"
        );
    }
}

#[test]
fn a_new_bisector_keeps_its_halves_grouped() {
    let mut state = GateState::default();
    state
        .add_gate(
            &editor_mapper(), 300.0, 300.0,
            Arc::from("FSC-A"), Arc::from("SSC-A"),
            None, None, PrimaryGateType::Bisector, Some("new split".to_string()),
        )
        .unwrap();

    let written = export_new(&state);
    let groups: Vec<&str> = objects(&written, &["tree", "filterContainers"])
        .values()
        .filter_map(|c| c.get("groupId").and_then(|v| v.as_str()))
        .collect();

    assert_eq!(groups.len(), 2);
    let indices: std::collections::BTreeSet<&str> =
        groups.iter().map(|g| g.rsplit_once("_SPLIT").unwrap().1).collect();
    assert_eq!(indices, ["0", "1"].into_iter().collect(), "got {groups:?}");
}

#[test]
fn every_corner_of_a_new_composite_gets_its_own_node() {
    let mut state = GateState::default();
    state
        .add_gate(
            &editor_mapper(), 300.0, 300.0,
            Arc::from("FSC-A"), Arc::from("SSC-A"),
            None, None, PrimaryGateType::Quadrant, Some("new quad".to_string()),
        )
        .unwrap();

    let written = export_new(&state);
    assert_eq!(
        objects(&written, &["tree", "nodes"]).len(),
        4,
        "each corner is its own node in Omiq"
    );
}

/// Writing zeros for the dataset and workflow would produce a plausible-looking
/// file that Omiq cannot place.
#[test]
fn exporting_without_an_imported_header_is_an_error() {
    let mut state = GateState::default();
    add(&mut state, PrimaryGateType::Rectangle, "new rect", None);

    let result = to_omiq_document(
        &state,
        &im::HashMap::with_hasher(FxBuildHasher),
        &fixture_axes(),
    );

    assert!(result.is_err(), "a header-less export must not be written");
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("never imported"),
        "the error should say why: {message}"
    );
}

#[test]
fn an_imported_file_exports_without_a_supplied_header() {
    let state = import(BEFORE);
    assert!(
        to_omiq_document(
            &state,
            &im::HashMap::with_hasher(FxBuildHasher),
            &fixture_axes()
        )
        .is_ok(),
        "an imported file carries its own header"
    );
}

/// New gates and imported ones have to coexist: the imported tree is untouched
/// and the new gate joins it.
#[test]
fn new_and_imported_gates_export_together() {
    let mut state = import(BEFORE);
    let before_nodes = objects(&export(BEFORE), &["tree", "nodes"]).len();

    let child = add(
        &mut state,
        PrimaryGateType::Rectangle,
        "a new one",
        Some(Arc::from("uN8Y")),
    );

    let written = export_new(&state);
    let nodes = objects(&written, &["tree", "nodes"]);

    assert_eq!(nodes.len(), before_nodes + 1, "exactly one node added");
    assert!(
        nodes.values().any(|n| n["filterContainerId"] == &*child),
        "the new gate is in the tree"
    );
}

#[test]
fn a_file_with_new_gates_can_be_imported_again() {
    let mut state = import(BEFORE);
    let child = add(
        &mut state,
        PrimaryGateType::Rectangle,
        "a new one",
        Some(Arc::from("uN8Y")),
    );
    let written = export_new(&state);

    let path = std::env::temp_dir().join(format!("clingate-new-{}.omiqgt", std::process::id()));
    std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();

    let mut again = GateState::default();
    again
        .upload_gates_from_file(
            path.clone(),
            &im::HashMap::with_hasher(FxBuildHasher),
            fixture_axes(),
        )
        .expect("re-imports");
    let _ = std::fs::remove_file(&path);

    assert!(again.is_registered(&child), "the new gate survived");
    assert_eq!(
        again.hierarchy_parent(&child),
        Some(Arc::from("uN8Y")),
        "and kept its place in the tree"
    );
}


// ─── Linked gates in the tree ─────────────────────────────────────────────────
//
// Stage 2: the hierarchy is keyed by node, so a gate applied at several points
// occupies several positions instead of whichever node the import saw last.

#[test]
fn a_linked_gate_occupies_every_point_it_is_applied_at() {
    let state = import_json(&linked_gate_json());
    let shared: GateId = Arc::from("shared");

    assert_eq!(state.placement_count(&shared), 2);
    assert!(state.is_linked(&shared));
}

#[test]
fn each_placement_keeps_its_own_parent() {
    let state = import_json(&linked_gate_json());
    let shared: GateId = Arc::from("shared");

    let mut parents: Vec<String> = state
        .nodes_for_gate(&shared)
        .iter()
        .filter_map(|n| state.parent_node(n))
        .map(|p| p.to_string())
        .collect();
    parents.sort();

    assert_eq!(parents, vec!["na".to_string(), "nb".to_string()]);
}

/// A plot's data depends on its gating chain and on nothing below it.
///
/// This is what lets `plot_window` re-filter only when a gate in the chain
/// moves. The expensive half of drawing a plot - filtering the frame and
/// rebuilding the event index - used to run on every gate edit anywhere,
/// because the resource depended on the whole resolver rather than on the
/// chain. A gate drawn *on* a plot is a child of that plot's parent node, so it
/// must not appear in the chain, or editing it would reload the plot it is
/// drawn on.
#[test]
fn a_nodes_chain_holds_its_ancestors_and_never_its_children() {
    let state = import_json(&linked_gate_json());

    // `na` is a root node showing gate `g1`; `nc` hangs below it showing
    // `shared`. A plot parented on `na` draws `shared` on top of it.
    let parent = NodeId::from(Arc::<str>::from("na"));
    let child = NodeId::from(Arc::<str>::from("nc"));

    let parent_chain: Vec<String> = state
        .gate_chain_for_node(&parent)
        .iter()
        .map(|g| g.to_string())
        .collect();
    let child_chain: Vec<String> = state
        .gate_chain_for_node(&child)
        .iter()
        .map(|g| g.to_string())
        .collect();

    assert_eq!(
        parent_chain,
        vec!["g1".to_string()],
        "a plot's chain is its own gate and its ancestors"
    );
    assert!(
        !parent_chain.contains(&"shared".to_string()),
        "a gate drawn on the plot must not filter the plot it is drawn on"
    );
    assert_eq!(
        child_chain,
        vec!["g1".to_string(), "shared".to_string()],
        "the child's own plot does narrow by both"
    );
}

/// The point of the exercise. Statistics for a linked gate were computed
/// against one arbitrary parent chain, because the chain was taken from the
/// gate and a gate had only one position.
#[test]
fn each_placement_gates_on_its_own_ancestors() {
    let state = import_json(&linked_gate_json());
    let shared: GateId = Arc::from("shared");

    let mut chains: Vec<Vec<String>> = state
        .nodes_for_gate(&shared)
        .iter()
        .map(|n| {
            state
                .gate_chain_for_node(n)
                .iter()
                .map(|g| g.to_string())
                .collect()
        })
        .collect();
    chains.sort();

    assert_eq!(
        chains,
        vec![
            vec!["g1".to_string(), "shared".to_string()],
            vec!["g2".to_string(), "shared".to_string()],
        ],
        "the two placements must narrow to different populations"
    );
}

#[test]
fn an_unlinked_gate_has_exactly_one_placement() {
    let state = import_json(&linked_gate_json());
    for id in [Arc::from("g1") as GateId, Arc::from("g2")] {
        assert_eq!(state.placement_count(&id), 1);
        assert!(!state.is_linked(&id));
    }
}

/// A node id is Omiq's, and is not the container id once a file is imported.
/// Anything that conflates the two silently reads the wrong row.
#[test]
fn node_ids_are_omiqs_not_the_containers() {
    let state = import_json(&linked_gate_json());
    let shared: GateId = Arc::from("shared");

    let nodes: Vec<String> = state
        .nodes_for_gate(&shared)
        .iter()
        .map(|n| n.to_string())
        .collect();

    assert!(!nodes.contains(&"shared".to_string()), "{nodes:?}");
    assert!(state.gate_for_node(&NodeId::from("nc")).is_some());
    assert_eq!(state.gate_for_node(&NodeId::from("nc")), Some(&shared));
}

#[test]
fn deleting_a_linked_gate_removes_every_placement() {
    let mut state = import_json(&linked_gate_json());
    let shared: GateId = Arc::from("shared");
    state.remove_gate(shared.clone()).unwrap();

    assert_eq!(state.placement_count(&shared), 0);
    assert!(!state.is_registered(&shared));
    // The gates it was applied under are untouched.
    assert!(state.is_registered(&Arc::from("g1")));
    assert!(state.is_registered(&Arc::from("g2")));
}

/// Every placement still has to reach the file, which is what the node table
/// now drives rather than a separate list.
#[test]
fn every_placement_survives_the_round_trip() {
    let state = import_json(&linked_gate_json());
    let doc = to_omiq_document_with_header(
        &state,
        &fixture_metadata(),
        &fixture_axes(),
        test_header(),
    )
    .expect("exports");

    let nodes = doc["tree"]["nodes"].as_object().unwrap();
    let shared_nodes = nodes
        .values()
        .filter(|n| n.get("filterContainerId").and_then(|v| v.as_str()) == Some("shared"))
        .count();

    assert_eq!(shared_nodes, 2, "both placements must be written");
}


// ─── The exported tree is the editor's tree ───────────────────────────────────
//
// Node emission used to come from two places: the placements captured when the
// file was read, for an imported gate, and a node minted at export time for one
// created in the session. The captured half was a snapshot, so the file
// described the tree as it was at import, not as it stood.

/// The invariant the fold establishes: one Omiq node per position in the tree,
/// with the same id, parent and gate. Nothing else decides the shape.
fn assert_nodes_match_tree(state: &GateState, written: &serde_json::Value) {
    let nodes = objects(written, &["tree", "nodes"]);
    let placements: Vec<_> = state.placements().collect();

    assert_eq!(
        nodes.len(),
        placements.len(),
        "one node per placement, no more and no fewer"
    );

    for (node_id, placement) in placements {
        let node = nodes
            .get(node_id.as_str())
            .unwrap_or_else(|| panic!("no node written for placement {node_id}"));

        assert_eq!(
            node["filterContainerId"], &*placement.gate_id,
            "node {node_id} names the wrong gate"
        );

        let expected_parent = state
            .parent_node(node_id)
            .map(|p| p.to_string())
            .filter(|p| p.as_str() != &**ROOTGATE)
            .unwrap_or_default();
        assert_eq!(
            node["parentId"], expected_parent,
            "node {node_id} has the wrong parent"
        );
    }
}

#[test]
fn an_imported_tree_exports_exactly_its_placements() {
    let state = import_json(&linked_gate_json());
    let written = to_omiq_document_with_header(
        &state, &fixture_metadata(), &fixture_axes(), test_header(),
    )
    .expect("exports");

    assert_nodes_match_tree(&state, &written);
}

#[test]
fn a_tree_of_new_gates_exports_exactly_its_placements() {
    let mut state = GateState::default();
    let parent = add(&mut state, PrimaryGateType::Rectangle, "parent", None);
    add(&mut state, PrimaryGateType::Rectangle, "child", Some(parent.clone()));

    assert_nodes_match_tree(&state, &export_new(&state));
}

#[test]
fn new_and_imported_gates_share_one_node_source() {
    let mut state = import_json(&linked_gate_json());
    add(&mut state, PrimaryGateType::Rectangle, "added later", Some(Arc::from("g1")));

    let written = to_omiq_document_with_header(
        &state, &fixture_metadata(), &fixture_axes(), test_header(),
    )
    .expect("exports");

    assert_nodes_match_tree(&state, &written);
}

/// A ghost has no position, so it writes a container and no node - which is
/// what keeps a boolean that references it working without putting a deleted
/// gate back in the user's tree.
#[test]
fn a_ghost_writes_a_container_but_no_node() {
    let state = import_json(&linked_gate_json());
    let ghosts: Vec<_> = state
        .registered_ids()
        .into_iter()
        .filter(|id| state.is_ghost(id))
        .collect();

    let written = to_omiq_document_with_header(
        &state, &fixture_metadata(), &fixture_axes(), test_header(),
    )
    .expect("exports");
    let nodes = objects(&written, &["tree", "nodes"]);

    for ghost in ghosts {
        assert!(
            !nodes
                .values()
                .any(|n| n.get("filterContainerId").and_then(|v| v.as_str()) == Some(&*ghost)),
            "ghost {ghost} must not appear in the tree"
        );
    }
    assert_nodes_match_tree(&state, &written);
}


// ─── Linking, unlinking, and deleting one instance ────────────────────────────

fn shared_gate() -> GateId {
    Arc::from("shared")
}

#[test]
fn deleting_one_instance_leaves_the_gate_at_its_other_points() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();
    let node = state.nodes_for_gate(&shared)[0].clone();

    state.delete_placement(&node).unwrap();

    assert_eq!(state.placement_count(&shared), 1, "one position went");
    assert!(state.is_registered(&shared), "the gate itself survives");
    assert!(!state.is_linked(&shared));
}

/// Dropping the last position of a gate a boolean is built on leaves a ghost,
/// not a hole: the boolean has to go on evaluating.
///
/// This used to be asserted against `linked_gate_json`, which has no boolean in
/// it at all - so it was pinning the mechanism (a ghost is always kept) rather
/// than the reason for it. The AFTER fixture has a real boolean, so the test
/// can now turn on whether anything actually reaches the gate.
#[test]
fn deleting_the_last_instance_leaves_a_ghost_when_a_boolean_needs_it() {
    let mut state = import(AFTER);
    let operand = the_live_operand(&state);

    for node in state.nodes_for_gate(&operand).to_vec() {
        state.delete_placement(&node).unwrap();
    }

    assert_eq!(state.placement_count(&operand), 0, "it is on no plot now");
    assert!(
        state.is_ghost(&operand),
        "but its boolean still reaches it, so it stays resolvable"
    );
}

/// And the other half: a gate nothing reaches is not worth keeping. It is drawn
/// nowhere, no boolean evaluates against it, and nothing in the UI brings it
/// back - it would only accumulate and be written out on export.
#[test]
fn deleting_the_last_instance_of_an_unreferenced_gate_collects_it() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();

    for node in state.nodes_for_gate(&shared).to_vec() {
        state.delete_placement(&node).unwrap();
    }

    assert_eq!(state.placement_count(&shared), 0);
    assert!(
        !state.is_registered(&shared),
        "no boolean references it, so there is nothing to keep it for"
    );
}

#[test]
fn deleting_an_instance_takes_that_placements_children_only() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();
    let nodes = state.nodes_for_gate(&shared).to_vec();
    let child = add(&mut state, PrimaryGateType::Rectangle, "under one", Some(nodes[0].as_arc().clone()));

    state.delete_placement(&nodes[0]).unwrap();

    assert_eq!(state.placement_count(&child), 0, "its child went with it");
    assert_eq!(state.placement_count(&shared), 1, "the other position is untouched");
}

#[test]
fn deleting_a_position_that_does_not_exist_is_an_error() {
    let mut state = import_json(&linked_gate_json());
    assert!(state.delete_placement(&NodeId::from("nowhere")).is_err());
}

#[test]
fn linking_makes_two_positions_share_one_gate() {
    let mut state = import_json(&linked_gate_json());
    let g1: GateId = Arc::from("g1");
    let g2: GateId = Arc::from("g2");
    let (node, target) = (
        state.nodes_for_gate(&g1)[0].clone(),
        state.nodes_for_gate(&g2)[0].clone(),
    );

    state.link_node_to_gate(&node, &target).unwrap();

    assert_eq!(state.gate_for_node(&node), Some(&g2), "it shows g2 now");
    assert_eq!(state.placement_count(&g2), 2);
    assert!(state.is_linked(&g2));
    assert_eq!(state.placement_count(&g1), 0, "g1 is applied nowhere");
    assert!(state.is_ghost(&g1), "but is still resolvable");
}

#[test]
fn linking_keeps_each_position_where_it_was() {
    let mut state = import_json(&linked_gate_json());
    let g1: GateId = Arc::from("g1");
    let g2: GateId = Arc::from("g2");
    let node = state.nodes_for_gate(&g1)[0].clone();
    let before = state.parent_node(&node);

    state
        .link_node_to_gate(&node, &state.nodes_for_gate(&g2)[0].clone())
        .unwrap();

    assert_eq!(state.parent_node(&node), before, "the tree did not move");
}

#[test]
fn a_position_cannot_be_linked_to_itself() {
    let mut state = import_json(&linked_gate_json());
    let node = state.nodes_for_gate(&Arc::from("g1"))[0].clone();
    assert!(state.link_node_to_gate(&node, &node).is_err());
}

#[test]
fn linking_two_positions_that_already_share_a_gate_is_an_error() {
    let mut state = import_json(&linked_gate_json());
    let nodes = state.nodes_for_gate(&shared_gate()).to_vec();
    assert!(state.link_node_to_gate(&nodes[0], &nodes[1]).is_err());
}

#[test]
fn gates_on_different_axes_cannot_be_linked() {
    let mut state = GateState::default();
    let a = add(&mut state, PrimaryGateType::Rectangle, "on fsc ssc", None);

    // A second gate on a different parameter pair.
    state
        .add_gate(
            &editor_mapper(), 300.0, 300.0,
            Arc::from("CD3"), Arc::from("CD4"),
            None, None, PrimaryGateType::Rectangle, Some("on cd3 cd4".to_string()),
        )
        .unwrap();
    let b = state
        .registered_ids()
        .into_iter()
        .find(|id| id != &a)
        .expect("a second gate");

    let err = state
        .link_node_to_gate(&NodeId::from(a.clone()), &NodeId::from(b))
        .unwrap_err()
        .to_string();

    assert!(err.contains("different axes"), "{err}");
}

#[test]
fn unlinking_gives_a_position_a_gate_of_its_own() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();
    let node = state.nodes_for_gate(&shared)[0].clone();

    let new_id = state.unlink_node(&node).unwrap();

    assert_ne!(new_id, shared);
    assert_eq!(state.gate_for_node(&node), Some(&new_id));
    assert_eq!(state.placement_count(&shared), 1, "the other keeps the original");
    assert_eq!(state.placement_count(&new_id), 1);
    assert!(!state.is_linked(&shared));
    assert!(!state.is_linked(&new_id));
}

#[test]
fn an_unlinked_copy_keeps_the_geometry_it_had() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();
    let node = state.nodes_for_gate(&shared)[0].clone();
    let before = state.registered_gate(&shared).unwrap();

    let new_id = state.unlink_node(&node).unwrap();
    let after = state.registered_gate(&new_id).unwrap();

    assert_eq!(after.get_params(), before.get_params());
    assert_eq!(
        after.get_gate_ref(None).map(|g| g.geometry.clone()).is_some(),
        true
    );
    assert_eq!(after.get_id(), new_id, "the copy answers to its own id");
}

#[test]
fn unlinking_a_gate_applied_once_is_an_error() {
    let mut state = import_json(&linked_gate_json());
    let node = state.nodes_for_gate(&Arc::from("g1"))[0].clone();
    assert!(state.unlink_node(&node).is_err());
}

/// The whole point: a link made here has to reach the file.
#[test]
fn a_link_made_here_is_written_to_the_file() {
    let mut state = import_json(&linked_gate_json());
    let g1: GateId = Arc::from("g1");
    let g2: GateId = Arc::from("g2");
    state
        .link_node_to_gate(
            &state.nodes_for_gate(&g1)[0].clone(),
            &state.nodes_for_gate(&g2)[0].clone(),
        )
        .unwrap();

    let written = to_omiq_document_with_header(
        &state, &fixture_metadata(), &fixture_axes(), test_header(),
    )
    .expect("exports");

    let nodes = objects(&written, &["tree", "nodes"]);
    let g2_nodes = nodes
        .values()
        .filter(|n| n.get("filterContainerId").and_then(|v| v.as_str()) == Some("g2"))
        .count();

    assert_eq!(g2_nodes, 2, "both positions must name g2");
    assert_nodes_match_tree(&state, &written);
}

#[test]
fn an_unlink_made_here_is_written_to_the_file() {
    let mut state = import_json(&linked_gate_json());
    let node = state.nodes_for_gate(&shared_gate())[0].clone();
    let new_id = state.unlink_node(&node).unwrap();

    let written = to_omiq_document_with_header(
        &state, &fixture_metadata(), &fixture_axes(), test_header(),
    )
    .expect("exports");

    let containers = objects(&written, &["tree", "filterContainers"]);
    assert!(containers.contains_key(&*new_id), "the copy needs a container");
    assert_nodes_match_tree(&state, &written);
}


// ─── What is drawn on a plot after linking and unlinking ──────────────────────
//
// Regression: unlinking added the new gate to the plot but never removed the
// one the position had stopped showing, so both rendered - the old one still
// moving with the gate it was linked to. Linking had the same defect: it only
// dropped a gate from the plot when the gate had no positions left anywhere,
// so a gate applied elsewhere stayed drawn where it no longer was.

#[test]
fn unlinking_leaves_one_gate_on_the_plot_not_two() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();
    let node = state.nodes_for_gate(&shared)[0].clone();
    let plot = state.parent_node(&node).unwrap().to_string();

    let before = state.view_ids_for_probe(&plot).len();
    let new_id = state.unlink_node(&node).unwrap();
    let drawn = state.view_ids_for_probe(&plot);

    assert_eq!(drawn.len(), before, "unlinking must not add a gate to the plot");
    assert!(drawn.contains(&new_id.to_string()), "the copy is drawn: {drawn:?}");
    assert!(
        !drawn.contains(&shared.to_string()),
        "the gate this position stopped showing must come off the plot: {drawn:?}"
    );
}

#[test]
fn unlinking_leaves_the_other_position_drawing_the_original() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();
    let nodes = state.nodes_for_gate(&shared).to_vec();
    let other_plot = state.parent_node(&nodes[1]).unwrap().to_string();

    state.unlink_node(&nodes[0]).unwrap();

    assert!(
        state.view_ids_for_probe(&other_plot).contains(&shared.to_string()),
        "the sibling position still shows the original"
    );
}

#[test]
fn linking_takes_the_old_gate_off_the_plot() {
    let mut state = import_json(&linked_gate_json());
    let g1: GateId = Arc::from("g1");
    let g2: GateId = Arc::from("g2");
    let node = state.nodes_for_gate(&g1)[0].clone();
    let plot = state.parent_node(&node).unwrap().to_string();

    state
        .link_node_to_gate(&node, &state.nodes_for_gate(&g2)[0].clone())
        .unwrap();

    let drawn = state.view_ids_for_probe(&plot);
    assert!(!drawn.contains(&g1.to_string()), "g1 is no longer applied here: {drawn:?}");
    assert!(drawn.contains(&g2.to_string()), "g2 is: {drawn:?}");
}

/// A gate applied at two positions under the *same* parent must survive losing
/// one of them - the plot still shows it.
#[test]
fn a_gate_shown_twice_on_one_plot_stays_when_one_position_goes() {
    let mut state = GateState::default();
    let a = add(&mut state, PrimaryGateType::Rectangle, "a", None);
    add(&mut state, PrimaryGateType::Rectangle, "b", None);
    let b = state
        .registered_ids()
        .into_iter()
        .find(|id| id != &a)
        .expect("a second gate");

    // Point b's position at a, so both root positions show a.
    state
        .link_node_to_gate(&NodeId::from(b.clone()), &NodeId::from(a.clone()))
        .unwrap();
    assert_eq!(state.placement_count(&a), 2);

    // Dropping one of them leaves the other, so a stays on the plot.
    state.delete_placement(&NodeId::from(b)).unwrap();

    let drawn = state.view_ids_for_probe(&ROOTGATE);
    assert!(drawn.contains(&a.to_string()), "still shown at its other position: {drawn:?}");
}

#[test]
fn deleting_a_position_takes_its_gate_off_that_plot() {
    let mut state = import_json(&linked_gate_json());
    let shared = shared_gate();
    let node = state.nodes_for_gate(&shared)[0].clone();
    let plot = state.parent_node(&node).unwrap().to_string();

    state.delete_placement(&node).unwrap();

    assert!(
        !state.view_ids_for_probe(&plot).contains(&shared.to_string()),
        "nothing shows it on that plot any more"
    );
}


// ─── Linking composites ───────────────────────────────────────────────────────
//
// Omiq links a composite by placing the whole group under each parent: in a
// real export a skewed quadrant has all four corners under the same three
// parents, each corner keeping its own ord. So a composite link is one action
// over every corner, and half a linked quadrant is a corrupt document.

/// Two quadrants under the root, so one can be linked to the other.
fn two_quadrants() -> (GateState, Arc<dyn DrawableGate>, Arc<dyn DrawableGate>) {
    let mut state = GateState::default();
    for name in ["first", "second"] {
        state
            .add_gate(
                &editor_mapper(), 300.0, 300.0, Arc::from("FSC-A"), Arc::from("SSC-A"),
                None, None, PrimaryGateType::Quadrant, Some(name.to_string()),
            )
            .unwrap();
    }
    let composites: Vec<_> = state
        .registered_ids()
        .into_iter()
        .filter_map(|id| {
            let g = state.registered_gate(&id)?;
            (g.is_composite() && g.get_id() == id).then_some(g)
        })
        .collect();
    assert_eq!(composites.len(), 2, "two composites");
    let (a, b) = (composites[0].clone(), composites[1].clone());
    (state, a, b)
}

#[test]
fn linking_a_composite_moves_every_corner() {
    let (mut state, source, target) = two_quadrants();
    let corner = source.get_inner_gate_ids()[0].clone();
    let node = state.nodes_for_gate(&corner)[0].clone();
    let target_node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();

    state.link_node_to_gate(&node, &target_node).unwrap();

    // Every corner of the source now shows the matching corner of the target.
    for (from, to) in source
        .get_inner_gate_ids()
        .iter()
        .zip(target.get_inner_gate_ids().iter())
    {
        assert_eq!(state.placement_count(from), 0, "{from} is applied nowhere now");
        assert_eq!(state.placement_count(to), 2, "{to} is applied at both points");
        assert!(state.is_linked(to));
    }
}

#[test]
fn corners_link_to_the_matching_corner_not_an_arbitrary_one() {
    let (mut state, source, target) = two_quadrants();
    let source_corners = source.get_inner_gate_ids();
    let target_corners = target.get_inner_gate_ids();

    let node = state.nodes_for_gate(&source_corners[0])[0].clone();
    let target_node = state.nodes_for_gate(&target_corners[0])[0].clone();
    // Remember which node held which corner before the link.
    let nodes: Vec<NodeId> = source_corners
        .iter()
        .map(|c| state.nodes_for_gate(c)[0].clone())
        .collect();

    state.link_node_to_gate(&node, &target_node).unwrap();

    for (i, n) in nodes.iter().enumerate() {
        assert_eq!(
            state.gate_for_node(n),
            Some(&target_corners[i]),
            "corner {i} must take the target's corner {i}"
        );
    }
}

#[test]
fn a_composite_cannot_be_linked_to_a_single_gate() {
    let (mut state, source, _) = two_quadrants();
    let single = add(&mut state, PrimaryGateType::Rectangle, "plain", None);
    let node = state.nodes_for_gate(&source.get_inner_gate_ids()[0])[0].clone();

    let err = state
        .link_node_to_gate(&node, &NodeId::from(single))
        .unwrap_err()
        .to_string();

    assert!(err.contains("only be linked to another composite"), "{err}");
}

#[test]
fn a_quadrant_cannot_be_linked_to_a_bisector() {
    let mut state = GateState::default();
    for (kind, name) in [
        (PrimaryGateType::Quadrant, "quad"),
        (PrimaryGateType::Bisector, "split"),
    ] {
        state
            .add_gate(
                &editor_mapper(), 300.0, 300.0, Arc::from("FSC-A"), Arc::from("SSC-A"),
                None, None, kind, Some(name.to_string()),
            )
            .unwrap();
    }
    let composites: Vec<_> = state
        .registered_ids()
        .into_iter()
        .filter_map(|id| {
            let g = state.registered_gate(&id)?;
            (g.is_composite() && g.get_id() == id).then_some(g)
        })
        .collect();
    let (quad, split) = if composites[0].get_inner_gate_ids().len() == 4 {
        (composites[0].clone(), composites[1].clone())
    } else {
        (composites[1].clone(), composites[0].clone())
    };

    let node = state.nodes_for_gate(&quad.get_inner_gate_ids()[0])[0].clone();
    let target = state.nodes_for_gate(&split.get_inner_gate_ids()[0])[0].clone();

    let err = state.link_node_to_gate(&node, &target).unwrap_err().to_string();
    assert!(err.contains("different numbers of parts"), "{err}");
}

/// A refused link must leave the tree exactly as it was - not half applied.
#[test]
fn a_refused_composite_link_changes_nothing() {
    let (mut state, source, _) = two_quadrants();
    let single = add(&mut state, PrimaryGateType::Rectangle, "plain", None);
    let corners = source.get_inner_gate_ids();
    let before: Vec<_> = corners.iter().map(|c| state.placement_count(c)).collect();

    let node = state.nodes_for_gate(&corners[0])[0].clone();
    let _ = state.link_node_to_gate(&node, &NodeId::from(single));

    let after: Vec<_> = corners.iter().map(|c| state.placement_count(c)).collect();
    assert_eq!(before, after, "nothing may move when the link is refused");
}

#[test]
fn a_linked_composite_is_written_to_the_file() {
    let (mut state, source, target) = two_quadrants();
    let node = state.nodes_for_gate(&source.get_inner_gate_ids()[0])[0].clone();
    let target_node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();
    state.link_node_to_gate(&node, &target_node).unwrap();

    let written = export_new(&state);
    let nodes = objects(&written, &["tree", "nodes"]);

    for corner in target.get_inner_gate_ids() {
        let count = nodes
            .values()
            .filter(|n| n.get("filterContainerId").and_then(|v| v.as_str()) == Some(&*corner))
            .count();
        assert_eq!(count, 2, "corner {corner} must be written at both points");
    }
    assert_nodes_match_tree(&state, &written);
}


/// A real export's linked composite, checked against the editor's model of it.
/// Gated on `OMIQ_GATING_FILE`; skipped when unset.
#[test]
fn a_real_linked_composite_is_modelled_as_a_group() {
    let Ok(path) = std::env::var("OMIQ_GATING_FILE") else {
        return;
    };
    let source: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let (metadata, axes) = real_ctx(&source);
    let mut state = GateState::default();
    state
        .upload_gates_from_file(std::path::PathBuf::from(&path), &metadata, axes)
        .expect("imports");

    // Every composite the file carries, found through its corners.
    let composites: Vec<Arc<dyn DrawableGate>> = state
        .registered_ids()
        .into_iter()
        .filter_map(|id| {
            let g = state.registered_gate(&id)?;
            (g.is_composite() && g.get_id() == id).then_some(g)
        })
        .collect();
    assert!(!composites.is_empty(), "the file has composites");

    let mut linked_groups = 0;
    for composite in &composites {
        let corners = composite.get_inner_gate_ids();
        let counts: Vec<usize> = corners.iter().map(|c| state.placement_count(c)).collect();

        // Omiq places the whole group together, so every corner of a composite
        // is applied the same number of times.
        assert!(
            counts.windows(2).all(|w| w[0] == w[1]),
            "corners of {} are placed unevenly: {counts:?}",
            composite.get_id()
        );

        if counts[0] > 1 {
            linked_groups += 1;
            // And at each plot the group occupies, every corner is present -
            // which is what makes a group link resolvable.
            // Sorted: nodes_for_gate is in insertion order, which follows the
            // file's node iteration and is not the same across corners. What
            // matters is the set of plots, which is what a group link resolves
            // against.
            let plots_of = |gate: &GateId| {
                let mut plots: Vec<String> = state
                    .nodes_for_gate(gate)
                    .iter()
                    .map(|n| state.plot_of_for_probe(n))
                    .collect();
                plots.sort();
                plots
            };
            let plots = plots_of(&corners[0]);
            for corner in &corners {
                assert_eq!(
                    plots_of(corner),
                    plots,
                    "corner {corner} is not placed at the same points as its siblings"
                );
            }
        }
    }

    assert_eq!(
        linked_groups, 1,
        "this export has exactly one linked composite group"
    );
}

/// Axis settings and metadata for a real gating file: scatter linear, the rest
/// arcsinh, as the app configures them, and a group per file so grouped
/// composites resolve. Shared by the tests gated on `OMIQ_GATING_FILE`.
fn real_ctx(
    source: &serde_json::Value,
) -> (MetaDataFileMap, im::HashMap<Arc<str>, AxisInfo, FxBuildHasher>) {
    let mut channels = std::collections::BTreeSet::new();
    fn walk(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for key in ["f1", "f2"] {
                    if let Some(x) = m.get(key).and_then(|x| x.as_str()) {
                        out.insert(x.to_string());
                    }
                }
                m.values().for_each(|x| walk(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(source, &mut channels);

    let mut axes = im::HashMap::with_hasher(FxBuildHasher);
    for channel in &channels {
        let linear =
            channel.contains("FSC") || channel.contains("SSC") || channel.contains("Time");
        axes.insert(
            Arc::from(channel.as_str()) as Arc<str>,
            AxisInfo {
                param: Param {
                    marker: Arc::from(channel.as_str()),
                    fluoro: Arc::from(channel.as_str()),
                },
                axis_lower: if linear { 0.0 } else { -1.0 },
                axis_upper: if linear { 4_194_304.0 } else { 6.0 },
                transform: if linear {
                    TransformType::Linear
                } else {
                    TransformType::Arcsinh { cofactor: 6000.0 }
                },
            },
        );
    }

    let mut files = std::collections::BTreeSet::new();
    let mut columns = std::collections::BTreeSet::new();
    for container in source["tree"]["filterContainers"].as_object().unwrap().values() {
        if let Some(per_file) = container.get("perFileFilters").and_then(|v| v.as_object()) {
            files.extend(per_file.keys().cloned());
        }
        if let Some(md) = container.get("md").and_then(|v| v.as_str()) {
            columns.insert(md.to_string());
        }
    }
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    for (i, file) in files.iter().enumerate() {
        let mut cols: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = rustc_hash::FxHashMap::default();
        for column in &columns {
            cols.insert(
                Arc::from(column.as_str()),
                Arc::from(format!("group{}", i % 3).as_str()),
            );
        }
        metadata.insert(Arc::from(file.as_str()) as Arc<str>, cols);
    }

    (metadata, axes)
}


#[test]
fn deleting_one_instance_of_a_composite_takes_the_whole_group() {
    let (mut state, source, target) = two_quadrants();
    let corners = source.get_inner_gate_ids();
    let node = state.nodes_for_gate(&corners[0])[0].clone();
    let target_node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();

    // Link so the target composite is applied at two points, then drop one.
    state.link_node_to_gate(&node, &target_node).unwrap();
    let target_corners = target.get_inner_gate_ids();
    for corner in &target_corners {
        assert_eq!(state.placement_count(corner), 2);
    }

    let one_corner_node = state.nodes_for_gate(&target_corners[0])[0].clone();
    state.delete_placement(&one_corner_node).unwrap();

    for corner in &target_corners {
        assert_eq!(
            state.placement_count(corner),
            1,
            "every corner loses the same position: a three-cornered quadrant is not a gate"
        );
    }
}

#[test]
fn deleting_the_only_instance_of_a_composite_takes_every_corner() {
    let (mut state, source, _) = two_quadrants();
    let corners = source.get_inner_gate_ids();
    let node = state.nodes_for_gate(&corners[0])[0].clone();

    state.delete_placement(&node).unwrap();

    for corner in &corners {
        assert_eq!(state.placement_count(corner), 0, "{corner} is placed nowhere");
    }
}


// ─── Unlinking composites ─────────────────────────────────────────────────────

/// Two quadrants with one linked to the other, so there is something to unlink.
fn linked_quadrants() -> (GateState, Arc<dyn DrawableGate>, Arc<dyn DrawableGate>) {
    let (mut state, source, target) = two_quadrants();
    let node = state.nodes_for_gate(&source.get_inner_gate_ids()[0])[0].clone();
    let target_node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();
    state.link_node_to_gate(&node, &target_node).unwrap();
    (state, source, target)
}

#[test]
fn unlinking_a_composite_gives_every_corner_a_new_gate() {
    let (mut state, _, target) = linked_quadrants();
    let target_corners = target.get_inner_gate_ids();
    let node = state.nodes_for_gate(&target_corners[0])[0].clone();
    let plot = state.parent_node(&node).map(|p| p.to_string());

    let new_id = state.unlink_node(&node).unwrap();

    let copy = state.registered_gate(&new_id).expect("the copy is registered");
    assert!(copy.is_composite());
    let new_corners = copy.get_inner_gate_ids();
    assert_eq!(new_corners.len(), target_corners.len());

    // Every corner at that plot now shows a corner of the copy.
    for corner in &new_corners {
        assert_eq!(state.placement_count(corner), 1, "{corner} is placed once");
        let n = &state.nodes_for_gate(corner)[0];
        assert_eq!(state.parent_node(n).map(|p| p.to_string()), plot);
    }
    // And the original keeps its other position.
    for corner in &target_corners {
        assert_eq!(state.placement_count(corner), 1, "{corner} keeps one position");
        assert!(!state.is_linked(corner));
    }
}

#[test]
fn an_unlinked_composite_copy_resolves_from_any_of_its_corners() {
    let (mut state, _, target) = linked_quadrants();
    let node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();
    let new_id = state.unlink_node(&node).unwrap();
    let copy = state.registered_gate(&new_id).unwrap();

    // A composite is registered under its own id and each corner's, all aliased
    // to one gate - that is what lets a corner id resolve at filter time.
    for corner in copy.get_inner_gate_ids() {
        let from_corner = state
            .registered_gate(&corner)
            .expect("each corner resolves");
        assert_eq!(from_corner.get_id(), new_id);
    }
}

#[test]
fn an_unlinked_composite_keeps_its_shape() {
    let (mut state, _, target) = linked_quadrants();
    let before = state.registered_gate(&target.get_inner_gate_ids()[0]).unwrap();
    let node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();

    let new_id = state.unlink_node(&node).unwrap();
    let after = state.registered_gate(&new_id).unwrap();

    assert_eq!(after.get_params(), before.get_params());
    assert_eq!(
        after.get_inner_gate_ids().len(),
        before.get_inner_gate_ids().len()
    );
    assert_ne!(after.get_id(), before.get_id(), "it is a different gate");
}

#[test]
fn unlinking_an_unlinked_composite_is_an_error() {
    let (mut state, source, _) = two_quadrants();
    let node = state.nodes_for_gate(&source.get_inner_gate_ids()[0])[0].clone();
    assert!(state.unlink_node(&node).is_err());
}

#[test]
fn an_unlinked_composite_is_written_to_the_file() {
    let (mut state, _, target) = linked_quadrants();
    let node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();
    let new_id = state.unlink_node(&node).unwrap();
    let copy = state.registered_gate(&new_id).unwrap();

    let written = export_new(&state);
    let containers = objects(&written, &["tree", "filterContainers"]);

    for corner in copy.get_inner_gate_ids() {
        assert!(
            containers.contains_key(&*corner),
            "corner {corner} needs a container"
        );
    }
    assert_nodes_match_tree(&state, &written);
}

/// The corners of a copy must stay tied together, or Omiq sees four unrelated
/// polygons instead of a quadrant.
#[test]
fn an_unlinked_composites_corners_stay_grouped() {
    let (mut state, _, target) = linked_quadrants();
    let node = state.nodes_for_gate(&target.get_inner_gate_ids()[0])[0].clone();
    let new_id = state.unlink_node(&node).unwrap();
    let copy = state.registered_gate(&new_id).unwrap();

    let written = export_new(&state);
    let containers = objects(&written, &["tree", "filterContainers"]);

    let corners = copy.get_inner_gate_ids();
    let groups: Vec<String> = corners
        .iter()
        .map(|corner| {
            let container = containers
                .get(&**corner)
                .unwrap_or_else(|| panic!("no container written for corner {corner}"));
            let group = container
                .get("groupId")
                .and_then(|g| g.as_str())
                .unwrap_or_else(|| panic!("corner {corner} was written with no groupId"));
            group
                .rsplit_once('_')
                .unwrap_or_else(|| panic!("corner {corner} has a malformed groupId {group}"))
                .0
                .to_string()
        })
        .collect();

    // Every corner produced a group - filter_map here would have let a missing
    // one pass silently - and they all name the same one.
    assert_eq!(groups.len(), corners.len());
    let distinct: std::collections::BTreeSet<&String> = groups.iter().collect();
    assert_eq!(distinct.len(), 1, "all corners share one group: {groups:?}");
}

// ─── Ghost collection ─────────────────────────────────────────────────────────
//
// A ghost is a gate with no node that a live boolean still evaluates against.
// Omiq leaves these behind and so do we, but once nothing reaches one it is
// stranded: registered, drawn nowhere, and still written out on export. The
// AFTER fixture is the real case - a quadrant deleted while a boolean built on
// one of its corners survived.

/// The id of the surviving boolean in the AFTER fixture, and of the corner it
/// still references. Taken from the file rather than hard-coded blind: the
/// boolean is the only registered gate whose operands include a ghost.
fn the_boolean_and_its_ghost(state: &GateState) -> (GateId, GateId) {
    for id in state.registered_ids() {
        let Some(gate) = state.registered_gate(&id) else {
            continue;
        };
        let Some(inner) = gate.get_gate_ref(None) else {
            continue;
        };
        if let flow_gates::GateGeometry::Boolean { operands, .. } = &inner.geometry
            && let Some(ghost) = operands.iter().find(|o| state.is_ghost(o))
        {
            return (id.clone(), ghost.clone());
        }
    }
    panic!("the AFTER fixture should carry a boolean built on a ghost");
}

/// The operand of the AFTER fixture's boolean that is still on the tree, as
/// opposed to the corner that was already deleted.
fn the_live_operand(state: &GateState) -> GateId {
    for id in state.registered_ids() {
        let Some(gate) = state.registered_gate(&id) else {
            continue;
        };
        let Some(inner) = gate.get_gate_ref(None) else {
            continue;
        };
        if let flow_gates::GateGeometry::Boolean { operands, .. } = &inner.geometry
            && let Some(live) = operands
                .iter()
                .find(|o| state.placement_count(o) > 0 && !state.is_ghost(o))
        {
            return live.clone();
        }
    }
    panic!("the AFTER fixture should carry a boolean with a live operand");
}

#[test]
fn a_ghost_survives_while_a_boolean_still_references_it() {
    let state = import(AFTER);
    let (_boolean, ghost) = the_boolean_and_its_ghost(&state);

    assert!(state.is_ghost(&ghost));
    assert_eq!(state.placement_count(&ghost), 0, "it is on no plot");
    assert!(
        state.is_registered(&ghost),
        "but it stays resolvable, or the boolean stops evaluating"
    );
}

#[test]
fn deleting_the_last_boolean_that_referenced_a_ghost_collects_it() {
    let mut state = import(AFTER);
    let (boolean, ghost) = the_boolean_and_its_ghost(&state);

    state.remove_gate(boolean).unwrap();

    assert!(
        !state.is_registered(&ghost),
        "nothing reaches the ghost now, so it must not linger in the registry \
         or be written back on export"
    );
}

#[test]
fn a_gate_a_live_boolean_still_needs_is_not_collected() {
    // The same sweep, run when the boolean is still there: deleting something
    // unrelated must not take the ghost with it.
    let mut state = import(AFTER);
    let (_boolean, ghost) = the_boolean_and_its_ghost(&state);
    let spare = add(&mut state, PrimaryGateType::Rectangle, "unrelated", None);

    state.remove_gate(spare).unwrap();

    assert!(
        state.is_registered(&ghost),
        "its boolean is still live, so the ghost stays"
    );
}

#[test]
fn collecting_leaves_omiqs_own_orphaned_containers_alone() {
    // Containers that were already unreachable in the file Omiq wrote are kept
    // verbatim so a round trip returns the document it was given. They were
    // never gates in this editor, and the sweep is about what this session
    // stranded, not about discarding what Omiq shipped.
    let mut state = import(AFTER);
    let before = state.omiq_rebuild().ghost_containers.len();
    let (boolean, _ghost) = the_boolean_and_its_ghost(&state);

    state.remove_gate(boolean).unwrap();

    assert_eq!(
        state.omiq_rebuild().ghost_containers.len(),
        before,
        "the sweep must not reach into the verbatim containers"
    );
}

/// Add a composite and return every key it registered under. The `add` helper
/// above deliberately skips composites, and a composite's corners are named
/// after their ids rather than after the gate, so the keys are taken as the
/// difference the call made to the registry.
fn add_composite(state: &mut GateState, kind: PrimaryGateType, name: &str) -> Vec<GateId> {
    let before: std::collections::HashSet<GateId> = state.registered_ids().into_iter().collect();
    state
        .add_gate(
            &editor_mapper(),
            300.0,
            300.0,
            Arc::from("FSC-A"),
            Arc::from("SSC-A"),
            None,
            None,
            kind,
            Some(name.to_string()),
        )
        .unwrap();
    let keys: Vec<GateId> = state
        .registered_ids()
        .into_iter()
        .filter(|id| !before.contains(id))
        .collect();
    assert!(!keys.is_empty(), "{name} should be registered");
    keys
}

/// The `is_ghost` false positive that would have had the sweep collect every
/// composite. A composite has no node of its own - only its corners do - so
/// asking its own key for a placement count always answers zero.
#[test]
fn a_freshly_added_quadrant_is_not_a_ghost() {
    let mut state = import(AFTER);
    let keys = add_composite(&mut state, PrimaryGateType::Quadrant, "fresh quad");

    assert!(
        keys.len() > 1,
        "a quadrant registers under its own id and each corner's, got {keys:?}"
    );
    for id in keys {
        assert!(
            !state.is_ghost(&id),
            "a composite on the tree is not a ghost, under any of its keys - \
             {id} reported as one"
        );
    }
}

#[test]
fn a_quadrant_on_the_tree_survives_a_sweep() {
    let mut state = import(AFTER);
    add_composite(&mut state, PrimaryGateType::Quadrant, "fresh quad");
    let before = state.registered_ids().len();
    let spare = add(&mut state, PrimaryGateType::Rectangle, "unrelated", None);

    state.remove_gate(spare).unwrap();

    assert_eq!(
        state.registered_ids().len(),
        before,
        "the sweep took only the gate that was deleted"
    );
}
