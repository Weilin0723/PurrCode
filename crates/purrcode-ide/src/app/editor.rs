//! The editor surface: a tab strip, a breadcrumb line, and an editable code
//! editor.
//!
//! egui 0.33 ships no tab widget, so the strip is hand-built: an active tab is
//! the colour of the canvas below it with an accent edge on top, so it reads as
//! *attached* to what it opens, and an inactive tab is the colour of the
//! chrome. The editor itself is `TextEdit::code_editor()` with a hand-drawn
//! line-number gutter and syntect-based syntax highlighting (via `egui_extras`),
//! composed so the gutter scrolls in lockstep with the text (PRD §22-27, §123).

use egui::{Align, Align2, FontId, Layout, Rect, RichText, ScrollArea, Sense, Ui, Vec2};

use super::PurrCodeIde;
use crate::icons::Glyph;
use crate::theme::{self, Tokens};

impl PurrCodeIde {
    /// The editor tab strip. The Agent session occupies the first tab; open
    /// files follow. The active tab is tinted; unsaved files show a dot in
    /// place of their close button.
    pub(crate) fn editor_tabs(&mut self, ui: &mut Ui) {
        let mut activate_agent = false;
        let mut activate_file: Option<usize> = None;
        let mut close_file: Option<usize> = None;

        egui::Frame::new()
            .fill(self.tokens.background_secondary)
            .show(ui, |ui| {
                ui.set_height(theme::TAB_BAR_HEIGHT);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;

                    let agent_active =
                        matches!(self.agent_location, super::AgentLocation::EditorTab);
                    let agent_title = self.session.title();
                    let agent_label = if agent_title == "Untitled session" {
                        "Agent".to_owned()
                    } else {
                        truncate(agent_title, 22)
                    };
                    let (clicked, _) = tab(
                        ui,
                        &self.tokens,
                        Some(TabIcon::Logo),
                        &agent_label,
                        agent_active,
                        self.agent_tab_dirty,
                        false,
                        "agent",
                    );
                    activate_agent |= clicked;

                    for (index, file) in self.open_files.iter().enumerate() {
                        let active = self.active_file == index && !agent_active;
                        let (clicked, closed) = tab(
                            ui,
                            &self.tokens,
                            Some(TabIcon::Glyph(Glyph::File)),
                            &file.label,
                            active,
                            file.modified,
                            true,
                            index,
                        );
                        if clicked {
                            activate_file = Some(index);
                        }
                        if closed {
                            close_file = Some(index);
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(6.0);
                        let aux_open = self.aux_panel.is_some();
                        if self
                            .icon_action(ui, Glyph::PanelRight, "Open the side panel")
                            .clicked()
                        {
                            self.aux_panel = if aux_open {
                                None
                            } else {
                                Some(super::AuxView::Source)
                            };
                        }
                        if self
                            .icon_action(ui, Glyph::Split, "Move the agent beside the code")
                            .clicked()
                        {
                            self.agent_location = match self.agent_location {
                                super::AgentLocation::EditorTab => super::AgentLocation::Aux,
                                _ => super::AgentLocation::EditorTab,
                            };
                            if matches!(self.agent_location, super::AgentLocation::Aux) {
                                self.aux_panel = Some(super::AuxView::Agent);
                            }
                        }
                    });
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom() - 0.5,
            self.tokens.hairline(),
        );

        if activate_agent {
            self.agent_location = super::AgentLocation::EditorTab;
            self.agent_tab_dirty = false;
        }
        if let Some(index) = activate_file {
            self.active_file = index;
            self.agent_location = super::AgentLocation::Aux;
        }
        if let Some(index) = close_file {
            self.close_file(index);
            if self.open_files.is_empty() {
                self.agent_location = super::AgentLocation::EditorTab;
            }
        }
    }

    /// The path line under the tabs.
    ///
    /// It answers "what am I looking at, and where does it live?" without
    /// costing a row of the editor — the same job the tab label cannot do once
    /// two files in different folders share a name.
    pub(crate) fn breadcrumbs(&mut self, ui: &mut Ui) {
        let trail: Vec<String> = match self.agent_location {
            super::AgentLocation::EditorTab | super::AgentLocation::Focus => {
                let mut trail = vec![self.repository_name()];
                if self.selected.is_some() {
                    let title = self.session.title();
                    trail.push(truncate(
                        if title == "Untitled session" {
                            "Agent"
                        } else {
                            title
                        },
                        60,
                    ));
                } else {
                    trail.push("Agent".to_owned());
                }
                trail
            }
            super::AgentLocation::Aux => {
                let Some(file) = self.open_files.get(self.active_file) else {
                    return;
                };
                let relative = file
                    .path
                    .strip_prefix(&self.repository)
                    .unwrap_or(&file.path);
                std::iter::once(self.repository_name())
                    .chain(
                        relative
                            .components()
                            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
                    )
                    .collect()
            }
        };

        egui::Frame::new()
            .fill(self.tokens.background_primary)
            .inner_margin(egui::Margin::symmetric(14, 0))
            .show(ui, |ui| {
                ui.set_height(theme::BREADCRUMB_HEIGHT);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let last = trail.len().saturating_sub(1);
                    for (index, part) in trail.iter().enumerate() {
                        if index > 0 {
                            crate::icons::inline(
                                ui,
                                Glyph::ChevronRight,
                                9.0,
                                self.tokens.text_muted,
                            );
                        }
                        ui.label(RichText::new(part).size(theme::TYPE_META).color(
                            if index == last {
                                self.tokens.text_secondary
                            } else {
                                self.tokens.text_muted
                            },
                        ));
                    }
                });
            });
    }

    /// A full editable code editor with a line-number gutter and syntax
    /// highlighting. The gutter and the text live in one horizontal row inside
    /// the same vertical scroll area so they scroll together.
    pub(crate) fn code_editor(&mut self, ui: &mut Ui, index: usize) {
        let Some(file) = self.open_files.get_mut(index) else {
            return;
        };
        let mut content = match file.body.clone() {
            Ok(content) => content,
            Err(_) => {
                ui.label(RichText::new("Could not read file").color(self.tokens.status_error));
                return;
            }
        };
        let path = file.path.clone();
        let jump_to = file.scroll_to_line;
        let conflicted = file.external_change;
        let mut caret: Option<crate::model::DocumentPosition> = None;
        let mut hovered: Option<crate::model::DocumentPosition> = None;
        let mut follow: Option<crate::model::DocumentPosition> = None;
        let line_count = content.lines().count().max(1);
        let mono = egui::FontId::monospace(theme::TYPE_CODE);
        // The gutter is sized to the widest line number *from the live face*,
        // not from a constant, so a narrow or wide monospace still lines every
        // number up to one right edge.
        let digit_w = ui.fonts_mut(|fonts| fonts.glyph_width(&mono, '0'));
        let gutter_width = 12.0 + digit_w * line_count.to_string().len() as f32;

        // A file that moved on disk while this buffer had unsaved edits is a
        // fork in the file's history. Neither side is discarded automatically:
        // the bar names the situation and the user picks, because silently
        // keeping either one loses work somebody meant to keep.
        if conflicted {
            let mut choice: Option<bool> = None;
            egui::Frame::new()
                .fill(self.tokens.accent_soft)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        crate::icons::inline(ui, Glyph::Warning, 12.0, self.tokens.status_warning);
                        ui.label(
                            RichText::new("This file changed on disk while you had unsaved edits.")
                                .size(theme::TYPE_META)
                                .color(self.tokens.text_primary),
                        );
                        if ui
                            .small_button("Reload from disk")
                            .on_hover_text("Discard the edits in this editor")
                            .clicked()
                        {
                            choice = Some(true);
                        }
                        if ui
                            .small_button("Keep my version")
                            .on_hover_text("Your next save overwrites the file on disk")
                            .clicked()
                        {
                            choice = Some(false);
                        }
                    });
                });
            match choice {
                Some(true) => self.reload_from_disk(index),
                Some(false) => self.keep_editor_version(index),
                None => {}
            }
            // The buffer may have been replaced by the reload, so re-read it
            // rather than drawing the copy taken before the choice.
            if choice.is_some() {
                ui.ctx().request_repaint();
                return;
            }
        }

        // One scroll area wraps a single horizontal row holding both columns,
        // so a single offset drives the numbers and the code together — a
        // gutter that does not follow its lines is confidently wrong.
        ScrollArea::vertical()
            .id_salt("editor_gutter_and_code")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    // Gutter.
                    ui.set_width(gutter_width);
                    ui.set_min_height(ui.available_height());
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for line in 1..=line_count {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(line.to_string())
                                    .monospace()
                                    .color(self.tokens.text_muted),
                            );
                        });
                    }
                    // Code.
                    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                        let text = buf.as_str();
                        let mut job = egui::text::LayoutJob::default();
                        if let Some(language) = language_for(&path) {
                            let theme =
                                egui_extras::syntax_highlighting::CodeTheme::from_style(ui.style());
                            job = egui_extras::syntax_highlighting::highlight(
                                ui.ctx(),
                                ui.style(),
                                &theme,
                                text,
                                &language,
                            );
                        } else {
                            job.append(text, 0.0, egui::TextFormat::default());
                        }
                        job.wrap.max_width = wrap_width;
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    // `show` rather than `add`: the caret and the laid-out
                    // galley are what make hover, go-to-definition and
                    // scroll-to-line possible, and `add` throws both away.
                    let output = egui::TextEdit::multiline(&mut content)
                        .code_editor()
                        .frame(false)
                        .font(egui::TextStyle::Monospace)
                        .id_salt("code_editor")
                        .desired_width(f32::INFINITY)
                        .layouter(&mut layouter)
                        .show(ui);

                    caret = output
                        .state
                        .cursor
                        .char_range()
                        .map(|range| document_position(&content, range.primary.index));

                    // A pointer resting over a token asks the language server
                    // what it is. The position is resolved through the galley
                    // so a wrapped line still maps to the right document
                    // coordinate.
                    if let Some(pointer) = output.response.hover_pos() {
                        let cursor = output.galley.cursor_from_pos(pointer - output.galley_pos);
                        let position = document_position(&content, cursor.index);
                        hovered = Some(position);
                        // ⌘/Ctrl-click is the universal "follow this symbol"
                        // gesture, and the modifier is what keeps it from
                        // stealing an ordinary click that places the caret.
                        if output.response.clicked() && ui.input(|input| input.modifiers.command) {
                            follow = Some(position);
                        }
                    }

                    // Jump requests set a line while the editor was not
                    // drawing; honour them now that there is a galley to
                    // measure against.
                    if let Some(line) = jump_to {
                        let cursor = egui::text::CCursor::new(char_index_of_line(&content, line));
                        let rect = output.galley.pos_from_cursor(cursor);
                        ui.scroll_to_rect(
                            rect.translate(output.galley_pos.to_vec2()),
                            Some(Align::Center),
                        );
                    }
                });
            });

        if let Some(file) = self.open_files.get_mut(index) {
            if file.body.as_ref() != Ok(&content) {
                file.body = Ok(content);
                file.modified = true;
                let path = file.path.clone();
                self.dirty.insert(path);
            }
            // Consume the jump: leaving it set would re-centre the view on
            // every frame and make the editor impossible to scroll.
            if jump_to.is_some() {
                file.scroll_to_line = None;
            }
        }

        self.caret = caret;
        if let Some(position) = caret {
            self.caret_word = word_at(
                self.open_files
                    .get(index)
                    .and_then(|file| file.body.as_ref().ok())
                    .map(String::as_str)
                    .unwrap_or_default(),
                position,
            );
        }
        if let Some(position) = follow {
            self.go_to_definition(&path, position);
        }
        if let Some(position) = hovered {
            self.note_hover(&path, position);
            self.show_hover_card(ui, &path, position);
        } else if self
            .hover_probe
            .as_ref()
            .is_some_and(|probe| probe.path == path)
        {
            // The pointer left the text: drop the probe and its answer rather
            // than leaving a tooltip pinned to a token nobody is pointing at.
            self.hover_probe = None;
            self.language.hover = None;
            self.language.hover_anchor = None;
        }
    }
    /// Write the active file's buffer back to disk. `Ok(())` clears the dirty
    /// dot; a failure raises a notice so the edit is never lost silently.
    ///
    /// The buffer lives in `open_files[index].body`, which `code_editor`
    /// refreshes from the live `TextEdit` at the end of every frame, so a save
    /// issued here writes exactly what the user is looking at.
    pub(crate) fn save_active_file(&mut self) -> bool {
        if self.active_file >= self.open_files.len() {
            return false;
        }
        self.save_file(self.active_file)
    }

    pub(crate) fn save_file(&mut self, index: usize) -> bool {
        let Some(path) = self.open_files.get(index).map(|f| f.path.clone()) else {
            return false;
        };
        let Some(body) = self.open_files[index].body.clone().ok() else {
            return false;
        };
        match std::fs::write(&path, body) {
            Ok(()) => {
                if let Some(file) = self.open_files.get_mut(index) {
                    file.modified = false;
                    // Adopt the stamp this write produced, so the editor's own
                    // save is not reported back as an external change.
                    file.disk_stamp = super::disk_stamp(&path);
                    file.external_change = false;
                }
                self.dirty.remove(&path);
                // The server's copy is now stale; hand it the saved text.
                self.open_in_language_server(&path);
                true
            }
            Err(err) => {
                self.push_notice(crate::app::errors::Notice {
                    kind: crate::app::errors::NoticeKind::Environment,
                    headline: format!("Couldn't save {}", path.display()),
                    impact: "Your edits are still in the editor — nothing was lost.".to_owned(),
                    next_step: Some(
                        "Check the folder's permissions, then try ⌘S again.".to_owned(),
                    ),
                    actions: vec![crate::app::errors::NoticeAction::Dismiss],
                    detail: Some(err.to_string()),
                    clears_on_reconnect: false,
                });
                false
            }
        }
    }
}

