//! A search kept by `fit_rule` on the shared test workspace, as the gallery
//! shows it: on the plot of the population its gate is drawn on, each
//! candidate's gate on the samples it moves, drawn apart from the gate as
//! drawn.
#![cfg(test)]

use std::sync::Arc;

use clingate_core::gate_rules::searches::{Search, kept};
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::NodeId;
use clingate_core::session::{FitAsk, Session};
use clingate_core::test_workspace::two_samples_with_a_rule;

use super::overlay::Flat;
use super::searched::{CANDIDATE_STROKE, as_candidate, drawn_here, placed_on, said, searches_here};

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
