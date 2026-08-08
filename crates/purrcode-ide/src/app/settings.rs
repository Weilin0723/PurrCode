//! Product settings with progressive disclosure.
//!
//! The IDE exposes daemon-owned controls without creating a second model or
//! permission state. Provider secrets are deliberately absent: this surface
//! accepts only a credential reference name that was stored through the
//! `purrcode credential set` CLI flow.
//!
//! Defect A (PRD §1) makes this surface honest about where its data comes from:
//! every page lists the daemon queries it renders (FR-A1), every mutation
//! renders its daemon error inline next to the control that produced it
//! (FR-A8), and the search field matches controls, not just page labels
//! (FR-A7).
//!
//! Every page is composed from `super::primitives` rather than from bare egui
//! widgets. That is not a tidying pass: a page assembled out of `ui.button`,
//! `ui.small_button` and `ComboBox` gives each control a different weight and
//! none of them the product's accent, and a row that lays its identity out
//! before it knows what its buttons need is a row whose text runs under those
//! buttons. Sections give the eye something to group by, `field_row` gives the
//! labels one left edge, and `list_row` makes the overlap unrepresentable.

use std::collections::BTreeMap;

use egui::{Align, Color32, FontId, Layout, Response, RichText, ScrollArea, Sense, Ui, Vec2};
use serde_json::Value;

use super::PurrCodeIde;
use super::primitives::{self, RowSpec, Tone};
use crate::daemon::Request;
use crate::theme::{self, Tokens};

const SETTINGS_DEFAULT_WIDTH: f32 = 920.0;
const SETTINGS_DEFAULT_HEIGHT: f32 = 680.0;
const SETTINGS_MIN_WIDTH: f32 = 560.0;
const SETTINGS_MIN_HEIGHT: f32 = 460.0;
const SETTINGS_COMPACT_WIDTH: f32 = 760.0;
const SETTINGS_NAV_MIN_WIDTH: f32 = 190.0;
const SETTINGS_NAV_MAX_WIDTH: f32 = 232.0;
const SETTINGS_COLUMN_GAP: f32 = 12.0;

const SETTINGS_SEARCH_HEIGHT: f32 = 30.0;
/// The gap between the eyebrow and the title inside the header lockup. The
/// lockup is one composed object, so the step from tracked caps to title is
/// tighter than a block gap and reads as belonging together.
const LOCKUP_GAP: f32 = 4.0;

/// A navigation entry's hit target and the inset its text and pill share.
const NAV_ITEM_HEIGHT: f32 = 28.0;
const NAV_ITEM_PAD_X: f32 = 10.0;

// ── The spacing scale ──────────────────────────────────────────────────
//
// Four steps for the whole surface, plus `primitives::SECTION_GAP` between
// cards. Eleven pages each reaching for the number that looked right in
// isolation is precisely what made this surface read as eleven forms; a page
// that needs a fifth value needs a section instead.

/// Between a control and the sentence that qualifies it.
const GAP_TIGHT: f32 = 5.0;
/// Between two controls that belong to the same decision.
const GAP_CONTROL: f32 = 8.0;
/// Between one group of controls inside a section and the next.
const GAP_GROUP: f32 = 10.0;
/// Between a page's heading and the band under it, and around the header bar.
const GAP_BLOCK: f32 = 14.0;

/// The role picker's width.
///
/// Fixed so the model-role rows all reserve the same action column: a picker
/// that sized itself to the longest role name would make one row's identity
/// shorter than its neighbour's for no reason the reader can see.
const ROLE_PICKER_WIDTH: f32 = 132.0;

/// How far a note under a list row is indented.
///
/// It matches the row's own left padding, so the note starts under the identity
/// it belongs to. A note flush with the card edge reads as a new item.
const ROW_NOTE_INDENT: f32 = 10.0;

/// The roles the daemon accepts for `AssignModelRole`. The bootstrap's
/// `control_capabilities` block does not yet carry a role list, so the IDE
/// offers the canonical daemon set (FR-A2) rather than only `coding_worker`.
const MODEL_ROLES: &[&str] = &[
    "coding_worker",
    "judge",
    "planner",
    "reviewer",
    "summarizer",
    "utility",
    "embedding",
];

/// Everything the Settings window has fetched from the daemon, plus the inline
/// mutation errors (FR-A8). Each field corresponds to one dedicated daemon
/// query; a page never renders a control whose backing value was not fetched
/// (FR-A1).
#[derive(Default)]
pub(crate) struct SettingsState {
    // ── Models & providers ────────────────────────────────────────────
    pub providers: Vec<Value>,
    pub provider: Value,
    pub provider_detail: Option<String>,
    pub provider_test: Value,
    pub discovered: Value,
    // ── Local models ──────────────────────────────────────────────────
    pub local_models: Value,
    pub recommendations: Value,
    pub qualification: Value,
    pub unload: Value,
    pub local_settings: Value,
    pub pull_proposal: Value,
    pub pull_approved: Value,
    pub pull_started: Value,
    pub pull_progress: Value,
    pub pull_cancelled: Value,
    pub pull_action_id: Option<String>,
    pub pull_session_id: Option<String>,
    pub last_pull_poll: Option<std::time::Instant>,
    // ── Skills ────────────────────────────────────────────────────────
    pub skills: Vec<Value>,
    pub skill: Value,
    pub skill_removed: Value,
    pub skill_search: Value,
    pub skill_search_action_id: Option<String>,
    pub skill_downloaded: Value,
    pub skill_download_action_id: Option<String>,
    pub skill_install: Value,
    pub skill_install_action_id: Option<String>,
    pub skill_publisher_blocked: Value,
    // ── MCP ───────────────────────────────────────────────────────────
    pub mcp_servers: Value,
    pub mcp_saved: Value,
    pub mcp_removed: Value,
    pub mcp_probe: Value,
    /// The latest connection report per server id. Keyed rather than global
    /// so testing one server does not relabel the others.
    pub mcp_tests: BTreeMap<String, Value>,
    // ── Codex ─────────────────────────────────────────────────────────
    pub codex: Value,
    pub codex_saved: Value,
    pub codex_doctor: Value,
    // ── FR-A8: errors rendered next to the control that produced them ──
    pub errors: BTreeMap<String, String>,
    /// The control whose mutation is in flight. The transport emits a single
    /// generic `Response::Failed`, so the UI records the last mutation target
    /// and attributes a failure to it — the control lane is serial, so this
    /// stays accurate in practice.
    pub pending: Option<String>,
}

impl SettingsState {
    /// Record that a mutation for `key` was sent: drop any stale error for it
    /// and remember the target so a `Response::Failed` lands inline.
    pub fn mutation_sent(&mut self, key: &str) {
        self.errors.remove(key);
        self.pending = Some(key.to_owned());
    }

    /// A mutation completed; clear the in-flight target. Errors for controls
    /// that failed were already inserted on failure and persist until the
    /// control is used again.
    pub fn mutation_succeeded(&mut self) {
        self.pending = None;
    }

    /// The inline error for `key`, if one is present.
    pub fn error(&self, key: &str) -> Option<&str> {
        self.errors.get(key).map(String::as_str)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsPage {
    General,
    Models,
    LocalModels,
    Skills,
    Mcp,
    Codex,
    Memory,
    Authority,
    Agent,
    Terminal,
    Privacy,
    Advanced,
}

impl SettingsPage {
    const ALL: &'static [Self] = &[
        Self::General,
        Self::Models,
        Self::LocalModels,
        Self::Skills,
        Self::Mcp,
        Self::Codex,
        Self::Memory,
        Self::Authority,
        Self::Agent,
        Self::Terminal,
        Self::Privacy,
        Self::Advanced,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::General => "General & appearance",
            Self::Models => "Models & providers",
            Self::LocalModels => "Local models",
            Self::Skills => "Skills",
            Self::Mcp => "MCP servers",
            Self::Codex => "Codex",
            Self::Memory => "Project memory",
            Self::Authority => "Authority & permissions",
            Self::Agent => "Agent behavior",
            Self::Terminal => "Terminal & Git",
            Self::Privacy => "Privacy & recovery",
            Self::Advanced => "Advanced",
        }
    }

    const fn group(self) -> &'static str {
        match self {
            Self::General => "WORKSPACE",
            Self::Models | Self::LocalModels => "MODELS",
            Self::Skills | Self::Mcp | Self::Codex => "EXTENSIONS",
            Self::Memory => "WORKSPACE",
            Self::Authority | Self::Agent | Self::Terminal => "RUNTIME",
            Self::Privacy | Self::Advanced => "SYSTEM",
        }
    }

    /// Control keywords for the "Find a setting" search (FR-A7). The label is
    /// always included; these are the settings a person might actually type.
    const fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::General => &["appearance", "theme", "dark", "light", "layout", "accent"],
            Self::Models => &[
                "provider",
                "model",
                "base url",
                "ollama",
                "lm-studio",
                "lm studio",
                "api",
                "keychain",
                "credential",
                "role",
                "default",
                "discover",
                "test",
                "remove",
                "replace",
            ],
            Self::LocalModels => &[
                "local",
                "qualify",
                "recommend",
                "unload",
                "pull",
                "memory",
                "idle timeout",
                "lifecycle",
                "keep loaded",
                "loaded",
            ],
            Self::Skills => &[
                "skill",
                "install",
                "search",
                "download",
                "publisher",
                "block",
                "scope",
                "signature",
                "qualification",
                "approve",
            ],
            Self::Mcp => &[
                "mcp",
                "server",
                "probe",
                "environment",
                "tool",
                "network",
                "working directory",
            ],
            Self::Codex => &[
                "codex",
                "binary",
                "doctor",
                "execution mode",
                "worktree",
                "auth",
                "timeout",
            ],
            Self::Memory => &[
                "memory",
                "knowledge",
                "remember",
                "forget",
                "build command",
                "rules",
            ],
            Self::Authority => &["permission", "approval", "pawgate", "authority", "mode"],
            Self::Agent => &["workflow", "budget", "search", "routing", "agent", "plan"],
            Self::Terminal => &["terminal", "git", "branch", "github", "shell"],
            Self::Privacy => &["privacy", "credential", "recovery", "history", "evidence"],
            Self::Advanced => &[
                "advanced",
                "diagnostics",
                "connectivity",
                "details",
                "debug",
            ],
        }
    }

    fn matches_query(self, query: &str) -> bool {
        let label = self.label().to_ascii_lowercase();
        if label.contains(query) {
            return true;
        }
        self.keywords()
            .iter()
            .any(|keyword| keyword.to_ascii_lowercase().contains(query))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SettingsLayout {
    compact: bool,
    nav_width: f32,
    content_width: f32,
}

fn settings_layout(available_width: f32) -> SettingsLayout {
    let available_width = available_width.max(0.0);
    if available_width < SETTINGS_COMPACT_WIDTH {
        return SettingsLayout {
            compact: true,
            nav_width: available_width,
            content_width: available_width,
        };
    }

    let nav_width = (available_width * 0.24).clamp(SETTINGS_NAV_MIN_WIDTH, SETTINGS_NAV_MAX_WIDTH);
    SettingsLayout {
        compact: false,
        nav_width,
        content_width: (available_width - nav_width - SETTINGS_COLUMN_GAP).max(0.0),
    }
}

// ── Small JSON readers ────────────────────────────────────────────────

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

fn text_or(value: &Value, key: &str, fallback: &str) -> String {
    text(value, key).unwrap_or_else(|| fallback.to_owned())
}

fn boolean(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn model_names(value: &Value) -> Vec<String> {
    array(value, "models")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Derive a stable provider profile name from a base URL hostname so the user
/// never has to invent one. `https://integrate.api.nvidia.com/v1` →
/// `integrate.api.nvidia.com`.
fn derive_provider_name(base_url: &str) -> String {
    let host = base_url
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("provider");
    if host.is_empty() {
        "provider".to_owned()
    } else {
        host.to_owned()
    }
}

/// Derive the provider type from the base URL so the user does not pick from a
/// dropdown. Known hosts map to their canonical type; everything else is
/// treated as an OpenAI-compatible endpoint.
fn derive_provider_type(base_url: &str) -> String {
    let url = base_url.to_ascii_lowercase();
    if url.contains("ollama") || url.contains("11434") {
        "ollama".to_owned()
    } else if url.contains("nvidia") {
        "nvidia-nim".to_owned()
    } else if url.contains("openai.com") || url.contains("openai.azure") {
        "openai".to_owned()
    } else {
        "openai-compatible".to_owned()
    }
}

/// `snake_case` -> "Title Case", for turning a code word into a label without
/// inventing vocabulary (the daemon owns the word; this only formats it).
fn title_case(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for (index, part) in word.split('_').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

// ── Page furniture ────────────────────────────────────────────────────
//
// The four shapes every page reaches for. They exist so a page does not invent
// its own idea of "a quiet line" and so the type sizes stay on the scale.

/// A quiet explanatory line under a control.
fn note(ui: &mut Ui, tokens: &Tokens, body: &str) {
    ui.label(
        RichText::new(body)
            .size(theme::TYPE_META)
            .line_height(Some(theme::META_LINE_HEIGHT))
            .color(tokens.text_muted),
    );
}

/// What a page says when the thing it lists does not exist yet. Body size
/// rather than meta: an empty state is the only content on screen, so it is
/// the thing being read, not an aside.
fn empty_state(ui: &mut Ui, tokens: &Tokens, body: &str) {
    ui.label(
        RichText::new(body)
            .size(theme::TYPE_BODY)
            .line_height(Some(theme::BODY_LINE_HEIGHT))
            .color(tokens.text_muted),
    );
}

/// A line in the colour of the state it reports. The words carry the meaning;
/// the colour only confirms it (PRD §27).
fn status_note(ui: &mut Ui, color: Color32, body: &str) {
    ui.label(
        RichText::new(body)
            .size(theme::TYPE_META)
            .line_height(Some(theme::META_LINE_HEIGHT))
            .color(color),
    );
}

/// A line belonging to the list row above it, indented to start under that
/// row's identity.
fn row_note(ui: &mut Ui, color: Color32, body: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(ROW_NOTE_INDENT);
        ui.label(
            RichText::new(body)
                .size(theme::TYPE_META)
                .line_height(Some(theme::META_LINE_HEIGHT))
                .color(color),
        );
    });
}

/// A read-only value sitting in a field row's control column.
fn value_line(ui: &mut Ui, color: Color32, body: &str) {
    ui.label(
        RichText::new(body)
            .size(theme::TYPE_BODY)
            .line_height(Some(theme::BODY_LINE_HEIGHT))
            .color(color),
    );
}

/// One entry in the settings navigation.
///
/// `ui.selectable_label` renders the current page as plain text with a faint
/// wash behind it, which in a column of eleven entries is not enough to say
/// where you are. The selected entry gets the product's accent-soft pill and an
/// accent hairline — the same two cues the segmented control uses, so "this one
/// is chosen" looks the same everywhere in the application.
fn nav_item(ui: &mut Ui, tokens: &Tokens, label: &str, selected: bool) -> Response {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, NAV_ITEM_HEIGHT), Sense::click());
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, theme::RADIUS_CONTROL, tokens.accent_soft);
        painter.rect_stroke(
            rect,
            theme::RADIUS_CONTROL,
            egui::Stroke::new(1.0_f32, tokens.accent_primary),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, theme::RADIUS_CONTROL, tokens.surface_hover);
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(1.0),
            theme::RADIUS_CONTROL,
            egui::Stroke::new(2.0_f32, tokens.accent_primary),
            egui::StrokeKind::Outside,
        );
    }
    let color = if selected {
        tokens.text_primary
    } else {
        tokens.text_secondary
    };
    let galley = primitives::fit_tail(
        ui,
        label,
        FontId::proportional(theme::TYPE_LABEL),
        color,
        (rect.width() - NAV_ITEM_PAD_X * 2.0).max(0.0),
    );
    painter.galley(
        egui::pos2(
            rect.left() + NAV_ITEM_PAD_X,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// What the user asked of one provider row.
#[derive(Clone, Copy, Eq, PartialEq)]
enum ProviderAction {
    Inspect,
    Test,
    Remove,
}

/// What the user asked of one qualification row.
#[derive(Clone, Copy, Eq, PartialEq)]
enum CardAction {
    Qualify,
    Unload,
}

/// What a skill row's buttons asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SkillAction {
    Remove,
    Toggle,
}

/// What a memory row's buttons asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemoryAction {
    Edit,
    Forget,
}

