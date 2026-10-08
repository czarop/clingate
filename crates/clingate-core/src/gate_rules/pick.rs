//! Picking, for each gate, the rule that comes closest to the gating drawn
//! by hand - which kind of rule as well as its settings.
//!
//! Trying every setting of every kind would be slow, so it goes in two
//! stages over one reading of the files. First each kind that can place the
//! gate is tried once, starting where the hand gating says it should: a band
//! around what the hand-drawn gates hold, a valley or the negative read on
//! each sample. Then the settings of the best kind are searched - and of the
//! second best too, when it came within [`REFINE_WITHIN`]. Everything tried
//! is ranked together, as [`crate::gate_rules::fit`] ranks a search.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::gate_rules::autogate::{Measurement, gated_of_each_specimen};
use crate::gate_rules::fit::{
    Candidate, FitSettings, Found, Readings, default_candidates, ranked_fit, solve_each,
    without_repeats,
};
use crate::gate_rules::rule::{
    AboveTheNegativeRule, BandAim, NegativeFinder, Pool, Rule, TailFractionRule, ValleyOrSmearRule,
    ValleyRule,
};
use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::RunInputs;
use crate::gate_rules::score::{ScoreSettings, Solved, median};
use crate::gates::GateState;
use crate::omiq::metadata::MetaDataFileMap;

/// A second kind of rule has its settings searched too when its typical
/// agreement came this close to the best kind's.
pub const REFINE_WITHIN: f64 = 0.05;
/// A band is started this far either side of what the hand gates hold, as a
/// share of it.
const BAND_SPREAD: (f64, f64) = (0.8, 1.25);

/// How far a pick has got, for the progress line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Picking {
    Reading {
        done: usize,
        total: usize,
    },
    /// Each kind of rule tried once.
    Kinds {
        done: usize,
        total: usize,
    },
    /// The best kinds' settings searched.
    Settings {
        done: usize,
        total: usize,
    },
}

impl Picking {
    /// How far through its own step the pick is.
    pub fn fraction(self) -> f64 {
        match self {
            Picking::Reading { done, total }
            | Picking::Kinds { done, total }
            | Picking::Settings { done, total }
                if total > 0 =>
            {
                done as f64 / total as f64
            }
            _ => 0.0,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Picking::Reading { done, total } => {
                format!("Step 1 of 3 - reading file {done} of {total}")
            }
            Picking::Kinds { done, total } => {
                format!("Step 2 of 3 - trying each kind of rule: {done} of {total}")
            }
            Picking::Settings { done, total } => {
                format!("Step 3 of 3 - searching the best kinds' settings: {done} of {total}")
            }
        }
    }
}

/// What `rule` would read beyond the hand-drawn line of each file a run
/// gates - on that file itself, its partner or a named file - typically, as a
/// share of what it reads. None when no gated file has a line drawn by hand.
pub(crate) fn held_by_hand(
    rule: &GateRule,
    measured: &[Measurement],
    store: &RuleStore,
    metadata: &MetaDataFileMap,
) -> Option<f64> {
    let gated = gated_of_each_specimen(&store.pairing, measured, metadata);
    let shares = gated.values().filter_map(|&at| {
        let gated = &measured[at];
        let line = gated.line.as_ref()?;
        let read_on = store.reference_file(&gated.file, &rule.measured_on, metadata)?;
        let read = measured
            .iter()
            .find(|m| m.file == read_on && m.gate_id == gated.gate_id)?;
        let values = &read.line.as_ref()?.values;
        (!values.is_empty())
            .then(|| rule.admitted(values, line.current) as f64 / values.len() as f64)
    });
    median(shares.collect())
}

/// A band rule read on `measured_on`, aimed at the middle of a band around
/// `held` - what the hand gates hold there.
fn band_around(current: &GateRule, measured_on: MeasuredOn, held: f64) -> Option<GateRule> {
    let pool = match &current.rule {
        Rule::TailFraction(band) => band.pool,
        _ => Pool::default(),
    };
    (held > 0.0).then(|| GateRule {
        measured_on,
        rule: Rule::TailFraction(TailFractionRule {
            pool,
            ..TailFractionRule::aimed(
                (held * BAND_SPREAD.0, (held * BAND_SPREAD.1).min(1.0)),
                BandAim::Middle,
            )
        }),
        ..current.clone()
    })
}

