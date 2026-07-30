//! Semantic style tokens.
//!
//! No primary component may name a terminal colour. Instead it names a [`Role`]
//! and an [`Emphasis`], and [`Tokens`] resolves those against the active theme.
//! When colour is unavailable the same call still returns a distinguishable
//! style built from weight and dimming, so status never depends on colour alone.

use ratatui::style::{Color, Modifier, Style};

use crate::theme::{Palette, Theme};

/// What a piece of interface *means*, independent of how it is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Body text.
    Primary,
    /// Secondary text: labels, hints, counts.
    Muted,
    /// Region separators and box edges.
    Border,
    /// Interactive affordance.
    Accent,
    /// Currently selected row or focused region.
    Selected,
    /// Completed or verified.
    Success,
    /// Needs attention but is not a failure.
    Warning,
    /// Failed or unsafe.
    Danger,
    /// A raised surface such as a focused modal.
    Elevated,
}

/// Weight applied on top of a role.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Emphasis {
    #[default]
    Normal,
    Strong,
    Dim,
}

#[derive(Clone, Copy, Debug)]
pub struct Tokens<'theme> {
    theme: &'theme Theme,
}

impl<'theme> Tokens<'theme> {
    pub fn new(theme: &'theme Theme) -> Self {
        Self { theme }
    }

    pub fn palette(&self) -> &Palette {
        &self.theme.palette
    }

    pub fn colors_enabled(&self) -> bool {
        self.theme.colors_enabled
    }

    pub fn unicode(&self) -> bool {
        self.theme.unicode_enabled
    }

    /// Resolve a role at normal emphasis.
    pub fn style(&self, role: Role) -> Style {
        self.styled(role, Emphasis::Normal)
    }

    /// Resolve a role and emphasis into a concrete style.
    ///
    /// Without colour, roles collapse onto weight and dimming. That keeps
    /// `Danger` and `Muted` visually distinct on a monochrome terminal, which a
    /// colour-only mapping would not.
    pub fn styled(&self, role: Role, emphasis: Emphasis) -> Style {
        let palette = &self.theme.palette;
        if !self.theme.colors_enabled {
            let base = Style::default().fg(palette.text_primary);
            return match (role, emphasis) {
                (Role::Danger | Role::Warning, _) => base.add_modifier(Modifier::BOLD),
                (Role::Success, _) => base.add_modifier(Modifier::BOLD),
                (Role::Selected, _) => base.add_modifier(Modifier::REVERSED),
                (Role::Muted | Role::Border, _) => base.add_modifier(Modifier::DIM),
                (_, Emphasis::Strong) => base.add_modifier(Modifier::BOLD),
                (_, Emphasis::Dim) => base.add_modifier(Modifier::DIM),
                _ => base,
            };
        }
        let color = match role {
            Role::Primary => palette.text_primary,
            Role::Muted => palette.text_muted,
            Role::Border => palette.border,
            Role::Accent => palette.accent,
            Role::Selected => palette.selected,
            Role::Success => palette.success,
            Role::Warning => palette.warning,
            Role::Danger => palette.danger,
            Role::Elevated => palette.text_primary,
        };
        let mut style = Style::default().fg(color);
        if role == Role::Elevated {
            style = style.bg(palette.elevated);
        }
        if role == Role::Selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        match emphasis {
            Emphasis::Strong => style.add_modifier(Modifier::BOLD),
            Emphasis::Dim => style.add_modifier(Modifier::DIM),
            Emphasis::Normal => style,
        }
    }

    /// Background for a surface level. When only black and white backgrounds are
    /// safe, all three levels are the same colour and depth comes from spacing,
    /// separators and weight instead.
    pub fn background(&self, elevated: bool) -> Color {
        if elevated {
            self.theme.palette.elevated
        } else {
            self.theme.palette.canvas
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dark() -> Theme {
        Theme {
            palette: Palette::dark(),
            colors_enabled: true,
            unicode_enabled: true,
        }
    }

    fn mono() -> Theme {
        Theme {
            palette: Palette::monochrome(),
            colors_enabled: false,
            unicode_enabled: false,
        }
    }

    const EVERY_ROLE: &[Role] = &[
        Role::Primary,
        Role::Muted,
        Role::Border,
        Role::Accent,
        Role::Selected,
        Role::Success,
        Role::Warning,
        Role::Danger,
        Role::Elevated,
    ];

    #[test]
    fn every_role_resolves_without_naming_a_colour_at_the_call_site() {
        let theme = dark();
        let tokens = Tokens::new(&theme);
        for role in EVERY_ROLE {
            let style = tokens.style(*role);
            assert!(style.fg.is_some(), "{role:?} must resolve a foreground");
        }
    }

    #[test]
    fn status_roles_stay_distinguishable_without_colour() {
        let theme = mono();
        let tokens = Tokens::new(&theme);
        let danger = tokens.style(Role::Danger);
        let muted = tokens.style(Role::Muted);
        let primary = tokens.style(Role::Primary);
        assert_ne!(
            danger, muted,
            "danger and muted must differ without colour support"
        );
        assert_ne!(danger, primary);
        assert_ne!(muted, primary);
    }

    #[test]
    fn selected_rows_are_marked_even_without_colour() {
        let theme = mono();
        let tokens = Tokens::new(&theme);
        let selected = tokens.style(Role::Selected);
        assert!(selected.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn emphasis_changes_weight_not_meaning() {
        let theme = dark();
        let tokens = Tokens::new(&theme);
        let normal = tokens.styled(Role::Primary, Emphasis::Normal);
        let strong = tokens.styled(Role::Primary, Emphasis::Strong);
        assert_eq!(normal.fg, strong.fg);
        assert!(strong.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn no_color_never_emits_a_palette_hue() {
        let theme = mono();
        let tokens = Tokens::new(&theme);
        for role in EVERY_ROLE {
            let style = tokens.style(*role);
            assert_eq!(
                style.fg,
                Some(Palette::monochrome().text_primary),
                "{role:?} must use the single high-contrast foreground"
            );
        }
    }
}
