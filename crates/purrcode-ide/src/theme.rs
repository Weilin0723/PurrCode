//! The PurrCode design tokens, expressed for egui.
//!
//! One token source drives both themes (PRD §26). Views never name a literal
//! colour: they ask for a semantic role, so a state that means "warning" looks
//! right in dark, light and high contrast without three copies of the layout.
//!
//! The dark set is the reference design, and it is deliberately a *workbench*
//! palette rather than a chat palette: a blue-black neutral shell, one cat-eye
//! cyan accent drawn from the mascot, and surfaces that step up in three small
//! accent, and surfaces that step up in three small increments (chrome →
//! canvas → card) instead of one large one. Contrast comes from type weight and
//! a single hairline border, not from boxing every region in its own colour.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Visuals,
};
use purrcode_runtime_core::StateColor;

// ── Geometry ───────────────────────────────────────────────────────────
//
// Named so every surface rounds and sizes the same way. A card that picks its
// own radius is what makes an interface look assembled rather than designed.

/// Buttons, chips, rows, tabs.
pub const RADIUS_CONTROL: u8 = 7;
/// Cards, the composer, popovers.
pub const RADIUS_CARD: u8 = 10;
/// Fully rounded — session pills, status chips.
pub const RADIUS_PILL: u8 = 99;

/// The height of a list row (session, file, search hit).
pub const ROW_HEIGHT: f32 = 28.0;
/// The command bar at the very top of the window.
pub const TITLE_BAR_HEIGHT: f32 = 52.0;
/// The editor tab strip.
pub const TAB_BAR_HEIGHT: f32 = 36.0;
/// The breadcrumb line under the tab strip.
pub const BREADCRUMB_HEIGHT: f32 = 26.0;
/// The vertical activity rail.
pub const RAIL_WIDTH: f32 = 60.0;
/// A rail item's hit target.
pub const RAIL_ITEM: f32 = 40.0;
/// The status line at the very bottom.
pub const STATUS_BAR_HEIGHT: f32 = 26.0;
/// Room for the macOS traffic lights when the window draws its own title bar.
pub const TRAFFIC_LIGHT_INSET: f32 = 78.0;

// ── Typography ────────────────────────────────────────────────────────
//
// Keep the native UI on a short, deliberate scale. Views may use these
// semantic roles for the occasional hand-painted label, but the regular
// widgets resolve to the same values through `install` below.
/// Secondary labels, breadcrumbs and status text.
pub const TYPE_META: f32 = 11.0;
/// Uppercase section eyebrows and tracked-cap headers. One step below META so
/// the two roles stay distinct, and every surface that writes "AGENT",
/// "RECENT", "START WITH" or a dock tab label draws from the same token.
pub const TYPE_EYEBROW: f32 = 10.5;
/// Compact controls and tab labels.
pub const TYPE_LABEL: f32 = 12.0;
/// Default body copy and readable form text. 13px was too small for dense
/// prose; 14 sits comfortably while keeping the ramp to LABEL distinct.
pub const TYPE_BODY: f32 = 14.0;
/// Section titles and product names.
pub const TYPE_TITLE: f32 = 20.0;
/// A single display-size statement, used sparingly for empty states.
pub const TYPE_DISPLAY: f32 = 24.0;
/// Code and terminal text.
pub const TYPE_CODE: f32 = 12.0;

/// The widest an agent prose column gets. 70pt of the SF face at 14pt is
/// roughly 72 characters — the far end of the 45–75ch measure both reference
/// rule-sets insist on. Code blocks and diffs are exempt: they keep the full
/// column and stay above the cap, because a wrapped code line is a lie.
pub const READING_MEASURE: f32 = 700.0;

