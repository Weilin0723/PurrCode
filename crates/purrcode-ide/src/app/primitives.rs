//! The design primitives the settings surface is built from.
//!
//! Settings is where a product's craft is easiest to lose: every page grows one
//! more `ui.horizontal` with one more bare button in it, and the result reads
//! as a form somebody assembled rather than a surface somebody designed. This
//! module is the answer — a section, a labelled field, a list row, a button
//! family and a segmented control, all drawing from `Tokens` so a settings page
//! and the workbench are visibly the same application.
//!
//! The layout rules here are not cosmetic. The "Assign model roles" list put a
//! long model id, a meta line and a right-aligned action group in one
//! `ui.horizontal`, which is a layout with no answer to the question "who gives
//! way when the window is narrow?" — so nobody did, and the id ran under the
//! buttons. `list_row` reserves the action column *before* the identity text is
//! laid out and hands the identity only what is left, with a middle ellipsis
//! when that is not enough. Overlap stops being a bug that gets fixed and
//! becomes a state the code cannot express.

// This module carried a blanket `allow(dead_code)` while the settings pages
// were still being migrated onto it. It does not any more, and it should not
// get one back: a design system nobody calls is indistinguishable from a
// design system that is wrong, and the compiler saying so is the only cheap
// way to find out which one this is.

use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, FontId, InnerResponse, Layout, Rect, Response, RichText, Sense, TextFormat, Ui,
    UiBuilder, Vec2,
};

use crate::theme::{self, Tokens};

// ── Metrics ────────────────────────────────────────────────────────────
//
// One set of numbers for the whole surface. A page that picks its own padding
// is what makes two settings pages look like two products.

/// The breathing room inside a section card.
const SECTION_PAD_X: i8 = 14;
const SECTION_PAD_Y: i8 = 14;
/// Between the section header and its body.
const HEADER_GAP: f32 = 9.0;
/// Between one section and the next.
pub(crate) const SECTION_GAP: f32 = 14.0;

/// The label column of a field row. Fixed, so "API key", "Base URL" and
/// "Model ID" start on the same pixel down the whole page.
pub(crate) const LABEL_COLUMN: f32 = 132.0;
/// Between the label column and the control column.
pub(crate) const COLUMN_GAP: f32 = 12.0;
/// Under this width the two columns stop being two columns: a label crushed
/// into 132px next to a text field crushed into 90px is worse than a label
/// sitting on its own line above a control that can breathe.
pub(crate) const STACK_BREAKPOINT: f32 = 460.0;
/// Between one field row and the next.
const FIELD_GAP: f32 = 8.0;

const ROW_PAD_X: f32 = 10.0;
const ROW_PAD_Y: f32 = 6.0;
/// The clear space between a row's identity text and its action group. Big
/// enough that "…-550b-a55b" and "Assign" read as two things.
pub(crate) const ACTION_GAP: f32 = 16.0;
/// The most of a row an action group may claim before the identity has had its
/// say. Actions still overflow this if the caller packs in more than fits — the
/// measurement is taken from what they actually used, not from this cap.
const ACTION_MAX_SHARE: f32 = 0.62;
/// The identity line and the meta line under it.
const ROW_IDENTITY_LINE: f32 = 20.0;
const ROW_META_LINE: f32 = 15.0;

const BUTTON_HEIGHT: f32 = 28.0;
/// Horizontal padding for Secondary, Quiet and Danger buttons. At 12px the
/// label and its border nearly touch; a hair more air makes the label sit
/// comfortably in its frame without any of the tones having to shout.
const BUTTON_PAD_X: f32 = 14.0;
const SEGMENT_HEIGHT: f32 = 26.0;
const SEGMENT_PAD_X: f32 = 12.0;
/// The gap between the segmented track's edge and the segments inside it.
const TRACK_INSET: f32 = 2.0;

// ── Text fitting ───────────────────────────────────────────────────────

