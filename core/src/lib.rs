//! sushLoad core: download engine, persistence and category rules. No UI code here.

pub mod categories;
pub mod engine;
pub mod model;

pub use engine::Engine;
pub use model::*;
