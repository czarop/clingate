//! Positioning gates by rule, sample by sample.
//!
//! This is the second of the two autogating approaches, and the direct
//! replacement for how the gates are placed by hand today: a written rule says
//! where a gate belongs ("the FMO gate should contain 0.2-0.5% positive
//! events"), the rule is evaluated on one sample, and the position it yields is
//! applied to that sample's partners. The other approach - measuring how far a
//! population moved from a QC sample and carrying the gate with it - lives in
//! `gate_move` and is not wired in yet.
//!
//! The layers are kept apart on purpose. [`threshold`] is the part that decides
//! *where a line goes*, and it is plain arithmetic over a slice of f64: no
//! polars, no store, no Dioxus, no notion of a gate. Everything above it - which
//! sample to measure, which edge of which shape to move, how to write the result
//! back - is mechanical by comparison, and keeping it out of here is what lets
//! the decisions be tested exhaustively.

pub mod confidence;
pub mod rule;
pub mod rule_store;
pub mod threshold;

#[cfg(test)]
mod confidence_tests;
mod harness_tests;
mod rule_store_tests;
mod rule_tests;
mod threshold_tests;
