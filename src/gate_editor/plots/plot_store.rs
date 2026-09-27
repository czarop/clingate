use crate::events::EventIndexMapped;
use crate::gate_editor::gates::{gate_store::FileId, gate_types::GateStats};
use dioxus::prelude::*;
use rustc_hash::FxHashMap;
use std::sync::Arc;

#[derive(Default, Store, Clone)]
pub struct PlotStore {
    pub current_file_id: FileId,
    pub event_index_map: Option<EventIndexMapped>,
    pub gate_stats: FxHashMap<Arc<str>, GateStats>,
    // current settings ordered by the current sample
}
