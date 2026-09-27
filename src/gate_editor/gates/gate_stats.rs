use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::events::EventIndexMapped;
use crate::gate_editor::gates::{
    gate_traits::DrawableGate,
    gate_types::{GateStatValue, GateStats},
};

pub fn get_percent_and_counts_gate(
    gate: Arc<dyn DrawableGate>,
    event_index_map: &EventIndexMapped,
    parental_events: f32,
) -> anyhow::Result<GateStats> {
    if !gate.is_composite() {
        let inner = gate.get_gate_ref(None).unwrap();
        let count = event_index_map.event_index.count_in_gate(inner)? as f32;
        let percent_parent = GateStatValue::Single((count / parental_events) * 100f32);
        let stats = GateStats {
            count: GateStatValue::Single(count),
            percent_parent,
        };
        Ok(stats)
    } else {
        let inner_ids = gate.get_inner_gate_ids();
        let capacity = inner_ids.len();
        let mut temp_counts =
            FxHashMap::with_capacity_and_hasher(capacity, rustc_hash::FxBuildHasher);
        let mut temp_percents =
            FxHashMap::with_capacity_and_hasher(capacity, rustc_hash::FxBuildHasher);
        for inner_id in inner_ids {
            let inner = gate.get_gate_ref(Some(&inner_id)).unwrap();
            let count = event_index_map.event_index.count_in_gate(inner)? as f32;
            let percent_parent = (count / parental_events) * 100f32;
            temp_counts.insert(inner_id.clone(), count);
            temp_percents.insert(inner_id.clone(), percent_parent);
        }
        let counts = GateStatValue::Composite(temp_counts);
        let percents = GateStatValue::Composite(temp_percents);
        let stats = GateStats {
            count: counts,
            percent_parent: percents,
        };
        Ok(stats)
    }
}
