//! Searching one gate's rule settings for those that come closest to the
//! gating drawn by hand.
//!
//! Each candidate is run as the scorer runs a rule - alone, under the gates
//! as drawn, on the full files - and summed up the same way. The files are
//! read once, and candidates that measure a population alike solve on one
//! measurement. A setting chosen on the samples it is judged by can be tuned
//! to a couple of them, so the specimens are split in two, alternately: the
//! candidates are ranked on one half and checked on the other.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::gate_rules::autogate::{describe, measured_alike};
use crate::gate_rules::rule::{
    AboveTheNegativeRule, BandAim, NegativeFinder, PercentileOffsetRule, PhenotypeRule, Rule,
    ShapeFit, TailFractionRule, ValleyOrSmearRule, ValleyRule,
};
use crate::gate_rules::rule_store::{GateRule, RuleStore, RuleTarget, human_order};
use crate::gate_rules::run::{Measured, Progress, RunInputs, measure_many};
use crate::gate_rules::score::{
    GateScore, PlacedGate, ScoreRow, ScoreSettings, Solved, least_agreeing_first, solved_rows,
    summarise,
};
use crate::gate_rules::trial::only;
use crate::gates::GateState;

/// The most candidates tried at once.
pub const MOST_CANDIDATES: usize = 64;
/// Fewer specimens than this are not split: half of them is too few to rank on.
pub const FEWEST_TO_SPLIT: usize = 8;

/// Smoothings tried for a valley rule.
const SMOOTHINGS: [f64; 5] = [0.5, 0.75, 1.0, 1.5, 2.0];
/// Shallowest dips tried for a valley rule, as a share of the lower peak.
const SMALLEST_DIPS: [Option<f64>; 3] = [None, Some(0.1), Some(0.25)];
/// What a band rule's band is multiplied by.
const BAND_SCALES: [f64; 5] = [0.5, 0.75, 1.0, 1.5, 2.0];
/// What an above-the-negative rule's distance is multiplied by.
const DISTANCE_SCALES: [f64; 5] = [0.8, 0.9, 1.0, 1.1, 1.25];
/// Percentiles tried for a percentile rule, beside its own.
const PERCENTILES: [f64; 3] = [95.0, 99.0, 99.9];
/// What a percentile rule's offset is multiplied by.
const OFFSET_SCALES: [f64; 3] = [0.5, 1.0, 1.5];
/// Shares of the matched cells tried for a phenotype rule's gate to hold.
const KEEPS: [f64; 3] = [0.9, 0.95, 0.99];

/// Every combination of the valley settings tried.
fn valley_settings() -> impl Iterator<Item = (f64, bool, Option<f64>)> {
    SMOOTHINGS.into_iter().flat_map(|smoothing| {
        [false, true].into_iter().flat_map(move |lowest_before| {
            SMALLEST_DIPS
                .into_iter()
                .map(move |smallest_dip| (smoothing, lowest_before, smallest_dip))
        })
    })
}

/// The settings of `rule`'s kind tried when none are given.
fn settings_tried(rule: &Rule) -> Vec<Rule> {
    match rule {
        Rule::TailFraction(band) => BAND_SCALES
            .into_iter()
            .filter(|scale| band.band.0 * scale < 1.0)
            .flat_map(|scale| {
                BandAim::ALL.map(|aim| {
                    Rule::TailFraction(TailFractionRule {
                        band: (band.band.0 * scale, (band.band.1 * scale).min(1.0)),
                        aim,
                        ..band.clone()
                    })
                })
            })
            .collect(),
        Rule::PercentileOffset(percentile) => PERCENTILES
            .into_iter()
            .chain([percentile.percentile])
            .flat_map(|at| {
                OFFSET_SCALES.map(|scale| {
                    Rule::PercentileOffset(PercentileOffsetRule {
                        percentile: at,
                        offset: percentile.offset * scale,
                        ..percentile.clone()
                    })
                })
            })
            .collect(),
        Rule::AboveTheNegative(above) => DISTANCE_SCALES
            .into_iter()
            .flat_map(|scale| {
                NegativeFinder::ALL.map(|find| {
                    Rule::AboveTheNegative(AboveTheNegativeRule {
                        scale: above.scale * scale,
                        find,
                        ..above.clone()
                    })
                })
            })
            .collect(),
        Rule::InTheValley(valley) => valley_settings()
            .map(|(smoothing, lowest_before, smallest_dip)| {
                Rule::InTheValley(ValleyRule {
                    smoothing,
                    lowest_before,
                    smallest_dip,
                    ..valley.clone()
                })
            })
            .collect(),
        Rule::ValleyOrSmear(valley) => valley_settings()
            .map(|(smoothing, lowest_before, smallest_dip)| {
                Rule::ValleyOrSmear(ValleyOrSmearRule {
                    smoothing,
                    lowest_before,
                    smallest_dip,
                    ..valley.clone()
                })
            })
            .collect(),
        Rule::MatchThePhenotype(phenotype) => ShapeFit::ALL
            .into_iter()
            .flat_map(|fit| {
                KEEPS.map(|keep| {
                    Rule::MatchThePhenotype(PhenotypeRule {
                        fit,
                        keep,
                        ..phenotype.clone()
                    })
                })
            })
            .collect(),
        Rule::FromAnotherGate(_) | Rule::NextToGate(_) => vec![rule.clone()],
    }
}

