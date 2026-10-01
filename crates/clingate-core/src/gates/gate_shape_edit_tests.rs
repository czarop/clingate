//! The editor's outline edits, each outline worked by hand.

use std::sync::Arc;

use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::gates::GateState;
use crate::gates::gate_contact::{Point, outline};
use crate::gates::gate_shape_edit::{as_polygon, level_side, with_point_added, without_point};
use crate::gates::gate_single::polygon_gate::PolygonGate;
use crate::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gates::gate_store::{FileId, GateSource};
use crate::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::{MetaDataFileMap, MetaDataKey};

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

fn flow_gate(geometry: flow_gates::GateGeometry) -> flow_gates::Gate {
    flow_gates::Gate {
        id: Arc::from("g"),
        name: "G".into(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    }
}

fn polygon(points: &[(f32, f32)]) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_polygon_geometry(points.to_vec(), X, Y).unwrap();
    Arc::new(PolygonGate::try_new(flow_gate(geometry), true).unwrap())
}

fn rectangle((x0, y0): (f32, f32), (x1, y1): (f32, f32)) -> Arc<dyn DrawableGate> {
    let geometry =
        flow_gates::create_rectangle_geometry(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)], X, Y)
            .unwrap();
    Arc::new(RectangleGate::try_new(flow_gate(geometry), true).unwrap())
}

fn id() -> Arc<str> {
    Arc::from("g")
}

fn points(gate: &Arc<dyn DrawableGate>) -> Vec<Point> {
    outline(gate, &id(), X, Y).unwrap()
}

fn is_polygon(gate: &Arc<dyn DrawableGate>) -> bool {
    gate.as_any().downcast_ref::<PolygonGate>().is_some()
}

fn triangle() -> Arc<dyn DrawableGate> {
    polygon(&[(0.0, 0.0), (100.0, 10.0), (50.0, 100.0)])
}

#[test]
fn a_side_made_horizontal_takes_the_middle_height_of_its_ends() {
    let level = level_side(&triangle(), &id(), 0, Y).unwrap();
    assert_eq!(points(&level), [(0.0, 5.0), (100.0, 5.0), (50.0, 100.0)]);
}

#[test]
fn a_side_made_vertical_takes_the_middle_of_its_ends_across() {
    let level = level_side(&triangle(), &id(), 1, X).unwrap();
    assert_eq!(points(&level), [(0.0, 0.0), (75.0, 10.0), (75.0, 100.0)]);
}

#[test]
fn the_last_side_runs_back_to_the_first_point() {
    let level = level_side(&triangle(), &id(), 2, Y).unwrap();
    assert_eq!(points(&level), [(0.0, 50.0), (100.0, 10.0), (50.0, 50.0)]);
}

#[test]
fn a_side_the_polygon_does_not_have_is_refused() {
    let refused = level_side(&triangle(), &id(), 3, Y).err().expect("refused");
    assert!(refused.to_string().contains("no side 3"), "{refused}");
}

