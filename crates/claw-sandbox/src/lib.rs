//! Shell-free command execution guarded by durable, single-use authorization.

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use globset::{Glob, GlobSetBuilder};
use purrcode_ninelives::{SessionStore, StoreError};
use purrcode_runtime_core::{
    ActionId, DeleteFileAction, ProposedAction, WriteFileAction, MAX_GREP_MAX_BYTES,
    MAX_GREP_MAX_RESULTS, MAX_LIST_MAX_ENTRIES, MAX_READ_FILE_BYTES,
};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxLevel {
    WorktreeWriteNoShell,
    RestrictedProcessNoNetwork,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxCapability {
    pub level: SandboxLevel,
    pub backend: String,
    pub network_isolation: bool,
    pub process_group_termination: bool,
}

pub fn sandbox_capability() -> SandboxCapability {
    #[cfg(target_os = "macos")]
    if Path::new("/usr/bin/sandbox-exec").is_file() {
        return SandboxCapability {
            level: SandboxLevel::RestrictedProcessNoNetwork,
            backend: "macos-sandbox-exec".into(),
            network_isolation: true,
            process_group_termination: true,
        };
    }
    #[cfg(target_os = "linux")]
    if executable_on_path("bwrap") {
        return SandboxCapability {
            level: SandboxLevel::RestrictedProcessNoNetwork,
            backend: "linux-bubblewrap".into(),
            network_isolation: true,
            process_group_termination: true,
        };
    }
    SandboxCapability {
        level: SandboxLevel::WorktreeWriteNoShell,
        backend: "process-filter-fallback".into(),
        network_isolation: false,
        process_group_termination: cfg!(unix),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub affected_paths: Vec<PathBuf>,
    pub sandbox_level: SandboxLevel,
    pub sandbox_backend: String,
}

pub struct ToolRuntime;

impl ToolRuntime {
    pub async fn execute(
        store: &mut SessionStore,
        action_id: ActionId,
        action: &ProposedAction,
        constraints: &purrcode_runtime_core::ActionConstraints,
    ) -> Result<ExecutionResult, ExecutionError> {
        let digest = action.digest(constraints)?;
        let authorization = store.consume_authorization(action_id, &digest)?;
        if authorization.constraints != *constraints {
            return Err(ExecutionError::ConstraintMismatch);
        }
        match action {
            ProposedAction::RepositoryRead(read) => execute_typed_read(read, constraints).await,
            ProposedAction::Command(command) => execute_command(command, constraints).await,
            ProposedAction::WriteFile(write) => {
                execute_write(write, constraints)?;
                Ok(ExecutionResult {
                    exit_code: Some(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    truncated: false,
                    affected_paths: vec![write.path.clone()],
                    sandbox_level: SandboxLevel::WorktreeWriteNoShell,
                    sandbox_backend: "cap-std".into(),
                })
            }
            ProposedAction::DeleteFile(delete) => {
                execute_delete(delete, constraints)?;
                Ok(ExecutionResult {
                    exit_code: Some(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    truncated: false,
                    affected_paths: vec![delete.path.clone()],
                    sandbox_level: SandboxLevel::WorktreeWriteNoShell,
                    sandbox_backend: "cap-std".into(),
                })
            }
            ProposedAction::ExternalTool(_) => Err(ExecutionError::UnsupportedConstraint(
                "external tool actions must execute through the isolated MCP host".into(),
            )),
        }
    }
}

async fn execute_typed_read(
    read: &purrcode_runtime_core::RepositoryReadAction,
    constraints: &purrcode_runtime_core::ActionConstraints,
) -> Result<ExecutionResult, ExecutionError> {
    let working_directory = &constraints.working_directory;
    match read {
        purrcode_runtime_core::RepositoryReadAction::ReadFile { path, max_bytes } => {
            let max_bytes = (*max_bytes).min(MAX_READ_FILE_BYTES);
            if max_bytes == 0 {
                return Err(ExecutionError::UnsupportedConstraint(
                    "read_file max_bytes must be greater than zero".into(),
                ));
            }
            let directory = Dir::open_ambient_dir(working_directory, ambient_authority())?;
            let file = directory.open(path)?;
            let mut bytes = Vec::with_capacity(max_bytes.min(8192));
            let mut truncated = false;
            let mut taken = file.take(max_bytes as u64 + 1);
            let mut chunk = [0_u8; 8192];
            loop {
                let n = taken.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                let remaining = max_bytes.saturating_sub(bytes.len());
                bytes.extend_from_slice(&chunk[..n.min(remaining)]);
                if n > remaining {
                    truncated = true;
                    break;
                }
            }
            let stdout = bytes;
            Ok(ExecutionResult {
                exit_code: Some(0),
                stdout,
                stderr: Vec::new(),
                truncated,
                affected_paths: Vec::new(),
                sandbox_level: SandboxLevel::WorktreeWriteNoShell,
                sandbox_backend: "cap-std".into(),
            })
        }
        purrcode_runtime_core::RepositoryReadAction::List { paths, max_entries } => {
            let max_entries = (*max_entries).min(MAX_LIST_MAX_ENTRIES);
            let directory = Dir::open_ambient_dir(working_directory, ambient_authority())?;
            let mut entries = 0u32;
            let mut output = Vec::new();
            for path in paths {
                let target = if path.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    path.as_path()
                };
                if entries >= max_entries {
                    break;
                }
                let metadata = directory.metadata(target)?;
                if !metadata.is_dir() {
                    let file_type = if metadata.is_file() { "-" } else { "?" };
                    let line = format!(
                        "{file_type} {:>8} {}",
                        metadata.len(),
                        target.to_string_lossy()
                    );
                    output.extend_from_slice(line.as_bytes());
                    output.push(b'\n');
                    entries += 1;
                    continue;
                }
                let dir = directory.open_dir(target)?;
                for entry in dir
                    .entries()
                    .map_err(|e| ExecutionError::Io(std::io::Error::new(e.kind(), e)))?
                {
                    if entries >= max_entries {
                        break;
                    }
                    let entry =
                        entry.map_err(|e| ExecutionError::Io(std::io::Error::new(e.kind(), e)))?;
                    let name = entry.file_name();
                    let metadata = entry
                        .metadata()
                        .map_err(|e| ExecutionError::Io(std::io::Error::new(e.kind(), e)))?;
                    let file_type = if metadata.is_dir() { "d" } else { "-" };
                    let size = metadata.len();
                    let line = format!("{file_type} {size:>8} {}", name.to_string_lossy());
                    output.extend_from_slice(line.as_bytes());
                    output.push(b'\n');
                    entries += 1;
                }
            }
            Ok(ExecutionResult {
                exit_code: Some(0),
                stdout: output,
                stderr: Vec::new(),
                truncated: entries > max_entries,
                affected_paths: Vec::new(),
                sandbox_level: SandboxLevel::WorktreeWriteNoShell,
                sandbox_backend: "cap-std".into(),
            })
        }
        purrcode_runtime_core::RepositoryReadAction::Find {
            paths,
            max_depth,
            max_entries,
        } => {
            let max_entries = (*max_entries).min(MAX_LIST_MAX_ENTRIES);
            let directory = Dir::open_ambient_dir(working_directory, ambient_authority())?;
            let mut entries = 0u32;
            let mut output = Vec::new();
            for path in paths {
                let target = if path.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    path.as_path()
                };
                if entries >= max_entries {
                    break;
                }
                let mut state = WalkState {
                    directory: &directory,
                    max_depth: *max_depth,
                    max_entries,
                    entries: &mut entries,
                    output: &mut output,
                };
                let base_path = working_directory.join(target);
                collect_files(&base_path, target, 0, &mut state)?;
            }
            Ok(ExecutionResult {
                exit_code: Some(0),
                stdout: output,
                stderr: Vec::new(),
                truncated: false,
                affected_paths: Vec::new(),
                sandbox_level: SandboxLevel::WorktreeWriteNoShell,
                sandbox_backend: "cap-std".into(),
            })
        }
        purrcode_runtime_core::RepositoryReadAction::RepositoryGrep {
            pattern,
            paths,
            case_insensitive,
            max_results,
            max_bytes,
        } => {
            let max_results = (*max_results).min(MAX_GREP_MAX_RESULTS);
            let max_bytes = (*max_bytes).min(MAX_GREP_MAX_BYTES);
            let directory = Dir::open_ambient_dir(working_directory, ambient_authority())?;
            let mut results = 0u32;
            let mut output_bytes = 0usize;
            let mut output = Vec::new();
            let search_paths: Vec<PathBuf> = if paths.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                paths.clone()
            };
            for path in &search_paths {
                if results >= max_results || output_bytes >= max_bytes {
                    break;
                }
                let target = if path.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    path.as_path()
                };
                let mut grep_state = GrepState {
                    directory: &directory,
                    working_directory,
                    pattern,
                    case_insensitive: *case_insensitive,
                    max_results,
                    max_bytes,
                    results: &mut results,
                    output_bytes: &mut output_bytes,
                    output: &mut output,
                };
                grep_file_recursive(target, &mut grep_state)?;
            }
            Ok(ExecutionResult {
                exit_code: Some(0),
                stdout: output,
                stderr: Vec::new(),
                truncated: results >= max_results || output_bytes >= max_bytes,
                affected_paths: Vec::new(),
                sandbox_level: SandboxLevel::WorktreeWriteNoShell,
                sandbox_backend: "cap-std".into(),
            })
        }
        // All Git variants still map to git subprocess execution.
        _ => {
            let command = read.to_command(constraints.working_directory.clone());
            execute_command(&command, constraints).await
        }
    }
}

