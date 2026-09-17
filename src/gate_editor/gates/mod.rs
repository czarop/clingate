pub mod gate_draft;
pub mod gate_drag;
pub mod gate_single;
pub mod gate_store;
pub mod gate_types;
pub use gate_store::{GateId, GateState, GatesOnPlotKey};
pub mod draw_gates;
pub mod gate_buttons;
pub mod gate_composite;
pub mod gate_filtering;
pub mod gate_hierarchy;
pub mod gate_stats;
pub mod gate_traits;

#[cfg(test)]
mod gate_filtering_tests;
#[cfg(test)]
mod gate_drag_tests;
#[cfg(test)]
mod gate_types_tests;
#[cfg(test)]
mod gate_stats_tests;
pub mod gate_paths;
#[cfg(test)]
mod gate_paths_tests;
