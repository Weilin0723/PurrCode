//! Git repository inspection and isolated session worktrees.

use globset::{Glob, GlobSetBuilder};
use purrcode_runtime_core::{ActionConstraints, SessionId};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_GIT_OUTPUT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositorySnapshot {
    pub root: PathBuf,
    pub head: String,
    pub dirty: bool,
    pub status_porcelain: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionWorktree {
    pub session_id: SessionId,
    pub source_repository: PathBuf,
    pub path: PathBuf,
    pub base_head: String,
    pub source_was_dirty: bool,
    pub initialized_submodules: Vec<PathBuf>,
    pub unavailable_submodules: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeEffects {
    pub status_porcelain: String,
    pub changed_files: Vec<PathBuf>,
    pub binary_patch: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectSnapshot {
    pub files: BTreeMap<PathBuf, Option<String>>,
    pub patch_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewHunk {
    pub index: usize,
    pub path: PathBuf,
    pub patch: Vec<u8>,
    pub preview: String,
}

pub struct RepositoryEngine;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplicationStrategy {
    ExportPatch(PathBuf),
    ApplyToCurrentTree,
    CreateBranch { name: String },
    Commit { message: String },
    LeaveForReview,
    Discard,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationResult {
    pub strategy: ApplicationStrategy,
    pub detail: String,
}

impl RepositoryEngine {
    pub async fn inspect(repository: &Path) -> Result<RepositorySnapshot, RepositoryError> {
        let root = canonical_repository_root(repository).await?;
        let head = git_text(&root, &["rev-parse", "HEAD"]).await?;
        let status_porcelain = git_text(&root, &["status", "--porcelain=v1", "-z"]).await?;
        Ok(RepositorySnapshot {
            root,
            head: head.trim().into(),
            dirty: !status_porcelain.is_empty(),
            status_porcelain,
        })
    }

    pub async fn create_worktree(
        repository: &Path,
        session_id: SessionId,
    ) -> Result<SessionWorktree, RepositoryError> {
        let snapshot = Self::inspect(repository).await?;
        ensure_purrcode_excluded(&snapshot.root).await?;
        let path = snapshot
            .root
            .join(".purrcode")
            .join("worktrees")
            .join(session_id.0.to_string());
        if path.exists() {
            return Err(RepositoryError::WorktreeAlreadyExists(path));
        }
        let parent = path
            .parent()
            .ok_or_else(|| RepositoryError::UnsafeWorktreePath(path.clone()))?;
        std::fs::create_dir_all(parent)?;
        let path_text = git_compatible_path(&path)?;
        git_text(
            &snapshot.root,
            &["worktree", "add", "--detach", &path_text, &snapshot.head],
        )
        .await?;
        let canonical_path = path.canonicalize()?;
        ensure_session_path(&snapshot.root, session_id, &canonical_path)?;
        let (initialized_submodules, unavailable_submodules) =
            initialize_local_submodules(&snapshot.root, &canonical_path).await?;
        Ok(SessionWorktree {
            session_id,
            source_repository: snapshot.root,
            path: canonical_path,
            base_head: snapshot.head,
            source_was_dirty: snapshot.dirty,
            initialized_submodules,
            unavailable_submodules,
        })
    }

    pub async fn effects(worktree: &SessionWorktree) -> Result<WorktreeEffects, RepositoryError> {
        ensure_session_path(
            &worktree.source_repository,
            worktree.session_id,
            &worktree.path,
        )?;
        let status_porcelain =
            git_text(&worktree.path, &["status", "--porcelain=v1", "-z"]).await?;
        let tracked = git_bytes(
            &worktree.path,
            &["diff", "--name-only", "-z", "HEAD", "--", "."],
        )
        .await?;
        let untracked = git_bytes(
            &worktree.path,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ".",
            ],
        )
        .await?;
        let mut changed_files = nul_paths(&tracked);
        changed_files.extend(nul_paths(&untracked));
        let changed_files = changed_files.into_iter().collect();
        let mut binary_patch =
            git_bytes(&worktree.path, &["diff", "--binary", "HEAD", "--", "."]).await?;
        for path in nul_paths(&untracked) {
            let path_text = path
                .to_str()
                .ok_or_else(|| RepositoryError::NonUtf8Path(path.clone()))?;
            let patch = git_bytes_allow_codes(
                &worktree.path,
                &[
                    "diff",
                    "--binary",
                    "--no-index",
                    "--",
                    "/dev/null",
                    path_text,
                ],
                &[0, 1],
            )
            .await?;
            binary_patch.extend_from_slice(&patch);
        }
        Ok(WorktreeEffects {
            status_porcelain,
            changed_files,
            binary_patch,
        })
    }

    pub async fn review_hunks(
        worktree: &SessionWorktree,
    ) -> Result<(String, Vec<ReviewHunk>), RepositoryError> {
        let effects = Self::effects(worktree).await?;
        let digest = blake3::hash(&effects.binary_patch).to_hex().to_string();
        Ok((digest, parse_review_hunks(&effects.binary_patch)?))
    }

    pub async fn apply_review_hunk(
        worktree: &SessionWorktree,
        index: usize,
        expected_patch_digest: &str,
    ) -> Result<ReviewHunk, RepositoryError> {
        ensure_session_path(
            &worktree.source_repository,
            worktree.session_id,
            &worktree.path,
        )?;
        let (digest, hunks) = Self::review_hunks(worktree).await?;
        if digest != expected_patch_digest {
            return Err(RepositoryError::ReviewPatchChanged);
        }
        let hunk = hunks
            .into_iter()
            .find(|hunk| hunk.index == index)
            .ok_or(RepositoryError::ReviewHunkNotFound(index))?;
        git_with_input(
            &worktree.source_repository,
            &["apply", "--check", "--whitespace=nowarn", "-"],
            &hunk.patch,
            &[0],
        )
        .await?;
        git_with_input(
            &worktree.source_repository,
            &["apply", "--whitespace=nowarn", "-"],
            &hunk.patch,
            &[0],
        )
        .await?;
        Ok(hunk)
    }

    pub async fn reject_review_hunk(
        worktree: &SessionWorktree,
        index: usize,
        expected_patch_digest: &str,
    ) -> Result<ReviewHunk, RepositoryError> {
        ensure_session_path(
            &worktree.source_repository,
            worktree.session_id,
            &worktree.path,
        )?;
        let (digest, hunks) = Self::review_hunks(worktree).await?;
        if digest != expected_patch_digest {
            return Err(RepositoryError::ReviewPatchChanged);
        }
        let hunk = hunks
            .into_iter()
            .find(|hunk| hunk.index == index)
            .ok_or(RepositoryError::ReviewHunkNotFound(index))?;
        git_with_input(
            &worktree.path,
            &["apply", "--check", "--reverse", "--whitespace=nowarn", "-"],
            &hunk.patch,
            &[0],
        )
        .await?;
        git_with_input(
            &worktree.path,
            &["apply", "--reverse", "--whitespace=nowarn", "-"],
            &hunk.patch,
            &[0],
        )
        .await?;
        Ok(hunk)
    }

    pub async fn snapshot(worktree: &SessionWorktree) -> Result<EffectSnapshot, RepositoryError> {
        let effects = Self::effects(worktree).await?;
        let mut files = BTreeMap::new();
        for path in &effects.changed_files {
            let absolute = worktree.path.join(path);
            let digest = match std::fs::symlink_metadata(&absolute) {
                Ok(metadata) if metadata.file_type().is_symlink() => Some(
                    blake3::hash(
                        std::fs::read_link(&absolute)?
                            .as_os_str()
                            .as_encoded_bytes(),
                    )
                    .to_hex()
                    .to_string(),
                ),
                Ok(metadata) if metadata.is_file() => Some(
                    blake3::hash(&std::fs::read(&absolute)?)
                        .to_hex()
                        .to_string(),
                ),
                Ok(_) => Some("non-file".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            files.insert(path.clone(), digest);
        }
        Ok(EffectSnapshot {
            files,
            patch_digest: blake3::hash(&effects.binary_patch).to_hex().to_string(),
        })
    }

    pub fn validate_effect_delta(
        before: &EffectSnapshot,
        after: &EffectSnapshot,
        constraints: &ActionConstraints,
    ) -> Result<Vec<PathBuf>, RepositoryError> {
        let paths: BTreeSet<_> = before
            .files
            .keys()
            .chain(after.files.keys())
            .cloned()
            .collect();
        let changed: Vec<_> = paths
            .into_iter()
            .filter(|path| before.files.get(path) != after.files.get(path))
            .collect();
        if before.patch_digest != after.patch_digest && changed.is_empty() {
            return Err(RepositoryError::UnattributedFilesystemEffect);
        }
        if changed.len() > constraints.maximum_changed_files {
            return Err(RepositoryError::TooManyFilesystemEffects {
                actual: changed.len(),
                allowed: constraints.maximum_changed_files,
            });
        }
        let mut builder = GlobSetBuilder::new();
        for pattern in &constraints.allowed_write_globs {
            builder.add(
                Glob::new(pattern)
                    .map_err(|error| RepositoryError::InvalidWriteGlob(error.to_string()))?,
            );
        }
        let globs = builder
            .build()
            .map_err(|error| RepositoryError::InvalidWriteGlob(error.to_string()))?;
        if let Some(path) = changed.iter().find(|path| !globs.is_match(path)) {
            return Err(RepositoryError::UnexpectedFilesystemEffect(path.clone()));
        }
        Ok(changed)
    }

    pub async fn apply_strategy(
        worktree: &SessionWorktree,
        strategy: ApplicationStrategy,
    ) -> Result<ApplicationResult, RepositoryError> {
        ensure_session_path(
            &worktree.source_repository,
            worktree.session_id,
            &worktree.path,
        )?;
        let effects = Self::effects(worktree).await?;
        match &strategy {
            ApplicationStrategy::ExportPatch(destination) => {
                atomic_write(destination, &effects.binary_patch)?;
                Ok(ApplicationResult {
                    strategy: strategy.clone(),
                    detail: format!(
                        "exported {} bytes to {}",
                        effects.binary_patch.len(),
                        destination.display()
                    ),
                })
            }
            ApplicationStrategy::ApplyToCurrentTree => {
                if effects.binary_patch.is_empty() {
                    return Err(RepositoryError::EmptyPatch);
                }
                git_with_input(
                    &worktree.source_repository,
                    &["apply", "--check", "--binary", "-"],
                    &effects.binary_patch,
                    &[0],
                )
                .await?;
                git_with_input(
                    &worktree.source_repository,
                    &["apply", "--binary", "-"],
                    &effects.binary_patch,
                    &[0],
                )
                .await?;
                Ok(ApplicationResult {
                    strategy: strategy.clone(),
                    detail: format!(
                        "applied {} changed paths to active tree",
                        effects.changed_files.len()
                    ),
                })
            }
            ApplicationStrategy::CreateBranch { name } => {
                validate_branch_name(name)?;
                git_bytes(&worktree.path, &["switch", "-c", name]).await?;
                Ok(ApplicationResult {
                    strategy: strategy.clone(),
                    detail: format!("created branch `{name}` in isolated worktree"),
                })
            }
            ApplicationStrategy::Commit { message } => {
                if message.trim().is_empty() {
                    return Err(RepositoryError::InvalidCommitMessage);
                }
                git_bytes(&worktree.path, &["add", "--all", "--", "."]).await?;
                git_bytes(&worktree.path, &["commit", "-m", message]).await?;
                let commit = git_text(&worktree.path, &["rev-parse", "HEAD"]).await?;
                Ok(ApplicationResult {
                    strategy: strategy.clone(),
                    detail: format!("created isolated commit {}", commit.trim()),
                })
            }
            ApplicationStrategy::LeaveForReview => Ok(ApplicationResult {
                strategy: strategy.clone(),
                detail: format!("worktree retained at {}", worktree.path.display()),
            }),
            ApplicationStrategy::Discard => {
                let path = git_compatible_path(&worktree.path)?;
                git_bytes(
                    &worktree.source_repository,
                    &["worktree", "remove", "--force", &path],
                )
                .await?;
                Ok(ApplicationResult {
                    strategy: strategy.clone(),
                    detail: "isolated worktree discarded; active tree was not modified".into(),
                })
            }
        }
    }

    pub async fn rollback_all(worktree: &SessionWorktree) -> Result<(), RepositoryError> {
        ensure_session_path(
            &worktree.source_repository,
            worktree.session_id,
            &worktree.path,
        )?;
        git_bytes(
            &worktree.path,
            &[
                "restore",
                "--staged",
                "--worktree",
                "--source",
                "HEAD",
                "--",
                ".",
            ],
        )
        .await?;
        let untracked = git_bytes(
            &worktree.path,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ".",
            ],
        )
        .await?;
        for path in nul_paths(&untracked) {
            remove_untracked(&worktree.path, &path)?;
        }
        Ok(())
    }
}

fn atomic_write(destination: &Path, content: &[u8]) -> Result<(), RepositoryError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| RepositoryError::Io(error.error))?;
    Ok(())
}

fn validate_branch_name(name: &str) -> Result<(), RepositoryError> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains("..")
        || name.contains([' ', '~', '^', ':', '?', '*', '[', '\\'])
    {
        return Err(RepositoryError::InvalidBranchName(name.into()));
    }
    Ok(())
}

fn remove_untracked(root: &Path, relative: &Path) -> Result<(), RepositoryError> {
    if relative.is_absolute()
        || !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err(RepositoryError::UnsafeWorktreePath(relative.into()));
    }
    let absolute = root.join(relative);
    let metadata = std::fs::symlink_metadata(&absolute)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(absolute)?;
    } else {
        std::fs::remove_file(absolute)?;
    }
    Ok(())
}

