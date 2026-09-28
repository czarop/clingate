//! Everything clingate computes, with no window: reading FCS files and
//! compensating them, the gates and the gating tree, the axis settings and the
//! metadata that key them, Omiq's gating files, the rules that position gates,
//! and the workspace that holds it all.
//!
//! The desktop app (`clingate`) draws and edits what is here; anything that
//! drives clingate without a window - a tool server, a script - uses it the same
//! way. Nothing here may use components, `rsx!`, `use_context` or any other
//! part of the UI: only the stores, so the app can watch the state it declares.

pub mod axis_info;
pub use axis_info::AxisInfo;
pub mod axis_store;
#[cfg(test)]
mod axis_store_tests;
pub mod compensation;
pub mod events;
pub mod file_load;
#[cfg(any(test, feature = "test-support"))]
pub mod file_load_tests;
pub mod gate_move;
pub mod gate_rules;
pub mod gates;
pub mod history;
pub mod macros;
pub mod omiq;
pub mod review;
pub mod sample_pairs;
#[cfg(test)]
mod sample_pairs_tests;
pub mod session;
#[cfg(any(test, feature = "test-support"))]
pub mod test_workspace;
pub mod working_copy;
pub mod workspace;

/// An insertion-ordered map with the fast hasher.
pub type FxIndexMap<K, V> = indexmap::IndexMap<K, V, rustc_hash::FxBuildHasher>;
#[cfg(test)]
mod workspace_tests;
