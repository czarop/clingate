//! A reviewer's report of a gate the rules placed badly - one the confidence
//! scores did not catch.
//!
//! Reports are how the scores get better without anyone sitting down to
//! label data: each one says what the rule did, every measure its confidence
//! was taken from, what the data looked like, what the reviewer thought was
//! wrong, and - once they save their fix - where the gate should have gone.
//! Gathered over many runs, they show which measures should have caught what.
//!
//! A report is about one gate on one sample. It keeps:
//!
//! - what the rule did for it, from the run kept in `reviews` (see
//!   [`super::run_record`]) - or that no rule placed it;
//! - the parent population on the gate's two parameters, for the sample and
//!   for the sample the rule read: a histogram of each parameter, a coarse
//!   two-dimensional density, and a subsample of up to
//!   [`SUBSAMPLE_EVENTS`] events, so the case can be run again against a
//!   changed rule without the FCS files. Only reports carry events: a run's
//!   ordinary placements never do;
//! - where the gate was when reported, and where it was when the workspace
//!   was next saved - the reviewer's correction ([`record_corrections`]).
//!
//! Reports are kept in the workspace's `reviews/reports` folder and, once a
//! run is marked reviewed ([`mark_reviewed`]), copied with the review into the
//! review library - a folder shared across runs (see [`super::library`]).
//!
//! The app's report dialog and the tools for Claude both report through
//! [`gather`], so a report says the same whichever made it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::run_record::{
    ExtentRecord, KeptRecord, PlacedRecord, PlacementStatus, RunRecord, SampleRef, Samples,
    extent_of, placement_status,
};
use crate::gate_rules::run::RunInputs;
use crate::gates::GateState;
use crate::gates::gate_store::{FileId, NodeId};
use crate::omiq::serialise::AxisSettings;

/// The folder in `reviews` that reports are kept in.
pub const REPORTS_DIR: &str = "reports";
/// The file a run's review is kept in, in `reviews`.
pub const REVIEW_FILE: &str = "review.json";
/// Bumped when the shape of a report changes in a way an older reader would
/// misread.
pub const FORMAT: u32 = 1;
/// At most this many of a population's events are kept in a report.
pub const SUBSAMPLE_EVENTS: usize = 5_000;
/// Bins across each parameter's axis, for the histograms and each side of
/// the density.
pub const BINS: usize = 64;

/// What the reviewer thought was wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The line is too high: positives left out.
    TooHigh,
    /// The line is too low: negatives let in.
    TooLow,
    /// The gate cuts through a population rather than between two.
    CutsThroughAPopulation,
    /// The gate is round the wrong cells.
    WrongPopulation,
    /// The gate is too tight round the right cells.
    TooTight,
    /// The gate is too loose round the right cells.
    TooLoose,
    /// The rule read the wrong sample to decide.
    WrongReference,
    /// The rule should have left the gate alone - too few events, no clear
    /// structure.
    ShouldNotHaveMoved,
    Other,
}

impl Problem {
    pub const ALL: [Problem; 9] = [
        Problem::TooHigh,
        Problem::TooLow,
        Problem::CutsThroughAPopulation,
        Problem::WrongPopulation,
        Problem::TooTight,
        Problem::TooLoose,
        Problem::WrongReference,
        Problem::ShouldNotHaveMoved,
        Problem::Other,
    ];

