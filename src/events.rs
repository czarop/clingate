//! A sample's events, read the one way every part of clingate reads them.
//!
//! Open the file, compensate it as its group says, put the arcsinh axes into
//! display space, keep each event's row in the file, then narrow to the events
//! under a gate chain and index two of their columns. The editor's plots, the
//! gallery, a rules run and anything that asks about a population all go
//! through here, so none of them can answer differently from the others: a
//! percentage in the gallery that disagreed with the editor's by a rounding
//! step, or a gate placed on events a person never sees, would each make every
//! number suspect.
//!
//! Everything here is blocking and plain: callers that must stay responsive
//! run it on a worker.

use std::path::Path;
use std::sync::Arc;

use flow_fcs::Fcs;
use flow_gates::EventIndex;
use polars::prelude::*;

use crate::gate_editor::gates::gate_filtering::filter_events_by_hierarchy_to_mask;
use crate::gate_editor::gates::gate_store::{GateId, GateOverrideResolver};

/// The column holding each event's row in the whole file, carried through
/// every filter so a gated event can be traced back to it.
pub const ROW_INDEX: &str = "original_index";

/// A spatial index over two columns of a population, and each indexed event's
/// row in the whole file.
#[derive(Clone)]
pub struct EventIndexMapped {
    pub event_index: Arc<EventIndex>,
    pub index_map: Arc<Vec<usize>>,
}

impl PartialEq for EventIndexMapped {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.event_index, &other.event_index)
            && Arc::ptr_eq(&self.index_map, &other.index_map)
    }
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

/// A file's events with its arcsinh channels in display space - only the
/// channels it carries, see [`cofactors_carried_by`] - and each event's row
/// in the file as [`ROW_INDEX`].
pub fn scaled(fcs: &Fcs, cofactors: &[(Arc<str>, f32)]) -> anyhow::Result<DataFrame> {
    let carried = cofactors_carried_by(fcs, cofactors);
    let refs: Vec<(&str, f32)> = carried.iter().map(|(k, v)| (k.as_ref(), *v)).collect();
    let frame = fcs.apply_arcsinh_transforms(&refs)?;
    Ok(frame.with_row_index(ROW_INDEX.into(), None)?)
}

/// Open a file, compensate it as `compensation` says, and scale it: its
/// events as they are drawn and gated.
pub fn read_scaled(
    path: &Path,
    compensation: &crate::compensation::Choice,
    cofactors: &[(Arc<str>, f32)],
) -> anyhow::Result<DataFrame> {
    let fcs = crate::compensation::open_compensated(path, compensation)?;
    scaled(&fcs, cofactors)
}

/// The events under a gate chain, outermost gate first. An empty chain is
/// every event, which is what the root shows.
pub fn under_chain(
    frame: &DataFrame,
    chain: &[GateId],
    resolver: &GateOverrideResolver,
) -> anyhow::Result<DataFrame> {
    if chain.is_empty() {
        return Ok(frame.clone());
    }
    let mask = filter_events_by_hierarchy_to_mask(frame, chain, resolver)?;
    Ok(frame.filter(&mask)?)
}

/// Every event's value on two columns, leaving out an event missing either.
pub fn points(frame: &DataFrame, x: &str, y: &str) -> anyhow::Result<Vec<(f32, f32)>> {
    let xs = frame.column(x)?.f32()?;
    let ys = frame.column(y)?.f32()?;
    Ok(xs
        .into_iter()
        .zip(ys.into_iter())
        .filter_map(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        })
        .collect())
}

/// A spatial index over two columns, which the statistics on a plot are
/// counted from.
pub fn index_over(frame: &DataFrame, x: &str, y: &str) -> anyhow::Result<Arc<EventIndex>> {
    let xr = frame.column(x)?.f32()?.rechunk();
    let yr = frame.column(y)?.f32()?.rechunk();
    let xs = xr
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("Failed to get contiguous slice for X"))?;
    let ys = yr
        .cont_slice()
        .map_err(|_| anyhow::anyhow!("Failed to get contiguous slice for Y"))?;
    Ok(Arc::new(
        EventIndex::build(xs, ys).map_err(|e| anyhow::anyhow!("{e}"))?,
    ))
}

/// Each event's row in the whole file, from [`ROW_INDEX`].
pub fn rows_in_file(frame: &DataFrame) -> anyhow::Result<Vec<usize>> {
    Ok(frame
        .column(ROW_INDEX)?
        .u32()?
        .into_iter()
        .flatten()
        .map(|v| v as usize)
        .collect())
}

/// [`index_over`] two columns, with each indexed event's row in the file.
pub fn index_mapped(frame: &DataFrame, x: &str, y: &str) -> anyhow::Result<EventIndexMapped> {
    Ok(EventIndexMapped {
        event_index: index_over(frame, x, y)?,
        index_map: Arc::new(rows_in_file(frame)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plotted_points_keep_their_order_and_skip_an_incomplete_event() {
        let df = df![
            "x" => [Some(1.0f32), None, Some(3.0), Some(4.0)],
            "y" => [Some(10.0f32), Some(20.0), None, Some(40.0)]
        ]
        .unwrap();
        assert_eq!(
            points(&df, "x", "y").unwrap(),
            vec![(1.0, 10.0), (4.0, 40.0)]
        );
    }

    #[test]
    fn a_missing_column_is_an_error_not_an_empty_plot() {
        let df = df!["x" => [1.0f32], "y" => [2.0f32]].unwrap();
        assert!(points(&df, "x", "CD3").is_err_and(|e| e.to_string().contains("CD3")));
    }

    #[test]
    fn an_index_is_built_over_the_two_named_columns() {
        let df =
            df!["x" => [1.0f32, 2.0, 3.0], "y" => [1.0f32, 2.0, 3.0], "z" => [0.0f32; 3]].unwrap();
        let index = index_over(&df, "x", "y").unwrap();
        let gate = flow_gates::Gate {
            id: Arc::from("g"),
            name: "g".into(),
            geometry: flow_gates::create_rectangle_geometry(
                vec![(1.5, 1.5), (3.5, 1.5), (3.5, 3.5), (1.5, 3.5)],
                "x",
                "y",
            )
            .unwrap(),
            mode: flow_gates::GateMode::Global,
            parameters: (Arc::from("x"), Arc::from("y")),
            label_position: None,
        };
        let mut inside = index.filter_by_gate(&gate).unwrap();
        inside.sort();
        assert_eq!(inside, vec![1, 2]);
    }

    #[test]
    fn an_index_over_a_column_that_is_not_float32_is_an_error() {
        let df = df!["x" => [1.0f64], "y" => [1.0f32]].unwrap();
        assert!(index_over(&df, "x", "y").is_err());
    }

    #[test]
    fn an_empty_chain_is_every_event() {
        let df = df!["x" => [1.0f32, 2.0]].unwrap();
        let resolver = GateOverrideResolver::default();
        assert_eq!(under_chain(&df, &[], &resolver).unwrap().height(), 2);
    }

    #[test]
    fn each_event_keeps_its_row_in_the_file() {
        let df = df!["x" => [5.0f32, 6.0, 7.0]]
            .unwrap()
            .with_row_index(ROW_INDEX.into(), None)
            .unwrap();
        let later = df.slice(1, 2);
        assert_eq!(rows_in_file(&later).unwrap(), vec![1, 2]);
    }
}