/// The line height for meta text (notes, hints, row meta): one deliberate step
/// above the font's natural row height, so a column of notes reads as airy
/// prose rather than stacked lines.
pub const META_LINE_HEIGHT: f32 = 16.0;
/// The line height for body copy that can wrap: prose and form helpers get a
/// comfortable leading step, keeping paragraphs scannable at the larger body
/// size instead of wall-to-wall lines. 20 on a 14px body is a deliberate 1.43×,
/// clearly above the browser-style defaults the wrapped prose used to inherit.
pub const BODY_LINE_HEIGHT: f32 = 20.0;
/// Extra letter spacing for uppercase eyebrows (the settings header, the
/// navigation groups), so a tracked-cap label reads as a composed header
/// rather than shouting text.
pub const EYEBROW_SPACING: f32 = 1.6;

/// The display face at a size drawn from the type scale. One name for the
/// family, so a view never has to remember the string or retype a size number.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("purrcode_display".into()))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    HighContrast,
}

impl Appearance {
    pub const ALL: &'static [Self] = &[Self::Dark, Self::Light, Self::HighContrast];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::HighContrast => "High contrast",
        }
    }
}

/// Blend `b` into `a` by `t` (0 → `a`, 1 → `b`), in straight sRGB.
///
/// Used to derive tinted surfaces from a base at build time, so nothing on
/// screen relies on alpha blending against whatever happens to be behind it.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// The semantic palette. The dark values are the reference design; the light
/// and high-contrast sets are held to the same role names so nothing downstream
/// has to branch on the theme.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    /// The canvas: the editor and the agent transcript.
    pub background_primary: Color32,
    /// The chrome: rail, sidebar, title bar, status bar, tab strip.
    pub background_secondary: Color32,
    /// Cards, the composer, popovers — one step above the canvas.
    pub background_raised: Color32,
    /// Row/tab hover.
    pub surface_hover: Color32,
    /// Row/tab selection, when the selection is not accent-tinted.
    pub surface_active: Color32,
    /// The hairline that separates two regions.
    pub border_subtle: Color32,
    /// The border of a raised card, and of a focused control.
    pub border_strong: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub accent_primary: Color32,
    pub accent_hover: Color32,
    /// A surface tinted with the accent: selected rail items, active tabs,
    /// the user's own messages.
    pub accent_soft: Color32,
    /// Text and icons drawn on top of a solid `accent_primary` fill.
    pub accent_on: Color32,
    pub status_success: Color32,
    pub status_warning: Color32,
    pub status_error: Color32,
    pub status_info: Color32,
    pub status_running: Color32,
    /// Diff *surfaces* — a whole added or removed line.
    pub diff_added: Color32,
    pub diff_removed: Color32,
    /// Diff *text* — a "+12 / −4" counter. Never the surface colour: a
    /// background tint used as a label is a label nobody can read.
    pub diff_added_text: Color32,
    pub diff_removed_text: Color32,
}