struct WalkState<'a> {
    directory: &'a Dir,
    max_depth: u8,
    max_entries: u32,
    entries: &'a mut u32,
    output: &'a mut Vec<u8>,
}

fn collect_files(
    base_path: &Path,
    rel_path: &Path,
    current_depth: u8,
    state: &mut WalkState,
) -> Result<(), ExecutionError> {
    if current_depth > state.max_depth || *state.entries >= state.max_entries {
        return Ok(());
    }
    if current_depth == 0 {
        let line = format!("{}\n", rel_path.to_string_lossy());
        state.output.extend_from_slice(line.as_bytes());
        *state.entries += 1;
    }
    let target = if rel_path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        rel_path
    };
    let dir = match state.directory.open_dir(target) {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };
    for entry in dir
        .entries()
        .map_err(|e| ExecutionError::Io(std::io::Error::new(e.kind(), e)))?
    {
        if *state.entries >= state.max_entries {
            break;
        }
        let entry = entry.map_err(|e| ExecutionError::Io(std::io::Error::new(e.kind(), e)))?;
        let name = entry.file_name();
        let child_rel = rel_path.join(&name);
        let line = format!("{}\n", child_rel.to_string_lossy());
        state.output.extend_from_slice(line.as_bytes());
        *state.entries += 1;
        if current_depth < state.max_depth {
            let child_abs = base_path.join(&name);
            if child_abs.is_dir() {
                collect_files(&child_abs, &child_rel, current_depth + 1, state)?;
            }
        }
    }
    Ok(())
}