/// One of each kind of rule that moves `current`'s edge, started from the
/// hand gating: a band around what the hand gates hold on what `current`
/// reads, `held_there`, and on each sample itself, `held_itself`; the
/// negative read two ways; and the valley, with or without a smear, on each
/// sample. What `current` falls back to is kept.
pub(crate) fn kinds_to_try(
    current: &GateRule,
    held_there: Option<f64>,
    held_itself: Option<f64>,
) -> Vec<GateRule> {
    let on = |measured_on: MeasuredOn, rule: Rule| GateRule {
        measured_on,
        rule,
        ..current.clone()
    };
    let (fallback, smear_example) = match &current.rule {
        Rule::InTheValley(valley) => (valley.fallback.clone(), None),
        Rule::ValleyOrSmear(valley) => (valley.fallback.clone(), valley.smear_example.clone()),
        _ => (None, None),
    };
    let mut kinds = vec![current.clone()];
    kinds.extend(
        held_there.and_then(|held| band_around(current, current.measured_on.clone(), held)),
    );
    if current.measured_on != MeasuredOn::Itself {
        kinds.extend(held_itself.and_then(|held| band_around(current, MeasuredOn::Itself, held)));
    }
    for find in NegativeFinder::ALL {
        kinds.push(on(
            current.measured_on.clone(),
            Rule::AboveTheNegative(AboveTheNegativeRule {
                find,
                ..AboveTheNegativeRule::default()
            }),
        ));
    }
    kinds.push(on(
        MeasuredOn::Itself,
        Rule::InTheValley(ValleyRule {
            fallback: fallback.clone(),
            ..ValleyRule::default()
        }),
    ));
    kinds.push(on(
        MeasuredOn::Itself,
        Rule::ValleyOrSmear(ValleyOrSmearRule {
            fallback,
            smear_example,
            ..ValleyOrSmearRule::default()
        }),
    ));
    without_repeats(kinds)
}

/// Whether `a` and `b` are the same kind of rule, read on the same sample.
fn same_kind(a: &GateRule, b: &GateRule) -> bool {
    std::mem::discriminant(&a.rule) == std::mem::discriminant(&b.rule)
        && a.measured_on == b.measured_on
}

fn typical(candidate: &Candidate) -> Option<f64> {
    candidate.fit.as_ref()?.typical_agreement
}

/// How much higher the typical agreement of the best of `candidates`, ranked
/// best first, is than the rule as it stands.
pub fn gain(candidates: &[Candidate]) -> Option<f64> {
    let best = typical(candidates.first()?)?;
    let current = typical(candidates.iter().find(|c| c.current)?)?;
    Some(best - current)
}

/// Whether the best of `candidates` matches the hand gating well: typically
/// at or above the line a sample is off below. When it does not, nothing
/// tried does, and the gate may be better gated by hand.
pub fn fits_well(candidates: &[Candidate], settings: &ScoreSettings) -> bool {
    candidates
        .first()
        .and_then(typical)
        .is_some_and(|best| best >= settings.off_below)
}

/// The kinds whose settings are searched, from the kinds tried ranked best
/// first: the best, and the next kind too when its typical agreement is
/// within [`REFINE_WITHIN`] of the best's.
pub(crate) fn to_refine(ranked: &[Candidate]) -> Vec<&GateRule> {
    let Some(best) = ranked.first() else {
        return Vec::new();
    };
    let mut chosen = vec![&best.rule];
    let next = ranked.iter().find(|c| !same_kind(&c.rule, &best.rule));
    if let Some((next, (best_typical, next_typical))) =
        next.and_then(|next| Some((next, (typical(best)?, typical(next)?))))
        && best_typical - next_typical <= REFINE_WITHIN
    {
        chosen.push(&next.rule);
    }
    chosen
}

/// The rule `target` has now, if a pick can replace it.
fn pickable<'r>(rules: &'r RuleStore, target: &RuleTarget) -> Result<&'r GateRule, String> {
    let rule = rules
        .get(target)
        .ok_or("no rule places this gate - a pick starts from the rule it has")?;
    if rule.rule.reads_another_gate() {
        return Err(
            "this gate takes its place from another gate - there is nothing to pick".into(),
        );
    }
    Ok(rule)
}

