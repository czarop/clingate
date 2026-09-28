//! Reviewing what the rules did.
//!
//! A rules run is kept once it is applied - every placement with every measure
//! its confidence was taken from - so a run can be reviewed after the fact:
//! by a person on the Gate Rules tab, by the tools for Claude, and later
//! against what a reviewer reported. See [`run_record`].

pub mod run_record;

pub use run_record::{RunRecord, placement_status};

/// The folder in a workspace that reviews are kept in, beside `rules` and
/// `figures`.
pub const REVIEWS_DIR: &str = "reviews";