impl Tokens {
    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self {
                background_primary: Color32::from_rgb(0x0f, 0x14, 0x18),
                background_secondary: Color32::from_rgb(0x09, 0x0e, 0x12),
                background_raised: Color32::from_rgb(0x17, 0x1d, 0x22),
                surface_hover: Color32::from_rgb(0x20, 0x28, 0x2f),
                surface_active: Color32::from_rgb(0x27, 0x33, 0x3b),
                border_subtle: Color32::from_rgb(0x26, 0x30, 0x38),
                border_strong: Color32::from_rgb(0x3a, 0x49, 0x53),
                text_primary: Color32::from_rgb(0xec, 0xf3, 0xf6),
                text_secondary: Color32::from_rgb(0xa7, 0xb4, 0xbb),
                text_muted: Color32::from_rgb(0x7d, 0x90, 0x9a),
                accent_primary: Color32::from_rgb(0x38, 0xbd, 0xf8),
                accent_hover: Color32::from_rgb(0x72, 0xd2, 0xff),
                accent_soft: Color32::from_rgb(0x10, 0x31, 0x40),
                accent_on: Color32::from_rgb(0x05, 0x12, 0x18),
                status_success: Color32::from_rgb(0x47, 0xc7, 0x8a),
                status_warning: Color32::from_rgb(0xf2, 0xa0, 0x72),
                status_error: Color32::from_rgb(0xff, 0x72, 0x72),
                status_info: Color32::from_rgb(0x58, 0xa6, 0xff),
                status_running: Color32::from_rgb(0x38, 0xbd, 0xf8),
                diff_added: Color32::from_rgb(0x10, 0x2b, 0x22),
                diff_removed: Color32::from_rgb(0x32, 0x1b, 0x1d),
                diff_added_text: Color32::from_rgb(0x47, 0xc7, 0x8a),
                diff_removed_text: Color32::from_rgb(0xff, 0x72, 0x72),
            },
            Appearance::Light => Self {
                background_primary: Color32::from_rgb(0xf9, 0xfb, 0xfc),
                background_secondary: Color32::from_rgb(0xef, 0xf4, 0xf6),
                background_raised: Color32::from_rgb(0xff, 0xff, 0xff),
                surface_hover: Color32::from_rgb(0xe7, 0xef, 0xf2),
                surface_active: Color32::from_rgb(0xda, 0xe7, 0xec),
                border_subtle: Color32::from_rgb(0xd9, 0xe3, 0xe8),
                border_strong: Color32::from_rgb(0xbd, 0xcc, 0xd3),
                text_primary: Color32::from_rgb(0x12, 0x1b, 0x20),
                text_secondary: Color32::from_rgb(0x45, 0x55, 0x5e),
                text_muted: Color32::from_rgb(0x66, 0x76, 0x7e),
                accent_primary: Color32::from_rgb(0x08, 0x70, 0x96),
                accent_hover: Color32::from_rgb(0x05, 0x5e, 0x7f),
                accent_soft: Color32::from_rgb(0xd9, 0xf1, 0xfa),
                accent_on: Color32::from_rgb(0xff, 0xff, 0xff),
                status_success: Color32::from_rgb(0x08, 0x70, 0x4a),
                status_warning: Color32::from_rgb(0xa8, 0x43, 0x16),
                status_error: Color32::from_rgb(0xb4, 0x2b, 0x2b),
                status_info: Color32::from_rgb(0x17, 0x5c, 0xd3),
                status_running: Color32::from_rgb(0x08, 0x70, 0x96),
                diff_added: Color32::from_rgb(0xe6, 0xf6, 0xec),
                diff_removed: Color32::from_rgb(0xfd, 0xec, 0xec),
                diff_added_text: Color32::from_rgb(0x06, 0x76, 0x47),
                diff_removed_text: Color32::from_rgb(0xb4, 0x23, 0x18),
            },
            // Pure black/white with saturated status hues, for users who need
            // the maximum separation the display can produce (PRD §27).
            Appearance::HighContrast => Self {
                background_primary: Color32::BLACK,
                background_secondary: Color32::BLACK,
                background_raised: Color32::from_rgb(0x14, 0x14, 0x14),
                surface_hover: Color32::from_rgb(0x24, 0x24, 0x24),
                surface_active: Color32::from_rgb(0x30, 0x30, 0x30),
                border_subtle: Color32::from_rgb(0x9a, 0x9a, 0x9a),
                border_strong: Color32::WHITE,
                text_primary: Color32::WHITE,
                text_secondary: Color32::from_rgb(0xe0, 0xe0, 0xe0),
                text_muted: Color32::from_rgb(0xbd, 0xbd, 0xbd),
                accent_primary: Color32::from_rgb(0x5d, 0xe1, 0xff),
                accent_hover: Color32::from_rgb(0xa0, 0xef, 0xff),
                accent_soft: Color32::from_rgb(0x00, 0x2e, 0x3b),
                accent_on: Color32::BLACK,
                status_success: Color32::from_rgb(0x3d, 0xff, 0x9a),
                status_warning: Color32::from_rgb(0xff, 0xd5, 0x4f),
                status_error: Color32::from_rgb(0xff, 0x6e, 0x8a),
                status_info: Color32::from_rgb(0x4f, 0xc3, 0xff),
                status_running: Color32::from_rgb(0x5d, 0xe1, 0xff),
                diff_added: Color32::from_rgb(0x00, 0x2e, 0x1a),
                diff_removed: Color32::from_rgb(0x35, 0x00, 0x0d),
                diff_added_text: Color32::from_rgb(0x3d, 0xff, 0x9a),
                diff_removed_text: Color32::from_rgb(0xff, 0x6e, 0x8a),
            },
        }
    }

    /// Map a runtime state colour role onto this theme.
    pub fn state(&self, role: StateColor) -> Color32 {
        match role {
            StateColor::Neutral => self.text_secondary,
            StateColor::Running => self.status_running,
            StateColor::Success => self.status_success,
            StateColor::Warning => self.status_warning,
            StateColor::Error => self.status_error,
            StateColor::Info => self.status_info,
        }
    }

    /// A surface tinted with `color`, for a badge or a state chip. Opaque, so
    /// it looks the same whatever it is drawn over.
    pub fn tint(&self, color: Color32) -> Color32 {
        mix(self.background_primary, color, 0.16)
    }

    /// The hairline used between two regions of chrome.
    pub fn hairline(&self) -> Stroke {
        Stroke::new(1.0_f32, self.border_subtle)
    }

    /// The frame every card in the product uses: raised fill, hairline border,
    /// card radius, comfortable padding.
    pub fn card(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.background_raised)
            .stroke(Stroke::new(1.0_f32, self.border_subtle))
            .corner_radius(RADIUS_CARD)
            .inner_margin(Margin::symmetric(10, 8))
    }

    /// A 2px accent ring outside `rect`, the one focus treatment the product
    /// uses. Every hand-painted control that takes keyboard focus calls this so
    /// focus is visible everywhere, not just in settings.
    pub fn focus_ring(&self, painter: &egui::Painter, rect: egui::Rect) {
        focus_ring(painter, rect, self.accent_primary);
    }

    pub fn visuals(&self, appearance: Appearance) -> Visuals {
        let mut visuals = if matches!(appearance, Appearance::Light) {
            Visuals::light()
        } else {
            Visuals::dark()
        };
        let radius = CornerRadius::same(RADIUS_CONTROL);
        visuals.panel_fill = self.background_primary;
        visuals.window_fill = self.background_raised;
        visuals.window_corner_radius = CornerRadius::same(RADIUS_CARD);
        visuals.menu_corner_radius = CornerRadius::same(RADIUS_CARD);
        visuals.extreme_bg_color = self.background_secondary;
        visuals.faint_bg_color = self.surface_hover;
        visuals.override_text_color = Some(self.text_primary);
        visuals.hyperlink_color = self.accent_primary;
        visuals.window_stroke = Stroke::new(1.0_f32, self.border_strong);
        visuals.selection.bg_fill = mix(self.background_primary, self.accent_primary, 0.40);
        visuals.selection.stroke = Stroke::new(1.0_f32, self.accent_primary);

        visuals.widgets.noninteractive.bg_fill = self.background_secondary;
        visuals.widgets.noninteractive.weak_bg_fill = self.background_secondary;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.border_subtle);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text_secondary);
        visuals.widgets.noninteractive.corner_radius = radius;

        // Resting controls are quiet: a card-coloured fill and a hairline. The
        // interface should read as type on surfaces, not as a field of buttons.
        visuals.widgets.inactive.bg_fill = self.background_raised;
        visuals.widgets.inactive.weak_bg_fill = self.background_raised;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, self.border_subtle);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text_secondary);
        visuals.widgets.inactive.corner_radius = radius;

        visuals.widgets.hovered.bg_fill = self.surface_hover;
        visuals.widgets.hovered.weak_bg_fill = self.surface_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.border_strong);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.hovered.corner_radius = radius;

        visuals.widgets.active.bg_fill = self.surface_active;
        visuals.widgets.active.weak_bg_fill = self.surface_active;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, self.accent_primary);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.active.corner_radius = radius;

        // `widgets.open` is reached only for open menus; a focused standard
        // widget resolves to `widgets.active` (style.rs:1180-1191). The
        // product's real focus ring is drawn by the hand-painted controls via
        // `Tokens::focus_ring`; standard widgets keep the 1px accent stroke
        // above, which doubles as both pressed and focused (PRD §27).
        visuals.widgets.open.bg_fill = self.surface_hover;
        visuals.widgets.open.weak_bg_fill = self.surface_hover;
        visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, self.border_strong);
        visuals.widgets.open.corner_radius = radius;
        visuals
    }
}