/// Draw one tab. Returns `(activated, closed)`.
///
/// A free function rather than a method because it needs the palette while the
/// caller still holds the list of tabs it is iterating. Exposed to the whole
/// `app` module so the terminal strip and the source-panel strip draw the same
/// tab as the editor — three tab strips, one visual language.
#[derive(Clone, Copy)]
pub(crate) enum TabIcon {
    Logo,
    Glyph(Glyph),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tab(
    ui: &mut Ui,
    tokens: &Tokens,
    icon: Option<TabIcon>,
    label: &str,
    active: bool,
    dirty: bool,
    closable: bool,
    id_salt: impl std::hash::Hash,
) -> (bool, bool) {
    let font = FontId::proportional(theme::TYPE_LABEL);
    let text_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(label.to_owned(), font.clone(), tokens.text_primary)
            .size()
            .x
    });
    let icon_width = if icon.is_some() { 23.0 } else { 0.0 };
    let trailing = if closable || dirty { 20.0 } else { 0.0 };
    let width = (12.0 + icon_width + text_width + trailing + 10.0).min(240.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, theme::TAB_BAR_HEIGHT), Sense::click());

    if active {
        // The canvas colour, so the tab and what it opens are one surface.
        ui.painter().rect_filled(rect, 0, tokens.background_primary);
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(rect.width(), 2.0)),
            0,
            tokens.accent_primary,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0, tokens.surface_hover);
    }
    ui.painter().vline(
        rect.right(),
        rect.y_range(),
        egui::Stroke::new(1.0_f32, tokens.border_subtle),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.has_focus() {
        tokens.focus_ring(ui.painter(), rect);
    }

    let mut x = rect.left() + 12.0;
    if let Some(icon) = icon {
        let icon_rect =
            Rect::from_center_size(egui::pos2(x + 8.5, rect.center().y), Vec2::splat(18.0));
        match icon {
            TabIcon::Logo => {
                // The supplied mascot already has a distinctive silhouette;
                // keep it free of an additional tab-coloured tile.
                crate::icons::brand_badge_at(ui, icon_rect);
            }
            TabIcon::Glyph(glyph) => crate::icons::draw(
                ui,
                Rect::from_center_size(icon_rect.center(), Vec2::splat(13.0)),
                glyph,
                if active {
                    tokens.accent_primary
                } else {
                    tokens.text_muted
                },
            ),
        }
        x += icon_width;
    }
    ui.painter().text(
        egui::pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        if active {
            tokens.text_primary
        } else {
            tokens.text_secondary
        },
    );

    let mut closed = false;
    if closable || dirty {
        let spot = Rect::from_center_size(
            egui::pos2(rect.right() - 14.0, rect.center().y),
            Vec2::splat(16.0),
        );
        let close = ui.interact(
            spot,
            ui.id().with(("purrcode_tab_close", &id_salt)),
            if closable {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        // An unsaved file shows a dot until you reach for the close button —
        // then the dot becomes the ×, so the state and the action share one
        // spot instead of competing for two.
        if dirty && !close.hovered() {
            ui.painter()
                .circle_filled(spot.center(), 3.5, tokens.text_secondary);
        } else if closable && (active || response.hovered() || close.hovered()) {
            if close.hovered() {
                ui.painter()
                    .rect_filled(spot, theme::RADIUS_CONTROL, tokens.surface_active);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            crate::icons::draw(
                ui,
                Rect::from_center_size(spot.center(), Vec2::splat(9.0)),
                Glyph::Close,
                tokens.text_secondary,
            );
        }
        closed = close.clicked();
    }

    (response.clicked() && !closed, closed)
}

/// Converts egui's flat character index into LSP's `(line, UTF-16 code unit)`.
///
/// Two mismatches are handled here. egui counts characters from the start of
/// the whole buffer; LSP counts lines and columns. And egui's column is a
/// character count while LSP's is a UTF-16 code-unit count — the two agree on
/// ASCII and diverge the moment a file contains CJK text or an emoji, which
/// would otherwise send the server to the wrong column.
fn document_position(text: &str, index: usize) -> crate::model::DocumentPosition {
    let mut line = 0_u64;
    let mut character = 0_u64;
    for (position, value) in text.chars().enumerate() {
        if position >= index {
            break;
        }
        if value == '\n' {
            line += 1;
            character = 0;
        } else {
            character += value.len_utf16() as u64;
        }
    }
    crate::model::DocumentPosition { line, character }
}

/// The flat character index of the start of a 1-based line, for scrolling.
///
/// A line past the end of the buffer clamps to the last line rather than
/// failing: a diagnostic from a stale analysis should still land the user
/// somewhere sensible in the file it names.
fn char_index_of_line(text: &str, display_line: usize) -> usize {
    let wanted = display_line.saturating_sub(1);
    let mut line = 0_usize;
    let mut line_start = 0_usize;
    for (index, value) in text.chars().enumerate() {
        if line == wanted {
            return line_start;
        }
        if value == '\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    // Past the end of the buffer: the start of the final line.
    line_start
}

/// The identifier at a document position, for labelling a references list.
///
/// Returns an empty string when the position is not on an identifier, so a
/// caller can tell "no word here" from a word it failed to read.
fn word_at(text: &str, position: crate::model::DocumentPosition) -> String {
    let Some(line) = text.split('\n').nth(position.line as usize) else {
        return String::new();
    };
    // Walk in UTF-16 units to match the position's own units.
    let mut units = 0_u64;
    let mut byte_index = line.len();
    for (index, character) in line.char_indices() {
        if units >= position.character {
            byte_index = index;
            break;
        }
        units += character.len_utf16() as u64;
    }
    let is_word = |character: char| character.is_alphanumeric() || character == '_';
    let start = line[..byte_index]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word(*character))
        .last()
        .map_or(byte_index, |(index, _)| index);
    let end = line[byte_index..]
        .char_indices()
        .find(|(_, character)| !is_word(*character))
        .map_or(line.len(), |(index, _)| byte_index + index);
    line[start..end].to_owned()
}

/// Shorten a label for a breadcrumb without cutting a character in half.
fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Map a file path to a syntect language name.
///
/// The extension is handed to the same closed map a fenced code block in the
/// conversation uses, so a file open in this editor and a snippet of that file
/// quoted in a reply are coloured by one list rather than by two that have to
/// be remembered together.
fn language_for(path: &std::path::Path) -> Option<String> {
    crate::markdown::syntect_language(path.extension().and_then(|ext| ext.to_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_breadcrumb_is_left_alone() {
        assert_eq!(truncate("tasks.md", 60), "tasks.md");
        assert_eq!(truncate("  spaced  ", 60), "spaced");
    }

    #[test]
    fn a_file_and_the_same_code_quoted_in_a_reply_resolve_to_one_language() {
        // The editor used to keep its own extension list. If this ever stops
        // delegating, a `.rs` file and a ```rs fence drift apart silently —
        // the same code, two colourings, on one screen.
        for (path, expected) in [
            ("src/lib.rs", Some("rust")),
            ("setup.py", Some("python")),
            ("Cargo.toml", Some("ini")),
            ("main.c", Some("c")),
            ("notes.md", Some("markdown")),
        ] {
            let path = std::path::Path::new(path);
            assert_eq!(
                language_for(path).as_deref(),
                expected,
                "{} should highlight as {expected:?}",
                path.display()
            );
            let extension = path.extension().and_then(|ext| ext.to_str());
            assert_eq!(
                language_for(path),
                crate::markdown::syntect_language(extension)
            );
        }
        // No extension, and an extension nobody mapped, are both "plain".
        assert_eq!(language_for(std::path::Path::new("LICENSE")), None);
        assert_eq!(language_for(std::path::Path::new("a.brainfuck")), None);
    }

    #[test]
    fn a_long_breadcrumb_is_cut_on_a_character_boundary() {
        let long = "写一个很长的中文目标描述用来测试截断行为是否正确";
        let cut = truncate(long, 10);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.ends_with('…'));
    }
}
