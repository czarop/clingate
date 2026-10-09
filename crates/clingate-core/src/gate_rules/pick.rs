//! Picking, for each gate, a rule close enough to the gating drawn by hand -
//! trying the kinds of rule in the order they are preferred, and taking the
//! first that is.
//!
//! Close enough is the user's to say ([`PickSettings`]): no more than a share
//! of the samples - a tenth, unless set - may agree less than an agreement -
//! [`AGREEMENT`], unless set - with the gate drawn by hand. The kinds are
//! tried in the order of [`Kind`]: a band read on each specimen's FMX at the
//! range the user accepts, as it is; above the negative; the valley or a
//! smear; a band read on the sample; and last, the phenotype on the plot's
//! two axes. The negative, the valley and the phenotype are calibrated, as
//! the Gate Rules tab makes them, on one sample gated by hand - the one the
//! rule as it stands is calibrated on, or else the specimen whose hand gate
//! holds the middle share of its parent - which is then not scored: read on
//! each sample itself, they would only find its own hand gate. Each is first
//! tried as the hand gating starts it, all together, and the first in order
//! that passes has its settings searched. When none passes as started, each
//! kind's settings are searched in turn until one passes. When none does,
//! the closest of everything tried is shown, flagged.
//!
//! The files are read once - and again only for the phenotype, the last
//! resort, for the gates that come to it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::gate_rules::autogate::{
    Measurement, NO_SMEAR_EXAMPLE, admitted_by, gated_of_each_specimen,
};
use crate::gate_rules::fit::{
    Candidate, Fit, FitSettings, Readings, default_candidates, halves_of, ranked_on, solve_each,
    without_repeats,
};
use crate::gate_rules::rule::{
    AboveTheNegativeRule, BandAim, PhenotypeRule, Pool, Rule, TailFractionRule, ValleyOrSmearRule,
};
use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget, human_order};
use crate::gate_rules::run::RunInputs;
use crate::gate_rules::score::{
    GateScore, ScoreSettings, Solved, least_agreeing_first, median, summarise,
};
use crate::gates::GateState;
use crate::omiq::metadata::MetaDataFileMap;

/// A sample agreeing less than this with its hand gate is off, unless set.
pub const AGREEMENT: f64 = 0.95;
/// A rule passes with no more than this share of its samples off, unless set.
pub const MOST_OFF: f64 = 0.1;
/// A band read on the sample is started this far either side of what the
/// hand gates hold, as a share of it.
const BAND_SPREAD: (f64, f64) = (0.8, 1.25);
/// The file a workspace's pick settings are kept in, in its rules folder.
const SETTINGS_FILE: &str = "pick_settings.json";

/// What a pick asks of a rule, and the band it tries first.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PickSettings {
    /// The lowest and highest share of each specimen's FMX events the user
    /// accepts above the line: a band read there is tried first, as it is.
    /// None tries no band on the FMX.
    pub fmx_band: Option<(f64, f64)>,
    /// How each sample is judged: one agreeing less than `off_below` with
    /// its hand gate is off.
    pub score: ScoreSettings,
    /// A rule passes with no more than this share of its samples off.
    pub most_off: f64,
}

impl Default for PickSettings {
    fn default() -> Self {
        Self {
            fmx_band: None,
            score: ScoreSettings {
                off_below: AGREEMENT,
                ..ScoreSettings::default()
            },
            most_off: MOST_OFF,
        }
    }
}

impl PickSettings {
    /// Refused unless the band and `most_off` are shares, the band's lowest
    /// at or below its highest and its highest above 0.
    pub fn checked(self) -> Result<Self, String> {
        self.score.checked()?;
        if !(0.0..=1.0).contains(&self.most_off) {
            return Err(format!(
                "most_off is a share of the samples, 0 to 1 - not {}",
                self.most_off
            ));
        }
        if let Some((lowest, highest)) = self.fmx_band
            && !(0.0 <= lowest && lowest <= highest && highest <= 1.0 && highest > 0.0)
        {
            return Err(format!(
                "the FMX band is two shares, 0 to 1, the first no higher than the second and \
                 the second above 0 - not {lowest} to {highest}"
            ));
        }
        Ok(self)
    }
}

/// Where `folder`'s pick settings are kept.
pub fn settings_file(folder: &Path) -> PathBuf {
    folder.join(crate::workspace::RULES_DIR).join(SETTINGS_FILE)
}

/// The pick settings kept in `folder`, or the defaults before any are.
pub fn kept_settings(folder: &Path) -> PickSettings {
    std::fs::read_to_string(settings_file(folder))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Keep `settings` in `folder`, for every pick after.
pub fn keep_settings(folder: &Path, settings: &PickSettings) -> anyhow::Result<()> {
    let path = settings_file(folder);
    crate::workspace::make_parent(&path)?;
    std::fs::write(path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

/// The kinds of rule a pick tries, in the order it prefers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A band read on each specimen's FMX, at the range the user accepts.
    FmxBand,
    AboveNegative,
    ValleyOrSmear,
    /// A band read on each sample itself, around what the hand gates hold.
    Band,
    /// The phenotype on the plot's two axes: the last resort.
    Phenotype,
}

impl Kind {
    pub fn describe(self) -> &'static str {
        match self {
            Kind::FmxBand => "a band read on the FMX, at the range given",
            Kind::AboveNegative => "above each sample's own negative, as far as on the reference",
            Kind::ValleyOrSmear => "in the valley, or on a smear as on one gated by hand",
            Kind::Band => "a band read on the sample itself",
            Kind::Phenotype => "the phenotype on the plot's two axes",
        }
    }

    /// Whether its settings are searched: all but the FMX band, which is
    /// tried at the range given.
    fn searched(self) -> bool {
        self != Kind::FmxBand
    }
}

/// How far a pick has got, for the progress line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Picking {
    Reading {
        done: usize,
        total: usize,
    },
    /// Each kind of rule tried as the hand gating starts it.
    Kinds {
        done: usize,
        total: usize,
    },
    /// A kind's settings searched.
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
            Picking::Reading { done, total } => format!("Reading file {done} of {total}"),
            Picking::Kinds { done, total } => {
                format!("Trying each kind of rule: {done} of {total}")
            }
            Picking::Settings { done, total } => {
                format!("Searching a kind's settings: {done} of {total}")
            }
        }
    }
}

