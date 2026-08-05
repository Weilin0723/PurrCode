//! Icons drawn with the painter, plus the brand mark loaded from `brand/`.
//!
//! egui's bundled fonts cover only a subset of emoji, so an emoji glyph in the
//! UI renders as a tofu box on a real machine. Every icon here is therefore
//! either a shape this crate draws itself or the actual PurrCode mascot, and
//! nothing on screen depends on the host having a particular font.

use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

/// The mascot, at the two sizes the interface uses.
pub const MARK_32: &[u8] = include_bytes!("../../../brand/icons/32.png");
pub const MARK_64: &[u8] = include_bytes!("../../../brand/icons/64.png");
/// The full horizontal lockup (mascot + wordmark), for large centered surfaces
/// like the start screen. Dark variant: `Purr` white, `Code` blue, transparent
/// paper.
pub const LOGO_HORIZONTAL: &[u8] =
    include_bytes!("../../../brand/purrcode-logo-horizontal-dark.png");

/// Render the full horizontal logo lockup at `height` points, preserving its
/// 2.4:1 aspect ratio. Unlike a cat mark with a separate wordmark underneath,
/// this is one centered image, so the two can never drift apart.
pub fn logo(ui: &mut Ui, height: f32) -> Response {
    let width = height * (761.0 / 320.0);
    ui.add(
        egui::Image::new(egui::ImageSource::Bytes {
            uri: "bytes://purrcode-logo-horizontal.png".into(),
            bytes: LOGO_HORIZONTAL.into(),
        })
        .fit_to_exact_size(egui::Vec2::new(width, height))
        .maintain_aspect_ratio(true),
    )
}

fn mark_source(size: f32) -> egui::ImageSource<'static> {
    if size <= 34.0 {
        egui::ImageSource::Bytes {
            uri: "bytes://purrcode-mark-32.png".into(),
            bytes: MARK_32.into(),
        }
    } else {
        egui::ImageSource::Bytes {
            uri: "bytes://purrcode-mark-64.png".into(),
            bytes: MARK_64.into(),
        }
    }
}

/// Render the PurrCode mascot at `size` points.
pub fn mark(ui: &mut Ui, size: f32) -> Response {
    ui.add(
        egui::Image::new(mark_source(size))
            .fit_to_exact_size(Vec2::splat(size))
            .maintain_aspect_ratio(true),
    )
}

/// Render the product wordmark with the brand's blue `Code` suffix.
///
/// Keeping this in one helper prevents individual surfaces from drifting back
/// to a single-colour `PurrCode` label.
///
/// Two colours, but *one* widget: the halves are sections of a single
/// `LayoutJob`, not two labels in a horizontal strip. A strip is laid out
/// left-aligned inside the full width it is handed, so under a centring layout
/// the wordmark would start at the container's left edge instead of sitting
/// under the mark above it. One galley is one rect, which every layout centres
/// correctly. Sections also butt straight up against each other, so `Purr` and
/// `Code` read as one word without anybody having to zero the item spacing.
pub fn wordmark(ui: &mut Ui, size: f32, purr_color: Color32, code_color: Color32) -> Response {
    let font = crate::theme::display(size);
    // Whatever a plain label would do in this layout, so the wordmark keeps the
    // baseline of the text and marks beside it in the application bar.
    let valign = ui.text_valign();
    let mut job = egui::text::LayoutJob::default();
    for (text, color) in [("Purr", purr_color), ("Code", code_color)] {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                valign,
                ..Default::default()
            },
        );
    }
    ui.label(job)
}

/// The source-derived mascot presented as the application icon.
///
/// Small product marks must not switch to the abstract line cat: the supplied
/// blue-eyed longhair is the identity. The mark is drawn on whatever surface it
/// sits on — it never gets a coloured tile behind it, because a filled accent
/// square reads as a button and turns the mascot into a badge on a badge.
pub fn brand_badge(ui: &mut Ui, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    brand_badge_at(ui, rect);
    response
}

/// Paint the source-derived App badge into an already allocated control.
/// Tabs use this form so the image does not create a second click target.
pub fn brand_badge_at(ui: &mut Ui, rect: Rect) {
    let size = rect.width().min(rect.height());
    let square = Rect::from_center_size(rect.center(), Vec2::splat(size));
    ui.put(
        square,
        egui::Image::new(mark_source(size))
            .fit_to_exact_size(square.size())
            .maintain_aspect_ratio(true),
    );
}

