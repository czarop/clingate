//! The editor's plot reads its events through [`crate::events`]; these are
//! the async wrappers that put the blocking steps on a worker.

use std::sync::Arc;

use crate::gate_editor::gates::gate_store::{GateId, GateOverrideResolver};

use flow_fcs::Fcs;
use polars::prelude::*;
use tokio::task;

/// Open a file, compensated as `compensation` says - see
/// [`crate::compensation::open_compensated`].
pub async fn get_flow_data(
    path: std::path::PathBuf,
    compensation: crate::compensation::Choice,
) -> Result<Fcs, Arc<anyhow::Error>> {
    task::spawn_blocking(move || {
        crate::compensation::open_compensated(&path, &compensation).map_err(Arc::new)
    })
    .await
    .map_err(|e| Arc::new(e.into()))?
}

/// The events under `chain` - see [`crate::events::under_chain`].
pub async fn get_filtered_dataframe(
    df: Arc<DataFrame>,
    chain: Vec<GateId>,
    resolver: GateOverrideResolver,
) -> Result<Arc<DataFrame>, anyhow::Error> {
    if chain.is_empty() {
        return Ok(df);
    }
    task::spawn_blocking(move || -> Result<Arc<DataFrame>, anyhow::Error> {
        Ok(Arc::new(crate::events::under_chain(
            &df, &chain, &resolver,
        )?))
    })
    .await?
}

/// Every event's value on two columns - see [`crate::events::points`].
pub async fn zip_cols_from_filtered_df(
    df: Arc<DataFrame>,
    col1_name: Arc<str>,
    col2_name: Arc<str>,
) -> Result<Vec<(f32, f32)>, anyhow::Error> {
    task::spawn_blocking(move || crate::events::points(&df, &col1_name, &col2_name)).await?
}
