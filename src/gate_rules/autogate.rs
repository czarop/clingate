//! Writing a solved threshold back onto a gate.
//!
//! A rule yields one number: where the line goes on one parameter, for one
//! sample. Everything here is about getting that number onto the gate without
//! disturbing anything else about it.
//!
//! Two decisions are worth stating, because both were measured rather than
//! assumed.
//!
//! **The gate moves, it does not resize.** Across 41 gates over 117 files of a
//! hand-gated export the width of every rectangle held constant to seven
//! figures while the bounding edge moved. So both edges on the rule's parameter
//! shift by the same delta, and the shape a person drew survives.
//!
//! **The position belongs to the specimen, not the file.** The 0.2-0.5% rule
//! measures the FMO and gates the full stain, and both want the same line - the
//! FMO to show that it captures the band, the full stain to read the positives
//! off. That is a group override keyed on the sample id column, which is what
//! [`GateSource::Group`] has always been for; this is the first thing in the
//! app to write one.

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate;
use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_store::{FileId, GateId, GateSource, GateSubStore};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_rules::rule_store::{Bound, SamplePairing};
use crate::omiq::metadata::{MetaDataFileMap, MetaDataKey};
use flow_gates::GateGeometry;
use std::sync::Arc;

/// Beyond this, an edge is Omiq's "unbounded" sentinel (`1e16`) rather than a
/// coordinate. Moving one would be meaningless, and turning one into a real
/// number would close a side the person left open.
const UNBOUNDED: f32 = 1e9;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("{0} is drawn as a shape a rule cannot slide along one axis")]
    UnsupportedShape(GateId),
    #[error("{gate} does not bound {parameter}")]
    NoSuchParameter { gate: GateId, parameter: Arc<str> },
    #[error("the edge of {0} that the rule positions is unbounded, so there is nothing to move")]
    UnboundedEdge(GateId),
    #[error("{0}")]
    Rebuild(String),
}

/// How far a gate reaches along one parameter, whatever shape it is drawn as.
///
/// A rule positions a *line*, and every shape that can sit either side of one
/// has a leading and a trailing extent on the parameter it cuts. Reading that
/// rather than a rectangle's corners is what lets the same rule work on the
/// polygons a real workflow is full of - in one export, 41 of 211 gates.
///
/// Returns `(low, high)`.
pub fn extent_on(geometry: &GateGeometry, parameter: &str) -> Option<(f32, f32)> {
    match geometry {
        GateGeometry::Rectangle { min, max } => Some((
            min.get_coordinate(parameter)?,
            max.get_coordinate(parameter)?,
        )),
        GateGeometry::Polygon { nodes, .. } => {
            let mut low = f32::INFINITY;
            let mut high = f32::NEG_INFINITY;
            for node in nodes {
                let v = node.get_coordinate(parameter)?;
                low = low.min(v);
                high = high.max(v);
            }
            low.is_finite().then_some((low, high))
        }
        // An ellipse carries a rotation, so its extent on a parameter is not
        // simply its radius, and sliding one is a different piece of work.
        // Better to say so than to move it wrongly.
        _ => None,
    }
}

/// The same shape, slid along `parameter` by `delta`.
///
/// Every point moves by the same amount, which is what makes this a
/// translation: the shape a person drew survives intact. An edge at Omiq's
/// `1e16` sentinel stays put - shifting it would be arithmetic on a flag, and
/// rounding it into a real number would close a side left open.
fn slide(geometry: &GateGeometry, parameter: &str, delta: f32) -> GateGeometry {
    let shift = |value: f32| -> f32 {
        if !value.is_finite() || value.abs() > UNBOUNDED {
            value
        } else {
            value + delta
        }
    };
    let mut moved = geometry.clone();
    match &mut moved {
        GateGeometry::Rectangle { min, max } => {
            if let Some(v) = min.get_coordinate(parameter) {
                min.set_coordinate(parameter, shift(v));
            }
            if let Some(v) = max.get_coordinate(parameter) {
                max.set_coordinate(parameter, shift(v));
            }
        }
        GateGeometry::Polygon { nodes, .. } => {
            for node in nodes.iter_mut() {
                if let Some(v) = node.get_coordinate(parameter) {
                    node.set_coordinate(parameter, shift(v));
                }
            }
        }
        _ => {}
    }
    moved
}