/// The glyphs the rail and the trees use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Glyph {
    Conversation,
    Files,
    Branch,
    Checklist,
    Play,
    Folder,
    FolderOpen,
    File,
    ChevronRight,
    ChevronDown,
    Plus,
    Close,
    Send,
    Stop,
    Shield,
    Gear,
    Search,
    /// Three dots: the overflow menu.
    More,
    /// The mascot reduced to what survives at 14–18px: ears, face, eyes.
    CatHead,
    /// A file was read.
    Eye,
    /// An edit was accepted; a step finished.
    Check,
    /// Diagnostics were checked.
    Bug,
    /// A command ran.
    Terminal,
    /// Previous sessions.
    History,
    /// The agent did something on its own initiative.
    Sparkle,
    /// A file was written.
    Pencil,
    /// Undo an accepted edit.
    Revert,
    /// Two versions of a file, side by side.
    Diff,
    /// Something needs the user's attention.
    Warning,
    /// Validation.
    Beaker,
    /// Toggle the primary sidebar.
    PanelLeft,
    /// Toggle the auxiliary panel.
    PanelRight,
    /// Toggle the bottom dock.
    PanelBottom,
    /// Split the editor.
    Split,
}

/// Draw `glyph` centred in `rect` using `color`.
///
/// Everything is built from lines, rectangles and circles so it stays crisp at
/// 14px and needs no font.
pub fn draw(ui: &Ui, rect: Rect, glyph: Glyph, color: Color32) {
    let painter = ui.painter();
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let half = s * 0.5;
    let stroke = Stroke::new((s * 0.09).max(1.0), color);

    match glyph {
        Glyph::Conversation => {
            let body = Rect::from_center_size(
                Pos2::new(c.x, c.y - s * 0.06),
                Vec2::new(s * 0.82, s * 0.62),
            );
            painter.rect_stroke(body, s * 0.16, stroke, egui::StrokeKind::Middle);
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.18, body.max.y),
                    Pos2::new(c.x - s * 0.30, body.max.y + s * 0.22),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.30, body.max.y + s * 0.22),
                    Pos2::new(c.x - s * 0.02, body.max.y),
                ],
                stroke,
            );
        }
        Glyph::Files => {
            let back = Rect::from_min_size(
                Pos2::new(c.x - half * 0.75, c.y - half * 0.85),
                Vec2::new(s * 0.62, s * 0.78),
            );
            let front = back.translate(Vec2::new(s * 0.18, s * 0.16));
            painter.rect_stroke(back, s * 0.10, stroke, egui::StrokeKind::Middle);
            painter.rect_filled(front, s * 0.10, ui.visuals().panel_fill);
            painter.rect_stroke(front, s * 0.10, stroke, egui::StrokeKind::Middle);
        }
        Glyph::Branch => {
            let x = c.x - s * 0.16;
            painter.circle_stroke(Pos2::new(x, c.y - half * 0.72), s * 0.11, stroke);
            painter.circle_stroke(Pos2::new(x, c.y + half * 0.72), s * 0.11, stroke);
            painter.circle_stroke(
                Pos2::new(c.x + s * 0.30, c.y - half * 0.10),
                s * 0.11,
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(x, c.y - half * 0.72 + s * 0.11),
                    Pos2::new(x, c.y + half * 0.72 - s * 0.11),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(x, c.y + s * 0.02),
                    Pos2::new(c.x + s * 0.30, c.y - half * 0.10 + s * 0.11),
                ],
                stroke,
            );
        }
        Glyph::Checklist => {
            for index in 0..3 {
                let y = c.y - s * 0.26 + index as f32 * s * 0.26;
                painter.line_segment(
                    [
                        Pos2::new(c.x - half * 0.78, y),
                        Pos2::new(c.x - half * 0.42, y),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        Pos2::new(c.x - half * 0.20, y),
                        Pos2::new(c.x + half * 0.82, y),
                    ],
                    stroke,
                );
            }
        }
        Glyph::Play => {
            let points = vec![
                Pos2::new(c.x - s * 0.22, c.y - s * 0.32),
                Pos2::new(c.x + s * 0.32, c.y),
                Pos2::new(c.x - s * 0.22, c.y + s * 0.32),
            ];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Glyph::Folder | Glyph::FolderOpen => {
            let body = Rect::from_center_size(c, Vec2::new(s * 0.86, s * 0.66));
            let filled = matches!(glyph, Glyph::FolderOpen);
            painter.rect_stroke(body, s * 0.10, stroke, egui::StrokeKind::Middle);
            painter.line_segment(
                [
                    Pos2::new(body.min.x, body.min.y),
                    Pos2::new(body.min.x + s * 0.26, body.min.y - s * 0.14),
                ],
                stroke,
            );
            if filled {
                painter.rect_filled(body.shrink(s * 0.12), s * 0.06, color.linear_multiply(0.35));
            }
        }
        Glyph::File => {
            let body = Rect::from_center_size(c, Vec2::new(s * 0.62, s * 0.82));
            painter.rect_stroke(body, s * 0.10, stroke, egui::StrokeKind::Middle);
            for index in 0..2 {
                let y = c.y - s * 0.10 + index as f32 * s * 0.22;
                painter.line_segment(
                    [
                        Pos2::new(body.min.x + s * 0.12, y),
                        Pos2::new(body.max.x - s * 0.12, y),
                    ],
                    stroke,
                );
            }
        }
        Glyph::More => {
            for offset in [-1.0_f32, 0.0, 1.0] {
                let _ = painter.circle_filled(
                    Pos2::new(c.x + offset * s * 0.28, c.y),
                    (s * 0.075).max(1.0),
                    color,
                );
            }
        }
        Glyph::ChevronRight => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.14, c.y - s * 0.26),
                    Pos2::new(c.x + s * 0.16, c.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 0.16, c.y),
                    Pos2::new(c.x - s * 0.14, c.y + s * 0.26),
                ],
                stroke,
            );
        }
        Glyph::ChevronDown => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.26, c.y - s * 0.14),
                    Pos2::new(c.x, c.y + s * 0.16),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x, c.y + s * 0.16),
                    Pos2::new(c.x + s * 0.26, c.y - s * 0.14),
                ],
                stroke,
            );
        }
        Glyph::Plus => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.28, c.y),
                    Pos2::new(c.x + s * 0.28, c.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x, c.y - s * 0.28),
                    Pos2::new(c.x, c.y + s * 0.28),
                ],
                stroke,
            );
        }
        Glyph::Close => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.22, c.y - s * 0.22),
                    Pos2::new(c.x + s * 0.22, c.y + s * 0.22),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 0.22, c.y - s * 0.22),
                    Pos2::new(c.x - s * 0.22, c.y + s * 0.22),
                ],
                stroke,
            );
        }
        Glyph::Send => {
            let points = vec![
                Pos2::new(c.x - s * 0.26, c.y - s * 0.30),
                Pos2::new(c.x + s * 0.32, c.y),
                Pos2::new(c.x - s * 0.26, c.y + s * 0.30),
            ];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Glyph::Stop => {
            painter.rect_filled(
                Rect::from_center_size(c, Vec2::splat(s * 0.46)),
                s * 0.08,
                color,
            );
        }
        Glyph::Shield => {
            let top = c.y - s * 0.34;
            let bottom = c.y + s * 0.36;
            let points = vec![
                Pos2::new(c.x, top),
                Pos2::new(c.x + s * 0.30, top + s * 0.14),
                Pos2::new(c.x + s * 0.22, bottom - s * 0.14),
                Pos2::new(c.x, bottom),
                Pos2::new(c.x - s * 0.22, bottom - s * 0.14),
                Pos2::new(c.x - s * 0.30, top + s * 0.14),
            ];
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        Glyph::Gear => {
            painter.circle_stroke(c, s * 0.20, stroke);
            for step in 0..6 {
                let angle = std::f32::consts::TAU * step as f32 / 6.0;
                let (sin, cos) = angle.sin_cos();
                painter.line_segment(
                    [
                        Pos2::new(c.x + cos * s * 0.26, c.y + sin * s * 0.26),
                        Pos2::new(c.x + cos * s * 0.38, c.y + sin * s * 0.38),
                    ],
                    stroke,
                );
            }
        }
        Glyph::CatHead => {
            // The raster mark turns to mud below about 20px, so the avatar is
            // drawn: two ears, a face, two eyes. It reads as the mascot at 14px
            // and stays sharp on any display scale.
            let face = s * 0.36;
            let top = c.y - face * 0.72;
            for side in [-1.0_f32, 1.0] {
                let base_inner = Pos2::new(c.x + side * face * 0.28, top - face * 0.10);
                let base_outer = Pos2::new(c.x + side * face * 0.92, top + face * 0.16);
                let tip = Pos2::new(c.x + side * face * 0.86, c.y - face * 1.16);
                painter.add(egui::Shape::convex_polygon(
                    vec![base_inner, tip, base_outer],
                    color,
                    Stroke::NONE,
                ));
            }
            let _ = painter.circle_filled(c, face, color);
            // The eyes are knocked out of the head, which is what makes the
            // silhouette read as a face rather than a blob.
            let eye = face * 0.19;
            let background = ui.visuals().panel_fill;
            for side in [-1.0_f32, 1.0] {
                let _ = painter.circle_filled(
                    Pos2::new(c.x + side * face * 0.36, c.y - face * 0.10),
                    eye,
                    background,
                );
            }
        }
        Glyph::Search => {
            painter.circle_stroke(Pos2::new(c.x - s * 0.06, c.y - s * 0.06), s * 0.22, stroke);
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 0.10, c.y + s * 0.10),
                    Pos2::new(c.x + s * 0.30, c.y + s * 0.30),
                ],
                stroke,
            );
        }
        Glyph::Eye => {
            // An almond built from two arcs, so it reads as an eye at 12px.
            let w = s * 0.40;
            let h = s * 0.22;
            let arc = |up: f32| {
                let mut points = Vec::with_capacity(13);
                for step in 0..=12 {
                    let t = step as f32 / 12.0;
                    let x = c.x - w + 2.0 * w * t;
                    let y = c.y + up * h * (std::f32::consts::PI * t).sin();
                    points.push(Pos2::new(x, y));
                }
                points
            };
            painter.add(egui::Shape::line(arc(-1.0), stroke));
            painter.add(egui::Shape::line(arc(1.0), stroke));
            painter.circle_stroke(c, s * 0.11, stroke);
        }
        Glyph::Check => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.28, c.y + s * 0.02),
                    Pos2::new(c.x - s * 0.08, c.y + s * 0.22),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.08, c.y + s * 0.22),
                    Pos2::new(c.x + s * 0.30, c.y - s * 0.22),
                ],
                stroke,
            );
        }
        Glyph::Bug => {
            let body = Rect::from_center_size(c, Vec2::new(s * 0.44, s * 0.52));
            painter.rect_stroke(body, s * 0.20, stroke, egui::StrokeKind::Middle);
            for side in [-1.0_f32, 1.0] {
                for row in [-1.0_f32, 0.0, 1.0] {
                    let y = c.y + row * s * 0.18;
                    painter.line_segment(
                        [
                            Pos2::new(c.x + side * s * 0.22, y),
                            Pos2::new(c.x + side * s * 0.40, y - row * s * 0.06),
                        ],
                        stroke,
                    );
                }
            }
            painter.line_segment(
                [
                    Pos2::new(c.x, c.y - s * 0.26),
                    Pos2::new(c.x, c.y - s * 0.40),
                ],
                stroke,
            );
        }
        Glyph::Terminal => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.34, c.y - s * 0.22),
                    Pos2::new(c.x - s * 0.08, c.y + s * 0.02),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.08, c.y + s * 0.02),
                    Pos2::new(c.x - s * 0.34, c.y + s * 0.26),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 0.04, c.y + s * 0.26),
                    Pos2::new(c.x + s * 0.36, c.y + s * 0.26),
                ],
                stroke,
            );
        }
        Glyph::History => {
            painter.circle_stroke(c, s * 0.32, stroke);
            painter.line_segment([c, Pos2::new(c.x, c.y - s * 0.20)], stroke);
            painter.line_segment([c, Pos2::new(c.x + s * 0.16, c.y + s * 0.06)], stroke);
        }
        Glyph::Sparkle => {
            // A four-point star: the universal "the model did this" mark.
            let long = s * 0.38;
            let short = s * 0.12;
            let points = vec![
                Pos2::new(c.x, c.y - long),
                Pos2::new(c.x + short, c.y - short),
                Pos2::new(c.x + long, c.y),
                Pos2::new(c.x + short, c.y + short),
                Pos2::new(c.x, c.y + long),
                Pos2::new(c.x - short, c.y + short),
                Pos2::new(c.x - long, c.y),
                Pos2::new(c.x - short, c.y - short),
            ];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Glyph::Pencil => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.28, c.y + s * 0.28),
                    Pos2::new(c.x + s * 0.24, c.y - s * 0.24),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.28, c.y + s * 0.28),
                    Pos2::new(c.x - s * 0.14, c.y + s * 0.32),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 0.10, c.y - s * 0.34),
                    Pos2::new(c.x + s * 0.34, c.y - s * 0.10),
                ],
                stroke,
            );
        }
        Glyph::Revert => {
            // An arrow curling back on itself.
            let radius = s * 0.28;
            let mut points = Vec::with_capacity(17);
            for step in 0..=16 {
                let angle =
                    std::f32::consts::PI * 0.35 + std::f32::consts::PI * 1.4 * step as f32 / 16.0;
                points.push(Pos2::new(
                    c.x + angle.cos() * radius,
                    c.y + angle.sin() * radius,
                ));
            }
            painter.add(egui::Shape::line(points, stroke));
            let head = Pos2::new(c.x + radius * 0.94, c.y - radius * 0.34);
            painter.line_segment(
                [head, Pos2::new(head.x - s * 0.16, head.y - s * 0.06)],
                stroke,
            );
            painter.line_segment(
                [head, Pos2::new(head.x + s * 0.02, head.y + s * 0.16)],
                stroke,
            );
        }
        Glyph::Diff => {
            for (side, dy) in [(-1.0_f32, -1.0_f32), (1.0, 1.0)] {
                let panel = Rect::from_center_size(
                    Pos2::new(c.x + side * s * 0.16, c.y + dy * s * 0.14),
                    Vec2::new(s * 0.44, s * 0.44),
                );
                painter.rect_stroke(panel, s * 0.08, stroke, egui::StrokeKind::Middle);
            }
        }
        Glyph::Warning => {
            let top = Pos2::new(c.x, c.y - s * 0.34);
            let left = Pos2::new(c.x - s * 0.38, c.y + s * 0.30);
            let right = Pos2::new(c.x + s * 0.38, c.y + s * 0.30);
            painter.add(egui::Shape::closed_line(vec![top, right, left], stroke));
            painter.line_segment(
                [
                    Pos2::new(c.x, c.y - s * 0.10),
                    Pos2::new(c.x, c.y + s * 0.08),
                ],
                stroke,
            );
            let _ =
                painter.circle_filled(Pos2::new(c.x, c.y + s * 0.20), (s * 0.055).max(0.9), color);
        }
        Glyph::Beaker => {
            let neck = s * 0.12;
            let points = vec![
                Pos2::new(c.x - neck, c.y - s * 0.34),
                Pos2::new(c.x + neck, c.y - s * 0.34),
                Pos2::new(c.x + neck, c.y - s * 0.06),
                Pos2::new(c.x + s * 0.32, c.y + s * 0.30),
                Pos2::new(c.x - s * 0.32, c.y + s * 0.30),
                Pos2::new(c.x - neck, c.y - s * 0.06),
            ];
            painter.add(egui::Shape::closed_line(points, stroke));
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.20, c.y + s * 0.14),
                    Pos2::new(c.x + s * 0.20, c.y + s * 0.14),
                ],
                stroke,
            );
        }
        Glyph::PanelLeft | Glyph::PanelRight | Glyph::PanelBottom | Glyph::Split => {
            let body = Rect::from_center_size(c, Vec2::splat(s * 0.70));
            painter.rect_stroke(body, s * 0.10, stroke, egui::StrokeKind::Middle);
            let (from, to) = match glyph {
                Glyph::PanelLeft => (
                    Pos2::new(body.min.x + s * 0.24, body.min.y),
                    Pos2::new(body.min.x + s * 0.24, body.max.y),
                ),
                Glyph::PanelRight => (
                    Pos2::new(body.max.x - s * 0.24, body.min.y),
                    Pos2::new(body.max.x - s * 0.24, body.max.y),
                ),
                Glyph::PanelBottom => (
                    Pos2::new(body.min.x, body.max.y - s * 0.22),
                    Pos2::new(body.max.x, body.max.y - s * 0.22),
                ),
                // Split: the divider sits in the middle.
                _ => (Pos2::new(c.x, body.min.y), Pos2::new(c.x, body.max.y)),
            };
            painter.line_segment([from, to], stroke);
        }
    }
}

