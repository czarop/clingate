//! A search's closest candidates on the gallery: the gate each puts on a
//! sample, over the gate as drawn, one candidate at a time - so close
//! candidates are told apart by eye. The searches are made by `fit_rule`
//! and kept in the workspace ([`clingate_core::gate_rules::searches`]).

use std::sync::Arc;

use dioxus::prelude::*;

use clingate_core::axis_store::PlotMapper;
use clingate_core::gate_rules::fit::Candidate;
use clingate_core::gate_rules::rule_store::RuleStore;
use clingate_core::gate_rules::score::PlacedGate;
use clingate_core::gate_rules::searches::Search;
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::{GateId, NodeId};
use clingate_core::gates::gate_traits::DrawableGate;

use super::overlay::{Flat, flatten};
use super::select::matched_to_axes;
use crate::components::toast::{say, use_toast};

/// A candidate's outline, apart from the gate as drawn.
pub const CANDIDATE_STROKE: &str = "magenta";

/// The searches of gates drawn on `node` that this plot's axes show.
pub fn searches_here(
    searches: &[Search],
    state: &GateState,
    node: &Arc<str>,
    x: &str,
    y: &str,
) -> Vec<Search> {
    let on_plot = |gate_id: &str| {
        state
            .child_nodes(&NodeId::from(node.clone()))
            .iter()
            .filter_map(|child| state.gate_for_node(child))
            .any(|child| &**child == gate_id)
            && state
                .registered_gate(&GateId::from(gate_id))
                .is_some_and(|gate| gate.match_to_plot_axis(x, y).is_ok())
    };
    searches
        .iter()
        .filter(|search| search.gate_id().is_some_and(on_plot))
        .cloned()
        .collect()
}

/// Where `candidate` puts the gate on the sample `file`, if it moves it there.
pub fn placed_on(candidate: &Candidate, file: &str) -> Option<PlacedGate> {
    candidate.placed.iter().find(|p| p.file == file).cloned()
}

/// Whether the gate's rule is still one `search` tried, so taking a
/// candidate replaces nothing made since.
pub fn still_as_searched(search: &Search, rules: &RuleStore) -> bool {
    let now = rules.get(&search.target);
    search.candidates.iter().any(|c| Some(&c.rule) == now)
}

/// Whether `candidate`'s rule can be taken for `search`'s gate.
pub fn can_take(search: &Search, candidate: &Candidate, rules: &RuleStore) -> bool {
    still_as_searched(search, rules) && rules.get(&search.target) != Some(&candidate.rule)
}

/// `placed` as a gate on this plot's axes.
pub fn drawn_here(placed: &PlacedGate, x: &str, y: &str) -> Option<Arc<dyn DrawableGate>> {
    let gate = clingate_core::review::replay::drawable(
        &placed.gate,
        &GateId::from(placed.gate_id.as_str()),
    )
    .ok()?;
    matched_to_axes(&[gate], x, y).into_iter().next()
}

/// `placed`'s outline on this plot, drawn as a candidate.
pub fn candidate_shapes(placed: &PlacedGate, x: &str, y: &str, mapper: &PlotMapper) -> Vec<Flat> {
    let Some(gate) = drawn_here(placed, x, y) else {
        return Vec::new();
    };
    as_candidate(flatten(gate.draw_self(false, None, mapper, &None), mapper))
}

/// `shapes` drawn as a candidate: dashed, unfilled and unlabelled, in
/// [`CANDIDATE_STROKE`].
pub fn as_candidate(shapes: Vec<Flat>) -> Vec<Flat> {
    shapes
        .into_iter()
        .filter_map(|shape| match shape {
            Flat::Path {
                points,
                closed,
                width,
                ..
            } => Some(Flat::Path {
                points,
                closed,
                stroke: CANDIDATE_STROKE,
                fill: "none",
                width,
                dashed: true,
            }),
            Flat::Ellipse {
                centre,
                radius,
                rotation,
                width,
                ..
            } => Some(Flat::Ellipse {
                centre,
                radius,
                rotation,
                stroke: CANDIDATE_STROKE,
                fill: "none",
                width,
                dashed: true,
            }),
            Flat::Text { .. } => None,
        })
        .collect()
}

