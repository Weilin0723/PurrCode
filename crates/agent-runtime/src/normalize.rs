use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use purrcode_runtime_core::adaptation::PermissionMode;
use purrcode_runtime_core::{
    ActionConstraints, ActionId, CapabilityRegistry, CommandAction, DeleteFileAction,
    JudgmentDecision, ProposedAction, RepositoryReadAction, SessionEvent, SessionState,
    ToolInvocation, WriteFileAction, canonicalize_repository_path,
};

use crate::errors::AgentError;
use crate::schema::AgentAction;

fn canonicalize_read_action(
    read: RepositoryReadAction,
) -> Result<RepositoryReadAction, AgentError> {
    use RepositoryReadAction::*;
    let err =
        |path: &Path| AgentError::InvalidModelTurn(format!("read path escapes worktree: {path:?}"));
    let canonicalize_paths = |paths: Vec<PathBuf>| {
        paths
            .into_iter()
            .map(|path| canonicalize_repository_path(&path).ok_or_else(|| err(&path)))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(match read {
        Find {
            paths,
            max_depth,
            max_entries,
        } => Find {
            paths: canonicalize_paths(paths)?,
            max_depth,
            max_entries,
        },
        List { paths, max_entries } => List {
            paths: canonicalize_paths(paths)?,
            max_entries,
        },
        RepositoryGrep {
            pattern,
            paths,
            case_insensitive,
            max_results,
            max_bytes,
        } => RepositoryGrep {
            pattern,
            paths: canonicalize_paths(paths)?,
            case_insensitive,
            max_results,
            max_bytes,
        },
        ReadFile { path, max_bytes } => ReadFile {
            path: canonicalize_repository_path(&path).ok_or_else(|| err(&path))?,
            max_bytes,
        },
        GitDiff { paths } => GitDiff {
            paths: canonicalize_paths(paths)?,
        },
        GitShow { revision, path } => GitShow {
            revision,
            path: canonicalize_repository_path(&path).ok_or_else(|| err(&path))?,
        },
        GitLsFiles { pathspec } => GitLsFiles {
            pathspec: canonicalize_paths(pathspec)?,
        },
        GitStatus => GitStatus,
        GitRevParse { revision } => GitRevParse { revision },
        GitLog { max_count, oneline } => GitLog { max_count, oneline },
    })
}

pub(crate) fn normalize_action(
    action: AgentAction,
    _worktree: &Path,
    profile: Option<&purrcode_runtime_core::AgentDescriptor>,
    registry: Option<&CapabilityRegistry>,
) -> Result<ProposedAction, AgentError> {
    // Profile-level allowlist enforcement BEFORE PawGate sees the action
    // (v1.3 §8 PR4). A project agent with `permissions.write: false` must be
    // refused here, not merely routed to approval — its ceiling already
    // forbids the write.
    if let Some(profile) = profile {
        let write_allowed = matches!(
            profile.ceiling().maximum_filesystem,
            purrcode_runtime_core::FilesystemScope::Worktree { .. }
        );
        let is_commit = matches!(
            &action,
            AgentAction::Tool { tool_id, .. } if tool_id.as_str() == "native:commit"
        );
        if !write_allowed
            && (matches!(
                action,
                AgentAction::WriteFile { .. } | AgentAction::DeleteFile { .. }
            ) || is_commit)
        {
            return Err(AgentError::InvalidModelTurn(
                "this agent profile is read-only; file mutation and commits are denied".into(),
            ));
        }
    }
    match action {
        AgentAction::Read(read) => {
            let read = canonicalize_read_action(read)?;
            read.validate_bounds()?;
            Ok(ProposedAction::RepositoryRead(read))
        }
        AgentAction::ReadCommand(command) => {
            let read = convert_legacy_command(&command).ok_or_else(|| {
                AgentError::InvalidModelTurn(
                    "legacy read command is unsupported, ambiguous, or unsafe".into(),
                )
            })?;
            let read = canonicalize_read_action(read)?;
            read.validate_bounds()?;
            Ok(ProposedAction::RepositoryRead(read))
        }
        AgentAction::WriteFile {
            path,
            content,
            expected_digest,
        } => {
            let path = canonicalize_repository_path(&path).ok_or_else(|| {
                AgentError::InvalidModelTurn(format!("write path escapes worktree: {path:?}"))
            })?;
            Ok(ProposedAction::WriteFile(WriteFileAction {
                path,
                content,
                expected_digest,
            }))
        }
        AgentAction::DeleteFile {
            path,
            expected_digest,
        } => {
            let path = canonicalize_repository_path(&path).ok_or_else(|| {
                AgentError::InvalidModelTurn(format!("delete path escapes worktree: {path:?}"))
            })?;
            Ok(ProposedAction::DeleteFile(DeleteFileAction {
                path,
                expected_digest,
            }))
        }
        AgentAction::Tool { tool_id, arguments } => {
            // A registry tool is admitted once per repository; the active
            // profile's allowlist (and its ceiling) were applied at admission.
            let Some(registry) = registry else {
                return Err(AgentError::InvalidModelTurn(format!(
                    "tool `{}` cannot be resolved: no tool registry is attached to this turn",
                    tool_id.as_str()
                )));
            };
            // The registry handed to a turn is the EFFECTIVE one
            // (`CapabilityRegistry::for_agent`): already filtered by the
            // profile's `ToolSelection` and already intersected with the
            // profile's ceiling on every axis. A tool the profile does not
            // select is therefore simply absent here — the same set the model
            // manifest was rendered from.
            let Some(descriptor) = registry.tool(&tool_id) else {
                return Err(AgentError::InvalidModelTurn(match profile {
                    Some(profile) => format!(
                        "tool `{}` is not available to agent profile `{}`",
                        tool_id.as_str(),
                        profile.name()
                    ),
                    None => format!(
                        "tool `{}` is not admitted in this registry",
                        tool_id.as_str()
                    ),
                }));
            };
            if let Some(profile) = profile {
                // Defence in depth. `for_agent` already forbids a write tool
                // under a read-only ceiling; this refuses one that reached here
                // through a registry that was not narrowed for this profile.
                let profile_allows_write = matches!(
                    profile.ceiling().maximum_filesystem,
                    purrcode_runtime_core::FilesystemScope::Worktree { .. }
                );
                if !profile_allows_write
                    && matches!(
                        descriptor.filesystem_scope(),
                        purrcode_runtime_core::FilesystemScope::Worktree { .. }
                    )
                {
                    return Err(AgentError::InvalidModelTurn(format!(
                        "tool `{}` mutates the worktree and is denied by this read-only agent profile",
                        tool_id.as_str()
                    )));
                }
            }
            if descriptor.approval_policy() == purrcode_runtime_core::ApprovalPolicy::Forbidden {
                return Err(AgentError::InvalidModelTurn(format!(
                    "tool `{}` is forbidden in this workspace",
                    tool_id.as_str()
                )));
            }
            // Native tools reuse the exact battle-tested legacy action path
            // (RepositoryRead / WriteFile / DeleteFile / Command) so their
            // execution, PawGate judgment, and digest accounting stay
            // byte-identical to today. The registry's role for native tools is
            // the manifest surface, the `allowed_tools` allowlist, and schema
            // validation — all enforced above.
            if descriptor.provider() == purrcode_runtime_core::ToolProvider::Native {
                if let Some(legacy) = convert_native_tool(&tool_id, &arguments, _worktree)? {
                    return Ok(legacy);
                }
            }
            // MCP and Skill tools flow through the provider dispatch executor.
            Ok(ProposedAction::Tool(ToolInvocation {
                tool_id,
                arguments,
                working_directory: _worktree.to_path_buf(),
                descriptor_digest: descriptor.descriptor_digest().to_owned(),
            }))
        }
    }
}

/// Convert a native registry-tool invocation back into its canonical legacy
/// action so it flows through the exact `ToolRuntime::execute` + `Policy::evaluate`
/// path native tools have used since v1.0. Returns `Ok(None)` when the tool id
/// is not a native tool (callers fall through to the `Tool` path).
fn convert_native_tool(
    tool_id: &purrcode_runtime_core::ToolId,
    arguments: &serde_json::Value,
    worktree: &Path,
) -> Result<Option<ProposedAction>, AgentError> {
    use purrcode_runtime_core::RepositoryReadAction as Read;
    let Some(name) = tool_id.as_str().strip_prefix("native:") else {
        return Ok(None);
    };
    let str_arg = |key: &str| arguments.get(key).and_then(serde_json::Value::as_str);
    let path_arg = |key: &str| {
        str_arg(key).map(PathBuf::from).ok_or_else(|| {
            AgentError::InvalidModelTurn(format!("native tool `{name}` requires `{key}`"))
        })
    };
    let paths_arg = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let read = match name {
        "git_status" => Some(Read::GitStatus),
        "git_rev_parse" => Some(Read::GitRevParse {
            revision: str_arg("revision").unwrap_or("HEAD").to_owned(),
        }),
        "git_log" => Some(Read::GitLog {
            max_count: arguments
                .get("max_count")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u32),
            oneline: arguments
                .get("oneline")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        }),
        "git_diff" => Some(Read::GitDiff {
            paths: paths_arg("paths"),
        }),
        "git_show" => Some(Read::GitShow {
            revision: str_arg("revision").unwrap_or("HEAD").to_owned(),
            // A bare `git show HEAD` (no path) is valid; default to empty path.
            path: str_arg("path").map(PathBuf::from).unwrap_or_default(),
        }),
        "git_ls_files" => Some(Read::GitLsFiles {
            pathspec: paths_arg("pathspec"),
        }),
        "repository_grep" => Some(Read::RepositoryGrep {
            pattern: str_arg("pattern")
                .ok_or_else(|| {
                    AgentError::InvalidModelTurn("repository_grep requires `pattern`".into())
                })?
                .to_owned(),
            paths: paths_arg("paths"),
            case_insensitive: arguments
                .get("case_insensitive")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            max_results: arguments
                .get("max_results")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u32)
                .unwrap_or(200),
            max_bytes: arguments
                .get("max_bytes")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(1_048_576),
        }),
        "find" => Some(Read::Find {
            paths: paths_arg("paths"),
            max_depth: arguments
                .get("max_depth")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u8)
                .unwrap_or(3),
            max_entries: arguments
                .get("max_entries")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u32)
                .unwrap_or(200),
        }),
        "list" => Some(Read::List {
            paths: paths_arg("paths"),
            max_entries: arguments
                .get("max_entries")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u32)
                .unwrap_or(200),
        }),
        "read_file" => Some(Read::ReadFile {
            path: path_arg("path")?,
            max_bytes: arguments
                .get("max_bytes")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(8192),
        }),
        "write_file" => {
            return Ok(Some(ProposedAction::WriteFile(
                purrcode_runtime_core::WriteFileAction {
                    path: path_arg("path")?,
                    content: str_arg("content")
                        .ok_or_else(|| {
                            AgentError::InvalidModelTurn("write_file requires `content`".into())
                        })?
                        .to_owned(),
                    expected_digest: arguments
                        .get("expected_digest")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                },
            )));
        }
        "delete_file" => {
            return Ok(Some(ProposedAction::DeleteFile(
                purrcode_runtime_core::DeleteFileAction {
                    path: path_arg("path")?,
                    expected_digest: arguments
                        .get("expected_digest")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                },
            )));
        }
        "command" => {
            let program = str_arg("program")
                .ok_or_else(|| AgentError::InvalidModelTurn("command requires `program`".into()))?;
            let program = PathBuf::from(program);
            return Ok(Some(ProposedAction::Command(
                purrcode_runtime_core::CommandAction {
                    program,
                    arguments: arguments
                        .get("arguments")
                        .and_then(serde_json::Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                    working_directory: worktree.to_path_buf(),
                    environment: std::collections::BTreeMap::new(),
                },
            )));
        }
        _ => None,
    };
    Ok(read.map(ProposedAction::RepositoryRead))
}

