//! Terminal-width-driven layout.
//!
//! Three breakpoints, one rule each:
//!
//! * **Wide** (≥120 columns) — conversation beside an inspector that appears
//!   only when opened or required.
//! * **Standard** (80–119) — conversation, activity, composer stacked; the
//!   inspector opens as a drawer that temporarily replaces activity.
//! * **Narrow** (<80) — one focused surface at a time. Workspace, activity,
//!   inspector and composer are never all on screen together.
//!
//! Everything here is pure geometry so it can be unit tested without rendering.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Breakpoint {
    Narrow,
    Standard,
    Wide,
}

impl Breakpoint {
    pub const fn of(width: u16) -> Self {
        if width >= 120 {
            Self::Wide
        } else if width >= 80 {
            Self::Standard
        } else {
            Self::Narrow
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Narrow => "narrow",
            Self::Standard => "standard",
            Self::Wide => "wide",
        }
    }

    /// True when the inspector may sit beside the conversation instead of
    /// replacing another region.
    pub const fn inspector_is_a_column(self) -> bool {
        matches!(self, Self::Wide)
    }
}

/// Why the inspector is on screen. It is hidden unless one of these holds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InspectorReason {
    #[default]
    Hidden,
    /// The user opened it.
    Requested,
    /// A decision requires it.
    ApprovalRequired,
    /// Recovery requires it.
    RecoveryRequired,
    /// Validation needs attention.
    ValidationAttention,
}

impl InspectorReason {
    pub const fn visible(self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// True when the inspector is required rather than merely requested, in
    /// which case a narrow terminal promotes it to a full-screen surface.
    pub const fn required(self) -> bool {
        matches!(
            self,
            Self::ApprovalRequired | Self::RecoveryRequired | Self::ValidationAttention
        )
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Requested => "requested",
            Self::ApprovalRequired => "approval required",
            Self::RecoveryRequired => "recovery required",
            Self::ValidationAttention => "validation needs attention",
        }
    }
}

/// The regions of the workbench for one frame. Absent regions are `None` rather
/// than zero-sized, so a component cannot accidentally draw into a region the
/// layout decided to hide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkbenchAreas {
    pub breakpoint: Breakpoint,
    pub header: Rect,
    pub conversation: Option<Rect>,
    pub activity: Option<Rect>,
    pub inspector: Option<Rect>,
    pub composer: Option<Rect>,
    pub hints: Rect,
}

/// Inputs the layout needs. Kept separate from `App` so the geometry is testable
/// without constructing an application.
#[derive(Clone, Copy, Debug)]
pub struct LayoutRequest {
    pub composer_lines: u16,
    pub activity_lines: u16,
    pub inspector: InspectorReason,
    /// True when a focused decision surface owns the whole frame.
    pub focused_surface: bool,
}

