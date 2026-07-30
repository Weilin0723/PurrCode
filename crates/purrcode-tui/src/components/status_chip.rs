//! A small labelled chip.
//!
//! Used for session state, privacy state and waiting state. A chip always
//! carries a word, never only a colour or glyph, so its meaning survives
//! monochrome terminals and screen readers reading the raw text.

use ratatui::text::Span;

use crate::design::{Emphasis, Role, Symbols, Tokens};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChipTone {
    Neutral,
    Positive,
    Attention,
    Negative,
}

impl ChipTone {
    const fn role(self) -> Role {
        match self {
            Self::Neutral => Role::Muted,
            Self::Positive => Role::Success,
            Self::Attention => Role::Warning,
            Self::Negative => Role::Danger,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StatusChip<'label> {
    pub label: &'label str,
    pub tone: ChipTone,
}

impl<'label> StatusChip<'label> {
    pub const fn new(label: &'label str, tone: ChipTone) -> Self {
        Self { label, tone }
    }

    pub fn span(&self, tokens: &Tokens<'_>) -> Span<'static> {
        let symbols = Symbols::new(tokens.unicode());
        let text = match self.tone {
            ChipTone::Attention | ChipTone::Negative => {
                format!("{} {}", symbols.attention(), self.label)
            }
            _ => self.label.to_owned(),
        };
        Span::styled(
            text,
            tokens.styled(
                self.tone.role(),
                if self.tone == ChipTone::Neutral {
                    Emphasis::Dim
                } else {
                    Emphasis::Strong
                },
            ),
        )
    }
}

/// Chip for a derived session state.
pub fn session_chip(state: crate::activity::SessionState) -> StatusChip<'static> {
    use crate::activity::SessionState;
    let tone = match state {
        SessionState::Safe => ChipTone::Neutral,
        SessionState::Completed => ChipTone::Positive,
        SessionState::Paused | SessionState::Uncertain | SessionState::Recoverable => {
            ChipTone::Attention
        }
        SessionState::Failed => ChipTone::Negative,
    };
    StatusChip::new(state.label(), tone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::SessionState;
    use crate::test_fixtures::{monochrome_theme, test_theme};

    fn text(chip: &StatusChip<'_>, mono: bool) -> String {
        let theme = if mono {
            monochrome_theme()
        } else {
            test_theme()
        };
        chip.span(&Tokens::new(&theme)).content.to_string()
    }

    #[test]
    fn every_chip_carries_a_word() {
        for tone in [
            ChipTone::Neutral,
            ChipTone::Positive,
            ChipTone::Attention,
            ChipTone::Negative,
        ] {
            let chip = StatusChip::new("state", tone);
            assert!(text(&chip, true).contains("state"));
        }
    }

    #[test]
    fn attention_tones_are_marked_in_the_text_not_only_in_colour() {
        assert!(text(&StatusChip::new("failed", ChipTone::Negative), true).starts_with('!'));
        assert!(text(&StatusChip::new("uncertain", ChipTone::Attention), true).starts_with('!'));
        assert!(!text(&StatusChip::new("safe", ChipTone::Neutral), true).starts_with('!'));
    }

    #[test]
    fn uncertain_is_never_toned_as_a_success() {
        let uncertain = session_chip(SessionState::Uncertain);
        let completed = session_chip(SessionState::Completed);
        assert_eq!(uncertain.tone, ChipTone::Attention);
        assert_eq!(completed.tone, ChipTone::Positive);
        assert_ne!(uncertain.tone, completed.tone);
    }

    #[test]
    fn every_session_state_maps_to_a_chip_with_its_own_label() {
        for state in [
            SessionState::Safe,
            SessionState::Paused,
            SessionState::Completed,
            SessionState::Uncertain,
            SessionState::Recoverable,
            SessionState::Failed,
        ] {
            let chip = session_chip(state);
            assert_eq!(chip.label, state.label());
            assert!(!text(&chip, false).is_empty());
        }
    }

    #[test]
    fn states_needing_attention_are_never_neutral() {
        for state in [
            SessionState::Paused,
            SessionState::Uncertain,
            SessionState::Recoverable,
            SessionState::Failed,
        ] {
            assert_ne!(
                session_chip(state).tone,
                ChipTone::Neutral,
                "{state:?} must stand out"
            );
        }
    }
}