/// A kind of rule, as tried for one gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindTried {
    pub kind: Kind,
    /// Its settings were searched, beside the rule it was first tried with.
    pub searched: bool,
    /// Its closest rule, in a line.
    pub closest: String,
    /// On how many of the samples it was judged on its closest was off.
    pub off: usize,
    pub judged: usize,
    pub typical_agreement: Option<f64>,
    /// One of its rules passes.
    pub passed: bool,
}

/// One gate's pick.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Picked {
    /// The rules of the kind picked that pass, best first, then the rule as
    /// it stands when it is not among them - or, when no kind passes,
    /// everything tried, closest first.
    pub fit: Fit,
    /// Whether the best passes. When not, it is only the closest.
    pub passed: bool,
    /// Each kind tried, in the order tried.
    pub kinds: Vec<KindTried>,
    /// How many rules were tried, of every kind.
    pub tried: usize,
}

/// Each gate's pick, or why it could not be made.
pub type Picks = Vec<(RuleTarget, Result<Picked, String>)>;

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

/// The file each specimen is gated on whose hand gate holds the middle
/// share of its parent, among `measured` - a typical one to calibrate on.
pub(crate) fn typical_gated(
    measured: &[Measurement],
    gates: &GateState,
    store: &RuleStore,
    metadata: &MetaDataFileMap,
) -> Option<Arc<str>> {
    let mut held: Vec<(f64, &Measurement)> =
        gated_of_each_specimen(&store.pairing, measured, metadata)
            .values()
            .filter_map(|&at| {
                let gated = &measured[at];
                let gate = gates.gate_for_file(&gated.gate_id, &gated.file, metadata)?;
                Some((admitted_by(&gate, &gated.index)?, gated))
            })
            .collect();
    held.sort_by(|(a, x), (b, y)| a.total_cmp(b).then(human_order(&x.file, &y.file)));
    held.get(held.len().saturating_sub(1) / 2)
        .map(|(_, gated)| gated.file.clone())
}

/// The partner a rule reads as the FMX: the one `current` reads, or the
/// sample type the pairing shows first, where it shows another after it.
fn fmx_of(current: &GateRule, store: &RuleStore) -> Option<MeasuredOn> {
    match &current.measured_on {
        MeasuredOn::Partner(kind) => Some(MeasuredOn::Partner(kind.clone())),
        _ => {
            let order = &store.pairing.display_order;
            (order.len() > 1).then(|| MeasuredOn::Partner(order[0].clone()))
        }
    }
}

/// A band rule read on each sample itself, aimed at the middle of a band
/// around `held` - what the hand gates hold there.
fn band_around(current: &GateRule, held: f64) -> Option<GateRule> {
    (held > 0.0).then(|| GateRule {
        measured_on: MeasuredOn::Itself,
        rule: Rule::TailFraction(TailFractionRule::aimed(
            (held * BAND_SPREAD.0, (held * BAND_SPREAD.1).min(1.0)),
            BandAim::Middle,
        )),
        ..current.clone()
    })
}

/// The kinds of rule a pick tries for a gate whose rule is `current`, in
/// order, each with the rule it is first tried with: the band on the FMX at
/// `fmx_band`, read on `fmx`; above the negative and the valley or a smear,
/// calibrated on `reference`, the valley keeping what `current` falls back to
/// and its smear example; a band read on each sample around `held_itself`, what the
/// hand gates hold there; and the phenotype on `axes`, described from
/// `reference`. A kind with nothing to start from is left out. A gate matched
/// by its phenotype is tried only so, as its rule stands.
pub(crate) fn first_tries(
    current: &GateRule,
    fmx_band: Option<(f64, f64)>,
    fmx: Option<&MeasuredOn>,
    reference: Option<&Arc<str>>,
    held_itself: Option<f64>,
    axes: Option<&(Arc<str>, Arc<str>)>,
) -> Vec<(Kind, GateRule)> {
    if matches!(current.rule, Rule::MatchThePhenotype(_)) {
        return vec![(Kind::Phenotype, current.clone())];
    }
    let on = |measured_on: MeasuredOn, rule: Rule| GateRule {
        measured_on,
        rule,
        ..current.clone()
    };
    let (fallback, smear_example) = match &current.rule {
        Rule::ValleyOrSmear(valley) => (valley.fallback.clone(), valley.smear_example.clone()),
        _ => (None, None),
    };
    let pool = match &current.rule {
        Rule::TailFraction(band) => band.pool,
        _ => Pool::default(),
    };
    let calibrated = reference.map(|file| MeasuredOn::File(file.clone()));
    let mut tries = Vec::new();
    if let (Some(band), Some(fmx)) = (fmx_band, fmx) {
        tries.push((
            Kind::FmxBand,
            on(
                fmx.clone(),
                Rule::TailFraction(TailFractionRule {
                    pool,
                    ..TailFractionRule::new(band)
                }),
            ),
        ));
    }
    if let Some(calibrated) = &calibrated {
        tries.push((
            Kind::AboveNegative,
            on(
                calibrated.clone(),
                Rule::AboveTheNegative(AboveTheNegativeRule::default()),
            ),
        ));
        tries.push((
            Kind::ValleyOrSmear,
            on(
                calibrated.clone(),
                Rule::ValleyOrSmear(ValleyOrSmearRule {
                    fallback,
                    smear_example,
                    ..ValleyOrSmearRule::default()
                }),
            ),
        ));
    }
    if let Some(band) = held_itself.and_then(|held| band_around(current, held)) {
        tries.push((Kind::Band, band));
    }
    if let (Some(calibrated), Some((x, y))) = (calibrated, axes) {
        tries.push((
            Kind::Phenotype,
            on(
                calibrated,
                Rule::MatchThePhenotype(PhenotypeRule {
                    markers: vec![x.clone(), y.clone()],
                    ..PhenotypeRule::default()
                }),
            ),
        ));
    }
    tries
}

