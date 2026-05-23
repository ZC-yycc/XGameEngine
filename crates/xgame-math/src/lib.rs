//! XGameEngine math library.
//!
//! Re-exports `glam` for linear algebra and adds engine-specific
//! types like `Transform` for hierarchical spatial transforms.

pub use glam;

pub mod projection;
mod transform;

pub use transform::Transform;
