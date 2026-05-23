//! XGameEngine core types.
//!
//! Foundation layer: window config, color, engine settings, and base traits.

mod color;
mod config;

pub use color::Color;
pub use config::{EngineConfig, WindowConfig};