/// Shorten `text` to `max_chars` by dropping the middle, not the tail.
///
/// Identifiers are the wrong shape for a trailing ellipsis. Cutting
/// `integrate.api.nvidia.com/nvidia/nemotron-3-ultra-550b-a55b` from the right
/// leaves every model on a host looking identical; keeping both ends leaves the
/// host *and* the model name, which is what the user was reading it for.
///
/// The budget counts characters, never bytes: the product ships a Chinese
/// interface, and slicing a `&str` at a byte offset would take the window down
/// on the first multi-byte label.
pub(crate) fn truncate_middle(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_owned();
    }
    // The tail gets the odd character: the end of an id is the part that says
    // *which* one this is.
    let budget = max_chars - 1;
    let head = budget / 2;
    let tail = budget - head;
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[count - tail..]);
    out
}

/// Lay `text` out on one line inside `max_width`, dropping the middle if it
/// does not fit. Returns the galley and whether anything was dropped, so the
/// caller can offer the full string on hover.
///
/// The character budget is found by search rather than by dividing the width by
/// an average glyph — proportional and CJK text have no useful average, and
/// guessing high is how text ends up painted over its neighbour.
fn fit_middle(
    ui: &Ui,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
) -> (Arc<egui::Galley>, bool) {
    let painter = ui.painter();
    let measure = |candidate: String| painter.layout_no_wrap(candidate, font.clone(), color);

    let full = measure(text.to_owned());
    if full.size().x <= max_width {
        return (full, false);
    }

    let (mut low, mut high) = (0_usize, text.chars().count());
    while low < high {
        let mid = (low + high).div_ceil(2);
        if measure(truncate_middle(text, mid)).size().x <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    (measure(truncate_middle(text, low)), true)
}

/// Lay prose out on one line, elided at the end.
///
/// Meta text is a sentence, not an identifier — the beginning carries the
/// meaning, so this is the one place a trailing ellipsis is the right cut.
///
/// Exposed to the rest of `app` because a surface that paints its own text
/// rather than composing it from widgets — the settings navigation pill — needs
/// the same galley, and a second copy of these four wrap settings is how two
/// parts of one column end up eliding differently.
pub(crate) fn fit_tail(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

// ── Section ────────────────────────────────────────────────────────────

/// A titled card. Everything on a settings page lives in one of these.
///
/// Hierarchy is the whole job: a flat run of labels and controls gives the eye
/// nothing to group by, so a page of twelve settings reads as twelve unrelated
/// decisions instead of three related ones.
pub(crate) fn section<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    title: &str,
    detail: Option<&str>,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let result = tokens
        .card()
        .inner_margin(egui::Margin::symmetric(SECTION_PAD_X, SECTION_PAD_Y))
        .show(ui, |ui| {
            // Cards run the full width of the column. A card that shrink-wraps
            // its widest control turns a page into a ragged staircase.
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(title)
                    .size(theme::TYPE_LABEL)
                    .strong()
                    .color(tokens.text_primary),
            );
            if let Some(detail) = detail {
                // Two points of air is deliberately tighter than any page gap:
                // a section's title and its detail sentence are one object.
                ui.add_space(2.0);
                ui.label(
                    RichText::new(detail)
                        .size(theme::TYPE_META)
                        .line_height(Some(theme::META_LINE_HEIGHT))
                        .color(tokens.text_muted),
                );
            }
            ui.add_space(HEADER_GAP);
            body(ui)
        })
        .inner;
    ui.add_space(SECTION_GAP);
    result
}

// ── Field row ──────────────────────────────────────────────────────────

/// How a labelled control row resolves at a given width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FieldLayout {
    /// Label left, control right, label column fixed.
    Columns { label: f32, control: f32 },
    /// Label above control, each on its own full-width line.
    Stacked,
}

/// Decide the shape of a field row for `row_width`.
///
/// Split out from the renderer because this is the part with a right answer,
/// and the part a test can hold to it.
pub(crate) fn field_layout(row_width: f32) -> FieldLayout {
    if row_width < STACK_BREAKPOINT {
        return FieldLayout::Stacked;
    }
    FieldLayout::Columns {
        label: LABEL_COLUMN,
        control: (row_width - LABEL_COLUMN - COLUMN_GAP).max(0.0),
    }
}

