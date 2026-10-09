//! A search kept by `fit_rule` on the shared test workspace, as the gallery
//! shows it: on the plot of the population its gate is drawn on, each
//! candidate's gate on the samples it moves, drawn apart from the gate as
//! drawn.
#![cfg(test)]

use std::sync::Arc;

use flow_fcs::TransformType;

use clingate_core::axis_store::PlotMapper;
use clingate_core::gate_rules::autogate::extent_on;
use clingate_core::gate_rules::rule_store::{GateRule, RuleStore};
use clingate_core::gate_rules::searches::{Search, kept};
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::NodeId;
use clingate_core::session::{FitAsk, Session};
use clingate_core::test_workspace::two_samples_with_a_rule;

use super::overlay::Flat;
use super::searched::{
    CANDIDATE_STROKE, as_candidate, can_take, candidate_shapes, drawn_here, placed_on, said,
    searches_here, still_as_searched,
};

/// The workspace after a search of Tmem's rule, the searches it keeps,
/// Tmem's node and its gate's two parameters.
fn searched(name: &str) -> (Session, Vec<Search>, NodeId, (String, String)) {
    let folder = two_samples_with_a_rule(name);
    let session = Session::open(&folder).unwrap();
    let ask = FitAsk {
        defaults: true,
        ..FitAsk::default()
    };
    session
        .fit_rule("Tmem", ask, Default::default(), Default::default())
        .unwrap();
    let state = session.gates();
    let id = state
        .registered_ids()
        .into_iter()
        .find(|id| state.registered_gate(id).unwrap().get_name() == "Tmem")
        .unwrap();
    let node = state.nodes_for_gate(&id)[0].clone();
    let (x, y) = state.registered_gate(&id).unwrap().get_params();
    let searches = kept(&folder).unwrap();
    (session, searches, node, (x.to_string(), y.to_string()))
}

fn parent(state: &GateState, node: &NodeId) -> Arc<str> {
    Arc::from(state.parent_node(node).unwrap().as_str())
}

#[test]
fn a_search_shows_on_its_gate_s_population_on_the_gate_s_axes_either_way_round() {
    let (session, searches, node, (x, y)) = searched("gallery-search-here");
    let state = session.gates();
    let above = parent(state, &node);
    assert_eq!(searches_here(&searches, state, &above, &x, &y).len(), 1);
    assert_eq!(searches_here(&searches, state, &above, &y, &x).len(), 1);
    assert!(searches_here(&searches, state, &above, &x, "FSC-A").is_empty());
    let own = Arc::from(node.as_str());
    assert!(
        searches_here(&searches, state, &own, &x, &y).is_empty(),
        "Tmem is not drawn on itself"
    );
}

#[test]
fn a_candidate_s_gate_is_drawn_on_each_sample_it_moves_and_no_other() {
    let (mut session, searches, _, (x, y)) = searched("gallery-search-placed");
    let current = searches[0]
        .candidates
        .iter()
        .find(|c| c.current)
        .expect("the rule as it stands is kept");
    let moved = session.preview_rules().unwrap().would_move.len();
    assert!(moved > 0);
    assert_eq!(current.placed.len(), moved);

    let placed = &current.placed[0];
    assert_eq!(placed_on(current, &placed.file), Some(placed.clone()));
    assert_eq!(placed_on(current, "no such sample"), None);

    let here = drawn_here(placed, &x, &y).unwrap();
    assert_eq!(
        here.get_gate_ref(None).unwrap().geometry,
        placed.gate.geometry
    );
    let swapped = drawn_here(placed, &y, &x).unwrap();
    let (first, second) = swapped.get_params();
    assert_eq!(
        (&*first, &*second),
        (y.as_str(), x.as_str()),
        "transposed onto the plot"
    );
    assert!(drawn_here(placed, &x, "FSC-A").is_none());
}

#[test]
fn a_candidate_is_drawn_dashed_unfilled_and_unlabelled() {
    let drawn = vec![
        Flat::Path {
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            closed: true,
            stroke: "cyan",
            fill: "rgba(0, 255, 255, 0.2)",
            width: 2.0,
            dashed: false,
        },
        Flat::Ellipse {
            centre: (5.0, 5.0),
            radius: (2.0, 3.0),
            rotation: 10.0,
            stroke: "cyan",
            fill: "rgba(0, 255, 255, 0.2)",
            width: 2.0,
            dashed: false,
        },
        Flat::Text {
            at: (1.0, 1.0),
            size: 10.0,
            text: "Tmem 12%".into(),
            anchor: None,
        },
    ];
    let candidate = as_candidate(drawn);
    assert_eq!(candidate.len(), 2, "the label dropped");
    for shape in &candidate {
        match shape {
            Flat::Path {
                stroke,
                fill,
                dashed,
                points,
                ..
            } => {
                assert_eq!((*stroke, *fill, *dashed), (CANDIDATE_STROKE, "none", true));
                assert_eq!(points.len(), 3, "the outline kept");
            }
            Flat::Ellipse {
                stroke,
                fill,
                dashed,
                radius,
                ..
            } => {
                assert_eq!((*stroke, *fill, *dashed), (CANDIDATE_STROKE, "none", true));
                assert_eq!(*radius, (2.0, 3.0));
            }
            Flat::Text { .. } => panic!("no label"),
        }
    }
}

