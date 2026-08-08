//! Language intelligence: what the editor knows because a language server
//! told it.
//!
//! PurrCode is an LSP *client* (v1.2 Pillar 1). This module owns the request
//! side — when to ask, what to ask about, and how to fold the answer back into
//! the editor — plus the surfaces that are specific to language intelligence:
//! the references list, the document outline, and the diagnostics feed behind
//! the Problems panel.
//!
//! The governing rule here is that absence is never dressed up as success. A
//! machine with no rust-analyzer has no Rust intelligence, and every affordance
//! that depends on one says so instead of doing nothing when clicked. A file
//! the servers have not analysed yet is "not analysed yet", never "no problems
//! found".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Align, Layout, RichText, ScrollArea, Ui};

use super::PurrCodeIde;
use crate::daemon::Request;
use crate::model::{DocumentPosition, Location, Symbol};
use crate::theme;

/// How often the Problems panel refreshes its diagnostics.
///
/// Diagnostics are pushed by language servers as analysis completes, so this
/// poll is how the IDE notices. Two seconds keeps a compile-error round trip
/// feeling immediate without making an idle window chatty.
pub(crate) const DIAGNOSTIC_POLL: Duration = Duration::from_secs(2);

/// How long the pointer must rest before a hover is requested.
///
/// Without a delay, dragging the pointer across a line fires a request per
/// token and the answers arrive out of order for positions the user has
/// already left.
const HOVER_DELAY: Duration = Duration::from_millis(350);

/// How often open files are compared against what is on disk.
///
/// An agent writing a file the user has open is the common case this catches,
/// and a second is fast enough for that to feel immediate without stat-ing
/// every open buffer on every frame.
pub(crate) const DISK_CHECK: Duration = Duration::from_secs(1);

/// Where the pointer is resting, and since when.
#[derive(Clone, Debug)]
pub(crate) struct HoverProbe {
    pub path: PathBuf,
    pub position: DocumentPosition,
    pub since: Instant,
    /// Set once the request has gone out, so resting longer does not re-ask.
    pub requested: bool,
}

impl PurrCodeIde {
    /// The project root language servers should be started in.
    ///
    /// The open folder, not the file's directory: a server started inside
    /// `src/app` sees no workspace and resolves no dependencies.
    pub(crate) fn language_root(&self) -> PathBuf {
        self.repository.clone()
    }

    /// Asks the daemon which language servers exist on this machine. Called
    /// once per workspace so the UI can tell "unsupported file type" from
    /// "server not installed".
    pub(crate) fn probe_language_servers(&mut self) {
        if self.language.servers_checked || self.repository.as_os_str().is_empty() {
            return;
        }
        // Mark before sending: a failed probe still counts as answered, so a
        // daemon that is down does not produce a request per frame.
        self.language.servers_checked = true;
        self.client.send(Request::LspServers);
    }

    /// Hands a document to its language server so analysis can start.
    ///
    /// Opening is what makes a server produce diagnostics at all, so this runs
    /// on open and after every save. It is a no-op for a file no installed
    /// server covers.
    pub(crate) fn open_in_language_server(&mut self, path: &Path) {
        if !self.language.supports(path) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        self.client.send(Request::LspOpen {
            path: path.to_path_buf(),
            root: self.language_root(),
            text,
        });
    }

    /// Requests the outline for the active document.
    pub(crate) fn request_symbols(&mut self, path: &Path) {
        if !self.language.supports(path) {
            return;
        }
        self.client.send(Request::LspSymbols {
            path: path.to_path_buf(),
            root: self.language_root(),
        });
    }

    /// Jumps to the definition of the symbol under the cursor.
    pub(crate) fn go_to_definition(&mut self, path: &Path, position: DocumentPosition) {
        if !self.announce_if_unsupported(path, "Go to definition") {
            return;
        }
        self.client.send(Request::LspDefinition {
            path: path.to_path_buf(),
            root: self.language_root(),
            line: position.line,
            character: position.character,
        });
    }

    /// Lists every use of the symbol under the cursor in the aux panel.
    pub(crate) fn find_references(&mut self, path: &Path, position: DocumentPosition, word: &str) {
        if !self.announce_if_unsupported(path, "Find references") {
            return;
        }
        self.language.references.clear();
        self.language.references_checked = false;
        self.language.references_for = Some(word.to_owned());
        self.aux_panel = Some(super::AuxView::References);
        self.client.send(Request::LspReferences {
            path: path.to_path_buf(),
            root: self.language_root(),
            line: position.line,
            character: position.character,
            label: word.to_owned(),
        });
    }