/// A labelled control row.
///
/// The label column is allocated at a fixed width rather than sized to the
/// text, so a page of fields has one left edge for its labels and one for its
/// controls. A label too long for the column is elided instead of being allowed
/// to shove the control sideways — one wordy label must not misalign the page.
pub(crate) fn field_row<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    label: &str,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let result = match field_layout(ui.available_width()) {
        FieldLayout::Stacked => {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(label)
                        .size(theme::TYPE_LABEL)
                        .color(tokens.text_secondary),
                );
                ui.add_space(2.0);
                body(ui)
            })
            .inner
        }
        FieldLayout::Columns {
            label: label_width,
            control,
        } => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(label_width, ui.spacing().interact_size.y),
                    Sense::hover(),
                );
                // Painted rather than `ui.label`d so the column keeps its width
                // whatever the text does. The label sits at the top of a tall
                // control (a multi-line editor) instead of floating beside its
                // middle, which is where a reader looks for it.
                let galley = fit_tail(
                    ui,
                    label,
                    FontId::proportional(theme::TYPE_LABEL),
                    tokens.text_secondary,
                    rect.width(),
                );
                ui.painter().galley(
                    egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
                    galley,
                    tokens.text_secondary,
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(control, ui.spacing().interact_size.y),
                    Layout::left_to_right(Align::Center),
                    body,
                )
                .inner
            })
            .inner
        }
    };
    ui.add_space(FIELD_GAP);
    result
}

// ── List row ───────────────────────────────────────────────────────────

/// What a list row says about the thing it lists.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RowSpec<'a> {
    /// The name of the thing — a model id, a provider, a workspace path.
    pub identity: &'a str,
    /// One quiet line under it: scope, state, assigned roles.
    pub meta: Option<&'a str>,
    /// A leading dot in a status colour, for rows that are special (the
    /// default model, a failing provider). Colour is never the only signal —
    /// the meta line says the same thing in words.
    pub marker: Option<Color32>,
}

impl<'a> RowSpec<'a> {
    pub(crate) fn new(identity: &'a str) -> Self {
        Self {
            identity,
            ..Self::default()
        }
    }

    pub(crate) fn meta(mut self, meta: &'a str) -> Self {
        self.meta = Some(meta);
        self
    }

    pub(crate) fn marker(mut self, color: Color32) -> Self {
        self.marker = Some(color);
        self
    }
}

/// How a row's width is divided between what it names and what it offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RowColumns {
    /// The width the identity text may use.
    pub identity: f32,
    /// The width the action group actually occupies.
    pub actions: f32,
}

impl RowColumns {
    /// Divide `row_width` once the action group has been measured.
    ///
    /// The actions are already on screen when this runs, so they cannot be
    /// asked to shrink — the identity takes the whole cost of a narrow window,
    /// down to zero. Zero is a row with no visible name, which is bad; the
    /// alternative is a name painted through the buttons, which is a defect.
    pub(crate) fn solve(row_width: f32, measured_actions: f32, gap: f32) -> Self {
        let row_width = row_width.max(0.0);
        let actions = measured_actions.clamp(0.0, row_width);
        Self {
            identity: (row_width - actions - gap.max(0.0)).max(0.0),
            actions,
        }
    }
}

/// The height of a list row, before anything is drawn in it.
///
/// Fixed up front so both columns can be centred against the same rect. Letting
/// the row size itself to its tallest child is what forces a two-pass layout,
/// and a two-pass layout is what tempted the original code into laying the
/// identity out before it knew what the actions needed.
pub(crate) fn row_height(has_meta: bool) -> f32 {
    let text = ROW_IDENTITY_LINE + if has_meta { ROW_META_LINE } else { 0.0 };
    (text + ROW_PAD_Y * 2.0).max(BUTTON_HEIGHT + ROW_PAD_Y * 2.0)
}