/// An icon-only button that reports whether it was clicked.
pub fn button(ui: &mut Ui, glyph: Glyph, size: f32, color: Color32, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let color = if response.hovered() {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        color
    };
    draw(ui, rect, glyph, color);
    response.on_hover_text(tooltip)
}

/// An icon button with a rounded hover surface.
///
/// This is the shape every action in the chrome uses — title bar, panel
/// headers, tab strip — so a clickable icon is always the same size target
/// with the same feedback, and an icon that is *not* a button never grows one.
pub fn ghost_button(
    ui: &mut Ui,
    glyph: Glyph,
    box_size: f32,
    icon_size: f32,
    color: Color32,
    hover_fill: Color32,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(box_size), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, crate::theme::RADIUS_CONTROL, hover_fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.has_focus() {
        crate::theme::focus_ring(ui.painter(), rect, color);
    }
    draw(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(icon_size)),
        glyph,
        color,
    );
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// One item of the vertical activity rail.
///
/// A selected item is a filled tile with an accent edge, not a coloured
/// letterform: at rail size the tile is what the eye finds, and the label
/// travels in the tooltip where it can be a full sentence.
pub fn rail_item(
    ui: &mut Ui,
    glyph: Glyph,
    selected: bool,
    tokens: &crate::theme::Tokens,
    tooltip: &str,
) -> Response {
    let size = crate::theme::RAIL_ITEM;
    let (outer, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().min(crate::theme::RAIL_WIDTH), size),
        Sense::click(),
    );
    let tile = Rect::from_center_size(outer.center(), Vec2::splat(size));
    let painter = ui.painter();
    if selected {
        painter.rect_filled(tile, crate::theme::RADIUS_CONTROL, tokens.accent_soft);
        // The edge marker: the rail's answer to "which one am I on?" that
        // survives a colour-blind reading of the tile tint.
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(outer.left(), tile.center().y - size * 0.30),
                Vec2::new(2.0, size * 0.60),
            ),
            1,
            tokens.accent_primary,
        );
    } else if response.hovered() {
        painter.rect_filled(tile, crate::theme::RADIUS_CONTROL, tokens.surface_hover);
    }
    if response.has_focus() {
        tokens.focus_ring(painter, tile);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if selected {
        tokens.accent_primary
    } else if response.hovered() {
        tokens.text_primary
    } else {
        tokens.text_muted
    };
    draw(
        ui,
        Rect::from_center_size(tile.center(), Vec2::splat(18.0)),
        glyph,
        color,
    );
    response.on_hover_text(tooltip)
}