struct GrepState<'a> {
    directory: &'a Dir,
    working_directory: &'a Path,
    pattern: &'a str,
    case_insensitive: bool,
    max_results: u32,
    max_bytes: usize,
    results: &'a mut u32,
    output_bytes: &'a mut usize,
    output: &'a mut Vec<u8>,
}

fn grep_file_recursive(rel_path: &Path, state: &mut GrepState) -> Result<(), ExecutionError> {
    if *state.results >= state.max_results || *state.output_bytes >= state.max_bytes {
        return Ok(());
    }
    let target = if rel_path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        rel_path
    };
    // Try to open as a directory first
    if let Ok(subdir) = state.directory.open_dir(target) {
        for entry in subdir
            .entries()
            .map_err(|e| ExecutionError::Io(std::io::Error::new(e.kind(), e)))?
        {
            if *state.results >= state.max_results {
                break;
            }
            let entry = entry.map_err(|e| ExecutionError::Io(std::io::Error::new(e.kind(), e)))?;
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child_rel = rel_path.join(&name);
            grep_file_recursive(&child_rel, state)?;
        }
        return Ok(());
    }
    // It's a file — search its contents
    if *state.results >= state.max_results || *state.output_bytes >= state.max_bytes {
        return Ok(());
    }
    let abs_path = state.working_directory.join(target);
    if abs_path.is_dir() {
        return Ok(());
    }
    let file = match std::fs::File::open(&abs_path) {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    let reader = std::io::BufReader::new(file);
    use std::io::BufRead;
    let mut current = 0u64;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if *state.results >= state.max_results || *state.output_bytes >= state.max_bytes {
            break;
        }
        current += 1;
        let matched = if state.case_insensitive {
            line.to_ascii_lowercase()
                .contains(&state.pattern.to_ascii_lowercase())
        } else {
            line.contains(state.pattern)
        };
        if matched {
            let line_bytes = format!("{}:{}:{}\n", target.to_string_lossy(), current, line);
            *state.output_bytes += line_bytes.len();
            if *state.output_bytes <= state.max_bytes {
                state.output.extend_from_slice(line_bytes.as_bytes());
            }
            *state.results += 1;
        }
    }
    Ok(())
}