/// One row of a settings list: identity on the left, actions on the right.
///
/// The action group is laid out first, into a `Ui` anchored to the right edge
/// that allocates nothing in the parent. Whatever it used is then subtracted,
/// and the identity is fitted into the remainder. Nothing here can overlap at
/// any window width, because the identity is never offered space the actions
/// have taken.
pub(crate) fn list_row<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    spec: RowSpec<'_>,
    actions: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let height = row_height(spec.meta.is_some());
    let width = ui.available_width().max(0.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, theme::RADIUS_CONTROL, tokens.surface_hover);
    }

    let inner = Rect::from_min_max(
        egui::pos2(rect.left() + ROW_PAD_X, rect.top() + ROW_PAD_Y),
        egui::pos2(
            (rect.right() - ROW_PAD_X).max(rect.left() + ROW_PAD_X),
            (rect.bottom() - ROW_PAD_Y).max(rect.top() + ROW_PAD_Y),
        ),
    );

    // `new_child` rather than `scope_builder`: this child must not advance the
    // parent's cursor, because the identity is drawn into the same row after
    // it. The row itself was already allocated above.
    let action_area = Rect::from_min_max(
        egui::pos2(
            inner.right() - inner.width() * ACTION_MAX_SHARE,
            inner.top(),
        ),
        inner.max,
    );
    let mut column = ui.new_child(
        UiBuilder::new()
            .max_rect(action_area)
            .layout(Layout::right_to_left(Align::Center))
            .id_salt(spec.identity),
    );
    // The default egui row spacing packs Inspect/Test/Remove into a run where
    // the labels nearly touch. A settings action group is one decision with
    // several buttons; six points of air keeps them readable as separate
    // presses while staying tighter than the 8pt field row default.
    column.spacing_mut().item_spacing.x = 6.0;
    let produced = actions(&mut column);
    let used = column.min_rect();

    let columns = RowColumns::solve(
        inner.width(),
        (inner.right() - used.left()).max(0.0),
        ACTION_GAP,
    );

    // The marker, then the text block, both inside the identity column.
    let mut text_left = inner.left();
    if let Some(marker) = spec.marker {
        ui.painter().circle_filled(
            egui::pos2(inner.left() + 3.0, inner.center().y),
            3.0,
            marker,
        );
        text_left += 12.0;
    }
    let text_width = (columns.identity - (text_left - inner.left())).max(0.0);

    let (identity, elided) = fit_middle(
        ui,
        spec.identity,
        &FontId::proportional(theme::TYPE_BODY),
        tokens.text_primary,
        text_width,
    );
    let meta = spec.meta.map(|meta| {
        fit_tail(
            ui,
            meta,
            FontId::proportional(theme::TYPE_META),
            tokens.text_muted,
            text_width,
        )
    });

    let block = identity.size().y + meta.as_ref().map_or(0.0, |m| m.size().y + 2.0);
    let top = inner.top() + ((inner.height() - block) * 0.5).max(0.0);
    let identity_height = identity.size().y;
    ui.painter()
        .galley(egui::pos2(text_left, top), identity, tokens.text_primary);
    if let Some(meta) = meta {
        ui.painter().galley(
            egui::pos2(text_left, top + identity_height + 2.0),
            meta,
            tokens.text_muted,
        );
    }

    // A shortened id is still readable — it is just not on screen. The full
    // string is one hover away rather than lost.
    let response = if elided {
        response.on_hover_text(spec.identity)
    } else {
        response
    };
    InnerResponse::new(produced, response)
}

// ── Buttons ────────────────────────────────────────────────────────────

/// How loudly a button asks to be pressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    /// The one action the surface is for. At most one per section.
    Primary,
    /// A real action that is not *the* action.
    Secondary,
    /// A minor action that should not draw the eye until it is looked for.
    Quiet,
    /// Removes something. Tinted, never filled: a solid red button reads as an
    /// alarm going off rather than as an option that exists.
    Danger,
}

/// A token-styled button.
///
/// Hand-painted because `egui::Button`'s fill is one colour for every state —
/// styling it through the widget API buys a button that does not respond to the
/// pointer, which is the one thing a button has to do.
pub(crate) fn button(ui: &mut Ui, tokens: &Tokens, tone: Tone, label: &str) -> Response {
    button_enabled(ui, tokens, tone, label, true)
}

