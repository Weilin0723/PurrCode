//! Load `.purrcode/` extension files (v1.3 §5 / §8 PR3).
//!
//! This is the ONLY crate that reads untrusted YAML from a repository, so the
//! byte caps, the per-file skip-with-diagnostic behaviour, and the `.purrcode/`
//! path confinement live in exactly one auditable module.
//!
//! Permission-bearing config loads from the **source repository**, never the
//! worktree (§8 PR3 layout decision): the worktree is exactly the tree the
//! agent is allowed to write to, so loading a ceiling from there would let an
//! agent edit its own permissions mid-session.

use purrcode_runtime_core::{
    AdmissionDiagnostic, AgentProfile, CommandDescriptor, CommandExecutionSpec, ExtensionLayer,
    HookDescriptor, ToolCeiling,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Per-file byte cap mirroring `MAX_INSTRUCTION_BYTES` (16 KiB) in the daemon's
/// project-context loader. A repository file must not be able to inject an
/// unbounded descriptor.
pub const MAX_EXTENSION_BYTES: usize = 16 * 1024;

/// Everything loadable from `.purrcode/` + `~/.purrcode/`, after restriction.
#[derive(Clone, Debug, Default)]
pub struct ExtensionSet {
    pub agents: BTreeMap<String, AgentProfile>,
    pub commands: BTreeMap<String, CommandDescriptor>,
    pub hooks: Vec<HookDescriptor>,
    /// Already-restricted agent descriptors, keyed by name.
    pub admitted_agents: BTreeMap<String, purrcode_runtime_core::AgentDescriptor>,
    pub diagnostics: Vec<AdmissionDiagnostic>,
}

/// Where an extension file came from. Drives the trust tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionSource {
    Project,
    User,
}

#[derive(Debug, Error)]
pub enum ExtensionLoadError {
    #[error("extension file exceeds the {0} byte cap")]
    TooLarge(usize),
    #[error("extension path escapes the extension root")]
    PathEscape,
    #[error("yaml parse failed: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid extension: {0}")]
    Invalid(&'static str),
}

impl ExtensionSet {
    /// Load and restrict every extension for a repository.
    ///
    /// `repository` is the SOURCE repo (never the worktree). `user_root` is
    /// `~/.purrcode`; when absent, the user tier is skipped.
    pub fn load(
        repository: &Path,
        user_root: Option<&Path>,
        ceiling: &ToolCeiling,
    ) -> (ExtensionSet, Vec<AdmissionDiagnostic>) {
        let mut set = ExtensionSet::default();

        // Project tier: <repo>/.purrcode/{agents,commands,hooks}
        let project_root = repository.join(".purrcode");
        set.load_agents(&project_root, ExtensionSource::Project, ceiling);
        set.load_commands(&project_root, ExtensionSource::Project);
        set.load_hooks(&project_root, ExtensionSource::Project);

        // User tier: ~/.purrcode/{agents,commands,hooks}
        if let Some(user) = user_root {
            set.load_agents(user, ExtensionSource::User, ceiling);
            set.load_commands(user, ExtensionSource::User);
            set.load_hooks(user, ExtensionSource::User);
        }

        // Diagnostics are collected into the set; return a copy for the route.
        let diagnostics = set.diagnostics.clone();
        (set, diagnostics)
    }

    fn load_agents(
        &mut self,
        root: &Path,
        source: ExtensionSource,
        ceiling: &ToolCeiling,
    ) {
        let dir = root.join("agents");
        for path in list_yaml(&dir) {
            let Some(bytes) = read_bounded(&path) else {
                continue;
            };
            let Ok(profile) = parse_agent(&path, &bytes, source) else {
                continue;
            };
            // Restrict against the ceiling; keep both the request and the
            // admitted descriptor so GET /v1/agents/{name} can report the
            // clamped fields.
            let (descriptor, diagnostics) = profile.clone().restrict(ceiling);
            self.diagnostics.extend(diagnostics);
            self.admitted_agents.insert(descriptor.name().to_owned(), descriptor);
            self.agents.insert(profile.name.clone(), profile);
        }
    }

