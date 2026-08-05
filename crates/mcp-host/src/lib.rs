//! Isolated skill discovery and judgment-bound MCP JSON-RPC execution.

#![allow(clippy::collapsible_if)]

use purrcode_ninelives::{SessionStore, StoreError};
use purrcode_runtime_core::{
    ActionConstraints, ActionId, ApprovalAuthority, Authorization, CommandAction,
    ExternalToolAction, JudgmentDecision, ProposedAction, SessionEvent, SessionId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Duration, timeout};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default)]
    pub required_permissions: Vec<String>,
    #[serde(default)]
    pub supported_platforms: Vec<String>,
    #[serde(default)]
    pub network_access: bool,
    #[serde(default)]
    pub secrets_required: Vec<String>,
    #[serde(default)]
    pub model_capabilities: Vec<String>,
    #[serde(default)]
    pub min_purrcode_version: Option<String>,
    #[serde(default)]
    pub entrypoints: BTreeMap<String, String>,
    #[serde(default)]
    pub qualification: Option<SkillQualificationFixture>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillQualificationFixture {
    pub entrypoint: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub expected_output_schema: Option<Value>,
    #[serde(default = "default_qualification_timeout")]
    pub timeout_seconds: u64,
}

fn default_qualification_timeout() -> u64 {
    10
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedSkill {
    pub root: PathBuf,
    pub instructions: PathBuf,
    pub manifest: SkillManifest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstalledSkill {
    pub name: String,
    pub version: String,
    pub digest: String,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

pub fn discover_skills(root: &Path) -> Result<Vec<LoadedSkill>, HostError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut skills = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if !path.is_dir()
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        let instructions = path.join("SKILL.md");
        let manifest_path = path.join("manifest.toml");
        if !instructions.is_file() || !manifest_path.is_file() {
            return Err(HostError::InvalidSkill(format!(
                "{} must contain SKILL.md and manifest.toml",
                path.display()
            )));
        }
        let manifest: SkillManifest = toml::from_str(&std::fs::read_to_string(&manifest_path)?)?;
        validate_manifest(&manifest)?;
        skills.push(LoadedSkill {
            root: path,
            instructions,
            manifest,
        });
    }
    skills.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
    Ok(skills)
}

pub fn install_skill(source: &Path, root: &Path) -> Result<InstalledSkill, HostError> {
    let loaded = load_skill(source)?;
    semver::Version::parse(&loaded.manifest.version)
        .map_err(|error| HostError::InvalidSkill(format!("version is not semver: {error}")))?;
    std::fs::create_dir_all(root)?;
    let destination = root.join(&loaded.manifest.name);
    if destination.exists() {
        return Err(HostError::SkillAlreadyInstalled(destination));
    }
    let temporary = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root)?;
    copy_skill_tree(source, temporary.path())?;
    let digest = skill_digest(temporary.path())?;
    let record = InstalledSkill {
        name: loaded.manifest.name,
        version: loaded.manifest.version,
        digest,
        installed_at: chrono::Utc::now(),
    };
    std::fs::write(
        temporary.path().join(".purrcode-install.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    let persisted = temporary.keep();
    if let Err(error) = std::fs::rename(&persisted, &destination) {
        let _ = std::fs::remove_dir_all(&persisted);
        return Err(error.into());
    }
    Ok(record)
}

pub fn verify_installed_skill(path: &Path) -> Result<InstalledSkill, HostError> {
    let record: InstalledSkill =
        serde_json::from_slice(&std::fs::read(path.join(".purrcode-install.json"))?)?;
    let loaded = load_skill(path)?;
    if loaded.manifest.name != record.name || loaded.manifest.version != record.version {
        return Err(HostError::SkillIntegrity(
            "installed manifest identity differs from installation record".into(),
        ));
    }
    if skill_digest(path)? != record.digest {
        return Err(HostError::SkillIntegrity(
            "installed skill content digest does not match".into(),
        ));
    }
    Ok(record)
}

pub fn uninstall_skill(name: &str, root: &Path) -> Result<PathBuf, HostError> {
    if !safe_identifier(name) {
        return Err(HostError::InvalidSkill("unsafe skill name".into()));
    }
    let source = root.join(name);
    let record = verify_installed_skill(&source)?;
    let trash = root.join(".trash");
    std::fs::create_dir_all(&trash)?;
    let destination = trash.join(format!(
        "{}-{}-{}",
        name,
        record.version,
        uuid::Uuid::new_v4()
    ));
    std::fs::rename(&source, &destination)?;
    Ok(destination)
}

fn load_skill(path: &Path) -> Result<LoadedSkill, HostError> {
    let instructions = path.join("SKILL.md");
    let manifest_path = path.join("manifest.toml");
    if !path.is_dir() || !instructions.is_file() || !manifest_path.is_file() {
        return Err(HostError::InvalidSkill(format!(
            "{} must contain SKILL.md and manifest.toml",
            path.display()
        )));
    }
    let manifest: SkillManifest = toml::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    validate_manifest(&manifest)?;
    Ok(LoadedSkill {
        root: path.to_path_buf(),
        instructions,
        manifest,
    })
}

fn copy_skill_tree(source: &Path, destination: &Path) -> Result<(), HostError> {
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    fn visit(
        source: &Path,
        destination: &Path,
        files: &mut usize,
        bytes: &mut u64,
    ) -> Result<(), HostError> {
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                return Err(HostError::InvalidSkill(
                    "skill packages cannot contain symbolic links".into(),
                ));
            }
            let target = destination.join(entry.file_name());
            if metadata.is_dir() {
                std::fs::create_dir_all(&target)?;
                visit(&entry.path(), &target, files, bytes)?;
            } else if metadata.is_file() {
                *files += 1;
                *bytes = bytes.saturating_add(metadata.len());
                if *files > 10_000 || *bytes > 100 * 1024 * 1024 {
                    return Err(HostError::InvalidSkill(
                        "skill package exceeds file-count or byte limit".into(),
                    ));
                }
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }
    visit(source, destination, &mut files, &mut bytes)
}

pub fn skill_digest(root: &Path) -> Result<String, HostError> {
    fn paths(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> Result<(), HostError> {
        for entry in std::fs::read_dir(current)? {
            let path = entry?.path();
            if path.is_dir() {
                paths(root, &path, output)?;
            } else if path.file_name().and_then(|name| name.to_str())
                != Some(".purrcode-install.json")
            {
                output.push(
                    path.strip_prefix(root)
                        .map_err(|_| HostError::SkillIntegrity("path escaped skill root".into()))?
                        .to_path_buf(),
                );
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    paths(root, root, &mut files)?;
    files.sort();
    let mut hasher = blake3::Hasher::new();
    for relative in files {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(&std::fs::read(root.join(relative))?);
        hasher.update(&[0]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn validate_manifest(manifest: &SkillManifest) -> Result<(), HostError> {
    if !safe_identifier(&manifest.name) || manifest.version.trim().is_empty() {
        return Err(HostError::InvalidSkill(
            "name must be a safe identifier and version must be non-empty".into(),
        ));
    }
    if manifest
        .entrypoints
        .values()
        .any(|path| !safe_relative_path(Path::new(path)))
    {
        return Err(HostError::InvalidSkill(
            "entrypoints must be normalized relative paths".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub id: String,
    pub program: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub environment_from: BTreeMap<String, String>,
    pub working_directory: PathBuf,
    #[serde(default)]
    pub network: bool,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_output_limit")]
    pub maximum_output_bytes: usize,
    #[serde(default = "default_memory_limit")]
    pub memory_limit_bytes: u64,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct McpToolDescriptor {
    pub server_id: String,
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct McpCallResult {
    pub value: Value,
    pub stderr: String,
    pub capability_token_id: String,
}

pub struct McpHost;

impl McpHost {
    pub fn translate(
        server_id: &str,
        tool_name: &str,
        arguments: Value,
        working_directory: PathBuf,
    ) -> ProposedAction {
        ProposedAction::ExternalTool(ExternalToolAction {
            server_id: server_id.into(),
            tool_name: tool_name.into(),
            arguments,
            working_directory,
        })
    }

    pub async fn call(
        store: &mut SessionStore,
        action_id: ActionId,
        action: &ProposedAction,
        constraints: &ActionConstraints,
        server: &McpServerConfig,
    ) -> Result<McpCallResult, HostError> {
        let external = authorize_external(store, action_id, action, constraints, server)?;
        if external.tool_name == "__discover__" {
            return Err(HostError::WrongActionType);
        }
        let (value, stderr, capability_token_id) = run_rpc(
            server,
            "tools/call",
            json!({"name":external.tool_name,"arguments":external.arguments}),
        )
        .await?;
        Ok(McpCallResult {
            value,
            stderr,
            capability_token_id,
        })
    }

    pub async fn discover_tools(
        store: &mut SessionStore,
        action_id: ActionId,
        action: &ProposedAction,
        constraints: &ActionConstraints,
        server: &McpServerConfig,
    ) -> Result<Vec<McpToolDescriptor>, HostError> {
        let external = authorize_external(store, action_id, action, constraints, server)?;
        if external.tool_name != "__discover__" {
            return Err(HostError::WrongActionType);
        }
        let (value, _, _) = run_rpc(server, "tools/list", json!({})).await?;
        let tools = value["tools"]
            .as_array()
            .ok_or_else(|| HostError::InvalidRpc(value.clone()))?;
        tools
            .iter()
            .map(|tool| {
                let name = tool["name"]
                    .as_str()
                    .filter(|name| safe_identifier(name))
                    .ok_or_else(|| HostError::InvalidRpc(tool.clone()))?;
                Ok(McpToolDescriptor {
                    server_id: server.id.clone(),
                    name: name.into(),
                    description: tool["description"].as_str().map(str::to_owned),
                    input_schema: tool["inputSchema"].clone(),
                })
            })
            .collect()
    }
}

fn authorize_external<'a>(
    store: &mut SessionStore,
    action_id: ActionId,
    action: &'a ProposedAction,
    constraints: &ActionConstraints,
    server: &McpServerConfig,
) -> Result<&'a ExternalToolAction, HostError> {
    let ProposedAction::ExternalTool(external) = action else {
        return Err(HostError::WrongActionType);
    };
    let digest = action.digest(constraints)?;
    let authorization = store.consume_authorization(action_id, &digest)?;
    if authorization.constraints != *constraints
        || external.server_id != server.id
        || external.working_directory != server.working_directory
        || constraints.working_directory != server.working_directory
        || server.network != constraints.network
    {
        return Err(HostError::ConstraintMismatch);
    }
    server.validate()?;
    Ok(external)
}

async fn run_rpc(
    server: &McpServerConfig,
    method: &str,
    params: Value,
) -> Result<(Value, String, String), HostError> {
    let token_id = uuid::Uuid::new_v4().to_string();
    let token_secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut child = spawn_server(server, &token_id, &token_secret)?;
    let mut stdin = child.stdin.take().ok_or(HostError::MissingPipe)?;
    let stdout = child.stdout.take().ok_or(HostError::MissingPipe)?;
    let stderr = child.stderr.take().ok_or(HostError::MissingPipe)?;
    let stderr_limit = server.maximum_output_bytes;
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).take((stderr_limit + 1) as u64);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    write_rpc(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18",
            "capabilities":{},
            "clientInfo":{"name":"purrcode","version":env!("CARGO_PKG_VERSION")}
        }}),
    )
    .await?;
    let mut reader = BufReader::new(stdout);
    let initialized = read_rpc(&mut reader, server).await?;
    ensure_rpc_success(&initialized, 1)?;
    write_rpc(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    )
    .await?;
    write_rpc(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}),
    )
    .await?;
    let response = read_rpc(&mut reader, server).await?;
    ensure_rpc_success(&response, 2)?;
    drop(stdin);
    terminate(&mut child).await?;
    let stderr = stderr_task.await??;
    if stderr.len() > server.maximum_output_bytes {
        return Err(HostError::OutputLimit);
    }
    Ok((
        response["result"].clone(),
        String::from_utf8_lossy(&stderr).into_owned(),
        token_id,
    ))
}

impl McpServerConfig {
    fn validate(&self) -> Result<(), HostError> {
        if !safe_identifier(&self.id)
            || self.program.as_os_str().is_empty()
            || self.timeout_seconds == 0
            || self.maximum_output_bytes == 0
            || self.memory_limit_bytes < 16 * 1024 * 1024
        {
            return Err(HostError::InvalidServer);
        }
        if !self.working_directory.is_absolute() || !self.working_directory.is_dir() {
            return Err(HostError::InvalidServer);
        }
        Ok(())
    }
}

fn spawn_server(
    server: &McpServerConfig,
    token_id: &str,
    token_secret: &str,
) -> Result<Child, HostError> {
    let mut command = isolated_server_command(server)?;
    command
        .current_dir(&server.working_directory)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("PURRCODE_CAPABILITY_ID", token_id)
        .env("PURRCODE_CAPABILITY_TOKEN", token_secret)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (target, source) in &server.environment_from {
        let value =
            std::env::var(source).map_err(|_| HostError::MissingEnvironment(source.clone()))?;
        command.env(target, value);
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(target_os = "linux")]
    {
        let memory = server.memory_limit_bytes;
        unsafe {
            command.pre_exec(move || {
                let limit = libc::rlimit {
                    rlim_cur: memory,
                    rlim_max: memory,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command.spawn().map_err(Into::into)
}

fn isolated_server_command(server: &McpServerConfig) -> Result<Command, HostError> {
    #[cfg(target_os = "macos")]
    if Path::new("/usr/bin/sandbox-exec").is_file() {
        let grant = server
            .working_directory
            .canonicalize()?
            .to_str()
            .ok_or(HostError::InvalidServer)?
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let network = if server.network {
            "(allow network*)"
        } else {
            "(deny network*)"
        };
        let profile = format!(
            "(version 1) (deny default) (allow process*) (allow sysctl-read) \
             (allow file-read*) (allow file-write* (subpath \"{grant}\")) \
             (allow file-write* (subpath \"/private/tmp\")) \
             (allow file-write* (literal \"/dev/null\")) {network}"
        );
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .arg("-p")
            .arg(profile)
            .arg(&server.program)
            .args(&server.arguments);
        return Ok(command);
    }
    #[cfg(target_os = "linux")]
    if executable_on_path("bwrap") {
        let mut command = Command::new("bwrap");
        command.args(["--die-with-parent"]);
        if !server.network {
            command.arg("--unshare-net");
        }
        command
            .args(["--ro-bind", "/", "/"])
            .arg("--bind")
            .arg(&server.working_directory)
            .arg(&server.working_directory)
            .arg("--chdir")
            .arg(&server.working_directory)
            .arg(&server.program)
            .args(&server.arguments);
        return Ok(command);
    }
    let mut command = Command::new(&server.program);
    command.args(&server.arguments);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn executable_on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(program).is_file())
    })
}

async fn write_rpc(stdin: &mut tokio::process::ChildStdin, value: &Value) -> Result<(), HostError> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_rpc(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    server: &McpServerConfig,
) -> Result<Value, HostError> {
    let mut line = String::new();
    timeout(
        Duration::from_secs(server.timeout_seconds),
        reader.read_line(&mut line),
    )
    .await
    .map_err(|_| HostError::Timeout)??;
    if line.len() > server.maximum_output_bytes {
        return Err(HostError::OutputLimit);
    }
    Ok(serde_json::from_str(&line)?)
}

fn ensure_rpc_success(response: &Value, id: u64) -> Result<(), HostError> {
    if response["jsonrpc"] != "2.0" || response["id"] != id || response.get("error").is_some() {
        return Err(HostError::InvalidRpc(response.clone()));
    }
    Ok(())
}

async fn terminate(child: &mut Child) -> Result<(), HostError> {
    if child.try_wait()?.is_none() {
        if let Err(error) = child.start_kill() {
            if !matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
            ) {
                return Err(error.into());
            }
        }
    }
    match child.wait().await {
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
            ) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn default_timeout() -> u64 {
    30
}
fn default_output_limit() -> usize {
    1024 * 1024
}
fn default_memory_limit() -> u64 {
    512 * 1024 * 1024
}

#[derive(Debug, Error)]
pub enum HostError {
    #[error("skill is invalid: {0}")]
    InvalidSkill(String),
    #[error("skill is already installed at {0}")]
    SkillAlreadyInstalled(PathBuf),
    #[error("installed skill integrity failed: {0}")]
    SkillIntegrity(String),
    #[error("MCP server configuration is invalid")]
    InvalidServer,
    #[error("MCP action does not match persisted authorization or server grants")]
    ConstraintMismatch,
    #[error("MCP host received a non-external action")]
    WrongActionType,
    #[error("MCP child process pipe is unavailable")]
    MissingPipe,
    #[error("MCP response exceeded the authorized output limit")]
    OutputLimit,
    #[error("MCP request timed out")]
    Timeout,
    #[error("MCP JSON-RPC response is invalid: {0}")]
    InvalidRpc(Value),
    #[error("required environment variable `{0}` is unavailable")]
    MissingEnvironment(String),
    #[error("session authorization failed: {0}")]
    Store(#[from] StoreError),
    #[error("action digest failed: {0}")]
    Domain(#[from] purrcode_runtime_core::DomainError),
    #[error("MCP process I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("MCP task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("MCP JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("skill manifest TOML failed: {0}")]
    Toml(#[from] toml::de::Error),
}

/// Qualification status for a skill.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum QualificationStatus {
    Qualified,
    QualifiedWithConstraints,
    Failed,
    Unverified,
    Blocked,
    Outdated,
    Incompatible,
}

/// Result of a qualification case.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QualificationCase {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// Full qualification report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillQualificationReport {
    pub name: String,
    pub version: String,
    pub status: QualificationStatus,
    pub cases: Vec<QualificationCase>,
    pub constraints: Option<ActionConstraints>,
}

fn find_symlinks(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_symlink() {
                found.push(path);
            } else if path.is_dir() {
                found.extend(find_symlinks(&path));
            }
        }
    }
    found
}

fn load_manifest(root: &Path) -> Result<SkillManifest, HostError> {
    let path = root.join("manifest.toml");
    let content = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&content)?)
}

pub fn read_skill_manifest(root: &Path) -> Result<SkillManifest, HostError> {
    load_manifest(root)
}

/// Static qualification engine for downloaded skills.
pub struct Qualifier;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DynamicQualificationRequest {
    pub entrypoint: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub timeout_seconds: u64,
    #[serde(default)]
    pub expected_output_schema: Option<Value>,
}

impl Qualifier {
    /// Run all qualification checks on a skill directory.
    /// Returns a report summarising which checks passed or failed.
    pub fn qualify(root: &Path) -> SkillQualificationReport {
        let manifest_path = root.join("manifest.toml");
        let mut cases = Vec::new();
        let mut failed = false;
        let mut blocked = false;
        let mut incompatible = false;

        // 1. Manifest schema validation
        if manifest_path.exists() {
            match std::fs::read_to_string(&manifest_path) {
                Ok(content) => match toml::from_str::<SkillManifest>(&content) {
                    Ok(_) => cases.push(QualificationCase {
                        name: "manifest_schema".into(),
                        passed: true,
                        detail: "manifest parses successfully".into(),
                    }),
                    Err(e) => {
                        failed = true;
                        cases.push(QualificationCase {
                            name: "manifest_schema".into(),
                            passed: false,
                            detail: format!("manifest parse error: {e}"),
                        });
                    }
                },
                Err(e) => {
                    failed = true;
                    cases.push(QualificationCase {
                        name: "manifest_schema".into(),
                        passed: false,
                        detail: format!("cannot read manifest: {e}"),
                    });
                }
            }
        } else {
            failed = true;
            blocked = true;
            cases.push(QualificationCase {
                name: "manifest_schema".into(),
                passed: false,
                detail: "manifest.toml not found".into(),
            });
        }

        // 2. Symlink / path-escape rejection
        let symlinks = find_symlinks(root);
        if symlinks.is_empty() {
            cases.push(QualificationCase {
                name: "no_symlinks".into(),
                passed: true,
                detail: "no symlinks found".into(),
            });
        } else {
            failed = true;
            blocked = true;
            cases.push(QualificationCase {
                name: "no_symlinks".into(),
                passed: false,
                detail: format!("symlinks rejected: {symlinks:?}"),
            });
        }

        // 3. Entrypoint validation
        if let Ok(manifest) = load_manifest(root) {
            let canonical_root = root.canonicalize().ok();
            let entrypoint_ok = canonical_root.as_ref().is_some_and(|canonical_root| {
                !manifest.entrypoints.is_empty()
                    && manifest.entrypoints.values().all(|ep| {
                        let declared = Path::new(ep);
                        !declared.is_absolute()
                            && root
                                .join(declared)
                                .canonicalize()
                                .is_ok_and(|resolved| resolved.starts_with(canonical_root))
                    })
            });
            if entrypoint_ok {
                cases.push(QualificationCase {
                    name: "entrypoints".into(),
                    passed: true,
                    detail: "entrypoints exist and are relative".into(),
                });
            } else {
                failed = true;
                blocked = true;
                cases.push(QualificationCase {
                    name: "entrypoints".into(),
                    passed: false,
                    detail: "one or more entrypoints missing or absolute".into(),
                });
            }

            // 4. Platform compatibility
            let platform = if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(target_os = "linux") {
                "linux"
            } else {
                "windows"
            };
            if manifest.supported_platforms.is_empty()
                || manifest.supported_platforms.contains(&platform.to_string())
            {
                cases.push(QualificationCase {
                    name: "platform_compatibility".into(),
                    passed: true,
                    detail: format!("compatible with {platform}"),
                });
            } else {
                failed = true;
                incompatible = true;
                cases.push(QualificationCase {
                    name: "platform_compatibility".into(),
                    passed: false,
                    detail: format!("not compatible with {platform}"),
                });
            }
            if let Some(minimum) = &manifest.min_purrcode_version {
                let compatible = semver::Version::parse(env!("CARGO_PKG_VERSION"))
                    .ok()
                    .zip(semver::Version::parse(minimum).ok())
                    .is_some_and(|(current, minimum)| current >= minimum);
                if !compatible {
                    failed = true;
                    incompatible = true;
                }
                cases.push(QualificationCase {
                    name: "purrcode_compatibility".into(),
                    passed: compatible,
                    detail: if compatible {
                        format!("requires PurrCode {minimum} or newer")
                    } else {
                        format!("requires incompatible PurrCode version {minimum}")
                    },
                });
            }
        } else {
            failed = true;
            cases.push(QualificationCase {
                name: "entrypoints".into(),
                passed: false,
                detail: "cannot load manifest".into(),
            });
            cases.push(QualificationCase {
                name: "platform_compatibility".into(),
                passed: false,
                detail: "cannot load manifest".into(),
            });
        }

        // 5. Content digest verification
        if let Ok(digest) = skill_digest(root) {
            cases.push(QualificationCase {
                name: "content_digest".into(),
                passed: true,
                detail: format!("digest: {digest}"),
            });
        } else {
            failed = true;
            cases.push(QualificationCase {
                name: "content_digest".into(),
                passed: false,
                detail: "cannot compute digest".into(),
            });
        }

        let status = if blocked {
            QualificationStatus::Blocked
        } else if incompatible {
            QualificationStatus::Incompatible
        } else if failed {
            QualificationStatus::Failed
        } else {
            QualificationStatus::Qualified
        };

        SkillQualificationReport {
            name: root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            version: load_manifest(root)
                .map(|manifest| manifest.version)
                .unwrap_or_else(|_| "unknown".into()),
            status,
            cases,
            constraints: None,
        }
    }

    pub async fn qualify_dynamic(
        store: &mut SessionStore,
        session_id: SessionId,
        root: &Path,
        request: &DynamicQualificationRequest,
    ) -> SkillQualificationReport {
        let mut report = Self::qualify(root);
        let canonical_root = match root.canonicalize() {
            Ok(root) => root,
            Err(error) => {
                report.status = QualificationStatus::Failed;
                report.cases.push(QualificationCase {
                    name: "dynamic_claw".into(),
                    passed: false,
                    detail: format!("skill root unavailable: {error}"),
                });
                return report;
            }
        };
        let declared = Path::new(&request.entrypoint);
        let entrypoint_is_declared = load_manifest(root).ok().is_some_and(|manifest| {
            manifest
                .entrypoints
                .values()
                .any(|entrypoint| entrypoint == &request.entrypoint)
        });
        let resolved = if declared.is_absolute() || !entrypoint_is_declared {
            None
        } else {
            canonical_root
                .join(declared)
                .canonicalize()
                .ok()
                .filter(|path| path.starts_with(&canonical_root))
        };
        let Some(program) = resolved else {
            report.status = QualificationStatus::Blocked;
            report.cases.push(QualificationCase {
                name: "dynamic_entrypoint_containment".into(),
                passed: false,
                detail: "entrypoint is undeclared or escapes the canonical skill root".into(),
            });
            return report;
        };
        let capability = purrcode_claw::sandbox_capability();
        if !capability.network_isolation {
            report.status = QualificationStatus::Unverified;
            report.cases.push(QualificationCase {
                name: "dynamic_claw".into(),
                passed: false,
                detail: format!(
                    "dynamic execution withheld because backend {} cannot prove network isolation",
                    capability.backend
                ),
            });
            return report;
        }
        let constraints = ActionConstraints {
            working_directory: canonical_root.clone(),
            network: false,
            timeout_seconds: request.timeout_seconds.clamp(1, 300),
            maximum_output_bytes: 1_048_576,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let action = ProposedAction::Command(CommandAction {
            program,
            arguments: request.arguments.clone(),
            working_directory: canonical_root,
            environment: BTreeMap::new(),
        });
        let digest = match action.digest(&constraints) {
            Ok(digest) => digest,
            Err(error) => {
                report.status = QualificationStatus::Failed;
                report.cases.push(QualificationCase {
                    name: "dynamic_authorization".into(),
                    passed: false,
                    detail: error.to_string(),
                });
                return report;
            }
        };
        if let Err(error) = store
            .append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                },
            )
            .and_then(|_| {
                store.append(
                    session_id,
                    &SessionEvent::JudgmentRecorded {
                        action_id,
                        decision: JudgmentDecision::AllowWithConstraints(constraints.clone()),
                    },
                )
            })
        {
            report.status = QualificationStatus::Failed;
            report.cases.push(QualificationCase {
                name: "dynamic_authorization".into(),
                passed: false,
                detail: error.to_string(),
            });
            return report;
        }
        if let Err(error) = store.authorize(&Authorization {
            action_id,
            session_id,
            action_digest: digest,
            constraints: constraints.clone(),
            authorized_at: chrono::Utc::now(),
            approved_by: ApprovalAuthority::DeterministicPolicy,
        }) {
            report.status = QualificationStatus::Failed;
            report.cases.push(QualificationCase {
                name: "dynamic_authorization".into(),
                passed: false,
                detail: error.to_string(),
            });
            return report;
        }
        let before = qualification_snapshot(root);
        if let Err(error) = store.append(session_id, &SessionEvent::ExecutionStarted { action_id })
        {
            report.status = QualificationStatus::Failed;
            report.cases.push(QualificationCase {
                name: "dynamic_execution_event".into(),
                passed: false,
                detail: error.to_string(),
            });
            return report;
        }
        match purrcode_claw::ToolRuntime::execute(store, action_id, &action, &constraints).await {
            Ok(result) => {
                if let Err(error) = store.append(
                    session_id,
                    &SessionEvent::ExecutionFinished {
                        action_id,
                        exit_code: result.exit_code,
                        truncated: result.truncated,
                        sandbox_level: Some(format!("{:?}", result.sandbox_level)),
                        sandbox_backend: Some(result.sandbox_backend.clone()),
                    },
                ) {
                    report.status = QualificationStatus::Failed;
                    report.cases.push(QualificationCase {
                        name: "dynamic_execution_event".into(),
                        passed: false,
                        detail: error.to_string(),
                    });
                    return report;
                }
                let after = qualification_snapshot(root);
                let filesystem_unchanged =
                    matches!((&before, &after), (Ok(before), Ok(after)) if before == after);
                let output = String::from_utf8_lossy(&result.stdout);
                let schema_valid = request
                    .expected_output_schema
                    .as_ref()
                    .is_none_or(|schema| {
                        serde_json::from_str::<Value>(&output).is_ok_and(|value| {
                            schema.as_object().is_none_or(|expected| {
                                expected.keys().all(|key| value.get(key).is_some())
                            })
                        })
                    });
                report.cases.push(QualificationCase {
                    name: "dynamic_claw".into(),
                    passed: result.exit_code == Some(0),
                    detail: format!(
                        "exit={:?}; backend={}; network_isolation={}",
                        result.exit_code,
                        result.sandbox_backend,
                        matches!(
                            result.sandbox_level,
                            purrcode_claw::SandboxLevel::RestrictedProcessNoNetwork
                        )
                    ),
                });
                report.cases.push(QualificationCase {
                    name: "observed_filesystem_access".into(),
                    passed: filesystem_unchanged,
                    detail: if filesystem_unchanged {
                        "no filesystem changes observed".into()
                    } else {
                        "qualification entrypoint changed skill files".into()
                    },
                });
                report.cases.push(QualificationCase {
                    name: "observed_network_access".into(),
                    passed: matches!(
                        result.sandbox_level,
                        purrcode_claw::SandboxLevel::RestrictedProcessNoNetwork
                    ),
                    detail: format!("network denied by {}", result.sandbox_backend),
                });
                report.cases.push(QualificationCase {
                    name: "secret_access".into(),
                    passed: true,
                    detail:
                        "child environment was cleared and rebuilt from the Claw safe allowlist"
                            .into(),
                });
                report.cases.push(QualificationCase {
                    name: "output_schema".into(),
                    passed: schema_valid,
                    detail: if schema_valid {
                        "output schema satisfied".into()
                    } else {
                        "output schema mismatch".into()
                    },
                });
                if result.exit_code != Some(0) || !schema_valid || !filesystem_unchanged {
                    report.status = QualificationStatus::Failed;
                } else if !matches!(
                    result.sandbox_level,
                    purrcode_claw::SandboxLevel::RestrictedProcessNoNetwork
                ) {
                    report.status = QualificationStatus::QualifiedWithConstraints;
                    report.constraints = Some(constraints);
                }
            }
            Err(error) => {
                if let Err(event_error) = store.append(
                    session_id,
                    &SessionEvent::ExecutionFinished {
                        action_id,
                        exit_code: None,
                        truncated: false,
                        sandbox_level: Some("qualification_failed".into()),
                        sandbox_backend: Some(capability.backend.clone()),
                    },
                ) {
                    report.cases.push(QualificationCase {
                        name: "dynamic_execution_event".into(),
                        passed: false,
                        detail: event_error.to_string(),
                    });
                }
                report.status = QualificationStatus::Failed;
                report.cases.push(QualificationCase {
                    name: "dynamic_claw".into(),
                    passed: false,
                    detail: error.to_string(),
                });
            }
        }
        report
    }
}

fn qualification_snapshot(root: &Path) -> Result<BTreeMap<PathBuf, String>, std::io::Error> {
    fn visit(
        root: &Path,
        directory: &Path,
        snapshot: &mut BTreeMap<PathBuf, String>,
    ) -> Result<(), std::io::Error> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                visit(root, &path, snapshot)?;
            } else {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_owned();
                snapshot.insert(
                    relative,
                    blake3::hash(&std::fs::read(&path)?).to_hex().to_string(),
                );
            }
        }
        Ok(())
    }
    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use chrono::Utc;
    #[cfg(unix)]
    use purrcode_runtime_core::{
        ApprovalAuthority, Authorization, JudgmentDecision, SessionEvent, SessionId,
    };

    #[test]
    fn skill_discovery_rejects_traversing_entrypoints() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("unsafe");
        std::fs::create_dir(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# Unsafe").unwrap();
        std::fs::write(
            skill.join("manifest.toml"),
            "name='unsafe'\nversion='1.0.0'\n[entrypoints]\nrun='../escape'\n",
        )
        .unwrap();
        assert!(matches!(
            discover_skills(root.path()),
            Err(HostError::InvalidSkill(_))
        ));
    }

    #[test]
    fn skill_installation_is_atomic_versioned_and_integrity_checked() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let installed = temporary.path().join("installed");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "# Safe skill").unwrap();
        std::fs::write(
            source.join("manifest.toml"),
            "name='safe-skill'\nversion='1.2.3'\n",
        )
        .unwrap();
        let record = install_skill(&source, &installed).unwrap();
        assert_eq!(record.version, "1.2.3");
        let path = installed.join("safe-skill");
        assert_eq!(verify_installed_skill(&path).unwrap().digest, record.digest);
        assert!(install_skill(&source, &installed).is_err());
        std::fs::write(path.join("SKILL.md"), "tampered").unwrap();
        assert!(matches!(
            verify_installed_skill(&path),
            Err(HostError::SkillIntegrity(_))
        ));
        std::fs::write(path.join("SKILL.md"), "# Safe skill").unwrap();
        let trash = uninstall_skill("safe-skill", &installed).unwrap();
        assert!(trash.is_dir());
        assert!(!path.exists());
        assert!(discover_skills(&installed).unwrap().is_empty());
    }

    #[test]
    fn qualification_reports_blocked_incompatible_and_failed_states() {
        let temporary = tempfile::tempdir().unwrap();
        let missing = temporary.path().join("missing");
        std::fs::create_dir(&missing).unwrap();
        assert_eq!(
            Qualifier::qualify(&missing).status,
            QualificationStatus::Blocked
        );

        let incompatible = temporary.path().join("incompatible");
        std::fs::create_dir(&incompatible).unwrap();
        std::fs::write(incompatible.join("SKILL.md"), "# incompatible").unwrap();
        std::fs::write(incompatible.join("run"), "fixture").unwrap();
        std::fs::write(
            incompatible.join("manifest.toml"),
            "name='incompatible'\nversion='1.0.0'\nsupported_platforms=['not-this-platform']\n[entrypoints]\nrun='run'\n",
        )
        .unwrap();
        assert_eq!(
            Qualifier::qualify(&incompatible).status,
            QualificationStatus::Incompatible
        );

        let invalid = temporary.path().join("invalid");
        std::fs::create_dir(&invalid).unwrap();
        std::fs::write(invalid.join("manifest.toml"), "this is not toml = [").unwrap();
        assert_eq!(
            Qualifier::qualify(&invalid).status,
            QualificationStatus::Failed
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dynamic_qualification_runs_only_contained_entrypoint_with_exact_authorization() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let skill = temporary.path().join("dynamic");
        std::fs::create_dir(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# Dynamic").unwrap();
        std::fs::write(
            skill.join("manifest.toml"),
            "name='dynamic'\nversion='1.0.0'\n[entrypoints]\nrun='run'\n",
        )
        .unwrap();
        let entrypoint = skill.join("run");
        std::fs::write(&entrypoint, "#!/bin/sh\nprintf '{\"ok\":true}'\n").unwrap();
        let mut permissions = std::fs::metadata(&entrypoint).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&entrypoint, permissions).unwrap();
        let session_id = SessionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        store
            .append(
                session_id,
                &purrcode_runtime_core::SessionEvent::SessionCreated {
                    objective: "qualify fixture".into(),
                    repository: skill.clone(),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        let report = Qualifier::qualify_dynamic(
            &mut store,
            session_id,
            &skill,
            &DynamicQualificationRequest {
                entrypoint: "run".into(),
                arguments: Vec::new(),
                timeout_seconds: 15,
                expected_output_schema: Some(serde_json::json!({"ok": true})),
            },
        )
        .await;
        if purrcode_claw::sandbox_capability().network_isolation {
            assert!(matches!(
                report.status,
                QualificationStatus::Qualified | QualificationStatus::QualifiedWithConstraints
            ));
            assert!(
                report
                    .cases
                    .iter()
                    .any(|case| case.name == "dynamic_claw" && case.passed)
            );
        } else {
            assert_eq!(report.status, QualificationStatus::Unverified);
        }

        let blocked = Qualifier::qualify_dynamic(
            &mut store,
            session_id,
            &skill,
            &DynamicQualificationRequest {
                entrypoint: "../escape".into(),
                arguments: Vec::new(),
                timeout_seconds: 2,
                expected_output_schema: None,
            },
        )
        .await;
        assert_eq!(blocked.status, QualificationStatus::Blocked);

        let undeclared = skill.join("undeclared");
        std::fs::write(&undeclared, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&undeclared).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&undeclared, permissions).unwrap();
        let blocked = Qualifier::qualify_dynamic(
            &mut store,
            session_id,
            &skill,
            &DynamicQualificationRequest {
                entrypoint: "undeclared".into(),
                arguments: Vec::new(),
                timeout_seconds: 2,
                expected_output_schema: None,
            },
        )
        .await;
        assert_eq!(blocked.status, QualificationStatus::Blocked);

        let schema_mismatch = Qualifier::qualify_dynamic(
            &mut store,
            session_id,
            &skill,
            &DynamicQualificationRequest {
                entrypoint: "run".into(),
                arguments: Vec::new(),
                timeout_seconds: 2,
                expected_output_schema: Some(serde_json::json!({"missing": true})),
            },
        )
        .await;
        if purrcode_claw::sandbox_capability().network_isolation {
            assert_eq!(schema_mismatch.status, QualificationStatus::Failed);
            assert!(
                schema_mismatch
                    .cases
                    .iter()
                    .any(|case| case.name == "output_schema" && !case.passed)
            );
        } else {
            assert_eq!(schema_mismatch.status, QualificationStatus::Unverified);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_call_requires_and_consumes_exact_authorization() {
        let repository = tempfile::tempdir().unwrap();
        let server = McpServerConfig {
            id: "fixture".into(),
            program: "/bin/sh".into(),
            arguments: vec![
                "-c".into(),
                "read init; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'; read notification; read call; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}'"
                    .into(),
            ],
            environment_from: BTreeMap::new(),
            working_directory: repository.path().to_path_buf(),
            network: false,
            timeout_seconds: 20,
            maximum_output_bytes: 4096,
            memory_limit_bytes: 64 * 1024 * 1024,
        };
        let action = McpHost::translate(
            "fixture",
            "echo",
            json!({"message":"hello"}),
            repository.path().to_path_buf(),
        );
        let constraints = ActionConstraints {
            working_directory: repository.path().to_path_buf(),
            network: false,
            timeout_seconds: 20,
            maximum_output_bytes: 4096,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        };
        let action_id = ActionId::new();
        let mut denied_store = SessionStore::in_memory().unwrap();
        assert!(matches!(
            McpHost::call(&mut denied_store, action_id, &action, &constraints, &server).await,
            Err(HostError::Store(StoreError::AuthorizationUnavailable))
        ));

        let mut mismatch_store = SessionStore::in_memory().unwrap();
        let mismatch_session_id = SessionId::new();
        mismatch_store
            .append(
                mismatch_session_id,
                &SessionEvent::SessionCreated {
                    objective: "test mismatched MCP authorization".into(),
                    repository: repository.path().to_path_buf(),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        mismatch_store
            .append(
                mismatch_session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                },
            )
            .unwrap();
        mismatch_store
            .append(
                mismatch_session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: JudgmentDecision::RequireApproval {
                        reason: "test exact MCP authorization".into(),
                        constraints: constraints.clone(),
                    },
                },
            )
            .unwrap();
        mismatch_store
            .authorize(&Authorization {
                action_id,
                session_id: mismatch_session_id,
                action_digest: "not-the-serialized-action-digest".into(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::Human,
            })
            .unwrap();
        assert!(
            McpHost::call(
                &mut mismatch_store,
                action_id,
                &action,
                &constraints,
                &server
            )
            .await
            .is_err()
        );

        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "test exact MCP authorization consumption".into(),
                    repository: repository.path().to_path_buf(),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        store
            .append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                },
            )
            .unwrap();
        store
            .append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: JudgmentDecision::RequireApproval {
                        reason: "test exact MCP authorization".into(),
                        constraints: constraints.clone(),
                    },
                },
            )
            .unwrap();
        store
            .authorize(&Authorization {
                action_id,
                session_id,
                action_digest: action.digest(&constraints).unwrap(),
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::Human,
            })
            .unwrap();
        let result = McpHost::call(&mut store, action_id, &action, &constraints, &server)
            .await
            .unwrap();
        assert_eq!(
            result.value["content"][0]["text"],
            serde_json::Value::String("ok".into())
        );
        assert!(matches!(
            McpHost::call(&mut store, action_id, &action, &constraints, &server).await,
            Err(HostError::Store(StoreError::AuthorizationUnavailable))
        ));
    }
}
