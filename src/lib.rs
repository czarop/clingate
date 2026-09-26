use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;
pub mod components;
pub mod file_load;
#[cfg(test)]
mod file_load_tests;
pub mod gate_editor;
pub mod gate_move;
pub mod gate_rules;
pub mod omiq;
pub mod searchable_select;
pub type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;
pub mod workspace;
#[cfg(test)]
mod workspace_tests;