/// Compatibility adapter: convert a legacy [`CommandAction`] into a canonical
/// typed [`RepositoryReadAction`] when the program/arguments match a known safe
/// pattern. Returns `None` (and therefore a denial downstream) for unsafe
/// patterns such as pipelines, redirection, or `find -exec`.
///
/// This bridges the gap between the old shell-string agent interface and the
/// new typed-read architecture so that existing usage of `find`, `rg`, and safe
/// Git sub-commands produces the same deterministic authorization record as the
/// corresponding typed read, without requiring the model to switch to the new
/// schema first.
pub(crate) fn convert_legacy_command(command: &CommandAction) -> Option<RepositoryReadAction> {
    let program = command.program.file_name()?.to_str()?;
    let args: Vec<&str> = command.arguments.iter().map(String::as_str).collect();
    reject_unsafe_shell_patterns(&args)?;
    match program {
        "git" => convert_git(&args),
        "find" => convert_find(&args),
        "rg" | "rg.exe" => convert_rg(&args),
        _ => None,
    }
}

fn reject_unsafe_shell_patterns(args: &[&str]) -> Option<()> {
    for arg in args {
        if arg.contains('|')
            || arg.contains('>')
            || arg.contains('<')
            || arg.contains(';')
            || arg.contains('$')
            || arg.contains('`')
            || arg.contains("$( ")
        {
            return None;
        }
    }
    // Reject explicit shell wrappers such as "sh -c" or "bash -c".
    if args.len() >= 2 && matches!(args[0], "sh" | "bash" | "zsh" | "dash") && args[1] == "-c" {
        return None;
    }
    Some(())
}

