//! Replaying reviewed runs: what a rule - as it was, or changed - does on the
//! samples it was run on, set against where a person said each gate belongs.
//!
//! A run keeps the events behind every gate it measured, each file's gate as
//! it stood before the run, and each file's metadata ([`super::events`]). A
//! replay rebuilds exactly that - the populations, the gates the solver
//! starts from, the specimens and sample types - and runs the real solver on
//! it ([`crate::gate_rules::autogate::solve_all`], through
//! [`crate::gate_rules::autogate::measure_population`]), with the run's own
//! rules or with changes to them. Nothing is re-implemented: a replay places
//! gates with the code a run does.
//!
//! What each placement is judged against comes from the review:
//!
//! - a placement accepted as placed is right where the rule put it;
//! - a reported one with a fix saved is right where the fix put it;
//! - one moved by hand without a report is right where it was moved;
//! - a gate the rule left alone, accepted, is right where it was;
//! - a reported one with no fix saved has no known right answer, only what
//!   the reviewer said was wrong with it.
//!
//! For a run not yet marked reviewed, the same is read off the gates as they
//! stand ([`super::report::review_as_it_stands`]) and said to be provisional.
//!
//! Each placement is then compared on the fraction of the sample's events
//! beyond the line on the rule's parameter - the rule's own measure, read the
//! same way for the right answer, the run's answer and the replay's. Close
//! enough is within [`TOLERANCE_RELATIVE`] of the right fraction, or
//! [`TOLERANCE_ABSOLUTE`], whichever is larger.
//!
//! A replay reads the events a run kept - a sample of each population, not
//! every event - so every placement is first replayed with the run's own
//! rules. Where that reproduces what the run did, a change is judged on the
//! replay; where it does not, the case is said to be not reproduced rather
//! than judged, so sampling can never pass for a rule change fixing or
//! breaking something. (A band rule stops at the first position inside its
//! band, so a placement near the band's edge can land either side of it.)
//!
//! Phenotype rules are not replayed: they read a marker panel, and runs keep
//! the events on the gate's two axes only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::events::{EventSample, KeptEvents};
use super::report::{Outcome, PlacementReport, Problem, RunReview};
use super::run_record::{ComponentRecord, ExtentRecord, KeptRecord, PlacedRecord, SampleRef};
use crate::gate_rules::rule::Rule;
use crate::gate_rules::rule_store::{Bound, GateRule, RuleStore, RuleTarget};
use crate::gates::GateState;
use crate::gates::gate_store::{GateId, GateSource};
use crate::gates::gate_traits::DrawableGate;

/// A placement is right if the fraction beyond its line is within this share
/// of the right fraction...
pub const TOLERANCE_RELATIVE: f64 = 0.2;
/// ...or within this many of the events, whichever is larger.
pub const TOLERANCE_ABSOLUTE: f64 = 0.002;

/// A change to one rule, tried in a replay: the rule for `target` replaced
/// by `rule`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleChange {
    pub target: RuleTarget,
    pub rule: GateRule,
}

/// Where a person said a gate belongs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Truth {
    /// Accepted where the rule put it.
    AcceptedAsPlaced,
    /// Left alone by the rule, and accepted.
    AcceptedLeftAlone,
    /// Reported, and the fix saved.
    Corrected { problem: Problem, note: String },
    /// Moved by hand without a report.
    MovedByHand,
    /// Reported, with no fix saved: no right answer, only what was wrong.
    ReportedWithoutFix { problem: Problem, note: String },
}

/// How a replay did against the right answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The run was wrong and the replay is right.
    Fixed,
    /// The run was right and the replay is wrong.
    Broken,
    /// Both wrong.
    StillWrong,
    /// Both right.
    StillRight,
    /// No right answer to judge against; the replay moved the line.
    Changed,
    /// No right answer to judge against; the replay agrees with the run.
    Unchanged,
    /// The replay with the run's own rules does not reproduce what the run
    /// did on the events kept - see [`Case::reproduced`] - so no change can
    /// be judged on it.
    NotReproduced,
    /// The replay could not place it - see the case's `why`.
    NotReplayed,
}