/// The click (40, 7) is 7 above the side from (0, 0) to (100, 0): the point
/// goes on the side, below it, between the side's two ends.
#[test]
fn a_point_added_goes_on_the_side_nearest_the_click() {
    let square = polygon(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
    let added = with_point_added(&square, &id(), 0, (X, Y), (40.0, 7.0)).unwrap();
    assert_eq!(
        points(&added),
        [
            (0.0, 0.0),
            (40.0, 0.0),
            (100.0, 0.0),
            (100.0, 100.0),
            (0.0, 100.0)
        ]
    );
}

#[test]
fn a_point_added_past_the_end_of_a_side_goes_at_its_end() {
    let added = with_point_added(&triangle(), &id(), 2, (X, Y), (-30.0, -30.0)).unwrap();
    assert_eq!(
        points(&added),
        [(0.0, 0.0), (100.0, 10.0), (50.0, 100.0), (0.0, 0.0)]
    );
}

#[test]
fn the_points_are_numbered_afresh_after_an_edit() {
    let added = with_point_added(&triangle(), &id(), 0, (X, Y), (50.0, 0.0)).unwrap();
    let shape = added.get_gate_ref(None).unwrap();
    let flow_gates::GateGeometry::Polygon { nodes, .. } = &shape.geometry else {
        panic!("a polygon");
    };
    let ids: Vec<&str> = nodes.iter().map(|node| &*node.id).collect();
    assert_eq!(
        ids,
        [
            "polygon_node_0",
            "polygon_node_1",
            "polygon_node_2",
            "polygon_node_3"
        ]
    );
}

#[test]
fn a_point_deleted_leaves_the_others_in_order() {
    let square = polygon(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
    let fewer = without_point(&square, &id(), 1).unwrap();
    assert_eq!(points(&fewer), [(0.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
}

#[test]
fn a_triangle_keeps_its_three_points() {
    let refused = without_point(&triangle(), &id(), 0).err().expect("refused");
    assert!(refused.to_string().contains("at least 3"), "{refused}");
}

#[test]
fn only_a_polygon_has_its_points_edited() {
    let rect = rectangle((0.0, 0.0), (10.0, 10.0));
    assert!(level_side(&rect, &id(), 0, Y).is_err());
    assert!(without_point(&rect, &id(), 0).is_err());
}

#[test]
fn a_rectangle_made_a_polygon_runs_round_its_four_corners() {
    let made = as_polygon(&rectangle((10.0, 20.0), (110.0, 220.0)), &id()).unwrap();
    assert!(is_polygon(&made));
    assert_eq!(
        points(&made),
        [(10.0, 20.0), (110.0, 20.0), (110.0, 220.0), (10.0, 220.0)]
    );
    assert_eq!((&*made.get_id(), &*made.get_name()), ("g", "G"));
}

#[test]
fn a_rectangle_open_to_the_end_of_an_axis_is_not_made_a_polygon() {
    let open = rectangle((10.0, 20.0), (1e16, 220.0));
    let refused = as_polygon(&open, &id()).err().expect("refused");
    assert!(refused.to_string().contains("open"), "{refused}");
}

#[test]
fn only_a_rectangle_is_made_a_polygon() {
    assert!(as_polygon(&triangle(), &id()).is_err());
}

fn file(name: &str) -> FileId {
    Arc::from(name)
}

/// a1 and a2 of DONOR-A, b1 of DONOR-B.
fn metadata() -> MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (name, donor) in [("a1", "DONOR-A"), ("a2", "DONOR-A"), ("b1", "DONOR-B")] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(donor));
        map.insert(file(name), columns);
    }
    map
}

fn donor_a() -> GateSource {
    GateSource::Group((
        id(),
        MetaDataKey {
            parameter: Arc::from("SampleID"),
            group: Arc::from("DONOR-A"),
        },
    ))
}

/// Drawn at x 0, a1's own at 50 then DONOR-A's at 100, so a1 shows its
/// group's, the newer; b1 has none of its own and shows the drawn one.
#[test]
fn a_rectangle_is_made_a_polygon_at_every_position_and_each_shows_the_same() {
    let mut state = GateState::default();
    let box_at = |x0: f32| rectangle((x0, 0.0), (x0 + 10.0, 10.0));
    state.place_gate(&[id()], &box_at(0.0), &GateSource::Global);
    state.place_new_gate(None, id()).unwrap();
    state.place_gate(
        &[id()],
        &box_at(50.0),
        &GateSource::Sample((id(), file("a1"))),
    );
    state.place_gate(&[id()], &box_at(100.0), &donor_a());

    state.convert_to_polygon(&id()).unwrap();

    let shown = |name: &str| {
        state
            .gate_for_file(&id(), &file(name), &metadata())
            .unwrap()
    };
    for (name, x0) in [("a1", 100.0), ("a2", 100.0), ("b1", 0.0)] {
        let gate = shown(name);
        assert!(is_polygon(&gate), "{name}");
        assert_eq!(
            points(&gate),
            [(x0, 0.0), (x0 + 10.0, 0.0), (x0 + 10.0, 10.0), (x0, 10.0)],
            "{name}"
        );
    }
    state.retain_group_positions(|_| false);
    let own = state
        .gate_for_file(&id(), &file("a1"), &metadata())
        .unwrap();
    assert!(
        is_polygon(&own),
        "a1's own position, hidden behind its group's"
    );
    assert_eq!(points(&own)[0], (50.0, 0.0));
}

/// A position that cannot be converted leaves every position as it was.
#[test]
fn a_rectangle_with_one_position_open_is_left_a_rectangle_everywhere() {
    let mut state = GateState::default();
    state.place_gate(
        &[id()],
        &rectangle((0.0, 0.0), (10.0, 10.0)),
        &GateSource::Global,
    );
    state.place_new_gate(None, id()).unwrap();
    state.place_gate(
        &[id()],
        &rectangle((0.0, 0.0), (1e16, 10.0)),
        &GateSource::Sample((id(), file("a1"))),
    );

    assert!(state.convert_to_polygon(&id()).is_err());

    for name in ["a1", "b1"] {
        let gate = state
            .gate_for_file(&id(), &file(name), &metadata())
            .unwrap();
        assert!(!is_polygon(&gate), "{name}");
    }
}