/// The first stage's rules for `current`: one of each kind - or, for a
/// phenotype rule, its own settings, the only kind that can place its gate.
fn first_stage(
    current: &GateRule,
    measured: Option<&[Measurement]>,
    inputs: &RunInputs,
) -> Vec<GateRule> {
    if matches!(current.rule, Rule::MatchThePhenotype(_)) {
        return without_repeats(
            std::iter::once(current.clone())
                .chain(default_candidates(current))
                .collect(),
        );
    }
    let held = |measured_on: MeasuredOn| {
        let reading = GateRule {
            measured_on,
            ..current.clone()
        };
        held_by_hand(&reading, measured?, &inputs.rules, &inputs.metadata)
    };
    kinds_to_try(
        current,
        held(current.measured_on.clone()),
        held(MeasuredOn::Itself),
    )
}

/// Pick the best rule for each of `targets` - its kind and its settings -
/// against the gates as drawn, the files read once for all of them: each
/// rule scored by `settings` and ranked as `fit` says, everything tried
/// ranked together. A gate that cannot be picked for says why in its place;
/// `progress` hears how far the pick has got. Moves nothing.
pub fn pick_rules(
    gates: &GateState,
    inputs: &RunInputs,
    targets: &[RuleTarget],
    settings: ScoreSettings,
    fit: FitSettings,
    cancel: &AtomicBool,
    progress: impl Fn(Picking) + Sync,
) -> Result<Found, String> {
    let settings = settings.checked()?;
    let fit = fit.checked()?;
    let stopped = || cancel.load(Ordering::Relaxed);
    let currents: Vec<Result<&GateRule, String>> = targets
        .iter()
        .map(|target| pickable(&inputs.rules, target))
        .collect();
    let read_for: Vec<(&RuleTarget, &GateRule)> = targets
        .iter()
        .zip(&currents)
        .filter_map(|(target, current)| Some((target, *current.as_ref().ok()?)))
        .collect();
    let readings = Readings::read(gates, inputs, &read_for, cancel, |done, total| {
        progress(Picking::Reading { done, total })
    });
    if stopped() {
        return Err("stopped".into());
    }

    let first: Vec<Vec<GateRule>> = read_for
        .iter()
        .map(|&(target, current)| {
            let measured = readings.of(target, current).map(|(m, _)| m.as_slice());
            first_stage(current, measured, inputs)
        })
        .collect();
    let first_solved = solve_stage(
        gates,
        inputs,
        &readings,
        &read_for,
        &first,
        settings,
        cancel,
        |done, total| progress(Picking::Kinds { done, total }),
    );
    if stopped() {
        return Err("stopped".into());
    }

    let second: Vec<Vec<GateRule>> = read_for
        .iter()
        .zip(&first)
        .zip(&first_solved)
        .map(|((&(target, current), tried), scored)| {
            if matches!(current.rule, Rule::MatchThePhenotype(_)) {
                return Vec::new();
            }
            let kinds = ranked_fit(target, tried, Some(current), scored, fit, Vec::new(), 0);
            to_refine(&kinds.candidates)
                .into_iter()
                .flat_map(default_candidates)
                .filter(|rule| !tried.contains(rule))
                .collect::<Vec<_>>()
        })
        .map(without_repeats)
        .collect();
    let second_solved = solve_stage(
        gates,
        inputs,
        &readings,
        &read_for,
        &second,
        settings,
        cancel,
        |done, total| progress(Picking::Settings { done, total }),
    );
    if stopped() {
        return Err("stopped".into());
    }

    let mut picked = read_for
        .iter()
        .zip(first.into_iter().zip(second))
        .zip(first_solved.into_iter().zip(second_solved))
        .map(
            |((&(target, current), (first, second)), (first_solved, second_solved))| {
                let tried: Vec<GateRule> = first.into_iter().chain(second).collect();
                let scored: Vec<Solved> = first_solved.into_iter().chain(second_solved).collect();
                let found = ranked_fit(
                    target,
                    &tried,
                    Some(current),
                    &scored,
                    fit,
                    readings.problems.clone(),
                    inputs.files.len(),
                );
                (target.clone(), found)
            },
        );
    Ok(targets
        .iter()
        .zip(currents)
        .map(|(target, current)| match current {
            Ok(_) => {
                let (_, found) = picked.next().expect("a pick for every gate read for");
                (target.clone(), Ok(found))
            }
            Err(why) => (target.clone(), Err(why)),
        })
        .collect())
}

