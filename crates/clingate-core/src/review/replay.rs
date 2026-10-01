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

/// A placement is right if the fraction of the population its gate holds is
/// within this share of the right fraction - of the right fraction or of
/// what it leaves out, whichever is smaller, so that a gate holding 97% is
/// held to as tight a standard as one holding 3%...
pub const TOLERANCE_RELATIVE: f64 = 0.2;
/// ...or within this many of the events kept, whichever is larger: a
/// difference of a couple of events is sampling, not placement.
pub const TOLERANCE_EVENTS: f64 = 3.0;

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

/// One placement's line, and what the gate holds with it there.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    /// Where the gate's leading side sits on the rule's parameter, in the
    /// plot's units.
    pub at: Option<f64>,
    /// The fraction of the sample's kept events inside the gate - the gate
    /// with its real shape, both axes, counted as the plot counts it. What
    /// a placement is judged on.
    pub holds: Option<f64>,
    /// The fraction of them past the line on the rule's parameter alone, the
    /// other axis ignored: how much of the population the line itself cuts.
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
    /// What went into this placement - the rule, the population and gate it
    /// was read from, and those of the file the rule read - as a
    /// fingerprint. Two runs with the same fingerprint for a placement did
    /// the same thing on the same data, and are counted once.
    pub input: Option<String>,
    /// Earlier runs that had exactly this input, and what their reviews
    /// said: the same placement reviewed again. Only the latest counts.
    pub earlier_reviews: Vec<EarlierReview>,
    /// Anything else worth knowing about how this case was judged.
    pub notes: Vec<String>,
}

/// An earlier review of a placement with the same input.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EarlierReview {
    pub run: String,
    pub run_applied_at: String,
    pub truth: Truth,
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
    /// Its placements that a later run repeated with exactly the same input,
    /// counted with the later run instead.
    pub repeated_later: usize,
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
        // A copy made before reports were tied to their run can hold an
        // earlier run's too.
        let reports: Vec<PlacementReport> = reports_under(&folder.join(super::report::REPORTS_DIR))
            .into_iter()
            .filter(|r| {
                r.run_applied_at
                    .as_deref()
                    .is_none_or(|at| at == review.run_applied_at)
            })
            .collect();
        let events = super::events::load_from_library(folder)?;
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

