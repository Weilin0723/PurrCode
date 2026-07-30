//! Adaptive layout and the explicit focus model.

pub mod adaptive;
pub mod focus;

pub use adaptive::{Breakpoint, WorkbenchAreas};
pub use focus::{Focus, FocusRing};
