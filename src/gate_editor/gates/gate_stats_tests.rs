//! Tests for gate statistics and the draft (in-progress) gate.
//!
//! The percentages here are what the user reads off the plot and what ends up
//! in a report, so an off-by-one in the denominator is a reporting error rather
//! than a cosmetic one.
//!
//! cargo test gate_stats -- --nocapture

#![cfg(test)]

use crate::gate_editor::gates::gate_composite::quadrant_gate::QuadrantGate;
use crate::gate_editor::gates::gate_draft::GateDraft;
use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_stats::get_percent_and_counts_gate;
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::gates::gate_types::{GateStatValue, ShapeType};
use crate::gate_editor::plots::axis_store::PlotMapper;
use crate::gate_editor::plots::plot_store::EventIndexMapped;
use flow_fcs::TransformType;
use flow_gates::{EventIndex, create_rectangle_geometry};
use std::sync::Arc;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

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

/// Ten events: four inside 0..400 on both axes, six outside.
fn index() -> EventIndexMapped {
    let xs: Vec<f32> = vec![
        100.0, 200.0, 300.0, 350.0, 500.0, 600.0, 700.0, 800.0, 900.0, 950.0,
    ];
    let ys: Vec<f32> = vec![
        100.0, 200.0, 300.0, 350.0, 500.0, 600.0, 700.0, 800.0, 900.0, 950.0,
    ];

    EventIndexMapped {
        event_index: Arc::new(EventIndex::build(&xs, &ys).unwrap()),
        index_map: Arc::new((0..10).collect()),
    }
}

fn rectangle(id: &str, min: (f32, f32), max: (f32, f32)) -> Arc<dyn DrawableGate> {
    let geometry =
        create_rectangle_geometry(vec![min, (max.0, min.1), max, (min.0, max.1)], X, Y).unwrap();
    let gate = flow_gates::Gate {
        id: Arc::from(id),
        name: id.to_string(),
        geometry,
        mode: flow_gates::GateMode::Global,
        parameters: (Arc::from(X), Arc::from(Y)),
        label_position: None,
    };
    Arc::new(RectangleGate::try_new(gate, true).unwrap())
}

// ─── Single gates ─────────────────────────────────────────────────────────────

#[test]
fn a_single_gate_counts_the_events_inside_it() {
    let stats =
        get_percent_and_counts_gate(rectangle("r", (0.0, 0.0), (400.0, 400.0)), &index(), 10.0)
            .unwrap();

    assert!(!stats.is_composite());
    match stats.count {
        GateStatValue::Single(c) => assert_eq!(c, 4.0),
        _ => panic!("a single gate should report a single count"),
    }
}

#[test]
fn the_percentage_is_taken_against_the_parent_population() {
    let stats =
        get_percent_and_counts_gate(rectangle("r", (0.0, 0.0), (400.0, 400.0)), &index(), 10.0)
            .unwrap();

    match stats.percent_parent {
        GateStatValue::Single(p) => assert_eq!(p, 40.0, "4 of 10 is 40%"),
        _ => panic!("a single gate should report a single percentage"),
    }
}

/// The denominator is the parent's event count, not the total acquired, so the
/// same gate reports a different percentage under a different parent.
#[test]
fn a_smaller_parent_population_raises_the_percentage() {
    let gate = rectangle("r", (0.0, 0.0), (400.0, 400.0));

    let of_ten = get_percent_and_counts_gate(gate.clone(), &index(), 10.0).unwrap();
    let of_five = get_percent_and_counts_gate(gate, &index(), 5.0).unwrap();

    match (of_ten.percent_parent, of_five.percent_parent) {
        (GateStatValue::Single(a), GateStatValue::Single(b)) => {
            assert_eq!(a, 40.0);
            assert_eq!(b, 80.0);
        }
        _ => panic!("expected single values"),
    }
}

#[test]
fn a_gate_containing_everything_reports_a_hundred_percent() {
    let stats = get_percent_and_counts_gate(
        rectangle("all", (-1.0, -1.0), (2000.0, 2000.0)),
        &index(),
        10.0,
    )
    .unwrap();

    match (stats.count, stats.percent_parent) {
        (GateStatValue::Single(c), GateStatValue::Single(p)) => {
            assert_eq!(c, 10.0);
            assert_eq!(p, 100.0);
        }
        _ => panic!("expected single values"),
    }
}

#[test]
fn an_empty_gate_reports_zero() {
    let stats = get_percent_and_counts_gate(
        rectangle("none", (5000.0, 5000.0), (6000.0, 6000.0)),
        &index(),
        10.0,
    )
    .unwrap();

    match (stats.count, stats.percent_parent) {
        (GateStatValue::Single(c), GateStatValue::Single(p)) => {
            assert_eq!(c, 0.0);
            assert_eq!(p, 0.0);
        }
        _ => panic!("expected single values"),
    }
}

// ─── Composite gates ──────────────────────────────────────────────────────────

#[test]
fn a_composite_reports_a_figure_for_each_subgate() {
    let quad: Arc<dyn DrawableGate> = Arc::new(
        QuadrantGate::try_new_from_raw_coord(
            &mapper(),
            Arc::from("q"),
            "q".to_string(),
            (300.0, 300.0),
            Arc::from(X),
            Arc::from(Y),
        )
        .unwrap(),
    );

    let stats = get_percent_and_counts_gate(quad.clone(), &index(), 10.0).unwrap();

    assert!(stats.is_composite());
    match &stats.count {
        GateStatValue::Composite(map) => {
            assert_eq!(map.len(), 4, "one entry per quadrant");
            for id in quad.get_inner_gate_ids() {
                assert!(map.contains_key(&id), "no count for subgate {id}");
            }
        }
        _ => panic!("a composite should report per-subgate counts"),
    }
}