/// The candidates tried for `rule` when none are given: every combination of
/// a few values of each setting that matters for its kind, with what it reads,
/// its reference, markers and fallback unchanged. A rule that takes its place
/// from another gate has nothing to try but itself.
pub fn default_candidates(rule: &GateRule) -> Vec<GateRule> {
    without_repeats(
        settings_tried(&rule.rule)
            .into_iter()
            .map(|kind| GateRule {
                rule: kind,
                ..rule.clone()
            })
            .collect(),
    )
}

/// `rules` with each one only once, in the order first given.
pub fn without_repeats(rules: Vec<GateRule>) -> Vec<GateRule> {
    let mut kept: Vec<GateRule> = Vec::with_capacity(rules.len());
    for rule in rules {
        if !kept.contains(&rule) {
            kept.push(rule);
        }
    }
    kept
}

/// What ranks the candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankBy {
    /// The highest typical agreement; among those tied with it, the fewest
    /// samples off.
    #[default]
    Typical,
    /// The fewest samples off; among those, the highest typical agreement.
    Off,
}

/// How the candidates are ranked.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FitSettings {
    pub rank_by: RankBy,
    /// Typical agreements no further apart than this are a tie.
    pub tie_within: f64,
    /// Rank on half the specimens and check on the other half.
    pub split: bool,
}

impl Default for FitSettings {
    fn default() -> Self {
        Self {
            rank_by: RankBy::Typical,
            tie_within: 0.02,
            split: true,
        }
    }
}

impl FitSettings {
    /// Refused unless `tie_within` is an agreement.
    pub fn checked(self) -> Result<Self, String> {
        if !(0.0..=1.0).contains(&self.tie_within) {
            return Err(format!(
                "tie_within is a difference in agreement, 0 to 1 - not {}",
                self.tie_within
            ));
        }
        Ok(self)
    }
}

/// One candidate, ranked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub rule: GateRule,
    /// The rule in a line.
    pub said: String,
    /// The rule the workspace has now.
    pub current: bool,
    /// Summed up on the specimens ranked on - all of them, when not split -
    /// and on those it is checked on.
    pub fit: Option<GateScore>,
    pub check: Option<GateScore>,
    /// Its place, 1 first: by typical agreement and by samples off on the
    /// specimens ranked on, and by the ordering asked for on those checked on.
    pub place_by_typical: usize,
    pub place_by_off: usize,
    pub place_on_check: Option<usize>,
    /// The best, or tied with it: worth looking at side by side.
    pub among_best: bool,
    /// Where it puts the gate on each sample it moves it on - kept only for
    /// the best, those tied with it and the rule as it stands, the ones to
    /// look at.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placed: Vec<PlacedGate>,
}

/// Every candidate for one gate, best first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fit {
    pub gate: String,
    pub files_read: usize,
    /// Files that could not be read or measured, and candidates refused before
    /// any sample was read.
    pub problems: Vec<String>,
    /// The specimens - or files, for a sample with none - ranked on and
    /// checked on; none checked on when not split.
    pub fit_on: Vec<String>,
    pub checked_on: Vec<String>,
    pub rank_by: RankBy,
    pub candidates: Vec<Candidate>,
}

/// The specimen a row is split by, or its file when it has none.
fn split_key(row: &ScoreRow) -> &str {
    row.specimen.as_deref().unwrap_or(&row.file)
}

/// The specimens in `rows`, in order, dealt alternately into two halves - or
/// all into the first when `split` is off or there are too few to split.
fn halves<'r>(rows: impl Iterator<Item = &'r ScoreRow>, split: bool) -> (Vec<String>, Vec<String>) {
    let unique: BTreeSet<&str> = rows.map(split_key).collect();
    let mut specimens: Vec<String> = unique.into_iter().map(str::to_string).collect();
    specimens.sort_by(|a, b| human_order(a, b));
    if !split || specimens.len() < FEWEST_TO_SPLIT {
        return (specimens, Vec::new());
    }
    let (mut fit_on, mut checked_on) = (Vec::new(), Vec::new());
    for (at, specimen) in specimens.into_iter().enumerate() {
        if at % 2 == 0 {
            fit_on.push(specimen);
        } else {
            checked_on.push(specimen);
        }
    }
    (fit_on, checked_on)
}

