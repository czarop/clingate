//! Tests for the Omiq interchange layer.
//!
//! This is the highest-value surface in the codebase to pin down: everything the
//! editor and the eventual autogater work on arrives through here, and the
//! export path will have to reproduce it exactly in reverse.
//!
//! NOTE: written without a compiler - the flow-fcs/flow-gates git dependencies
//! were not reachable in the environment these were authored in, so this module
//! has never been built or run. Treat a failure here as suspect-the-test first.
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
