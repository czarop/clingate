//! Replacing the scaling file under gates that are already drawn.
//!
//! Four modules meet here. The scaling file is read by `axis_store`
//! (`read_axis_configs`) and compared with what is loaded (`scaling_diff`);
//! the Workspace tab's `carry_to_scaling` then drives the gate store's
//! `rescale_gates` and `set_current_axis_limits` for every channel that
//! changed; and each gate type does the carrying its own way. The rescale
//! tests cover the last of these on a bare sub-store. This runs the whole of
//! it - files on disk, real stores in a headless runtime - and asks the one
//! thing a person relies on: do the gates hold the same cells afterwards.

mod common;

use clingate::gate_editor::AxisInfo;
use clingate::gate_editor::gates::GateState;
use clingate::gate_editor::gates::gate_composite::quadrant_gate::QuadrantGate;
use clingate::gate_editor::gates::gate_single::ellipse_gate::EllipseGate;
use clingate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use clingate::gate_editor::gates::gate_store::GateSource;
use clingate::gate_editor::gates::gate_traits::DrawableGate;
use clingate::gate_editor::plots::axis_store::{
    AxisStore, PlotMapper, ScalingInfoSource, read_axis_configs,
};
use clingate::gate_editor::workspace_window::{ScalingCarried, carry_to_scaling};
use common::*;
use dioxus::prelude::*;
use dioxus::stores::use_store_sync;
use flow_fcs::{TransformType, Transformable};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

const X: &str = "BV421-A";
const Y: &str = "SSC-A";

fn scaling(name: &str, cofactor: i64, rows_for_y: bool) -> Vec<AxisInfo> {
    scaling_over(name, cofactor, rows_for_y, (-500, 200_000))
}

fn scaling_over(name: &str, cofactor: i64, rows_for_y: bool, range: (i64, i64)) -> Vec<AxisInfo> {
    let path = scratch(name).join("scaling.csv");
    let mut rows = vec![(X, "CD3", "Arcsinh", cofactor, range.0, range.1)];
    if rows_for_y {
        rows.push((Y, "", "None (linear)", 1, 0, 262_144));
    }
    write_scaling(&path, &rows);
    read_axis_configs(path, ScalingInfoSource::Omiq).expect("the scaling reads")
}

fn transform_of(configs: &[AxisInfo], channel: &str) -> TransformType {
    configs
        .iter()
        .find(|a| &*a.param.fluoro == channel)
        .unwrap()
        .transform
        .clone()
}

fn inner(id: &str, geometry: flow_gates::GateGeometry) -> flow_gates::Gate {
    flow_gates::Gate {
        id: Arc::from(id),
        name: id.to_string(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    }
}

/// Gates drawn under `t`, at fixed raw positions on the marker.
fn gates(t: &TransformType, axis: &AxisInfo) -> Vec<Arc<dyn DrawableGate>> {
    let shown = |raw: f32| t.transform(&raw);
    let rectangle = flow_gates::create_rectangle_geometry(
        vec![
            (shown(800.0), 20_000.0),
            (shown(20_000.0), 20_000.0),
            (shown(20_000.0), 70_000.0),
            (shown(800.0), 70_000.0),
        ],
        X,
        Y,
    )
    .unwrap();
    let ellipse = clingate::omiq::deserialise::create_omiq_ellipse_geometry(
        (f64::from(shown(1_000.0)), 50_000.0),
        (f64::from(shown(30_000.0)), 50_000.0),
        (f64::from(shown(6_000.0)), 65_000.0),
        X,
        Y,
    )
    .unwrap();
    let mapper = PlotMapper::new(
        600.0,
        600.0,
        axis.axis_lower..=axis.axis_upper,
        0.0..=262_144.0,
        axis.axis_lower..=axis.axis_upper,
        0.0..=262_144.0,
        t.clone(),
        TransformType::Linear,
    );
    let (px, _) = mapper.data_to_pixel(shown(3_000.0), 0.0, None, None);
    vec![
        Arc::new(RectangleGate::try_new(inner("rect", rectangle), true).unwrap()),
        Arc::new(EllipseGate::try_new(inner("ellipse", ellipse), true).unwrap()),
        Arc::new(
            QuadrantGate::try_new_from_raw_coord(
                &mapper,
                Arc::from("quad"),
                "quad".into(),
                (px, 300.0),
                Arc::from(X),
                Arc::from(Y),
            )
            .unwrap(),
        ),
    ]
}

/// Raw events across both axes.
fn events() -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut x = 37.0f32;
    while x < 150_000.0 {
        let mut y = 1_370.0f32;
        while y < 250_000.0 {
            out.push((x, y));
            y += 4_130.0;
        }
        x = x * 1.061 + 7.3;
    }
    out
}