/// `rows` summed up over the specimens `on` names, or nothing when it names
/// none of them.
fn summed_on(rows: &[ScoreRow], on: &[String]) -> Option<GateScore> {
    let mut half: Vec<ScoreRow> = rows
        .iter()
        .filter(|row| on.iter().any(|specimen| specimen == split_key(row)))
        .cloned()
        .collect();
    half.sort_by(least_agreeing_first);
    let gate_id = half.first()?.gate_id.clone();
    Some(summarise(gate_id, &half))
}

fn typical(score: Option<&GateScore>) -> f64 {
    score
        .and_then(|s| s.typical_agreement)
        .unwrap_or(f64::NEG_INFINITY)
}

/// Samples off, every one of them for a candidate with nothing scored.
fn off(score: Option<&GateScore>) -> usize {
    score
        .filter(|s| s.typical_agreement.is_some())
        .map_or(usize::MAX, |s| s.off)
}

/// Fewest off first, then the highest typical agreement.
fn fewest_off_first(a: Option<&GateScore>, b: Option<&GateScore>) -> std::cmp::Ordering {
    off(a).cmp(&off(b)).then(typical(b).total_cmp(&typical(a)))
}

/// Whether `other` is tied with `leader` on typical agreement.
fn tied(leader: Option<&GateScore>, other: Option<&GateScore>, tie_within: f64) -> bool {
    typical(leader) - typical(other) <= tie_within
}

/// The indices of `scores`, best first by `rank_by`, and how many at the
/// front are the best or tied with it.
fn ranked(scores: &[Option<&GateScore>], rank_by: RankBy, tie_within: f64) -> (Vec<usize>, usize) {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    match rank_by {
        RankBy::Off => {
            order.sort_by(|&a, &b| fewest_off_first(scores[a], scores[b]));
            let best = scores[order[0]];
            let among_best = order
                .iter()
                .take_while(|&&i| off(scores[i]) == off(best) && tied(best, scores[i], tie_within))
                .count();
            (order, among_best.max(1))
        }
        RankBy::Typical => {
            order.sort_by(|&a, &b| typical(scores[b]).total_cmp(&typical(scores[a])));
            let mut grouped = Vec::with_capacity(order.len());
            let mut among_best = 0;
            let mut rest = order.as_slice();
            while let Some(&leader) = rest.first() {
                let group = rest
                    .iter()
                    .take_while(|&&i| tied(scores[leader], scores[i], tie_within))
                    .count()
                    .max(1);
                let mut tied_group = rest[..group].to_vec();
                tied_group.sort_by(|&a, &b| fewest_off_first(scores[a], scores[b]));
                if grouped.is_empty() {
                    among_best = group;
                }
                grouped.extend(tied_group);
                rest = &rest[group..];
            }
            (grouped, among_best)
        }
    }
}

/// Where `candidate` stands in `order`, 1 first.
fn place_of(order: &[usize], candidate: usize) -> usize {
    order
        .iter()
        .position(|&i| i == candidate)
        .map_or(0, |at| at + 1)
}

/// The files read once for many rules: one measurement for each set of a
/// target's rules that measure alike.
pub(crate) struct Readings {
    shapes: Vec<(RuleTarget, GateRule)>,
    measured: Vec<Measured>,
    /// Files that could not be read or measured.
    pub(crate) problems: Vec<String>,
}

impl Readings {
    /// Read the files once for each of `rules`, as its target's rule;
    /// `progress` hears how many files are done.
    pub(crate) fn read(
        gates: &GateState,
        inputs: &RunInputs,
        rules: &[Job<'_>],
        cancel: &AtomicBool,
        progress: impl Fn(usize, usize) + Sync,
    ) -> Self {
        let mut shapes: Vec<(RuleTarget, GateRule)> = Vec::new();
        for &(target, rule) in rules {
            if !shapes
                .iter()
                .any(|(read, shape)| read == target && measured_alike(shape, rule))
            {
                shapes.push((target.clone(), rule.clone()));
            }
        }
        let stores: Vec<RuleStore> = shapes
            .iter()
            .map(|(target, rule)| only(&inputs.rules, target, rule))
            .collect();
        let (measured, problems) = if stores.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            measure_many(
                gates,
                &inputs.files,
                &inputs.compensation,
                &inputs.names,
                &inputs.cofactors,
                &inputs.metadata,
                &stores,
                None,
                cancel,
                progress,
            )
        };
        Self {
            shapes,
            measured,
            problems,
        }
    }

    /// What `rule` solves on as `target`'s rule, if the files were read for it.
    pub(crate) fn of(&self, target: &RuleTarget, rule: &GateRule) -> Option<&Measured> {
        self.reading_of(target, rule)
            .and_then(|at| self.measured.get(at))
    }

