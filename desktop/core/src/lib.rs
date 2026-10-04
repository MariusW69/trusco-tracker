//! TrusCo Tracker core: samples the foreground window, folds samples into
//! per-document work blocks, persists them locally and syncs them to TrusCo.
//! Everything here is UI-free so it can be checked for every target platform.

pub mod api;
pub mod docx;
pub mod engine;
pub mod platform;
pub mod secrets;
pub mod store;

pub use engine::{Activity, Block, Engine, Sample, Settings, WindowInfo};