    /// As a reviewer would say it.
    pub fn describe(self) -> &'static str {
        match self {
            Problem::TooHigh => "too high - positives left out",
            Problem::TooLow => "too low - negatives let in",
            Problem::CutsThroughAPopulation => "cuts through a population",
            Problem::WrongPopulation => "round the wrong cells",
            Problem::TooTight => "too tight",
            Problem::TooLoose => "too loose",
            Problem::WrongReference => "the rule read the wrong sample",
            Problem::ShouldNotHaveMoved => "should not have moved",
            Problem::Other => "something else (see the note)",
        }
    }

    /// The name it is written under, and accepted from the tools.
    pub fn key(self) -> &'static str {
        match self {
            Problem::TooHigh => "too_high",
            Problem::TooLow => "too_low",
            Problem::CutsThroughAPopulation => "cuts_through_a_population",
            Problem::WrongPopulation => "wrong_population",
            Problem::TooTight => "too_tight",
            Problem::TooLoose => "too_loose",
            Problem::WrongReference => "wrong_reference",
            Problem::ShouldNotHaveMoved => "should_not_have_moved",
            Problem::Other => "other",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim().to_lowercase().replace([' ', '-'], "_");
        Self::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// Counts in equal bins between `lower` and `upper`; events outside fall in
/// the end bins, so none is lost from the total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Histogram {
    pub parameter: String,
    pub lower: f64,
    pub upper: f64,
    pub counts: Vec<u32>,
}

/// Counts on a grid over the gate's two parameters, rows along y from the
/// bottom, each row along x.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Density {
    pub x: String,
    pub y: String,
    pub x_range: (f64, f64),
    pub y_range: (f64, f64),
    pub bins: usize,
    pub counts: Vec<u32>,
}

/// A population on the gate's two parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopulationData {
    pub sample: SampleRef,
    pub events: usize,
    pub histograms: (Histogram, Histogram),
    pub density: Density,
    /// Up to [`SUBSAMPLE_EVENTS`] of its events, evenly through the file, as
    /// (x, y) in the plot's units.
    pub events_subsample: Vec<(f32, f32)>,
    /// Where the gate sat on this sample when it was reported.
    pub gate_at: Vec<ExtentRecord>,
}

/// What the rules decided for the reported gate on the reported sample, in
/// the last applied run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Placed(Box<PlacedRecord>),
    Kept(KeptRecord),
    /// No rule placed it in the last applied run - there was no run, no rule
    /// for this gate, or the run could not place it.
    NotPlaced {
        why: String,
    },
}

/// Where the reviewer put the gate: where it was when the workspace was
/// saved after the report, if that is not where the rule put it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    pub gate_at: Vec<ExtentRecord>,
    pub saved_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementReport {
    pub format: u32,
    pub id: String,
    pub reported_at: String,
    /// The workspace folder the report was made in.
    pub workspace: String,
    pub gate_id: String,
    pub gate: String,
    pub parent_gate: Option<String>,
    /// The gate from the root down, names joined with " > ".
    pub path: String,
    pub sample: SampleRef,
    /// Every metadata value of the sample: what it was, when, which plate.
    pub sample_metadata: BTreeMap<String, String>,
    pub problem: Problem,
    pub note: String,
    /// The rule for this gate, as it stood for the run.
    pub rule: Option<crate::gate_rules::rule_store::GateRule>,
    /// When the run that placed it was applied.
    pub run_applied_at: Option<String>,
    pub decision: Decision,
    /// The population on this sample, and on the sample the rule read.
    pub data: PopulationData,
    pub reference_data: Option<PopulationData>,
    /// Filled in when the workspace is saved after the report.
    pub correction: Option<Correction>,
}