fn nul_paths(bytes: &[u8]) -> BTreeSet<PathBuf> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| PathBuf::from(String::from_utf8_lossy(name).into_owned()))
        .collect()
}

async fn ensure_purrcode_excluded(repository: &Path) -> Result<(), RepositoryError> {
    let exclude = git_text(repository, &["rev-parse", "--git-path", "info/exclude"]).await?;
    let exclude = PathBuf::from(exclude.trim());
    let exclude = if exclude.is_absolute() {
        exclude
    } else {
        repository.join(exclude)
    };
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| line.trim() == ".purrcode/") {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)?;
    if !current.is_empty() && !current.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, ".purrcode/")?;
    file.sync_all()?;
    Ok(())
}

async fn canonical_repository_root(repository: &Path) -> Result<PathBuf, RepositoryError> {
    let repository = repository.canonicalize()?;
    let root = git_text(&repository, &["rev-parse", "--show-toplevel"]).await?;
    Ok(PathBuf::from(root.trim()).canonicalize()?)
}

fn ensure_session_path(
    repository: &Path,
    session_id: SessionId,
    candidate: &Path,
) -> Result<(), RepositoryError> {
    let expected = repository
        .join(".purrcode")
        .join("worktrees")
        .join(session_id.0.to_string());
    if candidate != expected && candidate != expected.canonicalize().unwrap_or(expected.clone()) {
        return Err(RepositoryError::UnsafeWorktreePath(candidate.to_path_buf()));
    }
    Ok(())
}

