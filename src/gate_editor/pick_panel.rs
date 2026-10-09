//! Picking the best rule for every gate against the gating drawn by hand,
//! from the Gate Rules tab - its kind as well as its settings, as the tools
//! for Claude pick with `pick_rule` - and taking the picks. Close picks are
//! stepped through on the Gallery tab.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::prelude::*;

use clingate_core::gate_rules::fit::FitSettings;
use clingate_core::gate_rules::pick::{Picking, fits_well, gain};
use clingate_core::gate_rules::rule_store::{GateRule, RuleStore, RuleTarget};
use clingate_core::gate_rules::score::ScoreSettings;
use clingate_core::gate_rules::searches::{Search, keep_all, kept, pick_every_rule};

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::gallery::searched::standing;
use crate::gate_editor::gate_rules_window::{RulesRun, use_stop_run_on_change};
use crate::gate_editor::route::Tab;
use crate::gate_editor::workspace_window::{GateStore, Loaded};

/// Where a gate's pick stands against the rules as they are now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Choice {
    /// The gate's rule is the pick.
    InUse,
    /// The gate still has the rule the pick was made against, and the pick
    /// differs from it: it can be taken.
    Better(GateRule),
    /// The gate's rule has changed since the pick was made.
    Changed,
}

/// Where `search`'s best stands against `rules`.
pub(crate) fn choice(search: &Search, rules: &RuleStore) -> Choice {
    let now = rules.get(&search.target);
    let Some(best) = search.candidates.first() else {
        return Choice::Changed;
    };
    let picked_against = search
        .candidates
        .iter()
        .find(|c| c.current)
        .map(|c| &c.rule);
    if now == Some(&best.rule) {
        Choice::InUse
    } else if now == picked_against {
        Choice::Better(best.rule.clone())
    } else {
        Choice::Changed
    }
}

/// A gate's pick in a line of the list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub target: RuleTarget,
    pub gate: String,
    pub best: String,
    pub as_it_stands: String,
    /// How much higher the best's typical agreement is than the rule it was
    /// picked against.
    pub gain: Option<f64>,
    pub fits_well: bool,
    pub choice: Choice,
}

/// The kept picks as lines against `rules`, those where the best does most
/// better first.
pub(crate) fn lines(searches: &[Search], rules: &RuleStore, settings: &ScoreSettings) -> Vec<Line> {
    let mut lines: Vec<Line> = searches
        .iter()
        .filter_map(|search| {
            let best = search.candidates.first()?;
            let current = search.candidates.iter().find(|c| c.current);
            Some(Line {
                target: search.target.clone(),
                gate: search.gate.clone(),
                best: format!("{} - {}", best.said, standing(best)),
                as_it_stands: current.map_or("no rule".to_string(), standing),
                gain: gain(&search.candidates),
                fits_well: fits_well(&search.candidates, settings),
                choice: choice(search, rules),
            })
        })
        .collect();
    lines.sort_by(|a, b| {
        let gain = |line: &Line| line.gain.unwrap_or(f64::NEG_INFINITY);
        gain(b).total_cmp(&gain(a))
    });
    lines
}

/// The picks taken all at once: each gate's best where the gate still has
/// the rule it was picked against and the best agrees more with the hand
/// gating - not where the two are only tied.
pub(crate) fn takeable(lines: &[Line]) -> Vec<(RuleTarget, GateRule)> {
    lines
        .iter()
        .filter(|line| line.gain.is_none_or(|gain| gain > 0.0))
        .filter_map(|line| match &line.choice {
            Choice::Better(rule) => Some((line.target.clone(), rule.clone())),
            _ => None,
        })
        .collect()
}