/// Install `appearance` so it survives the host's own light/dark setting.
///
/// egui keeps a visuals slot per theme and picks one from the system unless it
/// is told otherwise. Setting only one slot is why a dark palette can come out
/// as white boxes on a Mac in Light mode: the widgets were being drawn from the
/// untouched light slot. Both slots are written, and the preference is pinned,
/// so the window looks the same everywhere.
pub fn install(ctx: &egui::Context, appearance: Appearance) {
    install_fonts(ctx);

    let tokens = Tokens::for_appearance(appearance);
    let visuals = tokens.visuals(appearance);
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
    ctx.options_mut(|options| {
        options.theme_preference = match appearance {
            Appearance::Light => egui::ThemePreference::Light,
            _ => egui::ThemePreference::Dark,
        };
    });

    let mut style = (*ctx.style()).clone();

    // Type scale. Every native role maps to one of the product's restrained
    // sizes, so a settings dialog and the workbench read as one application.
    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(TYPE_META, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(TYPE_BODY, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(TYPE_LABEL, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(TYPE_TITLE, FontFamily::Name("purrcode_display".into())),
        ),
        (
            TextStyle::Monospace,
            FontId::new(TYPE_CODE, FontFamily::Monospace),
        ),
    ]
    .into();

    // A slightly taller minimum row for widgets, so the larger body scale does
    // not crowd the controls around it — a 22px text field next to 14px body
    // copy reads as a control that was crushed to fit rather than sized to the
    // text. Wrapped prose gets its own explicit `line_height` where it is laid
    // out (markdown, settings copy).
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(9.0, 5.0);
    style.spacing.window_margin = Margin::same(10);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.interact_size = egui::vec2(28.0, 24.0);
    style.spacing.indent = 14.0;

    // A scrollbar that only appears while it is being used, so a list of files
    // is a list of files rather than a list of files next to a grey bar.
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.floating_width = 4.0;
    style.spacing.scroll.bar_inner_margin = 2.0;
    style.spacing.scroll.bar_outer_margin = 0.0;

    style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(RADIUS_CONTROL);
    ctx.set_style(style);
}

/// A 2px accent ring outside `rect` — the focus treatment every hand-painted
/// control uses. A free function so icon-only helpers that do not carry a
/// `Tokens` can still paint a ring in the accent colour.
pub fn focus_ring(painter: &egui::Painter, rect: egui::Rect, accent: Color32) {
    painter.rect_stroke(
        rect.expand(1.0),
        RADIUS_CONTROL,
        Stroke::new(2.0_f32, accent),
        egui::StrokeKind::Outside,
    );
}

/// Build the font stack once per process.
///
/// Three families are resolved independently, each falling back until a file
/// on this machine can be read: a UI sans for the interface, a monospace for
/// code and terminals, and a CJK face so Chinese renders instead of tofu
/// boxes (egui bundles no CJK font). The CJK face is appended *after* the UI
/// font rather than in front of it, so Latin text keeps the interface font and
/// only the glyphs the interface font is missing fall through to it.
fn install_fonts(ctx: &egui::Context) {
    // `set_fonts` rebuilds the glyph atlas; doing it on every theme change
    // would drop a frame for no reason.
    let already = ctx.data_mut(|data| {
        let key = egui::Id::new("purrcode_fonts_installed");
        let installed: bool = data.get_temp(key).unwrap_or(false);
        data.insert_temp(key, true);
        installed
    });
    if already {
        return;
    }

    const UI: &[&str] = &[
        // macOS: the system UI face, which is what a Mac app is expected to
        // look like.
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/HelveticaNeue.ttc",
        // Windows
        "C:\\Windows\\Fonts\\segoeui.ttf",
        // Linux
        "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    const DISPLAY: &[&str] = &[
        "/System/Library/Fonts/Avenir Next.ttc",
        "C:\\Windows\\Fonts\\bahnschrift.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed-Bold.ttf",
    ];
    const MONO: &[&str] = &[
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
        "C:\\Windows\\Fonts\\consola.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    ];
    const CJK: &[&str] = &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
    ];

    let mut fonts = FontDefinitions::default();
    let load = |fonts: &mut FontDefinitions, key: &str, candidates: &[&str]| -> bool {
        let Some(data) = candidates
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .map(FontData::from_owned)
        else {
            return false;
        };
        fonts.font_data.insert(key.to_owned(), data.into());
        true
    };

    let has_ui = load(&mut fonts, "ui", UI);
    let has_display = load(&mut fonts, "display", DISPLAY);
    let has_mono = load(&mut fonts, "mono", MONO);
    let has_cjk = load(&mut fonts, "cjk", CJK);

    if has_ui {
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "ui".to_owned());
    }
    if has_display {
        fonts.families.insert(
            FontFamily::Name("purrcode_display".into()),
            vec!["display".to_owned(), "ui".to_owned()],
        );
    } else {
        let fallback = fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts
            .families
            .insert(FontFamily::Name("purrcode_display".into()), fallback);
    }
    if has_mono {
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "mono".to_owned());
    }
    if has_cjk {
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_luminance(color: Color32) -> f32 {
        let channel = |value: u8| {
            let value = value as f32 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (first, second) = (relative_luminance(a), relative_luminance(b));
        let (lighter, darker) = if first > second {
            (first, second)
        } else {
            (second, first)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn body_text_stays_readable_in_every_appearance() {
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            let ratio = contrast(tokens.text_primary, tokens.background_primary);
            assert!(
                ratio >= 4.5,
                "{} primary text contrast is {ratio:.2}, below the 4.5:1 floor",
                appearance.label()
            );
        }
    }

    #[test]
    fn native_type_scale_has_clear_steps() {
        let scale = [TYPE_META, TYPE_LABEL, TYPE_BODY, TYPE_TITLE, TYPE_DISPLAY];
        assert!(scale.windows(2).all(|step| step[0] < step[1]));
        assert_eq!([TYPE_CODE], [TYPE_LABEL]);
        assert_eq!([TYPE_META], [TYPE_LABEL - 1.0]);
        // META → LABEL → BODY → TITLE → DISPLAY reads as a deliberate ramp:
        // the small steps are subtle, and the step from body up to title is the
        // biggest, so a section title separates itself from its body copy.
        let steps: Vec<f32> = scale.windows(2).map(|step| step[1] - step[0]).collect();
        assert_eq!([TYPE_TITLE - TYPE_BODY], [steps[2]]);
        assert!(
            steps[2] > steps[0] && steps[2] > steps[1] && steps[2] >= steps[3],
            "the body→title step should lead the ramp, got {steps:?}"
        );
    }

    #[test]
    fn secondary_text_clears_the_large_text_floor() {
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            let ratio = contrast(tokens.text_secondary, tokens.background_primary);
            assert!(
                ratio >= 3.0,
                "{} secondary text contrast is {ratio:.2}",
                appearance.label()
            );
        }
    }

    #[test]
    fn muted_text_is_quiet_but_still_legible() {
        // Muted is the colour of timestamps, hints and shortcuts. It is allowed
        // to recede; it is not allowed to disappear.
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            let ratio = contrast(tokens.text_muted, tokens.background_secondary);
            assert!(
                ratio >= 3.0,
                "{} muted text contrast is {ratio:.2}",
                appearance.label()
            );
        }
    }

    #[test]
    fn every_status_colour_is_visible_on_its_own_surface() {
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            for (name, color) in [
                ("success", tokens.status_success),
                ("warning", tokens.status_warning),
                ("error", tokens.status_error),
                ("info", tokens.status_info),
                ("accent", tokens.accent_primary),
            ] {
                let ratio = contrast(color, tokens.background_primary);
                assert!(
                    ratio >= 3.0,
                    "{} {name} contrast is {ratio:.2}",
                    appearance.label()
                );
            }
        }
    }

    #[test]
    fn diff_counters_are_text_colours_not_surface_tints() {
        // A "+12" drawn in the added-line *surface* colour is invisible. The
        // two roles must stay far enough apart that they cannot be swapped by
        // accident.
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            for (surface, text, name) in [
                (tokens.diff_added, tokens.diff_added_text, "added"),
                (tokens.diff_removed, tokens.diff_removed_text, "removed"),
            ] {
                assert!(
                    contrast(text, tokens.background_primary) >= 3.0,
                    "{} {name} diff text is not legible",
                    appearance.label()
                );
                assert_ne!(
                    surface,
                    text,
                    "{} {name} diff surface and text must differ",
                    appearance.label()
                );
            }
        }
    }

    #[test]
    fn accent_text_is_legible_on_an_accent_tinted_surface() {
        // Selected rail items and the user's own messages sit on `accent_soft`.
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            let ratio = contrast(tokens.text_primary, tokens.accent_soft);
            assert!(
                ratio >= 4.5,
                "{} text on the accent surface is {ratio:.2}",
                appearance.label()
            );
        }
    }

    #[test]
    fn surfaces_step_up_from_the_chrome_without_colliding() {
        // chrome → canvas → card must be three distinguishable surfaces in the
        // dark theme, or the whole window flattens into one grey field.
        let tokens = Tokens::for_appearance(Appearance::Dark);
        assert_ne!(tokens.background_secondary, tokens.background_primary);
        assert_ne!(tokens.background_primary, tokens.background_raised);
        assert!(
            relative_luminance(tokens.background_secondary)
                < relative_luminance(tokens.background_primary),
            "the chrome must be darker than the canvas it frames"
        );
        assert!(
            relative_luminance(tokens.background_primary)
                < relative_luminance(tokens.background_raised),
            "a card must be lighter than the canvas it sits on"
        );
    }

    #[test]
    fn high_contrast_beats_the_default_dark_theme() {
        let dark = Tokens::for_appearance(Appearance::Dark);
        let high = Tokens::for_appearance(Appearance::HighContrast);
        assert!(
            contrast(high.text_primary, high.background_primary)
                > contrast(dark.text_primary, dark.background_primary),
            "high contrast must actually raise contrast"
        );
    }

    #[test]
    fn every_state_role_resolves_to_a_colour_in_every_theme() {
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            for role in [
                StateColor::Neutral,
                StateColor::Running,
                StateColor::Success,
                StateColor::Warning,
                StateColor::Error,
                StateColor::Info,
            ] {
                let color = tokens.state(role);
                assert!(
                    contrast(color, tokens.background_primary) >= 3.0,
                    "{} {role:?} is not legible",
                    appearance.label()
                );
            }
        }
    }

    #[test]
    fn error_and_success_are_never_the_same_colour() {
        // Status must not depend on colour alone, but it must also not use the
        // same colour for opposite meanings.
        for appearance in Appearance::ALL {
            let tokens = Tokens::for_appearance(*appearance);
            assert_ne!(tokens.status_success, tokens.status_error);
            assert_ne!(tokens.status_warning, tokens.status_success);
        }
    }

    #[test]
    fn mixing_stays_inside_the_two_endpoints() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(200, 100, 50);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(100, 50, 25));
        // Out-of-range factors clamp rather than wrap around.
        assert_eq!(mix(a, b, -1.0), a);
        assert_eq!(mix(a, b, 9.0), b);
    }

    #[test]
    fn the_font_stack_lays_out_latin_and_chinese_without_panicking() {
        // Every font this module loads comes off the host filesystem, so a
        // face this machine cannot parse would take the window down on the
        // first frame. Running a real frame is what proves it parses.
        let ctx = egui::Context::default();
        install(&ctx, Appearance::Dark);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("PurrCode — Agent");
                ui.label("写一个中文字符串测试");
                ui.monospace("fn main() { println!(\"hi\"); }");
            });
        });
        assert!(
            !output.textures_delta.set.is_empty(),
            "a laid-out frame must produce a glyph atlas"
        );
    }
}
