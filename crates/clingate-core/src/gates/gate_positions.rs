//! Which position a sample shows for a gate - the gate as drawn, its group's
//! under a metadata column, or its own - and moving it between them: what
//! the editor's Position menu does.

use std::sync::Arc;

use anyhow::anyhow;

use crate::gates::gate_store::{FileId, GateId, GateSource, GateState, GateSubStore};
use crate::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::{MetaDataFileMap, MetaDataKey, MetaDataParameter};

/// Where the position a sample shows for a gate comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Tier {
    /// The gate as drawn.
    Drawn,
    /// Its group's, under one metadata column.
    Group(MetaDataKey),
    /// The sample's own.
    Sample,
}

/// What becomes of the samples a column's positions are removed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// They show the gate as drawn.
    ToDrawn,
    /// Each keeps the position it shows, as its own.
    ToEachSample,
}

/// Where `file`'s position for `gate_id` comes from.
pub fn tier(
    state: &GateState,
    gate_id: &GateId,
    file: &FileId,
    metadata: &MetaDataFileMap,
) -> Option<Tier> {
    Some(
        match state.gate_and_source_for_file(gate_id, file, metadata)?.0 {
            GateSource::Global => Tier::Drawn,
            GateSource::Group((_, key)) => Tier::Group(key),
            GateSource::Sample(_) => Tier::Sample,
        },
    )
}

/// Every id a gate's positions are kept under: a quadrant's own and each of
/// its corners', whichever of them `gate_id` names. See
/// [`GateSubStore::ids_for`].
pub fn position_ids(gate: &Arc<dyn DrawableGate>, gate_id: &GateId) -> Vec<GateId> {
    let mut ids = GateSubStore::ids_for(gate, gate_id);
    if gate.is_composite() {
        ids.push(gate.get_id());
    }
    ids.sort();
    ids.dedup();
    ids
}

/// A copy of the position `file` shows. A copy, not the same gate: a
/// position that is the drawn gate itself reads as no position of its own,
/// and the export would leave it out.
fn copy_shown(
    state: &GateState,
    gate_id: &GateId,
    file: &FileId,
    metadata: &MetaDataFileMap,
) -> anyhow::Result<Arc<dyn DrawableGate>> {
    let gate = state
        .gate_for_file(gate_id, file, metadata)
        .ok_or_else(|| anyhow!("{gate_id} is not drawn on {file}"))?;
    Ok(Arc::from(gate.clone_box()))
}

/// `file`'s position kept as its own.
pub fn keep_for_sample(
    state: &mut GateState,
    gate_id: &GateId,
    file: &FileId,
    metadata: &MetaDataFileMap,
) -> anyhow::Result<()> {
    copy_to_samples(state, gate_id, file, std::slice::from_ref(file), metadata)
}

/// `file`'s position kept for every sample sharing its value of `column`.
pub fn keep_for_group(
    state: &mut GateState,
    gate_id: &GateId,
    file: &FileId,
    column: &MetaDataParameter,
    metadata: &MetaDataFileMap,
) -> anyhow::Result<()> {
    let group = metadata
        .get(file)
        .and_then(|columns| columns.get(column))
        .ok_or_else(|| anyhow!("{file} has no {column}"))?;
    let gate = copy_shown(state, gate_id, file, metadata)?;
    let key = MetaDataKey {
        parameter: column.clone(),
        group: group.clone(),
    };
    state.place_gate(
        &position_ids(&gate, gate_id),
        &gate,
        &GateSource::Group((gate_id.clone(), key)),
    );
    Ok(())
}

/// `from`'s position given to each of `to` as its own.
pub fn copy_to_samples(
    state: &mut GateState,
    gate_id: &GateId,
    from: &FileId,
    to: &[FileId],
    metadata: &MetaDataFileMap,
) -> anyhow::Result<()> {
    let gate = copy_shown(state, gate_id, from, metadata)?;
    let ids = position_ids(&gate, gate_id);
    for file in to {
        state.place_gate(
            &ids,
            &gate,
            &GateSource::Sample((gate_id.clone(), file.clone())),
        );
    }
    Ok(())
}

/// `file`'s own position removed: it shows its group's, or the gate as drawn.
pub fn release_sample(state: &mut GateState, gate_id: &GateId, file: &FileId) {
    let Some(gate) = state.registered_gate(gate_id) else {
        return;
    };
    let ids = position_ids(&gate, gate_id);
    state.retain_sample_positions(|(id, own)| !(ids.contains(id) && own == file));
}

/// Every position the gate holds under `column` removed. The samples that
/// showed one show the gate as drawn - any older position of their own,
/// hidden under it, goes too - or each keeps it as its own.
pub fn release_column(
    state: &mut GateState,
    gate_id: &GateId,
    column: &MetaDataParameter,
    how: Release,
    metadata: &MetaDataFileMap,
) -> anyhow::Result<()> {
    let gate = state
        .registered_gate(gate_id)
        .ok_or_else(|| anyhow!("{gate_id} is not registered"))?;
    let ids = position_ids(&gate, gate_id);
    let mut affected: Vec<FileId> = metadata
        .keys()
        .filter(|file| {
            matches!(
                tier(state, gate_id, file, metadata),
                Some(Tier::Group(key)) if key.parameter == *column
            )
        })
        .cloned()
        .collect();
    affected.sort();
    match how {
        Release::ToEachSample => {
            for file in &affected {
                keep_for_sample(state, gate_id, file, metadata)?;
            }
        }
        Release::ToDrawn => {
            state.retain_sample_positions(|(id, file)| {
                !(ids.contains(id) && affected.contains(file))
            });
        }
    }
    state.retain_group_positions(|(id, key)| !(ids.contains(id) && key.parameter == *column));
    Ok(())
}

/// The samples sharing `file`'s value of `column`, `file` among them, sorted.
pub fn sharing(
    metadata: &MetaDataFileMap,
    file: &FileId,
    column: &MetaDataParameter,
) -> Vec<FileId> {
    let Some(value) = metadata.get(file).and_then(|columns| columns.get(column)) else {
        return Vec::new();
    };
    let mut files: Vec<FileId> = metadata
        .iter()
        .filter(|(_, columns)| columns.get(column) == Some(value))
        .map(|(other, _)| other.clone())
        .collect();
    files.sort();
    files
}