/// The first sample, in order, a valley-or-smear rule met as a smear with no
/// smear gated by hand to place it from - the one a run would ask to be gated.
pub(crate) fn first_smear(solved: &Solved) -> Option<Arc<str>> {
    solved
        .rows
        .iter()
        .filter(|row| row.why_not.as_deref() == Some(NO_SMEAR_EXAMPLE))
        .map(|row| row.file.as_str())
        .min_by(|a, b| human_order(a, b))
        .map(Arc::from)
}

/// `solved` summed up over every sample it was judged on.
fn judged(solved: &Solved) -> Option<GateScore> {
    let mut rows = solved.rows.clone();
    rows.sort_by(least_agreeing_first);
    let gate_id = rows.first()?.gate_id.clone();
    Some(summarise(gate_id, &rows))
}

/// Whether a rule summed up as `score` passes: no more than `most_off` of
/// the samples it was judged on are off.
pub fn passes(score: &GateScore, most_off: f64) -> bool {
    let judged = score.scored + score.not_placed;
    // A share of a count lands just off a whole number: a tenth of 30 is
    // 3.0000000000000004.
    judged > 0 && score.off as f64 <= most_off * judged as f64 + 1e-9
}

/// Whether `solved`, summed up over every sample, passes.
fn solved_passes(solved: &Solved, most_off: f64) -> bool {
    judged(solved).is_some_and(|score| passes(&score, most_off))
}

/// How much higher the typical agreement of the best of `candidates`, ranked
/// best first, is than the rule as it stands.
pub fn gain(candidates: &[Candidate]) -> Option<f64> {
    let typical = |c: &Candidate| c.fit.as_ref()?.typical_agreement;
    let best = typical(candidates.first()?)?;
    let current = typical(candidates.iter().find(|c| c.current)?)?;
    Some(best - current)
}

/// Whether `a` and `b` are the same kind of rule, read on the same sample.
fn same_kind(a: &GateRule, b: &GateRule) -> bool {
    std::mem::discriminant(&a.rule) == std::mem::discriminant(&b.rule)
        && a.measured_on == b.measured_on
}

/// A kind of rule, as it goes for one gate.
struct Rung {
    kind: Kind,
    first: GateRule,
    searched: bool,
}

/// One gate's pick, as it goes.
struct Climb<'a> {
    target: &'a RuleTarget,
    current: &'a GateRule,
    rungs: Vec<Rung>,
    /// Every rule tried, and what it solved to.
    tried: Vec<(GateRule, Solved)>,
}

impl Climb<'_> {
    fn solved(&self, rule: &GateRule) -> Option<&Solved> {
        self.tried
            .iter()
            .find(|(tried, _)| tried == rule)
            .map(|(_, solved)| solved)
    }

    /// The rules tried of `rung`'s kind - its own, its settings and the rule
    /// as it stands, if alike - with what each solved to.
    fn of<'r>(&'r self, rung: &'r Rung) -> impl Iterator<Item = &'r (GateRule, Solved)> {
        self.tried
            .iter()
            .filter(move |(rule, _)| same_kind(rule, &rung.first))
    }

    /// The first kind, in order, one of whose rules passes.
    fn passing(&self, most_off: f64) -> Option<usize> {
        self.rungs.iter().position(|rung| {
            self.of(rung)
                .any(|(_, solved)| solved_passes(solved, most_off))
        })
    }

    /// The rules first tried of every kind but the phenotype - which reads
    /// the files again - unless it is the only kind; and the rule as it
    /// stands.
    fn first_tries(&self) -> Vec<GateRule> {
        let only = self.rungs.len() == 1;
        without_repeats(
            std::iter::once(self.current.clone())
                .chain(
                    self.rungs
                        .iter()
                        .filter(|rung| only || rung.kind != Kind::Phenotype)
                        .map(|rung| rung.first.clone()),
                )
                .collect(),
        )
    }

    /// The valley-or-smear rule with the first smear it met, unplaced for
    /// want of one, as its smear example - to be tried again so.
    fn with_smear_example(&mut self) -> Option<GateRule> {
        let at = self
            .rungs
            .iter()
            .position(|rung| rung.kind == Kind::ValleyOrSmear)?;
        let Rule::ValleyOrSmear(either) = self.rungs[at].first.rule.clone() else {
            return None;
        };
        if either.smear_example.is_some() {
            return None;
        }
        let example = first_smear(self.solved(&self.rungs[at].first)?)?;
        let rung = &mut self.rungs[at];
        rung.first.rule = Rule::ValleyOrSmear(ValleyOrSmearRule {
            smear_example: Some(example),
            ..either
        });
        Some(rung.first.clone())
    }

    /// The settings of the `at`th kind not tried yet, searched from now.
    fn search(&mut self, at: usize) -> Vec<GateRule> {
        self.rungs[at].searched = true;
        let first = &self.rungs[at].first;
        let wanted = std::iter::once(first.clone()).chain(default_candidates(first));
        without_repeats(wanted.filter(|rule| self.solved(rule).is_none()).collect())
    }

    /// The pick made: the kind that passed, or the closest of all.
    fn picked(
        self,
        most_off: f64,
        fit: FitSettings,
        problems: &[String],
        files_read: usize,
    ) -> Picked {
        let passing = self.passing(most_off);
        let split = halves_of(self.tried.iter().map(|(_, solved)| solved), fit);
        let ranked = |tried: &[&(GateRule, Solved)]| {
            let rules: Vec<GateRule> = tried.iter().map(|(rule, _)| rule.clone()).collect();
            let scored: Vec<&Solved> = tried.iter().map(|(_, solved)| solved).collect();
            ranked_on(
                self.target,
                &rules,
                Some(self.current),
                &scored,
                fit,
                split.clone(),
                problems.to_vec(),
                files_read,
            )
        };
        let tried = self.tried.len();
        let kinds = self
            .rungs
            .iter()
            .filter_map(|rung| kind_tried(rung, self.of(rung), most_off))
            .collect();
        let Some(at) = passing else {
            let everything: Vec<&(GateRule, Solved)> = self.tried.iter().collect();
            return Picked {
                fit: ranked(&everything),
                passed: false,
                kinds,
                tried,
            };
        };
        let contenders: Vec<&(GateRule, Solved)> = self
            .of(&self.rungs[at])
            .filter(|(_, solved)| solved_passes(solved, most_off))
            .collect();
        let mut fit = ranked(&contenders);
        if !fit.candidates.iter().any(|c| c.current)
            && let Some(current) = self.tried.iter().find(|(rule, _)| rule == self.current)
        {
            let with_current: Vec<&(GateRule, Solved)> =
                contenders.iter().copied().chain([current]).collect();
            let as_it_stands = ranked(&with_current)
                .candidates
                .into_iter()
                .find(|c| c.current)
                .map(|c| Candidate {
                    among_best: false,
                    ..c
                });
            fit.candidates.extend(as_it_stands);
        }
        Picked {
            fit,
            passed: true,
            kinds,
            tried,
        }
    }
}