fn git_compatible_path(path: &Path) -> Result<String, RepositoryError> {
    let path = path
        .to_str()
        .ok_or_else(|| RepositoryError::NonUtf8Path(path.to_path_buf()))?;
    #[cfg(windows)]
    let path = normalize_windows_git_path(path);
    #[cfg(not(windows))]
    let path = path.to_owned();
    Ok(path)
}

#[cfg(any(windows, test))]
fn normalize_windows_git_path(path: &str) -> String {
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{path}")
    } else if let Some(path) = path.strip_prefix(r"\\?\") {
        path.to_owned()
    } else {
        path.to_owned()
    }
}

async fn git_text(directory: &Path, arguments: &[&str]) -> Result<String, RepositoryError> {
    let bytes = git_bytes(directory, arguments).await?;
    String::from_utf8(bytes).map_err(RepositoryError::GitUtf8)
}

async fn git_bytes(directory: &Path, arguments: &[&str]) -> Result<Vec<u8>, RepositoryError> {
    git_bytes_allow_codes(directory, arguments, &[0]).await
}

async fn git_bytes_allow_codes(
    directory: &Path,
    arguments: &[&str],
    allowed_exit_codes: &[i32],
) -> Result<Vec<u8>, RepositoryError> {
    let mut process = Command::new("git");
    process
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .envs(safe_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = timeout(GIT_TIMEOUT, process.output())
        .await
        .map_err(|_| RepositoryError::GitTimeout)??;
    if output.stdout.len() > MAX_GIT_OUTPUT || output.stderr.len() > MAX_GIT_OUTPUT {
        return Err(RepositoryError::GitOutputLimit);
    }
    if !output
        .status
        .code()
        .is_some_and(|code| allowed_exit_codes.contains(&code))
    {
        return Err(RepositoryError::GitFailed {
            arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output.stdout)
}

async fn git_with_input(
    directory: &Path,
    arguments: &[&str],
    input: &[u8],
    allowed_exit_codes: &[i32],
) -> Result<Vec<u8>, RepositoryError> {
    let mut process = Command::new("git");
    process
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .envs(safe_environment())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = process.spawn()?;
    let mut stdin = child.stdin.take().ok_or(RepositoryError::MissingStdin)?;
    stdin.write_all(input).await?;
    drop(stdin);
    let output = timeout(GIT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| RepositoryError::GitTimeout)??;
    if !output
        .status
        .code()
        .is_some_and(|code| allowed_exit_codes.contains(&code))
    {
        return Err(RepositoryError::GitFailed {
            arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output.stdout)
}

async fn initialize_local_submodules(
    source: &Path,
    worktree: &Path,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), RepositoryError> {
    if !source.join(".gitmodules").is_file() {
        return Ok((Vec::new(), Vec::new()));
    }
    let modules = submodule_entries(source).await?;
    let source_status = git_text(source, &["submodule", "status", "--recursive"]).await?;
    let source_initialized: std::collections::BTreeSet<_> = source_status
        .lines()
        .filter(|line| !line.starts_with('-'))
        .filter_map(|line| line.split_whitespace().nth(1).map(PathBuf::from))
        .collect();
    let mut initialized = Vec::new();
    let mut unavailable = Vec::new();
    for (_, relative) in &modules {
        if source_initialized.contains(relative) {
            initialized.push(relative.clone());
        } else {
            unavailable.push(relative.clone());
        }
    }
    let mut overrides = Vec::<(String, PathBuf)>::new();
    collect_submodule_overrides(source, &mut overrides).await?;
    for relative in &initialized {
        let mut arguments = Vec::<String>::new();
        for (name, local_source) in &overrides {
            arguments.push("-c".into());
            arguments.push(format!(
                "submodule.{name}.url={}",
                git_compatible_path(local_source)?
            ));
        }
        arguments.extend(
            [
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "--recursive",
                "--no-fetch",
                "--",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        arguments.push(relative.to_string_lossy().into_owned());
        let references: Vec<_> = arguments.iter().map(String::as_str).collect();
        git_text(worktree, &references).await?;
    }
    let status = git_text(worktree, &["submodule", "status", "--recursive"]).await?;
    let initialized_paths = status
        .lines()
        .filter(|line| !line.starts_with('-'))
        .filter_map(|line| {
            let path = line.split_whitespace().nth(1)?;
            Some(PathBuf::from(path))
        })
        .collect();
    Ok((initialized_paths, unavailable))
}

async fn submodule_entries(repository: &Path) -> Result<Vec<(String, PathBuf)>, RepositoryError> {
    if !repository.join(".gitmodules").is_file() {
        return Ok(Vec::new());
    }
    let file = repository.join(".gitmodules");
    let file = git_compatible_path(&file)?;
    let output = git_text(
        repository,
        &[
            "config",
            "-f",
            &file,
            "--get-regexp",
            r"^submodule\..*\.path$",
        ],
    )
    .await?;
    output
        .lines()
        .map(|line| {
            let (key, path) = line
                .split_once(char::is_whitespace)
                .ok_or(RepositoryError::MalformedSubmoduleConfig)?;
            let name = key
                .strip_prefix("submodule.")
                .and_then(|key| key.strip_suffix(".path"))
                .filter(|name| !name.is_empty())
                .ok_or(RepositoryError::MalformedSubmoduleConfig)?;
            let path = PathBuf::from(path.trim());
            if path.is_absolute()
                || !path
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Err(RepositoryError::MalformedSubmoduleConfig);
            }
            Ok((name.to_owned(), path))
        })
        .collect()
}

fn collect_submodule_overrides<'a>(
    repository: &'a Path,
    output: &'a mut Vec<(String, PathBuf)>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RepositoryError>> + Send + 'a>> {
    Box::pin(async move {
        for (name, relative) in submodule_entries(repository).await? {
            let source = repository.join(relative);
            if source.join(".git").exists() {
                output.push((name, source.clone()));
                collect_submodule_overrides(&source, output).await?;
            }
        }
        Ok(())
    })
}

fn safe_environment() -> Vec<(String, String)> {
    ["PATH", "TMPDIR", "LANG", "LC_ALL"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
        .collect()
}

fn parse_review_hunks(patch: &[u8]) -> Result<Vec<ReviewHunk>, RepositoryError> {
    if patch.len() > MAX_GIT_OUTPUT {
        return Err(RepositoryError::GitOutputLimit);
    }
    let text = std::str::from_utf8(patch).map_err(|_| RepositoryError::BinaryReviewHunk)?;
    let mut blocks = Vec::<Vec<&str>>::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            blocks.push(Vec::new());
        }
        if let Some(block) = blocks.last_mut() {
            block.push(line);
        }
    }
    let mut output = Vec::new();
    for block in blocks {
        if block
            .iter()
            .any(|line| line.starts_with("GIT binary patch") || line.starts_with("Binary files "))
        {
            continue;
        }
        let first_hunk = block
            .iter()
            .position(|line| line.starts_with("@@ "))
            .unwrap_or(block.len());
        if first_hunk == block.len() {
            continue;
        }
        let path = block[..first_hunk]
            .iter()
            .find_map(|line| {
                line.strip_prefix("+++ ")
                    .or_else(|| line.strip_prefix("--- "))
                    .map(str::trim)
                    .filter(|path| *path != "/dev/null")
                    .map(|path| {
                        path.strip_prefix("b/")
                            .or_else(|| path.strip_prefix("a/"))
                            .unwrap_or(path)
                    })
                    .map(PathBuf::from)
            })
            .ok_or(RepositoryError::MalformedReviewPatch)?;
        if path.is_absolute()
            || !path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            return Err(RepositoryError::MalformedReviewPatch);
        }
        let header = block[..first_hunk].concat();
        let mut cursor = first_hunk;
        while cursor < block.len() {
            let next = block[cursor + 1..]
                .iter()
                .position(|line| line.starts_with("@@ "))
                .map(|offset| cursor + 1 + offset)
                .unwrap_or(block.len());
            let body = block[cursor..next].concat();
            let patch = format!("{header}{body}").into_bytes();
            output.push(ReviewHunk {
                index: output.len(),
                path: path.clone(),
                preview: body.chars().take(1200).collect(),
                patch,
            });
            cursor = next;
        }
    }
    Ok(output)
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command {arguments:?} failed with {exit_code:?}: {stderr}")]
    GitFailed {
        arguments: Vec<String>,
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("git command exceeded its internal timeout")]
    GitTimeout,
    #[error("git output exceeded the bounded collector limit")]
    GitOutputLimit,
    #[error("git returned non-UTF-8 metadata: {0}")]
    GitUtf8(#[from] std::string::FromUtf8Error),
    #[error("session worktree already exists: {0}")]
    WorktreeAlreadyExists(PathBuf),
    #[error("refusing unsafe session worktree path: {0}")]
    UnsafeWorktreePath(PathBuf),
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("filesystem changed but the changed path could not be attributed")]
    UnattributedFilesystemEffect,
    #[error("filesystem changed {actual} files but only {allowed} were authorized")]
    TooManyFilesystemEffects { actual: usize, allowed: usize },
    #[error("filesystem changed an unauthorized path: {0}")]
    UnexpectedFilesystemEffect(PathBuf),
    #[error("authorized write glob is invalid: {0}")]
    InvalidWriteGlob(String),
    #[error("patch is empty")]
    EmptyPatch,
    #[error("review patch changed since it was displayed")]
    ReviewPatchChanged,
    #[error("review hunk {0} was not found")]
    ReviewHunkNotFound(usize),
    #[error("binary patches require whole-file review")]
    BinaryReviewHunk,
    #[error("review patch metadata is malformed")]
    MalformedReviewPatch,
    #[error("submodule configuration is malformed or unsafe")]
    MalformedSubmoduleConfig,
    #[error("branch name is invalid: {0}")]
    InvalidBranchName(String),
    #[error("commit message cannot be empty")]
    InvalidCommitMessage,
    #[error("git stdin was unavailable")]
    MissingStdin,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    #[test]
    fn windows_extended_paths_are_normalized_for_git() {
        assert_eq!(
            normalize_windows_git_path(r"\\?\C:\Users\runner\repo"),
            r"C:\Users\runner\repo"
        );
        assert_eq!(
            normalize_windows_git_path(r"\\?\UNC\server\share\repo"),
            r"\\server\share\repo"
        );
        assert_eq!(
            normalize_windows_git_path(r"C:\Users\runner\repo"),
            r"C:\Users\runner\repo"
        );
    }

    fn git(repository: &Path, arguments: &[&str]) {
        let status = StdCommand::new("git")
            .args(arguments)
            .current_dir(repository)
            .env("GIT_AUTHOR_NAME", "PurrCode Test")
            .env("GIT_AUTHOR_EMAIL", "test@local.invalid")
            .env("GIT_COMMITTER_NAME", "PurrCode Test")
            .env("GIT_COMMITTER_EMAIL", "test@local.invalid")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[tokio::test]
    async fn dirty_source_is_recorded_and_not_copied_into_worktree() {
        let temporary = tempfile::tempdir().unwrap();
        git(temporary.path(), &["init", "-q"]);
        std::fs::write(temporary.path().join("tracked.txt"), "base").unwrap();
        git(temporary.path(), &["add", "tracked.txt"]);
        git(temporary.path(), &["commit", "-q", "-m", "base"]);
        std::fs::write(temporary.path().join("tracked.txt"), "user change").unwrap();

        let worktree = RepositoryEngine::create_worktree(temporary.path(), SessionId::new())
            .await
            .unwrap();
        assert!(worktree.source_was_dirty);
        assert_eq!(
            std::fs::read_to_string(worktree.path.join("tracked.txt")).unwrap(),
            "base"
        );
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("tracked.txt")).unwrap(),
            "user change"
        );
    }

    #[tokio::test]
    async fn reviewed_hunks_apply_or_reject_without_overwriting_other_user_work() {
        let temporary = tempfile::tempdir().unwrap();
        git(temporary.path(), &["init", "-q"]);
        let original = (1..=30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(temporary.path().join("file.txt"), &original).unwrap();
        git(temporary.path(), &["add", "file.txt"]);
        git(temporary.path(), &["commit", "-q", "-m", "base"]);
        let worktree = RepositoryEngine::create_worktree(temporary.path(), SessionId::new())
            .await
            .unwrap();
        let changed = original
            .replace("line 2\n", "line two changed\n")
            .replace("line 28\n", "line twenty-eight changed\n");
        std::fs::write(worktree.path.join("file.txt"), changed).unwrap();
        let (digest, hunks) = RepositoryEngine::review_hunks(&worktree).await.unwrap();
        assert_eq!(hunks.len(), 2);
        RepositoryEngine::apply_review_hunk(&worktree, 0, &digest)
            .await
            .unwrap();
        let source = std::fs::read_to_string(temporary.path().join("file.txt")).unwrap();
        assert!(source.contains("line two changed"));
        assert!(source.contains("line 28\n"));
        RepositoryEngine::reject_review_hunk(&worktree, 1, &digest)
            .await
            .unwrap();
        let isolated = std::fs::read_to_string(worktree.path.join("file.txt")).unwrap();
        assert!(isolated.contains("line 28\n"));
        assert!(!isolated.contains("line twenty-eight changed"));
    }

    #[tokio::test]
    async fn initialized_local_submodules_are_available_in_isolated_worktrees() {
        let temporary = tempfile::tempdir().unwrap();
        let child = temporary.path().join("child");
        let parent = temporary.path().join("parent");
        std::fs::create_dir(&child).unwrap();
        std::fs::create_dir(&parent).unwrap();
        git(&child, &["init", "-q"]);
        std::fs::write(child.join("library.txt"), "child source").unwrap();
        git(&child, &["add", "library.txt"]);
        git(&child, &["commit", "-q", "-m", "child"]);
        git(&parent, &["init", "-q"]);
        let child_text = child.to_string_lossy().into_owned();
        git(
            &parent,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &child_text,
                "modules/child",
            ],
        );
        git(&parent, &["commit", "-q", "-m", "parent"]);
        let worktree = RepositoryEngine::create_worktree(&parent, SessionId::new())
            .await
            .unwrap();
        assert_eq!(
            worktree.initialized_submodules,
            vec![PathBuf::from("modules/child")]
        );
        assert!(worktree.unavailable_submodules.is_empty());
        assert_eq!(
            std::fs::read_to_string(worktree.path.join("modules/child/library.txt")).unwrap(),
            "child source"
        );
        std::fs::write(
            worktree.path.join("modules/child/library.txt"),
            "isolated change",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(parent.join("modules/child/library.txt")).unwrap(),
            "child source"
        );
    }

    #[tokio::test]
    async fn deinitialized_submodules_are_reported_without_network_fetches() {
        let temporary = tempfile::tempdir().unwrap();
        let child = temporary.path().join("child");
        let parent = temporary.path().join("parent");
        std::fs::create_dir(&child).unwrap();
        std::fs::create_dir(&parent).unwrap();
        git(&child, &["init", "-q"]);
        std::fs::write(child.join("library.txt"), "child source").unwrap();
        git(&child, &["add", "library.txt"]);
        git(&child, &["commit", "-q", "-m", "child"]);
        git(&parent, &["init", "-q"]);
        let child_text = child.to_string_lossy().into_owned();
        git(
            &parent,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &child_text,
                "modules/child",
            ],
        );
        git(&parent, &["commit", "-q", "-m", "parent"]);
        git(
            &parent,
            &["submodule", "deinit", "-q", "-f", "--", "modules/child"],
        );

        let worktree = RepositoryEngine::create_worktree(&parent, SessionId::new())
            .await
            .unwrap();
        assert!(worktree.initialized_submodules.is_empty());
        assert_eq!(
            worktree.unavailable_submodules,
            vec![PathBuf::from("modules/child")]
        );
        assert!(!worktree.path.join("modules/child/library.txt").exists());
    }

    #[tokio::test]
    async fn effects_include_staged_and_untracked_files() {
        let temporary = tempfile::tempdir().unwrap();
        git(temporary.path(), &["init", "-q"]);
        std::fs::write(temporary.path().join("tracked.txt"), "base").unwrap();
        git(temporary.path(), &["add", "tracked.txt"]);
        git(temporary.path(), &["commit", "-q", "-m", "base"]);
        let worktree = RepositoryEngine::create_worktree(temporary.path(), SessionId::new())
            .await
            .unwrap();
        assert!(
            !RepositoryEngine::inspect(temporary.path())
                .await
                .unwrap()
                .dirty
        );
        std::fs::write(worktree.path.join("tracked.txt"), "changed").unwrap();
        git(&worktree.path, &["add", "tracked.txt"]);
        std::fs::write(worktree.path.join("untracked.txt"), "new").unwrap();
        let effects = RepositoryEngine::effects(&worktree).await.unwrap();
        assert_eq!(
            effects.changed_files,
            vec![PathBuf::from("tracked.txt"), PathBuf::from("untracked.txt")]
        );
        let patch = String::from_utf8_lossy(&effects.binary_patch);
        assert!(patch.contains("tracked.txt"));
        assert!(patch.contains("untracked.txt"));
    }

    #[tokio::test]
    async fn explicit_apply_preserves_unrelated_dirty_source_change() {
        let temporary = tempfile::tempdir().unwrap();
        git(temporary.path(), &["init", "-q"]);
        std::fs::write(temporary.path().join("a.txt"), "a-base").unwrap();
        std::fs::write(temporary.path().join("b.txt"), "b-base").unwrap();
        git(temporary.path(), &["add", "a.txt", "b.txt"]);
        git(temporary.path(), &["commit", "-q", "-m", "base"]);
        std::fs::write(temporary.path().join("b.txt"), "user-dirty").unwrap();
        let worktree = RepositoryEngine::create_worktree(temporary.path(), SessionId::new())
            .await
            .unwrap();
        std::fs::write(worktree.path.join("a.txt"), "agent-change").unwrap();
        RepositoryEngine::apply_strategy(&worktree, ApplicationStrategy::ApplyToCurrentTree)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("a.txt")).unwrap(),
            "agent-change"
        );
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("b.txt")).unwrap(),
            "user-dirty"
        );
    }

    #[tokio::test]
    async fn rollback_removes_only_isolated_worktree_changes() {
        let temporary = tempfile::tempdir().unwrap();
        git(temporary.path(), &["init", "-q"]);
        std::fs::write(temporary.path().join("tracked.txt"), "base").unwrap();
        git(temporary.path(), &["add", "tracked.txt"]);
        git(temporary.path(), &["commit", "-q", "-m", "base"]);
        let worktree = RepositoryEngine::create_worktree(temporary.path(), SessionId::new())
            .await
            .unwrap();
        std::fs::write(worktree.path.join("tracked.txt"), "changed").unwrap();
        std::fs::write(worktree.path.join("untracked.txt"), "new").unwrap();
        RepositoryEngine::rollback_all(&worktree).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(worktree.path.join("tracked.txt")).unwrap(),
            "base"
        );
        assert!(!worktree.path.join("untracked.txt").exists());
        assert!(RepositoryEngine::effects(&worktree)
            .await
            .unwrap()
            .changed_files
            .is_empty());
    }
}
