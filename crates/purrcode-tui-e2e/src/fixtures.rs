//! Isolated temporary repositories, tokens and state directories.
//!
//! Every test gets its own repository, its own daemon token and its own UI state
//! directory. Without that isolation a test would read a developer's real saved
//! draft and session, and a passing run would say nothing about a fresh install.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// A throwaway Git repository plus the isolated directories the workbench needs.
pub struct Workspace {
    root: tempfile::TempDir,
    token: String,
}

impl Workspace {
    /// Create a repository with one commit, so branch and status are meaningful.
    pub fn new() -> Result<Self> {
        Self::with_files(&[("README.md", "# fixture repository\n")])
    }

    pub fn with_files(files: &[(&str, &str)]) -> Result<Self> {
        let root = tempfile::tempdir().context("create temporary workspace")?;
        let repository = root.path().join("repository");
        std::fs::create_dir_all(&repository).context("create repository directory")?;
        std::fs::create_dir_all(root.path().join("state")).context("create state directory")?;

        for (path, contents) in files {
            let target = repository.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).context("create fixture directory")?;
            }
            std::fs::write(&target, contents).with_context(|| format!("write fixture {path}"))?;
        }

        git(&repository, &["init", "--initial-branch", "main"])?;
        git(
            &repository,
            &["config", "user.email", "fixture@purrcode.test"],
        )?;
        git(&repository, &["config", "user.name", "PurrCode Fixture"])?;
        git(&repository, &["config", "commit.gpgsign", "false"])?;
        git(&repository, &["add", "."])?;
        git(&repository, &["commit", "-m", "fixture"])?;

        // A distinct token per workspace: a test that accidentally reused another
        // daemon's token would pass for the wrong reason.
        let token = format!("token-{}", uuid::Uuid::new_v4().simple());
        std::fs::write(root.path().join("daemon.token"), &token).context("write daemon token")?;

        Ok(Self { root, token })
    }

    pub fn repository(&self) -> PathBuf {
        self.root.path().join("repository")
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn token_file(&self) -> PathBuf {
        self.root.path().join("daemon.token")
    }

    /// Directory the workbench writes its recovery state into.
    pub fn state_dir(&self) -> PathBuf {
        self.root.path().join("state")
    }

    /// The CLI's local session store. Every command opens one of these
    /// regardless of `workbench` mode attaching to a separate (fake) daemon,
    /// so without an isolated path here concurrent tests contend on the real
    /// shared default (`~/.local/share/purrcode/sessions.db`) and intermittently
    /// fail with "database is locked".
    pub fn database_path(&self) -> PathBuf {
        self.root.path().join("sessions.db")
    }

    /// Where failure artifacts are written.
    pub fn artifacts_dir(&self) -> PathBuf {
        self.root.path().join("artifacts")
    }

    /// Leave the workspace on disk so its artifacts survive the test process.
    ///
    /// Returns the path the directory was moved to.
    pub fn preserve(self, label: &str) -> Result<PathBuf> {
        let destination = artifact_root()?.join(label);
        if destination.exists() {
            std::fs::remove_dir_all(&destination).context("clear previous artifacts")?;
        }
        std::fs::create_dir_all(&destination).context("create artifact directory")?;
        let source = self.root.keep();
        copy_tree(&source, &destination)?;
        let _ = std::fs::remove_dir_all(&source);
        Ok(destination)
    }

    /// Make the repository dirty, for the dirty-repository capability.
    pub fn dirty(&self) -> Result<()> {
        std::fs::write(self.repository().join("uncommitted.txt"), "local change\n")
            .context("create uncommitted change")
    }

    /// Write a saved UI state file, simulating a previous session on this
    /// repository. Used by the restart and draft-restoration scenarios.
    pub fn seed_ui_state(&self, session_id: Option<&str>, draft: &str) -> Result<()> {
        // The workbench canonicalizes its repository argument, so the seeded
        // state must use the canonical form or it will look like state for a
        // different repository and be discarded.
        let repository = self
            .repository()
            .canonicalize()
            .unwrap_or_else(|_| self.repository());
        let state = serde_json::json!({
            "repository": repository,
            "session_id": session_id,
            "draft": draft,
        });
        std::fs::write(
            self.state_dir().join("tui-state.json"),
            serde_json::to_vec(&state).context("serialize ui state")?,
        )
        .context("write ui state")
    }

    /// The saved UI state, if the workbench wrote one.
    pub fn saved_ui_state(&self) -> Option<serde_json::Value> {
        let bytes = std::fs::read(self.state_dir().join("tui-state.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Root for preserved failure artifacts, honouring the CI convention.
pub fn artifact_root() -> Result<PathBuf> {
    let root = std::env::var_os("PURRCODE_E2E_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e-artifacts")
        });
    std::fs::create_dir_all(&root).context("create artifact root")?;
    Ok(root)
}

/// Path to the `purrcode` binary under test.
///
/// Prefers `PURRCODE_BIN` so CI can point at the released artifact, which is what
/// the dogfood suite requires; otherwise falls back to the workspace build next
/// to the test executable.
pub fn purrcode_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PURRCODE_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "PURRCODE_BIN points at {}, which is not a file",
            path.display()
        );
    }
    let current = std::env::current_exe().context("locate the test executable")?;
    // target/<profile>/deps/<test> → target/<profile>/purrcode
    let candidate = current.parent().and_then(Path::parent).map(|directory| {
        directory.join(if cfg!(windows) {
            "purrcode.exe"
        } else {
            "purrcode"
        })
    });
    match candidate {
        Some(path) if path.is_file() => Ok(path),
        Some(path) => bail!(
            "the purrcode binary is missing at {}; run `cargo build -p purrcode-cli` or set PURRCODE_BIN",
            path.display()
        ),
        None => bail!("could not locate the workspace target directory"),
    }
}

