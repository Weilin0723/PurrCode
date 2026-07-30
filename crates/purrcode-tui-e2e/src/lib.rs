//! End-to-end acceptance harness for the PurrCode terminal workbench.
//!
//! These tests drive the **real `purrcode` binary** inside a real pseudo-terminal
//! and assert on the parsed ANSI screen. Nothing here reaches into the TUI's
//! internals: the only inputs are key bytes and the only outputs are terminal
//! cells and durable daemon state, which is what makes a passing test evidence
//! that a user could actually perform the workflow.
//!
//! Two things are deliberately faked, and only these two:
//!
//! * the **daemon**, replaced by [`fake_daemon`] — an HTTP server that speaks the
//!   same API surface with scripted, deterministic durable state;
//! * the **provider**, replaced by [`fake_provider`] — deterministic model
//!   responses with controllable latency and failure.
//!
//! Everything else is production code.
//!
//! # No timing-only assertions
//!
//! Nothing in this harness sleeps and then assumes success. Every wait is
//! [`Harness::wait_for_text`] or [`Harness::wait_for_durable`] with an explicit
//! timeout, so a test either observes the state it needs or fails with the screen
//! it actually saw.

pub mod assertions;
pub mod fake_daemon;
pub mod fake_provider;
pub mod fixtures;
pub mod harness;
pub mod keys;
pub mod pty;
pub mod screen;

pub use assertions::{assert_absent, assert_visible};
pub use fake_daemon::{DaemonScript, FakeDaemon, StreamFrame};
pub use harness::{Harness, HarnessOptions};
pub use keys::Key;
pub use screen::Screen;

/// Default timeout for waiting on a visible or durable state change.
///
/// Generous enough for a cold binary on a loaded CI runner, short enough that a
/// genuinely stuck workflow fails the test rather than hanging the suite.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
