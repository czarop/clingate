//! Reviewing what the rules did.
//!
//! A rules run is kept once it is applied - every placement with every measure
//! its confidence was taken from - so a run can be reviewed after the fact:
//! by a person on the Gate Rules tab, by the tools for Claude, and later
//! against what a reviewer reported. See [`run_record`].
//!
//! A reviewer reports a gate the rules placed badly ([`report`]); marking a
//! run reviewed records every other placement as accepted, and copies the
//! review into the review library ([`library`]) where reviews add up across
//! runs.

pub mod assess;
pub mod board;
pub mod events;
pub mod explain;
pub mod library;
pub mod replay;
pub mod report;
pub mod run_record;
pub mod shape;

pub use report::{PlacementReport, Problem, ReportRequest, RunReview};
pub use run_record::{RunRecord, placement_status};

/// The folder in a workspace that reviews are kept in, beside `rules` and
/// `figures`.
pub const REVIEWS_DIR: &str = "reviews";