impl Verdict {
    pub const ALL: [Verdict; 8] = [
        Verdict::Fixed,
        Verdict::Broken,
        Verdict::StillWrong,
        Verdict::StillRight,
        Verdict::Changed,
        Verdict::Unchanged,
        Verdict::NotReproduced,
        Verdict::NotReplayed,
    ];
}

/// One placement's line, and what it lets through.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    /// Where the line sits on the rule's parameter, in the plot's units.
    pub at: Option<f64>,
    /// The fraction of the sample's kept events beyond it, on the gate's
    /// side, on the rule's parameter alone.
    pub beyond: Option<f64>,
}

/// What the run, or the replay, decided for one placement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decided {
    pub line: Line,
    /// Moved the gate, rather than leaving it where it met the rule.
    pub moved: bool,
    pub confidence: Option<f64>,
    pub weakest: Option<String>,
    pub components: Vec<ComponentRecord>,
    pub in_band: Option<bool>,
    /// The file the rule read.
    pub measured_on: Option<String>,
}

/// One placement of a reviewed run, replayed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Case {
    /// Which run: its workspace or library folder's name, and when it was
    /// applied.
    pub run: String,
    pub run_applied_at: String,
    pub gate_id: String,
    pub gate: String,
    pub parent_gate: Option<String>,
    pub sample: SampleRef,
    pub specimen: String,
    pub parameter: Option<String>,
    pub bound: Option<Bound>,
    /// The sample's events on the rule's parameter: how many, and how many
    /// were kept.
    pub events: Option<usize>,
    pub truth: Truth,
    /// The right line, where the review gives one.
    pub right: Line,
    pub run_decided: Decided,
    /// The replay with the run's own rules, on the events kept. Where it
    /// agrees with the run, the replay reproduces it and a change can be
    /// judged; where not, the events kept are not enough to say.
    pub baseline_decided: Option<Decided>,
    pub reproduced: bool,
    /// The replay with the changes tried.
    pub replay_decided: Option<Decided>,
    pub verdict: Verdict,
    /// Why it was not replayed, or anything else worth saying about it.
    pub why: Option<String>,
    /// Read off the gates as they stand rather than a marked review.
    pub provisional: bool,
}

/// A replay of one or more runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Replay {
    pub runs: Vec<RunSummary>,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunSummary {
    pub run: String,
    pub run_applied_at: String,
    pub cases: usize,
    pub provisional: bool,
    /// Why the run could not be replayed at all, if it could not.
    pub not_replayed: Option<String>,
}

impl Replay {
    pub fn count(&self, verdict: Verdict) -> usize {
        self.cases.iter().filter(|c| c.verdict == verdict).count()
    }

    /// Counts per verdict, per gate.
    pub fn by_gate(&self) -> BTreeMap<String, BTreeMap<Verdict, usize>> {
        let mut out: BTreeMap<String, BTreeMap<Verdict, usize>> = BTreeMap::new();
        for c in &self.cases {
            let name = crate::gate_rules::autogate::describe(&c.gate, c.parent_gate.as_deref());
            *out.entry(name).or_default().entry(c.verdict).or_default() += 1;
        }
        out
    }
}

// ── a reviewed run, whichever way it was read ─────────────────────────────

/// One run to replay: its rules, placements with what each should have been,
/// the reports that say where the fixes went, and its events.
pub struct ReviewedRun {
    pub name: String,
    pub applied_at: String,
    pub rules: RuleStore,
    pub placed: Vec<(PlacedRecord, Outcome)>,
    pub kept: Vec<(KeptRecord, Outcome)>,
    pub reports: Vec<PlacementReport>,
    pub events: Option<KeptEvents>,
    pub provisional: bool,
}

