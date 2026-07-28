//! Export targets.
//!
//! A target owns everything one output format decides for itself: its time
//! grid, its marker vocabulary, its track cosmetics, its schema version. It
//! reads [`crate::engine::projection::ProjectedProject`] and nothing else, so
//! adding a target cannot reach back into the conversion engine and cannot
//! change what another target writes.
pub mod svp;