/// A candidate's score in a few words.
pub fn standing(candidate: &Candidate) -> String {
    match &candidate.fit {
        Some(fit) => match fit.typical_agreement {
            Some(typical) => format!("typical {typical:.2}, {} off", fit.off),
            None => format!("{} off", fit.off),
        },
        None => "not scored".to_string(),
    }
}

/// The candidate in a line, for the bar above the plots.
pub fn said(candidate: &Candidate) -> String {
    let mut line = format!("{} · {}", candidate.said, standing(candidate));
    if candidate.current {
        line.push_str(" · the rule as it stands");
    }
    line
}

/// Above the plots: a kept search's candidates stepped through over the gate
/// as drawn, and one's rule taken. `candidate_at` is none when hidden.
#[component]
pub fn SearchBar(
    here: ReadSignal<Vec<Search>>,
    search_at: Signal<usize>,
    candidate_at: Signal<Option<usize>>,
) -> Element {
    let mut rules = use_context::<Signal<RuleStore>>();
    let toasts = use_toast();
    let searches = here.read();
    let Some(search) = searches.get(search_at()) else {
        return rsx! {};
    };
    let count = search.candidates.len();
    let showing = candidate_at().filter(|at| *at < count);
    let shown = showing.and_then(|at| search.candidates.get(at));
    let line = match (showing, shown) {
        (Some(at), Some(candidate)) => format!("{} of {count}: {}", at + 1, said(candidate)),
        _ => "hidden".to_string(),
    };
    let target = search.target.clone();
    let gate = search.gate.clone();
    let changed = !still_as_searched(search, &rules.read());
    let rule = shown
        .filter(|c| can_take(search, c, &rules.read()))
        .map(|c| c.rule.clone());
    let can_use = rule.is_some();
    let searched_at = search.searched_at.clone();
    let has_before = showing.is_some_and(|at| at > 0);
    let has_after = showing.is_some_and(|at| at + 1 < count);
    rsx! {
        div { class: "gallery-search",
            if searches.len() > 1 {
                select {
                    onchange: move |e| {
                        if let Ok(at) = e.value().parse::<usize>() {
                            search_at.set(at);
                            candidate_at.set(Some(0));
                        }
                    },
                    for (at , each) in searches.iter().enumerate() {
                        option { value: "{at}", selected: at == search_at(), "{each.gate}" }
                    }
                }
            } else {
                span { class: "gallery-search_gate", "Search of {gate}" }
            }
            button {
                disabled: !has_before,
                onclick: move |_| candidate_at.set(showing.map(|at| at - 1)),
                "‹"
            }
            span { class: "gallery-search_candidate", "{line}" }
            button {
                disabled: !has_after,
                onclick: move |_| candidate_at.set(showing.map(|at| at + 1)),
                "›"
            }
            button {
                onclick: move |_| candidate_at.set(if showing.is_some() { None } else { Some(0) }),
                if showing.is_some() { "Hide" } else { "Show" }
            }
            button {
                disabled: !can_use,
                onclick: move |_| {
                    if let Some(rule) = rule.clone() {
                        rules.write().insert(target.clone(), rule);
                        say(
                            &toasts,
                            format!(
                                "{gate} now has this candidate's rule - save the rules on the \
                                 Gate Rules tab to keep it"
                            ),
                        );
                    }
                },
                "Use this rule"
            }
            span { class: "gallery-search_key", "dashed: the candidate's gate, where it moves it · searched {searched_at}" }
            if changed {
                span { class: "gallery-search_changed",
                    "{gate}'s rule has changed since this search - search again to take a candidate"
                }
            }
        }
    }
}