impl ReviewedRun {
    fn from_review(
        name: String,
        review: RunReview,
        reports: Vec<PlacementReport>,
        events: Option<KeptEvents>,
        provisional: bool,
    ) -> Self {
        Self {
            name,
            applied_at: review.run_applied_at.clone(),
            rules: review.rules,
            placed: review
                .placements
                .into_iter()
                .map(|p| (p.placed, p.outcome))
                .collect(),
            kept: review
                .kept
                .into_iter()
                .map(|k| (k.kept, k.outcome))
                .collect(),
            reports,
            events,
            provisional,
        }
    }

    /// A run reviewed into the review library: a folder holding
    /// `review.json`, its `reports` and `run_events.bin`.
    pub fn from_library_folder(folder: &Path) -> anyhow::Result<Self> {
        let review: RunReview = serde_json::from_str(&std::fs::read_to_string(
            folder.join(super::report::REVIEW_FILE),
        )?)?;
        let reports = reports_under(&folder.join(super::report::REPORTS_DIR));
        let events = match std::fs::read(folder.join(super::events::EVENTS_FILE)) {
            Ok(bytes) => Some(super::events::decode(&bytes)?),
            Err(_) => None,
        };
        let name = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self::from_review(name, review, reports, events, false))
    }

    /// The workspace's last run, judged as the gates stand now - or, if it
    /// has been marked reviewed since it was applied, as marked.
    pub fn from_workspace(
        folder: &Path,
        state: &GateState,
        metadata: &crate::omiq::metadata::MetaDataFileMap,
    ) -> Result<Self, String> {
        let (live, run, reports) = super::report::review_as_it_stands(folder, state, metadata)?;
        let marked: Option<RunReview> = std::fs::read_to_string(
            folder
                .join(super::REVIEWS_DIR)
                .join(super::report::REVIEW_FILE),
        )
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|r: &RunReview| r.run_applied_at == run.applied_at);
        let provisional = marked.is_none();
        let review = marked.unwrap_or(live);
        let events = super::events::load(folder, &run.applied_at).map_err(|e| e.to_string())?;
        let name = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".into());
        Ok(Self::from_review(
            name,
            review,
            reports.into_iter().map(|(_, r)| r).collect(),
            events,
            provisional,
        ))
    }
}

fn reports_under(dir: &Path) -> Vec<PlacementReport> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&text).ok()
        })
        .collect()
}

/// Every reviewed run in a review library, oldest first. A folder that
/// cannot be read is passed over, and said so.
pub fn library_runs(library: &Path) -> (Vec<ReviewedRun>, Vec<String>) {
    let mut runs = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(library) else {
        return (runs, vec![format!("{}: no such folder", library.display())]);
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(super::report::REVIEW_FILE).is_file())
        .collect();
    folders.sort();
    for folder in folders {
        match ReviewedRun::from_library_folder(&folder) {
            Ok(run) => runs.push(run),
            Err(e) => problems.push(format!("{}: {e}", folder.display())),
        }
    }
    runs.sort_by(|a, b| a.applied_at.cmp(&b.applied_at));
    (runs, problems)
}

// ── lines and fractions ───────────────────────────────────────────────────

/// Where a gate's line sits: the leading side of its extent on the rule's
/// parameter.
fn line_of(extent: &[ExtentRecord], parameter: &str, bound: Bound) -> Option<f64> {
    let e = extent.iter().find(|e| e.parameter == parameter)?;
    match bound {
        Bound::Above => e.lower,
        Bound::Below => e.upper,
    }
}

/// The fraction of `points` beyond `line` on the rule's parameter, on the
/// gate's side.
fn beyond(points: &[(f32, f32)], on_x: bool, line: f64, bound: Bound) -> Option<f64> {
    if points.is_empty() {
        return None;
    }
    let past = points
        .iter()
        .filter(|(x, y)| {
            let v = if on_x { *x } else { *y } as f64;
            match bound {
                Bound::Above => v >= line,
                Bound::Below => v <= line,
            }
        })
        .count();
    Some(past as f64 / points.len() as f64)
}

