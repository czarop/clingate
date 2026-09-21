use std::sync::Arc;

use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{GateOverrideResolver, NodeId};

use dioxus::prelude::*;
use dioxus::stores::SyncStore;
use flow_fcs::Fcs;
use flow_gates::EventIndex;

use polars::prelude::*;
use tokio::task;

pub async fn get_flow_data(path: std::path::PathBuf) -> Result<Fcs, Arc<anyhow::Error>> {
    task::spawn_blocking(move || {
        let fcs_file = Fcs::open(path.to_str().unwrap_or_default())?;
        Ok(fcs_file)
    })
    .await
    .map_err(|e| Arc::new(e.into()))?
}

/// The cofactors that name a channel this file actually carries.
///
/// The axis settings describe the whole panel as the scaling file defines it,
/// and `apply_arcsinh_transforms` errors on the first parameter it cannot find.
/// So one channel absent from one file - a shorter panel, a renamed detector -
/// used to lose the entire plot rather than one axis of it, and on the editor
/// tab it lost it silently: the frame never arrived and the plot sat on
/// "Rendering Plot..." indefinitely, which reads as slowness rather than as an
/// error.
///
/// Nothing measured changes by skipping a missing channel. A transform for a
/// column that is not there cannot have reached a plot's axes or its gating
/// chain, both of which are columns that are. A gate that needs the missing
/// channel still fails at the filter and says so, which is the right answer to
/// gating on a parameter the file does not have.
pub fn cofactors_carried_by(fcs: &Fcs, cofactors: &[(Arc<str>, f32)]) -> Vec<(Arc<str>, f32)> {
    let present: rustc_hash::FxHashSet<&str> = fcs
        .parameters
        .values()
        .map(|parameter| parameter.channel_name.as_ref())
        .collect();
    cofactors
        .iter()
        .filter(|(channel, _)| present.contains(channel.as_ref()))
        .cloned()
        .collect()
}

pub async fn get_filtered_dataframe(
    df: Arc<DataFrame>,
    parental_gate_id: Option<Arc<str>>,
    resolver: GateOverrideResolver,
) -> Result<Arc<DataFrame>, anyhow::Error> {
    let df_clone = df.clone();
    let gate_store = use_context::<SyncStore<GateState>>();

    task::spawn_blocking(move || -> Result<Arc<DataFrame>, anyhow::Error> {
        // `parental_gate_id` is a tree position, not a gate. Resolving the chain
        // through the node is what makes a linked gate's statistics right: the
        // same gate at two points in the tree sits under different ancestors, so
        // a chain taken from the gate alone was whichever placement won the
        // import.
        let gate_chain: Option<Vec<Arc<str>>> = if let Some(parent) = parental_gate_id {
            let arcs = gate_store.peek().gate_chain_for_node(&NodeId::from(parent));

            if arcs.is_empty() { None } else { Some(arcs) }
        } else {
            None
        };

        if let Some(chain) = gate_chain {
            println!("{:#?}", chain);

            // 1. Get the final narrowed mask for the whole hierarchy
            let mask = super::super::gates::gate_filtering::filter_events_by_hierarchy_to_mask(
                &df, &chain, &resolver,
            )?;
            // 2. Filter the dataframe
            Ok(df.filter(&mask)?.into())
        } else {
            Ok(df_clone)
        }
    })
    .await?
}

pub async fn zip_cols_from_filtered_df(
    df: Arc<DataFrame>,
    col1_name: Arc<str>,
    col2_name: Arc<str>,
) -> Result<Vec<(f32, f32)>, anyhow::Error> {
    let df_clone = df.clone();

    task::spawn_blocking(move || -> Result<Vec<(f32, f32)>, anyhow::Error> {
        let x_series = df_clone.column(&col1_name)?.f32()?;
        let y_series = df_clone.column(&col2_name)?.f32()?;

        let zipped_cols: Vec<(f32, f32)> = x_series
            .into_iter()
            .zip(y_series.into_iter())
            .filter_map(|(x, y)| match (x, y) {
                (Some(vx), Some(vy)) => Some((vx, vy)),
                _ => None,
            })
            .collect();

        Ok(zipped_cols)
    })
    .await?
}

pub fn get_event_mask_from_scaled_df(
    scaled_df: Arc<DataFrame>,
    col1_name: Arc<str>,
    col2_name: Arc<str>,
) -> anyhow::Result<Arc<EventIndex>> {
    let col1_name = col1_name.clone();
    let col2_name = col2_name.clone();

    let x_rechunked = scaled_df.column(&col1_name)?.f32()?.rechunk();
    let y_rechunked = scaled_df.column(&col2_name)?.f32()?.rechunk();
    let x_slice = x_rechunked
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("Failed to get contiguous slice for X"))?;
    let y_slice = y_rechunked
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("Failed to get contiguous slice for Y"))?;

    match EventIndex::build(x_slice, y_slice) {
        Ok(index) => Ok(Arc::new(index)),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}