fn git(repository: &Path, arguments: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .with_context(|| format!("run git {}", arguments.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Copy a failed run's artifacts somewhere durable.
///
/// Entries that cannot be copied are skipped rather than propagated. A
/// workspace is a live git repository with a daemon and a PTY child that may
/// still be exiting, so a lock file or a socket can vanish between the
/// directory listing and the copy — and a broken symlink cannot be copied at
/// all. Failing here would fail the test this exists to help debug, turning a
/// diagnostic aid into a second, unrelated failure.
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in std::fs::read_dir(source).context("read artifact source")? {
        let Ok(entry) = entry else { continue };
        let target = destination.join(entry.file_name());
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            std::fs::create_dir_all(&target).context("create artifact subdirectory")?;
            copy_tree(&entry.path(), &target)?;
        } else if std::fs::copy(entry.path(), &target).is_err() {
            continue;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_is_a_real_git_repository_on_main() {
        let workspace = Workspace::new().expect("create workspace");
        assert!(workspace.repository().join(".git").is_dir());
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(workspace.repository())
            .output()
            .expect("git rev-parse");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
    }

    #[test]
    fn a_fresh_workspace_is_clean_and_can_be_made_dirty() {
        let workspace = Workspace::new().expect("create workspace");
        let status = |workspace: &Workspace| {
            let output = Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(workspace.repository())
                .output()
                .expect("git status");
            String::from_utf8_lossy(&output.stdout).to_string()
        };
        assert!(status(&workspace).trim().is_empty());
        workspace.dirty().expect("dirty the repository");
        assert!(status(&workspace).contains("uncommitted.txt"));
    }

    #[test]
    fn fixture_files_are_written_and_committed() {
        let workspace = Workspace::with_files(&[
            ("src/lib.rs", "pub fn one() {}\n"),
            ("docs/readme.md", "docs\n"),
        ])
        .expect("create workspace");
        assert!(workspace.repository().join("src/lib.rs").is_file());
        assert!(workspace.repository().join("docs/readme.md").is_file());
    }

    #[test]
    fn each_workspace_gets_its_own_token_and_directories() {
        let first = Workspace::new().expect("first");
        let second = Workspace::new().expect("second");
        assert_ne!(first.token(), second.token());
        assert_ne!(first.repository(), second.repository());
        assert_ne!(first.state_dir(), second.state_dir());
        assert_eq!(
            std::fs::read_to_string(first.token_file()).expect("read token"),
            first.token()
        );
    }

    #[test]
    fn seeded_ui_state_is_readable_back() {
        let workspace = Workspace::new().expect("create workspace");
        workspace
            .seed_ui_state(Some("session-42"), "an unsent draft")
            .expect("seed state");
        let state = workspace.saved_ui_state().expect("state must exist");
        assert_eq!(state["session_id"], "session-42");
        assert_eq!(state["draft"], "an unsent draft");
    }

    #[test]
    fn a_workspace_with_no_saved_state_reports_none() {
        let workspace = Workspace::new().expect("create workspace");
        assert!(workspace.saved_ui_state().is_none());
    }

    #[test]
    fn artifacts_can_be_preserved_outside_the_temporary_directory() {
        let workspace = Workspace::new().expect("create workspace");
        std::fs::create_dir_all(workspace.artifacts_dir()).expect("artifacts dir");
        std::fs::write(workspace.artifacts_dir().join("screen-after.txt"), "screen")
            .expect("write artifact");
        let preserved = workspace
            .preserve("fixtures-preserve-test")
            .expect("preserve");
        assert!(preserved.join("artifacts/screen-after.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(preserved.join("artifacts/screen-after.txt"))
                .expect("read preserved artifact"),
            "screen"
        );
        std::fs::remove_dir_all(preserved).expect("clean up");
    }

    #[test]
    fn preserving_artifacts_survives_an_entry_that_cannot_be_copied() {
        // A workspace holds a live git repository and a PTY child that may still
        // be exiting, so an entry can vanish or be a broken symlink. Preserving
        // artifacts must not fail because of one — it is a debugging aid, and
        // failing here replaces the real failure with an unrelated one.
        let workspace = Workspace::new().expect("create workspace");
        std::fs::create_dir_all(workspace.artifacts_dir()).expect("artifacts dir");
        std::fs::write(workspace.artifacts_dir().join("screen.txt"), "screen")
            .expect("write artifact");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            workspace.artifacts_dir().join("does-not-exist"),
            workspace.artifacts_dir().join("dangling"),
        )
        .expect("create dangling symlink");

        let preserved = workspace
            .preserve("fixtures-preserve-broken-entry")
            .expect("preserve must not fail on an uncopyable entry");
        assert!(preserved.join("artifacts/screen.txt").is_file());
        std::fs::remove_dir_all(preserved).expect("clean up");
    }

    #[test]
    fn the_binary_under_test_is_locatable_or_explains_how_to_build_it() {
        match purrcode_binary() {
            Ok(path) => assert!(path.is_file()),
            Err(error) => {
                let message = error.to_string();
                assert!(
                    message.contains("cargo build") || message.contains("PURRCODE_BIN"),
                    "the error must say how to fix it: {message}"
                );
            }
        }
    }
}
