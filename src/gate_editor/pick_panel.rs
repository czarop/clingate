//! Picking a rule for every gate against the gating drawn by hand, from the
//! Gate Rules tab - its kind as well as its settings, the kinds tried in the
//! user's order of preference until one passes their cut-off, as the tools
//! for Claude pick with `pick_rule` - and taking the picks. Close picks are
//! stepped through on the Gallery tab.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::prelude::*;

use clingate_core::gate_rules::fit::{Candidate, FitSettings};
use clingate_core::gate_rules::pick::{
    KindTried, PickSettings, Picking, gain, keep_settings, kept_settings,
};
use clingate_core::gate_rules::rule_store::{GateRule, RuleStore, RuleTarget};
use clingate_core::gate_rules::score::ScoreSettings;
use clingate_core::gate_rules::searches::{Search, keep_all, kept, pick_every_rule};

use crate::components::toast::{note, say, use_toast, warn};
use crate::gate_editor::gate_rules_window::{RulesRun, use_stop_run_on_change};
use crate::gate_editor::workspace_window::{GateStore, Loaded};

/// Where a gate's pick stands against the rules as they are now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Choice {
    /// The gate's rule is the pick.
    InUse,
    /// The gate still has the rule the pick was made against, and the pick
    /// is another: it can be taken.
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

/// The pick settings as typed: the FMX band's ends and the most samples off
/// in percent, the agreement as a share.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Typed {
    pub fmx_lowest: String,
    pub fmx_highest: String,
    pub agreement: String,
    pub most_off: String,
}

/// A share as a percentage, to a millionth of a percent.
fn percent(share: f64) -> String {
    format!("{}", (share * 100.0 * 1e6).round() / 1e6)
}

impl Typed {
    /// `settings` as the fields show them.
    pub(crate) fn of(settings: &PickSettings) -> Self {
        let (lowest, highest) = settings
            .fmx_band
            .map_or((String::new(), String::new()), |(lowest, highest)| {
                (percent(lowest), percent(highest))
            });
        Self {
            fmx_lowest: lowest,
            fmx_highest: highest,
            agreement: format!("{}", settings.score.off_below),
            most_off: percent(settings.most_off),
        }
    }

    /// The settings typed, keeping `kept`'s allowance for counting noise, or
    /// what is wrong with them. Both ends of the band left empty try no band
    /// on the FMX.
    pub(crate) fn settings(&self, kept: &PickSettings) -> Result<PickSettings, String> {
        let number = |typed: &str, what: &str| {
            typed
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("The {what} must be a number - not \"{typed}\""))
        };
        let fmx_band = match (self.fmx_lowest.trim(), self.fmx_highest.trim()) {
            ("", "") => None,
            ("", _) | (_, "") => {
                return Err("Give both ends of the FMX band, or neither".to_string());
            }
            (lowest, highest) => Some((
                number(lowest, "FMX band's lowest")? / 100.0,
                number(highest, "FMX band's highest")? / 100.0,
            )),
        };
        PickSettings {
            fmx_band,
            score: ScoreSettings {
                off_below: number(&self.agreement, "agreement")?,
                ..kept.score
            },
            most_off: number(&self.most_off, "share of samples off")? / 100.0,
        }
        .checked()
    }
}

/// How each kind of rule tried did, in a line.
fn tried_said(kinds: &[KindTried]) -> String {
    kinds
        .iter()
        .map(|kind| {
            let mut said = format!(
                "{}: {} of {} off",
                kind.kind.describe(),
                kind.off,
                kind.judged
            );
            if kind.passed {
                said.push_str(", passed");
            }
            said
        })
        .collect::<Vec<_>>()
        .join("; ")
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
    /// Whether the best passed the cut-off; false is only the closest.
    pub passed: Option<bool>,
    /// How each kind tried did.
    pub tried: String,
    pub choice: Choice,
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

/// The kept picks as lines against `rules`: those flagged first, then those
/// where the best does most better.
pub(crate) fn lines(searches: &[Search], rules: &RuleStore) -> Vec<Line> {
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
                passed: search.passed,
                tried: tried_said(&search.kinds),
                choice: choice(search, rules),
            })
        })
        .collect();
    lines.sort_by(|a, b| {
        let flagged = |line: &Line| line.passed == Some(false);
        let gain = |line: &Line| line.gain.unwrap_or(f64::NEG_INFINITY);
        flagged(b)
            .cmp(&flagged(a))
            .then(gain(b).total_cmp(&gain(a)))
    });
    lines
}