    fn reading_of(&self, target: &RuleTarget, rule: &GateRule) -> Option<usize> {
        self.shapes
            .iter()
            .position(|(read, shape)| read == target && measured_alike(shape, rule))
    }

    /// The densities `rule` reads as `target`'s rule: which reading, and at
    /// what smoothing.
    fn densities_of(&self, target: &RuleTarget, rule: &GateRule) -> Vec<Densities> {
        let Some(reading) = self.reading_of(target, rule) else {
            return Vec::new();
        };
        rule.rule
            .smoothings_read()
            .into_iter()
            .map(|smoothing| Densities {
                reading,
                smoothing: smoothing.to_bits(),
            })
            .collect()
    }

    /// Each line in `densities`'s reading, the values a density is worked
    /// out on.
    fn lines(&self, densities: Densities) -> impl Iterator<Item = &[f64]> {
        self.measured
            .get(densities.reading)
            .into_iter()
            .flat_map(|(measurements, _)| measurements)
            .filter_map(|measured| measured.line.as_ref())
            .map(|line| line.values.as_slice())
    }

    /// Work out every density in `densities` side by side, so no solve
    /// waits while another works out the one it needs.
    fn work_out(&self, densities: &[Densities]) {
        let lines: Vec<(&[f64], f64)> = densities
            .iter()
            .flat_map(|&d| {
                self.lines(d)
                    .map(move |values| (values, f64::from_bits(d.smoothing)))
            })
            .collect();
        lines.par_iter().for_each(|&(values, smoothing)| {
            crate::gate_rules::density::smoothed(values, smoothing);
        });
    }
}

/// A rule, as its target's.
type Job<'a> = (&'a RuleTarget, &'a GateRule);

/// The densities of one reading's lines at one smoothing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Densities {
    reading: usize,
    smoothing: u64,
}

/// Jobs solved together, and the densities they read.
#[derive(Debug, Default, PartialEq)]
struct Batch {
    jobs: std::ops::Range<usize>,
    densities: Vec<Densities>,
}

/// The most densities worked out ahead of a batch of jobs: half of those
/// kept, so that few are forgotten before they are read.
const DENSITIES_AHEAD: usize = crate::gate_rules::density::MOST_KEPT / 2;

/// Jobs, in order, cut into runs reading no more than `most` densities -
/// `read[job]` names a job's, and `lines` how many each names - each with
/// what it reads; a job reading more than `most` alone.
fn batches(read: &[Vec<Densities>], lines: impl Fn(Densities) -> usize, most: usize) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut batch = Batch::default();
    let mut counted = 0;
    for (job, densities) in read.iter().enumerate() {
        let unread = |batch: &Batch| {
            let mut new: Vec<Densities> = Vec::new();
            for &d in densities {
                if !batch.densities.contains(&d) && !new.contains(&d) {
                    new.push(d);
                }
            }
            let more: usize = new.iter().map(|&d| lines(d)).sum();
            (new, more)
        };
        let (mut new, mut more) = unread(&batch);
        if counted + more > most && !batch.jobs.is_empty() {
            batches.push(std::mem::take(&mut batch));
            batch.jobs = job..job;
            counted = 0;
            (new, more) = unread(&batch);
        }
        batch.jobs.end = job + 1;
        batch.densities.extend(new);
        counted += more;
    }
    batches.push(batch);
    batches
}

/// Each of `jobs` - a rule, as its target's - solved on what `readings` read
/// and read back against the gates as drawn, in the order given; `progress`
/// hears how many are done.
pub(crate) fn solve_each(
    gates: &GateState,
    inputs: &RunInputs,
    readings: &Readings,
    jobs: &[Job<'_>],
    settings: ScoreSettings,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> Vec<Solved> {
    let done = AtomicUsize::new(0);
    let solve = |&(target, rule): &Job<'_>| {
        let solved = match readings.of(target, rule) {
            Some(measured) => {
                let store = only(&inputs.rules, target, rule);
                solved_rows(gates, &inputs.metadata, &store, measured, settings, cancel)
            }
            None => Solved {
                rows: Vec::new(),
                refused: vec![format!(
                    "{}: the files were not read for this rule",
                    target.describe()
                )],
                placed: Vec::new(),
            },
        };
        progress(done.fetch_add(1, Ordering::Relaxed) + 1, jobs.len());
        solved
    };
    let mut solved = Vec::with_capacity(jobs.len());
    let read: Vec<Vec<Densities>> = jobs
        .iter()
        .map(|&(target, rule)| readings.densities_of(target, rule))
        .collect();
    let lines = |densities| readings.lines(densities).count();
    for batch in batches(&read, lines, DENSITIES_AHEAD) {
        readings.work_out(&batch.densities);
        solved.par_extend(jobs[batch.jobs].par_iter().map(solve));
    }
    solved
}

/// Each search's target, and what it found or why it could not be made.
pub type Found = Vec<(RuleTarget, Result<Fit, String>)>;

/// One rule's search: its target, and the candidates tried beside the rule
/// as it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub target: RuleTarget,
    pub candidates: Vec<GateRule>,
}