    /// Formats the active document. `then_save` writes the result to disk once
    /// the edits land, which is how format-on-save is implemented.
    pub(crate) fn format_document(&mut self, path: &Path, then_save: bool) {
        if !then_save && !self.announce_if_unsupported(path, "Format document") {
            return;
        }
        if then_save && !self.language.supports(path) {
            return;
        }
        self.client.send(Request::LspFormat {
            path: path.to_path_buf(),
            root: self.language_root(),
            then_save,
        });
    }

    /// Explains why a language action is unavailable, rather than letting the
    /// click do nothing. Returns `true` when the action can proceed.
    fn announce_if_unsupported(&mut self, path: &Path, action: &str) -> bool {
        if self.language.supports(path) {
            return true;
        }
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!(".{ext}"))
            .unwrap_or_else(|| "this file type".to_owned());
        let installed = if self.language.servers.is_empty() {
            "No language server is installed on this machine.".to_owned()
        } else {
            format!(
                "Installed servers cover: {}.",
                self.language
                    .servers
                    .iter()
                    .flat_map(|(_, extensions)| extensions.iter())
                    .map(|extension| format!(".{extension}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        self.push_notice(crate::app::errors::Notice {
            kind: crate::app::errors::NoticeKind::Environment,
            headline: format!("{action} needs a language server for {extension}"),
            impact: installed,
            next_step: Some(
                "Install the language server for this language and reopen the folder.".to_owned(),
            ),
            actions: vec![crate::app::errors::NoticeAction::Dismiss],
            detail: None,
            clears_on_reconnect: false,
        });
        false
    }

    /// Notices when an open file has changed on disk under its buffer.
    ///
    /// This is the editor half of v1.2's external-change detection: the daemon
    /// watcher records the event for the audit log, and this is what stops the
    /// user losing work to it. A file that changed while the buffer was clean
    /// is reloaded silently — there is nothing to lose and nothing to decide.
    /// A file that changed while the buffer was dirty is flagged instead, so
    /// neither version is thrown away without the user choosing.
    pub(crate) fn detect_external_changes(&mut self) {
        if self.last_disk_check.elapsed() < DISK_CHECK {
            return;
        }
        self.last_disk_check = Instant::now();
        let mut reloaded = Vec::new();
        for file in &mut self.open_files {
            let stamp = super::disk_stamp(&file.path);
            // No stamp on either side means the comparison is not available;
            // saying "changed" would be a guess.
            let (Some(current), Some(previous)) = (stamp, file.disk_stamp) else {
                continue;
            };
            if current == previous {
                continue;
            }
            if file.modified {
                file.external_change = true;
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&file.path) {
                file.body = Ok(content);
                file.disk_stamp = stamp;
                reloaded.push(file.path.clone());
            }
        }
        // Re-open in the language server so its view matches the new text.
        for path in reloaded {
            self.open_in_language_server(&path);
        }
    }

    /// Reloads one file from disk, discarding the editor buffer.
    pub(crate) fn reload_from_disk(&mut self, index: usize) {
        let Some(path) = self.open_files.get(index).map(|file| file.path.clone()) else {
            return;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        if let Some(file) = self.open_files.get_mut(index) {
            file.body = Ok(content);
            file.modified = false;
            file.external_change = false;
            file.disk_stamp = super::disk_stamp(&path);
        }
        self.dirty.remove(&path);
        self.open_in_language_server(&path);
    }

    /// Keeps the editor buffer and overwrites the file on the next save.
    pub(crate) fn keep_editor_version(&mut self, index: usize) {
        if let Some(file) = self.open_files.get_mut(index) {
            file.external_change = false;
            // Adopt the current stamp so the same change is not reported
            // again on the next check.
            file.disk_stamp = super::disk_stamp(&file.path);
        }
    }

    /// Polls for published diagnostics on the Problems cadence.
    pub(crate) fn poll_diagnostics(&mut self) {
        if self.repository.as_os_str().is_empty() || self.language.servers.is_empty() {
            return;
        }
        if self.last_diagnostic_poll.elapsed() < DIAGNOSTIC_POLL {
            return;
        }
        self.last_diagnostic_poll = Instant::now();
        self.client.send(Request::LspDiagnostics);
    }

    /// Issues a hover request once the pointer has rested long enough.
    pub(crate) fn settle_hover(&mut self) {
        let Some(probe) = self.hover_probe.as_mut() else {
            return;
        };
        if probe.requested || probe.since.elapsed() < HOVER_DELAY {
            return;
        }
        probe.requested = true;
        let (path, position) = (probe.path.clone(), probe.position);
        if !self.language.supports(&path) {
            return;
        }
        self.client.send(Request::LspHover {
            path,
            root: self.language_root(),
            line: position.line,
            character: position.character,
        });
    }

    /// Records where the pointer is resting inside the editor.
    ///
    /// Moving to a different position clears the previous answer immediately:
    /// leaving the old hover on screen while the new one is in flight shows
    /// one token's type over another token.
    pub(crate) fn note_hover(&mut self, path: &Path, position: DocumentPosition) {
        let same = self
            .hover_probe
            .as_ref()
            .is_some_and(|probe| probe.path == path && probe.position == position);
        if same {
            return;
        }
        self.language.hover = None;
        self.language.hover_anchor = None;
        self.hover_probe = Some(HoverProbe {
            path: path.to_path_buf(),
            position,
            since: Instant::now(),
            requested: false,
        });
    }

    /// Draws the hover card for the token under the pointer.
    ///
    /// A diagnostic on that line comes first: when the editor knows both what
    /// a symbol is and what is wrong with it, the problem is the more useful
    /// answer. Nothing is drawn while a request is in flight — an empty card
    /// that fills in later reads as a rendering fault.
    pub(crate) fn show_hover_card(&self, ui: &Ui, path: &Path, position: DocumentPosition) {
        let diagnostics: Vec<String> = self
            .language
            .for_file(path)
            .iter()
            .filter(|diagnostic| diagnostic.start.line == position.line)
            .map(|diagnostic| format!("{}: {}", diagnostic.severity_label(), diagnostic.message))
            .collect();
        let anchored = self
            .language
            .hover_anchor
            .as_ref()
            .is_some_and(|(anchor_path, anchor)| anchor_path == path && *anchor == position);
        let hover = anchored.then(|| self.language.hover.clone()).flatten();
        if diagnostics.is_empty() && hover.is_none() {
            return;
        }
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            egui::Id::new("purrcode_language_hover"),
            egui::PopupAnchor::Pointer,
        )
        .gap(12.0)
        .show(|ui| {
            for diagnostic in &diagnostics {
                ui.label(
                    RichText::new(diagnostic)
                        .size(theme::TYPE_META)
                        .color(self.tokens.status_error),
                );
            }
            if let Some(hover) = &hover {
                if !diagnostics.is_empty() {
                    ui.separator();
                }
                // Hover payloads are markdown from the language server. They
                // are rendered as monospace text rather than parsed: this is
                // data from a subprocess, and it is displayed, not
                // interpreted.
                ui.label(RichText::new(hover).monospace().size(theme::TYPE_META));
            }
        });
    }

    /// Opens a location a language server pointed at, scrolled to its line.
    pub(crate) fn open_location(&mut self, location: &Location) {
        let line = location.start.display_line();
        self.open_file(location.path.clone());
        if let Some(file) = self
            .open_files
            .iter_mut()
            .find(|file| file.path == location.path)
        {
            file.scroll_to_line = Some(line);
        }
    }

    /// Applies formatting edits to the open buffer.
    ///
    /// The edits are applied to the editor's buffer rather than to the file on
    /// disk, so a format is undoable and visible before it is saved. When the
    /// format was triggered by ⌘S, the save follows once the text is in.
    pub(crate) fn apply_format_edits(
        &mut self,
        path: &Path,
        value: &serde_json::Value,
        then_save: bool,
    ) {
        let edits = value.as_array().cloned().unwrap_or_default();
        let Some(index) = self.open_files.iter().position(|file| file.path == path) else {
            return;
        };
        let Ok(original) = self.open_files[index].body.clone() else {
            return;
        };
        let formatted = apply_text_edits(&original, &edits);
        if formatted != original {
            self.open_files[index].body = Ok(formatted);
            self.open_files[index].modified = true;
            self.dirty.insert(path.to_path_buf());
        }
        if then_save {
            self.save_file(index);
        }
    }

    // ── Surfaces ──────────────────────────────────────────────────────

    /// The references panel: every use of one symbol, grouped by file.
    pub(crate) fn references_panel(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        ui.horizontal(|ui| {
            let heading = match &self.language.references_for {
                Some(symbol) => format!("References to {symbol}"),
                None => "References".to_owned(),
            };
            self.section_heading(ui, &heading);
        });

        if !self.language.references_checked {
            ui.label(
                RichText::new("Asking the language server…")
                    .small()
                    .color(tokens.text_muted),
            );
            return;
        }
        if self.language.references.is_empty() {
            ui.label(
                RichText::new("No references found.")
                    .small()
                    .color(tokens.text_muted),
            );
            return;
        }

        let repository = self.repository.clone();
        let references = self.language.references.clone();
        let mut open: Option<Location> = None;
        ScrollArea::vertical()
            .id_salt("lsp_references")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut current: Option<PathBuf> = None;
                for reference in &references {
                    if current.as_ref() != Some(&reference.path) {
                        let shown = reference
                            .path
                            .strip_prefix(&repository)
                            .unwrap_or(&reference.path);
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(shown.display().to_string())
                                .size(theme::TYPE_META)
                                .color(tokens.text_secondary),
                        );
                        current = Some(reference.path.clone());
                    }
                    let label = format!("  line {}", reference.start.display_line());
                    if ui
                        .add(egui::Label::new(
                            RichText::new(label)
                                .monospace()
                                .size(theme::TYPE_META)
                                .color(tokens.text_muted),
                        ))
                        .on_hover_text(reference.label(&repository))
                        .interact(egui::Sense::click())
                        .clicked()
                    {
                        open = Some(reference.clone());
                    }
                }
            });
        if let Some(location) = open {
            self.open_location(&location);
        }
    }

    /// The document outline, listed in the order the server reported.
    pub(crate) fn outline_panel(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.section_heading(ui, "Outline");
        let Some(active) = self
            .open_files
            .get(self.active_file)
            .map(|file| file.path.clone())
        else {
            ui.label(
                RichText::new("Open a file to see its outline.")
                    .small()
                    .color(tokens.text_muted),
            );
            return;
        };
        if self.language.symbols_for.as_ref() != Some(&active) {
            ui.label(
                RichText::new("Reading symbols…")
                    .small()
                    .color(tokens.text_muted),
            );
            return;
        }
        if self.language.symbols.is_empty() {
            ui.label(
                RichText::new(if self.language.supports(&active) {
                    "This file has no symbols the language server reports."
                } else {
                    "No language server covers this file type."
                })
                .small()
                .color(tokens.text_muted),
            );
            return;
        }

        let symbols = self.language.symbols.clone();
        let mut jump: Option<usize> = None;
        ScrollArea::vertical()
            .id_salt("lsp_outline")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for symbol in &symbols {
                    if symbol_row(ui, &tokens, symbol).clicked() {
                        jump = Some(symbol.start.display_line());
                    }
                }
            });
        if let Some(line) = jump
            && let Some(file) = self.open_files.get_mut(self.active_file)
        {
            file.scroll_to_line = Some(line);
        }
    }
}