/// The same rectangle, shifted along `parameter` so its `bound` edge sits at
/// `to`.
///
/// The edge that moves is the one the gate keeps events *from*: the lower edge
/// for a positive gate, the upper for a negative. Its opposite number moves by
/// the same delta, which is what keeps this a translation. An opposite edge
/// that is unbounded stays unbounded - shifting the sentinel would be
/// arithmetic on a flag.
pub fn translate_edge_to(
    gate: &Arc<dyn DrawableGate>,
    parameter: &str,
    bound: Bound,
    to: f64,
) -> Result<Arc<dyn DrawableGate>, ApplyError> {
    let id = gate.get_id();
    let inner = gate
        .get_gate_ref(None)
        .ok_or_else(|| ApplyError::UnsupportedShape(id.clone()))?;

    let Some((low, high)) = extent_on(&inner.geometry, parameter) else {
        return Err(match &inner.geometry {
            GateGeometry::Rectangle { .. } | GateGeometry::Polygon { .. } => {
                ApplyError::NoSuchParameter {
                    gate: id.clone(),
                    parameter: Arc::from(parameter),
                }
            }
            _ => ApplyError::UnsupportedShape(id.clone()),
        });
    };

    // The edge the rule positions is the one the gate keeps events *from*.
    let leading = match bound {
        Bound::Above => low,
        Bound::Below => high,
    };
    if !leading.is_finite() || leading.abs() > UNBOUNDED {
        return Err(ApplyError::UnboundedEdge(id.clone()));
    }

    let moved_geometry = slide(&inner.geometry, parameter, to as f32 - leading);
    let mut moved = inner.clone();
    moved.geometry = moved_geometry;

    rebuild(moved, gate.is_primary(), &id)
}

fn rebuild(
    moved: flow_gates::Gate,
    is_primary: bool,
    id: &GateId,
) -> Result<Arc<dyn DrawableGate>, ApplyError> {
    let _ = id;
    match &moved.geometry {
        GateGeometry::Polygon { .. } => Ok(Arc::new(
            PolygonGate::try_new(moved, is_primary)
                .map_err(|e| ApplyError::Rebuild(e.to_string()))?,
        )),
        _ => Ok(Arc::new(
            RectangleGate::try_new(moved, is_primary)
                .map_err(|e| ApplyError::Rebuild(e.to_string()))?,
        )),
    }
}

/// The specimen a file belongs to, as a key into the group override tier.
///
/// Named by the pairing rather than hardcoded, because the column that groups a
/// specimen's files is exactly what differs between datasets.
pub fn specimen_of(
    pairing: &SamplePairing,
    file: &FileId,
    metadata: &MetaDataFileMap,
) -> Option<MetaDataKey> {
    let group = metadata.get(file)?.get(&pairing.sample_id_column)?.clone();
    Some(MetaDataKey {
        parameter: pairing.sample_id_column.clone(),
        group,
    })
}

/// Give this specimen its own copy of the gate, leaving every other specimen -
/// and the global position a person drew - untouched.
///
/// A composite is registered under its own id and each of its corners', so all
/// of them need the override or filtering would read the old position while the
/// plot drew the new one. [`GateSubStore::ids_for`] is what knows that.
pub fn place_for_specimen(
    state: &mut GateState,
    gate_id: &GateId,
    specimen: &MetaDataKey,
    gate: &Arc<dyn DrawableGate>,
) {
    let ids = GateSubStore::ids_for(gate, gate_id);
    state.place_gate(
        &ids,
        gate,
        &GateSource::Group((gate_id.clone(), specimen.clone())),
    );
}

// ── measuring what is there now ───────────────────────────────────────────

use crate::gate_editor::gates::gate_filtering::filter_events_by_hierarchy_to_mask;
use crate::gate_editor::gates::gate_stats::get_percent_and_counts_gate;
use crate::gate_editor::gates::gate_store::{GroupId, NodeId};
use crate::gate_editor::gates::gate_types::GateStatValue;
use crate::gate_editor::plots::data_helpers::get_event_mask_from_scaled_df;
use crate::gate_editor::plots::plot_store::EventIndexMapped;
use crate::gate_rules::rule_store::{GateRule, RuleStore};
use crate::omiq::metadata::MetaDataParameter;
use polars::prelude::*;
use rustc_hash::FxHashMap;