/// Whether a fraction is close enough to the right one.
pub fn close_enough(got: f64, right: f64) -> bool {
    (got - right).abs() <= (TOLERANCE_RELATIVE * right).max(TOLERANCE_ABSOLUTE)
}

// ── rebuilding what the run read ──────────────────────────────────────────

/// A drawable gate from a kept one - the kinds a rule slides along an axis.
pub fn drawable(gate: &flow_gates::Gate, id: &GateId) -> anyhow::Result<Arc<dyn DrawableGate>> {
    use crate::gates::gate_single::{
        ellipse_gate::EllipseGate, polygon_gate::PolygonGate, rectangle_gate::RectangleGate,
    };
    let mut gate = gate.clone();
    gate.id = id.clone();
    Ok(match &gate.geometry {
        flow_gates::GateGeometry::Rectangle { .. } => Arc::new(RectangleGate::try_new(gate, true)?),
        flow_gates::GateGeometry::Polygon { .. } => Arc::new(PolygonGate::try_new(gate, true)?),
        flow_gates::GateGeometry::Ellipse { .. } => Arc::new(EllipseGate::try_new(gate, true)?),
        _ => anyhow::bail!("a gate made of other gates is not replayed"),
    })
}

fn events_frame(sample: &EventSample) -> anyhow::Result<Arc<polars::prelude::DataFrame>> {
    use polars::prelude::*;
    let xs: Vec<f32> = sample.points.iter().map(|p| p.0).collect();
    let ys: Vec<f32> = sample.points.iter().map(|p| p.1).collect();
    Ok(Arc::new(
        df![sample.x.as_str() => xs, sample.y.as_str() => ys]?,
    ))
}

/// The rules a run is replayed with: its own, with `changes` in place.
pub fn with_changes(rules: &RuleStore, changes: &[RuleChange]) -> RuleStore {
    let mut out = rules.clone();
    for change in changes {
        out.insert(change.target.clone(), change.rule.clone());
    }
    out
}

/// What the solver decided for each gate on each file, replayed.
pub(crate) struct Solved {
    pub(crate) decided: BTreeMap<(String, String), Decided>,
    pub(crate) skipped: BTreeMap<(String, String), String>,
}