/// Applies a set of LSP `TextEdit`s to a buffer.
///
/// Two details matter for correctness. Edits are applied last-to-first so that
/// applying one does not invalidate the offsets of those before it — LSP
/// guarantees the edits do not overlap, but says nothing about their order.
/// And positions are counted in UTF-16 code units, not bytes or characters, so
/// a file containing an emoji or CJK text would be corrupted by naive byte
/// indexing.
pub(crate) fn apply_text_edits(original: &str, edits: &[serde_json::Value]) -> String {
    let mut resolved: Vec<(usize, usize, String)> = edits
        .iter()
        .filter_map(|edit| {
            let start = byte_offset(
                original,
                edit["range"]["start"]["line"].as_u64()?,
                edit["range"]["start"]["character"].as_u64()?,
            )?;
            let end = byte_offset(
                original,
                edit["range"]["end"]["line"].as_u64()?,
                edit["range"]["end"]["character"].as_u64()?,
            )?;
            if start > end {
                return None;
            }
            Some((
                start,
                end,
                edit["new_text"].as_str().unwrap_or_default().to_owned(),
            ))
        })
        .collect();
    resolved.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));

    let mut text = original.to_owned();
    for (start, end, replacement) in resolved {
        if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        text.replace_range(start..end, &replacement);
    }
    text
}