/// One gate on one file, as it stands before any rule is applied.
///
/// Only gates a rule actually names are measured. Everything here costs a
/// filtered pass over the frame and holds the population afterwards, and a
/// workflow has two hundred gates of which a handful carry rules.
#[derive(Clone)]
pub struct Measurement {
    pub file: FileId,
    pub gate_id: GateId,
    pub gate: Arc<str>,
    pub parent_gate: Option<Arc<str>>,
    /// The parameter the rule positions - taken from the rule, never guessed
    /// from which edge of the gate happens to look like a threshold.
    pub parameter: Arc<str>,
    pub bound: Bound,
    /// Where the gate's leading extent on that parameter sits now.
    pub current: f64,
    /// The parent population's values on `parameter`, for the solver.
    pub values: Vec<f64>,
    /// The parent population indexed exactly as the plot indexes it, so what a
    /// gate admits can be asked through the very function that draws the
    /// percentage on screen.
    pub index: EventIndexMapped,
    /// The plot's axes, in order.
    pub params: (Arc<str>, Arc<str>),
}

/// Every gate on one file that a rule names, with the population it cuts.
pub fn measure_file(
    state: &GateState,
    file: &FileId,
    df: &DataFrame,
    metadata: &MetaDataFileMap,
    rules: &RuleStore,
) -> anyhow::Result<(Vec<Measurement>, Vec<Unmeasured>)> {
    let mut unmeasured: Vec<Unmeasured> = Vec::new();
    let groups: FxHashMap<MetaDataParameter, GroupId> = metadata
        .get(file)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no metadata for {file}"))?;
    let resolver = state.get_current_sample(file.clone(), &groups);

    let mut out = Vec::new();
    for (node, placement) in state.placements() {
        let gate_id = &placement.gate_id;
        let Some(gate) = state.gate_for_file(gate_id, file, metadata) else {
            continue;
        };
        let name: Arc<str> = Arc::from(gate.get_name());
        let Some(parent) = state.parent_node(node) else {
            continue;
        };
        let parent_gate = parent_name(state, &parent);

        // The rule decides what is measured. Working the other way round -
        // looking at a gate and guessing which of its edges is "the" threshold -
        // is what made a rectangle bounding both axes look like a window with
        // nothing to solve, and rejected it outright.
        let Some(rule) = rules.rule_for(&name, parent_gate.as_deref()) else {
            continue;
        };

        let Some(inner) = gate.get_gate_ref(None) else {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                parent_gate: parent_gate.clone(),
                reason: "this gate has no geometry of its own".to_string(),
            });
            continue;
        };
        let params = gate.get_params();
        if *rule.parameter != *params.0 && *rule.parameter != *params.1 {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                parent_gate: parent_gate.clone(),
                reason: format!(
                    "the rule positions {} but this gate is drawn on {} and {}",
                    rule.parameter, params.0, params.1
                ),
            });
            continue;
        }
        let Some((low, high)) = extent_on(&inner.geometry, &rule.parameter) else {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                parent_gate: parent_gate.clone(),
                reason: "this gate is drawn as a shape a rule cannot slide along one axis"
                    .to_string(),
            });
            continue;
        };
        let current = match rule.bound {
            Bound::Above => low,
            Bound::Below => high,
        } as f64;
        if !current.is_finite() || current.abs() > UNBOUNDED as f64 {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                parent_gate: parent_gate.clone(),
                reason: "the side of this gate the rule positions is unbounded".to_string(),
            });
            continue;
        }

        let chain = state.gate_chain_for_node(&parent);
        let (values, index) =
            match parent_population(df, &chain, &resolver, &params, &rule.parameter) {
                Ok(p) => p,
                Err(e) => {
                    unmeasured.push(Unmeasured {
                        gate_id: gate_id.clone(),
                        gate: name,
                        parent_gate: parent_gate.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
        if values.len() < 2 {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                parent_gate: parent_gate.clone(),
                reason: format!("its parent population holds {} events", values.len()),
            });
            continue;
        }

        out.push(Measurement {
            file: file.clone(),
            gate_id: gate_id.clone(),
            gate: name,
            parent_gate,
            parameter: rule.parameter.clone(),
            bound: rule.bound,
            current,
            values,
            index,
            params,
        });
    }
    Ok((out, unmeasured))
}

fn parent_name(state: &GateState, parent: &NodeId) -> Option<Arc<str>> {
    state
        .gate_for_node(parent)
        .and_then(|id| state.registered_gate(id))
        .map(|g| Arc::from(g.get_name()))
}

