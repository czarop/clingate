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
use crate::gate_editor::gates::gate_single::line_gate::LineGate;
use crate::gate_editor::gates::gate_single::polygon_gate::PolygonGate;
use crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate;
use crate::gate_editor::gates::gate_store::{FileId, GateId, GateSource, GateSubStore};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_rules::rule::{NegativeRead, ValleyRead};
use crate::gate_rules::rule_store::{Bound, MeasuredOn, SamplePairing};
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

    rebuild(gate, moved)
}

/// Rebuild a moved gate as the kind it was.
///
/// Geometry alone cannot say: a line gate holds a rectangle - it is a threshold
/// drawn as one edge with a height - so rebuilding from the geometry turned
/// every line into a box, which draws differently and exports as a different
/// Omiq type. The gate it came from is what knows.
fn rebuild(
    was: &Arc<dyn DrawableGate>,
    moved: flow_gates::Gate,
) -> Result<Arc<dyn DrawableGate>, ApplyError> {
    let is_primary = was.is_primary();
    if let Some(line) = was.as_any().downcast_ref::<LineGate>() {
        return Ok(Arc::new(
            LineGate::try_new(moved, line.height, is_primary)
                .map_err(|e| ApplyError::Rebuild(e.to_string()))?,
        ));
    }
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

/// Where the gate's leading boundary sits on `parameter`, at one position on
/// the other axis - or `None` where the gate does not reach that far.
///
/// A gate's boundary is not a single number. A slanted polygon crosses the
/// marker axis at a different place for every height, and a gate boxed in its
/// other axis makes no statement at all outside that box. Reading one number
/// off the shape - its extreme vertex - put the cut well to the left of the
/// real boundary and read the negative from a sliver.
pub fn boundary_at(
    geometry: &GateGeometry,
    parameter: &str,
    other_parameter: &str,
    bound: Bound,
    other: f32,
) -> Option<f32> {
    match geometry {
        GateGeometry::Rectangle { min, max } => {
            let (lo, hi) = (
                min.get_coordinate(other_parameter)?,
                max.get_coordinate(other_parameter)?,
            );
            // Outside the box's own span the gate says nothing.
            if other < lo.min(hi) || other > lo.max(hi) {
                return None;
            }
            match bound {
                Bound::Above => min.get_coordinate(parameter),
                Bound::Below => max.get_coordinate(parameter),
            }
        }
        GateGeometry::Polygon { nodes, .. } => {
            // Every edge crossing this height gives one crossing point; the
            // leading boundary is the first of them from the side the gate
            // keeps events from.
            let pts: Vec<(f32, f32)> = nodes
                .iter()
                .filter_map(|n| {
                    Some((
                        n.get_coordinate(parameter)?,
                        n.get_coordinate(other_parameter)?,
                    ))
                })
                .collect();
            if pts.len() < 3 {
                return None;
            }
            let mut crossings: Vec<f32> = Vec::new();
            for i in 0..pts.len() {
                let (p1, q1) = pts[i];
                let (p2, q2) = pts[(i + 1) % pts.len()];
                if (q1 <= other && q2 > other) || (q2 <= other && q1 > other) {
                    let t = (other - q1) / (q2 - q1);
                    crossings.push(p1 + t * (p2 - p1));
                }
            }
            if crossings.is_empty() {
                return None;
            }
            Some(match bound {
                Bound::Above => crossings.iter().copied().fold(f32::INFINITY, f32::min),
                Bound::Below => crossings.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            })
        }
        _ => None,
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

/// How strongly a file is preferred as *the* file of its specimen to gate.
///
/// [`SamplePairing::display_order`] already names the types in order - the FMO
/// on the left where the line is set, the full stain on the right where the
/// positives are read off - so the later a file's type appears in it, the more
/// it is the one being gated. A file whose type is unknown, or named by no
/// order at all, ranks below every file that is.
pub fn gated_rank(pairing: &SamplePairing, file: &FileId, metadata: &MetaDataFileMap) -> usize {
    let Some(columns) = metadata.get(file) else {
        return 0;
    };
    let Some(kind) = pairing.sample_type_of(columns) else {
        return 0;
    };
    pairing
        .display_order
        .iter()
        .position(|named| *named == kind)
        .map(|at| at + 1)
        .unwrap_or(0)
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
    /// The same events, paired with how far each sits from the gate's leading
    /// boundary *at that event's own height* - negative inside the gate's
    /// shadow, positive inside the gate.
    ///
    /// Carrying the offset rather than a cut value is what lets the gate move:
    /// the shape translates rigidly, so a gate slid by `d` has the events with
    /// offset below `d` in its shadow. Events the gate does not reach at all -
    /// above or below a gate boxed in its other axis - are not here, because
    /// the gate makes no statement about them.
    pub shadow: Vec<(f64, f64)>,
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
    // A parent's bare name is not unique - CD4+CD8- can be drawn under two
    // different populations - so a rule naming one by name would name both.
    let names = crate::gate_editor::gates::gate_paths::unique_names(state);

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
        let parent_gate = names.get(&parent).cloned();

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
        let (values, points, index) =
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

        // Each event's distance from the gate's boundary at its own height.
        let other_parameter = if *rule.parameter == *params.0 {
            params.1.clone()
        } else {
            params.0.clone()
        };
        let on_x = *rule.parameter == *params.0;
        let shadow: Vec<(f64, f64)> = points
            .iter()
            .filter_map(|(x, y)| {
                let (value, other) = if on_x { (*x, *y) } else { (*y, *x) };
                let edge = boundary_at(
                    &inner.geometry,
                    &rule.parameter,
                    &other_parameter,
                    rule.bound,
                    other,
                )?;
                Some((value as f64, (value - edge) as f64))
            })
            .collect();

        out.push(Measurement {
            file: file.clone(),
            gate_id: gate_id.clone(),
            gate: name,
            parent_gate,
            parameter: rule.parameter.clone(),
            bound: rule.bound,
            current,
            values,
            shadow,
            index,
            params,
        });
    }
    Ok((out, unmeasured))
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
) -> anyhow::Result<(Vec<f64>, Vec<(f32, f32)>, EventIndexMapped)> {
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

    let xs = frame.column(params.0.as_ref())?.f32()?;
    let ys = frame.column(params.1.as_ref())?.f32()?;
    let points: Vec<(f32, f32)> = xs.into_no_null_iter().zip(ys.into_no_null_iter()).collect();

    let event_index =
        get_event_mask_from_scaled_df(frame.clone(), params.0.clone(), params.1.clone())?;
    let index_map: Vec<usize> = (0..frame.height()).collect();

    Ok((
        values,
        points,
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

/// What a bare threshold at `at` would admit, the gate's other sides ignored.
///
/// Deliberately naive - it is the control for [`admitted_by`], not a rival to
/// it. A gate reading far below this is one whose other axis is discarding
/// events; a gate matching it is one where the shape makes no difference.
pub fn beyond_the_line(values: &[f64], bound: Bound, at: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let past = values
        .iter()
        .filter(|v| match bound {
            Bound::Above => **v > at,
            Bound::Below => **v < at,
        })
        .count();
    past as f64 / values.len() as f64
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
    rebuild(gate, moved)
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
    /// What a bare threshold on the rule's parameter would admit from the same
    /// population - the gate's other sides ignored.
    ///
    /// Reported beside `achieved` because the two disagreeing is the one thing
    /// that separates "the data is not where the gate thinks" from "the gate's
    /// other axis is throwing the events away". Nothing else in the report can
    /// tell those apart, and they need completely different fixes.
    pub above_the_line: f64,
    /// The file whose population `achieved` was counted on. Not always the file
    /// the rule read: see `judged_on` in `position_one`.
    pub captured_on: FileId,
    pub reference_events: usize,
    /// Whether that landed inside the band the rule asked for.
    pub in_band: bool,
    /// For a rule that places against the negative: what it read on the
    /// reference, and what it read on this sample. The two side by side are
    /// the only way to tell a gate that moved because the negative moved from
    /// one that moved because the negative was measured differently.
    pub negative: Option<(NegativeRead, NegativeRead)>,
    /// For a rule that places in the valley: what it read on the reference, and
    /// on this sample. The two depths side by side are what says whether the
    /// structure the rule depends on is still there.
    pub valley: Option<(ValleyRead, ValleyRead)>,
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
    /// As [`Positioned::above_the_line`]: the same count with the gate's other
    /// sides ignored.
    pub above_the_line: f64,
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
    /// Specimens left alone because the rule calibrates from them. Reported
    /// separately from `unchanged`: those met the rule, these were never
    /// candidates.
    pub reference: Vec<Unchanged>,
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
    let (report, placements) = solve_all(state, store, measurements, unmeasured, metadata);
    apply_placements(state, &placements);
    report
}

/// One gate a solve decided on, ready to be written into a store.
///
/// Carried out of the solve rather than written during it, so the whole run can
/// happen on a worker thread against a snapshot and only the answers come back.
/// Handing back a whole cloned state instead would silently discard anything
/// the person moved while it ran.
pub struct Placement {
    pub gate_id: GateId,
    pub specimen: MetaDataKey,
    pub gate: Arc<dyn DrawableGate>,
}

/// Write a solve's answers into the store.
pub fn apply_placements(state: &mut GateState, placements: &[Placement]) {
    for placed in placements {
        place_for_specimen(state, &placed.gate_id, &placed.specimen, &placed.gate);
    }
}

/// Solve every rule that matches, without touching the store.
///
/// Taking `&GateState` is the point, and it is what makes running this on a
/// worker thread legitimate: a solve that cannot write cannot depend on its own
/// writes, so a snapshot taken before the run gives the same answers as the
/// live store would have. The borrow checker holds that, not a test - the test
/// below only guards the two entry points agreeing end to end.
///
/// It was true before this signature existed, for a reason worth keeping in
/// mind if the loop is ever reordered: overrides are keyed per specimen and
/// `done` stops a specimen being visited twice, so no gate's placement was ever
/// read by another's solve.
pub fn solve_all(
    state: &GateState,
    store: &RuleStore,
    measurements: &[Measurement],
    unmeasured: &[Unmeasured],
    metadata: &MetaDataFileMap,
) -> (Report, Vec<Placement>) {
    solve_all_reporting(state, store, measurements, unmeasured, metadata, |_, _| {})
}

/// [`solve_all`], calling `progress(done, total)` as it goes.
///
/// Solving one specimen bisects against an R-tree, several times over, so a run
/// spends long enough here to leave a progress bar sitting on one message with
/// nothing to say how far through it is.
pub fn solve_all_reporting(
    state: &GateState,
    store: &RuleStore,
    measurements: &[Measurement],
    unmeasured: &[Unmeasured],
    metadata: &MetaDataFileMap,
    progress: impl Fn(usize, usize),
) -> (Report, Vec<Placement>) {
    let mut placements: Vec<Placement> = Vec::new();
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

    // Which file of each specimen the answer is read from.
    //
    // A specimen holds several files - the FMO and the full stain at least -
    // and they share one gate position. They do not share a population: the FMO
    // is the control, and the full stain is what a person reads the positives
    // off. Taking whichever file sorted first meant a specimen whose FMO came
    // first had its negative read, its gate placed *and* its percentage
    // reported from the control, which is how a gate holding 6.24% came to be
    // reported as 0.016%.
    let mut chosen: FxHashMap<(Arc<str>, GateId), usize> = FxHashMap::default();
    for (i, m) in measurements.iter().enumerate() {
        let Some(specimen) = specimen_of(&store.pairing, &m.file, metadata) else {
            continue;
        };
        let key = (specimen.group.clone(), m.gate_id.clone());
        match chosen.entry(key) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(i);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let held = &measurements[*slot.get()];
                if gated_rank(&store.pairing, &m.file, metadata)
                    > gated_rank(&store.pairing, &held.file, metadata)
                {
                    slot.insert(i);
                }
            }
        }
    }

    for (seen, measured) in measurements.iter().enumerate() {
        progress(seen + 1, measurements.len());
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
        // One answer per specimen, read from the file that is actually gated.
        if chosen.get(&(specimen.group.clone(), measured.gate_id.clone())) != Some(&seen) {
            continue;
        }

        // The reference is ground truth: its gate is where a person put it, and
        // everything else is calibrated from that. Positioning it would
        // overwrite the hand placement, and worse, the next run would calibrate
        // from the moved gate and the drift would compound every time. Only a
        // rule naming one sample is affected - a partner rule's reference is
        // inside the specimen it is positioning, which is the point of it.
        if let MeasuredOn::File(named) = &rule.measured_on
            && specimen_of(&store.pairing, named, metadata).as_ref() == Some(&specimen)
        {
            let holds = state
                .gate_for_file(&measured.gate_id, &measured.file, metadata)
                .and_then(|gate| admitted_by(&gate, &measured.index))
                .unwrap_or(f64::NAN);
            report.reference.push(Unchanged {
                file: measured.file.clone(),
                gate: measured.gate.clone(),
                parent_gate: measured.parent_gate.clone(),
                specimen: specimen.group.clone(),
                achieved: holds,
                above_the_line: beyond_the_line(&measured.values, measured.bound, measured.current),
            });
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
            Ok(Outcome::Moved(p, placed)) => {
                report.positioned.push(p);
                placements.push(placed);
            }
            Ok(Outcome::Kept(u)) => report.unchanged.push(u),
            Err(reason) => report.skipped.push(Skipped {
                file: measured.file.clone(),
                gate: measured.gate.clone(),
                parent_gate: measured.parent_gate.clone(),
                reason,
            }),
        }
    }

    (report, placements)
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
    Moved(Positioned, Placement),
    Kept(Unchanged),
}

fn position_one(
    state: &GateState,
    rule: &GateRule,
    measured: &Measurement,
    reference: &Reference<'_>,
    specimen: &MetaDataKey,
    metadata: &MetaDataFileMap,
) -> Result<Outcome, String> {
    // Which population the placed gate is judged against, which is not the same
    // question for every rule.
    //
    // A band rule names a fraction *of the file it reads* - "0.2 to 0.5% of the
    // FMO" - so the figure that says whether it worked has to be counted on
    // that file. An above-the-negative rule names no fraction at all: it places
    // the gate from this sample's own negative, so what it captures on the
    // reference describes a sample nobody is looking at. Reported that way it
    // read 5.1% beside a plot showing 16.1%, which is not a figure anyone can
    // check a gate against.
    let judged_on = match &rule.rule {
        crate::gate_rules::rule::Rule::AboveTheNegative(_) => measured,
        _ => reference.measurement,
    };
    let population = &judged_on.index;

    // What the gate on the reference file admits from the reference population,
    // as the gate - not as a line. Its other sides can exclude events the line
    // lets through, and a fraction counted past the line is then not the
    // fraction anyone reads off the plot.
    let current_gate = state
        .gate_for_file(&measured.gate_id, &reference.id, metadata)
        .ok_or_else(|| "the gate no longer resolves".to_string())?;
    let already = admitted_by(&current_gate, &reference.measurement.index);

    if let (Some((lo, hi)), Some(already)) = (rule.rule.accepted_band(), already)
        && (lo..=hi).contains(&already)
    {
        return Ok(Outcome::Kept(Unchanged {
            file: measured.file.clone(),
            gate: measured.gate.clone(),
            parent_gate: measured.parent_gate.clone(),
            specimen: specimen.group.clone(),
            achieved: already,
            above_the_line: beyond_the_line(
                &reference.measurement.values,
                measured.bound,
                measured.current,
            ),
        }));
    }

    // A rule with a band names a fraction, not a place, so the gate is slid
    // until it holds that fraction - measured from the gate itself. A rule that
    // names a position is solved and the leading edge anchored to it, which is
    // what such a rule means.
    // Filled in only by the above-the-negative rule, which is the one whose
    // answer is otherwise impossible to check by eye: the reading it made on
    // the reference, and the reading it made on this sample.
    let mut reading: Option<(NegativeRead, NegativeRead)> = None;
    let mut valley: Option<(ValleyRead, ValleyRead)> = None;

    let (moved, to, achieved) = match &rule.rule {
        // Calibrated against the reference, then applied to this sample's own
        // negative. The reference supplies the distance; the sample supplies
        // the place to measure it from.
        crate::gate_rules::rule::Rule::AboveTheNegative(above) => {
            let from_reference = above
                .calibrate(
                    &reference.measurement.values,
                    &reference.measurement.shadow,
                    reference.measurement.current,
                )
                .ok_or_else(|| {
                    format!(
                        "{} has no negative peak clear enough to calibrate against",
                        reference.id
                    )
                })?;
            let here = above
                // The gate's current position on this sample - inherited from
                // the reference, so a good place for the refining finder to
                // start from.
                .place(
                    &measured.values,
                    &measured.shadow,
                    from_reference.widths,
                    measured.current,
                )
                .ok_or_else(|| "this sample has no negative peak to place against".to_string())?;
            let moved =
                translate_edge_to(&current_gate, &measured.parameter, measured.bound, here.at)
                    .map_err(|e| e.to_string())?;
            let got = admitted_by(&moved, population).unwrap_or(0.0);
            reading = Some((from_reference, here));
            (moved, here.at, got)
        }
        // The boundary read directly rather than paced out from the negative's
        // centre. Nothing is multiplied, so nothing is amplified.
        crate::gate_rules::rule::Rule::InTheValley(dip) => {
            let from_reference = dip
                .calibrate(&reference.measurement.values, reference.measurement.current)
                .map_err(|why| format!("the reference {}: {why}", reference.id))?;
            let here = dip
                .place(&measured.values, from_reference.offset)
                .map_err(|why| why.to_string())?;
            // A dip a fifth as deep as the reference's is still a dip, and its
            // lowest point is still the boundary. One that has gone entirely
            // means the two populations have merged and there is no boundary to
            // find - place nothing rather than something plausible-looking.
            if here.depth < from_reference.depth * dip.min_depth_fraction {
                return Err(format!(
                    "the valley here is {:.0}% as deep as the reference's ({:.3} against {:.3}) - the populations have merged",
                    100.0 * here.depth / from_reference.depth.max(f64::EPSILON),
                    here.depth,
                    from_reference.depth
                ));
            }
            let moved =
                translate_edge_to(&current_gate, &measured.parameter, measured.bound, here.at)
                    .map_err(|e| e.to_string())?;
            let got = admitted_by(&moved, population).unwrap_or(0.0);
            valley = Some((from_reference, here));
            (moved, here.at, got)
        }
        _ => match rule.rule.accepted_band() {
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
                let got =
                    admitted_by(&moved, population).unwrap_or(solved.threshold.fraction_admitted);
                (moved, solved.threshold.x, got)
            }
        },
    };

    let in_band = match rule.rule.accepted_band() {
        Some((lo, hi)) => (lo..=hi).contains(&achieved),
        None => true,
    };

    // Scored from what the gate actually did, not from the line that used to
    // stand in for it.
    let parent_events = population.event_index.len();
    let mut sorted = judged_on.values.clone();
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
    // Above-the-negative is *meant* to move the gate off the reference's
    // position - that is the whole rule - so scoring it on how far it travelled
    // marks every correct placement as suspect. It put 15 of 32 gates at zero
    // confidence on a run where all of them were right.
    let judge_displacement = !matches!(
        &rule.rule,
        crate::gate_rules::rule::Rule::AboveTheNegative(_)
            | crate::gate_rules::rule::Rule::InTheValley(_)
    );
    let confidence = rule
        .rule
        .assess(&threshold, judge_displacement.then_some(measured.current));

    Ok(Outcome::Moved(
        Positioned {
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
            captured_on: judged_on.file.clone(),
            above_the_line: beyond_the_line(&judged_on.values, measured.bound, to),
            reference_events: parent_events,
            in_band,
            negative: reading,
            valley,
        },
        Placement {
            gate_id: measured.gate_id.clone(),
            specimen: specimen.clone(),
            gate: moved,
        },
    ))
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