/// What a report is about, and what the reviewer said.
pub struct ReportRequest {
    /// Where the gate sits in the tree - one gate can appear in several
    /// places.
    pub node: NodeId,
    /// The sample, by gating id.
    pub sample: FileId,
    pub problem: Problem,
    pub note: String,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The gate at `node` and the gate names above it, root first.
fn gate_path(state: &GateState, node: &NodeId) -> Vec<String> {
    state
        .gate_chain_for_node(node)
        .iter()
        .filter_map(|id| state.registered_gate(id))
        .map(|g| g.get_name().to_string())
        .collect()
}

/// Where `gate_id` sits directly under the population at `parent`, or the
/// root when there is none - how the gate editor names a gate: the one
/// selected on a plot of `parent`.
pub fn node_under(
    state: &GateState,
    gate_id: &crate::gates::gate_store::GateId,
    parent: Option<&NodeId>,
) -> Option<NodeId> {
    let nodes = state.nodes_for_gate(gate_id);
    nodes
        .iter()
        .find(|node| {
            let above = state.parent_node(node);
            match (above, parent) {
                (Some(above), Some(parent)) => &above == parent,
                (None, None) => true,
                // The root may be named or not.
                (Some(above), None) => above.as_str() == &**crate::gates::gate_store::ROOTGATE,
                (None, Some(parent)) => parent.as_str() == &**crate::gates::gate_store::ROOTGATE,
            }
        })
        .or_else(|| nodes.first())
        .cloned()
}

/// Where the gate `gate_id` sits in the tree under the gate named
/// `parent_gate` - how a run names a gate - or its first place when that does
/// not settle it.
pub fn node_named(state: &GateState, gate_id: &str, parent_gate: Option<&str>) -> Option<NodeId> {
    let id: crate::gates::gate_store::GateId = Arc::from(gate_id);
    let nodes = state.nodes_for_gate(&id);
    nodes
        .iter()
        .find(|node| {
            let path = gate_path(state, node);
            let parent = (path.len() > 1).then(|| path[path.len() - 2].as_str());
            parent == parent_gate
        })
        .or_else(|| nodes.first())
        .cloned()
}

fn histogram(parameter: &str, values: impl Iterator<Item = f32>, range: (f64, f64)) -> Histogram {
    let (lower, upper) = range;
    let width = (upper - lower) / BINS as f64;
    let mut counts = vec![0u32; BINS];
    for v in values {
        let at = if width > 0.0 {
            ((v as f64 - lower) / width).floor()
        } else {
            0.0
        };
        let at = at.clamp(0.0, (BINS - 1) as f64) as usize;
        counts[at] += 1;
    }
    Histogram {
        parameter: parameter.to_string(),
        lower,
        upper,
        counts,
    }
}

fn density(x: &str, y: &str, points: &[(f32, f32)], xr: (f64, f64), yr: (f64, f64)) -> Density {
    let bin = |v: f32, (lo, hi): (f64, f64)| {
        let w = (hi - lo) / BINS as f64;
        let at = if w > 0.0 {
            ((v as f64 - lo) / w).floor()
        } else {
            0.0
        };
        at.clamp(0.0, (BINS - 1) as f64) as usize
    };
    let mut counts = vec![0u32; BINS * BINS];
    for &(px, py) in points {
        counts[bin(py, yr) * BINS + bin(px, xr)] += 1;
    }
    Density {
        x: x.to_string(),
        y: y.to_string(),
        x_range: xr,
        y_range: yr,
        bins: BINS,
        counts,
    }
}

/// Every `n`th event, so the subsample runs evenly through the file rather
/// than stopping at its first few thousand.
fn subsample(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if points.len() <= SUBSAMPLE_EVENTS {
        return points.to_vec();
    }
    let step = points.len() as f64 / SUBSAMPLE_EVENTS as f64;
    (0..SUBSAMPLE_EVENTS)
        .map(|i| points[(i as f64 * step) as usize])
        .collect()
}

/// A parameter's axis, as the plots draw it; the data's own range where the
/// scaling says nothing.
fn axis_range(axes: &AxisSettings, parameter: &str, values: &[f32]) -> (f64, f64) {
    if let Some(info) = axes.get(parameter)
        && info.axis_upper > info.axis_lower
    {
        return (info.axis_lower as f64, info.axis_upper as f64);
    }
    let finite = values.iter().copied().filter(|v| v.is_finite());
    let (lo, hi) = finite.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    if lo.is_finite() && hi > lo {
        (lo as f64, hi as f64)
    } else {
        (0.0, 1.0)
    }
}

/// The parent population of the gate at `node`, on the gate's parameters, in
/// one sample.
fn population_data(
    state: &GateState,
    node: &NodeId,
    sample: &FileId,
    inputs: &RunInputs,
    axes: &AxisSettings,
    samples: &Samples,
) -> Result<PopulationData, String> {
    let gate_id = state
        .gate_for_node(node)
        .cloned()
        .ok_or("that population has no gate")?;
    let chain = state.gate_chain_for_node(node);
    let above = &chain[..chain.len().saturating_sub(1)];
    let gate = state
        .registered_gate(&gate_id)
        .ok_or("the gate is no longer in the document")?;
    let (x, y) = gate.get_params();

    let name = inputs
        .names
        .iter()
        .find(|(_, id)| *id == sample)
        .map(|(name, _)| name.clone())
        .ok_or_else(|| format!("{sample}: no metadata row names this sample"))?;
    let path = inputs
        .files
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| p.clone())
        .ok_or_else(|| format!("{name}: the file is not in the workspace"))?;