/// Converts an LSP `(line, utf16 character)` position to a byte offset.
///
/// Returns `None` when the position is past the end of the text, so a stale
/// edit against a buffer the user has since shortened is dropped rather than
/// clamped into the middle of a line.
pub(crate) fn byte_offset(text: &str, line: u64, character: u64) -> Option<usize> {
    let mut offset = 0_usize;
    let mut remaining = line;
    let mut lines = text.split_inclusive('\n');
    for current in lines.by_ref() {
        if remaining == 0 {
            // Walk the line in UTF-16 code units, which is what the protocol
            // counts by default.
            let mut units = character;
            for (index, character) in current.char_indices() {
                if units == 0 {
                    return Some(offset + index);
                }
                let width = character.len_utf16() as u64;
                if width > units {
                    // The position landed inside a surrogate pair. Snap to the
                    // character start rather than producing a byte offset that
                    // is not a char boundary.
                    return Some(offset + index);
                }
                units -= width;
            }
            // A character offset at or past the end of the line resolves to
            // the line's end, which is how servers express "append here".
            return Some(offset + current.trim_end_matches(['\n', '\r']).len());
        }
        offset += current.len();
        remaining -= 1;
    }
    // One past the last line is the end of the text — servers use it to append.
    (remaining == 0).then_some(text.len())
}

