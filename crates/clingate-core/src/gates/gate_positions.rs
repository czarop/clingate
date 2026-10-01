//! How a gate is positioned - one mode for every sample: the gate as drawn,
//! one position per value of a metadata column, or one per sample - and
//! moving a gate between modes without moving it on any sample.
//!
//! The mode is read from the positions the gate holds, which the Omiq file
//! keeps along with the column they are grouped by. Everything that writes a
//! gate's positions goes through [`set_mode`], so it never holds more than
//! one kind - except a document from elsewhere, which [`settle_modes`]
//! puts right on loading.

use std::sync::Arc;

use anyhow::anyhow;
use rustc_hash::FxHashMap;

use crate::gates::gate_store::{FileId, GateId, GateSource, GateState, GateSubStore, GroupId};
use crate::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::{MetaDataFileMap, MetaDataKey, MetaDataParameter};

/// How a gate is positioned, for every sample alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// One position, the gate as drawn.
    Global,
    /// One position per value of a metadata column, which every sample with
    /// that value shows.
    ByColumn(MetaDataParameter),
    /// One position per sample.
    PerSample,
}

/// The gate's mode, read from the positions it holds. A gate holding more
/// than one kind - or positions by more than one column - reads as per
/// sample, which is what [`settle_modes`] makes it.
pub fn mode(state: &GateState, gate_id: &GateId) -> Mode {
    let columns = state.group_columns_newest_first(gate_id);
    match (state.has_sample_positions(gate_id), columns.as_slice()) {
        (false, []) => Mode::Global,
        (false, [column]) => Mode::ByColumn(column.clone()),
        _ => Mode::PerSample,
    }
}

fn mixed(state: &GateState, gate_id: &GateId) -> bool {
    let columns = state.group_columns_newest_first(gate_id).len();
    columns > 1 || (columns == 1 && state.has_sample_positions(gate_id))
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

/// Each value of `column`, with its samples sorted.
fn groups_of(
    metadata: &MetaDataFileMap,
    column: &MetaDataParameter,
) -> FxHashMap<GroupId, Vec<FileId>> {
    let mut groups: FxHashMap<GroupId, Vec<FileId>> = FxHashMap::default();
    for (file, columns) in metadata {
        if let Some(value) = columns.get(column) {
            groups.entry(value.clone()).or_default().push(file.clone());
        }
    }
    for files in groups.values_mut() {
        files.sort();
    }
    groups
}

/// The sample a value's position is taken from: `in_view` when it has that
/// value, otherwise the value's first.
fn source_for<'a>(files: &'a [FileId], in_view: Option<&'a FileId>) -> Option<&'a FileId> {
    in_view
        .filter(|file| files.contains(file))
        .or_else(|| files.first())
}

/// Put the gate in `to`, every sample showing what it showed - except that
/// a gate made global takes the position `in_view` shows, when one is given.
/// In the mode already, any sample or value missing a position of its own is
/// given what it shows.
pub fn set_mode(
    state: &mut GateState,
    gate_id: &GateId,
    to: &Mode,
    in_view: Option<&FileId>,
    metadata: &MetaDataFileMap,
) -> anyhow::Result<()> {
    let gate = state
        .registered_gate(gate_id)
        .ok_or_else(|| anyhow!("{gate_id} is not registered"))?;
    let ids = position_ids(&gate, gate_id);
    let already = mode(state, gate_id) == *to && !mixed(state, gate_id);
    let mut positions: Vec<(GateSource, Arc<dyn DrawableGate>)> = Vec::new();
    match to {
        Mode::Global if already => return Ok(()),
        Mode::Global => {
            let drawn = match in_view {
                Some(file) => copy_shown(state, gate_id, file, metadata)?,
                None => gate,
            };
            positions.push((GateSource::Global, drawn));
        }
        Mode::ByColumn(column) => {
            let groups = groups_of(metadata, column);
            if groups.is_empty() {
                return Err(anyhow!("no sample has a value of {column}"));
            }
            for (value, files) in groups {
                let key = MetaDataKey {
                    parameter: column.clone(),
                    group: value,
                };
                if already && state.has_group_position(gate_id, &key) {
                    continue;
                }
                let Some(source) = source_for(&files, in_view) else {
                    continue;
                };
                let shown = copy_shown(state, gate_id, source, metadata)?;
                positions.push((GateSource::Group((gate_id.clone(), key)), shown));
            }
        }
        Mode::PerSample => {
            let mut files: Vec<&FileId> = metadata.keys().collect();
            files.sort();
            for file in files {
                if already && state.has_sample_position(gate_id, file) {
                    continue;
                }
                let shown = copy_shown(state, gate_id, file, metadata)?;
                positions.push((GateSource::Sample((gate_id.clone(), file.clone())), shown));
            }
        }
    }
    if !already {
        state.retain_sample_positions(|(id, _)| !ids.contains(id));
        state.retain_group_positions(|(id, _)| !ids.contains(id));
    }
    for (source, gate) in positions {
        state.place_gate(&ids, &gate, &source);
    }
    Ok(())
}

/// Every gate holding positions of more than one kind made per sample, and
/// every gate's sample or value without a position of its own given what it
/// shows: a document from elsewhere, put in one mode per gate.
pub fn settle_modes(state: &mut GateState, metadata: &MetaDataFileMap) -> anyhow::Result<()> {
    let (grouped, own) = state.overridden_ids();
    let mut gates: Vec<GateId> = grouped
        .into_iter()
        .chain(own)
        .filter_map(|id| state.registered_gate(&id).map(|gate| gate.get_id()))
        .collect();
    gates.sort();
    gates.dedup();
    for gate_id in gates {
        let to = mode(state, &gate_id);
        set_mode(state, &gate_id, &to, None, metadata)?;
    }
    Ok(())
}

/// `from`'s position given to each of `to` as its own, for a gate per sample.
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

/// `from`'s position given to each of `values` of `column`, for a gate
/// positioned by that column.
pub fn copy_to_values(
    state: &mut GateState,
    gate_id: &GateId,
    from: &FileId,
    column: &MetaDataParameter,
    values: &[GroupId],
    metadata: &MetaDataFileMap,
) -> anyhow::Result<()> {
    let gate = copy_shown(state, gate_id, from, metadata)?;
    let ids = position_ids(&gate, gate_id);
    for value in values {
        let key = MetaDataKey {
            parameter: column.clone(),
            group: value.clone(),
        };
        state.place_gate(&ids, &gate, &GateSource::Group((gate_id.clone(), key)));
    }
    Ok(())
}

/// Every value of `column` in the workspace, sorted.
pub fn values_of(metadata: &MetaDataFileMap, column: &MetaDataParameter) -> Vec<GroupId> {
    let mut values: Vec<GroupId> = groups_of(metadata, column).into_keys().collect();
    values.sort();
    values
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
    groups_of(metadata, column)
        .remove(value)
        .unwrap_or_default()
}