pub(crate) fn solve(events: &KeptEvents, rules: &RuleStore) -> Solved {
    use crate::gate_rules::autogate::{Unmeasured, measure_population, solve_all};

    let metadata: crate::omiq::metadata::MetaDataFileMap = events
        .metadata
        .iter()
        .map(|(file, row)| {
            (
                Arc::from(file.as_str()),
                row.iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), Arc::from(v.as_str())))
                    .collect(),
            )
        })
        .collect();

    let mut state = GateState::default();
    let mut skipped: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut measured = Vec::new();
    let mut unmeasured: Vec<Unmeasured> = Vec::new();
    for sample in &events.samples {
        let key = (sample.gate_id.clone(), sample.file.clone());
        let Some(kept) = &sample.gate else {
            skipped.insert(
                key,
                "the run did not keep this gate as it stood before it".into(),
            );
            continue;
        };
        let id: GateId = Arc::from(sample.gate_id.as_str());
        let gate = match drawable(kept, &id) {
            Ok(gate) => gate,
            Err(e) => {
                skipped.insert(key, e.to_string());
                continue;
            }
        };
        let name: Arc<str> = Arc::from(kept.name.as_str());
        let parent: Option<Arc<str>> = sample.parent_gate.as_deref().map(Arc::from);
        let Some(rule) = rules.rule_for(&name, parent.as_deref()) else {
            skipped.insert(key, "no rule for this gate in the rules replayed".into());
            continue;
        };
        if matches!(rule.rule, Rule::MatchThePhenotype(_)) {
            skipped.insert(
                key,
                "a phenotype rule reads the marker panel, and runs keep only the gate's two axes"
                    .into(),
            );
            continue;
        }
        let file: Arc<str> = Arc::from(sample.file.as_str());
        state.place_gate(
            &[id.clone()],
            &gate,
            &GateSource::Sample((id.clone(), file.clone())),
        );
        let frame = match events_frame(sample) {
            Ok(frame) => frame,
            Err(e) => {
                skipped.insert(key, e.to_string());
                continue;
            }
        };
        match measure_population(&file, &id, name, parent, &gate, rule, &frame) {
            Ok(m) => measured.push(m),
            Err(u) => {
                skipped.insert(key, u.reason.clone());
                unmeasured.push(u);
            }
        }
    }

    let (report, _) = solve_all(&state, rules, &measured, &unmeasured, &metadata);
    let mut decided = BTreeMap::new();
    for p in &report.positioned {
        decided.insert(
            (p.gate_id.to_string(), p.file.to_string()),
            Decided {
                line: Line {
                    at: Some(p.to).filter(|v| v.is_finite()),
                    beyond: None,
                },
                moved: true,
                confidence: Some(p.confidence),
                weakest: p.weakest.map(str::to_string),
                components: p
                    .components
                    .iter()
                    .map(|c| ComponentRecord {
                        name: c.name.to_string(),
                        score: c.score,
                        detail: c.detail.clone(),
                    })
                    .collect(),
                in_band: Some(p.in_band),
                measured_on: Some(p.measured_on.to_string()),
            },
        );
    }
    for u in report.unchanged.iter().chain(&report.reference) {
        decided.insert(
            (u.gate_id.to_string(), u.file.to_string()),
            Decided {
                line: Line {
                    at: u.line,
                    beyond: None,
                },
                moved: false,
                confidence: None,
                weakest: None,
                components: Vec::new(),
                in_band: None,
                measured_on: u.measured_on.as_ref().map(|f| f.to_string()),
            },
        );
    }
    // What the solver could not place, by the gate's name and the file.
    for s in &report.skipped {
        let matching = events
            .samples
            .iter()
            .find(|e| e.file == *s.file && e.gate.as_ref().is_some_and(|g| *g.name == *s.gate));
        if let Some(sample) = matching {
            skipped
                .entry((sample.gate_id.clone(), sample.file.clone()))
                .or_insert_with(|| s.reason.clone());
        }
    }
    Solved { decided, skipped }
}

// ── replaying ─────────────────────────────────────────────────────────────

fn truth_of(
    outcome: &Outcome,
    gate_id: &str,
    sample: &str,
    reports: &[PlacementReport],
) -> Option<(Truth, Option<Vec<ExtentRecord>>)> {
    let report_for = || {
        reports
            .iter()
            .filter(|r| r.gate_id == gate_id && r.sample.id == sample)
            .max_by(|a, b| a.reported_at.cmp(&b.reported_at))
    };
    Some(match outcome {
        Outcome::Accepted => (Truth::AcceptedAsPlaced, None),
        Outcome::MovedUnreported { gate_at } => (Truth::MovedByHand, Some(gate_at.clone())),
        Outcome::Gone => return None,
        Outcome::Reported { .. } => {
            let report = report_for();
            let problem = report.map_or(Problem::Other, |r| r.problem);
            let note = report.map(|r| r.note.clone()).unwrap_or_default();
            match report.and_then(|r| r.correction.as_ref()) {
                Some(fix) => (
                    Truth::Corrected { problem, note },
                    Some(fix.gate_at.clone()),
                ),
                None => (Truth::ReportedWithoutFix { problem, note }, None),
            }
        }
    })
}

