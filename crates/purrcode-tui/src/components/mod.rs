//! Reusable presentation components.
//!
//! Screens compose these; they do not format whole screens as strings. Each
//! component takes data plus [`crate::design::Tokens`] and knows nothing about
//! the application state machine, which is what makes them unit-testable
//! against a `TestBackend`.

pub mod activity_rail;
pub mod approval_card;
pub mod composer;
pub mod empty_state;
pub mod header;
pub mod hints;
pub mod inspector;
pub mod message;
pub mod modal;
pub mod palette;
pub mod status_chip;
pub mod validation_summary;