/// The rules tried for `ask`: the rule as it stands, if any, then its
/// candidates, each once.
fn tried_for(base: &RuleStore, ask: &Ask) -> Result<Vec<GateRule>, String> {
    let current = base.get(&ask.target);
    let tried = without_repeats(
        current
            .into_iter()
            .chain(&ask.candidates)
            .cloned()
            .collect(),
    );
    if tried.is_empty() {
        return Err("no candidate rule to try".into());
    }
    if tried.len() > MOST_CANDIDATES {
        return Err(format!(
            "at most {MOST_CANDIDATES} candidates at once - {} given",
            tried.len()
        ));
    }
    Ok(tried)
}

/// `scored`, one for each of `tried` in turn, ranked as `fit` says.
pub(crate) fn ranked_fit(
    target: &RuleTarget,
    tried: &[GateRule],
    current: Option<&GateRule>,
    scored: &[Solved],
    fit: FitSettings,
    mut problems: Vec<String>,
    files_read: usize,
) -> Fit {
    for refused in scored.iter().flat_map(|solved| &solved.refused) {
        if !problems.contains(refused) {
            problems.push(refused.clone());
        }
    }
    let (fit_on, checked_on) = halves(scored.iter().flat_map(|solved| &solved.rows), fit.split);
    let fits: Vec<Option<GateScore>> = scored
        .iter()
        .map(|solved| summed_on(&solved.rows, &fit_on))
        .collect();
    let checks: Vec<Option<GateScore>> = scored
        .iter()
        .map(|solved| summed_on(&solved.rows, &checked_on))
        .collect();
    let on_fit: Vec<Option<&GateScore>> = fits.iter().map(Option::as_ref).collect();
    let on_check: Vec<Option<&GateScore>> = checks.iter().map(Option::as_ref).collect();
    let (by_typical, among_typical) = ranked(&on_fit, RankBy::Typical, fit.tie_within);
    let (by_off, among_off) = ranked(&on_fit, RankBy::Off, fit.tie_within);
    let checked =
        (!checked_on.is_empty()).then(|| ranked(&on_check, fit.rank_by, fit.tie_within).0);
    let (order, among_best) = match fit.rank_by {
        RankBy::Typical => (by_typical.clone(), among_typical),
        RankBy::Off => (by_off.clone(), among_off),
    };

    let candidates = order
        .iter()
        .enumerate()
        .map(|(at, &i)| {
            let is_current = current == Some(&tried[i]);
            let among_best = at < among_best;
            Candidate {
                rule: tried[i].clone(),
                said: tried[i].rule.describe(),
                current: is_current,
                fit: fits[i].clone(),
                check: checks[i].clone(),
                place_by_typical: place_of(&by_typical, i),
                place_by_off: place_of(&by_off, i),
                place_on_check: checked.as_ref().map(|order| place_of(order, i)),
                among_best,
                placed: if among_best || is_current {
                    scored[i].placed.clone()
                } else {
                    Vec::new()
                },
            }
        })
        .collect();
    Fit {
        gate: describe(&target.gate, target.parent.as_deref()),
        files_read,
        problems,
        fit_on,
        checked_on,
        rank_by: fit.rank_by,
        candidates,
    }
}

/// Search each of `asks`, the files read once for all of them: every
/// candidate scored against the gates as drawn by `settings`, ranked as `fit`
/// says. A search that cannot be made says why in its place; `progress` hears
/// how far the reading and the trying have got. Moves nothing.
pub fn fit_rules(
    gates: &GateState,
    inputs: &RunInputs,
    asks: &[Ask],
    settings: ScoreSettings,
    fit: FitSettings,
    cancel: &AtomicBool,
    progress: impl Fn(Progress) + Sync,
) -> Result<Found, String> {
    let settings = settings.checked()?;
    let fit = fit.checked()?;
    let tried: Vec<Result<Vec<GateRule>, String>> = asks
        .iter()
        .map(|ask| tried_for(&inputs.rules, ask))
        .collect();
    let jobs: Vec<Job<'_>> = asks
        .iter()
        .zip(&tried)
        .flat_map(|(ask, rules)| rules.iter().flatten().map(move |rule| (&ask.target, rule)))
        .collect();
    let readings = Readings::read(gates, inputs, &jobs, cancel, |done, total| {
        progress(Progress::Measuring { done, total })
    });
    if cancel.load(Ordering::Relaxed) {
        return Err("stopped".into());
    }
    let solved = solve_each(
        gates,
        inputs,
        &readings,
        &jobs,
        settings,
        cancel,
        |done, total| progress(Progress::Solving { done, total }),
    );
    if cancel.load(Ordering::Relaxed) {
        return Err("stopped".into());
    }

    let mut solved = solved.into_iter();
    Ok(asks
        .iter()
        .zip(tried)
        .map(|(ask, rules)| {
            let found = rules.map(|rules| {
                let scored: Vec<Solved> = solved.by_ref().take(rules.len()).collect();
                ranked_fit(
                    &ask.target,
                    &rules,
                    inputs.rules.get(&ask.target),
                    &scored,
                    fit,
                    readings.problems.clone(),
                    inputs.files.len(),
                )
            });
            (ask.target.clone(), found)
        })
        .collect())
}