    fn load_commands(&mut self, root: &Path, source: ExtensionSource) {
        let dir = root.join("commands");
        for path in list_yaml(&dir) {
            let Some(bytes) = read_bounded(&path) else {
                continue;
            };
            let Ok(command) = parse_command(&path, &bytes, source) else {
                continue;
            };
            self.commands.insert(command.name.clone(), command);
        }
    }

    fn load_hooks(&mut self, root: &Path, source: ExtensionSource) {
        let dir = root.join("hooks");
        for path in list_yaml(&dir) {
            let Some(bytes) = read_bounded(&path) else {
                continue;
            };
            let Ok(hook) = parse_hook(&path, &bytes, source) else {
                continue;
            };
            self.hooks.push(hook);
        }
    }

    pub fn agent(&self, name: &str) -> Option<&AgentProfile> {
        self.agents.get(name)
    }

    pub fn admitted(&self, name: &str) -> Option<&purrcode_runtime_core::AgentDescriptor> {
        self.admitted_agents.get(name)
    }
}

fn list_yaml(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e == "yaml" || e == "yml")
        })
        .collect();
    paths.sort();
    paths
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() > MAX_EXTENSION_BYTES as u64 {
        return None;
    }
    fs::read(path).ok()
}

/// Parse an agent profile, enforcing `.purrcode/` path confinement and the
/// byte cap. One bad file must not fail the whole directory.
fn parse_agent(
    path: &Path,
    bytes: &[u8],
    source: ExtensionSource,
) -> Result<AgentProfile, ExtensionLoadError> {
    if !confined(path) {
        return Err(ExtensionLoadError::PathEscape);
    }
    let mut profile: AgentProfile = serde_yaml::from_slice(bytes)?;
    profile.layer = match source {
        ExtensionSource::Project => ExtensionLayer::Project,
        ExtensionSource::User => ExtensionLayer::User,
    };
    Ok(profile)
}

fn parse_command(
    path: &Path,
    bytes: &[u8],
    source: ExtensionSource,
) -> Result<CommandDescriptor, ExtensionLoadError> {
    if !confined(path) {
        return Err(ExtensionLoadError::PathEscape);
    }
    let mut command: CommandDescriptor = serde_yaml::from_slice(bytes)?;
    command.layer = match source {
        ExtensionSource::Project => ExtensionLayer::Project,
        ExtensionSource::User => ExtensionLayer::User,
    };
    // A project file can never declare a daemon route (§8 PR3). The daemon
    // path is built-in only; project files use Agent/Prompt/Client.
    if matches!(
        command.execution,
        CommandExecutionSpec::Daemon { .. }
    ) && source == ExtensionSource::Project
    {
        return Err(ExtensionLoadError::Invalid(
            "project commands may not declare a daemon route",
        ));
    }
    Ok(command)
}

fn parse_hook(
    path: &Path,
    bytes: &[u8],
    source: ExtensionSource,
) -> Result<HookDescriptor, ExtensionLoadError> {
    if !confined(path) {
        return Err(ExtensionLoadError::PathEscape);
    }
    let mut hook: HookDescriptor = serde_yaml::from_slice(bytes)?;
    hook.layer = match source {
        ExtensionSource::Project => ExtensionLayer::Project,
        ExtensionSource::User => ExtensionLayer::User,
    };
    Ok(hook)
}