fn symbol_row(ui: &mut Ui, tokens: &theme::Tokens, symbol: &Symbol) -> egui::Response {
    let response = ui.allocate_response(
        egui::vec2(ui.available_width(), theme::ROW_HEIGHT),
        egui::Sense::click(),
    );
    let rect = response.rect;
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, theme::RADIUS_CONTROL, tokens.surface_hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &symbol.name,
        egui::FontId::proportional(theme::TYPE_LABEL),
        tokens.text_primary,
    );
    ui.painter().text(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        symbol.kind_label(),
        egui::FontId::proportional(theme::TYPE_META),
        tokens.text_muted,
    );
    match &symbol.detail {
        Some(detail) => response.on_hover_text(detail),
        None => response,
    }
}

/// The language-server line for the status bar.
///
/// Reports what is actually running, so "no intelligence available" is visible
/// rather than being indistinguishable from "everything is fine".
pub(crate) fn status_summary(language: &crate::model::LanguageIntelligence) -> String {
    if !language.servers_checked {
        return "Language servers: checking…".to_owned();
    }
    if language.servers.is_empty() {
        return "No language servers".to_owned();
    }
    let names: Vec<&str> = language
        .servers
        .iter()
        .map(|(program, _)| program.as_str())
        .collect();
    names.join(", ")
}