    let frame = crate::events::read_scaled(
        &path,
        &inputs.compensation.matrix_for(&path),
        &inputs.cofactors,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    let empty = Default::default();
    let groups = inputs.metadata.get(sample).unwrap_or(&empty);
    let resolver = state.get_current_sample(sample.clone(), groups);
    let parent = crate::events::under_chain(&frame, above, &resolver).map_err(|e| e.to_string())?;
    let points = crate::events::points(&parent, &x, &y).map_err(|e| e.to_string())?;

    let xs: Vec<f32> = points.iter().map(|p| p.0).collect();
    let ys: Vec<f32> = points.iter().map(|p| p.1).collect();
    let xr = axis_range(axes, &x, &xs);
    let yr = axis_range(axes, &y, &ys);
    let here = state
        .gate_for_file(&gate_id, sample, &inputs.metadata)
        .map(|g| extent_of(g.as_ref()))
        .unwrap_or_default();

    Ok(PopulationData {
        sample: samples.sample(sample),
        events: points.len(),
        histograms: (
            histogram(&x, xs.iter().copied(), xr),
            histogram(&y, ys.iter().copied(), yr),
        ),
        density: density(&x, &y, &points, xr, yr),
        events_subsample: subsample(&points),
        gate_at: here,
    })
}

/// Everything a report of `request` keeps, gathered from the workspace as it
/// stands and the run kept in `folder`. Reads the sample's file, and the
/// file the rule read.
pub fn gather(
    folder: &Path,
    request: &ReportRequest,
    state: &GateState,
    inputs: &RunInputs,
    axes: &AxisSettings,
) -> Result<PlacementReport, String> {
    let gate_id = state
        .gate_for_node(&request.node)
        .cloned()
        .ok_or("that population has no gate")?;
    let path = gate_path(state, &request.node);
    let gate = path.last().cloned().unwrap_or_default();
    let parent_gate = (path.len() > 1).then(|| path[path.len() - 2].clone());

    let run = RunRecord::load(folder).map_err(|e| e.to_string())?;
    // The rules as they stood for the run; the workspace's own without one.
    let rules = run.as_ref().map(|r| &r.rules).unwrap_or(&inputs.rules);
    let samples = Samples::new(&inputs.names, &inputs.metadata, &rules.pairing);

    let placed = run.as_ref().and_then(|r| {
        r.placed
            .iter()
            .find(|p| p.gate_id == *gate_id && p.sample.id == *request.sample)
    });
    let kept = run.as_ref().and_then(|r| {
        r.kept
            .iter()
            .find(|k| k.gate_id == *gate_id && k.sample.id == *request.sample)
    });
    let decision = match (placed, kept, &run) {
        (Some(p), _, _) => Decision::Placed(Box::new(p.clone())),
        (None, Some(k), _) => Decision::Kept(k.clone()),
        (None, None, None) => Decision::NotPlaced {
            why: "no rules run has been applied in this workspace".into(),
        },
        (None, None, Some(r)) => Decision::NotPlaced {
            why: match r
                .skipped
                .iter()
                .find(|s| s.gate == gate && s.sample.id == *request.sample)
            {
                Some(s) => format!("the run could not place it: {}", s.reason),
                None => "the last run did not place this gate for this sample".into(),
            },
        },
    };

    let data = population_data(
        state,
        &request.node,
        &request.sample,
        inputs,
        axes,
        &samples,
    )?;
    let reference_data = match placed {
        Some(p) if p.measured_on.id != *request.sample => {
            let reference: FileId = Arc::from(p.measured_on.id.as_str());
            Some(population_data(
                state,
                &request.node,
                &reference,
                inputs,
                axes,
                &samples,
            )?)
        }
        _ => None,
    };

    Ok(PlacementReport {
        format: FORMAT,
        id: uuid::Uuid::new_v4().to_string(),
        reported_at: now(),
        workspace: folder.display().to_string(),
        gate_id: gate_id.to_string(),
        gate,
        parent_gate: parent_gate.clone(),
        path: path.join(" > "),
        sample: samples.sample(&request.sample),
        sample_metadata: inputs
            .metadata
            .get(&request.sample)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        problem: request.problem,
        note: request.note.trim().to_string(),
        rule: rules
            .rule_for(
                &path.last().cloned().unwrap_or_default(),
                parent_gate.as_deref(),
            )
            .cloned(),
        run_applied_at: run.as_ref().map(|r| r.applied_at.clone()),
        decision,
        data,
        reference_data,
        correction: None,
    })
}

/// The folder a workspace keeps its reports in.
pub fn reports_dir(folder: &Path) -> PathBuf {
    folder.join(super::REVIEWS_DIR).join(REPORTS_DIR)
}

impl PlacementReport {
    fn file_name(&self) -> String {
        format!("{}.json", self.id)
    }

