//! Isolated skill discovery and judgment-bound MCP JSON-RPC execution.

use purrcode_ninelives::{SessionStore, StoreError};
use purrcode_runtime_core::{ActionConstraints, ActionId, ExternalToolAction, ProposedAction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{timeout, Duration};

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
    pub entrypoints: BTreeMap<String, String>,
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

fn skill_digest(root: &Path) -> Result<String, HostError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use chrono::Utc;
    #[cfg(unix)]
    use purrcode_runtime_core::{ApprovalAuthority, Authorization, SessionId};

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
            timeout_seconds: 5,
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
            timeout_seconds: 5,
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