/// The parent population, filtered and indexed exactly as the plot does it.
///
/// The same call `data_helpers` makes for the frame behind every plot, and the
/// same index the on-screen statistics are counted from. Sharing the code path
/// is the point: a second implementation could disagree with what a person
/// reads off the screen, and then the two numbers are both defensible and the
/// gate is still wrong.
fn parent_population(
    df: &DataFrame,
    chain: &[GateId],
    resolver: &crate::gate_editor::gates::gate_store::GateOverrideResolver,
    params: &(Arc<str>, Arc<str>),
    parameter: &str,
) -> anyhow::Result<(Vec<f64>, EventIndexMapped)> {
    let frame = if chain.is_empty() {
        df.clone()
    } else {
        let mask = filter_events_by_hierarchy_to_mask(df, chain, resolver)?;
        df.filter(&mask)?
    };
    let frame = Arc::new(frame);

    let values: Vec<f64> = frame
        .column(parameter)?
        .f32()?
        .into_no_null_iter()
        .map(|v| v as f64)
        .collect();

    let event_index =
        get_event_mask_from_scaled_df(frame.clone(), params.0.clone(), params.1.clone())?;
    let index_map: Vec<usize> = (0..frame.height()).collect();

    Ok((
        values,
        EventIndexMapped {
            event_index,
            index_map: Arc::new(index_map),
        },
    ))
}

/// What fraction of a population a gate admits.
///
/// Delegates to [`get_percent_and_counts_gate`] - the function that draws the
/// percentage beside the gate on screen - so the figure a run reports is the
/// figure a person reads back, by construction rather than by agreement.
pub fn admitted_by(gate: &Arc<dyn DrawableGate>, index: &EventIndexMapped) -> Option<f64> {
    let parent_events = index.event_index.len() as f32;
    if parent_events == 0.0 {
        return None;
    }
    let stats = get_percent_and_counts_gate(gate.clone(), index, parent_events).ok()?;
    match stats.percent_parent {
        GateStatValue::Single(percent) => Some(percent as f64 / 100.0),
        // A composite reports one figure per corner; a rule positions a single
        // gate, so there is no one number to compare against a band.
        GateStatValue::Composite(_) => None,
    }
}

/// Slide a gate along one parameter until it captures what the rule asks for.
///
/// This replaces solving a line and anchoring a corner to it, which only ever
/// worked for a gate whose boundary *is* that line. A real gate's boundary can
/// be slanted - a compensation artefact tilts it, and the population then
/// crosses it nowhere near the gate's extreme vertex. Anchoring the leftmost
/// corner of such a shape to a one-dimensional threshold moves it far too far,
/// which is how a gate meant to hold 0.35% came to hold 0.010%.
///
/// So nothing is inferred about where the boundary "is". The gate is moved and
/// asked what it now holds, through the same statistic the screen shows, and
/// the move is searched for. That works for any shape, slanted or not, and
/// needs no notion of an edge at all.
///
/// Monotone by construction: for a gate keeping the bright side, sliding it up
/// the parameter can only admit fewer events. Bisection is therefore exact to
/// the width of the bracket, and the step-like nature of a count is why the
/// result is still checked against the band afterwards rather than assumed.
fn slide_to_capture(
    gate: &Arc<dyn DrawableGate>,
    parameter: &str,
    bound: Bound,
    index: &EventIndexMapped,
    band: (f64, f64),
    bracket: (f64, f64),
) -> Option<(f64, f64)> {
    let (lo, hi) = band;
    let target = (lo + hi) / 2.0;
    let at = |delta: f64| -> Option<f64> {
        let moved = translate_by(gate, parameter, delta).ok()?;
        admitted_by(&moved, index)
    };

    // Sliding up the parameter admits fewer for an `Above` gate and more for a
    // `Below` one; normalise so `more_negative` always means "admits more".
    let sign = match bound {
        Bound::Above => 1.0,
        Bound::Below => -1.0,
    };

    let (mut low, mut high) = bracket;
    let mut best: Option<(f64, f64)> = None;
    let mut consider = |delta: f64, got: f64, best: &mut Option<(f64, f64)>| {
        let better = match best {
            None => true,
            Some((_, prev)) => {
                let d_new = if (lo..=hi).contains(&got) {
                    0.0
                } else {
                    (got - target).abs()
                };
                let d_old = if (lo..=hi).contains(prev) {
                    0.0
                } else {
                    (*prev - target).abs()
                };
                d_new < d_old
            }
        };
        if better {
            *best = Some((delta, got));
        }
    };

    for _ in 0..48 {
        let mid = (low + high) / 2.0;
        let Some(got) = at(mid) else { return best };
        consider(mid, got, &mut best);
        if (lo..=hi).contains(&got) {
            return best;
        }
        // Too many admitted means the gate must move further along the
        // parameter; too few, further back.
        if (got > hi) == (sign > 0.0) {
            low = mid;
        } else {
            high = mid;
        }
    }
    best
}