/// Which piece of which gate each raw event lands in, under `t`.
fn membership(state: &GateState, t: &TransformType) -> Vec<Vec<Option<Arc<str>>>> {
    let metadata = Default::default();
    ["rect", "ellipse", "quad"]
        .iter()
        .map(|id| {
            let gate = state
                .gate_for_file(&Arc::from(*id), &Arc::from("f1"), &metadata)
                .unwrap_or_else(|| panic!("{id} resolves"));
            let pieces: Vec<Option<Arc<str>>> = if gate.is_composite() {
                gate.get_inner_gate_ids().into_iter().map(Some).collect()
            } else {
                vec![None]
            };
            events()
                .iter()
                .map(|(raw, y)| {
                    let x = t.transform(raw);
                    pieces
                        .iter()
                        .find(|p| {
                            gate.get_gate_ref(p.as_deref()).is_some_and(|g| {
                                g.geometry.contains_point(x, *y, X, Y).unwrap_or(false)
                            })
                        })
                        .map(|p| p.clone().unwrap_or_else(|| Arc::from(*id)))
                })
                .collect()
        })
        .collect()
}

/// Load `before`, draw the gates, replace the scaling with `after` the way the
/// Workspace tab does, and hand back what happened and the gates afterwards.
fn replace(
    before: Vec<AxisInfo>,
    after: Vec<AxisInfo>,
) -> (GateState, ScalingCarried, AxisStore, GateState) {
    let t = transform_of(&before, X);
    let axis = before
        .iter()
        .find(|a| &*a.param.fluoro == X)
        .unwrap()
        .clone();
    let mut drawn = GateState::default();
    for gate in gates(&t, &axis) {
        let mut ids = vec![gate.get_id()];
        if gate.is_composite() {
            ids.extend(gate.get_inner_gate_ids());
        }
        drawn.place_gate(&ids, &gate, &GateSource::Global);
    }

    type Out = Rc<RefCell<Option<(ScalingCarried, AxisStore, GateState)>>>;
    let out: Out = Rc::new(RefCell::new(None));

    #[derive(Clone)]
    struct Setup {
        drawn: GateState,
        before: Vec<AxisInfo>,
        after: Vec<AxisInfo>,
        out: Out,
    }
    impl PartialEq for Setup {
        fn eq(&self, _: &Self) -> bool {
            true
        }
    }

    let mut dom = VirtualDom::new_with_props(
        |setup: Setup| {
            let gates = use_store_sync({
                let drawn = setup.drawn.clone();
                move || drawn
            });
            let axes = use_store_sync({
                let before = setup.before.clone();
                move || {
                    let mut store = AxisStore::default();
                    store.apply_axis_configs(before);
                    store
                }
            });
            if setup.out.borrow().is_none() {
                let carried = carry_to_scaling(axes, gates, setup.after.clone());
                *setup.out.borrow_mut() =
                    Some((carried, axes.peek().clone(), gates.peek().clone()));
            }
            rsx! {}
        },
        Setup {
            drawn: drawn.clone(),
            before,
            after,
            out: out.clone(),
        },
    );
    dom.rebuild_in_place();
    let (carried, axes, after) = out.borrow_mut().take().expect("the component ran");
    (drawn, carried, axes, after)
}

fn agreement(a: &[Option<Arc<str>>], b: &[Option<Arc<str>>]) -> f64 {
    a.iter().zip(b).filter(|(x, y)| x == y).count() as f64 / a.len() as f64
}

