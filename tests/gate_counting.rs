//! A gate's events, counted the two ways the program counts them.
//!
//! The plot a gate's children are drawn on is filtered through a polars mask
//! (`gate_filtering::filter_events_to_mask`); the percentage printed on the
//! gate is counted through an R-tree (`gate_stats`, over `flow_gates`'s
//! `EventIndex`). They are separate implementations of one question, written
//! at different times, and the autogater's thresholds are documented as
//! counting "the way the filter counts it". If the two disagree, the number
//! on the gate describes a different population from the one below it.
//!
//! So every gate type is counted both ways over the same events - including
//! events placed exactly on the gate's edges, where the conventions differ
//! if they are going to.

use clingate::gate_editor::gates::GateState;
use clingate::gate_editor::gates::gate_composite::bisector_gate::BisectorGate;
use clingate::gate_editor::gates::gate_composite::quadrant_gate::QuadrantGate;
use clingate::gate_editor::gates::gate_composite::skewed_quadrant_gate::SkewedQuadrantGate;
use clingate::gate_editor::gates::gate_filtering::filter_events_to_mask;
use clingate::gate_editor::gates::gate_single::ellipse_gate::EllipseGate;
use clingate::gate_editor::gates::gate_single::polygon_gate::PolygonGate;
use clingate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use clingate::gate_editor::gates::gate_stats::get_percent_and_counts_gate;
use clingate::gate_editor::gates::gate_store::GateSource;
use clingate::gate_editor::gates::gate_traits::DrawableGate;
use clingate::gate_editor::plots::axis_store::PlotMapper;
use clingate::gate_editor::plots::plot_store::EventIndexMapped;
use flow_fcs::TransformType;
use flow_gates::{EventIndex, create_polygon_geometry, create_rectangle_geometry};
use polars::prelude::*;
use std::sync::Arc;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

/// A spread of events over 0..1000, plus some sitting exactly on the edges
/// and corners of the rectangle below.
fn events() -> (Vec<f32>, Vec<f32>) {
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    let mut x = 3.7f32;
    while x < 1000.0 {
        let mut y = 5.3f32;
        while y < 1000.0 {
            xs.push(x);
            ys.push(y);
            y += 23.9;
        }
        x += 19.3;
    }
    for (ex, ey) in [
        (200.0, 350.0),
        (600.0, 350.0),
        (400.0, 200.0),
        (400.0, 500.0),
        (200.0, 200.0),
        (600.0, 500.0),
    ] {
        xs.push(ex);
        ys.push(ey);
    }
    (xs, ys)
}

fn frame((xs, ys): &(Vec<f32>, Vec<f32>)) -> DataFrame {
    df![X => xs.clone(), Y => ys.clone()].unwrap()
}