    /// Keep this in the workspace's `reviews/reports`, making the folders the
    /// first time.
    pub fn save(&self, folder: &Path) -> anyhow::Result<PathBuf> {
        let path = reports_dir(folder).join(self.file_name());
        crate::workspace::make_parent(&path)?;
        std::fs::write(&path, serde_json::to_string(self)?)?;
        Ok(path)
    }

    /// Whether the gate reported was one a rule placed.
    pub fn placed(&self) -> Option<&PlacedRecord> {
        match &self.decision {
            Decision::Placed(p) => Some(p),
            _ => None,
        }
    }
}

/// The reports kept in a workspace, oldest first. One that cannot be read is
/// passed over with a warning rather than hiding the rest.
pub fn reports_in(folder: &Path) -> Vec<(PathBuf, PlacementReport)> {
    let Ok(entries) = std::fs::read_dir(reports_dir(folder)) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, PlacementReport)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            match serde_json::from_str::<PlacementReport>(&text) {
                Ok(r) => Some((p, r)),
                Err(e) => {
                    tracing::warn!("{} could not be read: {e}", p.display());
                    None
                }
            }
        })
        .collect();
    found.sort_by(|a, b| a.1.reported_at.cmp(&b.1.reported_at));
    found
}

/// The workspace has just been saved: each report's gate is where the
/// reviewer left it, so that is its correction - unless it is back where the
/// rule put it, which is no correction. Returns how many reports changed.
///
/// Called from the one place both the app and the tools save
/// ([`crate::working_copy::WorkingCopy::save`]).
pub fn record_corrections(
    folder: &Path,
    state: &GateState,
    metadata: &crate::omiq::metadata::MetaDataFileMap,
) -> usize {
    let mut changed = 0;
    for (path, mut report) in reports_in(folder) {
        let gate_id: crate::gates::gate_store::GateId = Arc::from(report.gate_id.as_str());
        let sample: FileId = Arc::from(report.sample.id.as_str());
        let Some(gate) = state.gate_for_file(&gate_id, &sample, metadata) else {
            continue;
        };
        let now_at = extent_of(gate.as_ref());
        let as_placed = report
            .placed()
            .is_some_and(|p| placement_status(p, state, metadata) == PlacementStatus::AsPlaced);
        let unchanged_since_report = now_at == report.data.gate_at;
        let correction = if as_placed || (report.placed().is_none() && unchanged_since_report) {
            None
        } else {
            Some(Correction {
                gate_at: now_at,
                saved_at: now(),
            })
        };
        let same = match (&report.correction, &correction) {
            (None, None) => true,
            (Some(a), Some(b)) => a.gate_at == b.gate_at,
            _ => false,
        };
        if same {
            continue;
        }
        report.correction = correction;
        match serde_json::to_string(&report)
            .map_err(anyhow::Error::from)
            .and_then(|t| std::fs::write(&path, t).map_err(Into::into))
        {
            Ok(()) => changed += 1,
            Err(e) => tracing::warn!("{}: the correction could not be kept: {e}", path.display()),
        }
    }
    changed
}