async fn execute_command(
    command: &purrcode_runtime_core::CommandAction,
    constraints: &purrcode_runtime_core::ActionConstraints,
) -> Result<ExecutionResult, ExecutionError> {
    if command.working_directory != constraints.working_directory {
        return Err(ExecutionError::ConstraintMismatch);
    }
    if constraints.network {
        return Err(ExecutionError::UnsupportedConstraint(
            "network-enabled execution is not implemented by the process backend".into(),
        ));
    }
    let (mut process, sandbox_level, sandbox_backend) = sandboxed_command(command, constraints)?;
    process
        .current_dir(&command.working_directory)
        .env_clear()
        .envs(safe_environment())
        .envs(&command.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    process.process_group(0);
    let mut child = process.spawn()?;
    let mut process_group_guard = ProcessGroupGuard::new(child.id());
    let mut stdout = child.stdout.take().ok_or(ExecutionError::MissingPipe)?;
    let mut stderr = child.stderr.take().ok_or(ExecutionError::MissingPipe)?;
    let limit = constraints.maximum_output_bytes;
    let stdout_task = tokio::spawn(async move { read_bounded(&mut stdout, limit).await });
    let stderr_task = tokio::spawn(async move { read_bounded(&mut stderr, limit).await });
    let status = match timeout(
        Duration::from_secs(constraints.timeout_seconds),
        child.wait(),
    )
    .await
    {
        Ok(status) => status?,
        Err(_) => {
            terminate_process_tree(&mut child).await?;
            process_group_guard.disarm();
            stdout_task.abort();
            stderr_task.abort();
            return Err(ExecutionError::Timeout);
        }
    };
    process_group_guard.disarm();
    let (stdout_bytes, stdout_truncated) = stdout_task.await??;
    let (stderr_bytes, stderr_truncated) = stderr_task.await??;
    Ok(ExecutionResult {
        exit_code: status.code(),
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        truncated: stdout_truncated || stderr_truncated,
        affected_paths: Vec::new(),
        sandbox_level,
        sandbox_backend,
    })
}

struct ProcessGroupGuard {
    #[cfg(unix)]
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        #[cfg(not(unix))]
        let _ = pid;
        Self {
            #[cfg(unix)]
            pid,
        }
    }

    fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pid = None;
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

fn sandboxed_command(
    action: &purrcode_runtime_core::CommandAction,
    constraints: &purrcode_runtime_core::ActionConstraints,
) -> Result<(Command, SandboxLevel, String), ExecutionError> {
    #[cfg(target_os = "macos")]
    {
        if Path::new("/usr/bin/sandbox-exec").is_file() && !constraints.network {
            let canonical_worktree = action.working_directory.canonicalize()?;
            let writable = canonical_worktree
                .to_str()
                .ok_or_else(|| ExecutionError::Sandbox("worktree path is not UTF-8".into()))?
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            let worktree_write = if constraints.allowed_write_globs.is_empty()
                && constraints.maximum_changed_files == 0
            {
                String::new()
            } else {
                format!("(allow file-write* (subpath \"{writable}\"))")
            };
            let profile = format!(
                "(version 1) (deny default) (allow process*) (allow sysctl-read) \
                 (allow file-read*) {worktree_write} \
                 (allow file-write* (subpath \"/private/tmp\")) \
                 (allow file-write* (literal \"/dev/null\")) (deny network*)"
            );
            let mut command = Command::new("/usr/bin/sandbox-exec");
            command
                .arg("-p")
                .arg(profile)
                .arg(&action.program)
                .args(&action.arguments);
            return Ok((
                command,
                SandboxLevel::RestrictedProcessNoNetwork,
                "macos-sandbox-exec".into(),
            ));
        }
    }
    #[cfg(target_os = "linux")]
    {
        if executable_on_path("bwrap") && !constraints.network {
            let mut command = Command::new("bwrap");
            command.args(["--die-with-parent", "--unshare-net", "--ro-bind", "/", "/"]);
            command
                .arg(
                    if constraints.allowed_write_globs.is_empty()
                        && constraints.maximum_changed_files == 0
                    {
                        "--ro-bind"
                    } else {
                        "--bind"
                    },
                )
                .arg(&action.working_directory)
                .arg(&action.working_directory)
                .arg("--chdir")
                .arg(&action.working_directory)
                .arg(&action.program)
                .args(&action.arguments);
            return Ok((
                command,
                SandboxLevel::RestrictedProcessNoNetwork,
                "linux-bubblewrap".into(),
            ));
        }
    }
    if constraints.network {
        return Err(ExecutionError::UnsupportedConstraint(
            "network-enabled execution requires an approved network sandbox backend".into(),
        ));
    }
    let mut command = Command::new(&action.program);
    command.args(&action.arguments);
    Ok((
        command,
        SandboxLevel::WorktreeWriteNoShell,
        "process-filter-fallback (network isolation unavailable)".into(),
    ))
}

#[cfg(target_os = "linux")]
fn executable_on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(program).is_file())
    })
}

