//! Searching every rule's settings against the gating drawn by hand, from
//! the Gate Rules tab - the search the tools for Claude make for one rule
//! with `fit_rule`, for every rule at once - and what the kept searches
//! found. The candidates themselves are stepped through on the Gallery tab.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::prelude::*;

use clingate_core::gate_rules::fit::{Candidate, FitSettings};
use clingate_core::gate_rules::run::Progress;
use clingate_core::gate_rules::score::ScoreSettings;
use clingate_core::gate_rules::searches::{Search, keep_all, kept, search_every_rule};

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::gate_rules_window::{RulesRun, use_stop_run_on_change};
use crate::gate_editor::workspace_window::{GateStore, Loaded};

/// A kept search in a line of the list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub gate: String,
    pub best: String,
    pub as_it_stands: String,
    /// How much higher the best's typical agreement is than the rule as it
    /// stands.
    pub gain: Option<f64>,
}

/// A candidate's score in a few words.
fn standing(candidate: &Candidate) -> String {
    match &candidate.fit {
        Some(fit) => match fit.typical_agreement {
            Some(typical) => format!("typical {typical:.2}, {} off", fit.off),
            None => format!("{} off", fit.off),
        },
        None => "not scored".to_string(),
    }
}

fn typical(candidate: &Candidate) -> Option<f64> {
    candidate.fit.as_ref()?.typical_agreement
}

/// The kept searches as lines, those where the best does most better than
/// the rule as it stands first.
pub(crate) fn lines(searches: &[Search]) -> Vec<Line> {
    let mut lines: Vec<Line> = searches
        .iter()
        .filter_map(|search| {
            let best = search.candidates.first()?;
            let current = search.candidates.iter().find(|c| c.current);
            Some(Line {
                gate: search.gate.clone(),
                best: if best.current {
                    "the rule as it stands".to_string()
                } else {
                    format!("{} - {}", best.said, standing(best))
                },
                as_it_stands: current.map_or("no rule".to_string(), standing),
                gain: typical(best)
                    .zip(current.and_then(typical))
                    .map(|(best, current)| best - current),
            })
        })
        .collect();
    lines.sort_by(|a, b| {
        let gain = |line: &Line| line.gain.unwrap_or(f64::NEG_INFINITY);
        gain(b).total_cmp(&gain(a))
    });
    lines
}

/// The progress line, for a search.
fn searching(step: Progress) -> String {
    match step {
        Progress::Measuring { done, total } => format!("Reading file {done} of {total}"),
        Progress::Solving { done, total } => format!("Trying candidate {done} of {total}"),
    }
}

