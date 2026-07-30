//! Conversation-first daemon-backed terminal interface.

pub mod action_area;
mod app;
mod command_palette;
mod composer;
mod conversation;
mod diff_view;
pub mod glyphs;
mod keybindings;
mod provider_setup;
mod render;
mod skill_browser;
mod status_bar;
pub mod status_header;
mod streaming;
pub mod test_fixtures;
mod theme;
mod timeline;
pub mod trace_inspector;
mod ui_state;
mod workspace;

pub use app::{run, TuiConfig};