fn convert_git(args: &[&str]) -> Option<RepositoryReadAction> {
    let safe = ["diff", "log", "ls-files", "rev-parse", "show", "status"];
    let subcommand = *args.first()?;
    if !safe.contains(&subcommand) {
        return None;
    }
    match subcommand {
        "status" => Some(RepositoryReadAction::GitStatus),
        "rev-parse" => {
            let revision = args.get(1)?.to_string();
            if args.len() != 2
                || revision.is_empty()
                || revision.starts_with('-')
                || revision.contains("..")
                || revision.contains(char::is_whitespace)
            {
                return None;
            }
            Some(RepositoryReadAction::GitRevParse { revision })
        }
        "log" => {
            let mut max_count = None;
            let mut oneline = false;
            let mut i = 1;
            while i < args.len() {
                match args[i] {
                    "--oneline" => oneline = true,
                    arg if arg.starts_with('-') && arg.len() > 1 => {
                        let n: u32 = arg[1..].parse().ok()?;
                        max_count = Some(n);
                    }
                    _ => return None,
                }
                i += 1;
            }
            Some(RepositoryReadAction::GitLog { max_count, oneline })
        }
        "diff" => {
            let mut paths = Vec::new();
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--" {
                    i += 1;
                    while i < args.len() {
                        let p = canonicalize_repository_path(Path::new(args[i]))?;
                        paths.push(p);
                        i += 1;
                    }
                    break;
                }
                if args[i].starts_with('-') {
                    return None;
                }
                i += 1;
            }
            Some(RepositoryReadAction::GitDiff { paths })
        }
        "show" => {
            let revision_and_path = args.get(1).map(|s| s.to_string())?;
            if revision_and_path.is_empty()
                || revision_and_path.contains("..")
                || revision_and_path.contains(char::is_whitespace)
            {
                return None;
            }
            if let Some(idx) = revision_and_path.find(':') {
                let revision = revision_and_path[..idx].to_string();
                let path = PathBuf::from(&revision_and_path[idx + 1..]);
                Some(RepositoryReadAction::GitShow { revision, path })
            } else {
                Some(RepositoryReadAction::GitShow {
                    revision: revision_and_path,
                    path: PathBuf::new(),
                })
            }
        }
        "ls-files" => {
            let pathspec: Vec<PathBuf> = args[1..]
                .iter()
                .filter_map(|s| canonicalize_repository_path(Path::new(s)))
                .collect();
            Some(RepositoryReadAction::GitLsFiles { pathspec })
        }
        _ => None,
    }
}

fn convert_find(args: &[&str]) -> Option<RepositoryReadAction> {
    if args.is_empty() {
        return None;
    }
    // Reject -exec, -ok, -delete, -execdir
    if args
        .iter()
        .any(|a| matches!(*a, "-exec" | "-ok" | "-delete" | "-execdir"))
    {
        return None;
    }
    let root = canonicalize_repository_path(Path::new(args[0]))?
        .to_string_lossy()
        .to_string();
    let paths = vec![PathBuf::from(&root)];
    let mut max_depth: u8 = 5;
    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "-maxdepth" | "--maxdepth" => {
                let depth: u8 = args.get(i + 1)?.parse().ok()?;
                if depth == 0 || depth > 5 {
                    return None;
                }
                max_depth = depth;
                i += 2;
            }
            "-not" => {
                // Only accept -not -path patterns for repository subtrees
                if args.get(i + 1) != Some(&"-path") {
                    return None;
                }
                let _pattern = args.get(i + 2)?;
                i += 3;
            }
            other if other.starts_with('-') => {
                // Reject any other find expression
                return None;
            }
            _ => {
                i += 1;
            }
        }
    }
    Some(RepositoryReadAction::Find {
        paths,
        max_depth,
        max_entries: purrcode_runtime_core::DEFAULT_LIST_MAX_ENTRIES,
    })
}

