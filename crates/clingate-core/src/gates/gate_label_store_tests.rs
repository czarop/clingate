//! Labels on real gates and in the store: turned with their gate, written to
//! every position of a gate at once, measured from the gate as drawn.

use std::sync::Arc;

use flow_gates::types::LabelPosition;

use super::gate_label::*;
use super::gate_store::{GateSource, GateSubStore};
use super::gate_traits::DrawableGate;
use super::gate_types::{GateRenderShape, GateStatValue, GateStats};
use super::rescale_tests::{
    OLD, X, Y, bisector, ellipse, line, mapper, quadrant, rectangle, skewed, tilted_ellipse,
    triangle,
};
use crate::omiq::metadata::MetaDataKey;

fn placed_at(x: f32, y: f32) -> LabelPosition {
    LabelPosition {
        offset_x: x,
        offset_y: y,
    }
}

fn singles() -> Vec<Arc<dyn DrawableGate>> {
    vec![
        rectangle("rect", &OLD),
        triangle("tri", &OLD),
        ellipse("ell", &OLD),
        tilted_ellipse("tilt", &OLD),
        line("line", &OLD),
    ]
}

fn composites() -> Vec<Arc<dyn DrawableGate>> {
    vec![
        quadrant("quad", &OLD),
        skewed("skew", &OLD),
        bisector("bis", &OLD),
    ]
}

fn labelled(gate: &Arc<dyn DrawableGate>, label: LabelPosition) -> Arc<dyn DrawableGate> {
    Arc::from(
        gate.with_label(Some(label))
            .expect("a single gate takes a label"),
    )
}

fn label_of(gate: &Arc<dyn DrawableGate>) -> Option<LabelPosition> {
    gate.get_gate_ref(None).unwrap().label_position.clone()
}

fn turned(gate: &Arc<dyn DrawableGate>, x: &str, y: &str) -> Arc<dyn DrawableGate> {
    Arc::from(gate.match_to_plot_axis(x, y).unwrap().expect("turned"))
}

fn axes_xy() -> PlotAxes {
    PlotAxes::of(&mapper(&OLD), Arc::from(X), Arc::from(Y))
}

fn axes_yx() -> PlotAxes {
    let a = axes_xy();
    PlotAxes {
        x: a.y,
        y: a.x,
        x_range: a.y_range,
        y_range: a.x_range,
    }
}

fn at(gate: &Arc<dyn DrawableGate>, axes: &PlotAxes) -> (f32, f32) {
    placement(&gate.label_box().unwrap(), label_of(gate).as_ref(), axes)
        .unwrap()
        .at
}

fn close(a: (f32, f32), b: (f32, f32)) {
    let tol = |v: f32| 1e-3 * v.abs().max(1.0);
    assert!(
        (a.0 - b.0).abs() < tol(a.0) && (a.1 - b.1).abs() < tol(a.1),
        "{a:?} != {b:?}"
    );
}

#[test]
fn every_single_gate_turns_its_label_with_it_and_back_exactly() {
    // The numbers from a real export, which must come through untouched.
    let from_file = placed_at(219.5, -60.65164);
    for gate in singles() {
        let gate = labelled(&gate, from_file.clone());
        let once = turned(&gate, Y, X);
        assert_eq!(
            label_of(&once),
            swap_offset(Some(from_file.clone())),
            "{}",
            gate.get_id()
        );
        let back = turned(&once, X, Y);
        assert_eq!(
            label_of(&back),
            Some(from_file.clone()),
            "{}",
            gate.get_id()
        );
    }
}

#[test]
fn a_turned_gate_shows_its_label_at_the_same_place_among_the_cells() {
    for gate in singles() {
        let gate = labelled(&gate, placed_at(40.0, -90.0));
        let upright = at(&gate, &axes_xy());
        let on_its_side = at(&turned(&gate, Y, X), &axes_yx());
        close(upright, (on_its_side.1, on_its_side.0));
    }
}