/// How `rung`'s kind did, from its rules `tried`; nothing when none was.
fn kind_tried<'r>(
    rung: &Rung,
    tried: impl Iterator<Item = &'r (GateRule, Solved)>,
    most_off: f64,
) -> Option<KindTried> {
    let typical = |s: &GateScore| s.typical_agreement.unwrap_or(f64::NEG_INFINITY);
    let (rule, score) = tried
        .filter_map(|(rule, solved)| Some((rule, judged(solved)?)))
        .min_by(|(_, a), (_, b)| a.off.cmp(&b.off).then(typical(b).total_cmp(&typical(a))))?;
    Some(KindTried {
        kind: rung.kind,
        searched: rung.searched,
        closest: rule.rule.describe(),
        off: score.off,
        judged: score.scored + score.not_placed,
        typical_agreement: score.typical_agreement,
        passed: passes(&score, most_off),
    })
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

/// The climbs started: each gate's kinds of rule in order, from what was read
/// for its rule as it stands.
fn started<'a>(
    gates: &GateState,
    inputs: &RunInputs,
    readings: &Readings,
    read_for: &[(&'a RuleTarget, &'a GateRule)],
    settings: &PickSettings,
) -> Vec<Climb<'a>> {
    read_for
        .iter()
        .map(|&(target, current)| {
            let measured = readings
                .of(target, current)
                .map(|(m, _)| m.as_slice())
                .unwrap_or_default();
            let held_itself = held_by_hand(
                &GateRule {
                    measured_on: MeasuredOn::Itself,
                    ..current.clone()
                },
                measured,
                &inputs.rules,
                &inputs.metadata,
            );
            let reference = match &current.measured_on {
                MeasuredOn::File(file) => Some(file.clone()),
                _ => typical_gated(measured, gates, &inputs.rules, &inputs.metadata),
            };
            let axes = measured.first().map(|m| m.params.clone());
            let fmx = fmx_of(current, &inputs.rules);
            let rungs = first_tries(
                current,
                settings.fmx_band,
                fmx.as_ref(),
                reference.as_ref(),
                held_itself,
                axes.as_ref(),
            )
            .into_iter()
            .map(|(kind, first)| Rung {
                kind,
                first,
                searched: false,
            })
            .collect();
            Climb {
                target,
                current,
                rungs,
                tried: Vec::new(),
            }
        })
        .collect()
}

