//! Conversation-first daemon-backed terminal interface.

pub mod action_area;
pub mod activity;
mod app;
pub mod approval;
pub mod command_palette;
pub mod components;
mod composer;
mod conversation;
pub mod design;
mod diff_view;
pub mod glyphs;
mod keybindings;
pub mod layout;
mod provider_setup;
mod render;
pub mod review;
pub mod screens;
mod skill_browser;
mod status_bar;
mod streaming;
pub mod test_fixtures;
mod theme;
mod timeline;
pub mod trace_inspector;
pub mod ui_actions;
mod ui_state;
mod workspace;

pub use app::{run, TuiConfig};
pub use ui_actions::{
    coverage, orphan_commands, Availability, AvailabilityRule, CoverageRow, CoverageStatus,
    ScenarioKind, Shortcut, ShortcutContext, UiActionCategory, UiActionDefinition, UiActionHandler,
    UiActionId, UiContext, UiRiskClass, REGISTRY, SCENARIOS,
};