/// The Gate Rules tab's pick of the best rule for every gate, and its list.
#[component]
pub fn PickPanel() -> Element {
    let run_with = RulesRun::from_context();
    let gate_store = use_context::<GateStore>();
    let loaded = use_context::<Signal<Loaded>>();
    let mut rules = use_context::<Signal<RuleStore>>();
    let toasts = use_toast();
    let mut running = use_signal(|| false);
    let mut progress = use_signal(|| None::<Picking>);
    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);
    use_stop_run_on_change(cancel);
    let active = use_context::<Signal<Tab>>();
    // Counted up after each pick, so the kept picks are read again - as they
    // are each time the tab comes to the front, for a pick by the tools.
    let mut picked = use_signal(|| 0u64);
    let searches = use_memo(move || {
        picked();
        if active() != Tab::Rules {
            return Vec::new();
        }
        loaded
            .read()
            .folder
            .as_deref()
            .map(|folder| kept(folder).unwrap_or_default())
            .unwrap_or_default()
    });
    let listed =
        use_memo(move || lines(&searches.read(), &rules.read(), &ScoreSettings::default()));
    let mut take = move |picks: Vec<(RuleTarget, GateRule)>| {
        let count = picks.len();
        {
            let mut rules = rules.write();
            for (target, rule) in picks {
                rules.insert(target, rule);
            }
        }
        say(
            &toasts,
            format!(
                "{count} gate(s) now have their best rule - Save the rules below to keep them, \
                 or Load to go back to the rules saved"
            ),
        );
    };

    let start = move |_| {
        spawn(async move {
            if running() {
                return;
            }
            let Some(folder) = loaded.peek().folder.clone() else {
                warn(
                    &toasts,
                    "Open a workspace folder first - the picks are kept in it",
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
            progress.set(Some(Picking::Reading { done: 0, total: 0 }));
            let snapshot = gate_store.read().clone();
            let started_gates = snapshot.clone();
            let flag = Arc::new(AtomicBool::new(false));
            cancel.set(Some(flag.clone()));
            let stopped = flag.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Picking>();
            let worker = {
                let inputs = started.0.clone();
                tokio::task::spawn_blocking(move || {
                    pick_every_rule(
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
                    warn(&toasts, format!("The pick could not be made: {why}"));
                    return;
                }
                Err(e) => {
                    warn(&toasts, format!("The pick did not finish: {e}"));
                    return;
                }
            };
            if !gate_store.peek().unchanged_since(&started_gates)
                || run_with.inputs_now() != started
            {
                warn(
                    &toasts,
                    "The workspace changed while picking - nothing was kept; pick again",
                );
                return;
            }
            let count = every.searches.len();
            if let Err(e) = keep_all(&folder, every.searches) {
                warn(&toasts, format!("The picks could not be kept: {e}"));
                return;
            }
            picked += 1;
            let mut said = format!("Picked for {count} gate(s) - see the list below");
            if !every.not_searched.is_empty() {
                said.push_str(&format!("; not picked: {}", every.not_searched.join("; ")));
            }
            say(&toasts, said);
        });
    };

    let all = takeable(&listed.read());
    rsx! {
        fieldset { class: "gate_rules-form",
            legend { "Pick the best rule" }
            p { class: "gate_rules-hint gate_rules-span",
                "For every gate a rule places, tries each kind of rule that can place it - a band read on the FMX and on the sample, above the negative read two ways, the valley with and without a smear - each started from where your hand gating says, then searches the settings of the best kind. Every rule is scored against the gates as drawn by hand, each gate under its parent as drawn; the files are read once. Nothing changes until you take a pick. A gate placed from another gate is left as it is."
            }
            button {
                class: "gate_rules-add",
                disabled: running(),
                onclick: start,
                if running() { "Picking..." } else { "Pick the best rule for every gate" }
            }
            if let Some(step) = progress() {
                div { class: "gate_rules-progress gate_rules-span",
                    div { class: "gate_rules-bar",
                        div {
                            class: "gate_rules-bar_fill",
                            style: "width: {step.fraction() * 100.0}%",
                        }
                    }
                    span { class: "gate_rules-progress_text", "{step.describe()}" }
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
                p { class: "gate_rules-hint gate_rules-span",
                    "Step through a gate's closest picks over your gate on the Gallery tab. A pick marked \"no close rule\" is the best tried but still far from your gating - that gate may be better gated by hand."
                }
                button {
                    class: "gate_rules-add",
                    disabled: all.is_empty(),
                    onclick: {
                        let all = all.clone();
                        move |_| take(all.clone())
                    },
                    "Use the best for {all.len()} gate(s)"
                }
                table { class: "gate_rules-table gate_rules-span",
                    thead {
                        tr {
                            th { "Gate" }
                            th { "Best rule" }
                            th { "As it stands" }
                            th { "" }
                        }
                    }
                    tbody {
                        for (at , line) in listed.read().iter().enumerate() {
                            tr { key: "{at}",
                                td { "{line.gate}" }
                                td {
                                    "{line.best}"
                                    if !line.fits_well {
                                        " - no close rule"
                                    }
                                }
                                td { "{line.as_it_stands}" }
                                td {
                                    match &line.choice {
                                        Choice::InUse => rsx! { "in use" },
                                        Choice::Changed => rsx! { "rule changed since the pick" },
                                        Choice::Better(rule) => {
                                            let pick = (line.target.clone(), rule.clone());
                                            rsx! {
                                                button { onclick: move |_| take(vec![pick.clone()]), "Use" }
                                            }
                                        }
                                    }
                                }
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

    use clingate_core::gate_rules::fit::Candidate;
    use clingate_core::gate_rules::rule::{Rule, TailFractionRule, ValleyRule};
    use clingate_core::gate_rules::rule_store::{Bound, MeasuredOn};
    use clingate_core::gate_rules::score::GateScore;

    use super::*;

    fn rule(kind: Rule) -> GateRule {
        GateRule {
            parameter: Arc::from("CD69"),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: kind,
        }
    }

    fn band() -> GateRule {
        rule(Rule::TailFraction(TailFractionRule::new((0.01, 0.02))))
    }

    fn valley() -> GateRule {
        rule(Rule::InTheValley(ValleyRule::default()))
    }

    fn candidate(rule: GateRule, typical: f64, off: usize, current: bool) -> Candidate {
        Candidate {
            said: format!("{:?}", std::mem::discriminant(&rule.rule)),
            rule,
            current,
            fit: Some(GateScore {
                gate_id: "g".into(),
                gate: "g".into(),
                scored: 10,
                not_placed: 0,
                references: 0,
                typical_agreement: Some(typical),
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

    fn rules(of: &[(&str, GateRule)]) -> RuleStore {
        let mut rules = RuleStore::default();
        for (gate, rule) in of {
            rules.insert(RuleTarget::named(*gate), rule.clone());
        }
        rules
    }

    #[test]
    fn a_pick_can_be_taken_only_while_the_gate_has_the_rule_it_was_picked_against() {
        let picked = search(
            "CD69+",
            vec![
                candidate(valley(), 0.95, 0, false),
                candidate(band(), 0.7, 4, true),
            ],
        );
        assert_eq!(
            choice(&picked, &rules(&[("CD69+", band())])),
            Choice::Better(valley())
        );
        assert_eq!(
            choice(&picked, &rules(&[("CD69+", valley())])),
            Choice::InUse
        );
        let edited = rule(Rule::TailFraction(TailFractionRule::new((0.05, 0.1))));
        assert_eq!(
            choice(&picked, &rules(&[("CD69+", edited)])),
            Choice::Changed
        );
        assert_eq!(choice(&picked, &RuleStore::default()), Choice::Changed);
    }

    /// With no rule before, there is nothing to gain over: the pick is said
    /// against "no rule", listed last and still taken.
    #[test]
    fn a_pick_for_a_gate_that_had_no_rule_can_be_taken() {
        let searches = [
            search("CD25+", vec![candidate(valley(), 0.90, 1, false)]),
            search(
                "CD4+",
                vec![
                    candidate(valley(), 0.72, 5, false),
                    candidate(band(), 0.50, 9, true),
                ],
            ),
        ];
        let listed = lines(&searches, &rules(&[("CD4+", band())]), &ScoreSettings::default());
        assert_eq!(listed[1].gate, "CD25+");
        assert_eq!(listed[1].as_it_stands, "no rule");
        assert_eq!(listed[1].gain, None);
        assert_eq!(
            takeable(&listed),
            [
                (RuleTarget::named("CD4+"), valley()),
                (RuleTarget::named("CD25+"), valley())
            ]
        );
    }

    #[test]
    fn the_gates_where_the_best_does_most_better_come_first_and_all_that_do_better_can_be_taken() {
        let searches = [
            search(
                "CD25+",
                vec![
                    candidate(valley(), 0.90, 1, false),
                    candidate(band(), 0.85, 3, true),
                ],
            ),
            search("CD69+", vec![candidate(band(), 0.95, 0, true)]),
            search(
                "CD8+",
                vec![
                    candidate(valley(), 0.88, 1, false),
                    candidate(band(), 0.88, 2, true),
                ],
            ),
            search(
                "CD4+",
                vec![
                    candidate(valley(), 0.72, 5, false),
                    candidate(band(), 0.50, 9, true),
                ],
            ),
        ];
        let now = rules(&[
            ("CD25+", band()),
            ("CD69+", band()),
            ("CD4+", band()),
            ("CD8+", band()),
        ]);
        let listed = lines(&searches, &now, &ScoreSettings::default());
        let gates: Vec<&str> = listed.iter().map(|l| l.gate.as_str()).collect();
        assert_eq!(gates, ["CD4+", "CD25+", "CD69+", "CD8+"]);
        assert_eq!(listed[3].choice, Choice::Better(valley()), "tied, so takeable alone");
        assert_eq!(listed[0].as_it_stands, "typical 0.50, 9 off");
        assert!(!listed[0].fits_well, "0.72 is below the off line of 0.8");
        assert!(listed[1].fits_well);
        assert_eq!(listed[2].choice, Choice::InUse);
        assert_eq!(
            takeable(&listed),
            [
                (RuleTarget::named("CD4+"), valley()),
                (RuleTarget::named("CD25+"), valley())
            ]
        );
    }
}