#[test]
fn an_unplaced_label_sits_just_above_its_gate() {
    for gate in singles() {
        let b = gate.label_box().unwrap();
        let p = placement(&b, None, &axes_xy()).unwrap();
        assert!(!p.placed, "{}", gate.get_id());
        let top = b.hi.1.min(axes_xy().y_range.1);
        assert!(
            (p.at.1 - top).abs() < 1e-3,
            "{}: {p:?} vs {b:?}",
            gate.get_id()
        );
    }
    // A line gate's label is over the line, not over the whole axis it gates.
    let l = line("line", &OLD);
    let p = placement(&l.label_box().unwrap(), None, &axes_xy()).unwrap();
    assert_eq!(p.at.1, 500.0);
}

#[test]
fn composites_label_their_parts_and_are_never_placed() {
    let m = mapper(&OLD);
    for gate in composites() {
        assert!(gate.label_box().is_none(), "{}", gate.get_id());
        assert!(gate.with_label(Some(placed_at(1.0, 1.0))).is_none());
        let parts = gate.get_inner_gate_ids();
        let stats = GateStats {
            count: GateStatValue::Composite(parts.iter().map(|p| (p.clone(), 10.0)).collect()),
            percent_parent: GateStatValue::Composite(
                parts.iter().map(|p| (p.clone(), 25.0)).collect(),
            ),
        };
        let texts: Vec<String> = gate
            .draw_self(false, None, &m, &Some(stats))
            .into_iter()
            .filter_map(|s| match s {
                GateRenderShape::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2 * parts.len(), "{}: {texts:?}", gate.get_id());
        for part in &parts {
            let name = gate.get_gate_ref(Some(part)).unwrap().name.clone();
            assert!(
                texts.contains(&name),
                "{}: no {name} in {texts:?}",
                gate.get_id()
            );
        }
        assert_eq!(texts.iter().filter(|t| *t == "25.00%").count(), parts.len());
        assert!(label_shape(gate.as_ref(), None, &axes_xy()).is_none());
    }
}

#[test]
fn a_single_gate_draws_no_text_of_its_own() {
    let m = mapper(&OLD);
    let stats = GateStats {
        count: GateStatValue::Single(10.0),
        percent_parent: GateStatValue::Single(12.5),
    };
    for gate in singles() {
        let text = gate
            .draw_self(true, None, &m, &Some(stats.clone()))
            .into_iter()
            .filter(|s| {
                matches!(
                    s,
                    GateRenderShape::Text { .. } | GateRenderShape::Label { .. }
                )
            })
            .count();
        assert_eq!(text, 0, "{}", gate.get_id());
    }
}

#[test]
fn the_label_is_the_drawn_gates_name_and_the_shown_positions_percent() {
    let drawn = labelled(&rectangle("rect", &OLD), placed_at(10.0, -20.0));
    let stats = GateStats {
        count: GateStatValue::Single(10.0),
        percent_parent: GateStatValue::Single(61.714),
    };
    let Some(GateRenderShape::Label {
        at: where_,
        lines,
        valign,
        movable,
        ..
    }) = label_shape(drawn.as_ref(), Some(&stats), &axes_xy())
    else {
        panic!("a single gate has a label");
    };
    assert_eq!(lines, [drawn.get_name().to_string(), "61.71%".to_string()]);
    assert_eq!(valign, VAlign::Middle);
    assert_eq!(movable, Some(Arc::from("rect")));
    close(where_, at(&drawn, &axes_xy()));
}

fn group() -> MetaDataKey {
    MetaDataKey {
        parameter: Arc::from("SampleID"),
        group: Arc::from("donor"),
    }
}

/// A store with a rectangle drawn, positioned for a group, and positioned for
/// one sample on the other axes; the sample's is the newer.
fn store() -> GateSubStore {
    let drawn = rectangle("rect", &OLD);
    let mut store = GateSubStore::default();
    store.insert_for_source(&[drawn.get_id()], &drawn, &GateSource::Global);
    store.set_group_position((drawn.get_id(), group()), rectangle("rect", &OLD));
    store.set_sample_position(
        (drawn.get_id(), Arc::from("s1")),
        turned(&rectangle("rect", &OLD), Y, X),
    );
    store
}

fn newest(store: &GateSubStore) -> GateSource {
    let g = group();
    store
        .position_for(
            &Arc::from("rect"),
            &Arc::from("s1"),
            [(&g.parameter, &g.group)],
        )
        .unwrap()
        .0
}

#[test]
fn a_label_is_written_to_every_position_of_the_gate_each_its_own_way_round() {
    let mut store = store();
    let before = newest(&store);
    let o = placed_at(33.0, -44.0);
    store
        .set_label(&Arc::from("rect"), Some(o.clone()))
        .unwrap();

    let id: Arc<str> = Arc::from("rect");
    assert_eq!(
        label_of(&store.primary_and_subgate_registry[&id]),
        Some(o.clone())
    );
    assert_eq!(
        label_of(&store.group_position_overrides[&(id.clone(), group())]),
        Some(o.clone())
    );
    assert_eq!(
        label_of(&store.sample_position_overrides[&(id.clone(), Arc::from("s1"))]),
        swap_offset(Some(o)),
        "the sample's position is held the other way round"
    );
    assert_eq!(newest(&store), before, "a label is not a new position");
}

#[test]
fn a_composites_label_is_refused_and_nothing_changes() {
    let quad = quadrant("quad", &OLD);
    let mut store = GateSubStore::default();
    let ids = GateSubStore::ids_for(&quad, &quad.get_id());
    store.insert_for_source(&ids, &quad, &GateSource::Global);
    assert!(
        store
            .set_label(&quad.get_id(), Some(placed_at(1.0, 1.0)))
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &store.primary_and_subgate_registry[&quad.get_id()],
        &quad
    ));
    assert!(
        store
            .move_label(&quad.get_id(), (1.0, 1.0), &axes_xy())
            .is_err()
    );
}

