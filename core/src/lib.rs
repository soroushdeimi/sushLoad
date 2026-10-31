//! sushLoad core: download engine, persistence and category rules. No UI code here.

pub mod categories;
pub mod engine;
pub mod model;
pub mod redact;

pub use engine::{Engine, sanitize_file_name, validate_request};
pub use model::*;