#[test]
fn a_new_cofactor_leaves_every_gate_holding_the_same_cells() {
    let (v1, v2) = (scaling("cof-v1", 150, true), scaling("cof-v2", 1000, true));
    let (t1, t2) = (transform_of(&v1, X), transform_of(&v2, X));
    let (before, carried, _, after) = replace(v1, v2);

    assert!(carried.problems.is_empty(), "{:?}", carried.problems);
    let changed: Vec<&str> = carried.diff.changed.iter().map(|c| &*c.channel).collect();
    assert_eq!(changed, vec![X], "only the marker changed");

    let (was, now) = (membership(&before, &t1), membership(&after, &t2));
    // Guard against a fixture that cannot tell carrying from doing nothing:
    // left where they were, the gates would hold different cells.
    let uncarried = membership(&before, &t2);
    assert_ne!(uncarried[0], was[0], "an uncarried rectangle must differ");
    assert_ne!(uncarried[2], was[2], "an uncarried quadrant must differ");
    // The rectangle and the quadrant split along the axes, so nothing may
    // change sides; an ellipse is refitted, and holds nearly the same cells.
    assert_eq!(was[0], now[0], "the rectangle");
    assert_eq!(was[2], now[2], "the quadrant");
    let ellipse = agreement(&was[1], &now[1]);
    assert!(ellipse > 0.98, "the ellipse kept {:.1}%", ellipse * 100.0);
    assert!(
        was.iter().all(|g| g.iter().any(Option::is_some)),
        "every gate holds something"
    );
}

#[test]
fn the_same_scaling_again_changes_nothing() {
    let (v1, again) = (scaling("same-v1", 150, true), scaling("same-v2", 150, true));
    let t = transform_of(&v1, X);
    let (before, carried, _, after) = replace(v1, again);

    assert!(carried.diff.changed.is_empty() && carried.diff.dropped.is_empty());
    assert_eq!(membership(&before, &t), membership(&after, &t));
}

#[test]
fn a_channel_the_new_file_drops_is_reported_and_its_gates_left_alone() {
    // The new scaling has no SSC-A. The gates are on it as their y axis;
    // there is no new transform to carry them to, so they must not move -
    // and the replace must say that it happened.
    let (v1, v2) = (
        scaling("drop-v1", 150, true),
        scaling("drop-v2", 150, false),
    );
    let t = transform_of(&v1, X);
    let (before, carried, axes, after) = replace(v1, v2);

    let dropped: Vec<&str> = carried.diff.dropped.iter().map(|c| &**c).collect();
    assert_eq!(dropped, vec![Y]);
    assert!(
        axes.settings.get(Y).is_none(),
        "the scaling is replaced, not merged"
    );
    assert_eq!(membership(&before, &t), membership(&after, &t));
}

/// BUG (docs/test-audit.md, B-AX-3): nothing checks a scaling file's range
/// is the right way round. Replacing the scaling with one whose Min exceeds
/// its Max relimits every quadrant on the channel, and the quadrant's
/// `f32::clamp(lower + buffer, upper - buffer)` panics - loading a file
/// crashes the app. It should be refused, or the channel reported, with the
/// gates left as they were.
#[test]
#[ignore = "known bug B-AX-3: a scaling file with Min above Max crashes the scaling replace"]
fn a_scaling_file_with_its_range_the_wrong_way_round_is_refused_not_fatal() {
    let v1 = scaling("inverted-v1", 150, true);
    let inverted = scaling_over("inverted-v2", 150, true, (200_000, -500));
    let outcome = std::panic::catch_unwind(|| replace(v1, inverted));
    assert!(outcome.is_ok(), "replacing the scaling panicked");
}

/// BUG (docs/test-audit.md, B-AX-3): nor is the cofactor checked. The
/// editor's box refuses anything below 1, but a scaling file's cofactor goes
/// straight into the transform: 0 gives infinite axis bounds and a NaN in the
/// quadrant's clamp, a negative one an axis the wrong way round. Either
/// crashes the app on loading the file.
#[test]
#[ignore = "known bug B-AX-3: a scaling file with a cofactor of 0 or less crashes the scaling replace"]
fn a_scaling_file_with_a_cofactor_below_one_is_refused_not_fatal() {
    for cofactor in [0i64, -150] {
        let v1 = scaling(&format!("cofactor-v1-{cofactor}"), 150, true);
        let bad = scaling(&format!("cofactor-{cofactor}"), cofactor, true);
        let outcome = std::panic::catch_unwind(|| replace(v1, bad));
        assert!(
            outcome.is_ok(),
            "a cofactor of {cofactor} panicked the replace"
        );
    }
}
