//! The session settings the header shows: model, task mode, permission mode.
//!
//! These are the facts PRD §14 requires to be visible at all times and §3.5,
//! §12 require to be changeable without editing TOML or restarting. They live
//! in one place so the header, the composer, the command palette and the
//! session payload read the same value and cannot contradict each other.

/// What the user is asking PurrCode to do (PRD §11).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskMode {
    /// Questions about the repository. Reads only.
    Ask,
    /// Produce a durable plan without changing files.
    Plan,
    /// Edit, run, test, repair. The default.
    #[default]
    Build,
    /// Review the current diff for risk and regressions.
    Review,
}

impl TaskMode {
    pub const ALL: &'static [Self] = &[Self::Ask, Self::Plan, Self::Build, Self::Review];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Plan => "Plan",
            Self::Build => "Build",
            Self::Review => "Review",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Self::Ask),
            "plan" => Some(Self::Plan),
            "build" => Some(Self::Build),
            "review" => Some(Self::Review),
            _ => None,
        }
    }

    /// True when the mode must not change files. The session payload carries
    /// this so the daemon enforces it rather than trusting the client.
    pub const fn plan_only(self) -> bool {
        matches!(self, Self::Ask | Self::Plan)
    }

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(2);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// How much PurrCode may do without asking (PRD §12).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PermissionMode {
    /// Ask before writes, commands, installs, network and destructive actions.
    #[default]
    Ask,
    /// Repository-scoped reads, writes and recognised build/test commands run
    /// automatically; destructive or unexpected effects still ask.
    Auto,
    /// Everything the operating-system process, workspace and configured
    /// identities already permit. It grants nothing the process does not hold.
    FullAccess,
}

impl PermissionMode {
    pub const ALL: &'static [Self] = &[Self::Ask, Self::Auto, Self::FullAccess];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Auto => "Auto",
            Self::FullAccess => "Full Access",
        }
    }

    /// The authority mode the runtime records for this choice. Ask/Auto/Full
    /// Access is the user's vocabulary; governed/elevated/unrestricted is the
    /// authority contract's, and the session payload carries the latter.
    pub const fn authority_mode(self) -> &'static str {
        match self {
            Self::Ask => "governed",
            Self::Auto => "elevated",
            Self::FullAccess => "unrestricted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_")
            .as_str()
        {
            "ask" | "governed" => Some(Self::Ask),
            "auto" | "elevated" => Some(Self::Auto),
            "full" | "full_access" | "unrestricted" => Some(Self::FullAccess),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

pub struct StatusBar {
    pub model: String,
    pub privacy: String,
    pub local: bool,
    pub task_mode: TaskMode,
    pub permission: PermissionMode,
    pub context_info: String,
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            model: "none".into(),
            privacy: "local-only".into(),
            local: true,
            task_mode: TaskMode::default(),
            permission: PermissionMode::default(),
            context_info: String::new(),
        }
    }

    pub fn set_model(&mut self, model: &str) {
        self.model = model.to_string();
    }

    pub fn set_privacy(&mut self, privacy: &str) {
        self.privacy = privacy.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_modes_round_trip_through_their_labels() {
        for mode in TaskMode::ALL {
            assert_eq!(TaskMode::parse(mode.label()), Some(*mode));
        }
        assert_eq!(TaskMode::parse("BUILD"), Some(TaskMode::Build));
        assert_eq!(TaskMode::parse("nonsense"), None);
    }

    #[test]
    fn ask_and_plan_never_change_files() {
        assert!(TaskMode::Ask.plan_only());
        assert!(TaskMode::Plan.plan_only());
        assert!(!TaskMode::Build.plan_only());
        assert!(!TaskMode::Review.plan_only());
    }

    #[test]
    fn permission_modes_accept_both_vocabularies() {
        assert_eq!(
            PermissionMode::parse("Full Access"),
            Some(PermissionMode::FullAccess)
        );
        assert_eq!(
            PermissionMode::parse("full-access"),
            Some(PermissionMode::FullAccess)
        );
        assert_eq!(
            PermissionMode::parse("unrestricted"),
            Some(PermissionMode::FullAccess)
        );
        assert_eq!(PermissionMode::parse("auto"), Some(PermissionMode::Auto));
        assert_eq!(PermissionMode::parse("governed"), Some(PermissionMode::Ask));
        assert_eq!(PermissionMode::parse("root"), None);
    }

    #[test]
    fn permission_modes_map_onto_the_authority_contract() {
        assert_eq!(PermissionMode::Ask.authority_mode(), "governed");
        assert_eq!(PermissionMode::Auto.authority_mode(), "elevated");
        assert_eq!(PermissionMode::FullAccess.authority_mode(), "unrestricted");
    }

    #[test]
    fn cycling_wraps_in_both_families() {
        assert_eq!(TaskMode::Review.next(), TaskMode::Ask);
        assert_eq!(TaskMode::Build.next(), TaskMode::Review);
        assert_eq!(PermissionMode::FullAccess.next(), PermissionMode::Ask);
        assert_eq!(PermissionMode::Ask.next(), PermissionMode::Auto);
    }

    #[test]
    fn the_defaults_are_the_prd_defaults() {
        let bar = StatusBar::new();
        assert_eq!(bar.task_mode, TaskMode::Build);
        assert_eq!(bar.permission, PermissionMode::Ask);
    }
}