fn index((xs, ys): &(Vec<f32>, Vec<f32>)) -> EventIndexMapped {
    EventIndexMapped {
        event_index: Arc::new(EventIndex::build(xs, ys).unwrap()),
        index_map: Arc::new((0..xs.len()).collect()),
    }
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

fn mapper() -> PlotMapper {
    PlotMapper::new(
        600.0,
        600.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    )
}

fn gates() -> Vec<Arc<dyn DrawableGate>> {
    let rectangle = create_rectangle_geometry(
        vec![
            (200.0, 200.0),
            (600.0, 200.0),
            (600.0, 500.0),
            (200.0, 500.0),
        ],
        X,
        Y,
    )
    .unwrap();
    let polygon = create_polygon_geometry(
        vec![
            (100.0, 100.0),
            (800.0, 150.0),
            (500.0, 900.0),
            (150.0, 600.0),
        ],
        X,
        Y,
    )
    .unwrap();
    let ellipse = clingate::omiq::deserialise::create_omiq_ellipse_geometry(
        (300.0, 400.0),
        (700.0, 600.0),
        (450.0, 600.0),
        X,
        Y,
    )
    .unwrap();
    vec![
        Arc::new(RectangleGate::try_new(inner("rect", rectangle), true).unwrap()),
        Arc::new(PolygonGate::try_new(inner("poly", polygon), true).unwrap()),
        Arc::new(EllipseGate::try_new(inner("ellipse", ellipse), true).unwrap()),
        Arc::new(
            QuadrantGate::try_new_from_raw_coord(
                &mapper(),
                Arc::from("quad"),
                "quad".into(),
                (250.0, 330.0),
                Arc::from(X),
                Arc::from(Y),
            )
            .unwrap(),
        ),
        Arc::new(
            BisectorGate::try_new(
                &mapper(),
                Arc::from("split"),
                "split".into(),
                (310.0, 300.0),
                Arc::from(X),
                Arc::from(Y),
            )
            .unwrap(),
        ),
        Arc::new(
            SkewedQuadrantGate::try_new_from_raw_coord(
                &mapper(),
                Arc::from("skew"),
                "skew".into(),
                (280.0, 280.0),
                Arc::from(X),
                Arc::from(Y),
            )
            .unwrap(),
        ),
    ]
}

/// Every gate registered globally, and a resolver for one file.
fn state_with(gates: &[Arc<dyn DrawableGate>]) -> GateState {
    let mut state = GateState::default();
    for gate in gates {
        let mut ids = vec![gate.get_id()];
        if gate.is_composite() {
            ids.extend(gate.get_inner_gate_ids());
        }
        state.place_gate(&ids, gate, &GateSource::Global);
    }
    state
}

/// (id, events by mask, events by index) for every piece of every gate.
fn both_counts() -> Vec<(Arc<str>, usize, usize)> {
    let data = events();
    let df = frame(&data);
    let idx = index(&data);
    let gates = gates();
    let state = state_with(&gates);
    let resolver = state.get_current_sample(Arc::from("f1"), &Default::default());

    let mut out = Vec::new();
    for gate in &gates {
        let stats = get_percent_and_counts_gate(gate.clone(), &idx, data.0.len() as f32).unwrap();
        let pieces = if gate.is_composite() {
            gate.get_inner_gate_ids()
        } else {
            vec![gate.get_id()]
        };
        for piece in pieces {
            let mask = filter_events_to_mask(&df, piece.clone(), &resolver)
                .unwrap_or_else(|e| panic!("{piece} cannot be filtered: {e}"));
            let by_mask = mask.sum().unwrap_or(0) as usize;
            let by_index = stats.get_count_for_id(piece.clone()).unwrap() as usize;
            out.push((piece, by_mask, by_index));
        }
    }
    out
}

#[test]
fn the_filter_and_the_on_screen_count_agree_away_from_the_edges() {
    // Everything but the rectangle, whose edges have events placed on them.
    for (piece, by_mask, by_index) in both_counts() {
        if &*piece == "rect" {
            continue;
        }
        assert!(
            by_mask > 0 || by_index > 0,
            "{piece}: the fixture should hold something"
        );
        assert_eq!(
            by_mask, by_index,
            "{piece}: the filter and the count disagree"
        );
    }
}

/// Was B-CNT-1: the filter admitted an event only strictly inside a
/// rectangle (`gt(min) & lt(max)`) while the index counted one on the edge -
/// 240 against 246 here, exactly the six events placed on the edges and
/// corners - so the percentage on the gate counted events the population drawn
/// under it did not hold. Linear scatter values are often whole numbers and
/// gate edges often round ones, so it was not only a theoretical case.
#[test]
fn an_event_on_a_rectangle_s_edge_is_counted_the_same_way_by_both() {
    let (_, by_mask, by_index) = both_counts()
        .into_iter()
        .find(|(p, _, _)| &**p == "rect")
        .unwrap();
    assert_eq!(
        by_mask, by_index,
        "six events sit on the rectangle's edges and corners; the filter and the count \
         must agree about them"
    );
}

#[test]
fn the_quarters_of_a_quadrant_account_for_every_event_once() {
    let counts = both_counts();
    let total: usize = counts
        .iter()
        .filter(|(p, _, _)| p.starts_with("quad"))
        .map(|(_, by_mask, _)| by_mask)
        .sum();
    assert_eq!(total, events().0.len());
}

/// Was pinned as B-STAT-1 and kept on purpose: over an empty parent the
/// percentage is 0 / 0, NaN, and the gate shows "NaN%". That says something
/// 0% would not - there were no events to gate at all, rather than events of
/// which the gate held none - so it stays. Both cases are pinned.
#[test]
fn a_gate_over_an_empty_parent_shows_nan_and_an_empty_gate_shows_zero() {
    let empty = (Vec::<f32>::new(), Vec::<f32>::new());
    let idx = EventIndexMapped {
        event_index: Arc::new(EventIndex::build(&empty.0, &empty.1).expect("an empty index")),
        index_map: Arc::new(Vec::new()),
    };
    let stats = get_percent_and_counts_gate(gates()[0].clone(), &idx, 0.0).unwrap();
    let percent = stats.get_percent_for_id(Arc::from("rect")).unwrap();
    assert!(percent.is_nan(), "an empty parent shows {percent}%");

    // Events, none of them in the rectangle.
    let far = (vec![900.0f32, 950.0], vec![900.0f32, 950.0]);
    let idx = index(&far);
    let stats = get_percent_and_counts_gate(gates()[0].clone(), &idx, 2.0).unwrap();
    assert_eq!(stats.get_percent_for_id(Arc::from("rect")), Some(0.0));
}

// ── event for event, on every kind of edge ───────────────────────────────

/// Every event each piece of `gates` holds, by the filter and by the index,
/// wherever the two differ: (piece, index of the event, (x, y), by the filter,
/// by the index). Empty when they agree on every event.
fn disagreements(
    gates: &[Arc<dyn DrawableGate>],
    (xs, ys): &(Vec<f32>, Vec<f32>),
) -> Vec<(Arc<str>, usize, (f32, f32), bool, bool)> {
    let df = df![X => xs.clone(), Y => ys.clone()].unwrap();
    let idx = EventIndex::build(xs, ys).unwrap();
    let state = state_with(gates);
    let resolver = state.get_current_sample(Arc::from("f1"), &Default::default());
    let mut out = Vec::new();
    for gate in gates {
        let pieces = if gate.is_composite() {
            gate.get_inner_gate_ids()
        } else {
            vec![gate.get_id()]
        };
        for piece in pieces {
            let inner = gate.get_gate_ref(Some(&piece)).expect("a piece resolves");
            let by_index: std::collections::BTreeSet<usize> =
                idx.filter_by_gate(inner).unwrap().into_iter().collect();
            let mask = filter_events_to_mask(&df, piece.clone(), &resolver)
                .unwrap_or_else(|e| panic!("{piece} cannot be filtered: {e}"));
            for (i, held) in mask.into_iter().enumerate() {
                let (f, x) = (held.unwrap_or(false), by_index.contains(&i));
                if f != x {
                    out.push((piece.clone(), i, (xs[i], ys[i]), f, x));
                }
            }
        }
    }
    out
}

/// Where every piece of `gates` has its corners, the middle of each side, and
/// - for an ellipse - points round its boundary: the events most likely to
/// fall one way for one count and the other way for the other.
fn on_the_edges(gates: &[Arc<dyn DrawableGate>]) -> (Vec<f32>, Vec<f32>) {
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    let mut push = |x: f32, y: f32| {
        xs.push(x);
        ys.push(y);
    };
    for gate in gates {
        let pieces = if gate.is_composite() {
            gate.get_inner_gate_ids()
        } else {
            vec![gate.get_id()]
        };
        for piece in pieces {
            let inner = gate.get_gate_ref(Some(&piece)).unwrap();
            match &inner.geometry {
                flow_gates::GateGeometry::Rectangle { min, max } => {
                    let (x0, y0) = (
                        min.get_coordinate(X).unwrap(),
                        min.get_coordinate(Y).unwrap(),
                    );
                    let (x1, y1) = (
                        max.get_coordinate(X).unwrap(),
                        max.get_coordinate(Y).unwrap(),
                    );
                    for (x, y) in [
                        (x0, y0),
                        (x1, y1),
                        (x0, y1),
                        (x1, y0),
                        ((x0 + x1) / 2.0, y0),
                        ((x0 + x1) / 2.0, y1),
                        (x0, (y0 + y1) / 2.0),
                        (x1, (y0 + y1) / 2.0),
                    ] {
                        push(x, y);
                    }
                }
                flow_gates::GateGeometry::Polygon { nodes, .. } => {
                    let corners: Vec<(f32, f32)> = nodes
                        .iter()
                        .map(|n| (n.get_coordinate(X).unwrap(), n.get_coordinate(Y).unwrap()))
                        .collect();
                    for (i, &(x, y)) in corners.iter().enumerate() {
                        let (nx, ny) = corners[(i + 1) % corners.len()];
                        // Only the corners within the plot: a quadrant's
                        // reach out to its infinite bounds.
                        for (px, py) in [(x, y), ((x + nx) / 2.0, (y + ny) / 2.0)] {
                            if px.abs() < 1e6 && py.abs() < 1e6 {
                                push(px, py);
                            }
                        }
                    }
                }
                flow_gates::GateGeometry::Ellipse {
                    center,
                    radius_x,
                    radius_y,
                    angle,
                } => {
                    let (cx, cy) = (
                        center.get_coordinate(X).unwrap(),
                        center.get_coordinate(Y).unwrap(),
                    );
                    for k in 0..64 {
                        let t = k as f32 * std::f32::consts::TAU / 64.0;
                        let (u, v) = (radius_x * t.cos(), radius_y * t.sin());
                        push(
                            cx + u * angle.cos() - v * angle.sin(),
                            cy + u * angle.sin() + v * angle.cos(),
                        );
                    }
                }
                flow_gates::GateGeometry::Boolean { .. } => {}
            }
        }
    }
    // And the lines the composites divide along, through their centres.
    for x in [250.0f32, 310.0, 280.0] {
        for y in [0.0f32, 330.0, 300.0, 280.0, 1000.0] {
            push(x, y);
        }
    }
    (xs, ys)
}

/// Every kind of gate, events on every kind of edge: the population the filter
/// passes down holds exactly the events the percentage on the gate counts.
#[test]
fn the_filter_and_the_index_agree_on_every_event_on_every_kind_of_edge() {
    let gates = gates();
    let events = on_the_edges(&gates);
    assert!(
        events.0.len() > 100,
        "the fixture should hold plenty of edge events, has {}",
        events.0.len()
    );
    let wrong = disagreements(&gates, &events);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Whole-number events and gates with whole-number corners, so events land
/// exactly on edges, corners and slanted sides all the time - the case that
/// hid the disagreement on real, decimal data. A few NaN and infinite events
/// too, which neither may hold.
#[test]
fn the_filter_and_the_index_agree_on_whole_number_data_whatever_the_gates() {
    use rand::prelude::*;
    let mut rng = StdRng::seed_from_u64(17);
    for trial in 0..150 {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for x in 0..=40 {
            for y in 0..=40 {
                xs.push(x as f32);
                ys.push(y as f32);
            }
        }
        for _ in 0..5 {
            xs.push(f32::NAN);
            ys.push(rng.random_range(0..=40) as f32);
            xs.push(rng.random_range(0..=40) as f32);
            ys.push(f32::INFINITY);
        }
        let mut corner = || {
            (
                rng.random_range(0..=40) as f32,
                rng.random_range(0..=40) as f32,
            )
        };
        let ((ax, ay), (bx, by)) = (corner(), corner());
        let rect = create_rectangle_geometry(
            vec![
                (ax.min(bx), ay.min(by)),
                (ax.max(bx), ay.min(by)),
                (ax.max(bx), ay.max(by)),
                (ax.min(bx), ay.max(by)),
            ],
            X,
            Y,
        );
        let poly = create_polygon_geometry((0..5).map(|_| corner()).collect(), X, Y);
        let (c, l, r) = (corner(), corner(), corner());
        let ellipse = clingate::omiq::deserialise::create_omiq_ellipse_geometry(
            (l.0 as f64, l.1 as f64),
            (r.0 as f64, r.1 as f64),
            (c.0 as f64, c.1 as f64),
            X,
            Y,
        );
        let mut gates: Vec<Arc<dyn DrawableGate>> = Vec::new();
        // A degenerate draw - a rectangle or polygon with no area, an ellipse
        // with no width - is refused by the constructors; nothing to compare.
        if let Ok(g) = rect {
            if let Ok(g) = RectangleGate::try_new(inner("rect", g), true) {
                gates.push(Arc::new(g));
            }
        }
        if let Ok(g) = poly {
            if let Ok(g) = PolygonGate::try_new(inner("poly", g), true) {
                gates.push(Arc::new(g));
            }
        }
        if let Ok(g) = ellipse {
            if let Ok(g) = EllipseGate::try_new(inner("ellipse", g), true) {
                gates.push(Arc::new(g));
            }
        }
        let wrong = disagreements(&gates, &(xs, ys));
        assert!(wrong.is_empty(), "trial {trial}: {wrong:#?}");
    }
}

/// Was B-IDX-1, fixed in flow_gates: `EventIndex::build` panicked on an
/// event with a NaN value - `rstar`'s bulk load unwraps a comparison NaN
/// cannot answer - and a plot holding one showed no percentages. The event is
/// now left out of the index, as the filter leaves it out of every gate.
#[test]
fn an_index_can_be_built_over_an_event_with_no_value() {
    // Enough events for the bulk load to sort them: a handful goes into one
    // leaf without a comparison, and builds whatever it holds.
    let xs: Vec<f32> = (0..1000)
        .map(|i| if i == 500 { f32::NAN } else { i as f32 })
        .collect();
    let ys: Vec<f32> = (0..1000).map(|i| i as f32).collect();
    let built = std::panic::catch_unwind(|| EventIndex::build(&xs, &ys).is_ok());
    assert_eq!(built.ok(), Some(true), "panicked, or refused the data");
}
