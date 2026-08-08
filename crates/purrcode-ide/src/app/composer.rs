//! The universal composer — one input for all user intents, and the
//! completion surface behind its `/`, `@` and `#` prefixes.
//!
//! PRD §9, FR-001: one primary composer supports all user intents. No mode
//! selection is required before submission. The daemon resolves the workflow
//! from the request itself.
//!
//! v1.2 Pillar 2 makes the prefixes real. Until now they were static hints
//! that deliberately did not pretend to be clickable, because nothing was
//! behind them. Now `/` completes commands the daemon actually publishes, `@`
//! completes files and folders that actually exist, and `#` completes symbols
//! a language server actually reported. The rule that produced the honest
//! static version still governs: a suggestion is only offered when selecting
//! it will do something.
//!
//! The visual rendering of the composer frame lives in `workbench.rs`; this
//! file owns the grammar, the completion model, and the resolved-reference
//! row beneath the field.

use egui::{Key, RichText, Ui};

use super::PurrCodeIde;
use crate::daemon::Request;
use crate::theme;

/// The placeholder text for the universal composer.
///
/// Intent-oriented (PRD §3.1, behavioral spec §1.1): reads like an IDE
/// command line, not a chat greeting.
pub const COMPOSER_HINT: &str = "Add OAuth to auth flow … | Fix redirect in callback.ts …";

/// How many completions to offer at once.
const MAX_SUGGESTIONS: usize = 8;

/// What the user is part-way through typing at the caret.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActiveToken {
    /// Byte range of the token in the composer text.
    pub start: usize,
    pub end: usize,
    /// The whole token including its prefix, e.g. `@src/li`.
    pub text: String,
}

impl ActiveToken {
    /// The prefix character that decides which completions apply.
    fn prefix(&self) -> Option<char> {
        self.text.chars().next()
    }

    /// The typed part after the prefix, lowercased for matching.
    fn query(&self) -> String {
        self.text.chars().skip(1).collect::<String>().to_lowercase()
    }
}

/// One offered completion.
#[derive(Clone, Debug)]
pub(crate) struct Suggestion {
    /// The text that replaces the active token when chosen.
    pub insert: String,
    /// What the row shows.
    pub label: String,
    /// The muted right-hand side: a description, a kind, a containing path.
    pub detail: String,
}

/// Finds the token the caret is inside, if it is a reference or command.
///
/// Tokens are whitespace-delimited, matching the grammar the daemon's
/// reference resolver parses, so what completes here is exactly what resolves
/// there. Returns `None` when the caret is in ordinary prose — the completion
/// popup must not appear while someone is writing a sentence.
pub(crate) fn active_token(text: &str, caret: usize) -> Option<ActiveToken> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let start = text[..caret]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let end = text[caret..]
        .char_indices()
        .find(|(_, character)| character.is_whitespace())
        .map_or(text.len(), |(index, _)| caret + index);
    let token = &text[start..end];
    let first = token.chars().next()?;
    if !matches!(first, '/' | '@' | '#') {
        return None;
    }
    // A bare `/` only starts a command at the very beginning of the draft.
    // Otherwise every path in a sentence ("look in src/foo") would open the
    // command list.
    if first == '/' && start != 0 {
        return None;
    }
    Some(ActiveToken {
        start,
        end,
        text: token.to_owned(),
    })
}