// ── marking a run reviewed ───────────────────────────────────────────────

/// What became of one placement once its run was reviewed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Outcome {
    /// Left as the rule placed it, and not reported: the reviewer accepted it.
    Accepted,
    /// Reported as badly placed.
    Reported { reports: Vec<String> },
    /// Moved by the reviewer without a report - a correction, unexplained.
    MovedUnreported { gate_at: Vec<ExtentRecord> },
    /// Gone from the document.
    Gone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewedPlacement {
    pub placed: PlacedRecord,
    #[serde(flatten)]
    pub outcome: Outcome,
    #[serde(flatten)]
    pub flag: FlagOutcome,
}

/// Whether the assessment flagged a placement, and whether the reviewer then
/// judged it to look right - a flag it should not have raised.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlagOutcome {
    /// The measures the flag was raised on; empty if it was not flagged.
    #[serde(default)]
    pub flagged_on: Vec<String>,
    #[serde(default)]
    pub flag_severity: Option<f64>,
    #[serde(default)]
    pub looked_right: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewedKept {
    pub kept: KeptRecord,
    /// Accepted, or reported: a gate left alone has no placement to have
    /// moved from.
    #[serde(flatten)]
    pub outcome: Outcome,
    #[serde(flatten)]
    pub flag: FlagOutcome,
}

/// A run, reviewed: every placement with what the reviewer made of it - the
/// accepted ones as well as the reported ones, which is what tells a score
/// that flags good gates from one that flags bad ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReview {
    pub format: u32,
    pub reviewed_at: String,
    pub workspace: String,
    pub run_applied_at: String,
    pub rules: crate::gate_rules::rule_store::RuleStore,
    pub placements: Vec<ReviewedPlacement>,
    pub kept: Vec<ReviewedKept>,
    /// Reports about gates no rule placed or kept in the run.
    pub other_reports: Vec<String>,
}

impl RunReview {
    pub fn accepted(&self) -> usize {
        self.placements
            .iter()
            .filter(|p| p.outcome == Outcome::Accepted)
            .count()
    }

    pub fn reported(&self) -> usize {
        self.placements
            .iter()
            .filter(|p| matches!(p.outcome, Outcome::Reported { .. }))
            .count()
            + self
                .kept
                .iter()
                .filter(|k| matches!(k.outcome, Outcome::Reported { .. }))
                .count()
    }

    pub fn moved_unreported(&self) -> usize {
        self.placements
            .iter()
            .filter(|p| matches!(p.outcome, Outcome::MovedUnreported { .. }))
            .count()
    }
}

/// Where a reviewed run is kept in the review library: a folder per run,
/// named for the workspace and when the run was applied.
pub fn library_folder(library: &Path, workspace: &Path, run_applied_at: &str) -> PathBuf {
    let name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".into());
    let when: String = run_applied_at
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " _-.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    library.join(format!("{name}__{when}"))
}