/// Replay one reviewed run with its own rules and `changes`.
pub fn replay_run(run: &ReviewedRun, changes: &[RuleChange]) -> (RunSummary, Vec<Case>) {
    let rules = with_changes(&run.rules, changes);
    let summary = |cases: usize, not_replayed: Option<String>| RunSummary {
        run: run.name.clone(),
        run_applied_at: run.applied_at.clone(),
        cases,
        provisional: run.provisional,
        not_replayed,
    };
    let usable = run
        .events
        .as_ref()
        .filter(|e| !e.metadata.is_empty() && e.samples.iter().any(|s| s.gate.is_some()));
    let not_replayable = match &run.events {
        None => Some("no events were kept with this run".to_string()),
        Some(_) if usable.is_none() => Some(
            "this run's events were kept before replays were possible; run the rules again to replay it"
                .to_string(),
        ),
        _ => None,
    };
    let baseline = usable.map(|events| solve(events, &run.rules));
    let solved = if changes.is_empty() {
        None
    } else {
        usable.map(|events| solve(events, &rules))
    };
    let solved = solved.as_ref().or(baseline.as_ref());

    let mut cases = Vec::new();
    let placed = run.placed.iter().map(|(p, o)| {
        (
            p.gate_id.clone(),
            p.gate.clone(),
            p.parent_gate.clone(),
            p.sample.clone(),
            p.specimen.clone(),
            o,
            Decided {
                line: Line {
                    at: p.to,
                    beyond: None,
                },
                moved: true,
                confidence: Some(p.confidence),
                weakest: p.weakest.clone(),
                components: p.components.clone(),
                in_band: Some(p.in_band),
                measured_on: Some(p.measured_on.id.clone()),
            },
            p.bound,
        )
    });
    let kept = run.kept.iter().filter(|(k, _)| k.met_rule).map(|(k, o)| {
        (
            k.gate_id.clone(),
            k.gate.clone(),
            k.parent_gate.clone(),
            k.sample.clone(),
            k.specimen.clone(),
            o,
            Decided {
                line: Line {
                    at: k.line,
                    beyond: None,
                },
                moved: false,
                confidence: None,
                weakest: None,
                components: Vec::new(),
                in_band: None,
                measured_on: k.measured_on.as_ref().map(|s| s.id.clone()),
            },
            k.bound,
        )
    });
    for (gate_id, gate, parent_gate, sample, specimen, outcome, run_decided, bound) in
        placed.chain(kept)
    {
        let Some((mut truth, fixed_at)) = truth_of(outcome, &gate_id, &sample.id, &run.reports)
        else {
            continue;
        };
        if matches!(truth, Truth::AcceptedAsPlaced) && !run_decided.moved {
            truth = Truth::AcceptedLeftAlone;
        }
        let rule = rules.rule_for(&gate, parent_gate.as_deref());
        let parameter = rule.map(|r| r.parameter.to_string());
        let bound = rule.map(|r| r.bound).or(bound);
        let events = usable.and_then(|e| {
            e.samples
                .iter()
                .find(|s| s.gate_id == gate_id && s.file == sample.id)
        });
        let on_x = |s: &EventSample| parameter.as_deref() == Some(s.x.as_str());
        let fraction = |at: Option<f64>| -> Option<f64> {
            let (s, at, bound) = (events?, at?, bound?);
            beyond(&s.points, on_x(s), at, bound)
        };
        let right_at = match &truth {
            Truth::AcceptedAsPlaced | Truth::AcceptedLeftAlone => run_decided.line.at,
            _ => fixed_at
                .as_ref()
                .zip(parameter.as_deref())
                .zip(bound)
                .and_then(|((e, p), b)| line_of(e, p, b)),
        };
        let right = Line {
            at: right_at,
            beyond: fraction(right_at),
        };
        let mut run_decided = run_decided;
        run_decided.line.beyond = fraction(run_decided.line.at);

        let key = (gate_id.clone(), sample.id.clone());
        let baseline_decided = baseline
            .as_ref()
            .and_then(|b| b.decided.get(&key))
            .map(|d| {
                let mut d = d.clone();
                d.line.beyond = fraction(d.line.at);
                d
            });
        let reproduced = match (&baseline_decided, run_decided.line.beyond) {
            (Some(b), Some(was)) => b.line.beyond.is_some_and(|now| close_enough(now, was)),
            (Some(b), None) => b.line.at == run_decided.line.at,
            (None, _) => false,
        };
        let (replay_decided, why) = match solved {
            None => (None, not_replayable.clone()),
            Some(solved) => match solved.decided.get(&key) {
                Some(d) => {
                    let mut d = d.clone();
                    d.line.beyond = fraction(d.line.at);
                    (Some(d), None)
                }
                None => (
                    None,
                    Some(
                        solved
                            .skipped
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| "the replay did not reach this sample".into()),
                    ),
                ),
            },
        };

        let verdict = match (&replay_decided, right.beyond) {
            (None, _) => Verdict::NotReplayed,
            (Some(_), _) if !reproduced => Verdict::NotReproduced,
            (Some(replayed), Some(right)) => {
                let was = run_decided.line.beyond.map(|b| close_enough(b, right));
                let now = replayed.line.beyond.map(|b| close_enough(b, right));
                match (was, now) {
                    (Some(false), Some(true)) => Verdict::Fixed,
                    (Some(true), Some(false)) => Verdict::Broken,
                    (Some(false), Some(false)) => Verdict::StillWrong,
                    (Some(true), Some(true)) => Verdict::StillRight,
                    _ => Verdict::NotReplayed,
                }
            }
            (Some(replayed), None) => {
                let base = baseline_decided.as_ref().unwrap_or(&run_decided);
                let same = match (replayed.line.beyond, base.line.beyond) {
                    (Some(a), Some(b)) => close_enough(a, b),
                    _ => replayed.line.at == base.line.at,
                };
                if same {
                    Verdict::Unchanged
                } else {
                    Verdict::Changed
                }
            }
        };

        cases.push(Case {
            run: run.name.clone(),
            run_applied_at: run.applied_at.clone(),
            gate_id,
            gate,
            parent_gate,
            events: events.map(|e| e.events),
            sample,
            specimen,
            parameter,
            bound,
            truth,
            right,
            run_decided,
            baseline_decided,
            reproduced,
            replay_decided,
            verdict,
            why,
            provisional: run.provisional,
        });
    }
    (summary(cases.len(), not_replayable), cases)
}