/// The picks that can be taken: each gate's best where the gate still has
/// the rule it was picked against.
pub(crate) fn takeable(lines: &[Line]) -> Vec<(RuleTarget, GateRule)> {
    lines
        .iter()
        .filter_map(|line| match &line.choice {
            Choice::Better(rule) => Some((line.target.clone(), rule.clone())),
            _ => None,
        })
        .collect()
}

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
    // Counted up after each pick, so the kept picks are read again.
    let mut picked = use_signal(|| 0u64);
    let searches = use_memo(move || {
        picked();
        loaded
            .read()
            .folder
            .as_deref()
            .map(|folder| kept(folder).unwrap_or_default())
            .unwrap_or_default()
    });
    let listed = use_memo(move || lines(&searches.read(), &rules.read()));
    let kept_in = use_memo(move || {
        loaded
            .read()
            .folder
            .as_deref()
            .map(kept_settings)
            .unwrap_or_default()
    });
    let mut typed = use_signal(|| Typed::of(&PickSettings::default()));
    use_effect(move || typed.set(Typed::of(&kept_in())));
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
            let settings = match typed.peek().settings(&kept_in.peek()) {
                Ok(settings) => settings,
                Err(why) => {
                    warn(&toasts, why);
                    return;
                }
            };
            if let Err(e) = keep_settings(&folder, &settings) {
                warn(
                    &toasts,
                    format!("The settings could not be kept for next time: {e}"),
                );
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
                        settings,
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
                "For every gate a rule places, tries the kinds of rule in order - a band read on the FMX at the range below; above the negative; the valley, or a smear; a band read on the sample; and last, the phenotype on the plot's two axes - and takes the first that passes, then searches its settings. The negative, the valley and the phenotype are calibrated on one sample you gated by hand. Every rule is scored against your gates, each gate under its parent as drawn. Nothing changes until you take a pick. A gate placed from another gate is left as it is."
            }
            label { "FMX band (%)" }
            div { class: "gate_rules-band",
                input {
                    r#type: "number",
                    step: "any",
                    value: "{typed.read().fmx_lowest}",
                    oninput: move |e| typed.write().fmx_lowest = e.value(),
                }
                "to"
                input {
                    r#type: "number",
                    step: "any",
                    value: "{typed.read().fmx_highest}",
                    oninput: move |e| typed.write().fmx_highest = e.value(),
                }
            }
            p { class: "gate_rules-hint gate_rules-span",
                "The share of each specimen's FMX events above the line you accept. Tried first, as it is. Leave both empty to skip it."
            }
            label { "Off below agreement" }
            input {
                r#type: "number",
                step: "0.01",
                value: "{typed.read().agreement}",
                oninput: move |e| typed.write().agreement = e.value(),
            }
            label { "Most samples off (%)" }
            input {
                r#type: "number",
                step: "1",
                value: "{typed.read().most_off}",
                oninput: move |e| typed.write().most_off = e.value(),
            }
            p { class: "gate_rules-hint gate_rules-span",
                "A rule passes when no more than this share of the samples agree less than the agreement with your gate. Kept with the workspace for every pick after, and used by Claude's picks too."
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
                    "Step through a gate's closest picks over your gate on the Gallery tab. A flagged pick passed nothing: it is only the closest of everything tried, and that gate may be better gated by hand. Point at a gate to see how each kind did."
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
                            tr { key: "{at}", title: "{line.tried}",
                                td { "{line.gate}" }
                                td {
                                    "{line.best}"
                                    if line.passed == Some(false) {
                                        span { class: "gate_rules-weak",
                                            " - flagged: nothing passed, the closest of all tried"
                                        }
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

    use clingate_core::gate_rules::rule::{Rule, TailFractionRule, ValleyOrSmearRule};
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
        rule(Rule::ValleyOrSmear(ValleyOrSmearRule::default()))
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
            passed: None,
            kinds: Vec::new(),
        }
    }

    fn picked(gate: &str, candidates: Vec<Candidate>, passed: bool) -> Search {
        Search {
            passed: Some(passed),
            ..search(gate, candidates)
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

    #[test]
    fn the_flagged_gates_come_first_then_where_the_best_does_most_better() {
        let searches = [
            picked(
                "CD25+",
                vec![
                    candidate(valley(), 0.90, 1, false),
                    candidate(band(), 0.85, 3, true),
                ],
                true,
            ),
            picked("CD69+", vec![candidate(band(), 0.95, 0, true)], true),
            picked(
                "CD4+",
                vec![
                    candidate(valley(), 0.72, 5, false),
                    candidate(band(), 0.50, 9, true),
                ],
                true,
            ),
            picked(
                "CD8+",
                vec![
                    candidate(valley(), 0.70, 6, false),
                    candidate(band(), 0.69, 6, true),
                ],
                false,
            ),
        ];
        let now = rules(&[
            ("CD25+", band()),
            ("CD69+", band()),
            ("CD4+", band()),
            ("CD8+", band()),
        ]);
        let listed = lines(&searches, &now);
        let gates: Vec<&str> = listed.iter().map(|l| l.gate.as_str()).collect();
        assert_eq!(gates, ["CD8+", "CD4+", "CD25+", "CD69+"]);
        assert_eq!(
            listed[0].passed,
            Some(false),
            "flagged, though it gains least"
        );
        assert_eq!(listed[1].as_it_stands, "typical 0.50, 9 off");
        assert_eq!(listed[3].choice, Choice::InUse);
        assert_eq!(
            takeable(&listed),
            [
                (RuleTarget::named("CD8+"), valley()),
                (RuleTarget::named("CD4+"), valley()),
                (RuleTarget::named("CD25+"), valley())
            ],
            "a flagged pick can still be taken"
        );
        assert_eq!(
            lines(
                &[search("CD69+", vec![candidate(band(), 0.95, 0, true)])],
                &now
            )[0]
            .passed,
            None,
            "a search of a rule's settings is no pick"
        );
    }

    #[test]
    fn each_kind_tried_is_said_in_the_line() {
        use clingate_core::gate_rules::pick::Kind;
        let tried = |kind, off, passed| KindTried {
            kind,
            searched: false,
            closest: String::new(),
            off,
            judged: 20,
            typical_agreement: None,
            passed,
        };
        let search = Search {
            kinds: vec![
                tried(Kind::FmxBand, 4, false),
                tried(Kind::ValleyOrSmear, 1, true),
            ],
            ..picked("CD69+", vec![candidate(band(), 0.95, 0, true)], true)
        };
        let line = &lines(&[search], &rules(&[("CD69+", band())]))[0];
        assert_eq!(
            line.tried,
            format!(
                "{}: 4 of 20 off; {}: 1 of 20 off, passed",
                Kind::FmxBand.describe(),
                Kind::ValleyOrSmear.describe()
            )
        );
    }

    fn typed(lowest: &str, highest: &str, agreement: &str, most_off: &str) -> Typed {
        Typed {
            fmx_lowest: lowest.into(),
            fmx_highest: highest.into(),
            agreement: agreement.into(),
            most_off: most_off.into(),
        }
    }

    #[test]
    fn the_settings_are_typed_in_percent_and_read_back_as_shares() {
        let kept = PickSettings {
            score: ScoreSettings {
                noise_widths: 2.0,
                ..PickSettings::default().score
            },
            ..PickSettings::default()
        };
        let read = typed("0.5", "1", "0.9", "20").settings(&kept).unwrap();
        assert_eq!(read.fmx_band, Some((0.005, 0.01)));
        assert_eq!((read.score.off_below, read.most_off), (0.9, 0.2));
        assert_eq!(read.score.noise_widths, 2.0, "the allowance kept");
        assert_eq!(Typed::of(&read), typed("0.5", "1", "0.9", "20"));
        assert_eq!(
            Typed::of(&PickSettings::default()),
            typed("", "", "0.95", "10"),
            "no band, 0.95, a tenth"
        );
        let no_band = typed(" ", "", "0.95", "10").settings(&kept).unwrap();
        assert_eq!(no_band.fmx_band, None);
    }

    #[test]
    fn settings_typed_wrong_say_what_is_wrong() {
        let kept = PickSettings::default();
        let wrong = |t: Typed| t.settings(&kept).unwrap_err();
        assert!(wrong(typed("0.5", "", "0.95", "10")).contains("both ends"));
        assert!(wrong(typed("half", "1", "0.95", "10")).contains("lowest"));
        assert!(wrong(typed("0.5", "1", "high", "10")).contains("agreement"));
        assert!(wrong(typed("0.5", "1", "0.95", "")).contains("samples off"));
        assert!(wrong(typed("2", "1", "0.95", "10")).contains("FMX band"));
        assert!(wrong(typed("", "", "0.95", "150")).contains("most_off"));
    }
}