/// A token-styled button that may be unavailable.
///
/// A disabled button stays on screen instead of disappearing: the user needs to
/// see that the action exists and is not currently possible, which is why the
/// caller pairs this with a sentence saying what is missing.
pub(crate) fn button_enabled(
    ui: &mut Ui,
    tokens: &Tokens,
    tone: Tone,
    label: &str,
    enabled: bool,
) -> Response {
    let font = FontId::proportional(theme::TYPE_LABEL);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let size = Vec2::new(
        galley.size().x + BUTTON_PAD_X * 2.0,
        BUTTON_HEIGHT.max(galley.size().y + 8.0),
    );
    // Hover sense only when disabled, so the row still gets a tooltip but the
    // pointer never suggests a press that does nothing.
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let hovered = enabled && response.hovered();
    let pressed = enabled && response.is_pointer_button_down_on();
    let (fill, border, text) = if !enabled {
        (
            tokens.background_secondary,
            tokens.border_subtle,
            tokens.text_muted,
        )
    } else {
        match tone {
            Tone::Primary => (
                if pressed || hovered {
                    tokens.accent_hover
                } else {
                    tokens.accent_primary
                },
                Color32::TRANSPARENT,
                tokens.accent_on,
            ),
            Tone::Secondary => (
                if pressed {
                    tokens.surface_active
                } else if hovered {
                    tokens.surface_hover
                } else {
                    tokens.background_raised
                },
                // A resting Secondary button is a card-coloured control with a
                // hairline; on hover the hairline is promoted to the strong
                // border so the affordance is visible even though the fill
                // shift is deliberately quiet.
                if hovered || pressed {
                    tokens.accent_primary
                } else {
                    tokens.border_subtle
                },
                tokens.text_primary,
            ),
            Tone::Quiet => (
                if pressed {
                    tokens.surface_active
                } else if hovered {
                    tokens.surface_hover
                } else {
                    Color32::TRANSPARENT
                },
                Color32::TRANSPARENT,
                if hovered {
                    tokens.text_primary
                } else {
                    tokens.text_secondary
                },
            ),
            Tone::Danger => (
                if pressed || hovered {
                    theme::mix(tokens.background_primary, tokens.status_error, 0.28)
                } else {
                    tokens.tint(tokens.status_error)
                },
                tokens.status_error,
                tokens.status_error,
            ),
        }
    };

    let painter = ui.painter();
    painter.rect_filled(rect, theme::RADIUS_CONTROL, fill);
    if border != Color32::TRANSPARENT {
        painter.rect_stroke(
            rect,
            theme::RADIUS_CONTROL,
            egui::Stroke::new(1.0_f32, border),
            egui::StrokeKind::Inside,
        );
    }
    // Keyboard focus must be visible, not implied (PRD §27). egui gives a
    // click-sensed rect focus of its own; the ring is what makes it usable.
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(1.0),
            theme::RADIUS_CONTROL,
            egui::Stroke::new(2.0_f32, tokens.accent_primary),
            egui::StrokeKind::Outside,
        );
    }
    painter.galley(rect.center() - galley.size() * 0.5, galley, text);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

// ── Segmented control ──────────────────────────────────────────────────

/// Fit segments into `available`, shrinking them together when they do not.
///
/// Segments share a track, so they shrink proportionally or not at all: one
/// segment giving way while its neighbour keeps its width is what makes a
/// segmented control look broken rather than tight.
pub(crate) fn segment_widths(natural: &[f32], available: f32) -> Vec<f32> {
    let total: f32 = natural.iter().sum();
    if total <= available || total <= 0.0 {
        return natural.to_vec();
    }
    let scale = (available.max(0.0) / total).clamp(0.0, 1.0);
    natural.iter().map(|width| width * scale).collect()
}