impl PurrCodeIde {
    /// Builds the completion list for the token being typed.
    ///
    /// Every source here is something the window actually has: commands the
    /// daemon published, files on disk, symbols a language server reported.
    /// Nothing is offered speculatively.
    pub(crate) fn suggestions_for(&self, token: &ActiveToken) -> Vec<Suggestion> {
        let query = token.query();
        match token.prefix() {
            Some('/') => self
                .commands
                .iter()
                .filter(|(name, _, _)| name.trim_start_matches('/').starts_with(&query))
                .take(MAX_SUGGESTIONS)
                .map(|(name, description, group)| Suggestion {
                    insert: name.clone(),
                    label: name.clone(),
                    detail: format!("{description} · {group}"),
                })
                .collect(),
            Some('@') => self.reference_suggestions(&query),
            Some('#') => self
                .workspace_symbols
                .iter()
                .filter(|(name, _, _)| name.to_lowercase().contains(&query))
                .take(MAX_SUGGESTIONS)
                .map(|(name, kind, container)| Suggestion {
                    insert: format!("#{name}"),
                    label: format!("#{name}"),
                    detail: match container {
                        Some(container) => format!("{kind} · {container}"),
                        None => kind.clone(),
                    },
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Completions for `@`: the fixed forms first, then real paths.
    fn reference_suggestions(&self, query: &str) -> Vec<Suggestion> {
        let mut out = Vec::new();
        // The non-path references are offered by name so they are
        // discoverable; a user cannot guess `@git:HEAD~1` from an empty field.
        for (insert, detail) in [
            ("@diff", "Uncommitted changes in this repository"),
            ("@context", "The session's current context summary"),
            ("@git:HEAD~1", "A file or commit at a Git reference"),
            ("@folder:", "Every file in a folder"),
        ] {
            if insert.trim_start_matches('@').starts_with(query) {
                out.push(Suggestion {
                    insert: insert.to_owned(),
                    label: insert.to_owned(),
                    detail: detail.to_owned(),
                });
            }
        }
        // Then paths. A `@folder:` prefix means the user is naming a folder,
        // so only directories are offered.
        let want_folders = query.starts_with("folder:");
        let path_query = query.trim_start_matches("folder:");
        for path in self.matching_paths(path_query, want_folders, MAX_SUGGESTIONS) {
            let relative = path.strip_prefix(&self.repository).unwrap_or(&path);
            let shown = relative.display().to_string();
            out.push(Suggestion {
                insert: if want_folders {
                    format!("@folder:{shown}")
                } else {
                    format!("@{shown}")
                },
                label: format!("@{shown}"),
                detail: if want_folders { "folder" } else { "file" }.to_owned(),
            });
        }
        out.truncate(MAX_SUGGESTIONS);
        out
    }

    /// Replaces the active token with a chosen completion.
    ///
    /// A trailing space is appended so the next thing typed starts a new
    /// token rather than extending the one just completed.
    pub(crate) fn accept_suggestion(&mut self, token: &ActiveToken, suggestion: &Suggestion) {
        let mut next = String::with_capacity(self.composer.len() + suggestion.insert.len());
        next.push_str(&self.composer[..token.start]);
        next.push_str(&suggestion.insert);
        // `@folder:` is a prefix, not a complete reference: leave the caret
        // against it so the user keeps typing the folder name.
        let complete = !suggestion.insert.ends_with(':');
        if complete {
            next.push(' ');
        }
        let caret = next.len();
        next.push_str(&self.composer[token.end..]);
        self.composer = next;
        self.composer_caret = caret;
        self.focus_composer = true;
        self.completion_index = 0;
        if complete {
            self.refresh_references();
        }
    }

    /// Asks the daemon to resolve the draft's references.
    ///
    /// Debounced by comparing against the last text sent: this runs from the
    /// frame loop, and re-resolving an unchanged draft every frame would put
    /// a file read behind every repaint.
    pub(crate) fn refresh_references(&mut self) {
        let text = self.composer.clone();
        if self.references_requested_for.as_deref() == Some(text.as_str()) {
            return;
        }
        self.references_requested_for = Some(text.clone());
        if text.trim().is_empty() {
            self.resolved_references.clear();
            return;
        }
        // Nothing to resolve without a folder to resolve against.
        if self.repository.as_os_str().is_empty() {
            return;
        }
        self.client.send(Request::ResolveReferences {
            repository: self.repository_string(),
            text,
        });
    }

    /// Keeps the `#symbol` completion list current as the user types.
    pub(crate) fn refresh_workspace_symbols(&mut self, query: &str) {
        if self.workspace_symbols_for.as_deref() == Some(query) {
            return;
        }
        // Language servers pick per-language, so a project with no open file
        // has no server to ask. That is a real limit, and the popup says so
        // rather than showing an empty list that looks like "no matches".
        let Some(path) = self
            .open_files
            .get(self.active_file)
            .map(|file| file.path.clone())
        else {
            return;
        };
        if !self.language.supports(&path) {
            return;
        }
        self.workspace_symbols_for = Some(query.to_owned());
        self.client.send(Request::LspWorkspaceSymbols {
            path,
            root: self.language_root(),
            query: query.to_owned(),
        });
    }

    /// Draws the completion popup under the composer, and returns `true` when
    /// it consumed the frame's Enter — the caller must not also send.
    pub(crate) fn completion_popup(&mut self, ui: &mut Ui) -> bool {
        let Some(token) = self.active_completion.clone() else {
            return false;
        };
        if token.prefix() == Some('#') {
            self.refresh_workspace_symbols(&token.query());
        }
        let suggestions = self.suggestions_for(&token);
        if suggestions.is_empty() {
            // An explanation beats an empty box: "#" with no language server
            // is a different situation from "#" with no matches.
            if token.prefix() == Some('#') && !self.has_symbol_source() {
                ui.label(
                    RichText::new(
                        "Open a file in a language PurrCode has a server for to complete symbols.",
                    )
                    .size(theme::TYPE_META)
                    .color(self.tokens.text_muted),
                );
            }
            return false;
        }

        let tokens = self.tokens;
        self.completion_index = self.completion_index.min(suggestions.len() - 1);
        let (up, down, enter, tab, escape) = ui.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                input.key_pressed(Key::Enter),
                input.consume_key(egui::Modifiers::NONE, Key::Tab),
                input.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            self.completion_index = self.completion_index.saturating_sub(1);
        }
        if down {
            self.completion_index = (self.completion_index + 1).min(suggestions.len() - 1);
        }
        if escape {
            self.active_completion = None;
            return true;
        }

        let mut chosen: Option<Suggestion> = None;
        egui::Frame::new()
            .fill(tokens.background_raised)
            .stroke(egui::Stroke::new(1.0_f32, tokens.border_subtle))
            .corner_radius(theme::RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                for (index, suggestion) in suggestions.iter().enumerate() {
                    let selected = index == self.completion_index;
                    let response = ui.allocate_response(
                        egui::vec2(ui.available_width(), theme::ROW_HEIGHT),
                        egui::Sense::click(),
                    );
                    let rect = response.rect;
                    if selected {
                        ui.painter()
                            .rect_filled(rect, theme::RADIUS_CONTROL, tokens.accent_soft);
                    } else if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, theme::RADIUS_CONTROL, tokens.surface_hover);
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if response.has_focus() {
                        tokens.focus_ring(ui.painter(), rect);
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + 8.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &suggestion.label,
                        egui::FontId::monospace(theme::TYPE_META),
                        tokens.text_primary,
                    );
                    ui.painter().text(
                        egui::pos2(rect.right() - 8.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        &suggestion.detail,
                        egui::FontId::proportional(theme::TYPE_META),
                        tokens.text_muted,
                    );
                    if response.clicked() {
                        chosen = Some(suggestion.clone());
                    }
                }
            });

        // Enter and Tab both accept, which is what every completion surface
        // the user already knows does.
        if (enter || tab) && chosen.is_none() {
            chosen = suggestions.get(self.completion_index).cloned();
        }
        if let Some(suggestion) = chosen {
            self.accept_suggestion(&token, &suggestion);
            self.active_completion = None;
            // The Enter that accepted a completion must not also submit the
            // draft: the user was picking a file, not sending a request.
            return true;
        }
        false
    }

    /// Whether `#symbol` completion has anything to draw on.
    fn has_symbol_source(&self) -> bool {
        self.open_files
            .get(self.active_file)
            .is_some_and(|file| self.language.supports(&file.path))
    }

    /// The row of resolved references under the composer.
    ///
    /// This is the "what will the agent actually be given" surface. A
    /// reference that did not resolve is shown as unresolved with the reason,
    /// never dropped — a typo that silently sends nothing is how a user ends
    /// up believing the agent read a file it never saw.
    pub(crate) fn reference_chips(&mut self, ui: &mut Ui) {
        if self.resolved_references.is_empty() {
            return;
        }
        let tokens = self.tokens;
        let references = self.resolved_references.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for reference in &references {
                let colour = if reference.resolved {
                    tokens.text_secondary
                } else {
                    tokens.status_warning
                };
                let response = egui::Frame::new()
                    .fill(tokens.background_secondary)
                    .corner_radius(theme::RADIUS_PILL)
                    .inner_margin(egui::Margin::symmetric(7, 2))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        crate::icons::inline(
                            ui,
                            if reference.resolved {
                                crate::icons::Glyph::Check
                            } else {
                                crate::icons::Glyph::Warning
                            },
                            9.0,
                            colour,
                        );
                        ui.label(
                            RichText::new(&reference.display)
                                .size(theme::TYPE_EYEBROW)
                                .color(colour),
                        );
                    })
                    .response;
                // The preview is the proof: hovering shows the actual text
                // that will be attached, so "resolved" is checkable rather
                // than something the UI merely asserts.
                let hover = match (&reference.diagnostics, &reference.preview) {
                    (Some(problem), _) => problem.clone(),
                    (None, Some(preview)) => preview.clone(),
                    (None, None) => "Resolved, no preview available.".to_owned(),
                };
                response.on_hover_text(hover);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caret_in_prose_opens_no_completion() {
        // The popup appearing mid-sentence is the failure this guards: the
        // composer is for writing requests, not only for typing references.
        assert!(active_token("please fix the login bug", 10).is_none());
        assert!(active_token("", 0).is_none());
        assert!(active_token("hello", 5).is_none());
    }

    #[test]
    fn a_reference_token_is_found_with_its_span() {
        let text = "look at @src/auth.rs please";
        let token = active_token(text, 19).expect("caret inside the reference");
        assert_eq!(token.text, "@src/auth.rs");
        assert_eq!(&text[token.start..token.end], "@src/auth.rs");
        assert_eq!(token.query(), "src/auth.rs");
    }

    #[test]
    fn a_slash_only_starts_a_command_at_the_start_of_the_draft() {
        // Otherwise every path in a sentence would open the command list.
        assert!(active_token("/compact", 8).is_some());
        assert!(active_token("look in /etc for it", 12).is_none());
    }

    #[test]
    fn a_symbol_token_is_recognised() {
        let token = active_token("check #validate_session now", 20).expect("symbol token");
        assert_eq!(token.prefix(), Some('#'));
        assert_eq!(token.query(), "validate_session");
    }

    #[test]
    fn a_caret_at_the_very_end_of_a_token_still_completes() {
        // The common case: the user has just typed the last character and
        // expects to see suggestions without moving the caret.
        let token = active_token("@src/li", 7).expect("caret at the end");
        assert_eq!(token.text, "@src/li");
    }

    #[test]
    fn a_caret_past_the_end_of_the_text_is_handled() {
        // The caret is tracked separately from the text, so a stale index can
        // outlive an edit that shortened the draft.
        assert!(active_token("@a", 99).is_some());
        assert!(active_token("", 99).is_none());
    }

    #[test]
    fn a_multibyte_draft_does_not_split_a_character() {
        // A caret index landing inside a multi-byte character must not panic
        // or slice mid-character.
        let text = "修复 @src/认证.rs 里的问题";
        for caret in 0..=text.len() {
            let _ = active_token(text, caret);
        }
        let caret = text.find(".rs").expect("the reference is present") + 3;
        let token = active_token(text, caret).expect("caret inside the reference");
        assert_eq!(token.text, "@src/认证.rs");
    }
}