/// Try the rule `target` has now, if any, and each of `candidates` as its
/// rule - see [`fit_rules`].
pub fn fit_rule(
    gates: &GateState,
    inputs: &RunInputs,
    target: &RuleTarget,
    candidates: &[GateRule],
    settings: ScoreSettings,
    fit: FitSettings,
    cancel: &AtomicBool,
) -> Result<Fit, String> {
    let ask = Ask {
        target: target.clone(),
        candidates: candidates.to_vec(),
    };
    let (_, found) = fit_rules(gates, inputs, &[ask], settings, fit, cancel, |_| {})?
        .into_iter()
        .next()
        .ok_or("no search was made")?;
    found
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gate_rules::rule::{FromGateRule, Meet, NextToRule, Pool, Side};
    use crate::gate_rules::rule_store::{Bound, MeasuredOn};

    fn read_at(reading: usize) -> Densities {
        Densities {
            reading,
            smoothing: 1.0f64.to_bits(),
        }
    }

    fn cut(read: &[Vec<Densities>], most: usize) -> Vec<(std::ops::Range<usize>, Vec<usize>)> {
        let lines = |d: Densities| if d.reading == 9 { 10 } else { 3 };
        batches(read, lines, most)
            .into_iter()
            .map(|b| (b.jobs, b.densities.iter().map(|d| d.reading).collect()))
            .collect()
    }

    #[test]
    fn jobs_are_batched_in_order_by_the_densities_they_read() {
        let (a, b, c, big) = (read_at(0), read_at(1), read_at(2), read_at(9));
        assert_eq!(cut(&[], 6), [(0..0, vec![])]);
        assert_eq!(
            cut(&[vec![], vec![], vec![]], 6),
            [(0..3, vec![])],
            "jobs reading no density go together"
        );
        assert_eq!(
            cut(&[vec![a], vec![a], vec![b], vec![]], 6),
            [(0..4, vec![0, 1])],
            "a density two jobs read counts once"
        );
        assert_eq!(
            cut(&[vec![a], vec![b], vec![c]], 6),
            [(0..2, vec![0, 1]), (2..3, vec![2])],
            "a batch ends before it would read more than the most"
        );
        assert_eq!(
            cut(&[vec![a], vec![big], vec![b]], 6),
            [(0..1, vec![0]), (1..2, vec![9]), (2..3, vec![1])],
            "a job reading more than the most goes alone"
        );
        assert_eq!(
            cut(&[vec![a], vec![b], vec![b, c]], 6),
            [(0..2, vec![0, 1]), (2..3, vec![1, 2])],
            "a density read before a cut is read again after it"
        );
        assert_eq!(
            cut(&[vec![a, a], vec![b]], 6),
            [(0..2, vec![0, 1])],
            "a density a job names twice counts once"
        );
    }

    fn on_x(rule: Rule) -> GateRule {
        GateRule {
            parameter: Arc::from("CD69"),
            bound: Bound::Above,
            measured_on: MeasuredOn::Partner(Arc::from("FMX")),
            rule,
        }
    }

    fn valley() -> Rule {
        Rule::InTheValley(ValleyRule {
            fallback: Some(RuleTarget::under("CD69+", "CD8+")),
            ..ValleyRule::default()
        })
    }

    fn phenotype(markers: &[&str]) -> Rule {
        Rule::MatchThePhenotype(PhenotypeRule {
            markers: markers.iter().map(|m| Arc::from(*m)).collect(),
            ..PhenotypeRule::default()
        })
    }

    fn next_to() -> Rule {
        Rule::NextToGate(NextToRule {
            anchor: RuleTarget::named("CD69-"),
            parameter: Arc::from("CD69"),
            side: Side::Upper,
            meet: Meet::default(),
            gap: 0.0,
        })
    }

    #[test]
    fn a_valley_rule_tries_every_combination_of_its_settings() {
        let rule = on_x(valley());
        let tried = default_candidates(&rule);
        assert_eq!(tried.len(), 5 * 2 * 3);
        let settings: Vec<(f64, bool, Option<f64>)> = tried
            .iter()
            .map(|candidate| match &candidate.rule {
                Rule::InTheValley(v) => {
                    assert_eq!(v.fallback, Some(RuleTarget::under("CD69+", "CD8+")));
                    (v.smoothing, v.lowest_before, v.smallest_dip)
                }
                other => panic!("{other:?}"),
            })
            .collect();
        for wanted in [
            (0.5, true, Some(0.25)),
            (2.0, false, None),
            (1.0, true, Some(0.1)),
        ] {
            assert_eq!(
                settings.iter().filter(|s| **s == wanted).count(),
                1,
                "{wanted:?}"
            );
        }
        assert!(tried.iter().all(|c| c.parameter == rule.parameter
            && c.bound == rule.bound
            && c.measured_on == rule.measured_on));
    }

    #[test]
    fn a_band_is_tried_narrower_and_wider_aimed_both_ways() {
        let rule = on_x(Rule::TailFraction(TailFractionRule {
            pool: Pool::Run,
            ..TailFractionRule::new((0.002, 0.005))
        }));
        let tried = default_candidates(&rule);
        assert_eq!(tried.len(), 5 * 2);
        let bands: Vec<((f64, f64), BandAim, Pool)> = tried
            .iter()
            .map(|c| match &c.rule {
                Rule::TailFraction(t) => (t.band, t.aim, t.pool),
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(bands.contains(&((0.001, 0.0025), BandAim::Middle, Pool::Run)));
        assert!(bands.contains(&((0.004, 0.01), BandAim::AnywhereInBand, Pool::Run)));
    }

    #[test]
    fn a_band_is_never_tried_beyond_the_whole_parent() {
        let rule = on_x(Rule::TailFraction(TailFractionRule::new((0.6, 0.9))));
        let tried = default_candidates(&rule);
        // 0.6 doubled is past the whole parent; the other four scales are not.
        assert_eq!(tried.len(), 4 * 2);
        for candidate in &tried {
            let Rule::TailFraction(t) = &candidate.rule else {
                panic!()
            };
            assert!(t.band.0 < 1.0 && t.band.1 <= 1.0, "{t:?}");
        }
    }

    #[test]
    fn the_distance_above_the_negative_is_tried_by_both_finders() {
        let rule = on_x(Rule::AboveTheNegative(AboveTheNegativeRule {
            scale: 2.0,
            nudge: 30.0,
            ..AboveTheNegativeRule::default()
        }));
        let tried = default_candidates(&rule);
        assert_eq!(tried.len(), 5 * 2);
        let mut scales: Vec<f64> = tried
            .iter()
            .map(|c| match &c.rule {
                Rule::AboveTheNegative(a) => {
                    assert_eq!(a.nudge, 30.0);
                    a.scale
                }
                other => panic!("{other:?}"),
            })
            .collect();
        scales.sort_by(f64::total_cmp);
        scales.dedup();
        assert_eq!(scales, vec![1.6, 1.8, 2.0, 2.2, 2.5]);
    }

    #[test]
    fn a_percentile_with_no_offset_is_tried_once_per_percentile() {
        let rule = on_x(Rule::PercentileOffset(PercentileOffsetRule::new(97.5, 0.0)));
        let percentiles: Vec<f64> = default_candidates(&rule)
            .iter()
            .map(|c| match &c.rule {
                Rule::PercentileOffset(p) => p.percentile,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(percentiles, vec![95.0, 99.0, 99.9, 97.5]);
    }

    #[test]
    fn a_phenotype_is_tried_with_each_fit_and_share_kept_on_its_own_markers() {
        let rule = on_x(phenotype(&["CD161", "TCR Va7.2"]));
        let tried = default_candidates(&rule);
        assert_eq!(tried.len(), 3 * 3);
        for candidate in &tried {
            let Rule::MatchThePhenotype(p) = &candidate.rule else {
                panic!()
            };
            assert_eq!(p.markers.len(), 2);
        }
    }

    #[test]
    fn a_rule_placed_from_another_gate_has_only_itself_to_try() {
        let rule = on_x(next_to());
        assert_eq!(default_candidates(&rule), vec![rule]);
    }

    #[test]
    fn rules_measure_alike_when_they_read_the_same_edge_or_markers() {
        let band = on_x(Rule::TailFraction(TailFractionRule::new((0.01, 0.02))));
        assert!(measured_alike(&band, &on_x(valley())));
        let below = GateRule {
            bound: Bound::Below,
            ..band.clone()
        };
        assert!(!measured_alike(&band, &below));
        let other_marker = GateRule {
            parameter: Arc::from("CD25"),
            ..band.clone()
        };
        assert!(!measured_alike(&band, &other_marker));

        let mait = on_x(phenotype(&["CD161", "TCR Va7.2"]));
        assert!(measured_alike(
            &mait,
            &on_x(phenotype(&["CD161", "TCR Va7.2"]))
        ));
        assert!(!measured_alike(&mait, &on_x(phenotype(&["CD161"]))));
        assert!(!measured_alike(&mait, &band));

        let follows = on_x(Rule::FromAnotherGate(FromGateRule {
            same_shape_as: Some(RuleTarget::named("CD69+")),
            edges: Vec::new(),
        }));
        assert!(measured_alike(&follows, &on_x(next_to())));
        assert!(!measured_alike(&follows, &band));
    }

    fn standing(typical: Option<f64>, off: usize) -> GateScore {
        GateScore {
            gate_id: "cd69".into(),
            gate: "CD69+".into(),
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
        }
    }

    fn rank(scores: &[GateScore], rank_by: RankBy, tie_within: f64) -> (Vec<usize>, usize) {
        let scores: Vec<Option<&GateScore>> = scores.iter().map(Some).collect();
        ranked(&scores, rank_by, tie_within)
    }

    #[test]
    fn by_typical_agreement_a_tie_goes_to_the_fewest_off() {
        let scores = [
            standing(Some(0.92), 3),
            standing(Some(0.91), 0),
            standing(Some(0.80), 0),
        ];
        assert_eq!(rank(&scores, RankBy::Typical, 0.02), (vec![1, 0, 2], 2));
        assert_eq!(rank(&scores, RankBy::Typical, 0.0), (vec![0, 1, 2], 1));
    }

    #[test]
    fn by_samples_off_the_fewest_come_first_then_the_highest_typical() {
        let scores = [
            standing(Some(0.92), 3),
            standing(Some(0.80), 0),
            standing(Some(0.91), 0),
        ];
        assert_eq!(rank(&scores, RankBy::Off, 0.02), (vec![2, 1, 0], 1));
        let close = [
            standing(Some(0.90), 0),
            standing(Some(0.91), 0),
            standing(Some(0.5), 0),
        ];
        assert_eq!(rank(&close, RankBy::Off, 0.02), (vec![1, 0, 2], 2));
    }

    #[test]
    fn a_candidate_with_nothing_scored_is_last() {
        let scores = [standing(None, 0), standing(Some(0.5), 4)];
        for rank_by in [RankBy::Typical, RankBy::Off] {
            assert_eq!(rank(&scores, rank_by, 0.02).0, vec![1, 0], "{rank_by:?}");
        }
        let unscored: Vec<Option<&GateScore>> = vec![None, None];
        assert_eq!(ranked(&unscored, RankBy::Typical, 0.02).0.len(), 2);
    }

    #[test]
    fn a_tie_wider_than_any_agreement_is_refused() {
        let wide = FitSettings {
            tie_within: 1.5,
            ..FitSettings::default()
        };
        assert!(wide.checked().is_err());
        assert!(FitSettings::default().checked().is_ok());
    }

    fn row(specimen: Option<&str>, file: &str) -> ScoreRow {
        ScoreRow {
            gate_id: "cd69".into(),
            gate: "CD69+".into(),
            file: file.into(),
            specimen: specimen.map(str::to_string),
            sample_type: None,
            what: "moved",
            events: None,
            agreement: None,
            caught: None,
            extra: None,
            off_line: None,
            hand_holds: None,
            rule_holds: None,
            holds_difference: None,
            shift_iqrs: Vec::new(),
            hand_edge: None,
            rule_edge: None,
            edge_off_iqrs: None,
            confidence: None,
            why_not: None,
        }
    }

    #[test]
    fn specimens_are_dealt_alternately_into_two_halves_in_order() {
        let names = [
            "S10", "S2", "S1", "S9", "S3", "S4", "S8", "S5", "S7", "S6", "S2",
        ];
        let rows: Vec<ScoreRow> = names.iter().map(|s| row(Some(s), "f.fcs")).collect();
        let (fit_on, checked_on) = halves(rows.iter(), true);
        assert_eq!(fit_on, ["S1", "S3", "S5", "S7", "S9"]);
        assert_eq!(checked_on, ["S2", "S4", "S6", "S8", "S10"]);
    }

    #[test]
    fn too_few_specimens_or_no_split_asked_for_rank_on_them_all() {
        let rows: Vec<ScoreRow> = (1..=10)
            .map(|i| row(Some(&format!("S{i}")), "f.fcs"))
            .collect();
        let (all, none) = halves(rows.iter(), false);
        assert_eq!((all.len(), none.len()), (10, 0));
        let (all, none) = halves(rows[..FEWEST_TO_SPLIT - 1].iter(), true);
        assert_eq!((all.len(), none.len()), (FEWEST_TO_SPLIT - 1, 0));
    }

    #[test]
    fn a_sample_with_no_specimen_is_split_by_its_file() {
        let rows = [row(None, "a.fcs"), row(Some("D1"), "b.fcs")];
        assert_eq!(halves(rows.iter(), false).0, ["a.fcs", "D1"]);
    }
}