/// A pill segmented control, for a choice between a handful of options.
///
/// Four options in a dropdown hide three of them behind a click and tell the
/// user nothing about what they are choosing between. Laid out side by side,
/// the choice explains itself. Returns `true` when the selection changed.
pub(crate) fn segmented<T: Clone + PartialEq>(
    ui: &mut Ui,
    tokens: &Tokens,
    options: &[(T, &str)],
    selected: &mut T,
) -> bool {
    if options.is_empty() {
        return false;
    }
    let font = FontId::proportional(theme::TYPE_LABEL);
    let natural: Vec<f32> = options
        .iter()
        .map(|(_, label)| {
            ui.painter()
                .layout_no_wrap((*label).to_owned(), font.clone(), tokens.text_secondary)
                .size()
                .x
                + SEGMENT_PAD_X * 2.0
        })
        .collect();
    let room = (ui.available_width() - TRACK_INSET * 2.0).max(0.0);
    let widths = segment_widths(&natural, room);

    let track_width = widths.iter().sum::<f32>() + TRACK_INSET * 2.0;
    let (track, _) = ui.allocate_exact_size(
        Vec2::new(track_width, SEGMENT_HEIGHT + TRACK_INSET * 2.0),
        Sense::hover(),
    );
    let painter = ui.painter().clone();
    painter.rect_filled(track, theme::RADIUS_PILL, tokens.background_secondary);
    painter.rect_stroke(
        track,
        theme::RADIUS_PILL,
        tokens.hairline(),
        egui::StrokeKind::Inside,
    );

    let mut changed = false;
    let mut left = track.left() + TRACK_INSET;
    for (index, ((value, label), width)) in options.iter().zip(&widths).enumerate() {
        let rect = Rect::from_min_size(
            egui::pos2(left, track.top() + TRACK_INSET),
            Vec2::new(*width, SEGMENT_HEIGHT),
        );
        left += width;
        let response = ui.interact(rect, ui.id().with(("segment", index)), Sense::click());
        let active = *selected == *value;

        // The selection is a tinted pill with an accent hairline, not a solid
        // accent fill: a filled segment reads as a button that was pressed and
        // is still going, and a segmented control has no such state.
        if active {
            painter.rect_filled(rect, theme::RADIUS_PILL, tokens.accent_soft);
            painter.rect_stroke(
                rect,
                theme::RADIUS_PILL,
                egui::Stroke::new(1.0_f32, tokens.accent_primary),
                egui::StrokeKind::Inside,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, theme::RADIUS_PILL, tokens.surface_hover);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let color = if active {
            tokens.text_primary
        } else {
            tokens.text_secondary
        };
        let galley = fit_tail(ui, label, font.clone(), color, rect.width() - 6.0);
        painter.galley(rect.center() - galley.size() * 0.5, galley, color);

        if response.clicked() && !active {
            *selected = value.clone();
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Appearance;

    // ── truncate_middle ────────────────────────────────────────────────

    #[test]
    fn a_model_id_that_fits_is_returned_untouched() {
        assert_eq!(truncate_middle("gpt-4o", 40), "gpt-4o");
        assert_eq!(truncate_middle("", 10), "");
        // Exactly at the budget is still a fit.
        assert_eq!(truncate_middle("abcde", 5), "abcde");
    }

    #[test]
    fn a_long_model_id_keeps_its_host_and_its_name() {
        let id = "integrate.api.nvidia.com/nvidia/nemotron-3-ultra-550b-a55b";
        let cut = truncate_middle(id, 30);
        assert_eq!(cut.chars().count(), 30);
        assert_eq!(cut.matches('…').count(), 1);
        assert!(cut.starts_with("integrate"), "the head is gone: {cut}");
        assert!(cut.ends_with("550b-a55b"), "the tail is gone: {cut}");
    }

    #[test]
    fn truncation_never_exceeds_its_budget_at_any_length() {
        let id = "provider.example.com/org/model-name-with-a-very-long-suffix";
        for budget in 0..=id.chars().count() + 4 {
            let cut = truncate_middle(id, budget);
            assert!(
                cut.chars().count() <= budget,
                "budget {budget} produced {} characters",
                cut.chars().count()
            );
        }
    }

    #[test]
    fn a_budget_of_almost_nothing_degrades_instead_of_panicking() {
        assert_eq!(truncate_middle("abcdef", 0), "");
        assert_eq!(truncate_middle("abcdef", 1), "…");
        assert_eq!(truncate_middle("abcdef", 2), "…f");
        assert_eq!(truncate_middle("abcdef", 3), "a…f");
    }

    #[test]
    fn chinese_labels_are_cut_on_character_boundaries() {
        // The product ships a Chinese interface. Slicing this on a byte offset
        // is a panic, not a rendering glitch.
        let text = "配置模型角色与提供方的默认设置项";
        for budget in 0..=text.chars().count() {
            let cut = truncate_middle(text, budget);
            assert!(cut.chars().count() <= budget);
            // Round-tripping proves every char survived whole.
            assert_eq!(cut, String::from_utf8(cut.clone().into_bytes()).unwrap());
        }
        let cut = truncate_middle(text, 7);
        assert_eq!(cut.chars().count(), 7);
        assert!(cut.starts_with('配'));
        assert!(cut.ends_with('项'));
    }

    #[test]
    fn mixed_scripts_and_emoji_survive_the_middle_cut() {
        let text = "模型 nemotron-3-ultra 🐱 550b";
        let cut = truncate_middle(text, 12);
        assert_eq!(cut.chars().count(), 12);
        assert!(cut.contains('…'));
    }

    // ── list row geometry ──────────────────────────────────────────────

    #[test]
    fn the_identity_column_never_reaches_into_the_action_column() {
        // The defect this module exists for: a 58-character model id and a
        // 260px action group in a 720px row.
        let columns = RowColumns::solve(720.0, 260.0, ACTION_GAP);
        assert!(columns.identity > 0.0);
        assert_eq!(columns.actions, 260.0);
        assert!(
            columns.identity + ACTION_GAP + columns.actions <= 720.0,
            "identity {} runs under the actions",
            columns.identity
        );
    }

    #[test]
    fn a_real_gap_always_separates_the_two_columns() {
        for row in [320.0_f32, 480.0, 720.0, 1280.0] {
            for actions in [80.0_f32, 200.0, 260.0] {
                let columns = RowColumns::solve(row, actions, ACTION_GAP);
                if columns.identity > 0.0 {
                    assert!(columns.identity + ACTION_GAP + columns.actions <= row + f32::EPSILON);
                }
            }
        }
    }

    #[test]
    fn a_row_narrower_than_its_actions_still_yields_a_usable_width() {
        // The window is dragged to a sliver. The identity gives way to nothing,
        // but it must never be handed a negative width — egui debug-asserts on
        // one, so this is the difference between a cramped row and a crash.
        let columns = RowColumns::solve(120.0, 400.0, ACTION_GAP);
        assert_eq!(columns.identity, 0.0);
        assert!(columns.actions >= 0.0 && columns.actions <= 120.0);

        let degenerate = RowColumns::solve(0.0, 0.0, ACTION_GAP);
        assert_eq!(degenerate.identity, 0.0);
        assert_eq!(degenerate.actions, 0.0);

        let nonsense = RowColumns::solve(-40.0, -10.0, -5.0);
        assert!(nonsense.identity >= 0.0 && nonsense.actions >= 0.0);
    }

    #[test]
    fn a_row_with_no_actions_gives_the_identity_everything_but_the_gap() {
        let columns = RowColumns::solve(600.0, 0.0, ACTION_GAP);
        assert_eq!(columns.actions, 0.0);
        assert_eq!(columns.identity, 600.0 - ACTION_GAP);
    }

    #[test]
    fn a_meta_line_makes_a_row_taller_and_both_clear_the_buttons_in_it() {
        assert!(row_height(true) > row_height(false));
        assert!(row_height(false) >= BUTTON_HEIGHT + ROW_PAD_Y * 2.0);
        assert!(row_height(true) >= ROW_IDENTITY_LINE + ROW_META_LINE);
    }

    // ── field row ──────────────────────────────────────────────────────

    #[test]
    fn labels_share_one_column_at_every_comfortable_width() {
        for width in [STACK_BREAKPOINT, 700.0, 1100.0, 1600.0] {
            match field_layout(width) {
                FieldLayout::Columns { label, control } => {
                    assert_eq!(label, LABEL_COLUMN, "the label column must not drift");
                    assert!(control > 0.0);
                    assert!(label + COLUMN_GAP + control <= width + f32::EPSILON);
                }
                FieldLayout::Stacked => panic!("{width} is wide enough for two columns"),
            }
        }
    }

    #[test]
    fn a_narrow_panel_stacks_the_label_over_its_control() {
        for width in [0.0, 180.0, 320.0, STACK_BREAKPOINT - 1.0] {
            assert_eq!(
                field_layout(width),
                FieldLayout::Stacked,
                "{width} should stack rather than crush both columns"
            );
        }
    }

    // ── segmented control ──────────────────────────────────────────────

    #[test]
    fn segments_keep_their_natural_width_when_the_track_fits() {
        let natural = [70.0, 64.0, 110.0];
        assert_eq!(segment_widths(&natural, 400.0), natural.to_vec());
        assert!(segment_widths(&[], 400.0).is_empty());
    }

    #[test]
    fn segments_shrink_together_rather_than_spilling_out_of_the_track() {
        let natural = [70.0, 64.0, 110.0];
        let fitted = segment_widths(&natural, 120.0);
        assert!(fitted.iter().sum::<f32>() <= 120.0 + f32::EPSILON);
        // Proportions hold: the widest segment is still the widest.
        assert!(fitted[2] > fitted[0] && fitted[0] > fitted[1]);
        // A track with no room degrades to nothing instead of to a negative.
        assert!(segment_widths(&natural, 0.0).iter().all(|w| *w >= 0.0));
    }

    // ── the primitives in a real frame ─────────────────────────────────

    #[test]
    fn every_primitive_lays_out_at_a_width_that_fits_nothing() {
        // Not a painting test: egui debug-asserts on a negative allocation, so
        // running one frame at a hostile width is what proves the arithmetic
        // above never hands it one. 240px is narrower than the action group of
        // the model-role row it was written for.
        let ctx = egui::Context::default();
        theme::install(&ctx, Appearance::Dark);
        let tokens = Tokens::for_appearance(Appearance::Dark);
        let long = "integrate.api.nvidia.com/nvidia/nemotron-3-ultra-550b-a55b";

        for width in [240.0_f32, 460.0, 900.0] {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(width, 700.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    section(ui, &tokens, "Providers", Some("Where models run"), |ui| {
                        field_row(ui, &tokens, "Base URL", |ui| {
                            let mut text = String::from("http://localhost:11434");
                            ui.text_edit_singleline(&mut text);
                        });
                        let mut mode = "remote";
                        segmented(
                            ui,
                            &tokens,
                            &[("local", "Local"), ("remote", "Remote")],
                            &mut mode,
                        );
                        list_row(
                            ui,
                            &tokens,
                            RowSpec::new(long)
                                .meta("Remote · No assigned role")
                                .marker(tokens.status_success),
                            |ui| {
                                button(ui, &tokens, Tone::Secondary, "Make default");
                                button_enabled(ui, &tokens, Tone::Primary, "Assign", false);
                                button(ui, &tokens, Tone::Danger, "Remove");
                                button(ui, &tokens, Tone::Quiet, "Details");
                            },
                        );
                    });
                });
            });
        }
    }

    #[test]
    fn the_action_group_is_measured_where_it_is_actually_drawn() {
        // `RowColumns` only protects the identity if the width it is given is
        // the width the buttons really took. This runs the model-role row for
        // real and checks the measurement against the row it was drawn in: the
        // group ends at the row's right padding, and the identity column stops
        // a full gap short of where the group starts.
        let ctx = egui::Context::default();
        theme::install(&ctx, Appearance::Dark);
        let tokens = Tokens::for_appearance(Appearance::Dark);
        let long = "integrate.api.nvidia.com/nvidia/nemotron-3-ultra-550b-a55b";

        for width in [520.0_f32, 760.0, 1200.0] {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(width, 700.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let row = list_row(
                        ui,
                        &tokens,
                        RowSpec::new(long).meta("Remote · No assigned role"),
                        |ui| {
                            button(ui, &tokens, Tone::Secondary, "Make default");
                            button(ui, &tokens, Tone::Primary, "Assign");
                            ui.min_rect()
                        },
                    );
                    let actions = row.inner;
                    let outer = row.response.rect;

                    assert!(
                        actions.width() > 0.0,
                        "the action group was not measured at {width}px"
                    );
                    assert!(
                        (actions.right() - (outer.right() - ROW_PAD_X)).abs() <= 1.0,
                        "the action group is not anchored to the row's right edge"
                    );

                    let inner_width = outer.width() - ROW_PAD_X * 2.0;
                    let columns = RowColumns::solve(
                        inner_width,
                        outer.right() - ROW_PAD_X - actions.left(),
                        ACTION_GAP,
                    );
                    let identity_right = outer.left() + ROW_PAD_X + columns.identity;
                    assert!(
                        identity_right <= actions.left() - ACTION_GAP + 1.0,
                        "at {width}px the identity reaches {identity_right} but the actions \
                         start at {}",
                        actions.left()
                    );
                });
            });
        }
    }
}