fn convert_rg(args: &[&str]) -> Option<RepositoryReadAction> {
    // Reject --pre which can execute arbitrary commands.
    if args
        .iter()
        .any(|a| *a == "--pre" || a.starts_with("--pre="))
    {
        return None;
    }
    let mut case_insensitive = false;
    let mut pattern: Option<String> = None;
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-i" | "--ignore-case" => case_insensitive = true,
            "--" => {
                // Everything after -- is positional: pattern then paths
                if let Some(p) = args.get(i + 1) {
                    pattern = Some(p.to_string());
                    for path in args.iter().skip(i + 2) {
                        if let Some(canon) = canonicalize_repository_path(Path::new(path)) {
                            paths.push(canon);
                        }
                    }
                }
                break;
            }
            arg if !arg.starts_with('-') => {
                if pattern.is_none() {
                    pattern = Some(arg.to_string());
                } else if let Some(canon) = canonicalize_repository_path(Path::new(arg)) {
                    paths.push(canon);
                }
            }
            unknown => {
                // Reject unsupported flags to ensure normalization is semantics-preserving.
                // Silently ignoring a flag would produce an action digest that differs from
                // the model's intent, making deduplication and audit unreliable.
                let _ = unknown;
                return None;
            }
        }
        i += 1;
    }
    Some(RepositoryReadAction::RepositoryGrep {
        pattern: pattern?,
        paths,
        case_insensitive,
        max_results: purrcode_runtime_core::DEFAULT_GREP_MAX_RESULTS,
        max_bytes: purrcode_runtime_core::DEFAULT_READ_FILE_MAX_BYTES,
    })
}

pub(crate) fn decision_constraints(decision: &JudgmentDecision) -> Option<&ActionConstraints> {
    match decision {
        JudgmentDecision::AllowWithConstraints(constraints)
        | JudgmentDecision::RequireApproval { constraints, .. } => Some(constraints),
        _ => None,
    }
}

/// Apply the session's permission mode to a PawGate decision.
///
/// Pure by design: (mode, decision, worktree) in, decision out, so the bypass
/// logic is reproducible and unit-testable without a provider or store.
///
/// * `Auto` (the default) converts a `RequireApproval` into an allow with the
///   exact constraints PawGate computed — execution stays bounded, only the
///   prompt is skipped. A `Deny` still stands (Auto does not override refusals).
/// * `FullAccess` additionally converts a `Deny` into a bounded read-only
///   allow, so the human's standing "do it" wins but the action still cannot
///   write or reach the network.
/// * `Ask` (Governed) leaves every decision untouched.
/// * `ModifyAction`/`Replan`/`Allow` are never touched: they are advice about
///   correctness, not authority.
///
/// v1.3 §9.4 Hole A: a registered tool with `AlwaysAsk` or higher friction, or
/// any side effect beyond read, is NEVER auto-approved — even under `Auto` or
/// `FullAccess`. And `FullAccess` never converts a `Deny` into an allow.
pub(crate) fn apply_permission_mode(
    mode: PermissionMode,
    decision: JudgmentDecision,
    _worktree: &Path,
    descriptor: Option<&purrcode_runtime_core::ToolDescriptor>,
) -> JudgmentDecision {
    use JudgmentDecision::*;
    if let Some(descriptor) = descriptor {
        if descriptor.approval_policy() >= purrcode_runtime_core::ApprovalPolicy::AlwaysAsk
            || descriptor.side_effect_class() > purrcode_runtime_core::SideEffectClass::Read
        {
            return decision;
        }
    }
    match mode {
        PermissionMode::Auto => match decision {
            RequireApproval { constraints, .. } => AllowWithConstraints(constraints),
            other => other,
        },
        PermissionMode::FullAccess => match decision {
            RequireApproval { constraints, .. } => AllowWithConstraints(constraints),
            // §9.4 Hole A: FullAccess NEVER converts a Deny into an allow. A
            // checked-in project file must not overrule an explicit
            // organizational Deny, and a session mode a human set for
            // convenience must not either.
            other => other,
        },
        PermissionMode::Ask => decision,
    }
}

/// Deterministically allowlisted repository reads are already confined to the
/// session worktree, denied network access, prohibited from writing, and
/// bounded by time/output limits. A semantic re-review can only make those
/// stable reads provider-dependent and spuriously turn routine exploration
/// into repeated approval prompts.
pub(crate) fn requires_contextual_judgment(
    action: &ProposedAction,
    deterministic: &JudgmentDecision,
) -> bool {
    !matches!(
        (action, deterministic),
        (
            ProposedAction::RepositoryRead(_) | ProposedAction::Command(_),
            JudgmentDecision::AllowWithConstraints(_)
        )
    )
}

