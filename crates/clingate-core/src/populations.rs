//! The populations a file's gates are measured on.
//!
//! Every ruled gate is measured on its parent population, and many gates share
//! a parent - every gate drawn under CD4+, every corner of a quadrant - while a
//! search or a score measures the same file for several sets of rules. So each
//! population's mask is worked out once per file, from its parent's, and a
//! gate's population is cut on only the columns its rule reads: a mask is a bit
//! per event, where a population on every channel is a copy of the file.
//!
//! Each gate filters as it does under a plot - `filter_events_to_mask`, the
//! filter [`crate::events::under_chain`] applies gate by gate - so a population
//! measured here is the one a person sees drawn.
//!
//! A channel is scaled the first time anything reads it, since scaling is most
//! of the cost of reading a file and a level of a run reads few of its
//! channels. The values are the ones [`crate::events::scaled`] gives.

use std::sync::Arc;

use flow_fcs::{TransformType, Transformable};
use polars::prelude::*;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::gates::GateState;
use crate::gates::gate_filtering::filter_events_to_mask;
use crate::gates::gate_store::{FileId, GateId, GateOverrideResolver};
use crate::omiq::metadata::MetaDataFileMap;

/// One file's events, and the mask of every population asked for so far.
pub struct Populations {
    /// The events, each channel in display space once it has been read.
    frame: DataFrame,
    /// The cofactor of every arcsinh channel not yet scaled.
    unscaled: FxHashMap<Arc<str>, f32>,
    resolver: GateOverrideResolver,
    masks: FxHashMap<Vec<GateId>, BooleanChunked>,
}

impl Populations {
    /// `frame`, unscaled, with its gates resolved as `file`'s metadata places
    /// them.
    pub fn of(
        state: &GateState,
        file: &FileId,
        frame: DataFrame,
        cofactors: &[(Arc<str>, f32)],
        metadata: &MetaDataFileMap,
    ) -> anyhow::Result<Self> {
        let groups = metadata
            .get(file)
            .ok_or_else(|| anyhow::anyhow!("no metadata for {file}"))?;
        Ok(Self::new(
            frame,
            cofactors,
            state.get_current_sample(file.clone(), groups),
        ))
    }

    /// `frame`, whose channels `cofactors` names are not yet scaled.
    pub fn new(
        frame: DataFrame,
        cofactors: &[(Arc<str>, f32)],
        resolver: GateOverrideResolver,
    ) -> Self {
        Self {
            frame,
            unscaled: cofactors.iter().cloned().collect(),
            resolver,
            masks: FxHashMap::default(),
        }
    }

    /// Which events every gate of `chain`, outermost first, holds.
    pub fn mask(&mut self, chain: &[GateId]) -> anyhow::Result<BooleanChunked> {
        let Some((last, above)) = chain.split_last() else {
            return Ok(BooleanChunked::full(
                "mask".into(),
                true,
                self.frame.height(),
            ));
        };
        if let Some(mask) = self.masks.get(chain) {
            return Ok(mask.clone());
        }
        let above = self.mask(above)?;
        let channels = self.channels_of(last);
        self.scale(&channels)?;
        let mask = above & filter_events_to_mask(&self.frame, last.clone(), &self.resolver)?;
        self.masks.insert(chain.to_vec(), mask.clone());
        Ok(mask)
    }

    /// The events under `chain` on those of `columns` the file has, each
    /// once, or on every column for `None`.
    pub fn under(
        &mut self,
        chain: &[GateId],
        columns: Option<&[Arc<str>]>,
    ) -> anyhow::Result<DataFrame> {
        let frame = match columns {
            Some(columns) => {
                let mut seen = FxHashSet::default();
                let columns: Vec<Arc<str>> = columns
                    .iter()
                    .filter(|column| seen.insert(*column) && self.frame.column(column).is_ok())
                    .cloned()
                    .collect();
                self.scale(&columns)?;
                self.frame.select(
                    columns
                        .iter()
                        .map(|column| PlSmallStr::from(column.as_ref())),
                )?
            }
            None => {
                let every: Vec<Arc<str>> = self.unscaled.keys().cloned().collect();
                self.scale(&every)?;
                self.frame.clone()
            }
        };
        if chain.is_empty() {
            return Ok(frame);
        }
        let mask = self.mask(chain)?;
        Ok(frame.filter(&mask)?)
    }

    /// The channels the gate `id` reads, its operands' for a boolean gate.
    fn channels_of(&self, id: &GateId) -> Vec<Arc<str>> {
        let Some(gate) = self
            .resolver
            .active_gates
            .get(id)
            .and_then(|drawable| drawable.get_gate_ref(Some(id)).cloned())
        else {
            return Vec::new();
        };
        let mut channels = vec![gate.parameters.0.clone(), gate.parameters.1.clone()];
        if let flow_gates::GateGeometry::Boolean { operands, .. } = &gate.geometry {
            for operand in operands {
                channels.extend(self.channels_of(operand));
            }
        }
        channels
    }

    /// Scale those of `channels` that are not yet, as the file is drawn.
    fn scale(&mut self, channels: &[Arc<str>]) -> anyhow::Result<()> {
        for channel in channels {
            let Some(&cofactor) = self.unscaled.get(channel) else {
                continue;
            };
            if let Ok(column) = self.frame.column(channel) {
                let transform = TransformType::Arcsinh { cofactor };
                let scaled: Vec<f32> = column
                    .f32()?
                    .rechunk()
                    .cont_slice()?
                    .par_iter()
                    .map(|value| transform.transform(value))
                    .collect();
                self.frame
                    .replace(channel, Series::new(channel.as_ref().into(), scaled).into())?;
            }
            // Only once it is scaled: a channel that could not be stays unscaled,
            // and fails every reader rather than only the first.
            self.unscaled.remove(channel);
        }
        Ok(())
    }

    /// How many populations have been worked out.
    #[cfg(test)]
    pub(crate) fn worked_out(&self) -> usize {
        self.masks.len()
    }

    /// Whether `channel` is still as the file holds it.
    #[cfg(test)]
    pub(crate) fn is_unscaled(&self, channel: &str) -> bool {
        self.unscaled.contains_key(channel)
    }
}
