//! The universal composer — one input for all user intents.
//!
//! PRD §9, FR-001: one primary composer supports all user intents. No mode
//! selection is required before submission. The daemon resolves the workflow
//! from the request itself.
//!
//! This module is minimal by design; the visual rendering lives in
//! `workbench.rs`. This file re-exports the keyboard-handling logic as a
//! shared constant.

/// The placeholder text for the universal composer.
///
/// Intent-oriented (PRD §3.1, behavioral spec §1.1): reads like an IDE
/// command line, not a chat greeting.
pub const COMPOSER_HINT: &str = "Add OAuth to auth flow … | Fix redirect in callback.ts …";