#[component]
pub fn SearchPanel() -> Element {
    let run_with = RulesRun::from_context();
    let gate_store = use_context::<GateStore>();
    let loaded = use_context::<Signal<Loaded>>();
    let toasts = use_toast();
    let mut running = use_signal(|| false);
    let mut progress = use_signal(|| None::<Progress>);
    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);
    use_stop_run_on_change(cancel);
    // Counted up after each search, so the list reads the kept searches again.
    let mut searched = use_signal(|| 0u64);
    let listed = use_memo(move || {
        searched();
        loaded
            .read()
            .folder
            .as_deref()
            .map(|folder| lines(&kept(folder).unwrap_or_default()))
            .unwrap_or_default()
    });

    let start = move |_| {
        spawn(async move {
            if running() {
                return;
            }
            let Some(folder) = loaded.peek().folder.clone() else {
                warn(
                    &toasts,
                    "Open a workspace folder first - the searches are kept in it",
                );
                return;
            };
            let started = run_with.inputs_now();
            if started.0.files.is_empty() {
                warn(
                    &toasts,
                    "No FCS files are loaded - open a workspace on the first tab",
                );
                return;
            }
            running.set(true);
            progress.set(Some(Progress::Measuring { done: 0, total: 0 }));
            let snapshot = gate_store.read().clone();
            let started_gates = snapshot.clone();
            let flag = Arc::new(AtomicBool::new(false));
            cancel.set(Some(flag.clone()));
            let stopped = flag.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
            let worker = {
                let inputs = started.0.clone();
                tokio::task::spawn_blocking(move || {
                    search_every_rule(
                        &snapshot,
                        &inputs,
                        ScoreSettings::default(),
                        FitSettings::default(),
                        &flag,
                        |step| {
                            let _ = tx.send(step);
                        },
                    )
                })
            };
            while let Some(step) = rx.recv().await {
                progress.set(Some(step));
            }
            let outcome = worker.await;
            progress.set(None);
            cancel.set(None);
            running.set(false);

            let every = match outcome {
                Ok(Ok(every)) => every,
                Ok(Err(_)) if stopped.load(Ordering::Relaxed) => {
                    note(&toasts, "Stopped - nothing was kept");
                    return;
                }
                Ok(Err(why)) => {
                    warn(&toasts, format!("The search could not be made: {why}"));
                    return;
                }
                Err(e) => {
                    warn(&toasts, format!("The search did not finish: {e}"));
                    return;
                }
            };
            if !gate_store.peek().unchanged_since(&started_gates)
                || run_with.inputs_now() != started
            {
                warn(
                    &toasts,
                    "The workspace changed while searching - nothing was kept; search again",
                );
                return;
            }
            let count = every.searches.len();
            if let Err(e) = keep_all(&folder, every.searches) {
                warn(&toasts, format!("The searches could not be kept: {e}"));
                return;
            }
            searched += 1;
            let mut said = format!(
                "Searched {count} rule(s) - step through each gate's closest candidates on the Gallery tab"
            );
            if !every.not_searched.is_empty() {
                said.push_str(&format!(
                    "; not searched: {}",
                    every.not_searched.join("; ")
                ));
            }
            say(&toasts, said);
        });
    };

    rsx! {
        fieldset { class: "gate_rules-form",
            legend { "Search the rules' settings" }
            p { class: "gate_rules-hint gate_rules-span",
                "Tries a few values of each setting that matters for every rule - smoothing and dips for a valley, the band's width and aim, how far above the negative - and scores each against the gates as drawn by hand, every gate under its parent as drawn. The files are read once for all of them; nothing is moved. The closest candidates and the rule as it stands are kept, to step through on the Gallery tab, where Use this rule takes one."
            }
            button {
                class: "gate_rules-add",
                disabled: running(),
                onclick: start,
                if running() { "Searching..." } else { "Search every rule" }
            }
            if let Some(step) = progress() {
                div { class: "gate_rules-progress gate_rules-span",
                    div { class: "gate_rules-bar",
                        div {
                            class: "gate_rules-bar_fill",
                            style: "width: {step.fraction() * 100.0}%",
                        }
                    }
                    span { class: "gate_rules-progress_text", "{searching(step)}" }
                    button {
                        class: "gate_rules-cancel",
                        onclick: move |_| {
                            if let Some(flag) = cancel() {
                                flag.store(true, Ordering::Relaxed);
                                note(&toasts, "Stopping...");
                            }
                        },
                        "Stop"
                    }
                }
            }
            if !listed.read().is_empty() {
                table { class: "gate_rules-table gate_rules-span",
                    thead {
                        tr {
                            th { "Gate" }
                            th { "Best" }
                            th { "As it stands" }
                        }
                    }
                    tbody {
                        for (at , line) in listed.read().iter().enumerate() {
                            tr { key: "{at}",
                                td { "{line.gate}" }
                                td { "{line.best}" }
                                td { "{line.as_it_stands}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clingate_core::gate_rules::rule::{Rule, TailFractionRule};
    use clingate_core::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleTarget};
    use clingate_core::gate_rules::score::GateScore;

    use super::*;

    fn candidate(said: &str, typical: Option<f64>, off: usize, current: bool) -> Candidate {
        Candidate {
            rule: GateRule {
                parameter: Arc::from("CD69"),
                bound: Bound::Above,
                measured_on: MeasuredOn::Itself,
                rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
            },
            said: said.to_string(),
            current,
            fit: Some(GateScore {
                gate_id: "g".into(),
                gate: "g".into(),
                scored: 10,
                not_placed: 0,
                references: 0,
                typical_agreement: typical,
                lowest_agreement: None,
                off,
                off_samples: Vec::new(),
                median_caught: None,
                median_extra: None,
                median_holds_difference: None,
                median_edge_off_iqrs: None,
            }),
            check: None,
            place_by_typical: 1,
            place_by_off: 1,
            place_on_check: None,
            among_best: true,
            placed: Vec::new(),
        }
    }

    fn search(gate: &str, candidates: Vec<Candidate>) -> Search {
        Search {
            format: 1,
            searched_at: String::new(),
            target: RuleTarget::named(gate),
            gate: gate.to_string(),
            candidates,
        }
    }

    #[test]
    fn the_gates_where_the_best_does_most_better_come_first() {
        let searches = [
            search(
                "CD25+",
                vec![
                    candidate("wider", Some(0.90), 1, false),
                    candidate("as is", Some(0.85), 3, true),
                ],
            ),
            search("CD69+", vec![candidate("as is", Some(0.95), 0, true)]),
            search("CD8+", vec![candidate("new", Some(0.80), 2, false)]),
            search(
                "CD4+",
                vec![
                    candidate("deeper dip", Some(0.92), 0, false),
                    candidate("as is", Some(0.70), 6, true),
                ],
            ),
        ];
        let listed = lines(&searches);
        let gates: Vec<&str> = listed.iter().map(|l| l.gate.as_str()).collect();
        assert_eq!(gates, ["CD4+", "CD25+", "CD69+", "CD8+"]);
        assert_eq!(listed[0].best, "deeper dip - typical 0.92, 0 off");
        assert_eq!(listed[0].as_it_stands, "typical 0.70, 6 off");
        assert!((listed[0].gain.unwrap() - 0.22).abs() < 1e-12);
        assert_eq!(listed[2].best, "the rule as it stands");
        assert_eq!(listed[2].gain, Some(0.0));
        assert_eq!(
            (listed[3].as_it_stands.as_str(), listed[3].gain),
            ("no rule", None)
        );
    }
}