/// Replay every run given, with `changes`.
pub fn replay(runs: &[ReviewedRun], changes: &[RuleChange]) -> Replay {
    let mut out = Replay {
        runs: Vec::new(),
        cases: Vec::new(),
    };
    for run in runs {
        let (summary, cases) = replay_run(run, changes);
        out.runs.push(summary);
        out.cases.extend(cases);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_enough_is_a_fifth_of_the_right_fraction_or_a_fifth_of_a_percent() {
        assert!(close_enough(0.11, 0.10));
        assert!(!close_enough(0.13, 0.10));
        // Small fractions: the absolute floor.
        assert!(close_enough(0.003, 0.001));
        assert!(!close_enough(0.004, 0.001));
    }

    #[test]
    fn a_line_is_the_leading_side_of_the_gate_on_the_rule_s_parameter() {
        let extent = vec![
            ExtentRecord {
                parameter: "x".into(),
                lower: Some(1.0),
                upper: Some(5.0),
            },
            ExtentRecord {
                parameter: "y".into(),
                lower: Some(-1.0),
                upper: None,
            },
        ];
        assert_eq!(line_of(&extent, "x", Bound::Above), Some(1.0));
        assert_eq!(line_of(&extent, "x", Bound::Below), Some(5.0));
        assert_eq!(line_of(&extent, "y", Bound::Below), None);
        assert_eq!(line_of(&extent, "z", Bound::Above), None);
    }

    #[test]
    fn what_a_line_lets_through_is_counted_on_its_own_side_and_axis() {
        let points: Vec<(f32, f32)> = (0..100).map(|i| (i as f32, 100.0 - i as f32)).collect();
        assert_eq!(beyond(&points, true, 90.0, Bound::Above), Some(0.10));
        assert_eq!(beyond(&points, true, 9.0, Bound::Below), Some(0.10));
        // On y the order runs the other way.
        assert_eq!(beyond(&points, false, 91.0, Bound::Above), Some(0.10));
        assert_eq!(beyond(&[], true, 1.0, Bound::Above), None);
    }
}