#[test]
fn a_candidate_is_said_with_its_score_and_whether_it_is_the_rule_as_it_stands() {
    let (_, searches, _, _) = searched("gallery-search-said");
    for candidate in &searches[0].candidates {
        let line = said(candidate);
        let fit = candidate.fit.as_ref().unwrap();
        assert!(line.starts_with(&candidate.said), "{line}");
        assert!(line.contains(&format!("{} off", fit.off)), "{line}");
        assert!(line.contains("typical"), "{line}");
        assert_eq!(
            line.ends_with("the rule as it stands"),
            candidate.current,
            "{line}"
        );
    }
}

fn with_rule(search: &Search, rule: &GateRule) -> RuleStore {
    let mut rules = RuleStore::default();
    rules.insert(search.target.clone(), rule.clone());
    rules
}

/// Any candidate but the gate's rule now can be taken, back and forth among
/// them, until the gate's rule becomes one the search never tried.
#[test]
fn a_candidate_can_be_taken_only_while_the_gate_has_a_rule_the_search_tried() {
    let (_, searches, _, _) = searched("gallery-search-take");
    let search = &searches[0];
    let current = search.candidates.iter().find(|c| c.current).unwrap();
    let other = search.candidates.iter().find(|c| !c.current).unwrap();

    let as_it_stood = with_rule(search, &current.rule);
    assert!(still_as_searched(search, &as_it_stood));
    assert!(!can_take(search, current, &as_it_stood), "already the rule");
    assert!(can_take(search, other, &as_it_stood));

    let taken = with_rule(search, &other.rule);
    assert!(can_take(search, current, &taken), "and back again");
    assert!(!can_take(search, other, &taken));

    let mut edited = current.rule.clone();
    edited.parameter = "FSC-A".into();
    let edited = with_rule(search, &edited);
    assert!(!still_as_searched(search, &edited));
    assert!(search.candidates.iter().all(|c| !can_take(search, c, &edited)));
    assert!(!can_take(search, other, &RuleStore::default()), "the rule deleted");
}

/// A linear plot 600 pixels square over `x` and `y`, data ranges in hand.
fn plot_over(x: (f32, f32), y: (f32, f32)) -> PlotMapper {
    PlotMapper::new(
        600.0,
        600.0,
        x.0..=x.1,
        y.0..=y.1,
        x.0..=x.1,
        y.0..=y.1,
        TransformType::Linear,
        TransformType::Linear,
    )
}

/// The dashed outline sits where the candidate puts the gate: its corners
/// are the gate's extents taken to pixels.
#[test]
fn a_candidate_s_outline_is_drawn_dashed_where_it_puts_the_gate() {
    let (_, searches, _, (x, y)) = searched("gallery-search-outline");
    let current = searches[0].candidates.iter().find(|c| c.current).unwrap();
    let placed = &current.placed[0];
    let x_extent = extent_on(&placed.gate.geometry, &x).unwrap();
    let y_extent = extent_on(&placed.gate.geometry, &y).unwrap();
    let pad = |(lo, hi): (f32, f32)| (lo - (hi - lo), hi + (hi - lo));
    let mapper = plot_over(pad(x_extent), pad(y_extent));

    let shapes = candidate_shapes(placed, &x, &y, &mapper);
    let points: Vec<(f32, f32)> = shapes
        .iter()
        .flat_map(|shape| match shape {
            Flat::Path {
                points,
                stroke,
                dashed,
                ..
            } => {
                assert_eq!((*stroke, *dashed), (CANDIDATE_STROKE, true));
                points.clone()
            }
            other => panic!("a path, not {other:?}"),
        })
        .collect();
    assert!(!points.is_empty());
    let (left, bottom) = mapper.data_to_pixel(x_extent.0, y_extent.0, None, None);
    let (right, top) = mapper.data_to_pixel(x_extent.1, y_extent.1, None, None);
    let least = |at: fn(&(f32, f32)) -> f32| points.iter().map(at).fold(f32::INFINITY, f32::min);
    let most = |at: fn(&(f32, f32)) -> f32| points.iter().map(at).fold(f32::NEG_INFINITY, f32::max);
    let close = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(close(least(|p| p.0), left) && close(most(|p| p.0), right), "{points:?}");
    assert!(close(least(|p| p.1), top.min(bottom)), "{points:?}");
    assert!(close(most(|p| p.1), top.max(bottom)), "{points:?}");

    assert!(candidate_shapes(placed, &x, "FSC-A", &mapper).is_empty());
}