/// Slide a gate until it holds the band, returning the moved gate, where its
/// leading extent ended up, and what it now holds.
///
/// The operation the autogater performs, exposed so it can be exercised
/// directly on a shape.
pub fn position_by_capture(
    gate: &Arc<dyn DrawableGate>,
    parameter: &str,
    bound: Bound,
    index: &EventIndexMapped,
    band: (f64, f64),
    values: &[f64],
    current: f64,
) -> Option<(Arc<dyn DrawableGate>, f64, f64)> {
    let bracket = bracket_for(values, current);
    let (delta, got) = slide_to_capture(gate, parameter, bound, index, band, bracket)?;
    let moved = translate_by(gate, parameter, delta).ok()?;
    Some((moved, current + delta, got))
}

/// The same gate, moved `delta` along `parameter`.
fn translate_by(
    gate: &Arc<dyn DrawableGate>,
    parameter: &str,
    delta: f64,
) -> Result<Arc<dyn DrawableGate>, ApplyError> {
    let id = gate.get_id();
    let inner = gate
        .get_gate_ref(None)
        .ok_or_else(|| ApplyError::UnsupportedShape(id.clone()))?;
    let mut moved = inner.clone();
    moved.geometry = slide(&inner.geometry, parameter, delta as f32);
    rebuild(moved, gate.is_primary(), &id)
}

// ── solving and placing ──────────────────────────────────────────────────

/// One gate that was positioned, and what it took to do it.
pub struct Positioned {
    pub file: FileId,
    pub gate: Arc<str>,
    /// The population it is drawn on. Without it a report naming "a4b7+" five
    /// times says nothing: the same marker gated on five parents is five
    /// different gates, and only the parent tells them apart.
    pub parent_gate: Option<Arc<str>>,
    pub specimen: Arc<str>,
    /// The file whose population the rule read.
    pub measured_on: FileId,
    pub from: f64,
    pub to: f64,
    pub confidence: f64,
    /// The measure holding the confidence down, for a person deciding what to
    /// review first.
    pub weakest: Option<&'static str>,
    /// What the moved gate actually admits from the reference population -
    /// measured by asking the gate, not by counting past a line.
    pub achieved: f64,
    pub reference_events: usize,
    /// Whether that landed inside the band the rule asked for.
    pub in_band: bool,
}

/// A gate no rule could even be tried against, and why.
pub struct Unmeasured {
    pub gate_id: GateId,
    pub gate: Arc<str>,
    pub parent_gate: Option<Arc<str>>,
    pub reason: String,
}

/// A gate that already satisfied its rule and was left alone.
pub struct Unchanged {
    pub file: FileId,
    pub gate: Arc<str>,
    /// The population it is drawn on. Without it a report naming "a4b7+" five
    /// times says nothing: the same marker gated on five parents is five
    /// different gates, and only the parent tells them apart.
    pub parent_gate: Option<Arc<str>>,
    pub specimen: Arc<str>,
    pub achieved: f64,
}

/// One gate that was not positioned, and why.
pub struct Skipped {
    pub file: FileId,
    pub gate: Arc<str>,
    /// The population it is drawn on. Without it a report naming "a4b7+" five
    /// times says nothing: the same marker gated on five parents is five
    /// different gates, and only the parent tells them apart.
    pub parent_gate: Option<Arc<str>>,
    pub reason: String,
}

/// "a4b7+ of CD4+", or just the gate where it has no parent.
pub fn describe(gate: &str, parent: Option<&str>) -> String {
    match parent {
        Some(parent) => format!("{gate} of {parent}"),
        None => gate.to_string(),
    }
}

#[derive(Default)]
pub struct Report {
    pub positioned: Vec<Positioned>,
    pub unchanged: Vec<Unchanged>,
    pub skipped: Vec<Skipped>,
}