/// The brand mark on its own tile — the top of the activity rail.
pub fn mark_badge(ui: &mut Ui, size: f32, fill: Color32, glyph_color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let tile = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.82));
    ui.painter()
        .rect_filled(tile, crate::theme::RADIUS_CONTROL, fill);
    draw(
        ui,
        Rect::from_center_size(tile.center(), Vec2::splat(size * 0.56)),
        Glyph::CatHead,
        glyph_color,
    );
    response
}

/// Draw a glyph inline with text, without allocating an interactive widget.
pub fn inline(ui: &mut Ui, glyph: Glyph, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    draw(ui, rect, glyph, color);
}

/// The checklist marker for a semantic step.
///
/// A filled dot means running, a tick means done, a cross means failed, an
/// empty ring means not started. The status word always travels with it, so
/// nothing here carries meaning by shape alone.
pub fn step_marker(ui: &mut Ui, status: &str, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let c = rect.center();
    let r = size * 0.30;
    let stroke = Stroke::new((size * 0.10).max(1.2), color);
    match status {
        "done" => {
            let _ = painter.circle_stroke(c, r, stroke);
            painter.line_segment(
                [
                    Pos2::new(c.x - r * 0.50, c.y),
                    Pos2::new(c.x - r * 0.10, c.y + r * 0.45),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - r * 0.10, c.y + r * 0.45),
                    Pos2::new(c.x + r * 0.55, c.y - r * 0.45),
                ],
                stroke,
            );
        }
        "running" => {
            let _ = painter.circle_filled(c, r * 0.62, color);
            let _ = painter.circle_stroke(c, r, Stroke::new(stroke.width * 0.7, color));
        }
        "failed" => {
            let _ = painter.circle_stroke(c, r, stroke);
            painter.line_segment(
                [
                    Pos2::new(c.x - r * 0.42, c.y - r * 0.42),
                    Pos2::new(c.x + r * 0.42, c.y + r * 0.42),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + r * 0.42, c.y - r * 0.42),
                    Pos2::new(c.x - r * 0.42, c.y + r * 0.42),
                ],
                stroke,
            );
        }
        "blocked" => {
            let _ = painter.circle_stroke(c, r, stroke);
            painter.line_segment(
                [
                    Pos2::new(c.x, c.y - r * 0.45),
                    Pos2::new(c.x, c.y + r * 0.10),
                ],
                stroke,
            );
            let _ =
                painter.circle_filled(Pos2::new(c.x, c.y + r * 0.42), stroke.width * 0.6, color);
        }
        _ => {
            let _ = painter.circle_stroke(c, r, stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context with no window behind it, with the display family bound to
    /// whatever the default stack offers. `theme::install` reads faces off the
    /// host, which a test must not depend on; the family only has to exist,
    /// because what is under test is layout, not typeface.
    fn headless_context() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let proportional = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts.families.insert(
            egui::FontFamily::Name("purrcode_display".into()),
            proportional,
        );
        ctx.set_fonts(fonts);
        ctx
    }

    #[test]
    fn the_wordmark_is_one_label_a_centring_layout_can_centre() {
        // The start screen puts the wordmark under the mascot inside
        // `vertical_centered`. While it was a two-label `ui.horizontal` strip
        // it was laid out left-aligned across the full container width, so it
        // sat against the card's left edge instead of under the cat — and the
        // `centered_and_justified` workaround for that swallowed the card's
        // whole remaining height. One galley needs neither.
        const PANEL_WIDTH: f32 = 800.0;
        let ctx = headless_context();
        let (mut word, mut panel) = (None, None);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(PANEL_WIDTH, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    panel = Some(ui.max_rect());
                    ui.vertical_centered(|ui| {
                        word = Some(wordmark(ui, 24.0, Color32::WHITE, Color32::BLUE).rect);
                    });
                });
            },
        );
        let word = word.expect("the wordmark was laid out");
        let panel = panel.expect("the panel was laid out");
        assert!(
            word.width() > 1.0 && word.width() < panel.width() * 0.5,
            "the wordmark must occupy its own text width, not the container's \
             ({}pt inside a {}pt panel)",
            word.width(),
            panel.width()
        );
        assert!(
            (word.center().x - panel.center().x).abs() < 2.0,
            "a centring layout must land the wordmark on the container's centre \
             (wordmark centre {}, panel centre {})",
            word.center().x,
            panel.center().x
        );
    }

    #[test]
    fn the_start_screen_centres_the_wordmark_without_claiming_the_card() {
        // `centered_and_justified` hands its single child every remaining
        // point of width *and height*, which is what used to strand the
        // wordmark in the vertical middle of a tall card.
        let source = include_str!("welcome.rs");
        assert!(
            !source.contains("centered_and_justified"),
            "the start screen must not centre by claiming the rest of the panel"
        );
    }

    #[test]
    fn the_brand_marks_are_real_pngs_at_the_sizes_the_ui_asks_for() {
        for (bytes, expected) in [(MARK_32, 32u32), (MARK_64, 64)] {
            let image = image::load_from_memory(bytes).expect("brand mark must decode");
            assert_eq!(image.width(), expected);
            assert_eq!(image.height(), expected);
        }
    }

    #[test]
    fn no_icon_depends_on_an_emoji_font() {
        // The whole point of this module: a machine without an emoji font must
        // still show icons rather than tofu boxes. If any glyph ever needs
        // text, this assertion is where that decision gets caught.
        let source = include_str!("icons.rs");
        let body = source
            .split("pub fn draw(")
            .nth(1)
            .expect("draw must exist")
            .split("\n#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in ["RichText::new", "ui.label(", "galley"] {
            assert!(
                !body.contains(forbidden),
                "icon drawing must not fall back to text ({forbidden})"
            );
        }
    }
}
