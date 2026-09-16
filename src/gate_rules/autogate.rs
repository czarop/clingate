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
    #[error("only rectangles can be positioned by a rule; {0} is not one")]
    NotARectangle(GateId),
    #[error("{gate} does not bound {parameter}")]
    NoSuchParameter { gate: GateId, parameter: Arc<str> },
    #[error("the edge of {0} that the rule positions is unbounded, so there is nothing to move")]
    UnboundedEdge(GateId),
    #[error("{0}")]
    Rebuild(String),
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
        .ok_or_else(|| ApplyError::NotARectangle(id.clone()))?;
    let GateGeometry::Rectangle { min, max } = &inner.geometry else {
        return Err(ApplyError::NotARectangle(id.clone()));
    };

    let (Some(low), Some(high)) = (min.get_coordinate(parameter), max.get_coordinate(parameter))
    else {
        return Err(ApplyError::NoSuchParameter {
            gate: id.clone(),
            parameter: Arc::from(parameter),
        });
    };

    // The edge the rule positions, and the one that follows it.
    let leading = match bound {
        Bound::Above => low,
        Bound::Below => high,
    };
    if !leading.is_finite() || leading.abs() > UNBOUNDED {
        return Err(ApplyError::UnboundedEdge(id.clone()));
    }
    let delta = to as f32 - leading;

    let shift = |value: f32| -> f32 {
        if !value.is_finite() || value.abs() > UNBOUNDED {
            value
        } else {
            value + delta
        }
    };

    let mut moved = inner.clone();
    if let GateGeometry::Rectangle { min, max } = &mut moved.geometry {
        min.set_coordinate(parameter, shift(low));
        max.set_coordinate(parameter, shift(high));
    }

    let rebuilt = RectangleGate::try_new(moved, gate.is_primary())
        .map_err(|e| ApplyError::Rebuild(e.to_string()))?;
    Ok(Arc::new(rebuilt))
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
