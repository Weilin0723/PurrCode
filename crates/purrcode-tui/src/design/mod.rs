//! Semantic design system for the workbench.
//!
//! Components ask for a *role* (`surface`, `border`, `danger`) rather than a
//! terminal colour, and for named spacing and symbols rather than literals.
//! That is what makes `NO_COLOR`, monochrome terminals and the ASCII fallback
//! work without every component reimplementing them.

pub mod spacing;
pub mod symbols;
pub mod tokens;

pub use spacing::Spacing;
pub use symbols::Symbols;
pub use tokens::{Emphasis, Role, Tokens};