#[cfg(unix)]
async fn terminate_process_tree(child: &mut tokio::process::Child) -> Result<(), std::io::Error> {
    if let Some(pid) = child.id() {
        // The child is created as a process-group leader above. Negative PID targets that group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        let leader_status = timeout(Duration::from_secs(1), child.wait()).await;
        // The group leader can exit before one of its descendants. Always follow the grace period
        // with SIGKILL for the original process group; ESRCH simply means the group is already gone.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        if let Ok(status) = leader_status {
            return status.map(|_| ());
        }
    }
    child.wait().await.map(|_| ())
}

#[cfg(not(unix))]
async fn terminate_process_tree(child: &mut tokio::process::Child) -> Result<(), std::io::Error> {
    child.kill().await?;
    child.wait().await.map(|_| ())
}

fn execute_write(
    action: &WriteFileAction,
    constraints: &purrcode_runtime_core::ActionConstraints,
) -> Result<(), ExecutionError> {
    validate_mutation_path(&action.path, constraints)?;
    let directory = Dir::open_ambient_dir(&constraints.working_directory, ambient_authority())?;
    if let Some(expected) = &action.expected_digest {
        let existing = read_cap_file(&directory, &action.path)?;
        if blake3::hash(&existing).to_hex().as_str() != expected {
            return Err(ExecutionError::OptimisticConcurrency(action.path.clone()));
        }
    }
    if let Some(parent) = action
        .path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        directory.create_dir_all(parent)?;
    }
    let temporary = temporary_sibling(&action.path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = directory.open_with(&temporary, &options)?;
    file.write_all(action.content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = directory.rename(&temporary, &directory, &action.path) {
        let _ = directory.remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn execute_delete(
    action: &DeleteFileAction,
    constraints: &purrcode_runtime_core::ActionConstraints,
) -> Result<(), ExecutionError> {
    validate_mutation_path(&action.path, constraints)?;
    let directory = Dir::open_ambient_dir(&constraints.working_directory, ambient_authority())?;
    let existing = read_cap_file(&directory, &action.path)?;
    if blake3::hash(&existing).to_hex().as_str() != action.expected_digest {
        return Err(ExecutionError::OptimisticConcurrency(action.path.clone()));
    }
    directory.remove_file(&action.path)?;
    Ok(())
}

fn read_cap_file(directory: &Dir, path: &Path) -> Result<Vec<u8>, ExecutionError> {
    let file = directory.open(path)?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(ExecutionError::FileSizeLimit(path.to_path_buf()));
    }
    Ok(bytes)
}

fn validate_mutation_path(
    path: &Path,
    constraints: &purrcode_runtime_core::ActionConstraints,
) -> Result<(), ExecutionError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(ExecutionError::UnsafePath(path.to_path_buf()));
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in &constraints.allowed_write_globs {
        builder.add(
            Glob::new(pattern)
                .map_err(|error| ExecutionError::InvalidWriteGlob(error.to_string()))?,
        );
    }
    let globs = builder
        .build()
        .map_err(|error| ExecutionError::InvalidWriteGlob(error.to_string()))?;
    if constraints.maximum_changed_files < 1 || !globs.is_match(path) {
        return Err(ExecutionError::WriteOutsideConstraints(path.to_path_buf()));
    }
    Ok(())
}

fn temporary_sibling(path: &Path) -> Result<PathBuf, ExecutionError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ExecutionError::UnsafePath(path.to_path_buf()))?;
    let temporary_name = format!(".{file_name}.purrcode-{}.tmp", uuid::Uuid::new_v4());
    Ok(path.with_file_name(temporary_name))
}

