#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub colors_enabled: bool,
    pub unicode_enabled: bool,
}

impl Theme {
    pub fn detect() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        Self {
            colors_enabled: std::env::var_os("NO_COLOR").is_none() && term != "dumb",
            unicode_enabled: term != "dumb",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dumb_terminal_has_plain_text_fallback() {
        let theme = Theme {
            colors_enabled: false,
            unicode_enabled: false,
        };
        assert!(!theme.colors_enabled);
        assert!(!theme.unicode_enabled);
    }
}
