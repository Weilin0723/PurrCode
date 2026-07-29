use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use purrcode_runtime_core::{
    canonicalize_repository_path, ActionConstraints, ActionId, CommandAction, DeleteFileAction,
    JudgmentDecision, ProposedAction, RepositoryReadAction, SessionEvent, SessionState,
    WriteFileAction,
};

use crate::errors::AgentError;
use crate::schema::AgentAction;

fn canonicalize_read_action(
    read: RepositoryReadAction,
) -> Result<RepositoryReadAction, AgentError> {
    use RepositoryReadAction::*;
    let err =
        |path: &Path| AgentError::InvalidModelTurn(format!("read path escapes worktree: {path:?}"));
    Ok(match read {
        Find {
            paths,
            max_depth,
            max_entries,
        } => Find {
            paths: paths
                .into_iter()
                .map(|p| canonicalize_repository_path(&p))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| err(&PathBuf::new()))?,
            max_depth,
            max_entries,
        },
        List { paths, max_entries } => List {
            paths: paths
                .into_iter()
                .map(|p| canonicalize_repository_path(&p))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| err(&PathBuf::new()))?,
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
            paths: paths
                .into_iter()
                .map(|p| canonicalize_repository_path(&p))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| err(&PathBuf::new()))?,
            case_insensitive,
            max_results,
            max_bytes,
        },
        ReadFile { path, max_bytes } => ReadFile {
            path: canonicalize_repository_path(&path).ok_or_else(|| err(&path))?,
            max_bytes,
        },
        GitDiff { paths } => GitDiff {
            paths: paths
                .into_iter()
                .map(|p| canonicalize_repository_path(&p))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| err(&PathBuf::new()))?,
        },
        GitShow { revision, path } => GitShow {
            revision,
            path: canonicalize_repository_path(&path).ok_or_else(|| err(&path))?,
        },
        GitLsFiles { pathspec } => GitLsFiles {
            pathspec: pathspec
                .into_iter()
                .map(|p| canonicalize_repository_path(&p))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| err(&PathBuf::new()))?,
        },
        GitStatus => GitStatus,
        GitRevParse { revision } => GitRevParse { revision },
        GitLog { max_count, oneline } => GitLog { max_count, oneline },
    })
}

pub(crate) fn normalize_action(
    action: AgentAction,
    _worktree: &Path,
) -> Result<ProposedAction, AgentError> {
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
    }
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
                    _ => {}
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
                } else {
                    if let Some(canon) = canonicalize_repository_path(Path::new(arg)) {
                        paths.push(canon);
                    }
                }
            }
            _ => {}
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
        convert_legacy_command, normalize_action, requires_contextual_judgment,
        successful_duplicate_action,
    };
    use crate::schema::AgentAction;
    use purrcode_pawgate::Policy;
    use purrcode_runtime_core::{
        ActionId, CommandAction, JudgmentDecision, ProposedAction, RepositoryReadAction,
        SessionEvent, SessionId, SessionState,
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
        let action =
            normalize_action(AgentAction::Read(RepositoryReadAction::GitStatus), worktree).unwrap();
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
    fn typed_deterministic_reads_do_not_gain_semantic_approval_prompts() {
        let worktree = std::env::temp_dir().join("purrcode-policy-read-session");
        let action = normalize_action(
            AgentAction::Read(RepositoryReadAction::List {
                paths: vec![PathBuf::from(".")],
                max_entries: 32,
            }),
            &worktree,
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
    fn exact_successful_action_is_reused_instead_of_replayed() {
        let worktree = std::env::temp_dir().join("purrcode-duplicate-read-session");
        let action = normalize_action(
            AgentAction::Read(RepositoryReadAction::List {
                paths: vec![PathBuf::from(".")],
                max_entries: 32,
            }),
            &worktree,
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
        );
        assert!(result.is_err());
    }
}