/// Whether a fraction is close enough to the right one, for a population
/// of `kept` events.
pub fn close_enough(got: f64, right: f64, kept: usize) -> bool {
    let smaller_side = right.min(1.0 - right).max(0.0);
    let sampling = TOLERANCE_EVENTS / kept.max(1) as f64;
    (got - right).abs() <= (TOLERANCE_RELATIVE * smaller_side).max(sampling)
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
        if matches!(rule.rule, Rule::FromAnotherGate(_) | Rule::NextToGate(_)) {
            skipped.insert(
                key,
                "this gate follows another gate's position, which a replay does not place - \
                 replay the gate it follows"
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
                    holds: None,
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
                    holds: None,
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

// ── what a line holds ─────────────────────────────────────────────────────

/// A population the run kept, ready to count what a gate holds of it -
/// through the same index and the same statistic the plot uses.
struct Counted<'a> {
    sample: &'a EventSample,
    index: Option<crate::events::EventIndexMapped>,
}

impl<'a> Counted<'a> {
    fn new(sample: &'a EventSample) -> Self {
        let index = events_frame(sample).ok().and_then(|frame| {
            let event_index = crate::events::index_over(&frame, &sample.x, &sample.y).ok()?;
            Some(crate::events::EventIndexMapped {
                event_index,
                index_map: Arc::new((0..frame.height()).collect()),
            })
        });
        Self { sample, index }
    }

    fn kept(&self) -> usize {
        self.sample.points.len()
    }

    /// What `gate` holds of the population.
    fn held_by(&self, gate: &flow_gates::Gate) -> Option<f64> {
        let id: GateId = Arc::from(gate.id.as_ref());
        let drawable = drawable(gate, &id).ok()?;
        crate::gate_rules::autogate::admitted_by(&drawable, self.index.as_ref()?)
    }

    /// What the gate as it stood before the run holds with its leading side
    /// moved to `at` - which is what every rule that moves a line does to it:
    /// slides the whole shape, unchanged, along the parameter.
    fn held_at(&self, parameter: &str, bound: Bound, at: f64) -> Option<f64> {
        let gate = self.sample.gate.as_ref()?;
        let id: GateId = Arc::from(gate.id.as_ref());
        let drawable = drawable(gate, &id).ok()?;
        let moved =
            crate::gate_rules::autogate::translate_edge_to(&drawable, parameter, bound, at).ok()?;
        crate::gate_rules::autogate::admitted_by(&moved, self.index.as_ref()?)
    }

    fn beyond(&self, parameter: &str, bound: Bound, at: f64) -> Option<f64> {
        beyond(&self.sample.points, parameter == self.sample.x, at, bound)
    }

    fn line(&self, parameter: &str, bound: Bound, at: Option<f64>) -> Line {
        Line {
            at,
            holds: at.and_then(|at| self.held_at(parameter, bound, at)),
            beyond: at.and_then(|at| self.beyond(parameter, bound, at)),
        }
    }
}

// ── the same input twice ──────────────────────────────────────────────────

/// A gate's shape and where it sits, as bytes to fingerprint: the corner
/// coordinates on its two parameters, with the leading side on `parameter`
/// taken out unless `with_position`. Nothing else about the gate - its id,
/// its label, which files it applies to - changes what a rule does with it.
fn geometry_bytes(
    gate: &flow_gates::Gate,
    parameter: &str,
    bound: Bound,
    with_position: bool,
) -> Vec<u8> {
    let lead = if with_position {
        0.0
    } else {
        crate::gate_rules::autogate::extent_on(&gate.geometry, parameter)
            .map(|(low, high)| match bound {
                Bound::Above => low,
                Bound::Below => high,
            })
            .unwrap_or(0.0)
    };
    let (x, y) = (&gate.parameters.0, &gate.parameters.1);
    // To six figures: a shape slid and slid back differs from itself in the
    // last bit of a float, and no rule can tell those apart.
    let point = |n: &flow_gates::GateNode, out: &mut Vec<u8>| {
        for p in [x, y] {
            let v = n.get_coordinate(p).unwrap_or(f32::NAN) as f64;
            let v = if **p == *parameter {
                v - lead as f64
            } else {
                v
            };
            let v = if v.abs() < 1e-9 { 0.0 } else { v };
            out.extend_from_slice(format!("{v:.5e};").as_bytes());
        }
    };
    let mut out = Vec::new();
    out.extend_from_slice(x.as_bytes());
    out.push(0);
    out.extend_from_slice(y.as_bytes());
    out.push(0);
    match &gate.geometry {
        flow_gates::GateGeometry::Rectangle { min, max } => {
            out.push(b'R');
            point(min, &mut out);
            point(max, &mut out);
        }
        flow_gates::GateGeometry::Polygon { nodes, .. } => {
            out.push(b'P');
            for n in nodes {
                point(n, &mut out);
            }
        }
        // Not a shape a rule slides; any difference counts as a difference.
        other => {
            out.push(b'O');
            out.extend_from_slice(&serde_json::to_vec(other).unwrap_or_default());
        }
    }
    out
}

/// Whether where the gate starts on the sample changes where `rule` puts it.
///
/// A band rule starts its search from there and keeps a gate already in its
/// band; the finder that refines from the gate starts from there. The other
/// rules read only the population and the reference: the peak finder and
/// the valley find the population's own features, and a percentile is a
/// percentile wherever the gate began.
fn starting_position_matters(rule: &GateRule) -> bool {
    use crate::gate_rules::rule::NegativeFinder;
    match &rule.rule {
        Rule::TailFraction(_) => true,
        Rule::AboveTheNegative(r) => r.find == NegativeFinder::BelowTheGate,
        Rule::PercentileOffset(_) | Rule::InTheValley(_) | Rule::MatchThePhenotype(_) => false,
        // Where it goes is read from another gate, not from where it began.
        Rule::FromAnotherGate(_) => false,
        // Grown up to another gate, its far side stays where it began.
        Rule::NextToGate(_) => true,
    }
}

/// The fingerprint of what went into one placement: the rule; the sample's
/// population, its metadata, and its gate's shape - and where that gate
/// started, when the rule reads it; and the same of the file the rule read,
/// whose gate's position always counts, since the rule is calibrated on it.
fn input_of(
    rule: &GateRule,
    sample: &EventSample,
    reference: Option<&EventSample>,
    metadata: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<String> {
    let parameter = rule.parameter.as_ref();
    let mut h = blake3::Hasher::new();
    h.update(b"clingate placement input 1\0");
    h.update(&serde_json::to_vec(rule).ok()?);
    let one = |s: &EventSample, with_position: bool, h: &mut blake3::Hasher| {
        h.update(s.gate_id.as_bytes());
        h.update(&[0]);
        h.update(s.parent_gate.as_deref().unwrap_or("").as_bytes());
        h.update(&[0]);
        h.update(s.file.as_bytes());
        h.update(&[0]);
        h.update(super::events::population_key(s).as_bytes());
        if let Some(row) = metadata.get(&s.file) {
            h.update(&serde_json::to_vec(row).unwrap_or_default());
        }
        match &s.gate {
            Some(g) => h.update(&geometry_bytes(g, parameter, rule.bound, with_position)),
            None => h.update(b"no gate"),
        };
    };
    one(sample, starting_position_matters(rule), &mut h);
    match reference {
        Some(r) if r.file != sample.file => {
            h.update(b"read");
            one(r, true, &mut h);
        }
        // The rule read the sample itself - calibrated on its own gate, so
        // where that gate started counts after all.
        _ => {
            if let Some(g) = &sample.gate {
                h.update(b"itself");
                h.update(&geometry_bytes(g, parameter, rule.bound, true));
            }
        }
    }
    Some(h.finalize().to_hex()[..16].to_string())
}

// ── replaying ─────────────────────────────────────────────────────────────

/// Where the review says a gate belongs, and the gate the reviewer left
/// when it keeps one.
struct Judged {
    truth: Truth,
    extent: Option<Vec<ExtentRecord>>,
    gate: Option<flow_gates::Gate>,
}

fn truth_of(
    outcome: &Outcome,
    gate_id: &str,
    sample: &str,
    reports: &[PlacementReport],
) -> Option<Judged> {
    let report_for = || {
        reports
            .iter()
            .filter(|r| r.gate_id == gate_id && r.sample.id == sample)
            .max_by(|a, b| a.reported_at.cmp(&b.reported_at))
    };
    let judged = |truth, extent, gate| Judged {
        truth,
        extent,
        gate,
    };
    Some(match outcome {
        Outcome::Accepted => judged(Truth::AcceptedAsPlaced, None, None),
        Outcome::MovedUnreported { gate_at, gate } => {
            judged(Truth::MovedByHand, Some(gate_at.clone()), gate.clone())
        }
        Outcome::Gone => return None,
        Outcome::Reported { .. } => {
            let report = report_for();
            let problem = report.map_or(Problem::Other, |r| r.problem);
            let note = report.map(|r| r.note.clone()).unwrap_or_default();
            match report.and_then(|r| r.correction.as_ref()) {
                Some(fix) => judged(
                    Truth::Corrected { problem, note },
                    Some(fix.gate_at.clone()),
                    fix.gate.clone(),
                ),
                None => judged(Truth::ReportedWithoutFix { problem, note }, None, None),
            }
        }
    })
}

/// Whether the reviewer's gate is the gate before the run, moved - the same
/// extent on every parameter but the rule's, and the same width on that.
fn only_moved(before: &flow_gates::Gate, after: &[ExtentRecord], parameter: &str) -> bool {
    use crate::gate_rules::autogate::extent_on;
    let (x, y) = (&before.parameters.0, &before.parameters.1);
    [x, y].into_iter().all(|p| {
        let Some((low, high)) = extent_on(&before.geometry, p) else {
            return false;
        };
        let Some(e) = after.iter().find(|e| e.parameter == **p) else {
            return false;
        };
        let (Some(l), Some(h)) = (e.lower, e.upper) else {
            return false;
        };
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-4 * (1.0 + a.abs().max(b.abs()));
        if **p == *parameter {
            near(h - l, (high - low) as f64)
        } else {
            near(l, low as f64) && near(h, high as f64)
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
        repeated_later: 0,
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
                    holds: None,
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
                    holds: None,
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
        let Some(mut judged) = truth_of(outcome, &gate_id, &sample.id, &run.reports) else {
            continue;
        };
        if matches!(judged.truth, Truth::AcceptedAsPlaced) && !run_decided.moved {
            judged.truth = Truth::AcceptedLeftAlone;
        }
        let rule = rules.rule_for(&gate, parent_gate.as_deref());
        let parameter = rule.map(|r| r.parameter.to_string());
        let bound = rule.map(|r| r.bound).or(bound);
        let find = |file: &str| {
            usable.and_then(|e| {
                e.samples
                    .iter()
                    .find(|s| s.gate_id == gate_id && s.file == file)
            })
        };
        let events = find(&sample.id);
        let counted = events.map(Counted::new);
        let line = |at: Option<f64>| -> Line {
            match (&counted, parameter.as_deref(), bound) {
                (Some(c), Some(p), Some(b)) => c.line(p, b, at),
                _ => Line {
                    at,
                    holds: None,
                    beyond: None,
                },
            }
        };
        let mut notes = Vec::new();

        let right_at = match &judged.truth {
            Truth::AcceptedAsPlaced | Truth::AcceptedLeftAlone => run_decided.line.at,
            _ => judged
                .extent
                .as_ref()
                .zip(parameter.as_deref())
                .zip(bound)
                .and_then(|((e, p), b)| line_of(e, p, b)),
        };
        let mut right = line(right_at);
        // Where the reviewer left the gate itself, when the review kept it:
        // what it holds is counted from that, reshaped or not.
        if let (Some(fixed), Some(c)) = (&judged.gate, &counted) {
            right.holds = c.held_by(fixed);
        } else if let (Some(extent), Some(before), Some(p)) = (
            &judged.extent,
            events.and_then(|e| e.gate.as_ref()),
            parameter.as_deref(),
        ) && !only_moved(before, extent, p)
        {
            notes.push(
                "the reviewer reshaped the gate, and this review predates keeping the shape: \
                 it is judged as the gate before the run moved to the reviewer's line"
                    .to_string(),
            );
        }

        let mut run_decided = run_decided;
        run_decided.line = line(run_decided.line.at);

        let key = (gate_id.clone(), sample.id.clone());
        let baseline_decided = baseline
            .as_ref()
            .and_then(|b| b.decided.get(&key))
            .map(|d| {
                let mut d = d.clone();
                d.line = line(d.line.at);
                d
            });
        let kept_n = counted.as_ref().map_or(1, |c| c.kept());
        let reproduced = match (&baseline_decided, run_decided.line.holds) {
            (Some(b), Some(was)) => b
                .line
                .holds
                .is_some_and(|now| close_enough(now, was, kept_n)),
            (Some(b), None) => b.line.at == run_decided.line.at,
            (None, _) => false,
        };
        let (replay_decided, why) = match solved {
            None => (None, not_replayable.clone()),
            Some(solved) => match solved.decided.get(&key) {
                Some(d) => {
                    let mut d = d.clone();
                    d.line = line(d.line.at);
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

        let verdict = match (&replay_decided, right.holds) {
            (None, _) => Verdict::NotReplayed,
            (Some(_), _) if !reproduced => Verdict::NotReproduced,
            (Some(replayed), Some(right)) => {
                let was = run_decided
                    .line
                    .holds
                    .map(|b| close_enough(b, right, kept_n));
                let now = replayed.line.holds.map(|b| close_enough(b, right, kept_n));
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
                let same = match (replayed.line.holds, base.line.holds) {
                    (Some(a), Some(b)) => close_enough(a, b, kept_n),
                    _ => replayed.line.at == base.line.at,
                };
                if same {
                    Verdict::Unchanged
                } else {
                    Verdict::Changed
                }
            }
        };

        // The input is the run's: its own rule, and the file it read.
        let input = run
            .rules
            .rule_for(&gate, parent_gate.as_deref())
            .zip(events)
            .and_then(|(rule, events)| {
                let read = run_decided
                    .measured_on
                    .as_deref()
                    .and_then(|file| find(file));
                input_of(rule, events, read, &usable?.metadata)
            });

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
            truth: judged.truth,
            right,
            run_decided,
            baseline_decided,
            reproduced,
            replay_decided,
            verdict,
            why,
            provisional: run.provisional,
            input,
            earlier_reviews: Vec::new(),
            notes,
        });
    }
    (summary(cases.len(), not_replayable), cases)
}

/// Replay every run given, with `changes`.
///
/// A placement whose input a later run repeated exactly - the same rule on
/// the same population from the same gate - is the same experiment run
/// twice. It is counted once, with the later run and its review; what the
/// earlier reviews said is kept with it, since two reviews of one placement
/// disagreeing is worth knowing.
pub fn replay(runs: &[ReviewedRun], changes: &[RuleChange]) -> Replay {
    let mut order: Vec<&ReviewedRun> = runs.iter().collect();
    order.sort_by(|a, b| a.applied_at.cmp(&b.applied_at));
    let mut out = Replay {
        runs: Vec::new(),
        cases: Vec::new(),
    };
    let mut at_input: BTreeMap<String, usize> = BTreeMap::new();
    for run in order {
        let (summary, cases) = replay_run(run, changes);
        out.runs.push(summary);
        for mut case in cases {
            let Some(input) = case.input.clone() else {
                out.cases.push(case);
                continue;
            };
            match at_input.get(&input) {
                Some(&i) => {
                    let earlier = std::mem::replace(
                        &mut out.cases[i],
                        Case {
                            earlier_reviews: Vec::new(),
                            ..case.clone()
                        },
                    );
                    if let Some(r) = out.runs.iter_mut().find(|r| {
                        r.run == earlier.run && r.run_applied_at == earlier.run_applied_at
                    }) {
                        r.repeated_later += 1;
                    }
                    case.earlier_reviews = earlier.earlier_reviews;
                    case.earlier_reviews.push(EarlierReview {
                        run: earlier.run,
                        run_applied_at: earlier.run_applied_at,
                        truth: earlier.truth,
                    });
                    out.cases[i] = case;
                }
                None => {
                    at_input.insert(input, out.cases.len());
                    out.cases.push(case);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_enough_is_a_fifth_of_the_smaller_side_or_three_events() {
        let many = 100_000;
        assert!(close_enough(0.11, 0.10, many));
        assert!(!close_enough(0.13, 0.10, many));
        // A gate holding nearly everything is judged on what it leaves out.
        assert!(close_enough(0.965, 0.97, many));
        assert!(!close_enough(0.96, 0.97, many));
        // Below a few events' worth, the difference is sampling.
        assert!(close_enough(0.004, 0.001, 1_000));
        assert!(!close_enough(0.0045, 0.001, 1_000));
        assert!(!close_enough(0.004, 0.001, many));
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

    // ── what a gate holds, and what counts as the same input ──────────────

    use crate::gate_rules::rule::{
        AboveTheNegativeRule, NegativeFinder, PercentileOffsetRule, TailFractionRule,
    };
    use crate::gate_rules::rule_store::MeasuredOn;

    /// A parallelogram leaning right: its left side runs from (1, 0) to
    /// (2, 2), so the boundary sits at x = 1 + y / 2 - not at its leftmost
    /// corner for any event above the bottom.
    fn leaning(id: &str) -> flow_gates::Gate {
        flow_gates::Gate {
            id: Arc::from(id),
            name: "leaning".into(),
            geometry: flow_gates::create_polygon_geometry(
                vec![(1.0, 0.0), (3.0, 0.0), (4.0, 2.0), (2.0, 2.0)],
                "m",
                "o",
            )
            .unwrap(),
            mode: flow_gates::GateMode::Global,
            parameters: (Arc::from("m"), Arc::from("o")),
            label_position: None,
        }
    }

    fn population(file: &str, gate: flow_gates::Gate) -> EventSample {
        // Two rows of events, clear of every edge the gate is slid to.
        let points: Vec<(f32, f32)> = [0.5f32, 1.5]
            .iter()
            .flat_map(|y| (0..51).map(move |k| (0.02 + 0.1 * k as f32, *y)))
            .collect();
        EventSample {
            gate_id: "g".into(),
            parent_gate: Some("CD4+".into()),
            file: file.into(),
            x: "m".into(),
            y: "o".into(),
            events: points.len(),
            points,
            gate: Some(gate),
        }
    }

    #[test]
    fn a_gate_holds_what_its_real_shape_holds_wherever_it_is_slid() {
        let sample = population("f", leaning("g"));
        let counted = Counted::new(&sample);
        let n = sample.points.len() as f64;
        for at in [1.0, 2.0, 4.0, 0.0] {
            let shift = (at - 1.0) as f32;
            let inside = sample
                .points
                .iter()
                .filter(|(x, y)| *x > 1.0 + y / 2.0 + shift && *x < 3.0 + y / 2.0 + shift)
                .count() as f64;
            let held = counted.held_at("m", Bound::Above, at).unwrap();
            // The plot's statistic is a percentage in single precision.
            assert!(
                (held - inside / n).abs() < 1e-6,
                "at {at}: {held} against {}",
                inside / n
            );
        }
        // Past the line on the parameter alone is another thing entirely.
        let line = counted.line("m", Bound::Above, Some(1.0));
        assert_eq!(line.at, Some(1.0));
        assert!(line.beyond.unwrap() > 2.0 * line.holds.unwrap(), "{line:?}");
        // The gate as it stands, counted directly, is the gate slid nowhere.
        assert_eq!(
            counted.held_by(&leaning("g")),
            counted.held_at("m", Bound::Above, 1.0)
        );
        // No line, nothing to count.
        assert_eq!(
            counted.line("m", Bound::Above, None),
            Line {
                at: None,
                holds: None,
                beyond: None
            }
        );
    }

    #[test]
    fn a_reviewer_s_gate_is_told_apart_from_the_same_gate_moved() {
        let before = leaning("g");
        let moved: Vec<ExtentRecord> = vec![
            ExtentRecord {
                parameter: "m".into(),
                lower: Some(1.5),
                upper: Some(4.5),
            },
            ExtentRecord {
                parameter: "o".into(),
                lower: Some(0.0),
                upper: Some(2.0),
            },
        ];
        assert!(only_moved(&before, &moved, "m"));
        let wider = vec![
            ExtentRecord {
                parameter: "m".into(),
                lower: Some(1.5),
                upper: Some(5.0),
            },
            moved[1].clone(),
        ];
        assert!(!only_moved(&before, &wider, "m"));
        let taller = vec![
            moved[0].clone(),
            ExtentRecord {
                parameter: "o".into(),
                lower: Some(0.0),
                upper: Some(2.5),
            },
        ];
        assert!(!only_moved(&before, &taller, "m"));
    }

    fn rule(kind: Rule, on: MeasuredOn) -> GateRule {
        GateRule {
            parameter: Arc::from("m"),
            bound: Bound::Above,
            measured_on: on,
            rule: kind,
        }
    }

    fn slid(gate: &flow_gates::Gate, by: f64) -> flow_gates::Gate {
        let mut moved = gate.clone();
        let id: GateId = Arc::from(gate.id.as_ref());
        let d = drawable(gate, &id).unwrap();
        let lead = crate::gate_rules::autogate::extent_on(&gate.geometry, "m")
            .unwrap()
            .0 as f64;
        let to = crate::gate_rules::autogate::translate_edge_to(&d, "m", Bound::Above, lead + by)
            .unwrap();
        moved.geometry = to.get_gate_ref(None).unwrap().geometry.clone();
        moved
    }

    #[test]
    fn the_same_rule_on_the_same_populations_from_the_same_gates_is_the_same_input() {
        let meta: BTreeMap<String, BTreeMap<String, String>> = [
            (
                "fs".to_string(),
                BTreeMap::from([("SampleType".to_string(), "FS".to_string())]),
            ),
            (
                "fmx".to_string(),
                BTreeMap::from([("SampleType".to_string(), "FMX".to_string())]),
            ),
        ]
        .into();
        let band = rule(
            Rule::TailFraction(TailFractionRule::new((0.002, 0.005))),
            MeasuredOn::Partner("FMX".into()),
        );
        let fs = population("fs", leaning("g"));
        let fmx = population("fmx", leaning("g"));
        let key = |r: &GateRule,
                   s: &EventSample,
                   read: Option<&EventSample>,
                   m: &BTreeMap<_, _>| { input_of(r, s, read, m).unwrap() };
        let base = key(&band, &fs, Some(&fmx), &meta);
        assert_eq!(base.len(), 16);
        assert_eq!(base, key(&band, &fs.clone(), Some(&fmx.clone()), &meta));

        // What the gate is called, its label, the files it applies to: nothing
        // a rule reads.
        let mut renamed = fs.clone();
        let g = renamed.gate.as_mut().unwrap();
        g.id = Arc::from("another id");
        g.name = "renamed".into();
        g.mode = flow_gates::GateMode::FileSpecific {
            guid: Arc::from("x"),
        };
        assert_eq!(base, key(&band, &renamed, Some(&fmx), &meta));

        // Anything the rule reads.
        let mut other_band = band.clone();
        other_band.rule = Rule::TailFraction(TailFractionRule::new((0.002, 0.006)));
        assert_ne!(base, key(&other_band, &fs, Some(&fmx), &meta));
        let mut one_event = fs.clone();
        one_event.points[3].0 += 0.01;
        assert_ne!(base, key(&band, &one_event, Some(&fmx), &meta));
        let mut fmx_event = fmx.clone();
        fmx_event.points[3].0 += 0.01;
        assert_ne!(base, key(&band, &fs, Some(&fmx_event), &meta));
        let mut reference_moved = fmx.clone();
        reference_moved.gate = Some(slid(&leaning("g"), 0.3));
        assert_ne!(base, key(&band, &fs, Some(&reference_moved), &meta));
        let mut reshaped = fs.clone();
        reshaped.gate = Some(flow_gates::Gate {
            geometry: flow_gates::create_polygon_geometry(
                vec![(1.0, 0.0), (3.0, 0.0), (4.5, 2.0), (2.0, 2.0)],
                "m",
                "o",
            )
            .unwrap(),
            ..leaning("g")
        });
        assert_ne!(base, key(&band, &reshaped, Some(&fmx), &meta));
        let mut relabelled = meta.clone();
        relabelled
            .get_mut("fs")
            .unwrap()
            .insert("SampleType".into(), "FMX".into());
        assert_ne!(base, key(&band, &fs, Some(&fmx), &relabelled));
    }

    #[test]
    fn where_the_gate_started_on_the_sample_counts_only_for_the_rules_that_read_it() {
        let meta = BTreeMap::new();
        let fmx = population("fmx", leaning("g"));
        let fs = population("fs", leaning("g"));
        let mut fs_started_elsewhere = fs.clone();
        fs_started_elsewhere.gate = Some(slid(&leaning("g"), 0.4));
        let partner = MeasuredOn::Partner("FMX".into());
        let same = |r: GateRule| {
            input_of(&r, &fs, Some(&fmx), &meta)
                == input_of(&r, &fs_started_elsewhere, Some(&fmx), &meta)
        };
        // A band search starts there, and keeps a gate already in its band.
        assert!(!same(rule(
            Rule::TailFraction(TailFractionRule::new((0.1, 0.2))),
            partner.clone()
        )));
        // The finder that refines from the gate starts there.
        assert!(!same(rule(
            Rule::AboveTheNegative(AboveTheNegativeRule {
                find: NegativeFinder::BelowTheGate,
                ..AboveTheNegativeRule::default()
            }),
            partner.clone()
        )));
        // The others read the population and the reference alone.
        assert!(same(rule(
            Rule::AboveTheNegative(AboveTheNegativeRule {
                find: NegativeFinder::NegativePeak,
                ..AboveTheNegativeRule::default()
            }),
            partner.clone()
        )));
        assert!(same(rule(
            Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.2)),
            partner.clone()
        )));
        assert!(same(rule(Rule::InTheValley(Default::default()), partner)));
        // Read on the sample itself, the rule is calibrated on where its own
        // gate stood - so that counts for every rule.
        let itself = rule(
            Rule::PercentileOffset(PercentileOffsetRule::new(99.0, 0.2)),
            MeasuredOn::Itself,
        );
        assert_ne!(
            input_of(&itself, &fs, None, &meta),
            input_of(&itself, &fs_started_elsewhere, None, &meta)
        );
    }
}