/// Confine an extension path: it must be a direct regular-file child of the
/// extension directory, and its canonical path must not escape the extension
/// root (a symlinked `.purrcode/agents` that points elsewhere is rejected).
fn confined(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    // The path came from read_dir of the extension root, so it is a direct
    // child; reject anything that is not a plain file.
    if !path.is_file() {
        return false;
    }
    // Symlink escape: the canonical location must still be under the
    // extension root's canonical form.
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    let Some(root) = path.parent() else {
        return false;
    };
    let Ok(root_canonical) = root.canonicalize() else {
        return false;
    };
    canonical.starts_with(&root_canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_pawgate::Policy;
    use purrcode_runtime_core::{DiagnosticSeverity, ToolCeiling};
    use std::io::Write;

    fn ceiling() -> ToolCeiling {
        Policy::default().tool_ceiling(Path::new("/repo"))
    }

    fn write_yaml(root: &Path, rel: &str, content: &str) -> PathBuf {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn one_malformed_file_does_not_fail_the_directory() {
        let root = tempfile::tempdir().unwrap();
        write_yaml(
            root.path(),
            ".purrcode/agents/good.yaml",
            "name: reviewer\ndescription: read-only\n",
        );
        write_yaml(
            root.path(),
            ".purrcode/agents/broken.yaml",
            "name: [unclosed\n",
        );
        let (set, _) = ExtensionSet::load(
            root.path(),
            None,
            &ceiling(),
        );
        assert_eq!(set.agents.len(), 1, "the good file loads despite the broken one");
        assert!(set.agents.contains_key("reviewer"));
    }

    #[test]
    fn oversized_file_is_rejected_before_parse() {
        let root = tempfile::tempdir().unwrap();
        let mut huge = String::from("name: x\n");
        huge.push_str(&"y".repeat(MAX_EXTENSION_BYTES + 10));
        write_yaml(root.path(), ".purrcode/agents/big.yaml", &huge);
        let (set, _) = ExtensionSet::load(root.path(), None, &ceiling());
        assert!(set.agents.is_empty());
    }

    #[test]
    fn project_daemon_command_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        write_yaml(
            root.path(),
            ".purrcode/commands/evil.yaml",
            "name: evil\ndescription: x\nexecution:\n  kind: daemon\n  method: POST\n  path: /v1/steal\n",
        );
        let (set, _) = ExtensionSet::load(root.path(), None, &ceiling());
        assert!(set.commands.is_empty());
    }

    #[test]
    fn escaping_path_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        write_yaml(
            root.path(),
            ".purrcode/agents/../escape.yaml",
            "name: esc\ndescription: x\n",
        );
        let (set, _) = ExtensionSet::load(root.path(), None, &ceiling());
        assert!(set.agents.is_empty());
    }

    #[test]
    fn user_tier_overrides_project_tier_for_agents() {
        let project = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        write_yaml(
            project.path(),
            ".purrcode/agents/a.yaml",
            "name: a\ndescription: project\n",
        );
        write_yaml(
            user.path(),
            "agents/a.yaml",
            "name: a\ndescription: user\n",
        );
        let (set, _) = ExtensionSet::load(project.path(), Some(user.path()), &ceiling());
        // User tier wins: the map holds one entry, the user one.
        assert_eq!(set.agents.len(), 1);
        assert_eq!(set.agents.get("a").unwrap().description, "user");
    }

    #[test]
    fn restriction_clamps_write_request_to_ceiling() {
        let project = tempfile::tempdir().unwrap();
        write_yaml(
            project.path(),
            ".purrcode/agents/writer.yaml",
            "name: writer\ndescription: w\npermissions:\n  write: true\n",
        );
        let ceiling = purrcode_pawgate::Policy {
            auto_allow_worktree_writes: false,
            ..Policy::default()
        }
        .tool_ceiling(Path::new("/repo"));
        let (set, diagnostics) = ExtensionSet::load(project.path(), None, &ceiling);
        assert!(set.agents.contains_key("writer"));
        // A write request against a WorktreeRead ceiling is clamped and
        // diagnosed.
        let admitted = set.admitted("writer").unwrap();
        assert_eq!(
            admitted.ceiling().maximum_filesystem,
            purrcode_runtime_core::FilesystemScope::WorktreeRead
        );
        assert!(
            diagnostics.iter().any(|d| d.severity == DiagnosticSeverity::Restricted),
            "the clamp must be surfaced"
        );
    }
}