fn safe_environment() -> Vec<(String, String)> {
    ["PATH", "TMPDIR", "LANG", "LC_ALL", "TERM"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect()
}

async fn read_bounded<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<(Vec<u8>, bool), std::io::Error> {
    let mut output = Vec::with_capacity(maximum.min(8192));
    let mut chunk = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = maximum.saturating_sub(output.len());
        output.extend_from_slice(&chunk[..read.min(remaining)]);
        if read > remaining {
            truncated = true;
        }
    }
    Ok((output, truncated))
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("authorization store rejected execution: {0}")]
    Store(#[from] StoreError),
    #[error("action digest failed: {0}")]
    Domain(#[from] purrcode_runtime_core::DomainError),
    #[error("authorized constraints do not match execution request")]
    ConstraintMismatch,
    #[error("process backend cannot enforce constraint: {0}")]
    UnsupportedConstraint(String),
    #[error("sandbox setup failed: {0}")]
    Sandbox(String),
    #[error("process I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("process exceeded its authorized timeout")]
    Timeout,
    #[error("process output pipe was unavailable")]
    MissingPipe,
    #[error("output collector failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("unsafe repository-relative path: {0}")]
    UnsafePath(PathBuf),
    #[error("write path is outside authorized constraints: {0}")]
    WriteOutsideConstraints(PathBuf),
    #[error("authorized write glob is invalid: {0}")]
    InvalidWriteGlob(String),
    #[error("file changed since it was proposed: {0}")]
    OptimisticConcurrency(PathBuf),
    #[error("file exceeds the 16 MiB mutation limit: {0}")]
    FileSizeLimit(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use purrcode_runtime_core::{
        ActionConstraints, ApprovalAuthority, Authorization, RepositoryReadAction, SessionId,
    };

    #[tokio::test]
    async fn authorized_write_is_atomic_and_single_use() {
        let temporary = tempfile::tempdir().unwrap();
        let action = ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("src/new.txt"),
            content: "safe".into(),
            expected_digest: None,
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 1024,
            allowed_write_globs: vec!["src/new.txt".into()],
            maximum_changed_files: 1,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::Human,
            })
            .unwrap();
        ToolRuntime::execute(&mut store, action_id, &action, &constraints)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("src/new.txt")).unwrap(),
            "safe"
        );
        assert!(
            ToolRuntime::execute(&mut store, action_id, &action, &constraints)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn changed_file_fails_optimistic_concurrency() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("tracked.txt"), "newer").unwrap();
        let action = ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("tracked.txt"),
            content: "replacement".into(),
            expected_digest: Some(blake3::hash(b"older").to_hex().to_string()),
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 1024,
            allowed_write_globs: vec!["tracked.txt".into()],
            maximum_changed_files: 1,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::Human,
            })
            .unwrap();
        let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints).await;
        assert!(matches!(
            result,
            Err(ExecutionError::OptimisticConcurrency(_))
        ));
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("tracked.txt")).unwrap(),
            "newer"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_terminates_the_entire_process_group() {
        use purrcode_runtime_core::CommandAction;
        let temporary = tempfile::tempdir().unwrap();
        let pid_file = temporary.path().join("child.pid");
        let action = ProposedAction::Command(CommandAction {
            program: PathBuf::from("sh"),
            arguments: vec![
                "-c".into(),
                format!("sleep 30 & echo $! > {}; wait", pid_file.display()),
            ],
            working_directory: temporary.path().to_path_buf(),
            environment: Default::default(),
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 1,
            maximum_output_bytes: 1024,
            allowed_write_globs: vec!["child.pid".into()],
            maximum_changed_files: 1,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            })
            .unwrap();
        assert!(matches!(
            ToolRuntime::execute(&mut store, action_id, &action, &constraints).await,
            Err(ExecutionError::Timeout)
        ));
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while process_is_running(pid) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let exists = process_is_running(pid);
        assert!(
            !exists,
            "background child {pid} survived process-group timeout"
        );
    }

    #[cfg(target_os = "linux")]
    fn process_is_running(pid: i32) -> bool {
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(_) => return false,
        };
        // A zombie has terminated and cannot execute; it only awaits collection by its adopter.
        stat.rsplit_once(") ")
            .and_then(|(_, fields)| fields.chars().next())
            .is_some_and(|state| state != 'Z')
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn process_is_running(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[tokio::test]
    async fn native_list_executes_without_external_program() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("a.txt"), "alpha").unwrap();
        std::fs::write(temporary.path().join("b.txt"), "beta").unwrap();
        std::fs::create_dir(temporary.path().join("sub")).unwrap();
        std::fs::write(temporary.path().join("sub/c.txt"), "gamma").unwrap();

        let action = ProposedAction::RepositoryRead(RepositoryReadAction::List {
            paths: vec![PathBuf::from(".")],
            max_entries: 32,
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 4096,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            })
            .unwrap();
        let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints)
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0));
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("a.txt"), "list should contain a.txt");
        assert!(stdout.contains("b.txt"), "list should contain b.txt");
    }

    #[tokio::test]
    async fn native_list_accepts_mixed_file_and_directory_paths() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("Cargo.toml"), "[workspace]").unwrap();
        std::fs::create_dir(temporary.path().join("src")).unwrap();
        std::fs::write(temporary.path().join("src/lib.rs"), "pub fn purr() {}").unwrap();

        let action = ProposedAction::RepositoryRead(RepositoryReadAction::List {
            paths: vec![PathBuf::from("Cargo.toml"), PathBuf::from("src")],
            max_entries: 32,
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 4096,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            })
            .unwrap();

        let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints)
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("Cargo.toml"));
        assert!(stdout.contains("lib.rs"));
    }

    #[tokio::test]
    async fn native_find_executes_without_external_find() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("a.txt"), "alpha").unwrap();
        std::fs::create_dir(temporary.path().join("sub")).unwrap();
        std::fs::write(temporary.path().join("sub/b.txt"), "beta").unwrap();

        let action = ProposedAction::RepositoryRead(RepositoryReadAction::Find {
            paths: vec![PathBuf::from(".")],
            max_depth: 3,
            max_entries: 32,
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 4096,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            })
            .unwrap();
        let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints)
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout).replace('\\', "/");
        assert!(stdout.contains("a.txt"), "find should find a.txt");
        assert!(
            stdout.contains("sub/b.txt"),
            "find should find sub/b.txt recursively"
        );
    }

    #[tokio::test]
    async fn native_grep_matches_file_content_without_external_rg() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("findme.txt"), "KEYWORD content").unwrap();
        std::fs::write(temporary.path().join("other.txt"), "no match here").unwrap();

        let action = ProposedAction::RepositoryRead(RepositoryReadAction::RepositoryGrep {
            pattern: "KEYWORD".into(),
            paths: vec![PathBuf::from(".")],
            case_insensitive: false,
            max_results: 64,
            max_bytes: 4096,
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 4096,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            })
            .unwrap();
        let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints)
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            stdout.contains("findme.txt"),
            "grep should find match in findme.txt"
        );
        assert!(
            !stdout.contains("other.txt"),
            "grep should not match other.txt"
        );
    }

    #[tokio::test]
    async fn native_read_file_returns_bounded_content() {
        let temporary = tempfile::tempdir().unwrap();
        let content = "Hello, PurrCode!";
        std::fs::write(temporary.path().join("greeting.txt"), content).unwrap();

        let action = ProposedAction::RepositoryRead(RepositoryReadAction::ReadFile {
            path: PathBuf::from("greeting.txt"),
            max_bytes: 1024,
        });
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 4096,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            })
            .unwrap();
        let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints)
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert_eq!(stdout, content);
    }

    #[tokio::test]
    async fn authorized_repository_read_executes_through_sandbox() {
        let temporary = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .current_dir(temporary.path())
            .output()
            .unwrap();
        std::fs::write(temporary.path().join("tracked.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(temporary.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Tester",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ])
            .current_dir(temporary.path())
            .output()
            .unwrap();

        let action = ProposedAction::RepositoryRead(RepositoryReadAction::GitStatus);
        let constraints = ActionConstraints {
            working_directory: temporary.path().to_path_buf(),
            network: false,
            timeout_seconds: 10,
            maximum_output_bytes: 4096,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id: SessionId::new(),
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            })
            .unwrap();
        let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints)
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.truncated);
        let capability = sandbox_capability();
        assert_eq!(result.sandbox_level, capability.level);
        assert!(result.sandbox_backend.starts_with(&capability.backend));
        if !capability.network_isolation {
            assert!(result
                .sandbox_backend
                .contains("network isolation unavailable"));
        }
        assert!(
            ToolRuntime::execute(&mut store, action_id, &action, &constraints)
                .await
                .is_err(),
            "authorization record must be consumed exactly once"
        );
    }
}