#[test]
fn a_label_moved_on_either_plot_lands_where_it_was_put() {
    for axes in [axes_xy(), axes_yx()] {
        let mut store = store();
        let id: Arc<str> = Arc::from("rect");
        let drawn = store.primary_and_subgate_registry[&id].clone();
        let (px, py) = if axes.x.as_ref() == X {
            (
                axes.x_range.0 + 0.3 * (axes.x_range.1 - axes.x_range.0),
                820.0,
            )
        } else {
            (
                820.0,
                axes.y_range.0 + 0.3 * (axes.y_range.1 - axes.y_range.0),
            )
        };
        store.move_label(&id, (px, py), &axes).unwrap();
        let now = store.primary_and_subgate_registry[&id].clone();
        assert_eq!(
            now.get_params(),
            drawn.get_params(),
            "the gate is not turned"
        );
        close(at(&now, &axes), (px, py));
    }
}

#[test]
fn a_placed_label_goes_with_the_drawn_gate_when_it_is_dragged() {
    use super::gate_drag::GateDragData;
    for gate in singles() {
        let gate = labelled(&gate, placed_at(25.0, -50.0));
        let before = at(&gate, &axes_xy());
        // Dragged from (1, 400) to (1.5, 500): half a unit right, 100 up.
        let drag = GateDragData::new(gate.get_id(), (1.0, 400.0), (1.5, 500.0));
        let Some(dragged) = gate.replace_points(drag).unwrap() else {
            continue;
        };
        let dragged: Arc<dyn DrawableGate> = Arc::from(dragged);
        assert_eq!(label_of(&dragged), label_of(&gate), "{}", gate.get_id());
        // As far as the gate moved: the whole drag for a shape, along its
        // range only for a line gate, whose line stays at its height.
        let centre = |g: &Arc<dyn DrawableGate>| {
            let b = g.label_box().unwrap();
            ((b.lo.0 + b.hi.0) / 2.0, (b.lo.1 + b.hi.1) / 2.0)
        };
        let (c0, c1) = (centre(&gate), centre(&dragged));
        assert!((c1.0 - c0.0 - 0.5).abs() < 1e-3, "{}", gate.get_id());
        close(
            at(&dragged, &axes_xy()),
            (before.0 + c1.0 - c0.0, before.1 + c1.1 - c0.1),
        );
    }
}
