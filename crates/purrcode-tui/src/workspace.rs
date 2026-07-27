//! Bounded repository metadata for the workspace panel. File contents are never read.

use std::path::{Path, PathBuf};

const MAX_VISIBLE_PATHS: usize = 80;
// Startup is Tier 0: only root entries. Deeper traversal is task-triggered by Whisker.
const MAX_DEPTH: usize = 0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspacePath {
    pub display: String,
    pub directory: bool,
    pub sensitive: bool,
}

#[derive(Clone, Debug)]
pub struct WorkspaceContext {
    pub repository_name: String,
    pub branch: String,
    pub source_state: String,
    pub daemon_health: String,
    pub model: String,
    pub sandbox: String,
    pub session_phase: String,
    pub paths: Vec<WorkspacePath>,
    pub file_panel_visible: bool,
}

impl WorkspaceContext {
    pub fn inspect(repository: &Path) -> Self {
        let repository_name = repository
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repository")
            .to_owned();
        Self {
            repository_name,
            branch: read_branch(repository).unwrap_or_else(|| "detached/unknown".into()),
            // The TUI does not execute Git independently of the daemon. Do not invent clean state.
            source_state: "preserved".into(),
            daemon_health: "checking".into(),
            model: "detecting".into(),
            sandbox: "daemon-managed".into(),
            session_phase: "ready".into(),
            paths: bounded_paths(repository),
            file_panel_visible: false,
        }
    }

    pub fn toggle_files(&mut self) {
        self.file_panel_visible = !self.file_panel_visible;
    }
}

fn read_branch(repository: &Path) -> Option<String> {
    let dot_git = repository.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let pointer = std::fs::read_to_string(dot_git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir: ")?;
        let target = PathBuf::from(target);
        if target.is_absolute() {
            target
        } else {
            repository.join(target)
        }
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    Some(
        head.trim()
            .strip_prefix("ref: refs/heads/")
            .unwrap_or_else(|| head.trim().get(..12).unwrap_or(head.trim()))
            .to_owned(),
    )
}

fn bounded_paths(repository: &Path) -> Vec<WorkspacePath> {
    fn visit(root: &Path, directory: &Path, depth: usize, output: &mut Vec<WorkspacePath>) {
        if depth > MAX_DEPTH || output.len() >= MAX_VISIBLE_PATHS {
            return;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let remaining = MAX_VISIBLE_PATHS.saturating_sub(output.len());
        let mut bounded = Vec::with_capacity(remaining);
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | ".purrcode" | "node_modules" | "target"
            ) {
                continue;
            }
            if bounded.len() < remaining {
                bounded.push(entry);
                continue;
            }
            let Some((largest_index, largest)) = bounded
                .iter()
                .enumerate()
                .max_by_key(|(_, candidate): &(usize, &std::fs::DirEntry)| candidate.file_name())
            else {
                continue;
            };
            if entry.file_name() < largest.file_name() {
                bounded[largest_index] = entry;
            }
        }
        let mut entries = bounded;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            if output.len() >= MAX_VISIBLE_PATHS {
                break;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | ".purrcode" | "node_modules" | "target"
            ) {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
            let sensitive = is_sensitive_path(&relative);
            let directory = metadata.is_dir();
            output.push(WorkspacePath {
                display: relative.into_owned(),
                directory,
                sensitive,
            });
            if directory && !metadata.file_type().is_symlink() {
                visit(root, &path, depth + 1, output);
            }
        }
    }
    let mut paths = Vec::new();
    visit(repository, repository, 0, &mut paths);
    paths
}

fn is_sensitive_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == ".env"
        || lower.ends_with("/.env")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.contains("credential")
        || lower.contains("secret")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_panel_is_bounded_and_marks_sensitive_paths_without_reading_contents() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir(temporary.path().join(".git")).unwrap();
        std::fs::write(
            temporary.path().join(".git/HEAD"),
            "ref: refs/heads/feature/ui\n",
        )
        .unwrap();
        std::fs::write(temporary.path().join("README.md"), "visible").unwrap();
        std::fs::write(temporary.path().join(".env"), "NEVER_READ=this-value").unwrap();
        let context = WorkspaceContext::inspect(temporary.path());
        assert_eq!(context.branch, "feature/ui");
        assert!(context.paths.iter().any(|path| path.display == "README.md"));
        assert!(context
            .paths
            .iter()
            .any(|path| path.display == ".env" && path.sensitive));
        assert!(context.paths.len() <= MAX_VISIBLE_PATHS);
        assert!(!format!("{context:?}").contains("NEVER_READ"));
    }

    #[test]
    fn startup_listing_is_tier_zero_and_does_not_recurse() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("src/nested")).unwrap();
        std::fs::write(temporary.path().join("src/nested/large.rs"), "ignored").unwrap();
        let context = WorkspaceContext::inspect(temporary.path());
        assert!(context.paths.iter().any(|path| path.display == "src"));
        assert!(!context
            .paths
            .iter()
            .any(|path| path.display.contains("large.rs")));
    }

    #[test]
    fn startup_root_selection_is_sorted_and_memory_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        for index in (0..1_000).rev() {
            std::fs::write(
                temporary.path().join(format!("file-{index:04}.txt")),
                "metadata only",
            )
            .unwrap();
        }
        let context = WorkspaceContext::inspect(temporary.path());
        assert_eq!(context.paths.len(), MAX_VISIBLE_PATHS);
        assert!(context
            .paths
            .windows(2)
            .all(|pair| pair[0].display <= pair[1].display));
        assert_eq!(context.paths[0].display, "file-0000.txt");
        assert_eq!(
            context.paths[MAX_VISIBLE_PATHS - 1].display,
            "file-0079.txt"
        );
    }
}
