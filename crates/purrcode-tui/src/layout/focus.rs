//! Explicit keyboard focus.
//!
//! Focus is a value, not an implicit consequence of which panel happens to be
//! visible. That makes the tab order testable and lets every surface render a
//! visible focus indicator for exactly one region.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Focus {
    #[default]
    Composer,
    Conversation,
    Activity,
    Inspector,
}

impl Focus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Conversation => "conversation",
            Self::Activity => "activity",
            Self::Inspector => "inspector",
        }
    }
}

/// The focusable regions of the current frame, in tab order.
///
/// The ring only ever contains regions the layout actually drew, so Tab can
/// never move focus onto something invisible.
#[derive(Clone, Debug, Default)]
pub struct FocusRing {
    regions: Vec<Focus>,
    current: Focus,
}

impl FocusRing {
    /// Build the ring from which regions exist this frame. The composer is
    /// always first when present, because typing is the common case.
    pub fn new(conversation: bool, activity: bool, inspector: bool, composer: bool) -> Self {
        let mut regions = Vec::new();
        if composer {
            regions.push(Focus::Composer);
        }
        if conversation {
            regions.push(Focus::Conversation);
        }
        if activity {
            regions.push(Focus::Activity);
        }
        if inspector {
            regions.push(Focus::Inspector);
        }
        let current = regions.first().copied().unwrap_or(Focus::Composer);
        Self { regions, current }
    }

    pub fn regions(&self) -> &[Focus] {
        &self.regions
    }

    pub fn current(&self) -> Focus {
        self.current
    }

    /// Keep an existing selection when the layout changes, but never leave focus
    /// on a region that disappeared.
    pub fn restore(mut self, previous: Focus) -> Self {
        if self.regions.contains(&previous) {
            self.current = previous;
        }
        self
    }

    pub fn next(&mut self) {
        self.step(1);
    }

    pub fn previous(&mut self) {
        self.step(-1);
    }

    fn step(&mut self, delta: isize) {
        if self.regions.is_empty() {
            return;
        }
        let position = self
            .regions
            .iter()
            .position(|region| *region == self.current)
            .unwrap_or(0) as isize;
        let length = self.regions.len() as isize;
        let next = (position + delta).rem_euclid(length) as usize;
        self.current = self.regions[next];
    }

    pub fn focus(&mut self, region: Focus) -> bool {
        if self.regions.contains(&region) {
            self.current = region;
            true
        } else {
            false
        }
    }

    pub fn is_focused(&self, region: Focus) -> bool {
        self.current == region
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_order_is_composer_conversation_activity_inspector() {
        let mut ring = FocusRing::new(true, true, true, true);
        assert_eq!(ring.current(), Focus::Composer);
        ring.next();
        assert_eq!(ring.current(), Focus::Conversation);
        ring.next();
        assert_eq!(ring.current(), Focus::Activity);
        ring.next();
        assert_eq!(ring.current(), Focus::Inspector);
        ring.next();
        assert_eq!(ring.current(), Focus::Composer, "the ring wraps");
    }

    #[test]
    fn reverse_order_mirrors_forward_order() {
        let mut ring = FocusRing::new(true, true, true, true);
        ring.previous();
        assert_eq!(ring.current(), Focus::Inspector);
        ring.previous();
        assert_eq!(ring.current(), Focus::Activity);
    }

    #[test]
    fn focus_never_lands_on_a_hidden_region() {
        let mut ring = FocusRing::new(true, false, false, true);
        assert_eq!(ring.regions(), &[Focus::Composer, Focus::Conversation]);
        assert!(!ring.focus(Focus::Inspector));
        ring.next();
        ring.next();
        assert_eq!(ring.current(), Focus::Composer);
    }

    #[test]
    fn a_disappearing_region_hands_focus_back_to_a_visible_one() {
        let restored = FocusRing::new(true, false, false, true).restore(Focus::Inspector);
        assert_eq!(
            restored.current(),
            Focus::Composer,
            "focus must not stay on a region the layout removed"
        );
    }

    #[test]
    fn an_existing_selection_survives_a_layout_change() {
        let restored = FocusRing::new(true, true, true, true).restore(Focus::Activity);
        assert_eq!(restored.current(), Focus::Activity);
    }

    #[test]
    fn exactly_one_region_is_focused_at_a_time() {
        let ring = FocusRing::new(true, true, true, true);
        let focused = ring
            .regions()
            .iter()
            .filter(|region| ring.is_focused(**region))
            .count();
        assert_eq!(focused, 1);
    }

    #[test]
    fn an_empty_frame_does_not_panic() {
        let mut ring = FocusRing::new(false, false, false, false);
        ring.next();
        ring.previous();
        assert_eq!(ring.current(), Focus::Composer);
        assert!(ring.regions().is_empty());
    }
}