pub(crate) fn successful_duplicate_action(
    state: &SessionState,
    session_events: &[SessionEvent],
    proposed: &ProposedAction,
    constraints: &ActionConstraints,
) -> Result<Option<ActionId>, AgentError> {
    let successful = session_events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ExecutionFinished {
                action_id,
                exit_code: Some(0),
                ..
            } => Some(*action_id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let proposed_digest = proposed.digest(constraints)?;
    for action_id in successful {
        let Some(previous) = state.proposed_actions.get(&action_id) else {
            continue;
        };
        let Some(previous_constraints) = state
            .judgments
            .get(&action_id)
            .and_then(decision_constraints)
        else {
            continue;
        };
        if previous.digest(previous_constraints)? == proposed_digest {
            return Ok(Some(action_id));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod action_normalization_tests {
    use super::{
        apply_permission_mode, convert_legacy_command, normalize_action,
        requires_contextual_judgment, successful_duplicate_action,
    };
    use crate::schema::AgentAction;
    use purrcode_pawgate::Policy;
    use purrcode_runtime_core::adaptation::PermissionMode;
    use purrcode_runtime_core::{
        ActionConstraints, ActionId, AgentDescriptor, CommandAction, JudgmentDecision,
        ProposedAction, RepositoryReadAction, SessionEvent, SessionId, SessionState,
    };
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::path::PathBuf;

    fn legacy_command(program: &str, args: &[&str]) -> CommandAction {
        CommandAction {
            program: PathBuf::from(program),
            arguments: args.iter().map(|s| (*s).to_owned()).collect(),
            working_directory: PathBuf::from("/repo"),
            environment: BTreeMap::new(),
        }
    }

    #[test]
    fn legacy_git_status_converts_to_typed_read() {
        let cmd = legacy_command("git", &["status"]);
        let read = convert_legacy_command(&cmd).unwrap();
        assert!(matches!(read, RepositoryReadAction::GitStatus));
    }

    #[test]
    fn legacy_git_log_converts_oneline_limit() {
        let cmd = legacy_command("git", &["log", "--oneline", "-5"]);
        let RepositoryReadAction::GitLog { max_count, oneline } =
            convert_legacy_command(&cmd).unwrap()
        else {
            panic!("expected GitLog")
        };
        assert_eq!(max_count, Some(5));
        assert!(oneline);
    }

    #[test]
    fn legacy_git_diff_with_paths_converts() {
        let cmd = legacy_command("git", &["diff", "--", "src/main.rs"]);
        let RepositoryReadAction::GitDiff { paths } = convert_legacy_command(&cmd).unwrap() else {
            panic!("expected GitDiff")
        };
        assert_eq!(paths, vec![PathBuf::from("src/main.rs")]);
    }

    #[test]
    fn legacy_git_commit_rejected_as_not_safe() {
        let cmd = legacy_command("git", &["commit", "-m", "unsafe"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_find_with_maxdepth_converts() {
        let cmd = legacy_command("find", &[".", "-maxdepth", "3"]);
        let RepositoryReadAction::Find {
            max_depth, paths, ..
        } = convert_legacy_command(&cmd).unwrap()
        else {
            panic!("expected Find")
        };
        assert_eq!(max_depth, 3);
        assert!(!paths.is_empty());
    }

    #[test]
    fn legacy_find_with_exec_rejected() {
        let cmd = legacy_command("find", &[".", "-maxdepth", "2", "-exec", "rm"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_rg_without_pre_converts() {
        let cmd = legacy_command("rg", &["TODO", "src/"]);
        let RepositoryReadAction::RepositoryGrep { pattern, .. } =
            convert_legacy_command(&cmd).unwrap()
        else {
            panic!("expected RepositoryGrep")
        };
        assert_eq!(pattern, "TODO");
    }

    #[test]
    fn legacy_rg_with_preprocessor_rejected() {
        let cmd = legacy_command("rg", &["--pre", "grep", "pattern", "."]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_shell_wrapper_rejected() {
        let cmd = legacy_command("sh", &["-c", "echo unsafe"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_git_rev_parse_converts() {
        let cmd = legacy_command("git", &["rev-parse", "HEAD"]);
        let read = convert_legacy_command(&cmd).unwrap();
        assert!(matches!(
            read,
            RepositoryReadAction::GitRevParse { ref revision } if revision == "HEAD"
        ));
    }

    #[test]
    fn legacy_git_show_with_revision_only_converts() {
        let cmd = legacy_command("git", &["show", "HEAD"]);
        let read = convert_legacy_command(&cmd).unwrap();
        assert!(matches!(
            read,
            RepositoryReadAction::GitShow { ref revision, ref path }
                if revision == "HEAD" && path.as_os_str().is_empty()
        ));
    }

    #[test]
    fn legacy_read_command_is_normalized_before_policy() {
        let worktree = std::env::temp_dir().join("purrcode-legacy-read-session");
        let action = normalize_action(
            AgentAction::ReadCommand(legacy_command("git", &["rev-parse", "HEAD"])),
            &worktree,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            action,
            ProposedAction::RepositoryRead(RepositoryReadAction::GitRevParse { ref revision })
                if revision == "HEAD"
        ));
    }

    #[test]
    fn legacy_git_show_with_path_converts() {
        let cmd = legacy_command("git", &["show", "HEAD:src/main.rs"]);
        let RepositoryReadAction::GitShow { path, .. } = convert_legacy_command(&cmd).unwrap()
        else {
            panic!("expected GitShow")
        };
        assert_eq!(path, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn legacy_git_diff_with_unknown_flag_rejected() {
        let cmd = legacy_command("git", &["diff", "--cached"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_git_diff_with_only_paths_converts() {
        let cmd = legacy_command("git", &["diff", "--", "src/main.rs"]);
        let RepositoryReadAction::GitDiff { paths } = convert_legacy_command(&cmd).unwrap() else {
            panic!("expected GitDiff")
        };
        assert_eq!(paths, vec![PathBuf::from("src/main.rs")]);
    }

    #[test]
    fn legacy_git_add_rejected_as_not_safe() {
        let cmd = legacy_command("git", &["add", "src/file.txt"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_git_push_rejected_as_not_safe() {
        let cmd = legacy_command("git", &["push"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_pipeline_rejected() {
        let cmd = legacy_command("rg", &["pattern", "|", "head"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn legacy_redirection_rejected() {
        let cmd = legacy_command("rg", &["pattern", ">", "output.txt"]);
        assert!(convert_legacy_command(&cmd).is_none());
    }

    #[test]
    fn typed_read_normalizes_to_repository_read() {
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let action = normalize_action(
            AgentAction::Read(RepositoryReadAction::GitStatus),
            worktree,
            None,
            None,
        )
        .unwrap();
        let ProposedAction::RepositoryRead(read) = action else {
            panic!("expected typed repository read")
        };
        assert!(matches!(read, RepositoryReadAction::GitStatus));
    }

    #[test]
    fn typed_read_with_paths_normalizes_to_repository_read() {
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let action = normalize_action(
            AgentAction::Read(RepositoryReadAction::RepositoryGrep {
                pattern: "TODO".into(),
                paths: vec!["src".into()],
                case_insensitive: false,
                max_results: 64,
                max_bytes: 4096,
            }),
            worktree,
            None,
            None,
        )
        .unwrap();
        let ProposedAction::RepositoryRead(read) = action else {
            panic!("expected typed repository read")
        };
        let RepositoryReadAction::RepositoryGrep { pattern, .. } = read else {
            panic!("expected grep variant")
        };
        assert_eq!(pattern, "TODO");
    }

    #[test]
    fn typed_list_accepts_empty_path_as_repository_root() {
        let action = normalize_action(
            AgentAction::Read(RepositoryReadAction::List {
                paths: vec![PathBuf::new()],
                max_entries: 32,
            }),
            Path::new("/repo/.purrcode/worktrees/session"),
            None,
            None,
        )
        .unwrap();
        let ProposedAction::RepositoryRead(RepositoryReadAction::List { paths, .. }) = action
        else {
            panic!("expected list read")
        };
        assert_eq!(paths, vec![PathBuf::new()]);
    }

    #[test]
    fn typed_read_reports_the_actual_escaping_path() {
        let error = normalize_action(
            AgentAction::Read(RepositoryReadAction::List {
                paths: vec![PathBuf::from("src"), PathBuf::from("/outside")],
                max_entries: 32,
            }),
            Path::new("/repo/.purrcode/worktrees/session"),
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "model turn is invalid: read path escapes worktree: \"/outside\""
        );
    }

    #[test]
    fn typed_deterministic_reads_do_not_gain_semantic_approval_prompts() {
        let worktree = std::env::temp_dir().join("purrcode-policy-read-session");
        let action = normalize_action(
            AgentAction::Read(RepositoryReadAction::List {
                paths: vec![PathBuf::from(".")],
                max_entries: 32,
            }),
            &worktree,
            None,
            None,
        )
        .unwrap();
        let decision = Policy::default().evaluate(&action, &worktree);
        assert!(matches!(
            decision,
            JudgmentDecision::AllowWithConstraints(_)
        ));
        assert!(!requires_contextual_judgment(&action, &decision));
    }

    #[test]
    fn writes_still_require_contextual_judgment() {
        let worktree = std::env::temp_dir()
            .join(".purrcode")
            .join("worktrees")
            .join("session");
        let action = normalize_action(
            AgentAction::WriteFile {
                path: "src/file.txt".into(),
                content: "value".into(),
                expected_digest: None,
            },
            &worktree,
            None,
            None,
        )
        .unwrap();
        let decision = Policy::default().evaluate(&action, &worktree);
        assert!(
            matches!(decision, JudgmentDecision::RequireApproval { .. }),
            "expected write to require approval, got {decision:?}"
        );
        assert!(requires_contextual_judgment(&action, &decision));
    }

    #[test]
    fn auto_mode_converts_approval_into_bounded_allow_but_not_denies() {
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let approval = JudgmentDecision::RequireApproval {
            reason: "policy wants a human".into(),
            constraints: ActionConstraints::read_only(worktree.to_path_buf()),
        };
        let converted = apply_permission_mode(PermissionMode::Auto, approval, worktree, None);
        assert!(
            matches!(converted, JudgmentDecision::AllowWithConstraints(_)),
            "Auto must skip the prompt but keep bounds, got {converted:?}"
        );
        // Auto does not override a refusal.
        let denied = apply_permission_mode(
            PermissionMode::Auto,
            JudgmentDecision::Deny {
                reason: "policy refuses".into(),
            },
            worktree,
            None,
        );
        assert!(matches!(denied, JudgmentDecision::Deny { .. }));
    }

    #[test]
    fn full_access_does_not_override_a_deny() {
        // v1.3 §9.4 Hole A: FullAccess must NOT convert a Deny into an allow —
        // a checked-in project file must not overrule an explicit
        // organizational Deny, and a session mode a human set for convenience
        // must not either.
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let denied = apply_permission_mode(
            PermissionMode::FullAccess,
            JudgmentDecision::Deny {
                reason: "policy refuses".into(),
            },
            worktree,
            None,
        );
        assert!(
            matches!(denied, JudgmentDecision::Deny { .. }),
            "FullAccess must never convert a Deny into an allow"
        );
    }

    #[test]
    fn auto_never_auto_approves_a_registered_non_read_tool() {
        // §9.4 Hole A: an AlwaysAsk / write-class descriptor is never
        // auto-approved, even under Auto.
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let mut registry = purrcode_runtime_core::CapabilityRegistry::new();
        let ceiling = purrcode_runtime_core::ToolCeiling {
            maximum_side_effect: purrcode_runtime_core::SideEffectClass::Destructive,
            maximum_network: purrcode_runtime_core::NetworkScope::Any,
            maximum_filesystem: purrcode_runtime_core::FilesystemScope::maximum(),
            minimum_approval: purrcode_runtime_core::ApprovalPolicy::PreAuthorized,
            denied_tool_ids: std::collections::BTreeSet::new(),
        };
        let write = registry
            .admit_tool(
                purrcode_runtime_core::ToolDescriptorProposal {
                    id: purrcode_runtime_core::ToolId::mcp("github", "create_issue"),
                    provider: purrcode_runtime_core::ToolProvider::Mcp,
                    display_name: "create_issue".into(),
                    description: "write".into(),
                    schema: serde_json::json!({ "type": "object" }),
                    capabilities: std::collections::BTreeSet::new(),
                    side_effect_class: purrcode_runtime_core::SideEffectClass::Write,
                    network_scope: purrcode_runtime_core::NetworkScope::None,
                    filesystem_scope: purrcode_runtime_core::FilesystemScope::WorktreeRead,
                    approval_policy: purrcode_runtime_core::ApprovalPolicy::ByClass,
                    origin: purrcode_runtime_core::DescriptorOrigin::RemoteDiscovery,
                },
                &ceiling,
            )
            .clone();
        let approval = JudgmentDecision::RequireApproval {
            reason: "tool may mutate external state".into(),
            constraints: ActionConstraints::read_only(worktree.to_path_buf()),
        };
        let kept = apply_permission_mode(PermissionMode::Auto, approval, worktree, Some(&write));
        assert!(
            matches!(kept, JudgmentDecision::RequireApproval { .. }),
            "a write-class registered tool must never be auto-approved"
        );
    }

    #[test]
    fn ask_leaves_every_decision_untouched() {
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let approval = JudgmentDecision::RequireApproval {
            reason: "policy wants a human".into(),
            constraints: ActionConstraints::read_only(worktree.to_path_buf()),
        };
        assert!(matches!(
            apply_permission_mode(PermissionMode::Ask, approval, worktree, None),
            JudgmentDecision::RequireApproval { .. }
        ));
        let advice = JudgmentDecision::Replan {
            reason: "plan drifted".into(),
        };
        assert!(matches!(
            apply_permission_mode(PermissionMode::FullAccess, advice, worktree, None),
            JudgmentDecision::Replan { .. }
        ));
    }

    #[test]
    fn exact_successful_action_is_reused_instead_of_replayed() {
        let worktree = std::env::temp_dir().join("purrcode-duplicate-read-session");
        let action = normalize_action(
            AgentAction::Read(RepositoryReadAction::List {
                paths: vec![PathBuf::from(".")],
                max_entries: 32,
            }),
            &worktree,
            None,
            None,
        )
        .unwrap();
        let decision = Policy::default().evaluate(&action, &worktree);
        let JudgmentDecision::AllowWithConstraints(constraints) = decision.clone() else {
            panic!("expected constrained allow")
        };
        let prior_id = ActionId::new();
        let mut state = SessionState::empty(SessionId::new());
        state.proposed_actions.insert(prior_id, action.clone());
        state.judgments.insert(prior_id, decision);
        let events = vec![SessionEvent::ExecutionFinished {
            action_id: prior_id,
            exit_code: Some(0),
            truncated: false,
            sandbox_level: None,
            sandbox_backend: None,
            affected_paths: Vec::new(),
        }];

        assert_eq!(
            successful_duplicate_action(&state, &events, &action, &constraints).unwrap(),
            Some(prior_id)
        );

        let distinct = normalize_action(
            AgentAction::Read(RepositoryReadAction::List {
                paths: vec![PathBuf::from("src")],
                max_entries: 32,
            }),
            &worktree,
            None,
            None,
        )
        .unwrap();
        let distinct_decision = Policy::default().evaluate(&distinct, &worktree);
        let JudgmentDecision::AllowWithConstraints(distinct_constraints) = distinct_decision else {
            panic!("expected constrained allow")
        };
        assert_eq!(
            successful_duplicate_action(&state, &events, &distinct, &distinct_constraints).unwrap(),
            None
        );
    }

    #[test]
    fn write_action_with_leading_parent_dir_is_rejected() {
        let worktree = std::env::temp_dir().join("purrcode-path-escape-session");
        let result = normalize_action(
            AgentAction::WriteFile {
                path: "../secret".into(),
                content: "leak".into(),
                expected_digest: None,
            },
            &worktree,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn delete_action_with_absolute_path_is_rejected() {
        let worktree = std::env::temp_dir().join("purrcode-path-escape-session");
        let result = normalize_action(
            AgentAction::DeleteFile {
                path: "/etc/passwd".into(),
                expected_digest: "digest".into(),
            },
            &worktree,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn read_only_profile_rejects_file_mutation_before_pawgate() {
        // v1.3 §8 PR4 acceptance test step 3: a project agent with
        // permissions.write:false proposing a WriteFile is refused in
        // normalize_action, BEFORE ActionProposed is appended.
        let worktree = std::env::temp_dir().join("purrcode-readonly-profile");
        let profile = AgentDescriptor::default(); // permissive default is read-only
        let error = normalize_action(
            AgentAction::WriteFile {
                path: "src/file.txt".into(),
                content: "value".into(),
                expected_digest: None,
            },
            &worktree,
            Some(&profile),
            None,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("read-only"),
            "write must be refused in normalize_action, got {error}"
        );
        // Reads still pass through a read-only profile.
        let read = normalize_action(
            AgentAction::Read(RepositoryReadAction::GitStatus),
            &worktree,
            Some(&profile),
            None,
        )
        .unwrap();
        assert!(matches!(read, ProposedAction::RepositoryRead(..)));
    }

    #[test]
    fn read_only_profile_cannot_bypass_via_native_write_tool() {
        use purrcode_runtime_core::{
            ApprovalPolicy, CapabilityRegistry, DescriptorOrigin, FilesystemScope, NetworkScope,
            SideEffectClass, ToolCeiling, ToolDescriptorProposal, ToolId, ToolProvider,
        };
        use std::collections::BTreeSet;
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let mut registry = CapabilityRegistry::new();
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::PreAuthorized,
            denied_tool_ids: BTreeSet::new(),
        };
        registry.admit_tool(
            ToolDescriptorProposal {
                id: ToolId::native("write_file"),
                provider: ToolProvider::Native,
                display_name: "write_file".into(),
                description: "write".into(),
                schema: serde_json::json!({ "type": "object" }),
                capabilities: BTreeSet::new(),
                side_effect_class: SideEffectClass::Write,
                network_scope: NetworkScope::None,
                filesystem_scope: FilesystemScope::maximum(),
                approval_policy: ApprovalPolicy::ByClass,
                origin: DescriptorOrigin::Builtin,
            },
            &ceiling,
        );
        // The default AgentDescriptor has a WorktreeRead ceiling (read-only).
        let profile = AgentDescriptor::default();
        let error = normalize_action(
            AgentAction::Tool {
                tool_id: ToolId::native("write_file"),
                arguments: serde_json::json!({ "path": "src/file.txt", "content": "x" }),
            },
            worktree,
            Some(&profile),
            Some(&registry),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("read-only"),
            "a read-only profile must refuse a native write tool, got {error}"
        );
    }

    #[test]
    fn registry_tool_is_resolved_and_native_tools_convert_to_legacy_actions() {
        use purrcode_runtime_core::{
            ApprovalPolicy, CapabilityRegistry, DescriptorOrigin, FilesystemScope, NetworkScope,
            SideEffectClass, ToolCeiling, ToolDescriptorProposal, ToolId, ToolProvider,
        };
        use std::collections::BTreeSet;
        let worktree = Path::new("/repo/.purrcode/worktrees/session");
        let mut registry = CapabilityRegistry::new();
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::PreAuthorized,
            denied_tool_ids: BTreeSet::new(),
        };
        registry.admit_tool(
            ToolDescriptorProposal {
                id: ToolId::native("read_file"),
                provider: ToolProvider::Native,
                display_name: "read_file".into(),
                description: "read a file".into(),
                schema: serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }),
                capabilities: BTreeSet::new(),
                side_effect_class: SideEffectClass::Read,
                network_scope: NetworkScope::None,
                filesystem_scope: FilesystemScope::WorktreeRead,
                approval_policy: ApprovalPolicy::ByClass,
                origin: DescriptorOrigin::Builtin,
            },
            &ceiling,
        );
        // A native tool converts back into its canonical legacy action so it
        // flows through the exact ToolRuntime path (byte-identical execution).
        let proposed = normalize_action(
            AgentAction::Tool {
                tool_id: ToolId::native("read_file"),
                arguments: serde_json::json!({ "path": "Cargo.toml", "max_bytes": 1024 }),
            },
            worktree,
            None,
            Some(&registry),
        )
        .unwrap();
        assert!(
            matches!(
                &proposed,
                ProposedAction::RepositoryRead(RepositoryReadAction::ReadFile { path, max_bytes })
                    if path == Path::new("Cargo.toml") && *max_bytes == 1024
            ),
            "native read_file must convert to RepositoryRead, got {proposed:?}"
        );
        // An unadmitted tool id is refused.
        let error = normalize_action(
            AgentAction::Tool {
                tool_id: ToolId::mcp("unknown", "tool"),
                arguments: serde_json::json!({}),
            },
            worktree,
            None,
            Some(&registry),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("not admitted"),
            "an unadmitted tool must be refused, got {error}"
        );
        // An MCP tool id produces a Tool invocation (not a legacy action).
        registry.admit_tool(
            ToolDescriptorProposal {
                id: ToolId::mcp("server", "ping"),
                provider: ToolProvider::Mcp,
                display_name: "ping".into(),
                description: "ping".into(),
                schema: serde_json::json!({ "type": "object" }),
                capabilities: BTreeSet::new(),
                side_effect_class: SideEffectClass::Read,
                network_scope: NetworkScope::None,
                filesystem_scope: FilesystemScope::WorktreeRead,
                approval_policy: ApprovalPolicy::ByClass,
                origin: DescriptorOrigin::RemoteDiscovery,
            },
            &ceiling,
        );
        let proposed = normalize_action(
            AgentAction::Tool {
                tool_id: ToolId::mcp("server", "ping"),
                arguments: serde_json::json!({}),
            },
            worktree,
            None,
            Some(&registry),
        )
        .unwrap();
        assert!(
            matches!(proposed, ProposedAction::Tool(ref invocation) if invocation.tool_id.as_str() == "mcp:server/ping"),
            "an mcp tool must produce a Tool invocation, got {proposed:?}"
        );
    }

    #[test]
    fn a_server_denied_mcp_tool_cannot_be_invoked_by_the_model() {
        // `deny_tools` is folded into the ceiling at admission, so the tool is
        // minted Forbidden. Normalization must refuse it — the generic,
        // model-driven path cannot be a way around a deny the explicit /mcp
        // endpoint honours.
        use purrcode_runtime_core::{
            ApprovalPolicy, CapabilityRegistry, DescriptorOrigin, FilesystemScope, NetworkScope,
            SideEffectClass, ToolCeiling, ToolDescriptorProposal, ToolId, ToolProvider,
        };
        use std::collections::BTreeSet;
        let worktree = Path::new("/repo");
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: ["mcp:github/delete_repo".to_string()].into_iter().collect(),
        };
        let mut registry = CapabilityRegistry::new();
        registry.admit_tool(
            ToolDescriptorProposal {
                id: ToolId::mcp("github", "delete_repo"),
                provider: ToolProvider::Mcp,
                display_name: "delete_repo".into(),
                description: "delete a repository".into(),
                schema: serde_json::json!({ "type": "object" }),
                capabilities: BTreeSet::new(),
                side_effect_class: SideEffectClass::Read,
                network_scope: NetworkScope::None,
                filesystem_scope: FilesystemScope::WorktreeRead,
                // Even a proposal claiming it needs no approval at all.
                approval_policy: ApprovalPolicy::PreAuthorized,
                origin: DescriptorOrigin::RemoteDiscovery,
            },
            &ceiling,
        );
        let error = normalize_action(
            AgentAction::Tool {
                tool_id: ToolId::mcp("github", "delete_repo"),
                arguments: serde_json::json!({}),
            },
            worktree,
            None,
            Some(&registry),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("forbidden"),
            "a denied MCP tool must be refused at normalization, got {error}"
        );
    }
}