/// Every event in the parent falls into exactly one quadrant, so the four
/// counts must sum to the parent population.
#[test]
fn the_quadrant_counts_sum_to_the_parent_population() {
    let quad: Arc<dyn DrawableGate> = Arc::new(
        QuadrantGate::try_new_from_raw_coord(
            &mapper(),
            Arc::from("q"),
            "q".to_string(),
            (400.0, 400.0),
            Arc::from(X),
            Arc::from(Y),
        )
        .unwrap(),
    );

    let stats = get_percent_and_counts_gate(quad, &index(), 10.0).unwrap();

    match &stats.count {
        GateStatValue::Composite(map) => {
            let total: f32 = map.values().sum();
            assert_eq!(total, 10.0, "quadrants should partition the population");
        }
        _ => panic!("expected composite counts"),
    }
}

#[test]
fn composite_figures_are_retrievable_by_subgate_id() {
    let quad: Arc<dyn DrawableGate> = Arc::new(
        QuadrantGate::try_new_from_raw_coord(
            &mapper(),
            Arc::from("q"),
            "q".to_string(),
            (400.0, 400.0),
            Arc::from(X),
            Arc::from(Y),
        )
        .unwrap(),
    );
    let ids = quad.get_inner_gate_ids();
    let stats = get_percent_and_counts_gate(quad, &index(), 10.0).unwrap();

    let mut total = 0.0;
    let mut percent = 0.0;
    for id in ids {
        total += stats
            .get_count_for_id(id.clone())
            .expect("a count per quarter");
        percent += stats
            .get_percent_for_id(id)
            .expect("a percentage per quarter");
    }
    // The quarters tile the plane: every event is in exactly one of them.
    assert_eq!(total, 10.0, "the ten events split across the quarters");
    assert!(
        (percent - 100.0).abs() < 1e-3,
        "the quarters' percentages sum to {percent}"
    );
}

// ─── Draft gates ──────────────────────────────────────────────────────────────

fn draft(points: Vec<(f32, f32)>) -> GateDraft {
    GateDraft::new_polygon(points, Arc::from(X), Arc::from(Y))
}

#[test]
fn a_draft_is_never_finalised() {
    assert!(!draft(vec![(0.0, 0.0)]).is_finalised());
}

#[test]
fn a_draft_reports_the_points_clicked_so_far() {
    let points = vec![(1.0, 1.0), (2.0, 2.0), (3.0, 1.0)];
    assert_eq!(draft(points.clone()).get_points(), points);
}

#[test]
fn an_empty_draft_draws_nothing() {
    assert!(draft(vec![]).draw_self().is_empty());
}

/// One click is a dot, two is a line, three or more is a closed polygon - the
/// draft has to render sensibly at every stage of being drawn.
#[test]
fn a_single_click_draws_a_point() {
    let shapes = draft(vec![(5.0, 5.0)]).draw_self();

    assert_eq!(shapes.len(), 1);
    assert!(matches!(
        shapes[0],
        crate::gate_editor::gates::gate_types::GateRenderShape::Circle { .. }
    ));
}

#[test]
fn two_clicks_draw_a_line() {
    let shapes = draft(vec![(0.0, 0.0), (5.0, 5.0)]).draw_self();

    assert_eq!(shapes.len(), 1);
    assert!(matches!(
        shapes[0],
        crate::gate_editor::gates::gate_types::GateRenderShape::PolyLine { .. }
    ));
}

#[test]
fn three_clicks_draw_a_closed_polygon() {
    let shapes = draft(vec![(0.0, 0.0), (5.0, 0.0), (2.5, 5.0)]).draw_self();

    assert_eq!(shapes.len(), 1);
    match &shapes[0] {
        crate::gate_editor::gates::gate_types::GateRenderShape::Polygon { points, .. } => {
            assert_eq!(
                points.len(),
                4,
                "the loop is closed by repeating the first point"
            );
            assert_eq!(points[0], points[3]);
        }
        _ => panic!("expected a polygon"),
    }
}

#[test]
fn a_draft_is_tagged_so_the_mouse_handlers_ignore_it() {
    for points in [
        vec![(0.0, 0.0)],
        vec![(0.0, 0.0), (1.0, 1.0)],
        vec![(0.0, 0.0), (1.0, 0.0), (0.5, 1.0)],
    ] {
        for shape in draft(points).draw_self() {
            let tagged = match shape {
                crate::gate_editor::gates::gate_types::GateRenderShape::Circle {
                    shape_type,
                    ..
                }
                | crate::gate_editor::gates::gate_types::GateRenderShape::PolyLine {
                    shape_type,
                    ..
                }
                | crate::gate_editor::gates::gate_types::GateRenderShape::Polygon {
                    shape_type,
                    ..
                } => {
                    matches!(shape_type, ShapeType::DraftGate)
                }
                _ => false,
            };
            assert!(tagged, "every draft shape should carry the DraftGate tag");
        }
    }
}
