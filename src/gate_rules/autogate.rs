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

    let rebuilt: Arc<dyn DrawableGate> = match &moved.geometry {
        GateGeometry::Polygon { .. } => Arc::new(
            PolygonGate::try_new(moved, gate.is_primary())
                .map_err(|e| ApplyError::Rebuild(e.to_string()))?,
        ),
        _ => Arc::new(
            RectangleGate::try_new(moved, gate.is_primary())
                .map_err(|e| ApplyError::Rebuild(e.to_string()))?,
        ),
    };
    Ok(rebuilt)
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
use crate::gate_editor::gates::gate_store::{GroupId, NodeId};
use crate::gate_rules::rule_store::RuleStore;
use crate::omiq::metadata::MetaDataParameter;
use polars::prelude::*;
use rustc_hash::FxHashMap;

/// One gate on one file, as it stands before any rule is applied.
///
/// The parent population travels with it because solving needs the values, and
/// reading them costs a filtered pass over the frame that is not worth doing
/// twice - the rule measures the FMO's population and applies the answer to the
/// full stain's gate, so both files' measurements are needed at once.
#[derive(Clone)]
pub struct Measurement {
    pub file: FileId,
    pub gate_id: GateId,
    pub gate: Arc<str>,
    pub parent_gate: Option<Arc<str>>,
    /// The parameter the bounding edge lies on - read off the gate, never
    /// inferred from which axis it happens to be drawn on.
    pub parameter: Arc<str>,
    pub bound: Bound,
    /// Where the edge sits now.
    pub current: f64,
    /// The parent population's values on `parameter`.
    pub values: Vec<f64>,
}

