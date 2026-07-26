//! Conversation-first daemon-backed terminal interface.

mod app;
mod command_palette;
mod composer;
mod conversation;
mod diff_view;
mod keybindings;
mod provider_setup;
mod render;
mod skill_browser;
mod status_bar;
mod streaming;
mod workspace;

pub use app::{run, TuiConfig};