impl Report {
    /// Gates worth opening by hand: a weak placement, or one that could not be
    /// brought inside the band at all.
    pub fn needs_review(&self, floor: f64) -> impl Iterator<Item = &Positioned> {
        self.positioned
            .iter()
            .filter(move |p| p.confidence < floor || !p.in_band)
    }
}

/// What the rule reads, and the gate it reads it through, for one specimen.
struct Reference<'a> {
    id: FileId,
    measurement: &'a Measurement,
}

/// Solve every rule that matches, and give each specimen its own gate.
///
/// Work is done once per specimen and gate, not once per file: a specimen's
/// files share one position, so solving for each of them in turn repeats the
/// same answer and reports it twice.
pub fn position_all(
    state: &mut GateState,
    store: &RuleStore,
    measurements: &[Measurement],
    unmeasured: &[Unmeasured],
    metadata: &MetaDataFileMap,
) -> Report {
    let mut report = Report::default();

    let mut told: FxHashMap<GateId, ()> = FxHashMap::default();
    for miss in unmeasured {
        if told.insert(miss.gate_id.clone(), ()).is_some() {
            continue;
        }
        report.skipped.push(Skipped {
            file: Arc::from(""),
            gate: miss.gate.clone(),
            parent_gate: miss.parent_gate.clone(),
            reason: miss.reason.clone(),
        });
    }

    let mut done: FxHashMap<(Arc<str>, GateId), ()> = FxHashMap::default();

    for measured in measurements {
        let Some(rule) = store.rule_for(&measured.gate, measured.parent_gate.as_deref()) else {
            continue;
        };
        let Some(specimen) = specimen_of(&store.pairing, &measured.file, metadata) else {
            report.skipped.push(Skipped {
                file: measured.file.clone(),
                gate: measured.gate.clone(),
                parent_gate: measured.parent_gate.clone(),
                reason: format!(
                    "no {} for this file, so there is no specimen to position",
                    store.pairing.sample_id_column
                ),
            });
            continue;
        };
        // One answer per specimen: its files share a position.
        if done
            .insert((specimen.group.clone(), measured.gate_id.clone()), ())
            .is_some()
        {
            continue;
        }

        let Some(reference) = resolve_reference(store, measured, measurements, metadata) else {
            report.skipped.push(Skipped {
                file: measured.file.clone(),
                gate: measured.gate.clone(),
                parent_gate: measured.parent_gate.clone(),
                reason: "no reference sample to measure".to_string(),
            });
            continue;
        };

        match position_one(state, rule, measured, &reference, &specimen, metadata) {
            Ok(Outcome::Moved(p)) => report.positioned.push(p),
            Ok(Outcome::Kept(u)) => report.unchanged.push(u),
            Err(reason) => report.skipped.push(Skipped {
                file: measured.file.clone(),
                gate: measured.gate.clone(),
                parent_gate: measured.parent_gate.clone(),
                reason,
            }),
        }
    }

    report
}

fn resolve_reference<'a>(
    store: &RuleStore,
    measured: &Measurement,
    measurements: &'a [Measurement],
    metadata: &MetaDataFileMap,
) -> Option<Reference<'a>> {
    let rule = store.rule_for(&measured.gate, measured.parent_gate.as_deref())?;
    let id = store.reference_file(&measured.file, &rule.measured_on, metadata)?;
    let measurement = measurements
        .iter()
        .find(|m| m.file == id && m.gate_id == measured.gate_id)?;
    Some(Reference { id, measurement })
}

enum Outcome {
    Moved(Positioned),
    Kept(Unchanged),
}