/// Each target's `rules` solved, one stage of a pick: a list of what was
/// solved for each target, in the order given.
#[allow(clippy::too_many_arguments)]
fn solve_stage(
    gates: &GateState,
    inputs: &RunInputs,
    readings: &Readings,
    targets: &[(&RuleTarget, &GateRule)],
    rules: &[Vec<GateRule>],
    settings: ScoreSettings,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> Vec<Vec<Solved>> {
    let jobs: Vec<(&RuleTarget, &GateRule)> = targets
        .iter()
        .zip(rules)
        .flat_map(|(&(target, _), rules)| rules.iter().map(move |rule| (target, rule)))
        .collect();
    let mut solved =
        solve_each(gates, inputs, readings, &jobs, settings, cancel, progress).into_iter();
    rules
        .iter()
        .map(|rules| solved.by_ref().take(rules.len()).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gate_rules::rule::PhenotypeRule;
    use crate::gate_rules::rule_store::Bound;
    use crate::gate_rules::score::GateScore;

    fn current(measured_on: MeasuredOn, rule: Rule) -> GateRule {
        GateRule {
            parameter: Arc::from("CD69"),
            bound: Bound::Above,
            measured_on,
            rule,
        }
    }

    fn fmx() -> MeasuredOn {
        MeasuredOn::Partner(Arc::from("FMX"))
    }

    fn band_of(rule: &GateRule) -> Option<((f64, f64), BandAim)> {
        match &rule.rule {
            Rule::TailFraction(band) => Some((band.band, band.aim)),
            _ => None,
        }
    }

    #[test]
    fn each_kind_is_started_where_the_hand_gating_says() {
        let valley = current(
            fmx(),
            Rule::InTheValley(ValleyRule {
                fallback: Some(RuleTarget::named("CD69+ of CD8+")),
                ..ValleyRule::default()
            }),
        );
        let kinds = kinds_to_try(&valley, Some(0.004), Some(0.2));
        assert_eq!(kinds[0], valley, "the rule as it stands first");
        let bands: Vec<(&MeasuredOn, ((f64, f64), BandAim))> = kinds
            .iter()
            .filter_map(|k| Some((&k.measured_on, band_of(k)?)))
            .collect();
        assert_eq!(
            bands,
            [
                (&fmx(), ((0.004 * 0.8, 0.004 * 1.25), BandAim::Middle)),
                (
                    &MeasuredOn::Itself,
                    ((0.2 * 0.8, 0.2 * 1.25), BandAim::Middle)
                ),
            ]
        );
        let finders: Vec<NegativeFinder> = kinds
            .iter()
            .filter_map(|k| match &k.rule {
                Rule::AboveTheNegative(above) => Some(above.find),
                _ => None,
            })
            .collect();
        assert_eq!(finders, NegativeFinder::ALL);
        let smear = kinds
            .iter()
            .find_map(|k| match &k.rule {
                Rule::ValleyOrSmear(v) => Some((v.fallback.clone(), &k.measured_on)),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            smear,
            (
                Some(RuleTarget::named("CD69+ of CD8+")),
                &MeasuredOn::Itself
            ),
            "the fallback kept, the valley read on each sample"
        );
        assert_eq!(kinds.len(), 7, "{kinds:#?}");
    }

    #[test]
    fn no_band_is_started_where_the_hand_gates_hold_nothing_known() {
        let itself = current(
            MeasuredOn::Itself,
            Rule::TailFraction(TailFractionRule::new((0.1, 0.2))),
        );
        let kinds = kinds_to_try(&itself, Some(0.0), None);
        let bands = kinds.iter().filter(|k| band_of(k).is_some()).count();
        assert_eq!(bands, 1, "only the rule as it stands");
        let kinds = kinds_to_try(&itself, Some(0.3), Some(0.3));
        let bands = kinds.iter().filter(|k| band_of(k).is_some()).count();
        assert_eq!(
            bands, 2,
            "a rule read on each sample is not tried there twice"
        );
    }

    #[test]
    fn a_band_never_starts_beyond_the_whole_parent() {
        let itself = current(MeasuredOn::Itself, Rule::InTheValley(ValleyRule::default()));
        let kinds = kinds_to_try(&itself, Some(0.9), None);
        let band = kinds.iter().find_map(band_of).unwrap();
        assert_eq!(band.0, (0.9 * 0.8, 1.0));
    }

    #[test]
    fn a_phenotype_rule_is_tried_only_with_its_own_settings() {
        let phenotype = current(
            MeasuredOn::File(Arc::from("qc")),
            Rule::MatchThePhenotype(PhenotypeRule::default()),
        );
        let inputs = RunInputs {
            files: Vec::new(),
            compensation: Default::default(),
            names: Default::default(),
            cofactors: Vec::new(),
            metadata: Default::default(),
            rules: RuleStore::default(),
        };
        let tried = first_stage(&phenotype, None, &inputs);
        assert!(
            tried
                .iter()
                .all(|r| matches!(r.rule, Rule::MatchThePhenotype(_)))
        );
        assert_eq!(
            tried.len(),
            9,
            "three fits, three shares kept, the rule as it stands among them"
        );
    }

    fn ranked(kinds: &[(Rule, MeasuredOn, f64)]) -> Vec<Candidate> {
        kinds
            .iter()
            .map(|(rule, measured_on, typical)| Candidate {
                rule: current(measured_on.clone(), rule.clone()),
                said: String::new(),
                current: false,
                fit: Some(GateScore {
                    gate_id: "g".into(),
                    gate: "g".into(),
                    scored: 10,
                    not_placed: 0,
                    references: 0,
                    typical_agreement: Some(*typical),
                    lowest_agreement: None,
                    off: 0,
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
                among_best: false,
                placed: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn the_gain_is_the_best_over_the_rule_as_it_stands() {
        let band = Rule::TailFraction(TailFractionRule::new((0.1, 0.2)));
        let mut candidates = ranked(&[
            (band.clone(), MeasuredOn::Itself, 0.9),
            (band.clone(), fmx(), 0.6),
        ]);
        assert_eq!(gain(&candidates), None, "no rule as it stands");
        candidates[1].current = true;
        assert!((gain(&candidates).unwrap() - 0.3).abs() < 1e-12);
        candidates[0].current = true;
        assert_eq!(
            gain(&candidates),
            Some(0.0),
            "the best is the rule as it stands"
        );
    }

    #[test]
    fn the_best_fits_well_at_or_above_the_off_line() {
        let band = Rule::TailFraction(TailFractionRule::new((0.1, 0.2)));
        let settings = ScoreSettings {
            off_below: 0.8,
            noise_widths: 1.0,
        };
        assert!(fits_well(
            &ranked(&[(band.clone(), MeasuredOn::Itself, 0.8)]),
            &settings
        ));
        assert!(!fits_well(
            &ranked(&[(band.clone(), MeasuredOn::Itself, 0.79)]),
            &settings
        ));
        assert!(!fits_well(&[], &settings));
    }

    #[test]
    fn the_next_kind_is_refined_too_only_when_it_came_close() {
        let valley = Rule::InTheValley(ValleyRule::default());
        let smoother = Rule::InTheValley(ValleyRule {
            smoothing: 2.0,
            ..ValleyRule::default()
        });
        let band = Rule::TailFraction(TailFractionRule::new((0.1, 0.2)));
        let close = ranked(&[
            (valley.clone(), MeasuredOn::Itself, 0.95),
            (smoother.clone(), MeasuredOn::Itself, 0.94),
            (band.clone(), MeasuredOn::Itself, 0.91),
        ]);
        let refined: Vec<&Rule> = to_refine(&close).iter().map(|r| &r.rule).collect();
        assert_eq!(
            refined,
            [&valley, &band],
            "the next kind, not the same kind again"
        );

        let far = ranked(&[
            (valley.clone(), MeasuredOn::Itself, 0.95),
            (band.clone(), MeasuredOn::Itself, 0.89),
        ]);
        assert_eq!(to_refine(&far).len(), 1);

        let read_elsewhere = ranked(&[
            (band.clone(), MeasuredOn::Itself, 0.95),
            (band.clone(), fmx(), 0.95),
        ]);
        assert_eq!(
            to_refine(&read_elsewhere).len(),
            2,
            "a band read on the FMX is another kind"
        );
    }
}
