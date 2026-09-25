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

/// BUG (docs/test-audit.md, B-CNT-1): the filter admits an event strictly
/// inside a rectangle (`gt(min) & lt(max)`); the index counts one on the
/// edge. Here that is 240 against 246 - exactly the six events placed on the
/// edges and corners - so the percentage on the gate counts events the
/// population drawn under it does not hold. Linear scatter values are often
/// whole numbers and gate edges often round ones, so this is not only a
/// theoretical case.
#[test]
#[ignore = "known bug B-CNT-1: the filter and the on-screen count disagree about events on an edge"]
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

/// BUG (docs/test-audit.md, B-STAT-1): the percentage is `count / parent *
/// 100` with no guard, so a gate over an empty population - a parent that
/// excludes everything on this sample - shows NaN%.
#[test]
#[ignore = "known bug B-STAT-1: a gate over an empty parent shows NaN%"]
fn a_gate_over_an_empty_population_reports_no_percentage_rather_than_nan() {
    let empty = (Vec::<f32>::new(), Vec::<f32>::new());
    let Ok(event_index) = EventIndex::build(&empty.0, &empty.1) else {
        // An index cannot be built over nothing, so there is nothing to show.
        return;
    };
    let idx = EventIndexMapped {
        event_index: Arc::new(event_index),
        index_map: Arc::new(Vec::new()),
    };
    let stats = get_percent_and_counts_gate(gates()[0].clone(), &idx, 0.0).unwrap();
    let percent = stats.get_percent_for_id(Arc::from("rect")).unwrap();
    assert!(percent.is_finite(), "an empty parent shows {percent}%");
}
