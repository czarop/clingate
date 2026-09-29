//! The gallery tab: one gate, every sample in the run, as pictures.
//!
//! See [`window`] for what it is for. The split is by concern rather than by
//! size:
//!
//! - [`select`] - what a plot shows, as plain functions of the gate state.
//! - [`render`] - turning that into a bitmap, off the UI thread.
//! - [`cache`] - knowing when a bitmap is still the right picture.
//! - [`overlay`] - gate outlines as a drawing rather than as a control.
//! - [`plot`] - one plot on the page, with the queue that limits how many
//!   are drawn at once.
//! - [`pdf`] / [`export`] - the same pictures as a file, for the record.

pub mod cache;
pub mod export;
pub mod overlay;
pub mod pdf;
pub mod plot;
pub mod render;
pub mod select;
pub mod window;

#[cfg(test)]
mod tests;