/// Solve the rules each climb wants, all together, and give each its own.
#[allow(clippy::too_many_arguments)]
fn solve_wanted(
    gates: &GateState,
    inputs: &RunInputs,
    readings: &Readings,
    climbs: &mut [Climb<'_>],
    wanted: Vec<Vec<GateRule>>,
    settings: ScoreSettings,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) {
    let jobs: Vec<(&RuleTarget, &GateRule)> = climbs
        .iter()
        .zip(&wanted)
        .flat_map(|(climb, rules)| rules.iter().map(move |rule| (climb.target, rule)))
        .collect();
    if jobs.is_empty() {
        return;
    }
    let mut solved =
        solve_each(gates, inputs, readings, &jobs, settings, cancel, progress).into_iter();
    for (climb, rules) in climbs.iter_mut().zip(wanted) {
        for rule in rules {
            let solved = solved.next().expect("a solve for every rule wanted");
            climb.tried.push((rule, solved));
        }
    }
}

/// Pick a rule for each of `targets` against the gates as drawn, trying the
/// kinds of rule in order until one passes as `settings` asks - see the
/// module's notes - each ranked as `fit` says. A gate that cannot be picked
/// for says why in its place; `progress` hears how far the pick has got.
/// Moves nothing.
pub fn pick_rules(
    gates: &GateState,
    inputs: &RunInputs,
    targets: &[RuleTarget],
    settings: PickSettings,
    fit: FitSettings,
    cancel: &AtomicBool,
    progress: impl Fn(Picking) + Sync,
) -> Result<Picks, String> {
    let settings = settings.checked()?;
    let fit = fit.checked()?;
    let stopped = || cancel.load(Ordering::Relaxed);
    let score = settings.score;
    let currents: Vec<Result<&GateRule, String>> = targets
        .iter()
        .map(|target| pickable(&inputs.rules, target))
        .collect();
    let read_for: Vec<(&RuleTarget, &GateRule)> = targets
        .iter()
        .zip(&currents)
        .filter_map(|(target, current)| Some((target, *current.as_ref().ok()?)))
        .collect();
    let reading = |done, total| progress(Picking::Reading { done, total });
    let mut readings = Readings::read(gates, inputs, &read_for, cancel, reading);
    if stopped() {
        return Err("stopped".into());
    }
    let mut climbs = started(gates, inputs, &readings, &read_for, &settings);

    let first: Vec<Vec<GateRule>> = climbs.iter().map(Climb::first_tries).collect();
    let kinds = |done, total| progress(Picking::Kinds { done, total });
    solve_wanted(
        gates,
        inputs,
        &readings,
        &mut climbs,
        first,
        score,
        cancel,
        kinds,
    );
    let smears: Vec<Vec<GateRule>> = climbs
        .iter_mut()
        .map(|climb| climb.with_smear_example().into_iter().collect())
        .collect();
    solve_wanted(
        gates,
        inputs,
        &readings,
        &mut climbs,
        smears,
        score,
        cancel,
        kinds,
    );
    if stopped() {
        return Err("stopped".into());
    }

    let searching = |done, total| progress(Picking::Settings { done, total });
    let picked_kind: Vec<Vec<GateRule>> = climbs
        .iter_mut()
        .map(|climb| match climb.passing(settings.most_off) {
            Some(at) if climb.rungs[at].kind.searched() => climb.search(at),
            _ => Vec::new(),
        })
        .collect();
    solve_wanted(
        gates,
        inputs,
        &readings,
        &mut climbs,
        picked_kind,
        score,
        cancel,
        searching,
    );

    let most_rungs = climbs.iter().map(|c| c.rungs.len()).max().unwrap_or(0);
    for at in 0..most_rungs {
        if stopped() {
            return Err("stopped".into());
        }
        let wanted: Vec<Vec<GateRule>> = climbs
            .iter_mut()
            .map(|climb| {
                let searchable = climb
                    .rungs
                    .get(at)
                    .is_some_and(|rung| rung.kind.searched() && !rung.searched);
                if searchable && climb.passing(settings.most_off).is_none() {
                    climb.search(at)
                } else {
                    Vec::new()
                }
            })
            .collect();
        let unread: Vec<(&RuleTarget, &GateRule)> = climbs
            .iter()
            .zip(&wanted)
            .flat_map(|(climb, rules)| rules.iter().map(move |rule| (climb.target, rule)))
            .collect();
        readings.read_more(gates, inputs, &unread, cancel, reading);
        solve_wanted(
            gates,
            inputs,
            &readings,
            &mut climbs,
            wanted,
            score,
            cancel,
            searching,
        );
    }
    if stopped() {
        return Err("stopped".into());
    }

    let mut picked = climbs.into_iter().map(|climb| {
        climb.picked(
            settings.most_off,
            fit,
            &readings.problems,
            inputs.files.len(),
        )
    });
    Ok(targets
        .iter()
        .zip(currents)
        .map(|(target, current)| match current {
            Ok(_) => {
                let picked = picked.next().expect("a pick for every gate read for");
                (target.clone(), Ok(picked))
            }
            Err(why) => (target.clone(), Err(why)),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_load_tests::scratch;
    use crate::gate_rules::rule::NegativeFinder;
    use crate::gate_rules::rule_store::{Bound, SamplePairing};
    use crate::gate_rules::score::{GateScore, agreeing_row, refused_row};

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

    fn kinds(tries: &[(Kind, GateRule)]) -> Vec<Kind> {
        tries.iter().map(|(kind, _)| *kind).collect()
    }

    fn rule_of(tries: &[(Kind, GateRule)], wanted: Kind) -> &GateRule {
        &tries.iter().find(|(kind, _)| *kind == wanted).unwrap().1
    }

    fn qc() -> Arc<str> {
        Arc::from("d2_fs")
    }

    fn axes() -> (Arc<str>, Arc<str>) {
        (Arc::from("CD69"), Arc::from("CD4"))
    }

    #[test]
    fn the_kinds_are_tried_in_the_order_preferred() {
        let valley = current(
            MeasuredOn::Itself,
            Rule::ValleyOrSmear(ValleyOrSmearRule {
                fallback: Some(RuleTarget::named("CD69+ of CD8+")),
                ..ValleyOrSmearRule::default()
            }),
        );
        let tries = first_tries(
            &valley,
            Some((0.005, 0.01)),
            Some(&fmx()),
            Some(&qc()),
            Some(0.2),
            Some(&axes()),
        );
        assert_eq!(
            kinds(&tries),
            [
                Kind::FmxBand,
                Kind::AboveNegative,
                Kind::ValleyOrSmear,
                Kind::Band,
                Kind::Phenotype
            ]
        );
        let on = |measured_on: MeasuredOn, rule: Rule| GateRule {
            measured_on,
            rule,
            ..valley.clone()
        };
        let on_qc = MeasuredOn::File(qc());
        assert_eq!(
            rule_of(&tries, Kind::FmxBand),
            &on(
                fmx(),
                Rule::TailFraction(TailFractionRule::new((0.005, 0.01)))
            ),
            "the band given, read on the FMX, stopping anywhere in it"
        );
        assert_eq!(
            rule_of(&tries, Kind::ValleyOrSmear),
            &on(
                on_qc.clone(),
                Rule::ValleyOrSmear(ValleyOrSmearRule {
                    fallback: Some(RuleTarget::named("CD69+ of CD8+")),
                    ..ValleyOrSmearRule::default()
                })
            ),
            "calibrated on the sample gated by hand, what the rule falls back to kept"
        );
        assert_eq!(
            rule_of(&tries, Kind::AboveNegative),
            &on(
                on_qc.clone(),
                Rule::AboveTheNegative(AboveTheNegativeRule::default())
            )
        );
        assert_eq!(
            rule_of(&tries, Kind::Band),
            &on(
                MeasuredOn::Itself,
                Rule::TailFraction(TailFractionRule::aimed(
                    (0.2 * 0.8, 0.2 * 1.25),
                    BandAim::Middle
                ))
            ),
            "around what the hand gates hold on each sample"
        );
        assert_eq!(
            rule_of(&tries, Kind::Phenotype),
            &on(
                on_qc,
                Rule::MatchThePhenotype(PhenotypeRule {
                    markers: vec![Arc::from("CD69"), Arc::from("CD4")],
                    ..PhenotypeRule::default()
                })
            ),
            "on the plot's two axes, described from the sample gated by hand"
        );
    }

    #[test]
    fn a_kind_with_nothing_to_start_from_is_left_out() {
        let band = current(fmx(), Rule::TailFraction(TailFractionRule::new((0.1, 0.2))));
        let without = first_tries(&band, None, Some(&fmx()), Some(&qc()), Some(0.0), None);
        assert_eq!(
            kinds(&without),
            [Kind::AboveNegative, Kind::ValleyOrSmear],
            "no band given, the hand gates holding nothing, no axes to match on"
        );
        let nothing = first_tries(&band, Some((0.0, 0.01)), None, None, None, Some(&axes()));
        assert!(
            nothing.is_empty(),
            "no FMX, and no sample gated by hand to calibrate on: {nothing:?}"
        );
    }

    #[test]
    fn a_smear_example_the_rule_has_is_kept() {
        let either = current(
            MeasuredOn::File(Arc::from("qc")),
            Rule::ValleyOrSmear(ValleyOrSmearRule {
                smear_example: Some(Arc::from("d7_fs")),
                ..ValleyOrSmearRule::default()
            }),
        );
        let tries = first_tries(&either, None, None, Some(&qc()), None, None);
        let Rule::ValleyOrSmear(tried) = &rule_of(&tries, Kind::ValleyOrSmear).rule else {
            panic!("valley or smear");
        };
        assert_eq!(tried.smear_example.as_deref(), Some("d7_fs"));
    }

    #[test]
    fn a_pooled_band_stays_pooled_on_the_fmx() {
        let pooled = current(
            fmx(),
            Rule::TailFraction(TailFractionRule {
                pool: Pool::Run,
                ..TailFractionRule::new((0.1, 0.2))
            }),
        );
        let tries = first_tries(&pooled, Some((0.0, 0.01)), Some(&fmx()), None, None, None);
        let Rule::TailFraction(band) = &rule_of(&tries, Kind::FmxBand).rule else {
            panic!("a band");
        };
        assert_eq!((band.band, band.pool), ((0.0, 0.01), Pool::Run));
    }

    #[test]
    fn a_phenotype_gate_is_tried_only_by_its_phenotype() {
        let phenotype = current(
            MeasuredOn::File(Arc::from("qc")),
            Rule::MatchThePhenotype(PhenotypeRule::default()),
        );
        let tries = first_tries(
            &phenotype,
            Some((0.0, 0.01)),
            Some(&fmx()),
            Some(&qc()),
            Some(0.2),
            Some(&axes()),
        );
        assert_eq!(tries, [(Kind::Phenotype, phenotype)]);
    }

    #[test]
    fn a_band_on_the_sample_never_starts_beyond_the_whole_parent() {
        let itself = current(
            MeasuredOn::Itself,
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        let tries = first_tries(&itself, None, None, None, Some(0.9), None);
        let Rule::TailFraction(band) = &rule_of(&tries, Kind::Band).rule else {
            panic!("a band");
        };
        assert_eq!(band.band, (0.9 * 0.8, 1.0));
    }

    #[test]
    fn the_fmx_is_what_the_rule_reads_or_the_first_sample_type_shown() {
        let store = |order: &[&str]| {
            RuleStore::with_pairing(SamplePairing {
                display_order: order.iter().map(|t| Arc::from(*t)).collect(),
                ..SamplePairing::default()
            })
        };
        let fmo = MeasuredOn::Partner(Arc::from("FMO"));
        let reads_fmo = current(
            fmo.clone(),
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        assert_eq!(fmx_of(&reads_fmo, &store(&["FMX", "FS"])), Some(fmo));
        let itself = current(
            MeasuredOn::Itself,
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        assert_eq!(fmx_of(&itself, &store(&["FMX", "FS"])), Some(fmx()));
        assert_eq!(fmx_of(&itself, &store(&["FS"])), None, "one kind of sample");
    }

    fn scored(off: usize, not_placed: usize, judged: usize) -> GateScore {
        GateScore {
            gate_id: "g".into(),
            gate: "g".into(),
            scored: judged - not_placed,
            not_placed,
            references: 1,
            typical_agreement: Some(0.9),
            lowest_agreement: None,
            off,
            off_samples: Vec::new(),
            median_caught: None,
            median_extra: None,
            median_holds_difference: None,
            median_edge_off_iqrs: None,
        }
    }

    #[test]
    fn a_rule_passes_with_no_more_than_its_share_of_samples_off() {
        assert!(passes(&scored(2, 0, 20), 0.1));
        assert!(!passes(&scored(3, 0, 20), 0.1));
        assert!(
            passes(&scored(3, 0, 30), 0.1),
            "a tenth of 30, in floating point"
        );
        assert!(
            passes(&scored(2, 1, 20), 0.1),
            "a sample not placed is off, and judged"
        );
        assert!(passes(&scored(0, 0, 1), 0.0));
        assert!(
            !passes(&scored(0, 0, 0), 1.0),
            "nothing judged passes nothing"
        );
    }

    fn solved(rows: Vec<crate::gate_rules::score::ScoreRow>) -> Solved {
        Solved {
            rows,
            refused: Vec::new(),
            placed: Vec::new(),
        }
    }

    #[test]
    fn the_first_smear_in_order_is_the_example() {
        let settings = ScoreSettings::default();
        let met = solved(vec![
            refused_row("d10_fs", NO_SMEAR_EXAMPLE),
            refused_row("d1_fs", "the parent holds 3 events"),
            agreeing_row("d0_fs", 1_000, settings),
            refused_row("d2_fs", NO_SMEAR_EXAMPLE),
        ]);
        assert_eq!(first_smear(&met), Some(Arc::from("d2_fs")));
        let none = solved(vec![refused_row("d1_fs", "the parent holds 3 events")]);
        assert_eq!(first_smear(&none), None);
    }

    #[test]
    fn a_valley_that_met_a_smear_is_tried_again_with_it_as_the_example() {
        let target = RuleTarget::named("CD69+");
        let either = current(
            MeasuredOn::File(qc()),
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        let met = solved(vec![
            agreeing_row("d1_fs", 990, ScoreSettings::default()),
            refused_row("d4_fs", NO_SMEAR_EXAMPLE),
            refused_row("d3_fs", NO_SMEAR_EXAMPLE),
        ]);
        let mut climb = Climb {
            target: &target,
            current: &either,
            rungs: vec![rung(Kind::ValleyOrSmear, &either)],
            tried: vec![(either.clone(), met)],
        };
        let again = climb.with_smear_example().expect("a smear met");
        assert_eq!(
            again,
            GateRule {
                rule: Rule::ValleyOrSmear(ValleyOrSmearRule {
                    smear_example: Some(Arc::from("d3_fs")),
                    ..ValleyOrSmearRule::default()
                }),
                ..either.clone()
            }
        );
        assert_eq!(
            climb.rungs[0].first, again,
            "its settings are searched from it"
        );
        assert_eq!(climb.with_smear_example(), None, "it has an example now");

        let mut clean = Climb {
            target: &target,
            current: &either,
            rungs: vec![rung(Kind::ValleyOrSmear, &either)],
            tried: vec![(either.clone(), agreeing(0, 990, 990))],
        };
        assert_eq!(clean.with_smear_example(), None, "no smear met");
    }

    fn candidate(rule: GateRule, typical: f64, current: bool) -> Candidate {
        Candidate {
            said: String::new(),
            rule,
            current,
            fit: Some(GateScore {
                typical_agreement: Some(typical),
                ..scored(0, 0, 10)
            }),
            check: None,
            place_by_typical: 1,
            place_by_off: 1,
            place_on_check: None,
            among_best: false,
            placed: Vec::new(),
        }
    }

    #[test]
    fn the_gain_is_the_best_over_the_rule_as_it_stands() {
        let band = current(
            MeasuredOn::Itself,
            Rule::TailFraction(TailFractionRule::new((0.1, 0.2))),
        );
        let mut candidates = vec![
            candidate(band.clone(), 0.9, false),
            candidate(band.clone(), 0.6, false),
        ];
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
    fn settings_out_of_range_are_refused() {
        let with = |fmx_band, most_off| PickSettings {
            fmx_band,
            most_off,
            ..PickSettings::default()
        };
        assert!(with(Some((0.0, 0.01)), 0.1).checked().is_ok());
        assert!(
            with(Some((0.01, 0.01)), 0.0).checked().is_ok(),
            "one share exactly"
        );
        assert!(with(None, 1.0).checked().is_ok());
        for band in [(0.02, 0.01), (-0.1, 0.01), (0.0, 1.5), (0.0, 0.0)] {
            let refused = with(Some(band), 0.1).checked().unwrap_err();
            assert!(refused.contains("FMX band"), "{band:?}: {refused}");
        }
        assert!(with(None, 1.1).checked().unwrap_err().contains("most_off"));
        let loose = PickSettings {
            score: ScoreSettings {
                off_below: 1.5,
                ..ScoreSettings::default()
            },
            ..PickSettings::default()
        };
        assert!(loose.checked().unwrap_err().contains("off_below"));
    }

    #[test]
    fn the_settings_are_kept_in_the_workspace_for_every_pick_after() {
        let folder = scratch("pick-settings");
        assert_eq!(kept_settings(&folder), PickSettings::default());
        assert_eq!(
            (
                PickSettings::default().score.off_below,
                PickSettings::default().most_off
            ),
            (0.95, 0.1)
        );
        let chosen = PickSettings {
            fmx_band: Some((0.005, 0.01)),
            score: ScoreSettings {
                off_below: 0.9,
                noise_widths: 2.0,
            },
            most_off: 0.2,
        };
        keep_settings(&folder, &chosen).unwrap();
        assert_eq!(kept_settings(&folder), chosen);
        std::fs::write(settings_file(&folder), "not settings").unwrap();
        assert_eq!(
            kept_settings(&folder),
            PickSettings::default(),
            "an unreadable file"
        );
        std::fs::write(settings_file(&folder), r#"{"fmx_band": [0.0, 0.02]}"#).unwrap();
        assert_eq!(
            kept_settings(&folder),
            PickSettings {
                fmx_band: Some((0.0, 0.02)),
                ..PickSettings::default()
            },
            "what a file leaves out is the default"
        );
    }

    /// Ten samples, `off` of them agreeing `low`, the rest `high` - in
    /// thousandths - judged with no allowance for counting noise.
    fn agreeing(off: usize, low: usize, high: usize) -> Solved {
        let settings = ScoreSettings {
            off_below: AGREEMENT,
            noise_widths: 0.0,
        };
        solved(
            (0..10)
                .map(|at| {
                    let both = if at < off { low } else { high };
                    agreeing_row(&format!("d{at}_fs"), both, settings)
                })
                .collect(),
        )
    }

    fn rung(kind: Kind, first: &GateRule) -> Rung {
        Rung {
            kind,
            first: first.clone(),
            searched: false,
        }
    }

    fn unsplit() -> FitSettings {
        FitSettings {
            split: false,
            ..FitSettings::default()
        }
    }

    fn rules_of(picked: &Picked) -> Vec<&GateRule> {
        picked.fit.candidates.iter().map(|c| &c.rule).collect()
    }

    #[test]
    fn the_first_kind_in_order_that_passes_is_picked_over_a_closer_one_after_it() {
        let target = RuleTarget::named("CD69+");
        let as_it_stands = current(
            MeasuredOn::Itself,
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        let on_fmx = current(
            fmx(),
            Rule::TailFraction(TailFractionRule::new((0.0, 0.01))),
        );
        let either = current(
            MeasuredOn::Itself,
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        let climb = Climb {
            target: &target,
            current: &as_it_stands,
            rungs: vec![
                rung(Kind::FmxBand, &on_fmx),
                rung(Kind::ValleyOrSmear, &either),
            ],
            tried: vec![
                (as_it_stands.clone(), agreeing(10, 500, 500)),
                (on_fmx.clone(), agreeing(1, 900, 960)),
                (either.clone(), agreeing(0, 990, 990)),
            ],
        };
        let picked = climb.picked(MOST_OFF, unsplit(), &[], 10);
        assert!(picked.passed);
        assert_eq!(
            rules_of(&picked),
            [&on_fmx, &as_it_stands],
            "the FMX band, one sample in ten off, before the closer valley; the rule as it \
             stands after"
        );
        let stood = picked.fit.candidates.last().unwrap();
        assert!(stood.current && !stood.among_best);
        assert!(picked.fit.candidates[0].among_best);
        let tried: Vec<(Kind, bool, usize)> = picked
            .kinds
            .iter()
            .map(|k| (k.kind, k.passed, k.off))
            .collect();
        assert_eq!(
            tried,
            [(Kind::FmxBand, true, 1), (Kind::ValleyOrSmear, true, 0)]
        );
        assert_eq!(picked.tried, 3);
    }

    #[test]
    fn only_the_settings_that_pass_contend_for_the_kind_picked() {
        let target = RuleTarget::named("CD69+");
        let either = current(
            MeasuredOn::Itself,
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        let smoother = GateRule {
            rule: Rule::ValleyOrSmear(ValleyOrSmearRule {
                smoothing: 2.0,
                ..ValleyOrSmearRule::default()
            }),
            ..either.clone()
        };
        let climb = Climb {
            target: &target,
            current: &either,
            rungs: vec![Rung {
                searched: true,
                ..rung(Kind::ValleyOrSmear, &either)
            }],
            tried: vec![
                (either.clone(), agreeing(1, 900, 960)),
                (smoother.clone(), agreeing(2, 500, 999)),
            ],
        };
        let picked = climb.picked(MOST_OFF, unsplit(), &[], 10);
        assert!(picked.passed);
        assert_eq!(
            rules_of(&picked),
            [&either],
            "a higher typical agreement with two samples in ten off does not pass"
        );
        assert!(picked.kinds[0].searched);
    }

    #[test]
    fn with_no_kind_passing_the_closest_of_everything_is_picked_and_flagged() {
        let target = RuleTarget::named("CD69+");
        let as_it_stands = current(
            MeasuredOn::Itself,
            Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        );
        let on_fmx = current(
            fmx(),
            Rule::TailFraction(TailFractionRule::new((0.0, 0.01))),
        );
        let above = current(
            MeasuredOn::Itself,
            Rule::AboveTheNegative(AboveTheNegativeRule {
                find: NegativeFinder::NegativePeak,
                ..AboveTheNegativeRule::default()
            }),
        );
        let climb = Climb {
            target: &target,
            current: &as_it_stands,
            rungs: vec![
                rung(Kind::FmxBand, &on_fmx),
                rung(Kind::AboveNegative, &above),
            ],
            tried: vec![
                (as_it_stands.clone(), agreeing(4, 800, 990)),
                (on_fmx.clone(), agreeing(3, 900, 960)),
                (above.clone(), agreeing(2, 100, 999)),
            ],
        };
        let picked = climb.picked(MOST_OFF, unsplit(), &[], 10);
        assert!(!picked.passed);
        assert_eq!(
            rules_of(&picked),
            [&above, &as_it_stands, &on_fmx],
            "everything tried, the highest typical agreement first - tied with the rule \
             as it stands, and fewer off"
        );
        assert!(picked.kinds.iter().all(|k| !k.passed));
        assert_eq!(
            picked.kinds.len(),
            2,
            "the rule as it stands is no kind tried"
        );
    }
}