impl WorkbenchAreas {
    pub fn compute(area: Rect, request: LayoutRequest) -> Self {
        let breakpoint = Breakpoint::of(area.width);
        let composer_height = request.composer_lines.clamp(1, 8).saturating_add(1);

        // A required inspector on a narrow terminal takes the whole frame: at 60
        // columns a decision cannot share space and stay readable.
        let inspector_owns_frame = request.focused_surface
            || (breakpoint == Breakpoint::Narrow && request.inspector.required());
        if inspector_owns_frame {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(3),
                    Constraint::Length(1),
                ])
                .split(area);
            return Self {
                breakpoint,
                header: rows[0],
                conversation: None,
                activity: None,
                inspector: Some(rows[1]),
                composer: None,
                hints: rows[2],
            };
        }

        let activity_height = if request.activity_lines == 0 {
            0
        } else {
            // Activity must never carry the same visual weight as the
            // conversation, so it is capped well below it.
            request.activity_lines.min(match breakpoint {
                Breakpoint::Narrow => 3,
                Breakpoint::Standard => 5,
                Breakpoint::Wide => 6,
            })
        };

        let drawer_replaces_activity =
            breakpoint == Breakpoint::Standard && request.inspector.visible();
        // Narrow shows one surface at a time: an open inspector replaces the
        // conversation, and the activity rail steps aside with it.
        let narrow_inspector = breakpoint == Breakpoint::Narrow && request.inspector.visible();
        let show_activity = activity_height > 0 && !drawer_replaces_activity && !narrow_inspector;
        let drawer_height = if drawer_replaces_activity {
            activity_height.max(5)
        } else {
            0
        };

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(if show_activity { activity_height } else { 0 }),
                Constraint::Length(drawer_height),
                Constraint::Length(composer_height),
                Constraint::Length(1),
            ])
            .split(area);
        let (header, body, activity, drawer, composer, hints) =
            (rows[0], rows[1], rows[2], rows[3], rows[4], rows[5]);

        let (conversation, side_inspector) =
            if breakpoint.inspector_is_a_column() && request.inspector.visible() {
                let columns = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(50), Constraint::Length(40)])
                    .split(body);
                (columns[0], Some(columns[1]))
            } else if breakpoint == Breakpoint::Narrow && request.inspector.visible() {
                // Narrow: the inspector replaces the conversation entirely.
                (body, Some(body))
            } else {
                (body, None)
            };

        let narrow_inspector_takes_body =
            breakpoint == Breakpoint::Narrow && request.inspector.visible();
        Self {
            breakpoint,
            header,
            conversation: (!narrow_inspector_takes_body).then_some(conversation),
            activity: show_activity.then_some(activity),
            inspector: if drawer_replaces_activity {
                Some(drawer)
            } else {
                side_inspector
            },
            composer: Some(composer),
            hints,
        }
    }

    /// Regions actually drawn this frame, for layout assertions.
    pub fn visible_regions(&self) -> Vec<&'static str> {
        let mut regions = vec!["header"];
        if self.conversation.is_some() {
            regions.push("conversation");
        }
        if self.activity.is_some() {
            regions.push("activity");
        }
        if self.inspector.is_some() {
            regions.push("inspector");
        }
        if self.composer.is_some() {
            regions.push("composer");
        }
        regions.push("hints");
        regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LayoutRequest {
        LayoutRequest {
            composer_lines: 1,
            activity_lines: 4,
            inspector: InspectorReason::Hidden,
            focused_surface: false,
        }
    }

    #[test]
    fn breakpoints_match_the_product_contract() {
        assert_eq!(Breakpoint::of(160), Breakpoint::Wide);
        assert_eq!(Breakpoint::of(120), Breakpoint::Wide);
        assert_eq!(Breakpoint::of(119), Breakpoint::Standard);
        assert_eq!(Breakpoint::of(80), Breakpoint::Standard);
        assert_eq!(Breakpoint::of(79), Breakpoint::Narrow);
        assert_eq!(Breakpoint::of(60), Breakpoint::Narrow);
    }

    #[test]
    fn hidden_inspector_consumes_no_space() {
        let areas = WorkbenchAreas::compute(Rect::new(0, 0, 160, 40), request());
        assert!(areas.inspector.is_none());
        let conversation = areas.conversation.unwrap();
        assert_eq!(
            conversation.width, 160,
            "the conversation must reclaim the inspector column when it is hidden"
        );
    }

    #[test]
    fn wide_layout_places_the_inspector_beside_the_conversation() {
        let areas = WorkbenchAreas::compute(
            Rect::new(0, 0, 160, 40),
            LayoutRequest {
                inspector: InspectorReason::Requested,
                ..request()
            },
        );
        let conversation = areas.conversation.unwrap();
        let inspector = areas.inspector.unwrap();
        assert_eq!(conversation.y, inspector.y, "side by side, not stacked");
        assert_eq!(conversation.right(), inspector.x);
        assert!(areas.activity.is_some(), "wide keeps the activity rail");
    }

    #[test]
    fn standard_layout_opens_the_inspector_as_a_drawer_replacing_activity() {
        let areas = WorkbenchAreas::compute(
            Rect::new(0, 0, 100, 30),
            LayoutRequest {
                inspector: InspectorReason::Requested,
                ..request()
            },
        );
        assert_eq!(areas.breakpoint, Breakpoint::Standard);
        assert!(
            areas.activity.is_none(),
            "the drawer temporarily replaces activity instead of stacking"
        );
        assert!(areas.inspector.is_some());
        assert!(areas.conversation.is_some());
        assert!(areas.composer.is_some());
    }

    #[test]
    fn narrow_layout_shows_one_focused_surface_at_a_time() {
        let areas = WorkbenchAreas::compute(Rect::new(0, 0, 60, 24), request());
        assert_eq!(
            areas.visible_regions(),
            vec!["header", "conversation", "activity", "composer", "hints"],
            "narrow must not add an inspector column"
        );

        let opened = WorkbenchAreas::compute(
            Rect::new(0, 0, 60, 24),
            LayoutRequest {
                inspector: InspectorReason::Requested,
                ..request()
            },
        );
        assert!(
            opened.conversation.is_none(),
            "at 60 columns the inspector replaces the conversation"
        );
    }

    #[test]
    fn a_required_decision_owns_the_whole_narrow_frame() {
        let areas = WorkbenchAreas::compute(
            Rect::new(0, 0, 60, 24),
            LayoutRequest {
                inspector: InspectorReason::ApprovalRequired,
                ..request()
            },
        );
        assert_eq!(
            areas.visible_regions(),
            vec!["header", "inspector", "hints"],
            "approval at 60 columns must be a full-screen focused view"
        );
        let inspector = areas.inspector.unwrap();
        assert_eq!(inspector.width, 60);
        assert!(inspector.height >= 20);
    }

    #[test]
    fn narrow_never_shows_activity_inspector_and_composer_together() {
        for reason in [
            InspectorReason::Requested,
            InspectorReason::ApprovalRequired,
            InspectorReason::RecoveryRequired,
            InspectorReason::ValidationAttention,
        ] {
            let areas = WorkbenchAreas::compute(
                Rect::new(0, 0, 79, 24),
                LayoutRequest {
                    inspector: reason,
                    ..request()
                },
            );
            let crowded =
                areas.activity.is_some() && areas.inspector.is_some() && areas.composer.is_some();
            assert!(!crowded, "narrow layout crowded with {reason:?}");
        }
    }

    #[test]
    fn activity_never_outweighs_the_conversation() {
        for width in [60, 80, 120, 160] {
            let areas = WorkbenchAreas::compute(
                Rect::new(0, 0, width, 40),
                LayoutRequest {
                    activity_lines: 50,
                    ..request()
                },
            );
            let conversation = areas.conversation.unwrap().height;
            let activity = areas.activity.unwrap().height;
            assert!(
                activity < conversation,
                "at {width} columns activity {activity} must stay below conversation {conversation}"
            );
        }
    }

    #[test]
    fn a_tall_composer_is_bounded_so_the_conversation_survives() {
        let areas = WorkbenchAreas::compute(
            Rect::new(0, 0, 120, 30),
            LayoutRequest {
                composer_lines: 200,
                ..request()
            },
        );
        assert!(areas.composer.unwrap().height <= 9);
        assert!(areas.conversation.unwrap().height >= 3);
    }

    #[test]
    fn empty_activity_reclaims_its_rows() {
        let areas = WorkbenchAreas::compute(
            Rect::new(0, 0, 120, 30),
            LayoutRequest {
                activity_lines: 0,
                ..request()
            },
        );
        assert!(areas.activity.is_none());
    }

    #[test]
    fn a_very_small_terminal_still_produces_a_usable_frame() {
        let areas = WorkbenchAreas::compute(Rect::new(0, 0, 40, 10), request());
        assert!(areas.conversation.unwrap().height >= 1);
        assert!(areas.composer.is_some());
        assert_eq!(areas.header.height, 1);
        assert_eq!(areas.hints.height, 1);
    }
}
