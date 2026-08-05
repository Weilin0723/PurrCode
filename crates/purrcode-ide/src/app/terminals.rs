//! Terminal management methods for the application context.
//!
//! These methods handle terminal responses and rendering coordination.
//! The actual terminal rendering logic lives in `crate::terminal::GuiTerminal`.

use crate::app::PurrCodeIde;
use crate::terminal::{GuiTerminal, TerminalKind, TerminalScope};

fn started_terminal(value: &serde_json::Value) -> &serde_json::Value {
    value.get("terminal").unwrap_or(value)
}

/// The bytes of one `/v1/terminals/{id}/output` chunk.
///
/// `TerminalChunk::bytes` is a `Vec<u8>` on the wire, so it arrives as a JSON
/// array of numbers rather than as a string: a terminal transcript is not valid
/// UTF-8 in general, and pretending otherwise would corrupt every box-drawing
/// character a TUI writes.
fn chunk_bytes(chunk: &serde_json::Value) -> Vec<u8> {
    match chunk.get("bytes") {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(serde_json::Value::as_u64)
            .map(|byte| byte as u8)
            .collect(),
        // A daemon that ever answers with a plain string still works.
        Some(serde_json::Value::String(text)) => text.as_bytes().to_vec(),
        _ => Vec::new(),
    }
}

/// The shell a snapshot was started with, reduced to the program name.
fn snapshot_kind(snapshot: &serde_json::Value) -> TerminalKind {
    snapshot
        .get("shell")
        .and_then(serde_json::Value::as_str)
        .and_then(|shell| shell.rsplit('/').next())
        .map(TerminalKind::classify)
        .unwrap_or(TerminalKind::Shell)
}

impl PurrCodeIde {
    /// Handle a terminal started response from the daemon.
    pub(crate) fn terminal_started(&mut self, value: serde_json::Value) {
        let snapshot = started_terminal(&value).clone();
        let Some(id) = snapshot
            .get("terminal_id")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        if let Some(index) = self.terminals.iter().position(|t| t.terminal_id == id) {
            // A reconnect can report a terminal we are already showing.
            self.terminals[index].adopt_snapshot(&snapshot);
            self.active_terminal = index;
            self.bottom_panel = Some(super::DockTab::Terminal);
            return;
        }
        let mut term = GuiTerminal::new(
            id.to_owned(),
            snapshot_kind(&snapshot),
            self.terminal_scope(&snapshot),
        );
        term.adopt_snapshot(&snapshot);
        // The daemon replays a bounded tail so a reattached terminal opens on
        // the prompt the user last saw instead of on an empty grid.
        let tail = snapshot
            .get("transcript_tail")
            .map(|value| chunk_bytes(&serde_json::json!({ "bytes": value })))
            .unwrap_or_default();
        if !tail.is_empty() {
            term.append_chunk(&tail, tail.len() as u64, false);
        }
        self.terminals.push(term);
        self.active_terminal = self.terminals.len() - 1;
        self.bottom_panel = Some(super::DockTab::Terminal);
        // A terminal the user just opened should take the keyboard: making
        // them click the black rectangle first is the difference between a
        // terminal and a picture of one.
        self.focus_terminal = true;
    }

    /// Handle terminal output from the daemon.
    ///
    /// The daemon answers `{"chunk": {"bytes": [...], "next_offset": n,
    /// "truncated": bool}, "alive": bool, "owner": ..., "generation": n}` and
    /// `daemon.rs` stamps the terminal id into it.
    pub(crate) fn terminal_output(&mut self, value: serde_json::Value) {
        let Some(id) = value.get("terminal_id").and_then(serde_json::Value::as_str) else {
            return;
        };
        let Some(index) = self.terminals.iter().position(|t| t.terminal_id == id) else {
            return;
        };
        let term = &mut self.terminals[index];
        if let Some(chunk) = value.get("chunk") {
            let bytes = chunk_bytes(chunk);
            let next_offset = chunk
                .get("next_offset")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_else(|| term.last_offset() + bytes.len() as u64);
            let truncated = chunk
                .get("truncated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            term.append_chunk(&bytes, next_offset, truncated);
        }
        term.adopt_snapshot(&value);
    }

    /// Handle terminal state change from the daemon.
    pub(crate) fn terminal_changed(&mut self, value: serde_json::Value) {
        let snapshot = started_terminal(&value).clone();
        let Some(id) = snapshot
            .get("terminal_id")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        if let Some(index) = self.terminals.iter().position(|t| t.terminal_id == id) {
            self.terminals[index].adopt_snapshot(&snapshot);
        }
    }

    /// Handle the list of terminals from the daemon.
    pub(crate) fn terminals_listed(&mut self, value: serde_json::Value) {
        let items = match value.as_array() {
            Some(items) => items.clone(),
            None => return,
        };
        for item in &items {
            let Some(id) = item.get("terminal_id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            match self.terminals.iter().position(|t| t.terminal_id == id) {
                Some(index) => self.terminals[index].adopt_snapshot(item),
                None => {
                    let mut term = GuiTerminal::new(
                        id.to_owned(),
                        snapshot_kind(item),
                        self.terminal_scope(item),
                    );
                    term.adopt_snapshot(item);
                    self.terminals.push(term);
                }
            }
        }
    }

    /// Which tree a terminal is looking at.
    ///
    /// PRD §14: this is never a guess from "is a session selected". It is read
    /// from the directory the daemon actually started the process in, because a
    /// user who cannot tell whether `git status` describes their checkout or the
    /// agent's is being actively misled.
    fn terminal_scope(&self, snapshot: &serde_json::Value) -> TerminalScope {
        let repository = self.repository_string();
        match snapshot
            .get("working_directory")
            .and_then(serde_json::Value::as_str)
        {
            Some(directory) if !repository.is_empty() && directory.starts_with(&repository) => {
                TerminalScope::Workspace
            }
            Some(_) => TerminalScope::AgentWorktree,
            None => TerminalScope::Workspace,
        }
    }

    /// Poll all active terminals for new output.
    pub(crate) fn poll_terminals(&mut self) {
        for term in self.terminals.iter() {
            if term.is_running() {
                self.client.send(crate::daemon::Request::PollTerminal {
                    terminal: term.terminal_id.clone(),
                    since: term.last_offset(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::started_terminal;

    #[test]
    fn unwraps_the_daemon_terminal_envelope() {
        let response = serde_json::json!({
            "terminal": { "terminal_id": "terminal-1", "kind": "shell" }
        });
        assert_eq!(started_terminal(&response)["terminal_id"], "terminal-1");
    }

    #[test]
    fn accepts_legacy_unwrapped_terminal_payloads() {
        let response = serde_json::json!({ "terminal_id": "terminal-2" });
        assert_eq!(started_terminal(&response)["terminal_id"], "terminal-2");
    }
}