/// Draws the diagnostics feed for the Problems panel.
pub(crate) fn diagnostics_list(
    ui: &mut Ui,
    tokens: &theme::Tokens,
    language: &crate::model::LanguageIntelligence,
    repository: &Path,
) -> Option<Location> {
    if !language.servers_checked || language.servers.is_empty() {
        ui.label(
            RichText::new(
                "No language server is installed, so there is nothing analysing this project.",
            )
            .small()
            .color(tokens.text_muted),
        );
        return None;
    }
    if !language.diagnostics_checked {
        ui.label(
            RichText::new("Language servers are still warming up…")
                .small()
                .color(tokens.text_muted),
        );
        return None;
    }
    let total: usize = language.diagnostics.values().map(Vec::len).sum();
    if total == 0 {
        // Deliberately not "no problems": the servers report what they have
        // analysed, and a project they have not finished reading is not a
        // project with a clean bill of health.
        ui.label(
            RichText::new("Nothing reported by the language servers so far.")
                .small()
                .color(tokens.text_muted),
        );
        return None;
    }

    let mut open: Option<Location> = None;
    ScrollArea::vertical()
        .id_salt("lsp_diagnostics")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (path, diagnostics) in &language.diagnostics {
                if diagnostics.is_empty() {
                    continue;
                }
                let shown = path.strip_prefix(repository).unwrap_or(path);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(shown.display().to_string())
                        .size(theme::TYPE_META)
                        .color(tokens.text_secondary),
                );
                for diagnostic in diagnostics {
                    let colour = if diagnostic.is_error() {
                        tokens.status_error
                    } else {
                        tokens.status_warning
                    };
                    let response = ui
                        .horizontal(|ui| {
                            ui.label(
                                RichText::new(diagnostic.severity_label())
                                    .size(theme::TYPE_META)
                                    .color(colour),
                            );
                            ui.label(
                                RichText::new(format!("{}", diagnostic.start.display_line()))
                                    .monospace()
                                    .size(theme::TYPE_META)
                                    .color(tokens.text_muted),
                            );
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(&diagnostic.message)
                                        .size(theme::TYPE_META)
                                        .color(tokens.text_primary),
                                )
                            })
                            .inner
                        })
                        .inner;
                    let response = match &diagnostic.source {
                        Some(source) => response.on_hover_text(match &diagnostic.code {
                            Some(code) => format!("{source} · {code}"),
                            None => source.clone(),
                        }),
                        None => response,
                    };
                    if response.interact(egui::Sense::click()).clicked() {
                        open = Some(Location {
                            path: path.clone(),
                            start: diagnostic.start,
                        });
                    }
                }
            }
        });
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Diagnostic, LanguageIntelligence};
    use serde_json::json;

    fn intelligence_with(servers: &[(&str, &[&str])]) -> LanguageIntelligence {
        LanguageIntelligence {
            servers: servers
                .iter()
                .map(|(program, extensions)| {
                    (
                        (*program).to_owned(),
                        extensions.iter().map(|e| (*e).to_owned()).collect(),
                    )
                })
                .collect(),
            servers_checked: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_file_type_no_installed_server_covers_is_not_supported() {
        let language = intelligence_with(&[("rust-analyzer", &["rs"])]);
        assert!(language.supports(Path::new("/repo/src/lib.rs")));
        // Extension matching is case-insensitive, but a `.py` file with only
        // rust-analyzer installed must not offer Python intelligence.
        assert!(language.supports(Path::new("/repo/src/LIB.RS")));
        assert!(!language.supports(Path::new("/repo/setup.py")));
        assert!(!language.supports(Path::new("/repo/LICENSE")));
    }

    #[test]
    fn nothing_analysed_yet_never_reads_as_a_clean_project() {
        // The distinction this protects: a project whose servers have not
        // answered must not look identical to a project with no problems.
        let language = intelligence_with(&[("rust-analyzer", &["rs"])]);
        assert!(!language.diagnostics_checked);
        let (errors, others) = language.counts();
        assert_eq!((errors, others), (0, 0));

        let mut analysed = language.clone();
        analysed.absorb_diagnostics(&json!({"files": [], "total": 0}));
        assert!(analysed.diagnostics_checked);
    }

    #[test]
    fn a_file_declared_clean_keeps_its_entry() {
        // An empty array is the server saying "this file is now clean". If
        // that dropped the key, a fixed file would be indistinguishable from
        // one that was never analysed.
        let mut language = intelligence_with(&[("rust-analyzer", &["rs"])]);
        language.absorb_diagnostics(&json!({
            "files": [{"path": "/repo/src/lib.rs", "diagnostics": []}],
        }));
        assert!(
            language
                .diagnostics
                .contains_key(Path::new("/repo/src/lib.rs"))
        );
        assert!(language.for_file(Path::new("/repo/src/lib.rs")).is_empty());
    }

    #[test]
    fn a_diagnostic_with_no_severity_counts_as_an_error() {
        // Servers may omit severity. Filing an unlabelled diagnostic as a hint
        // would hide it under a filter the user set to "errors only".
        let unlabelled = Diagnostic::parse(&json!({
            "range": {"start": {"line": 3, "character": 0}},
            "message": "something is wrong",
        }));
        assert!(unlabelled.is_error());
        assert_eq!(unlabelled.severity_label(), "Unspecified");

        let warning = Diagnostic::parse(&json!({
            "range": {"start": {"line": 3, "character": 0}},
            "severity": 2,
            "message": "watch out",
        }));
        assert!(!warning.is_error());
    }

    #[test]
    fn diagnostic_counts_split_errors_from_everything_else() {
        let mut language = intelligence_with(&[("rust-analyzer", &["rs"])]);
        language.absorb_diagnostics(&json!({
            "files": [{
                "path": "/repo/src/lib.rs",
                "diagnostics": [
                    {"range": {"start": {"line": 1, "character": 0}}, "severity": 1, "message": "boom"},
                    {"range": {"start": {"line": 2, "character": 0}}, "severity": 2, "message": "hmm"},
                    {"range": {"start": {"line": 3, "character": 0}}, "severity": 4, "message": "fyi"},
                ],
            }],
        }));
        assert_eq!(language.counts(), (1, 2));
    }

    #[test]
    fn a_location_prefers_the_identifier_over_the_whole_body() {
        // targetRange for a function is its entire body; jumping there would
        // scroll the user to the closing brace on a long function.
        let location = Location::parse(&json!({
            "target_uri": "file:///repo/src/lib.rs",
            "target_range": {"start": {"line": 10, "character": 0}, "end": {"line": 90, "character": 1}},
            "target_selection_range": {"start": {"line": 10, "character": 7}, "end": {"line": 10, "character": 20}},
        }))
        .expect("a well-formed link parses");
        assert_eq!(location.start.character, 7);
        assert_eq!(location.start.display_line(), 11);
        assert_eq!(location.label(Path::new("/repo")), "src/lib.rs:11");
    }

    #[test]
    fn a_link_with_no_target_is_dropped_rather_than_pointing_at_line_one() {
        assert!(Location::parse(&json!({"target_range": {}})).is_none());
        assert!(Location::parse(&json!({"target_uri": ""})).is_none());
    }

    fn edit(start: (u64, u64), end: (u64, u64), new_text: &str) -> serde_json::Value {
        json!({
            "range": {
                "start": {"line": start.0, "character": start.1},
                "end": {"line": end.0, "character": end.1},
            },
            "new_text": new_text,
        })
    }

    #[test]
    fn edits_apply_back_to_front_so_earlier_offsets_stay_valid() {
        // Given in ascending order, which is what most servers send. Applying
        // them in that order would shift every later range by the length
        // delta of the earlier one.
        let original = "let a=1;\nlet b=2;\n";
        let formatted = apply_text_edits(
            original,
            &[edit((0, 5), (0, 6), " = "), edit((1, 5), (1, 6), " = ")],
        );
        assert_eq!(formatted, "let a = 1;\nlet b = 2;\n");
    }

    #[test]
    fn positions_are_counted_in_utf16_units_not_bytes() {
        // "héllo" is 6 bytes but 5 UTF-16 units. A byte-indexed implementation
        // would replace the wrong span and could split the é in half.
        let original = "héllo world\n";
        let formatted = apply_text_edits(original, &[edit((0, 6), (0, 11), "there")]);
        assert_eq!(formatted, "héllo there\n");

        // An astral-plane character is two UTF-16 units wide.
        let original = "let x = \"🐱\";\n";
        let offset = byte_offset(original, 0, 9).expect("inside the string literal");
        assert!(original.is_char_boundary(offset));
        let after_cat = byte_offset(original, 0, 11).expect("after the emoji");
        assert_eq!(&original[offset..after_cat], "🐱");
    }

    #[test]
    fn a_position_inside_a_surrogate_pair_snaps_to_the_character() {
        // Character 10 lands between the two halves of the emoji. Returning a
        // byte offset mid-character would panic on `replace_range`.
        let original = "let x = \"🐱\";\n";
        let offset = byte_offset(original, 0, 10).expect("mid-surrogate resolves");
        assert!(original.is_char_boundary(offset));
    }

    #[test]
    fn an_edit_past_the_end_of_the_buffer_is_dropped_not_clamped() {
        // A reply arriving after the user deleted half the file must not be
        // applied to whatever now occupies those offsets.
        let original = "short\n";
        let formatted = apply_text_edits(original, &[edit((40, 0), (41, 0), "junk")]);
        assert_eq!(formatted, original);
    }

    #[test]
    fn a_character_offset_past_the_line_end_resolves_to_the_line_end() {
        // Servers use this to mean "append to this line".
        let original = "abc\ndef\n";
        let offset = byte_offset(original, 0, 99).expect("clamps to the line end");
        assert_eq!(offset, 3);
        let formatted = apply_text_edits(original, &[edit((0, 99), (0, 99), "!")]);
        assert_eq!(formatted, "abc!\ndef\n");
    }

    #[test]
    fn an_inverted_range_is_ignored() {
        let original = "abc\n";
        assert_eq!(
            apply_text_edits(original, &[edit((0, 3), (0, 1), "x")]),
            original
        );
    }

    #[test]
    fn the_status_line_distinguishes_checking_from_none_installed() {
        let mut language = LanguageIntelligence::default();
        assert_eq!(status_summary(&language), "Language servers: checking…");
        language.servers_checked = true;
        assert_eq!(status_summary(&language), "No language servers");
        language.servers.push(("gopls".into(), vec!["go".into()]));
        assert_eq!(status_summary(&language), "gopls");
    }
}