/// Mark the workspace's last applied run as reviewed: every placement that
/// was not reported and is still where the rule put it was accepted; one
/// moved without a report was corrected without one. Kept as
/// `reviews/review.json` and, with the run's reports, copied into `library`
/// when there is one. Marking again replaces the review.
pub fn mark_reviewed(
    folder: &Path,
    state: &GateState,
    metadata: &crate::omiq::metadata::MetaDataFileMap,
    library: Option<&Path>,
) -> Result<(RunReview, Option<PathBuf>), String> {
    let run = RunRecord::load(folder)
        .map_err(|e| e.to_string())?
        .ok_or("no rules run has been applied in this workspace, so there is nothing to review")?;
    // Corrections as the gates stand now, whether or not saved yet.
    let reports = reports_in(folder);
    // What the assessment flagged, and which flags the reviewer cleared.
    let assessment = super::assess::assess(&run, Some((state, metadata)));
    let looks = super::board::LooksRight::load(folder, &run);
    let flag_outcome = |gate_id: &str, sample: &str| {
        let flag = assessment
            .flags
            .iter()
            .find(|f| f.gate_id == gate_id && f.sample.id == sample);
        FlagOutcome {
            flagged_on: flag
                .map(|f| f.reasons.iter().map(|r| r.measure.to_string()).collect())
                .unwrap_or_default(),
            flag_severity: flag.map(|f| f.severity),
            looked_right: looks.contains(gate_id, sample),
        }
    };
    let reports_for = |gate_id: &str, sample: &str| -> Vec<String> {
        reports
            .iter()
            .filter(|(_, r)| r.gate_id == gate_id && r.sample.id == sample)
            .map(|(_, r)| r.id.clone())
            .collect()
    };

    let placements = run
        .placed
        .iter()
        .map(|p| {
            let reported = reports_for(&p.gate_id, &p.sample.id);
            let outcome = if !reported.is_empty() {
                Outcome::Reported { reports: reported }
            } else {
                match placement_status(p, state, metadata) {
                    PlacementStatus::AsPlaced => Outcome::Accepted,
                    PlacementStatus::Gone => Outcome::Gone,
                    PlacementStatus::Moved => {
                        let gate_id: crate::gates::gate_store::GateId =
                            Arc::from(p.gate_id.as_str());
                        let sample: FileId = Arc::from(p.sample.id.as_str());
                        Outcome::MovedUnreported {
                            gate_at: state
                                .gate_for_file(&gate_id, &sample, metadata)
                                .map(|g| extent_of(g.as_ref()))
                                .unwrap_or_default(),
                        }
                    }
                }
            };
            ReviewedPlacement {
                placed: p.clone(),
                outcome,
                flag: flag_outcome(&p.gate_id, &p.sample.id),
            }
        })
        .collect();
    let kept = run
        .kept
        .iter()
        .map(|k| {
            let reported = reports_for(&k.gate_id, &k.sample.id);
            ReviewedKept {
                kept: k.clone(),
                outcome: if reported.is_empty() {
                    Outcome::Accepted
                } else {
                    Outcome::Reported { reports: reported }
                },
                flag: flag_outcome(&k.gate_id, &k.sample.id),
            }
        })
        .collect();
    let covered = |r: &PlacementReport| {
        run.placed
            .iter()
            .any(|p| p.gate_id == r.gate_id && p.sample.id == r.sample.id)
            || run
                .kept
                .iter()
                .any(|k| k.gate_id == r.gate_id && k.sample.id == r.sample.id)
    };
    let review = RunReview {
        format: FORMAT,
        reviewed_at: now(),
        workspace: folder.display().to_string(),
        run_applied_at: run.applied_at.clone(),
        rules: run.rules.clone(),
        placements,
        kept,
        other_reports: reports
            .iter()
            .filter(|(_, r)| !covered(r))
            .map(|(_, r)| r.id.clone())
            .collect(),
    };

    let text = serde_json::to_string_pretty(&review).map_err(|e| e.to_string())?;
    let here = folder.join(super::REVIEWS_DIR).join(REVIEW_FILE);
    crate::workspace::make_parent(&here).map_err(|e| e.to_string())?;
    std::fs::write(&here, &text).map_err(|e| format!("{}: {e}", here.display()))?;

    let copied = match library {
        Some(library) => {
            let into = library_folder(library, folder, &run.applied_at);
            let write = || -> anyhow::Result<()> {
                std::fs::create_dir_all(into.join(REPORTS_DIR))?;
                std::fs::write(into.join(REVIEW_FILE), &text)?;
                for (path, report) in &reports {
                    std::fs::copy(path, into.join(REPORTS_DIR).join(report.file_name()))?;
                }
                Ok(())
            };
            write().map_err(|e| format!("the review library {}: {e}", into.display()))?;
            Some(into)
        }
        None => None,
    };
    Ok((review, copied))
}