/// Every gate on one file, with the population each one cuts.
///
/// A gate contributes a measurement only when exactly one of its four edges
/// lies inside the data. None means it thresholds nothing; more than one is a
/// window or a genuinely two-dimensional gate, and neither is a single line for
/// a rule to solve.
pub fn measure_file(
    state: &GateState,
    file: &FileId,
    df: &DataFrame,
    metadata: &MetaDataFileMap,
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
        let Some(inner) = gate.get_gate_ref(None) else {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                reason: "this gate has no geometry of its own".to_string(),
            });
            continue;
        };
        let Some(parent) = state.parent_node(node) else {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                reason: "this gate sits at the root, so it has no parent population".to_string(),
            });
            continue;
        };
        let chain = state.gate_chain_for_node(&parent);
        let (x_param, y_param) = gate.get_params();

        // Every bounding extent, each carrying the parameter it bounds. Read off
        // the shape rather than assumed from the axis: the same marker appears
        // on x in one plot and y in another within a single workflow. Polygons
        // are as ordinary here as rectangles - in one real export, 41 of 211.
        let (x_low, x_high) = extent_on(&inner.geometry, &x_param).unzip();
        let (y_low, y_high) = extent_on(&inner.geometry, &y_param).unzip();
        if x_low.is_none() && y_low.is_none() {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: name,
                reason: "this gate is drawn as a shape a rule cannot slide along one axis"
                    .to_string(),
            });
            continue;
        }
        let edges: [(Bound, Arc<str>, Option<f32>); 4] = [
            (Bound::Above, x_param.clone(), x_low),
            (Bound::Below, x_param.clone(), x_high),
            (Bound::Above, y_param.clone(), y_low),
            (Bound::Below, y_param.clone(), y_high),
        ];

        let mut candidates: Vec<(Bound, Arc<str>, f64, Vec<f64>)> = Vec::new();
        let mut cache: FxHashMap<Arc<str>, Vec<f64>> = FxHashMap::default();
        for (bound, param, value) in edges {
            let Some(value) = value else { continue };
            let value = value as f64;
            if !value.is_finite() || value.abs() > UNBOUNDED as f64 {
                continue;
            }
            if df.column(param.as_ref()).is_err() {
                continue;
            }
            let values = match cache.get(&param) {
                Some(v) => v.clone(),
                None => {
                    let v = parent_population(df, &chain, &resolver, &param)?;
                    cache.insert(param.clone(), v.clone());
                    v
                }
            };
            if values.len() < 2 {
                continue;
            }
            let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let inside = match bound {
                Bound::Above => value > lo,
                Bound::Below => value < hi,
            };
            if inside {
                candidates.push((bound, param, value, values));
            }
        }
        if candidates.len() != 1 {
            unmeasured.push(Unmeasured {
                gate_id: gate_id.clone(),
                gate: Arc::from(gate.get_name()),
                reason: if candidates.is_empty() {
                    "no edge of this gate lies inside the data, so it thresholds nothing"
                        .to_string()
                } else {
                    format!(
                        "{} edges lie inside the data - a rule positions one line, not a window",
                        candidates.len()
                    )
                },
            });
            continue;
        }
        let (bound, parameter, current, values) = candidates.pop().expect("one candidate");

        out.push(Measurement {
            file: file.clone(),
            gate_id: gate_id.clone(),
            gate: Arc::from(gate.get_name()),
            parent_gate: parent_name(state, &parent),
            parameter,
            bound,
            current,
            values,
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

fn parent_population(
    df: &DataFrame,
    chain: &[GateId],
    resolver: &crate::gate_editor::gates::gate_store::GateOverrideResolver,
    channel: &str,
) -> anyhow::Result<Vec<f64>> {
    let frame = if chain.is_empty() {
        df.clone()
    } else {
        let mask = filter_events_by_hierarchy_to_mask(df, chain, resolver)?;
        df.filter(&mask)?
    };
    Ok(frame
        .column(channel)?
        .f32()?
        .into_no_null_iter()
        .map(|v| v as f64)
        .collect())
}

// ── solving and placing ──────────────────────────────────────────────────

/// One gate that was positioned, and what it took to do it.
pub struct Positioned {
    pub file: FileId,
    pub gate: Arc<str>,
    pub specimen: Arc<str>,
    /// The file whose population the rule read.
    pub measured_on: FileId,
    pub from: f64,
    pub to: f64,
    pub confidence: f64,
    /// The measure holding the confidence down, for a person deciding what to
    /// review first.
    pub weakest: Option<&'static str>,
    /// What the gate actually captures on the sample it was measured against,
    /// and how many events that was out of.
    pub achieved: f64,
    pub reference_events: usize,
    /// Whether that landed inside the band the rule asked for.
    ///
    /// It can miss even when the solver did its best. A gate admits a whole
    /// number of events, so a narrow band over a small population may contain
    /// no achievable fraction at all, and ties in the data can block the ones
    /// it does contain. The threshold is then the nearest achievable - worth
    /// having, but not worth trusting silently.
    pub in_band: bool,
}

/// A gate no rule could even be tried against, and why.
///
/// Worth carrying rather than dropping: a gate a rule names but which never
/// produces a measurement would otherwise vanish without a word, and "nothing
/// happened" is the least useful thing an autogater can say.
pub struct Unmeasured {
    pub gate_id: GateId,
    pub gate: Arc<str>,
    pub reason: String,
}

/// One gate that was not positioned, and why.
pub struct Skipped {
    pub file: FileId,
    pub gate: Arc<str>,
    pub reason: String,
}

/// A gate that already satisfied its rule and was left alone.
pub struct Unchanged {
    pub file: FileId,
    pub gate: Arc<str>,
    pub specimen: Arc<str>,
    /// What it already captures on the file the rule measures.
    pub achieved: f64,
}

#[derive(Default)]
pub struct Report {
    pub positioned: Vec<Positioned>,
    pub unchanged: Vec<Unchanged>,
    pub skipped: Vec<Skipped>,
}

impl Report {
    /// Gates whose confidence falls below `floor`, which is the list worth
    /// opening by hand. On the hand-gated export everything at 0.30 or above
    /// landed within a typical manual adjustment.
    pub fn needs_review(&self, floor: f64) -> impl Iterator<Item = &Positioned> {
        self.positioned
            .iter()
            .filter(move |p| p.confidence < floor || !p.in_band)
    }
}

/// Solve every rule that matches, and give each specimen its own gate.
///
/// `measurements` must span every file the rules might measure, not just the
/// ones being gated: a rule that reads the FMO needs the FMO's population, and
/// that lives on a different file from the gate it positions.
pub fn position_all(
    state: &mut GateState,
    store: &RuleStore,
    measurements: &[Measurement],
    unmeasured: &[Unmeasured],
    metadata: &MetaDataFileMap,
) -> Report {
    let mut report = Report::default();

    // A gate a rule names but which never yielded a measurement is the one case
    // that used to pass in silence - nothing positioned, nothing skipped, and
    // no clue why. Reported once per gate rather than once per file, since the
    // reason is a property of how the gate is drawn.
    let mut told: FxHashMap<GateId, ()> = FxHashMap::default();
    for miss in unmeasured {
        if store.rule_for(&miss.gate, None).is_none()
            && !store.entries().iter().any(|e| e.target.gate == miss.gate)
        {
            continue;
        }
        if told.insert(miss.gate_id.clone(), ()).is_some() {
            continue;
        }
        report.skipped.push(Skipped {
            file: Arc::from(""),
            gate: miss.gate.clone(),
            reason: miss.reason.clone(),
        });
    }

    for measured in measurements {
        let Some(rule) = store.rule_for(&measured.gate, measured.parent_gate.as_deref()) else {
            continue;
        };
        // A rule names the parameter it positions. A gate bounding a different
        // one is not what this rule is about, however it is named.
        if rule.parameter != measured.parameter {
            continue;
        }

        let mut skip = |reason: String| {
            report.skipped.push(Skipped {
                file: measured.file.clone(),
                gate: measured.gate.clone(),
                reason,
            });
        };

        let Some(specimen) = specimen_of(&store.pairing, &measured.file, metadata) else {
            skip(format!(
                "no {} for this file, so there is no specimen to position",
                store.pairing.sample_id_column
            ));
            continue;
        };
        let Some(reference_id) = store.reference_file(&measured.file, &rule.measured_on, metadata)
        else {
            skip("no reference sample to measure".to_string());
            continue;
        };
        // The rule reads its population from the reference file at this same
        // gate - the FMO's events inside the same parent.
        let Some(reference) = measurements
            .iter()
            .find(|m| m.file == reference_id && m.gate_id == measured.gate_id)
        else {
            skip(format!("{reference_id} was not measured"));
            continue;
        };

        // A gate already capturing what the rule asks for is already right, and
        // the best thing to do with it is nothing. Solving anyway costs a pass
        // over the population for no gain, and can make things actively worse:
        // a band narrow enough to allow only one or two whole events can be
        // missed by the solver even where the current position hits it, so a
        // gate sitting at 0.38% gets "corrected" to 0.19%.
        if let Some((lo, hi)) = rule.rule.accepted_band() {
            let already = rule.admitted(&reference.values, measured.current) as f64
                / reference.values.len().max(1) as f64;
            if (lo..=hi).contains(&already) {
                report.unchanged.push(Unchanged {
                    file: measured.file.clone(),
                    gate: measured.gate.clone(),
                    specimen: specimen.group.clone(),
                    achieved: already,
                });
                continue;
            }
        }

        let solved = match rule.solve(&reference.values, Some(measured.current)) {
            Ok(s) => s,
            Err(e) => {
                skip(e.to_string());
                continue;
            }
        };

        let Some(current) = state.gate_for_file(&measured.gate_id, &measured.file, metadata) else {
            skip("the gate no longer resolves".to_string());
            continue;
        };
        let moved = match translate_edge_to(
            &current,
            &measured.parameter,
            measured.bound,
            solved.threshold.x,
        ) {
            Ok(g) => g,
            Err(e) => {
                skip(e.to_string());
                continue;
            }
        };
        place_for_specimen(state, &measured.gate_id, &specimen, &moved);

        report.positioned.push(Positioned {
            file: measured.file.clone(),
            gate: measured.gate.clone(),
            specimen: specimen.group.clone(),
            measured_on: reference_id,
            from: measured.current,
            to: solved.threshold.x,
            confidence: solved.confidence.score,
            weakest: solved.confidence.weakest().map(|c| c.name),
            achieved: solved.threshold.fraction_admitted,
            reference_events: solved.threshold.parent_events,
            in_band: matches!(
                solved.threshold.status,
                crate::gate_rules::threshold::Status::InBand
                    | crate::gate_rules::threshold::Status::NoBand
            ),
        });
    }

    report
}