/// The heading for one memory kind, in the user's words rather than the
/// database's.
fn kind_label(kind: &str) -> String {
    match kind {
        "build" => "Build".to_owned(),
        "architecture" => "Architecture".to_owned(),
        "learnings" => "Learnings".to_owned(),
        "user_rules" => "Your rules".to_owned(),
        // An unknown kind is titled from its own name rather than dropped:
        // memory recorded by a newer daemon must still be visible and
        // forgettable here.
        other => {
            let mut text = other.replace('_', " ");
            if let Some(first) = text.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            text
        }
    }
}

/// What the user asked of one MCP server row.
#[derive(Clone, Copy, Eq, PartialEq)]
enum McpAction {
    Probe,
    Remove,
    /// Connect to the server directly and list its tools, with no session.
    Test,
}

impl PurrCodeIde {
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }

        let mut open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_width(SETTINGS_DEFAULT_WIDTH)
            .default_height(SETTINGS_DEFAULT_HEIGHT)
            .min_width(SETTINGS_MIN_WIDTH)
            .min_height(SETTINGS_MIN_HEIGHT)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(self.tokens.background_secondary)
                    .stroke(egui::Stroke::new(1.0_f32, self.tokens.border_subtle))
                    .corner_radius(theme::RADIUS_CARD)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        self.settings_header(ui);
                    });
                ui.add_space(GAP_BLOCK);

                let available = ui.available_size();
                let layout = settings_layout(available.x);
                if layout.compact {
                    // A narrow settings window becomes a reading order rather
                    // than forcing two columns into the same pixels.
                    ui.vertical(|ui| {
                        egui::Frame::new()
                            .fill(self.tokens.background_secondary)
                            .stroke(egui::Stroke::new(1.0_f32, self.tokens.border_subtle))
                            .corner_radius(theme::RADIUS_CARD)
                            .inner_margin(egui::Margin::symmetric(8, 10))
                            .show(ui, |ui| self.settings_navigation(ui));
                        ui.add_space(SETTINGS_COLUMN_GAP);
                        let content_width = ui.available_width();
                        ScrollArea::vertical()
                            .id_salt("purrcode_settings_content_compact")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(content_width);
                                self.settings_content(ui, ctx);
                            });
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.allocate_ui_with_layout(
                            Vec2::new(layout.nav_width, available.y),
                            Layout::top_down(Align::Min),
                            |ui| {
                                egui::Frame::new()
                                    .fill(self.tokens.background_secondary)
                                    .stroke(egui::Stroke::new(1.0_f32, self.tokens.border_subtle))
                                    .corner_radius(theme::RADIUS_CARD)
                                    .inner_margin(egui::Margin::symmetric(8, 10))
                                    .show(ui, |ui| self.settings_navigation(ui));
                            },
                        );
                        ui.add_space(SETTINGS_COLUMN_GAP);
                        ui.allocate_ui_with_layout(
                            Vec2::new(layout.content_width, available.y),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ScrollArea::vertical()
                                    .id_salt("purrcode_settings_content")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(layout.content_width);
                                        self.settings_content(ui, ctx);
                                    });
                            },
                        );
                    });
                }
            });
        self.settings_open = open;
    }

    /// The header: the brand badge + title lockup on one line, then the
    /// full-width search field on its own line below it.
    ///
    /// The same stack at every width: the search field spans the header's full
    /// available width, so it lines up with the settings content column beneath
    /// it instead of squatting on the right side of the bar.
    fn settings_header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            crate::icons::brand_badge(ui, 34.0);
            ui.add_space(GAP_BLOCK);
            self.settings_header_copy(ui);
        });
        ui.add_space(GAP_BLOCK);
        let width = ui.available_width();
        self.settings_search_field(ui, width);
    }

    fn settings_header_copy(&self, ui: &mut Ui) {
        ui.vertical(|ui| {
            // A title block is one object, not three stacked labels: the lines
            // are set tight so the eye reads them together and the bar keeps a
            // height the search field can be centred against. The eyebrow is
            // letter-spaced caps in the accent; the subtitle tucks directly
            // under the title with a hair of air, then the title's own cap
            // height carries the lockup.
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                RichText::new("PURRCODE / SETTINGS")
                    .size(theme::TYPE_META)
                    .extra_letter_spacing(theme::EYEBROW_SPACING)
                    .strong()
                    .color(self.tokens.accent_primary),
            );
            ui.add_space(LOCKUP_GAP);
            ui.label(
                RichText::new("Shape your workspace.")
                    .font(theme::display(theme::TYPE_TITLE))
                    .strong()
                    .color(self.tokens.text_primary),
            );
            ui.add_space(LOCKUP_GAP);
            ui.label(
                RichText::new(
                    "Choose how PurrCode looks and works. Safety boundaries remain explicit.",
                )
                .size(theme::TYPE_META)
                .line_height(Some(theme::META_LINE_HEIGHT))
                .color(self.tokens.text_muted),
            );
        });
    }

    /// The "Find a setting" field, painted as one control so the magnifier and
    /// the text share a single border instead of sitting next to each other.
    fn settings_search_field(&mut self, ui: &mut Ui, width: f32) {
        let tokens = self.tokens;
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(width.max(0.0), SETTINGS_SEARCH_HEIGHT),
            Sense::hover(),
        );
        let painter = ui.painter().clone();
        painter.rect_filled(rect, theme::RADIUS_CONTROL, tokens.background_raised);
        painter.rect_stroke(
            rect,
            theme::RADIUS_CONTROL,
            tokens.hairline(),
            egui::StrokeKind::Inside,
        );
        // The magnifier optically wants to sit a touch above the text baseline,
        // so it is nudged up a fraction rather than centred on the box.
        let glyph = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 15.0, rect.center().y - 0.5),
            Vec2::splat(13.0),
        );
        crate::icons::draw(ui, glyph, crate::icons::Glyph::Search, tokens.text_muted);
        let field = egui::Rect::from_min_max(
            egui::pos2(glyph.right() + 7.0, rect.top() + 3.0),
            egui::pos2(
                (rect.right() - 8.0).max(glyph.right() + 7.0),
                rect.bottom() - 3.0,
            ),
        );
        // The focus ring is painted on top of the hairline border, exactly like
        // the buttons and nav pills do it — a 2px accent ring outside the same
        // radius, so "this field is focused" reads the same as "this button is
        // focused" everywhere in the product.
        let response = ui.put(
            field,
            egui::TextEdit::singleline(&mut self.settings_search)
                .frame(false)
                .hint_text("Find a setting"),
        );
        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(1.0),
                theme::RADIUS_CONTROL,
                egui::Stroke::new(2.0_f32, tokens.accent_primary),
                egui::StrokeKind::Outside,
            );
        }
    }

    /// Whether the current search query should show a control whose keywords
    /// are `keywords`. An empty query shows everything (FR-A7).
    fn control_matches(&self, keywords: &[&str]) -> bool {
        let query = self.settings_search.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        keywords
            .iter()
            .any(|keyword| keyword.to_ascii_lowercase().contains(&query))
    }

    fn settings_content(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        match self.settings_page {
            SettingsPage::General => self.settings_general(ui, ctx),
            SettingsPage::Models => self.settings_models(ui),
            SettingsPage::LocalModels => self.settings_local_models(ui),
            SettingsPage::Skills => self.settings_skills(ui),
            SettingsPage::Mcp => self.settings_mcp(ui),
            SettingsPage::Codex => self.settings_codex(ui),
            SettingsPage::Memory => self.settings_memory(ui),
            SettingsPage::Authority => self.settings_authority(ui),
            SettingsPage::Agent => self.settings_agent(ui),
            SettingsPage::Terminal => self.settings_terminal(ui),
            SettingsPage::Privacy => self.settings_privacy(ui),
            SettingsPage::Advanced => self.settings_advanced(ui),
        }
    }

    fn settings_navigation(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        let query = self.settings_search.trim().to_ascii_lowercase();
        let searching = !query.is_empty();
        let mut previous_group: Option<&'static str> = None;
        let mut shown = 0;
        let mut target: Option<SettingsPage> = None;
        // Pills carry their own padding; the default row spacing on top of it
        // would break the column into eleven separate objects.
        ui.spacing_mut().item_spacing.y = 2.0;
        for page in SettingsPage::ALL {
            if searching && !page.matches_query(&query) {
                continue;
            }
            let group = page.group();
            if !searching && previous_group != Some(group) {
                if previous_group.is_some() {
                    ui.add_space(GAP_BLOCK);
                }
                ui.horizontal(|ui| {
                    // Indented to the pills' text, so the group reads as the
                    // heading of the entries under it.
                    ui.add_space(NAV_ITEM_PAD_X);
                    ui.label(
                        RichText::new(group)
                            .size(theme::TYPE_META)
                            .strong()
                            .color(tokens.text_muted),
                    );
                });
                ui.add_space(GAP_TIGHT);
                previous_group = Some(group);
            }
            if nav_item(ui, &tokens, page.label(), self.settings_page == *page).clicked() {
                target = Some(*page);
            }
            shown += 1;
        }
        if let Some(page) = target
            && self.settings_page != page
        {
            self.settings_page = page;
            self.settings_refresh_current();
        }
        if searching && shown == 0 {
            ui.add_space(GAP_GROUP);
            note(ui, &tokens, "No setting matches");
            note(ui, &tokens, "Try provider, model, skill, codex, mcp…");
        }
    }

    fn settings_heading(&self, ui: &mut Ui, title: &str, detail: &str) {
        // The same tracked-caps eyebrow the header uses, naming the page's
        // group. It ties every page back to the header lockup and, because the
        // group already appears in the navigation, costs nothing to read.
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(
            RichText::new(self.settings_page.group())
                .size(theme::TYPE_META)
                .extra_letter_spacing(theme::EYEBROW_SPACING)
                .strong()
                .color(self.tokens.accent_primary),
        );
        ui.add_space(LOCKUP_GAP);
        ui.label(
            RichText::new(title)
                .font(theme::display(theme::TYPE_TITLE))
                .strong()
                .color(self.tokens.text_primary),
        );
        // Not on the page scale: a title and its subtitle are one object, and
        // the smallest gap the scale offers would separate them into two.
        ui.add_space(2.0);
        ui.label(
            RichText::new(detail)
                .size(theme::TYPE_BODY)
                .line_height(Some(theme::BODY_LINE_HEIGHT))
                .color(self.tokens.text_secondary),
        );
        ui.add_space(GAP_BLOCK);
    }

    /// A status line, then the controls. Every page opens this way (§1.5).
    ///
    /// Drawn as a band in the chrome colour rather than as one more muted
    /// paragraph: this sentence says where the page's data came from, which is
    /// a different kind of statement from the settings below it and should not
    /// be mistaken for one of them.
    fn settings_status(&self, ui: &mut Ui, body: &str) {
        let tokens = self.tokens;
        egui::Frame::new()
            .fill(tokens.background_secondary)
            .stroke(tokens.hairline())
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(body)
                            .size(theme::TYPE_META)
                            .line_height(Some(theme::META_LINE_HEIGHT))
                            .color(tokens.text_muted),
                    );
                });
            });
        ui.add_space(primitives::SECTION_GAP);
    }

    /// The inline FR-A8 error for `key`, rendered directly under the control.
    fn settings_inline_error(&self, ui: &mut Ui, key: &str) {
        if let Some(error) = self.settings_state.error(key) {
            ui.add_space(GAP_TIGHT);
            status_note(ui, self.tokens.status_error, &format!("Failed: {error}"));
        }
    }

    /// Every role the daemon accepts (FR-A2).
    ///
    /// The daemon's bootstrap may advertise a `roles` list under
    /// `control_capabilities`; fall back to the canonical daemon set when it
    /// does not, so the picker is never reduced to `coding_worker`.
    fn model_roles(&self) -> Vec<String> {
        let advertised: Vec<String> = self
            .bootstrap
            .pointer("/control_capabilities/roles")
            .and_then(Value::as_array)
            .map(|roles| {
                roles
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if advertised.is_empty() {
            MODEL_ROLES.iter().map(|role| (*role).to_owned()).collect()
        } else {
            advertised
        }
    }

    // ── General & appearance ──────────────────────────────────────────

    fn settings_general(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "General & appearance",
            "Keep the workspace familiar while preserving readable contrast.",
        );
        self.settings_status(
            ui,
            "Appearance applies locally; the daemon is not involved.",
        );

        if self.control_matches(&["appearance", "theme", "dark", "light", "accent"]) {
            let mut apply = false;
            let mut reset = false;
            primitives::section(
                ui,
                &tokens,
                "Appearance",
                Some(
                    "Three palettes over one token set. High contrast is the accessible option, not a novelty.",
                ),
                |ui| {
                    // Three options laid side by side explain the choice; the
                    // same three behind a dropdown explain nothing.
                    let options: Vec<(theme::Appearance, &str)> = theme::Appearance::ALL
                        .iter()
                        .map(|appearance| (*appearance, appearance.label()))
                        .collect();
                    primitives::segmented(ui, &tokens, &options, &mut self.pending_appearance);
                    ui.add_space(GAP_GROUP);
                    let pending = self.pending_appearance != self.appearance;
                    if pending {
                        note(
                            ui,
                            &tokens,
                            &format!(
                                "Showing {}. Apply to switch to {}.",
                                self.appearance.label(),
                                self.pending_appearance.label()
                            ),
                        );
                        ui.add_space(GAP_CONTROL);
                    }
                    ui.horizontal(|ui| {
                        apply = primitives::button_enabled(
                            ui,
                            &tokens,
                            Tone::Primary,
                            "Apply appearance",
                            pending,
                        )
                        .clicked();
                        reset =
                            primitives::button_enabled(ui, &tokens, Tone::Quiet, "Reset", pending)
                                .clicked();
                    });
                },
            );
            if apply {
                self.appearance = self.pending_appearance;
                theme::install(ctx, self.appearance);
                self.tokens = Tokens::for_appearance(self.appearance);
            }
            if reset {
                self.pending_appearance = self.appearance;
            }
        }
    }

    // ── Models & providers ────────────────────────────────────────────

    fn settings_models(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Models & providers",
            "Auto remains the safe default. Choose a configured model globally or for this session.",
        );

        let providers = self.settings_state.providers.clone();
        let models = self.models.clone();
        self.settings_status(
            ui,
            &format!(
                "{} provider profile(s) · {} configured model(s).",
                providers.len(),
                models.len()
            ),
        );

        // ── Provider profiles ─────────────────────────────────────────
        if self.control_matches(&["provider", "remove", "test", "role", "default"]) {
            primitives::section(
                ui,
                &tokens,
                "Provider profiles",
                Some(
                    "Every profile the daemon has stored. Inspect lists a profile's models; Test probes its connection.",
                ),
                |ui| {
                    if providers.is_empty() {
                        empty_state(ui, &tokens, "No providers configured yet. Add one below.");
                        return;
                    }
                    // Scoped so a provider and a model that happen to share a
                    // name cannot collide on the row id.
                    ui.push_id("provider_profiles", |ui| {
                        for provider in &providers {
                            let name = text_or(provider, "name", "unnamed");
                            let inspected = self.settings_state.provider_detail.as_deref()
                                == Some(name.as_str());
                            let configured = if inspected {
                                model_names(&self.settings_state.provider)
                            } else {
                                Vec::new()
                            };
                            let meta = if !inspected {
                                "Provider profile · inspect to list its models".to_owned()
                            } else if configured.is_empty() {
                                "Provider profile · no configured models".to_owned()
                            } else {
                                format!(
                                    "Provider profile · {} configured model(s)",
                                    configured.len()
                                )
                            };
                            let asked = primitives::list_row(
                                ui,
                                &tokens,
                                RowSpec::new(&name).meta(&meta),
                                |ui| {
                                    let mut asked: Option<ProviderAction> = None;
                                    // Right-to-left, so this reads
                                    // Inspect · Test · Remove on screen.
                                    if primitives::button(ui, &tokens, Tone::Danger, "Remove")
                                        .on_hover_text("Remove this provider profile")
                                        .clicked()
                                    {
                                        asked = Some(ProviderAction::Remove);
                                    }
                                    if primitives::button(ui, &tokens, Tone::Secondary, "Test")
                                        .on_hover_text("Probe this provider's connection")
                                        .clicked()
                                    {
                                        asked = Some(ProviderAction::Test);
                                    }
                                    if primitives::button(ui, &tokens, Tone::Quiet, "Inspect")
                                        .on_hover_text("Show this provider's configured models")
                                        .clicked()
                                    {
                                        asked = Some(ProviderAction::Inspect);
                                    }
                                    asked
                                },
                            )
                            .inner;

                            if inspected && !configured.is_empty() {
                                row_note(
                                    ui,
                                    tokens.text_secondary,
                                    &format!("Models: {}", configured.join(", ")),
                                );
                            }
                            self.settings_inline_error(ui, &format!("remove:{name}"));
                            self.settings_inline_error(ui, &format!("test:{name}"));

                            match asked {
                                Some(ProviderAction::Remove) => {
                                    self.settings_state.mutation_sent(&format!("remove:{name}"));
                                    self.client
                                        .send(Request::RemoveProvider { name: name.clone() });
                                }
                                Some(ProviderAction::Test) => {
                                    self.settings_state.mutation_sent(&format!("test:{name}"));
                                    self.client
                                        .send(Request::TestProvider { name: name.clone() });
                                }
                                Some(ProviderAction::Inspect) => {
                                    self.settings_state.provider_detail = Some(name.clone());
                                    self.client
                                        .send(Request::GetProvider { name: name.clone() });
                                }
                                None => {}
                            }
                        }
                    });

                    // A Test button that produces nothing visible is a button
                    // the user presses twice. The probe's own words, once.
                    let probe = self.settings_state.provider_test.clone();
                    if !probe.is_null() {
                        let available = boolean(&probe, "available");
                        let latency = number(&probe, "latency_ms");
                        let detail = text(&probe, "detail").unwrap_or_default();
                        let mut line = format!(
                            "Last connection test: {}",
                            if available {
                                "reachable"
                            } else {
                                "not reachable"
                            }
                        );
                        if latency > 0 {
                            line.push_str(&format!(" · {latency} ms"));
                        }
                        if !detail.is_empty() {
                            line.push_str(&format!(" · {detail}"));
                        }
                        ui.add_space(GAP_CONTROL);
                        status_note(
                            ui,
                            if available {
                                tokens.status_success
                            } else {
                                tokens.status_error
                            },
                            &line,
                        );
                    }
                },
            );
        }

        // ── Discover local models ─────────────────────────────────────
        if self.control_matches(&["discover", "ollama", "lm-studio", "lm studio"]) {
            let mut discover = false;
            primitives::section(
                ui,
                &tokens,
                "Discover models from a local runtime",
                Some("Ask a runtime already on this machine what it can serve."),
                |ui| {
                    let options = [
                        ("ollama".to_owned(), "Ollama"),
                        ("lm-studio".to_owned(), "LM Studio"),
                        ("openai-compatible".to_owned(), "OpenAI-compatible"),
                    ];
                    primitives::segmented(ui, &tokens, &options, &mut self.discover_type);
                    ui.add_space(GAP_GROUP);
                    discover =
                        primitives::button(ui, &tokens, Tone::Secondary, "Discover").clicked();
                    self.settings_inline_error(ui, &format!("discover:{}", self.discover_type));
                    let discovered = model_names(&self.settings_state.discovered);
                    if !discovered.is_empty() {
                        ui.add_space(GAP_CONTROL);
                        status_note(
                            ui,
                            tokens.text_secondary,
                            &format!("Found: {}", discovered.join(", ")),
                        );
                    }
                },
            );
            if discover {
                let provider_type = self.discover_type.clone();
                self.settings_state
                    .mutation_sent(&format!("discover:{provider_type}"));
                self.client
                    .send(Request::DiscoverProviderModels { provider_type });
            }
        }

        // ── Add / edit a provider ─────────────────────────────────────
        if self.control_matches(&["provider", "base url", "api key", "credential", "replace"]) {
            let base_url = self.provider_base_url.trim().to_owned();
            let provider_name = derive_provider_name(&base_url);
            let provider_type = derive_provider_type(&base_url);
            // FR-A2: editing an existing profile sends `replace: true`, so a
            // re-save succeeds instead of erroring with "already exists".
            let replace = providers
                .iter()
                .any(|provider| text(provider, "name").as_deref() == Some(provider_name.as_str()));
            let mut submit = false;
            primitives::section(
                ui,
                &tokens,
                "Add or edit provider",
                Some(
                    "Enter the API key, base URL and a model. The profile name and type are derived from the base URL.",
                ),
                |ui| {
                    primitives::field_row(ui, &tokens, "API key", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.provider_api_key)
                                .password(true)
                                .hint_text("Optional for local providers")
                                .desired_width(room),
                        );
                    });
                    primitives::field_row(ui, &tokens, "Base URL", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.provider_base_url)
                                .desired_width(room),
                        );
                    });
                    primitives::field_row(ui, &tokens, "Model ID", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.provider_model)
                                .desired_width(room),
                        );
                    });
                    ui.add_space(GAP_TIGHT);
                    status_note(
                        ui,
                        tokens.text_secondary,
                        &format!("{provider_name} · {provider_type}"),
                    );
                    if replace {
                        ui.add_space(GAP_TIGHT);
                        status_note(
                            ui,
                            tokens.status_warning,
                            &format!(
                                "Editing existing provider `{provider_name}` — the profile will be replaced."
                            ),
                        );
                    }
                    ui.add_space(GAP_BLOCK);
                    let ready = !self.provider_model.trim().is_empty() && !base_url.is_empty();
                    let label = if replace {
                        "Replace provider"
                    } else {
                        "Test and add provider"
                    };
                    submit = primitives::button_enabled(ui, &tokens, Tone::Primary, label, ready)
                        .clicked();
                    if !ready {
                        ui.add_space(GAP_TIGHT);
                        note(ui, &tokens, "A base URL and a model ID are required.");
                    }
                    self.settings_inline_error(ui, "configure");
                },
            );
            if submit {
                self.settings_state.mutation_sent("configure");
                self.client.send(Request::ConfigureProvider {
                    name: provider_name,
                    provider_type,
                    base_url: base_url.clone(),
                    model: self.provider_model.trim().to_owned(),
                    credential_name: None,
                    secret: (!self.provider_api_key.trim().is_empty())
                        .then(|| self.provider_api_key.trim().to_owned()),
                    replace,
                });
            }
        }

        // ── Role assignment ───────────────────────────────────────────
        if self.control_matches(&["role", "default", "assign"]) {
            primitives::section(
                ui,
                &tokens,
                "Assign model roles",
                Some(
                    "Which model answers which kind of work. A long model id gives way before the buttons do, and hovering shows it in full.",
                ),
                |ui| {
                    if models.is_empty() {
                        empty_state(ui, &tokens, "Configure a provider and its model first.");
                        return;
                    }
                    let roles = self.model_roles();
                    let mut assigned: Option<(String, String)> = None;
                    ui.push_id("model_roles", |ui| {
                        for model in &models {
                            let scope = if model.local { "Local" } else { "Remote" };
                            let held = if model.roles.is_empty() {
                                "No assigned role".to_owned()
                            } else {
                                model.roles.join(", ")
                            };
                            // The default is stated in words as well as marked
                            // with a dot: colour is never the only signal.
                            let meta = format!(
                                "{scope} · {held}{}",
                                if model.is_default { " · Default" } else { "" }
                            );
                            let mut spec = RowSpec::new(&model.id).meta(&meta);
                            if model.is_default {
                                spec = spec.marker(tokens.status_success);
                            }
                            let mut chosen = self
                                .provider_role
                                .get(&model.id)
                                .cloned()
                                .unwrap_or_else(|| "coding_worker".to_owned());
                            let asked = primitives::list_row(ui, &tokens, spec, |ui| {
                                let mut asked: Option<(String, String)> = None;
                                if primitives::button_enabled(
                                    ui,
                                    &tokens,
                                    Tone::Secondary,
                                    "Make default",
                                    !model.is_default,
                                )
                                .on_hover_text("Assign to coding_worker and set as default")
                                .clicked()
                                {
                                    asked = Some(("coding_worker".to_owned(), model.id.clone()));
                                }
                                if primitives::button(ui, &tokens, Tone::Primary, "Assign")
                                    .on_hover_text("Assign this model to the chosen role")
                                    .clicked()
                                {
                                    asked = Some((chosen.clone(), model.id.clone()));
                                }
                                // Seven roles is past what a segmented control
                                // can show, so this one choice stays a picker —
                                // at a fixed width, so every row's action column
                                // is the same width.
                                egui::ComboBox::from_id_salt(("model_role", &model.id))
                                    .selected_text(&chosen)
                                    .width(ROLE_PICKER_WIDTH)
                                    .show_ui(ui, |ui| {
                                        for role in &roles {
                                            ui.selectable_value(
                                                &mut chosen,
                                                role.clone(),
                                                role.clone(),
                                            );
                                        }
                                    });
                                asked
                            })
                            .inner;

                            if chosen
                                != self
                                    .provider_role
                                    .get(&model.id)
                                    .cloned()
                                    .unwrap_or_default()
                            {
                                self.provider_role.insert(model.id.clone(), chosen.clone());
                            }
                            if let Some(request) = asked {
                                assigned = Some(request);
                            }
                        }
                    });
                    if let Some((role, model)) = assigned {
                        self.settings_state.mutation_sent(&format!("role:{role}"));
                        self.client.send(Request::AssignModelRole { role, model });
                    }
                    if let Some(key) = self.settings_state.pending.clone()
                        && key.starts_with("role:")
                    {
                        self.settings_inline_error(ui, &key);
                    }
                },
            );
        }
    }

    // ── Local models ──────────────────────────────────────────────────

    fn settings_local_models(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Local models",
            "Qualify, unload and pull local Ollama models without touching a terminal.",
        );

        let status = self.settings_state.local_models.clone();
        let reachable = boolean(&status, "reachable");
        let installed = array(&status, "installed")
            .iter()
            .filter_map(|m| text(m, "name"))
            .collect::<Vec<_>>();
        let loaded = array(&status, "loaded")
            .iter()
            .filter_map(|m| text(m, "name"))
            .collect::<Vec<_>>();
        let resources = status.get("resources").cloned().unwrap_or(Value::Null);
        let pressure = text_or(&resources, "memory_pressure", "unknown");
        let version = text_or(&status, "version", "unknown");

        let status_line = if reachable {
            format!(
                "Ollama reachable ({version}) · {} installed · {} loaded · memory pressure {pressure}",
                installed.len(),
                loaded.len()
            )
        } else {
            "Ollama is not reachable. Start it, then open this page again.".to_owned()
        };
        self.settings_status(ui, &status_line);

        // ── Lifecycle settings ────────────────────────────────────────
        if self.control_matches(&["lifecycle", "idle timeout", "keep loaded", "unload"]) {
            let settings = self.settings_state.local_settings.clone();
            let policy = text_or(&settings, "policy", "unload_after_request");
            let idle = number(&settings, "idle_timeout_seconds");
            let mut save = false;
            primitives::section(
                ui,
                &tokens,
                "Lifecycle policy",
                Some(
                    "When a loaded model is released. The daemon owns the policy; this chooses which one it enforces.",
                ),
                |ui| {
                    // Exactly four options, so they are shown rather than hidden
                    // one click deep.
                    let options = [
                        ("unload_after_request".to_owned(), "Unload after request"),
                        ("idle_timeout".to_owned(), "Idle timeout"),
                        ("keep_loaded".to_owned(), "Keep loaded"),
                        ("external".to_owned(), "External"),
                    ];
                    let mut chosen = self.local_policy.clone().unwrap_or_else(|| policy.clone());
                    primitives::segmented(ui, &tokens, &options, &mut chosen);
                    self.local_policy = Some(chosen.clone());
                    ui.add_space(GAP_GROUP);
                    if chosen == "idle_timeout" {
                        primitives::field_row(ui, &tokens, "Idle timeout (seconds)", |ui| {
                            let mut timeout =
                                self.local_idle_timeout.unwrap_or_else(|| idle.max(30));
                            ui.add(
                                egui::DragValue::new(&mut timeout)
                                    .range(30..=86_400)
                                    .speed(60),
                            );
                            self.local_idle_timeout = Some(timeout);
                        });
                        ui.add_space(GAP_TIGHT);
                    }
                    save = primitives::button(ui, &tokens, Tone::Primary, "Save policy").clicked();
                    self.settings_inline_error(ui, "local_settings");
                },
            );
            if save {
                let body = serde_json::json!({
                    "policy": self.local_policy.clone().unwrap_or_else(|| policy.clone()),
                    "idle_timeout_seconds": self.local_idle_timeout.unwrap_or(1800).clamp(30, 86_400),
                });
                self.settings_state.mutation_sent("local_settings");
                self.client
                    .send(Request::LocalModelsPutSettings { settings: body });
            }
        }

        // ── Qualification cards ───────────────────────────────────────
        if self.control_matches(&["qualify", "recommend", "loaded", "unload", "memory"]) {
            let cards = self
                .settings_state
                .recommendations
                .pointer("/report/cards")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            primitives::section(
                ui,
                &tokens,
                "Qualification",
                Some("A card's status and its stated risks, never a bare recommendation badge."),
                |ui| {
                    if cards.is_empty() {
                        empty_state(ui, &tokens, "No local models to qualify yet.");
                        return;
                    }
                    let mut qualify: Option<String> = None;
                    let mut unload: Option<String> = None;
                    ui.push_id("qualification_cards", |ui| {
                        for card in &cards {
                            let model = text_or(card, "model", "unknown model");
                            let status_word = text_or(card, "status", "not_recommended");
                            let loaded_now = boolean(card, "currently_loaded");
                            let installed_now = boolean(card, "installed");
                            let params = card
                                .get("parameter_count")
                                .and_then(Value::as_u64)
                                .map(|v| format!("{:.1}B", v as f64 / 1_000_000_000.0))
                                .unwrap_or_else(|| "unknown size".to_owned());
                            let quant = text(card, "quantization").unwrap_or_default();
                            let state_color = match status_word.as_str() {
                                "recommended" => tokens.status_success,
                                "eligible_alternative" => tokens.status_info,
                                _ => tokens.text_muted,
                            };
                            let meta = format!(
                                "{} · {params}{}{}",
                                title_case(&status_word),
                                if quant.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {quant}")
                                },
                                if loaded_now { " · Loaded" } else { "" }
                            );
                            let asked = primitives::list_row(
                                ui,
                                &tokens,
                                RowSpec::new(&model).meta(&meta).marker(state_color),
                                |ui| {
                                    let mut asked: Option<CardAction> = None;
                                    if installed_now
                                        && primitives::button(ui, &tokens, Tone::Primary, "Qualify")
                                            .on_hover_text(
                                                "Run a bounded generation probe (can take minutes)",
                                            )
                                            .clicked()
                                    {
                                        asked = Some(CardAction::Qualify);
                                    }
                                    if loaded_now
                                        && primitives::button(
                                            ui,
                                            &tokens,
                                            Tone::Secondary,
                                            "Unload",
                                        )
                                        .clicked()
                                    {
                                        asked = Some(CardAction::Unload);
                                    }
                                    asked
                                },
                            )
                            .inner;

                            // State the risks in words, never by colour alone.
                            for risk in array(card, "risks") {
                                let detail = text(&risk, "detail")
                                    .or_else(|| text(&risk, "code"))
                                    .unwrap_or_else(|| "risk".to_owned());
                                row_note(ui, tokens.status_warning, &format!("Risk: {detail}"));
                            }
                            for blocker in array(card, "not_recommended_reasons") {
                                let detail = text(&blocker, "detail")
                                    .or_else(|| text(&blocker, "code"))
                                    .unwrap_or_else(|| "reason".to_owned());
                                row_note(
                                    ui,
                                    tokens.text_muted,
                                    &format!("Not recommended: {detail}"),
                                );
                            }

                            match asked {
                                Some(CardAction::Qualify) => qualify = Some(model.clone()),
                                Some(CardAction::Unload) => unload = Some(model.clone()),
                                None => {}
                            }
                        }
                    });
                    if let Some(model) = qualify {
                        self.settings_state
                            .mutation_sent(&format!("qualify:{model}"));
                        self.client.send(Request::LocalModelsQualify { model });
                    }
                    if let Some(model) = unload {
                        self.settings_state
                            .mutation_sent(&format!("unload:{model}"));
                        self.client
                            .send(Request::LocalModelsUnload { model, all: false });
                    }
                    if let Some(key) = self.settings_state.pending.clone()
                        && (key.starts_with("qualify:") || key.starts_with("unload:"))
                    {
                        self.settings_inline_error(ui, &key);
                    }
                },
            );
        }

        // ── Pull flow (propose → approve → start → poll → cancel) ────
        if self.control_matches(&["pull", "approve", "cancel"]) {
            let proposal = self.settings_state.pull_proposal.clone();
            let action_id = self.settings_state.pull_action_id.clone();
            let phase = if action_id.is_some() {
                let progress = self.settings_state.pull_progress.clone();
                if progress.is_null() {
                    text_or(&proposal, "status", "proposed")
                } else {
                    text_or(&progress, "phase", "unknown")
                }
            } else {
                String::new()
            };

            // Poll the JSON progress route while a pull is running.
            if let Some(id) = &action_id {
                let terminal = matches!(phase.as_str(), "completed" | "cancelled" | "failed");
                let due = self
                    .settings_state
                    .last_pull_poll
                    .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(1));
                if !terminal && due {
                    self.settings_state.last_pull_poll = Some(std::time::Instant::now());
                    self.client.send(Request::LocalModelsPullPoll {
                        action_id: id.clone(),
                    });
                }
            }

            let mut propose = false;
            primitives::section(
                ui,
                &tokens,
                "Pull a model",
                Some(
                    "Four approved steps: propose, approve, start, then watch progress. Cancel stays available throughout.",
                ),
                |ui| {
                    primitives::field_row(ui, &tokens, "Model name", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.pull_model_input)
                                .hint_text("e.g. qwen2.5-coder:7b")
                                .desired_width(room),
                        );
                    });
                    ui.add_space(GAP_TIGHT);
                    propose = primitives::button_enabled(
                        ui,
                        &tokens,
                        Tone::Primary,
                        "Propose pull",
                        !self.pull_model_input.trim().is_empty(),
                    )
                    .clicked();
                    let pull_key = format!("pull:{}", self.pull_model_input.trim());
                    self.settings_inline_error(ui, &pull_key);

                    if phase.is_empty() {
                        return;
                    }
                    let message = text_or(&self.settings_state.pull_progress, "message", &phase);
                    ui.add_space(GAP_GROUP);
                    status_note(
                        ui,
                        match phase.as_str() {
                            "completed" => tokens.status_success,
                            "cancelled" | "failed" => tokens.status_error,
                            _ => tokens.status_running,
                        },
                        &format!("Pull status: {} — {message}", title_case(&phase)),
                    );
                    let Some(id) = action_id.clone() else {
                        return;
                    };
                    let session_id = self
                        .settings_state
                        .pull_session_id
                        .clone()
                        .unwrap_or_default();
                    ui.add_space(GAP_CONTROL);
                    ui.horizontal(|ui| {
                        // Each step is offered only in the phase it belongs to,
                        // so the flow cannot be skipped from the interface.
                        if matches!(phase.as_str(), "proposed" | "requires_approval")
                            && primitives::button(ui, &tokens, Tone::Primary, "Approve pull")
                                .clicked()
                        {
                            self.settings_state
                                .mutation_sent(&format!("pull_approve:{id}"));
                            self.client.send(Request::LocalModelsPullApprove {
                                session_id: session_id.clone(),
                                action_id: id.clone(),
                            });
                        }
                        if phase == "approved"
                            && primitives::button(ui, &tokens, Tone::Primary, "Start pull")
                                .clicked()
                        {
                            self.settings_state
                                .mutation_sent(&format!("pull_start:{id}"));
                            self.client.send(Request::LocalModelsPullStart {
                                session_id: session_id.clone(),
                                action_id: id.clone(),
                            });
                        }
                        if !matches!(phase.as_str(), "completed" | "cancelled" | "failed")
                            && primitives::button(ui, &tokens, Tone::Danger, "Cancel pull")
                                .clicked()
                        {
                            self.settings_state
                                .mutation_sent(&format!("pull_cancel:{id}"));
                            self.client.send(Request::LocalModelsPullCancel {
                                session_id: session_id.clone(),
                                action_id: id.clone(),
                            });
                        }
                    });
                    self.settings_inline_error(ui, &format!("pull_approve:{id}"));
                    self.settings_inline_error(ui, &format!("pull_start:{id}"));
                    self.settings_inline_error(ui, &format!("pull_cancel:{id}"));
                },
            );
            if propose {
                let model = self.pull_model_input.trim().to_owned();
                self.settings_state.mutation_sent(&format!("pull:{model}"));
                // The daemon accepts an existing session *or* a repository
                // for a dedicated pull session, never both.
                let (session_id, repository) = match self.selected.clone() {
                    Some(session) => (Some(session), None),
                    None => (None, Some(self.repository_string())),
                };
                self.client.send(Request::LocalModelsPullPropose {
                    session_id,
                    repository,
                    model,
                });
            }
        }
    }

    // ── Skills ────────────────────────────────────────────────────────

    fn settings_skills(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Skills",
            "Inspect, remove and install qualified skills. Installing walks the full approval gate.",
        );

        let skills = self.settings_state.skills.clone();
        self.settings_status(ui, &format!("{} skill(s) installed.", skills.len()));

        let has_session = self.selected.is_some();
        if !has_session {
            status_note(
                ui,
                tokens.status_warning,
                "Search, download and install need a selected session, because every step is approval-gated on that session. Open a session first.",
            );
            ui.add_space(primitives::SECTION_GAP);
        }

        // ── Installed skills ──────────────────────────────────────────
        if self.control_matches(&["skill", "remove", "scope", "publisher", "signature"]) {
            primitives::section(
                ui,
                &tokens,
                "Installed skills",
                Some(
                    "Scope, publisher, signature and qualification for everything this workspace can call.",
                ),
                |ui| {
                    if skills.is_empty() {
                        empty_state(ui, &tokens, "No skills installed yet.");
                        return;
                    }
                    let mut remove: Option<String> = None;
                    let mut toggle: Option<(String, bool)> = None;
                    ui.push_id("installed_skills", |ui| {
                        for skill in &skills {
                            let id = text_or(skill, "skill_id", "unnamed");
                            let scope = text_or(skill, "scope", "?");
                            let publisher = text_or(skill, "publisher", "unknown publisher");
                            let signature = text_or(skill, "signature_status", "unsigned");
                            let qualification =
                                text_or(skill, "qualification_status", "unverified");
                            let uses = number(skill, "successful_uses");
                            let failures = number(skill, "failed_uses");
                            // A skill record from a daemon that predates the
                            // toggle has no `enabled` field. Treating the
                            // absence as "disabled" would silently switch off
                            // every installed skill on upgrade, so an unstated
                            // value means enabled — which is what it was.
                            let enabled = skill
                                .get("enabled")
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                            let meta =
                                format!("{scope} · {publisher} · {signature} · {qualification}");
                            let asked = primitives::list_row(
                                ui,
                                &tokens,
                                RowSpec::new(&id).meta(&meta),
                                |ui| {
                                    let mut asked: Option<SkillAction> = None;
                                    if primitives::button(ui, &tokens, Tone::Danger, "Remove")
                                        .on_hover_text("Remove this skill from the workspace")
                                        .clicked()
                                    {
                                        asked = Some(SkillAction::Remove);
                                    }
                                    if primitives::button(
                                        ui,
                                        &tokens,
                                        Tone::Secondary,
                                        if enabled { "Disable" } else { "Enable" },
                                    )
                                    .on_hover_text(if enabled {
                                        "Keep it installed and inspectable, but never invoke it"
                                    } else {
                                        "Allow the agent to invoke this skill again"
                                    })
                                    .clicked()
                                    {
                                        asked = Some(SkillAction::Toggle);
                                    }
                                    asked
                                },
                            )
                            .inner;
                            row_note(
                                ui,
                                tokens.text_secondary,
                                &format!("Uses: {uses} successful · {failures} failed"),
                            );
                            if !enabled {
                                // Disabled is a real state with a real
                                // consequence: the agent looks elsewhere for
                                // the capability rather than quietly losing it.
                                row_note(
                                    ui,
                                    tokens.status_warning,
                                    "Disabled — the agent cannot invoke this skill.",
                                );
                            }
                            match asked {
                                Some(SkillAction::Remove) => remove = Some(id.clone()),
                                Some(SkillAction::Toggle) => {
                                    toggle = Some((id.clone(), !enabled));
                                }
                                None => {}
                            }
                        }
                    });
                    if let Some(id) = remove {
                        self.settings_state
                            .mutation_sent(&format!("skill_remove:{id}"));
                        self.client.send(Request::RemoveSkill { id });
                    }
                    if let Some((id, enabled)) = toggle {
                        self.settings_state
                            .mutation_sent(&format!("skill_toggle:{id}"));
                        self.client.send(Request::SkillSetEnabled { id, enabled });
                    }
                    if let Some(key) = self.settings_state.pending.clone()
                        && key.starts_with("skill_remove:")
                    {
                        self.settings_inline_error(ui, &key);
                    }
                },
            );
        }

        // ── Search / download / install gate ──────────────────────────
        if self.control_matches(&["search", "install", "download", "approve", "block"]) {
            let session_id = self.selected.clone();
            let mut download: Option<(String, String)> = None;

            primitives::section(
                ui,
                &tokens,
                "Find a skill",
                Some(
                    "Search → approve search → download → approve download → qualify → approve install → execute. The IDE never skips the gate.",
                ),
                |ui| {
                    primitives::field_row(ui, &tokens, "Capability", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.skill_capability_input)
                                .hint_text("e.g. \"web scraping\"")
                                .desired_width(room),
                        );
                    });
                    ui.add_space(GAP_TIGHT);
                    let search_action = self.settings_state.skill_search_action_id.clone();
                    ui.horizontal(|ui| {
                        if primitives::button_enabled(
                            ui,
                            &tokens,
                            Tone::Primary,
                            "Search",
                            has_session && !self.skill_capability_input.trim().is_empty(),
                        )
                        .clicked()
                        {
                            let capability = self.skill_capability_input.trim().to_owned();
                            self.settings_state.mutation_sent("skill_search");
                            self.settings_state.skill_search_action_id = None;
                            self.client.send(Request::SkillSearch {
                                session_id: session_id.clone().unwrap_or_default(),
                                capability,
                                keywords: Vec::new(),
                                action_id: None,
                            });
                        }
                        if let Some(action_id) = &search_action
                            && primitives::button_enabled(
                                ui,
                                &tokens,
                                Tone::Secondary,
                                "Approve search",
                                has_session,
                            )
                            .clicked()
                        {
                            self.settings_state.mutation_sent("skill_search_approve");
                            self.client.send(Request::SkillSearch {
                                session_id: session_id.clone().unwrap_or_default(),
                                capability: self.skill_capability_input.trim().to_owned(),
                                keywords: Vec::new(),
                                action_id: Some(action_id.clone()),
                            });
                        }
                    });
                    if !has_session {
                        ui.add_space(GAP_TIGHT);
                        note(
                            ui,
                            &tokens,
                            "Search is disabled until a session is selected.",
                        );
                    }
                    self.settings_inline_error(ui, "skill_search");
                    self.settings_inline_error(ui, "skill_search_approve");

                    // The search answer is an array of candidates.
                    let candidates = self
                        .settings_state
                        .skill_search
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    if candidates.is_empty() {
                        return;
                    }
                    ui.add_space(GAP_GROUP);
                    ui.push_id("skill_candidates", |ui| {
                        for candidate in &candidates {
                            let manifest = candidate
                                .get("manifest")
                                .cloned()
                                .unwrap_or_else(|| candidate.clone());
                            let id = text_or(&manifest, "candidate_id", "candidate");
                            let name = text_or(&manifest, "name", &id);
                            let version = text_or(&manifest, "version", "?");
                            let publisher = text_or(&manifest, "publisher", "unknown publisher");
                            let commit = manifest
                                .pointer("/immutable_source/GitCommit/commit")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .unwrap_or_default();
                            let identity = format!("{name} {version}");
                            let meta = format!("{publisher} · {id}");
                            let asked = primitives::list_row(
                                ui,
                                &tokens,
                                RowSpec::new(&identity).meta(&meta),
                                |ui| {
                                    // Without an immutable source there is
                                    // nothing to pin the download to, so the
                                    // action is not offered at all.
                                    !commit.is_empty()
                                        && primitives::button_enabled(
                                            ui,
                                            &tokens,
                                            Tone::Secondary,
                                            "Download",
                                            has_session,
                                        )
                                        .clicked()
                                },
                            )
                            .inner;
                            if asked {
                                download = Some((id.clone(), commit.clone()));
                            }
                        }
                    });
                },
            );
            if let Some((candidate_id, commit)) = download {
                self.settings_state
                    .mutation_sent(&format!("skill_download:{candidate_id}"));
                self.settings_state.skill_download_action_id = None;
                self.client.send(Request::SkillDownload {
                    session_id: session_id.clone().unwrap_or_default(),
                    candidate_id,
                    commit,
                    action_id: None,
                });
            }

            // ── Approve and install ───────────────────────────────────
            let downloaded = self.settings_state.skill_downloaded.clone();
            let source_path = text(&downloaded, "source_path");
            let content_digest = text(&downloaded, "content_digest");
            let candidate_id = text(&downloaded, "candidate_id");
            let download_action = self.settings_state.skill_download_action_id.clone();
            let install_proposal = self.settings_state.skill_install.clone();
            let install_action = self.settings_state.skill_install_action_id.clone();
            primitives::section(
                ui,
                &tokens,
                "Approve and install",
                Some(
                    "Each gate stays visible after it is passed, so the state of an install is readable rather than remembered.",
                ),
                |ui| {
                    // The empty state does not short-circuit the section: an
                    // approval that failed leaves an FR-A8 error behind and no
                    // gate to hang it on, and an error nobody renders is an
                    // error the user has to guess at.
                    if download_action.is_none()
                        && candidate_id.is_none()
                        && install_action.is_none()
                    {
                        empty_state(
                            ui,
                            &tokens,
                            "Nothing is waiting for approval. Search for a skill to begin.",
                        );
                    }
                    if let Some(action_id) = &download_action
                        && primitives::button_enabled(
                            ui,
                            &tokens,
                            Tone::Primary,
                            "Approve download",
                            has_session,
                        )
                        .clicked()
                    {
                        self.settings_state.mutation_sent("skill_download_approve");
                        if let (Some(cid), Some(commit)) =
                            (candidate_id.clone(), text(&downloaded, "commit"))
                        {
                            self.client.send(Request::SkillDownload {
                                session_id: session_id.clone().unwrap_or_default(),
                                candidate_id: cid,
                                commit,
                                action_id: Some(action_id.clone()),
                            });
                        }
                    }
                    self.settings_inline_error(
                        ui,
                        &format!(
                            "skill_download:{}",
                            candidate_id.as_deref().unwrap_or("candidate")
                        ),
                    );
                    self.settings_inline_error(ui, "skill_download_approve");

                    if let (Some(candidate_id), Some(source_path), Some(content_digest)) = (
                        candidate_id.clone(),
                        source_path.clone(),
                        content_digest.clone(),
                    ) {
                        ui.add_space(GAP_CONTROL);
                        if primitives::button_enabled(
                            ui,
                            &tokens,
                            Tone::Secondary,
                            "Propose install",
                            has_session,
                        )
                        .clicked()
                        {
                            let version = text_or(&downloaded, "version", "0.0.0");
                            let publisher = text(&downloaded, "publisher");
                            self.settings_state.mutation_sent("skill_install_propose");
                            self.settings_state.skill_install_action_id = None;
                            self.client.send(Request::SkillInstallPropose {
                                session_id: session_id.clone().unwrap_or_default(),
                                candidate_id,
                                version,
                                scope: "user".to_owned(),
                                source_path,
                                content_digest,
                                publisher,
                                approved_permissions: Value::Object(Default::default()),
                                signature: None,
                                publisher_public_key: None,
                            });
                        }
                    }
                    self.settings_inline_error(ui, "skill_install_propose");

                    let Some(action_id) = install_action.clone() else {
                        return;
                    };
                    let approved = text_or(&install_proposal, "status", "") == "approved";
                    ui.add_space(GAP_CONTROL);
                    ui.horizontal(|ui| {
                        if primitives::button_enabled(
                            ui,
                            &tokens,
                            Tone::Secondary,
                            "Approve install",
                            has_session,
                        )
                        .clicked()
                        {
                            self.settings_state.mutation_sent("skill_install_approve");
                            self.client.send(Request::SkillInstallApprove {
                                session_id: session_id.clone().unwrap_or_default(),
                                action_id: action_id.clone(),
                            });
                        }
                        if primitives::button_enabled(
                            ui,
                            &tokens,
                            Tone::Primary,
                            "Execute install",
                            has_session && approved,
                        )
                        .clicked()
                        {
                            self.settings_state.mutation_sent("skill_install_execute");
                            self.client.send(Request::SkillInstall {
                                session_id: session_id.clone().unwrap_or_default(),
                                action_id: action_id.clone(),
                            });
                        }
                    });
                    if !approved {
                        ui.add_space(GAP_TIGHT);
                        note(
                            ui,
                            &tokens,
                            "Execute stays unavailable until the daemon reports the proposal as approved.",
                        );
                    }
                    self.settings_inline_error(ui, "skill_install_approve");
                    self.settings_inline_error(ui, "skill_install_execute");
                },
            );

            // ── Block a publisher ─────────────────────────────────────
            let mut block = false;
            primitives::section(
                ui,
                &tokens,
                "Blocked publishers",
                Some(
                    "A blocked publisher's skills are never offered again, whatever a search returns.",
                ),
                |ui| {
                    primitives::field_row(ui, &tokens, "Publisher", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.skill_publisher_input)
                                .hint_text("Publisher to block")
                                .desired_width(room),
                        );
                    });
                    ui.add_space(GAP_TIGHT);
                    block = primitives::button_enabled(
                        ui,
                        &tokens,
                        Tone::Danger,
                        "Block publisher",
                        !self.skill_publisher_input.trim().is_empty(),
                    )
                    .clicked();
                    if let Some(key) = self.settings_state.pending.clone()
                        && key.starts_with("block:")
                    {
                        self.settings_inline_error(ui, &key);
                    }
                },
            );
            if block {
                let publisher = self.skill_publisher_input.trim().to_owned();
                self.settings_state
                    .mutation_sent(&format!("block:{publisher}"));
                self.client.send(Request::SkillBlockPublisher {
                    publisher,
                    reason: "Blocked from Settings by the user".to_owned(),
                });
            }
        }
    }

    // ── Project memory ────────────────────────────────────────────────

    /// Durable project knowledge, with its provenance on the surface.
    ///
    /// v1.2 Pillar 6. The deliberate choice here is that PurrCode does not
    /// secretly remember things: every entry is visible, says where it came
    /// from and how sure it is, and can be edited or forgotten. Memory the
    /// user cannot see is memory they cannot correct, and a wrong fact that
    /// silently shapes every future session is worse than no memory at all.
    fn settings_memory(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Project memory",
            "What PurrCode carries between sessions about this project. Every entry shows where \
             it came from, and you can change or remove any of it.",
        );

        if self.repository.as_os_str().is_empty() {
            note(ui, &tokens, "Open a folder to see its project memory.");
            return;
        }

        let entries = self.memory.clone();
        self.settings_status(
            ui,
            &if entries.is_empty() {
                "Nothing remembered for this project yet.".to_owned()
            } else {
                format!(
                    "{} entr{} remembered.",
                    entries.len(),
                    if entries.len() == 1 { "y" } else { "ies" }
                )
            },
        );

        let mut forget: Option<String> = None;
        let mut edit: Option<(String, String)> = None;
        for (kind, description) in crate::model::MEMORY_KINDS {
            let group: Vec<_> = entries.iter().filter(|entry| entry.kind == *kind).collect();
            if group.is_empty() && !self.control_matches(&[kind, "memory"]) {
                continue;
            }
            primitives::section(ui, &tokens, &kind_label(kind), Some(description), |ui| {
                if group.is_empty() {
                    empty_state(ui, &tokens, "Nothing here yet.");
                    return;
                }
                ui.push_id(format!("memory_{kind}"), |ui| {
                    for entry in &group {
                        let asked =
                            primitives::list_row(ui, &tokens, RowSpec::new(&entry.content), |ui| {
                                let mut asked: Option<MemoryAction> = None;
                                if primitives::button(ui, &tokens, Tone::Danger, "Forget")
                                    .on_hover_text(
                                        "Remove this permanently. It will not come back on \
                                             its own.",
                                    )
                                    .clicked()
                                {
                                    asked = Some(MemoryAction::Forget);
                                }
                                if primitives::button(ui, &tokens, Tone::Secondary, "Edit")
                                    .clicked()
                                {
                                    asked = Some(MemoryAction::Edit);
                                }
                                asked
                            })
                            .inner;
                        // The provenance line is the whole argument for
                        // this surface: a fact with no visible source is
                        // a fact nobody can check.
                        row_note(ui, tokens.text_muted, &entry.provenance());
                        match asked {
                            Some(MemoryAction::Forget) => forget = Some(entry.id.clone()),
                            Some(MemoryAction::Edit) => {
                                edit = Some((entry.id.clone(), entry.content.clone()));
                            }
                            None => {}
                        }
                    }
                });
            });
        }

        if let Some(id) = forget {
            self.settings_state
                .mutation_sent(&format!("memory_forget:{id}"));
            self.client.send(Request::ForgetMemory { id });
        }
        if let Some((id, content)) = edit {
            self.editing_memory = Some((id, content));
        }

        // ── Remember something ────────────────────────────────────────
        let mut save = false;
        primitives::section(
            ui,
            &tokens,
            "Remember something",
            Some(
                "Added entries are attributed to you, and marked unverified until something confirms them.",
            ),
            |ui| {
                primitives::field_row(ui, &tokens, "Kind", |ui| {
                    for (kind, _) in crate::model::MEMORY_KINDS {
                        ui.selectable_value(
                            &mut self.memory_kind,
                            (*kind).to_owned(),
                            kind_label(kind),
                        );
                    }
                });
                primitives::field_row(ui, &tokens, "What to remember", |ui| {
                    let room = ui.available_width();
                    ui.add(
                        egui::TextEdit::multiline(&mut self.memory_content)
                            .hint_text("Integration tests need Redis running on :6379")
                            .desired_rows(2)
                            .desired_width(room),
                    );
                });
                ui.add_space(GAP_CONTROL);
                let ready = !self.memory_content.trim().is_empty();
                save = primitives::button_enabled(ui, &tokens, Tone::Primary, "Remember", ready)
                    .clicked();
                self.settings_inline_error(ui, "memory_save");
            },
        );
        if save {
            self.settings_state.mutation_sent("memory_save");
            self.client.send(Request::CreateMemory {
                repository: self.repository_string(),
                kind: self.memory_kind.clone(),
                content: self.memory_content.trim().to_owned(),
                // Attribution is honest: the user typed this, so the entry
                // says so rather than claiming a session discovered it.
                source: "Added by you in Settings".to_owned(),
            });
            self.memory_content.clear();
        }
    }

    /// The edit dialog for one memory entry.
    pub(crate) fn memory_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, content)) = self.editing_memory.clone() else {
            return;
        };
        let tokens = self.tokens;
        let mut next = content;
        let (choice, ()) = primitives::dialog(
            ctx,
            &tokens,
            "purrcode_edit_memory",
            "Edit memory",
            ("Save", Tone::Primary),
            !next.trim().is_empty(),
            |ui| {
                ui.label(
                    egui::RichText::new("The entry keeps its original source and date.")
                        .size(crate::theme::TYPE_META)
                        .color(tokens.text_secondary),
                );
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::multiline(&mut next)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            },
        );
        match choice {
            Some(primitives::DialogChoice::Confirm) if !next.trim().is_empty() => {
                self.client.send(Request::UpdateMemory {
                    id,
                    content: next.trim().to_owned(),
                });
                self.editing_memory = None;
            }
            Some(primitives::DialogChoice::Cancel) => self.editing_memory = None,
            _ => self.editing_memory = Some((id, next)),
        }
    }

    // ── MCP servers ───────────────────────────────────────────────────

    /// What this server's tools are allowed to do without being asked.
    ///
    /// This is the sentence that makes PurrCode's MCP surface different from
    /// "did the server connect". A trusted tool runs without a per-call
    /// prompt; a denied one cannot run at all. Both are stated here, because
    /// a user who cannot see which tools are pre-approved has not really
    /// approved them.
    fn mcp_trust_summary(&self, ui: &mut Ui, server: &Value) {
        let names = |key: &str| {
            array(server, key)
                .into_iter()
                .filter_map(|tool| tool.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        let trusted = names("trusted_tools");
        let denied = names("deny_tools");
        // Two independent facts, stated independently — a 2x2 match over them
        // spelled the same two sentences four times.
        let trust = if trusted.is_empty() {
            "Every tool asks before it runs.".to_owned()
        } else {
            format!("Runs without asking: {}.", trusted.join(", "))
        };
        let deny = if denied.is_empty() {
            "No tool is denied.".to_owned()
        } else {
            format!("Denied: {}.", denied.join(", "))
        };
        let summary = format!("{trust} {deny}");
        row_note(
            ui,
            if trusted.is_empty() {
                self.tokens.text_muted
            } else {
                // Pre-approved tools are a standing grant, so the line that
                // describes them is not muted chrome.
                self.tokens.status_warning
            },
            &summary,
        );
    }

    /// The result of the last connection test for one server.
    fn mcp_test_report(&self, ui: &mut Ui, id: &str) {
        let Some(report) = self.settings_state.mcp_tests.get(id) else {
            return;
        };
        let connected = boolean(report, "connected");
        let count = number(report, "tool_count");
        let diagnostics = text_or(report, "diagnostics", "");
        row_note(
            ui,
            if connected {
                self.tokens.status_success
            } else {
                self.tokens.status_error
            },
            &if connected {
                format!("Connected · {count} tool(s) discovered")
            } else {
                format!("Not reachable · {diagnostics}")
            },
        );
        if !connected {
            return;
        }
        // The tool names themselves, so "12 tools" can be checked rather than
        // taken on faith — and so the user can see what they would be
        // trusting before they trust it.
        let names = array(report, "tools")
            .iter()
            .filter_map(|tool| text(tool, "name"))
            .collect::<Vec<_>>();
        if !names.is_empty() {
            row_note(ui, self.tokens.text_muted, &names.join(", "));
        }
    }

    fn settings_mcp(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "MCP servers",
            "Configure external tool servers. Only environment variable *names* are shown or entered — never secrets.",
        );

        let servers = self.settings_state.mcp_servers.clone();
        let server_count = servers.as_object().map(|map| map.len()).unwrap_or(0);
        self.settings_status(
            ui,
            &format!(
                "{server_count} MCP server(s) configured. Restarting the daemon preserves them."
            ),
        );

        let has_session = self.selected.is_some();
        if !has_session {
            note(
                ui,
                &tokens,
                "Discovery probes run against a selected session. Select one to probe a server.",
            );
            ui.add_space(primitives::SECTION_GAP);
        }

        if self.control_matches(&["server", "probe", "remove", "network"]) {
            primitives::section(
                ui,
                &tokens,
                "Configured servers",
                Some(
                    "What each server runs, where it runs, and which environment variable names it inherits.",
                ),
                |ui| {
                    let Some(map) = servers.as_object() else {
                        empty_state(ui, &tokens, "No MCP servers are configured yet.");
                        return;
                    };
                    if map.is_empty() {
                        empty_state(ui, &tokens, "No MCP servers are configured yet.");
                        return;
                    }
                    let mut remove: Option<String> = None;
                    let mut probe: Option<String> = None;
                    let mut test: Option<String> = None;
                    ui.push_id("mcp_servers", |ui| {
                        for (id, server) in map {
                            let transport = match text_or(server, "transport", "stdio").as_str() {
                                "http" => format!("HTTP {}", text_or(server, "url", "?")),
                                other => other.to_owned(),
                            };
                            let program = text_or(server, "program", "?");
                            let args = array(server, "arguments")
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" ");
                            let cwd = text_or(server, "working_directory", "?");
                            let network = boolean(server, "network");
                            let timeout = number(server, "timeout_seconds");
                            let env = server
                                .get("environment_from")
                                .and_then(Value::as_object)
                                .map(|map| map.keys().cloned().collect::<Vec<_>>().join(", "))
                                .unwrap_or_else(|| "none".to_owned());
                            let meta = format!("{program} {args}").trim_end().to_owned();
                            let asked = primitives::list_row(
                                ui,
                                &tokens,
                                RowSpec::new(id).meta(&meta),
                                |ui| {
                                    let mut asked: Option<McpAction> = None;
                                    if primitives::button(ui, &tokens, Tone::Danger, "Remove")
                                        .clicked()
                                    {
                                        asked = Some(McpAction::Remove);
                                    }
                                    if primitives::button_enabled(
                                        ui,
                                        &tokens,
                                        Tone::Secondary,
                                        "Probe",
                                        has_session,
                                    )
                                    .on_hover_text("Discover this server's tools via the session")
                                    .clicked()
                                    {
                                        asked = Some(McpAction::Probe);
                                    }
                                    // Unlike Probe, this needs no session: a
                                    // server has to be checkable while it is
                                    // being set up, which is the moment the
                                    // configuration is most likely wrong.
                                    if primitives::button(ui, &tokens, Tone::Secondary, "Test")
                                        .on_hover_text("Connect now and list this server's tools")
                                        .clicked()
                                    {
                                        asked = Some(McpAction::Test);
                                    }
                                    asked
                                },
                            )
                            .inner;
                            row_note(
                                ui,
                                tokens.text_secondary,
                                &format!(
                                    "{transport} · {cwd} · network {} · timeout {timeout}s",
                                    if network { "on" } else { "off" }
                                ),
                            );
                            row_note(
                                ui,
                                tokens.text_muted,
                                &format!("Environment variables: {env}"),
                            );
                            // Trust is the point of this surface. A server
                            // with auto-approved tools is materially different
                            // from one where every call is asked about, so the
                            // row says which tools those are rather than
                            // leaving it to a config file.
                            self.mcp_trust_summary(ui, server);
                            self.mcp_test_report(ui, id);
                            match asked {
                                Some(McpAction::Remove) => remove = Some(id.clone()),
                                Some(McpAction::Probe) => probe = Some(id.clone()),
                                Some(McpAction::Test) => test = Some(id.clone()),
                                None => {}
                            }
                        }
                    });
                    if let Some(id) = probe
                        && let Some(session) = self.selected.clone()
                    {
                        self.settings_state.mutation_sent(&format!("probe:{id}"));
                        self.client.send(Request::McpProbe {
                            session,
                            server: id.clone(),
                        });
                    }
                    if let Some(id) = remove {
                        self.settings_state
                            .mutation_sent(&format!("mcp_remove:{id}"));
                        self.client.send(Request::McpRemove { id });
                    }
                    if let Some(id) = test {
                        self.settings_state.mutation_sent(&format!("mcp_test:{id}"));
                        // Clear the previous report first: leaving a stale
                        // "connected, 12 tools" on screen while a new test runs
                        // claims the server is reachable before anyone asked.
                        self.settings_state.mcp_tests.remove(&id);
                        self.client.send(Request::McpTest { id });
                    }
                    if let Some(key) = self.settings_state.pending.clone()
                        && (key.starts_with("probe:") || key.starts_with("mcp_remove:"))
                    {
                        self.settings_inline_error(ui, &key);
                    }
                    let probe_result = self.settings_state.mcp_probe.clone();
                    if !probe_result.is_null() {
                        let tools = array(&probe_result, "tools")
                            .iter()
                            .filter_map(|tool| text(tool, "name"))
                            .collect::<Vec<_>>();
                        ui.add_space(GAP_CONTROL);
                        status_note(
                            ui,
                            tokens.text_secondary,
                            &format!(
                                "Last probe discovered {} tool(s){}.",
                                tools.len(),
                                if tools.is_empty() {
                                    String::new()
                                } else {
                                    format!(": {}", tools.join(", "))
                                }
                            ),
                        );
                    }
                },
            );
        }

        if self.control_matches(&["server", "add", "working directory", "environment"]) {
            let mut save = false;
            primitives::section(
                ui,
                &tokens,
                "Add a server",
                Some(
                    "Environment fields take variable *names* (e.g. $API_KEY), matching purrcode.toml.example. Secret values are rejected.",
                ),
                |ui| {
                    primitives::field_row(ui, &tokens, "Server id", |ui| {
                        let room = ui.available_width();
                        ui.add(egui::TextEdit::singleline(&mut self.mcp_id).desired_width(room));
                    });
                    primitives::field_row(ui, &tokens, "Transport", |ui| {
                        ui.selectable_value(&mut self.mcp_http, false, "stdio");
                        ui.selectable_value(&mut self.mcp_http, true, "HTTP");
                    });
                    // Only the fields the chosen transport actually uses are
                    // shown. A "Program" box on an HTTP server is a question
                    // with no right answer.
                    if self.mcp_http {
                        primitives::field_row(ui, &tokens, "URL", |ui| {
                            let room = ui.available_width();
                            ui.add(
                                egui::TextEdit::singleline(&mut self.mcp_url)
                                    .hint_text("https://host/mcp")
                                    .desired_width(room),
                            );
                        });
                    } else {
                        primitives::field_row(ui, &tokens, "Program", |ui| {
                            let room = ui.available_width();
                            ui.add(
                                egui::TextEdit::singleline(&mut self.mcp_program)
                                    .desired_width(room),
                            );
                        });
                        primitives::field_row(ui, &tokens, "Arguments", |ui| {
                            let room = ui.available_width();
                            ui.add(
                                egui::TextEdit::singleline(&mut self.mcp_arguments)
                                    .hint_text("Comma-separated")
                                    .desired_width(room),
                            );
                        });
                    }
                    primitives::field_row(ui, &tokens, "Working directory", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mcp_working_directory)
                                .desired_width(room),
                        );
                    });
                    primitives::field_row(ui, &tokens, "Network", |ui| {
                        ui.checkbox(&mut self.mcp_network, "Allows network access");
                    });
                    primitives::field_row(ui, &tokens, "Env var names", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mcp_environment)
                                .hint_text("KEY=$KEY, NAME=$NAME")
                                .desired_width(room),
                        );
                    });
                    // Trust is configured with the server, not after it: the
                    // moment somebody adds a tool server is the moment to say
                    // what it may do unattended.
                    primitives::field_row(ui, &tokens, "Run without asking", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mcp_trusted_tools)
                                .hint_text(
                                    "Comma-separated tool names — leave empty to ask every time",
                                )
                                .desired_width(room),
                        );
                    });
                    primitives::field_row(ui, &tokens, "Never run", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mcp_deny_tools)
                                .hint_text("Comma-separated tool names")
                                .desired_width(room),
                        );
                    });
                    ui.add_space(GAP_CONTROL);
                    let endpoint_given = if self.mcp_http {
                        !self.mcp_url.trim().is_empty()
                    } else {
                        !self.mcp_program.trim().is_empty()
                    };
                    let ready = !self.mcp_id.trim().is_empty()
                        && endpoint_given
                        && !self.mcp_working_directory.trim().is_empty();
                    save = primitives::button_enabled(
                        ui,
                        &tokens,
                        Tone::Primary,
                        "Save server",
                        ready,
                    )
                    .clicked();
                    if !ready {
                        ui.add_space(GAP_TIGHT);
                        note(
                            ui,
                            &tokens,
                            if self.mcp_http {
                                "A server id, a URL and a working directory are required."
                            } else {
                                "A server id, a program and a working directory are required."
                            },
                        );
                    }
                    self.settings_inline_error(ui, "mcp_save");
                },
            );
            if save {
                let mut environment_from = BTreeMap::new();
                for pair in self.mcp_environment.split(',') {
                    if let Some((child, host)) = pair.split_once('=') {
                        environment_from.insert(child.trim().to_owned(), host.trim().to_owned());
                    }
                }
                let tool_list = |raw: &str| {
                    raw.split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                };
                let server = serde_json::json!({
                    "id": self.mcp_id.trim().to_owned(),
                    "transport": if self.mcp_http { "http" } else { "stdio" },
                    "url": self.mcp_url.trim().to_owned(),
                    "trusted_tools": tool_list(&self.mcp_trusted_tools),
                    "deny_tools": tool_list(&self.mcp_deny_tools),
                    "program": self.mcp_program.trim().to_owned(),
                    "arguments": tool_list(&self.mcp_arguments),
                    "working_directory": self.mcp_working_directory.trim().to_owned(),
                    "network": self.mcp_network,
                    "environment_from": environment_from,
                });
                self.settings_state.mutation_sent("mcp_save");
                self.client.send(Request::McpUpsert { server });
            }
        }
    }

    // ── Codex ─────────────────────────────────────────────────────────

    fn settings_codex(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Codex",
            "Link the OpenAI Codex CLI as an execution adapter.",
        );

        let codex = self.settings_state.codex.clone();
        let enabled = boolean(&codex, "enabled");
        self.settings_status(
            ui,
            if enabled {
                "Codex is enabled."
            } else {
                "Codex is disabled."
            },
        );

        if !self.control_matches(&["codex", "enable", "toggle", "binary", "auth", "worktree"]) {
            return;
        }

        // ── Enable / disable ──────────────────────────────────────────
        let mut toggle = false;
        primitives::section(
            ui,
            &tokens,
            "Adapter",
            Some(
                "Codex runs in a worktree of its own; enabling it does not hand it the active tree.",
            ),
            |ui| {
                toggle = primitives::button_enabled(
                    ui,
                    &tokens,
                    if enabled {
                        Tone::Secondary
                    } else {
                        Tone::Primary
                    },
                    if enabled {
                        "Disable Codex"
                    } else {
                        "Enable Codex"
                    },
                    !codex.is_null(),
                )
                .clicked();
                if codex.is_null() {
                    ui.add_space(GAP_TIGHT);
                    note(
                        ui,
                        &tokens,
                        "The daemon has not reported a Codex configuration yet.",
                    );
                }
                self.settings_inline_error(ui, "codex_enable");
            },
        );
        if toggle {
            let mut config = codex.clone();
            config["enabled"] = Value::Bool(!enabled);
            self.settings_state.mutation_sent("codex_enable");
            self.client.send(Request::CodexPut { config });
        }

        // ── Configuration ─────────────────────────────────────────────
        if !codex.is_null() {
            let binary = text_or(&codex, "binary", "codex");
            let execution_mode = text_or(&codex, "execution_mode", "worktree");
            let timeout = number(&codex, "timeout_seconds");
            let inherit_auth = boolean(&codex, "inherit_auth");
            let require_diff = boolean(&codex, "require_final_diff_judgment");
            let allow_tree_write = boolean(&codex, "allow_active_tree_write");
            if self.codex_binary.trim().is_empty() {
                self.codex_binary = binary.clone();
            }
            if self.codex_timeout == 0 {
                self.codex_timeout = timeout.max(3600);
            }

            let mut save = false;
            primitives::section(
                ui,
                &tokens,
                "Adapter configuration",
                Some(
                    "Saved to the daemon, which re-verifies every authorization before Codex runs.",
                ),
                |ui| {
                    primitives::field_row(ui, &tokens, "Binary", |ui| {
                        let room = ui.available_width();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.codex_binary)
                                .hint_text(&binary)
                                .desired_width(room),
                        );
                    });
                    primitives::field_row(ui, &tokens, "Execution mode", |ui| {
                        // Read-only: the worktree boundary is the adapter's
                        // safety property, not a preference.
                        value_line(ui, tokens.text_secondary, &execution_mode);
                    });
                    primitives::field_row(ui, &tokens, "Timeout (seconds)", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.codex_timeout)
                                .range(30..=86_400)
                                .speed(60),
                        );
                    });
                    primitives::field_row(ui, &tokens, "Auth", |ui| {
                        ui.checkbox(
                            &mut self.codex_inherit_auth,
                            "Inherit the logged-in session",
                        );
                    });
                    primitives::field_row(ui, &tokens, "Diff judgment", |ui| {
                        ui.checkbox(
                            &mut self.codex_require_diff,
                            "Require an independent final diff judgment",
                        );
                    });
                    primitives::field_row(ui, &tokens, "Active tree", |ui| {
                        ui.checkbox(
                            &mut self.codex_allow_tree_write,
                            "Allow writing the active working tree",
                        );
                    });
                    ui.add_space(GAP_CONTROL);
                    let changed = self.codex_binary.trim() != binary
                        || self.codex_timeout != timeout.max(3600)
                        || self.codex_inherit_auth != inherit_auth
                        || self.codex_require_diff != require_diff
                        || self.codex_allow_tree_write != allow_tree_write;
                    save = primitives::button_enabled(
                        ui,
                        &tokens,
                        Tone::Primary,
                        "Save Codex configuration",
                        changed,
                    )
                    .clicked();
                    if !changed {
                        ui.add_space(GAP_TIGHT);
                        note(ui, &tokens, "Nothing has changed since the daemon's copy.");
                    }
                    self.settings_inline_error(ui, "codex_save");
                },
            );
            if save {
                let config = serde_json::json!({
                    "enabled": enabled,
                    "binary": self.codex_binary.trim().to_owned(),
                    "execution_mode": "worktree",
                    "timeout_seconds": self.codex_timeout,
                    "inherit_auth": self.codex_inherit_auth,
                    "require_final_diff_judgment": self.codex_require_diff,
                    "allow_active_tree_write": self.codex_allow_tree_write,
                });
                self.settings_state.mutation_sent("codex_save");
                self.client.send(Request::CodexPut { config });
            }
        }

        // ── Check Codex ───────────────────────────────────────────────
        let mut check = false;
        primitives::section(
            ui,
            &tokens,
            "Check Codex",
            Some("Runs the doctor: version, adapter, authentication and the event contract."),
            |ui| {
                check = primitives::button(ui, &tokens, Tone::Secondary, "Check Codex").clicked();
                // FR-A6: if the binary is missing, the doctor error names the
                // path that was tried rather than leaving a bare failure.
                if let Some(error) = self.settings_state.error("codex_doctor") {
                    let binary = text_or(&codex, "binary", "codex");
                    let lower = error.to_ascii_lowercase();
                    let missing = lower.contains("no such file")
                        || lower.contains("not found")
                        || lower.contains("os error 2")
                        || lower.contains("exec")
                        || lower.contains("spawn");
                    if missing {
                        ui.add_space(GAP_TIGHT);
                        status_note(
                            ui,
                            tokens.status_error,
                            &format!(
                                "The Codex binary at `{binary}` was not found or could not run. Install Codex (or set the correct path above) and check again."
                            ),
                        );
                    }
                }
                self.settings_inline_error(ui, "codex_doctor");

                let doctor = self.settings_state.codex_doctor.clone();
                if doctor.is_null() {
                    return;
                }
                ui.add_space(GAP_GROUP);
                status_note(
                    ui,
                    tokens.text_secondary,
                    &format!(
                        "Codex {} · adapter {} · {}",
                        text_or(&doctor, "version", "?"),
                        text_or(&doctor, "adapter", "?"),
                        if boolean(&doctor, "authenticated") {
                            "authenticated"
                        } else {
                            "not authenticated"
                        }
                    ),
                );
                note(
                    ui,
                    &tokens,
                    &format!(
                        "JSON events: {} · non-interactive: {} · worktree only: {}",
                        boolean(&doctor, "json_events"),
                        boolean(&doctor, "noninteractive"),
                        text_or(&doctor, "worktree_only", "?")
                    ),
                );
            },
        );
        if check {
            self.settings_state.mutation_sent("codex_doctor");
            self.client.send(Request::CodexDoctor);
        }
    }

    // ── Authority / Agent / Terminal / Privacy / Advanced ─────────────

    fn settings_authority(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Authority & permissions",
            "Execution approval and orchestration are separate controls.",
        );
        self.settings_status(
            ui,
            "Shown for the selected session; a new session uses the workspace default.",
        );
        if self.control_matches(&["permission", "approval", "pawgate", "authority", "mode"]) {
            let mode = self.session.permission_mode.clone();
            primitives::section(
                ui,
                &tokens,
                "Effective for this session",
                Some(
                    "Reported by the daemon. The IDE does not keep a permission state of its own.",
                ),
                |ui| {
                    primitives::field_row(ui, &tokens, "Permission mode", |ui| {
                        value_line(ui, tokens.accent_primary, &mode);
                    });
                    ui.add_space(GAP_CONTROL);
                    note(
                        ui,
                        &tokens,
                        "Every native tool action still requires a durable PawGate judgment, and the execution adapter re-verifies the exact authorization before running it.",
                    );
                },
            );
        }
    }

    fn settings_agent(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Agent behavior",
            "The daemon selects a direct, standard, or rigorous workflow from your intent.",
        );
        self.settings_status(ui, "Reported by the selected session's controls.");
        if self.control_matches(&["workflow", "budget", "search", "routing", "agent", "plan"]) {
            let controls = self.session.controls.clone();
            primitives::section(
                ui,
                &tokens,
                "Resolved controls",
                Some(
                    "What the daemon chose for this session, not a set of switches to pre-empt it with.",
                ),
                |ui| {
                    for (label, value) in [
                        ("Workflow", controls.workflow.as_str()),
                        ("Search", controls.search.as_str()),
                        ("Budget", controls.budget.as_str()),
                        ("Routing", controls.routing.as_str()),
                    ] {
                        primitives::field_row(ui, &tokens, label, |ui| {
                            value_line(ui, tokens.text_primary, value);
                        });
                    }
                    ui.add_space(GAP_CONTROL);
                    note(
                        ui,
                        &tokens,
                        "New sessions use Auto; a greeting does not become a build plan.",
                    );
                },
            );
        }
    }

    fn settings_terminal(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Terminal & Git",
            "Terminal ownership, source control and delivery remain workspace-scoped.",
        );
        self.settings_status(ui, "Status for the open workspace.");
        if self.control_matches(&["terminal", "git", "branch", "github", "shell"]) {
            let branch = self
                .workspace_state
                .branch
                .clone()
                .unwrap_or_else(|| "Unavailable".to_owned());
            let terminals = self.terminals.len();
            let remote = self.workspace_state.github_remote_configured;
            primitives::section(
                ui,
                &tokens,
                "This workspace",
                Some("Read from the open folder. Nothing here changes the repository."),
                |ui| {
                    primitives::field_row(ui, &tokens, "Branch", |ui| {
                        value_line(ui, tokens.text_primary, &branch);
                    });
                    primitives::field_row(ui, &tokens, "Open terminals", |ui| {
                        value_line(ui, tokens.text_primary, &terminals.to_string());
                    });
                    primitives::field_row(ui, &tokens, "GitHub remote", |ui| {
                        value_line(
                            ui,
                            if remote {
                                tokens.text_primary
                            } else {
                                tokens.text_muted
                            },
                            if remote {
                                "Configured"
                            } else {
                                "Not configured"
                            },
                        );
                    });
                    ui.add_space(GAP_CONTROL);
                    note(
                        ui,
                        &tokens,
                        if remote {
                            "A GitHub remote is configured; authentication is not assumed."
                        } else {
                            "No GitHub remote is configured."
                        },
                    );
                },
            );
        }
    }

    fn settings_privacy(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Privacy & recovery",
            "Credentials stay outside model context, events and child-process environments.",
        );
        self.settings_status(ui, "Durable guarantees; no credentials are stored here.");
        if self.control_matches(&["privacy", "credential", "recovery", "history", "evidence"]) {
            primitives::section(
                ui,
                &tokens,
                "What is guaranteed",
                Some(
                    "These are properties of the runtime, not options — there is nothing here to switch off.",
                ),
                |ui| {
                    for line in [
                        "Session history and evidence are durable.",
                        "Recovery never silently replays or discards uncertain effects.",
                        "A stored secret is never displayed by this surface, only referenced by name.",
                    ] {
                        ui.label(
                            RichText::new(line)
                                .size(theme::TYPE_BODY)
                                .line_height(Some(theme::BODY_LINE_HEIGHT))
                                .color(tokens.text_secondary),
                        );
                    }
                },
            );
        }
    }

    fn settings_advanced(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.settings_heading(
            ui,
            "Advanced",
            "Inspect connectivity and contract details without exposing credentials.",
        );
        self.settings_status(ui, "Diagnostics stay on this machine.");
        if self.control_matches(&[
            "advanced",
            "diagnostics",
            "connectivity",
            "details",
            "debug",
        ]) {
            let mut open = false;
            primitives::section(
                ui,
                &tokens,
                "Diagnostics",
                Some("Connectivity, contract versions and the raw bootstrap, in their own window."),
                |ui| {
                    open = primitives::button(ui, &tokens, Tone::Secondary, "Open diagnostics")
                        .clicked();
                },
            );
            if open {
                self.diagnostics_open = true;
                self.settings_open = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Appearance;

    /// The id from the screenshot the rebuild was ordered from.
    const LONG_MODEL_ID: &str = "integrate.api.nvidia.com/nvidia/nemotron-3-ultra-550b-a55b";

    /// Enough width to read a middle-elided model id: the host, the ellipsis
    /// and the part of the name that says which model this is.
    const IDENTITY_FLOOR: f32 = 140.0;

    /// The horizontal padding `primitives::section` puts around its body. The
    /// width a field row or a list row actually sees is the content column
    /// minus this, which is what the narrow-window assertions have to reason
    /// about.
    const SECTION_INSET: f32 = 28.0;

    fn frame(width: f32, mut body: impl FnMut(&mut Ui, &Tokens)) {
        let ctx = egui::Context::default();
        theme::install(&ctx, Appearance::Dark);
        let tokens = Tokens::for_appearance(Appearance::Dark);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(width, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    ui.set_width(width);
                    body(ui, &tokens);
                });
        });
    }

    #[test]
    fn settings_columns_leave_a_gap_without_overflowing() {
        let layout = settings_layout(920.0);
        assert!(!layout.compact);
        assert!(layout.nav_width >= SETTINGS_NAV_MIN_WIDTH);
        assert!(layout.nav_width <= SETTINGS_NAV_MAX_WIDTH);
        assert_eq!(
            layout.nav_width + SETTINGS_COLUMN_GAP + layout.content_width,
            920.0
        );
    }

    #[test]
    fn settings_switch_to_reading_order_below_breakpoint() {
        let layout = settings_layout(640.0);
        assert!(layout.compact);
        assert_eq!(layout.nav_width, layout.content_width);
    }

    #[test]
    fn search_matches_controls_not_just_page_labels() {
        // "provider" is a control keyword, not any page label.
        assert!(SettingsPage::Models.matches_query("provider"));
        assert!(SettingsPage::Models.matches_query("base url"));
        // "qualify" lives on Local models.
        assert!(SettingsPage::LocalModels.matches_query("qualify"));
        assert!(SettingsPage::LocalModels.matches_query("idle timeout"));
        // "codex" matches by label.
        assert!(SettingsPage::Codex.matches_query("codex"));
        // A nonsense query matches nothing.
        assert!(!SettingsPage::General.matches_query("zzz"));
    }

    #[test]
    fn search_finds_settings_that_are_not_pages() {
        // A query that names a control (not a page label) still surfaces a page.
        let query = "credential";
        assert!(
            SettingsPage::ALL
                .iter()
                .any(|page| page.matches_query(query)),
            "credential should find Models & providers"
        );
    }

    #[test]
    fn a_mutation_error_is_recorded_inline_and_cleared_on_success() {
        let mut state = SettingsState::default();
        state.mutation_sent("configure");
        // A Failed response lands on the pending key.
        state
            .errors
            .insert("configure".into(), "provider profile already exists".into());
        assert_eq!(
            state.error("configure"),
            Some("provider profile already exists")
        );
        // Re-using the control clears the stale error.
        state.mutation_sent("configure");
        assert_eq!(state.error("configure"), None);
        state.mutation_succeeded();
        assert_eq!(state.pending, None);
    }

    #[test]
    fn settings_navigation_groups_cover_the_new_surfaces() {
        // The PRD §1.5 groups, with Local models / Skills / MCP / Codex present.
        assert_eq!(SettingsPage::Models.group(), "MODELS");
        assert_eq!(SettingsPage::LocalModels.group(), "MODELS");
        assert_eq!(SettingsPage::Skills.group(), "EXTENSIONS");
        assert_eq!(SettingsPage::Mcp.group(), "EXTENSIONS");
        assert_eq!(SettingsPage::Codex.group(), "EXTENSIONS");
        assert_eq!(SettingsPage::Authority.group(), "RUNTIME");
        assert_eq!(SettingsPage::Advanced.group(), "SYSTEM");
    }

    #[test]
    fn every_page_still_carries_control_keywords_for_the_search() {
        // FR-A7 is a property of the data, not of one page: a rebuild that
        // dropped a page's keyword list would make its controls unfindable
        // without breaking anything a layout test would notice.
        for page in SettingsPage::ALL {
            assert!(
                !page.keywords().is_empty(),
                "{} has no control keywords",
                page.label()
            );
            for keyword in page.keywords() {
                assert_eq!(
                    *keyword,
                    keyword.to_ascii_lowercase(),
                    "{keyword} is matched case-insensitively against a lowercased query"
                );
            }
        }
    }

    #[test]
    fn the_label_column_survives_the_narrowest_settings_window() {
        // Labels align on one pixel at every size the window can be dragged to,
        // not only at the default. The card pads itself, so the width a field
        // row really sees is the content column minus the section's inset.
        let content = settings_layout(SETTINGS_MIN_WIDTH).content_width;
        match primitives::field_layout(content - SECTION_INSET) {
            primitives::FieldLayout::Columns { label, control } => {
                assert_eq!(label, primitives::LABEL_COLUMN);
                assert!(control > 0.0);
            }
            primitives::FieldLayout::Stacked => {
                panic!("the minimum settings width should still afford two columns")
            }
        }
    }

    #[test]
    fn labelled_fields_share_one_control_column() {
        // The provider form's own labels, plus the longest label on the surface.
        // A wordy label must be elided rather than allowed to shove its control
        // sideways and misalign the page under it.
        let content = settings_layout(SETTINGS_DEFAULT_WIDTH).content_width;
        frame(content, |ui, tokens| {
            let mut lefts = Vec::new();
            for label in [
                "API key",
                "Base URL",
                "Model ID",
                "Working directory",
                "Env var names that are far too long for the column",
            ] {
                let rect = primitives::field_row(ui, tokens, label, |ui| {
                    let mut value = String::new();
                    let room = ui.available_width();
                    ui.add(egui::TextEdit::singleline(&mut value).desired_width(room));
                    ui.min_rect()
                });
                lefts.push(rect.left());
            }
            let first = lefts[0];
            for left in &lefts {
                assert!(
                    (left - first).abs() <= 0.5,
                    "controls start at {lefts:?}, which is not one column"
                );
            }
        });
    }

    #[test]
    fn a_model_id_and_its_actions_never_share_a_pixel_in_the_narrowest_window() {
        // The defect this page was rebuilt for, held at the smallest window the
        // user can drag Settings to: the action group is measured where it is
        // drawn, and the identity is fitted into what is left with a full gap
        // between them.
        let content = settings_layout(SETTINGS_MIN_WIDTH).content_width;
        frame(content - SECTION_INSET, |ui, tokens| {
            let row = primitives::list_row(
                ui,
                tokens,
                RowSpec::new(LONG_MODEL_ID).meta("Remote · No assigned role"),
                |ui| {
                    primitives::button_enabled(ui, tokens, Tone::Secondary, "Make default", true);
                    primitives::button(ui, tokens, Tone::Primary, "Assign");
                    egui::ComboBox::from_id_salt("model_role_probe")
                        .selected_text("coding_worker")
                        .width(ROLE_PICKER_WIDTH)
                        .show_ui(ui, |_| {});
                    ui.min_rect()
                },
            );
            let actions = row.inner;
            let outer = row.response.rect;
            assert!(
                actions.width() > 0.0 && actions.left() > outer.left(),
                "the action group was not measured"
            );

            let columns = primitives::RowColumns::solve(
                outer.width(),
                outer.right() - actions.left(),
                primitives::ACTION_GAP,
            );
            assert!(
                columns.identity >= IDENTITY_FLOOR,
                "the role row's actions leave the model id only {}px at the \
                 narrowest settings window",
                columns.identity
            );
            assert!(
                outer.left() + columns.identity + primitives::ACTION_GAP <= actions.left() + 1.0,
                "the identity column reaches into the action column"
            );
        });
    }

    #[test]
    fn the_navigation_pills_fill_their_column_without_overflowing_it() {
        // The selection pill is painted across the whole column, so a page
        // label can never make one entry wider than its neighbour — which is
        // what an accent pill would make obvious.
        frame(SETTINGS_NAV_MIN_WIDTH, |ui, tokens| {
            let width = ui.available_width();
            for page in SettingsPage::ALL {
                let rect = nav_item(ui, tokens, page.label(), *page == SettingsPage::Models).rect;
                assert!(
                    (rect.width() - width).abs() <= 0.5,
                    "{} claimed {}px of a {width}px column",
                    page.label(),
                    rect.width()
                );
                assert_eq!(rect.height(), NAV_ITEM_HEIGHT);
            }
        });
    }
}