fn position_one(
    state: &mut GateState,
    rule: &GateRule,
    measured: &Measurement,
    reference: &Reference<'_>,
    specimen: &MetaDataKey,
    metadata: &MetaDataFileMap,
) -> Result<Outcome, String> {
    let population = &reference.measurement.index;

    // What the gate on the reference file admits from the reference population,
    // as the gate - not as a line. Its other sides can exclude events the line
    // lets through, and a fraction counted past the line is then not the
    // fraction anyone reads off the plot.
    let current_gate = state
        .gate_for_file(&measured.gate_id, &reference.id, metadata)
        .ok_or_else(|| "the gate no longer resolves".to_string())?;
    let already = admitted_by(&current_gate, population);

    if let (Some((lo, hi)), Some(already)) = (rule.rule.accepted_band(), already)
        && (lo..=hi).contains(&already)
    {
        return Ok(Outcome::Kept(Unchanged {
            file: measured.file.clone(),
            gate: measured.gate.clone(),
            parent_gate: measured.parent_gate.clone(),
            specimen: specimen.group.clone(),
            achieved: already,
        }));
    }

    // A rule with a band names a fraction, not a place, so the gate is slid
    // until it holds that fraction - measured from the gate itself. A rule that
    // names a position is solved and the leading edge anchored to it, which is
    // what such a rule means.
    let (moved, to, achieved) = match rule.rule.accepted_band() {
        Some(band) => {
            let bracket = bracket_for(&reference.measurement.values, measured.current);
            let (delta, got) = slide_to_capture(
                &current_gate,
                &measured.parameter,
                measured.bound,
                population,
                band,
                bracket,
            )
            .ok_or_else(|| "no position along this axis holds the band".to_string())?;
            let moved = translate_by(&current_gate, &measured.parameter, delta)
                .map_err(|e| e.to_string())?;
            (moved, measured.current + delta, got)
        }
        None => {
            let solved = rule
                .solve(&reference.measurement.values, Some(measured.current))
                .map_err(|e| e.to_string())?;
            let moved = translate_edge_to(
                &current_gate,
                &measured.parameter,
                measured.bound,
                solved.threshold.x,
            )
            .map_err(|e| e.to_string())?;
            let got = admitted_by(&moved, population).unwrap_or(solved.threshold.fraction_admitted);
            (moved, solved.threshold.x, got)
        }
    };

    let in_band = match rule.rule.accepted_band() {
        Some((lo, hi)) => (lo..=hi).contains(&achieved),
        None => true,
    };

    // Scored from what the gate actually did, not from the line that used to
    // stand in for it.
    let parent_events = population.event_index.len();
    let mut sorted = reference.measurement.values.clone();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let spread = crate::gate_rules::threshold::interquartile_spread(&sorted);
    // Nudge the gate either side and see how much of its contents survive.
    // Relative to what it holds, not an absolute count: the model reads a swing
    // of 1 as "a nudge changes the contents by as much as the gate holds", and
    // feeding it a raw event difference made every small gate look unstable.
    let nudge = (spread * crate::gate_rules::threshold::STABILITY_WINDOW).max(f64::EPSILON);
    let at = |delta: f64| {
        translate_by(&moved, &measured.parameter, delta)
            .ok()
            .and_then(|g| admitted_by(&g, population))
    };
    let swing = match (at(-nudge), at(nudge), achieved) {
        (Some(back), Some(forward), held) if held > 0.0 => (back - forward).abs() / held,
        _ => 0.0,
    };

    let threshold = crate::gate_rules::threshold::Threshold {
        x: to,
        events_admitted: (achieved * parent_events as f64).round() as usize,
        fraction_admitted: achieved,
        parent_events,
        count_swing: swing,
        parent_spread: spread,
        status: if in_band {
            crate::gate_rules::threshold::Status::InBand
        } else {
            match rule.rule.accepted_band() {
                Some(band) => crate::gate_rules::threshold::Status::OutOfBand { band },
                None => crate::gate_rules::threshold::Status::NoBand,
            }
        },
    };
    let confidence = rule.rule.assess(&threshold, Some(measured.current));

    place_for_specimen(state, &measured.gate_id, specimen, &moved);

    Ok(Outcome::Moved(Positioned {
        file: measured.file.clone(),
        gate: measured.gate.clone(),
        parent_gate: measured.parent_gate.clone(),
        specimen: specimen.group.clone(),
        measured_on: reference.id.clone(),
        from: measured.current,
        to,
        confidence: confidence.score,
        weakest: confidence.weakest().map(|c| c.name),
        achieved,
        reference_events: parent_events,
        in_band,
    }))
}

/// The range of moves worth trying, as deltas.
///
/// Wide enough to carry the gate from one end of the population to the other,
/// *and* to reach it in the first place: a gate can start well outside the data
/// - drawn on a different sample, or simply pushed aside - and a bracket only
/// as wide as the population would then never touch it.
fn bracket_for(values: &[f64], current: f64) -> (f64, f64) {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() || !hi.is_finite() {
        return (-1.0, 1.0);
    }
    // A margin either side so the gate can sit clear of the population at
    // both ends, which is what admitting none or all of it takes.
    let margin = (hi - lo).abs().max(1.0);
    (lo - current - margin, hi - current + margin)
}
