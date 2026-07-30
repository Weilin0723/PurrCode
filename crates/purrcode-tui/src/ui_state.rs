//! Small, repository-scoped recovery state. Secret-like draft values are never persisted.

use crate::composer::Composer;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Default, Deserialize, Serialize)]
pub struct UiState {
    repository: PathBuf,
    session_id: Option<String>,
    draft: String,
}

impl UiState {
    pub fn load(repository: &Path) -> Self {
        let Ok(bytes) = std::fs::read(state_path()) else {
            return Self::default();
        };
        let Ok(state) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        if state.repository == repository {
            state
        } else {
            Self::default()
        }
    }

    pub fn save(repository: &Path, session_id: Option<&str>, composer: &Composer) {
        let draft = safe_draft(&composer.buffer);
        let state = Self {
            repository: repository.to_owned(),
            session_id: session_id.map(str::to_owned),
            draft,
        };
        let path = state_path();
        let Some(parent) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let temporary = path.with_extension("tmp");
        if serde_json::to_vec(&state)
            .ok()
            .and_then(|bytes| std::fs::write(&temporary, bytes).ok())
            .is_some()
        {
            let _ = std::fs::rename(temporary, path);
        }
    }

    pub fn restore(self, composer: &mut Composer) -> Option<String> {
        composer.restore_draft(&self.draft);
        self.session_id
    }
}

fn safe_draft(draft: &str) -> String {
    match purrcode_provider_import::detect_content(draft) {
        Ok(detection) if detection.secret_findings.is_empty() => draft.to_owned(),
        Ok(_) => purrcode_provider_import::redact_source(draft)
            .map(|r| r.display)
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// Where repository-scoped recovery state lives.
///
/// `PURRCODE_STATE_DIR` overrides the platform data directory. End-to-end tests
/// rely on this to stay isolated from a developer's real state, and it lets a
/// user relocate the file without patching the binary.
fn state_path() -> PathBuf {
    if let Some(directory) = std::env::var_os("PURRCODE_STATE_DIR") {
        return PathBuf::from(directory).join("tui-state.json");
    }
    ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .map(|dirs| dirs.data_local_dir().join("tui-state.json"))
        .unwrap_or_else(|| PathBuf::from(".purrcode-tui-state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_state_never_keeps_secret_like_values() {
        let draft = safe_draft("OPENAI_API_KEY=sk-test-abcdefghijklmnopqrstuvwxyz123456");
        assert!(!draft.contains("sk-test-abcdefghijklmnopqrstuvwxyz123456"));
    }
}
