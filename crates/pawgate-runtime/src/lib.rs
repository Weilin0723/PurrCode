//! Deterministic, model-independent pre-execution policy.

use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use purrcode_runtime_core::{ActionConstraints, JudgmentDecision, ProposedAction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Policy {
    #[serde(default = "default_read_only_programs")]
    pub read_only_programs: BTreeSet<String>,
    #[serde(default = "default_approval_programs")]
    pub approval_required_programs: BTreeSet<String>,
    #[serde(default = "default_denied_fragments")]
    pub denied_argument_fragments: BTreeSet<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_output")]
    pub maximum_output_bytes: usize,
    #[serde(default)]
    pub auto_allow_worktree_writes: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SignedPolicyPack {
    pub version: String,
    pub issuer: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub allowed_overrides: BTreeSet<String>,
    pub payload_hash: String,
    pub signature: String,
    pub policy: Policy,
}

#[derive(Serialize)]
struct SignedPayload<'a> {
    version: &'a str,
    issuer: &'a str,
    expires_at: DateTime<Utc>,
    allowed_overrides: &'a BTreeSet<String>,
    policy: &'a Policy,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            read_only_programs: default_read_only_programs(),
            approval_required_programs: default_approval_programs(),
            denied_argument_fragments: default_denied_fragments(),
            timeout_seconds: default_timeout(),
            maximum_output_bytes: default_output(),
            auto_allow_worktree_writes: false,
        }
    }
}

impl Policy {
    pub fn load(path: &Path) -> Result<Self, PolicyError> {
        Ok(toml::from_str(&fs::read_to_string(path)?)?)
    }

    pub fn load_effective(
        repository_policy: Option<&Path>,
        organization_pack: &Path,
        public_key_hex: &str,
    ) -> Result<Self, PolicyError> {
        let local = match repository_policy {
            Some(path) if path.exists() => Self::load(path)?,
            _ => Self::default(),
        };
        let organization = SignedPolicyPack::load_verified(organization_pack, public_key_hex)?;
        Ok(organization.restrict(local))
    }

    pub fn evaluate(&self, action: &ProposedAction, repository: &Path) -> JudgmentDecision {
        match action {
            ProposedAction::Command(command) => {
                let Some(program) = command.program.file_name().and_then(|p| p.to_str()) else {
                    return JudgmentDecision::Deny {
                        reason: "program path has no valid executable name".into(),
                    };
                };
                if command.program != Path::new(program) {
                    return JudgmentDecision::Deny {
                        reason: "program must be a bare policy name, not a path".into(),
                    };
                }
                if command.working_directory != repository {
                    return JudgmentDecision::Deny {
                        reason:
                            "working directory does not exactly match the authorized repository"
                                .into(),
                    };
                }
                let normalized = command.arguments.join(" ").to_ascii_lowercase();
                if let Some(fragment) = self
                    .denied_argument_fragments
                    .iter()
                    .find(|f| normalized.contains(f.as_str()))
                {
                    return JudgmentDecision::Deny {
                        reason: format!("arguments contain hard-denied fragment: {fragment}"),
                    };
                }
                if !command.environment.is_empty() {
                    return JudgmentDecision::RequireApproval {
                        reason: "custom process environment requires human review".into(),
                        constraints: ActionConstraints::read_only(repository.to_path_buf()),
                    };
                }
                if self.read_only_programs.contains(program) {
                    if let Some(reason) =
                        unsafe_read_command(program, &command.arguments, repository)
                    {
                        return JudgmentDecision::Deny { reason };
                    }
                    return JudgmentDecision::AllowWithConstraints(ActionConstraints {
                        working_directory: repository.to_path_buf(),
                        network: false,
                        timeout_seconds: self.timeout_seconds,
                        maximum_output_bytes: self.maximum_output_bytes,
                        allowed_write_globs: Vec::new(),
                        maximum_changed_files: 0,
                    });
                }
                if self.approval_required_programs.contains(program) {
                    return JudgmentDecision::RequireApproval {
                        reason: format!("{program} may mutate repository or external state"),
                        constraints: ActionConstraints {
                            working_directory: repository.to_path_buf(),
                            network: false,
                            timeout_seconds: self.timeout_seconds,
                            maximum_output_bytes: self.maximum_output_bytes,
                            allowed_write_globs: Vec::new(),
                            maximum_changed_files: 0,
                        },
                    };
                }
                JudgmentDecision::Deny {
                    reason: format!("program `{program}` is not present in policy"),
                }
            }
            ProposedAction::WriteFile(write) => {
                self.evaluate_file_mutation(repository, &write.path, "write")
            }
            ProposedAction::DeleteFile(delete) => {
                self.evaluate_file_mutation(repository, &delete.path, "delete")
            }
            ProposedAction::ExternalTool(external) => {
                if external.working_directory != repository {
                    return JudgmentDecision::Deny {
                        reason:
                            "external tool working directory does not match the session worktree"
                                .into(),
                    };
                }
                if external.server_id.is_empty()
                    || external.tool_name.is_empty()
                    || !external
                        .server_id
                        .chars()
                        .chain(external.tool_name.chars())
                        .all(|character| {
                            character.is_ascii_alphanumeric()
                                || matches!(character, '-' | '_' | '.')
                        })
                {
                    return JudgmentDecision::Deny {
                        reason: "external server and tool names must be non-empty safe identifiers"
                            .into(),
                    };
                }
                JudgmentDecision::RequireApproval {
                    reason: format!(
                        "external tool `{}/{}` requires explicit authorization",
                        external.server_id, external.tool_name
                    ),
                    constraints: ActionConstraints {
                        working_directory: repository.to_path_buf(),
                        network: false,
                        timeout_seconds: self.timeout_seconds,
                        maximum_output_bytes: self.maximum_output_bytes,
                        allowed_write_globs: Vec::new(),
                        maximum_changed_files: 0,
                    },
                }
            }
        }
    }

    fn evaluate_file_mutation(
        &self,
        repository: &Path,
        relative_path: &Path,
        operation: &str,
    ) -> JudgmentDecision {
        if !is_safe_relative_path(relative_path) {
            return JudgmentDecision::Deny {
                reason: "file mutation path must be a normalized repository-relative path".into(),
            };
        }
        if !repository
            .components()
            .any(|component| component.as_os_str() == "worktrees")
            || !repository
                .components()
                .any(|component| component.as_os_str() == ".purrcode")
        {
            return JudgmentDecision::Deny {
                reason: "file mutation is only permitted inside an isolated PurrCode worktree"
                    .into(),
            };
        }
        let constraints = ActionConstraints {
            working_directory: repository.to_path_buf(),
            network: false,
            timeout_seconds: self.timeout_seconds,
            maximum_output_bytes: self.maximum_output_bytes,
            allowed_write_globs: vec![relative_path.to_string_lossy().into_owned()],
            maximum_changed_files: 1,
        };
        if self.auto_allow_worktree_writes {
            JudgmentDecision::AllowWithConstraints(constraints)
        } else {
            JudgmentDecision::RequireApproval {
                reason: format!(
                    "{operation} of `{}` requires human approval",
                    relative_path.display()
                ),
                constraints,
            }
        }
    }
}

impl SignedPolicyPack {
    pub fn load_verified(path: &Path, public_key_hex: &str) -> Result<Self, PolicyError> {
        let pack: Self = toml::from_str(&fs::read_to_string(path)?)?;
        pack.verify(public_key_hex)?;
        Ok(pack)
    }

    pub fn verify(&self, public_key_hex: &str) -> Result<(), PolicyError> {
        if self.version.trim().is_empty()
            || self.issuer.trim().is_empty()
            || self.expires_at <= Utc::now()
        {
            return Err(PolicyError::InvalidSignedPack(
                "version and issuer are required and expiration must be in the future".into(),
            ));
        }
        let known: BTreeSet<_> = [
            "read_only_programs",
            "approval_required_programs",
            "denied_argument_fragments",
            "timeout_seconds",
            "maximum_output_bytes",
            "auto_allow_worktree_writes",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        if !self.allowed_overrides.is_subset(&known) {
            return Err(PolicyError::InvalidSignedPack(
                "allowed_overrides contains an unknown policy field".into(),
            ));
        }
        let payload = self.payload_bytes()?;
        let actual_hash = blake3::hash(&payload).to_hex().to_string();
        if actual_hash != self.payload_hash {
            return Err(PolicyError::InvalidSignedPack(
                "payload hash does not match signed policy content".into(),
            ));
        }
        let key_bytes = hex::decode(public_key_hex)
            .map_err(|_| PolicyError::InvalidSignedPack("public key is not valid hex".into()))?;
        let key_array: [u8; 32] = key_bytes.try_into().map_err(|_| {
            PolicyError::InvalidSignedPack("Ed25519 public key must contain 32 bytes".into())
        })?;
        let key = VerifyingKey::from_bytes(&key_array)
            .map_err(|_| PolicyError::InvalidSignedPack("Ed25519 public key is invalid".into()))?;
        let signature_bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.signature)
            .map_err(|_| PolicyError::InvalidSignedPack("signature is not valid base64".into()))?;
        let signature = Signature::from_slice(&signature_bytes).map_err(|_| {
            PolicyError::InvalidSignedPack("signature must contain 64 bytes".into())
        })?;
        key.verify_strict(&payload, &signature)
            .map_err(|_| PolicyError::InvalidSignedPack("signature verification failed".into()))
    }

    fn payload_bytes(&self) -> Result<Vec<u8>, PolicyError> {
        Ok(serde_json::to_vec(&SignedPayload {
            version: &self.version,
            issuer: &self.issuer,
            expires_at: self.expires_at,
            allowed_overrides: &self.allowed_overrides,
            policy: &self.policy,
        })?)
    }

    fn restrict(&self, local: Policy) -> Policy {
        Policy {
            read_only_programs: field_override(
                &self.allowed_overrides,
                "read_only_programs",
                local.read_only_programs.clone(),
                self.policy
                    .read_only_programs
                    .intersection(&local.read_only_programs)
                    .cloned()
                    .collect(),
            ),
            approval_required_programs: field_override(
                &self.allowed_overrides,
                "approval_required_programs",
                local.approval_required_programs.clone(),
                self.policy
                    .approval_required_programs
                    .union(&local.approval_required_programs)
                    .cloned()
                    .collect(),
            ),
            denied_argument_fragments: field_override(
                &self.allowed_overrides,
                "denied_argument_fragments",
                local.denied_argument_fragments.clone(),
                self.policy
                    .denied_argument_fragments
                    .union(&local.denied_argument_fragments)
                    .cloned()
                    .collect(),
            ),
            timeout_seconds: field_override(
                &self.allowed_overrides,
                "timeout_seconds",
                local.timeout_seconds,
                self.policy.timeout_seconds.min(local.timeout_seconds),
            ),
            maximum_output_bytes: field_override(
                &self.allowed_overrides,
                "maximum_output_bytes",
                local.maximum_output_bytes,
                self.policy
                    .maximum_output_bytes
                    .min(local.maximum_output_bytes),
            ),
            auto_allow_worktree_writes: field_override(
                &self.allowed_overrides,
                "auto_allow_worktree_writes",
                local.auto_allow_worktree_writes,
                self.policy.auto_allow_worktree_writes && local.auto_allow_worktree_writes,
            ),
        }
    }
}

fn field_override<T>(allowed: &BTreeSet<String>, field: &str, local: T, restrictive: T) -> T {
    if allowed.contains(field) {
        local
    } else {
        restrictive
    }
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn default_read_only_programs() -> BTreeSet<String> {
    ["find", "git", "ls", "rg"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn unsafe_read_command(program: &str, arguments: &[String], repository: &Path) -> Option<String> {
    match program {
        "git" => {
            let Some(subcommand) = arguments.first() else {
                return Some("git requires an explicitly safe subcommand".into());
            };
            let safe = ["diff", "log", "ls-files", "rev-parse", "show", "status"];
            if safe.contains(&subcommand.as_str()) {
                None
            } else {
                Some(format!("git subcommand `{subcommand}` is not read-only"))
            }
        }
        "rg" if arguments
            .iter()
            .any(|arg| arg == "--pre" || arg.starts_with("--pre=")) =>
        {
            Some("rg preprocessor execution is denied".into())
        }
        "ls" => unsafe_ls(arguments, repository),
        "find" => unsafe_find(arguments, repository),
        _ => None,
    }
}

fn unsafe_ls(arguments: &[String], repository: &Path) -> Option<String> {
    if arguments.is_empty() {
        return None;
    }
    for argument in arguments {
        if let Some(flags) = argument.strip_prefix('-') {
            if argument == "--"
                || (!argument.starts_with("--")
                    && flags
                        .chars()
                        .all(|flag| matches!(flag, 'a' | 'A' | 'l' | '1')))
            {
                continue;
            }
            return Some(format!(
                "ls option `{argument}` is not an allowed bounded read"
            ));
        }
        if !path_is_within_repository(Path::new(argument), repository) {
            return Some("ls path must remain inside the authorized repository".into());
        }
    }
    None
}

fn unsafe_find(arguments: &[String], repository: &Path) -> Option<String> {
    let Some(root) = arguments.first() else {
        return Some("find requires an explicit repository root".into());
    };
    if Path::new(root) != repository {
        return Some("find root must exactly match the authorized repository".into());
    }
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "-maxdepth" | "--maxdepth" => {
                let Some(depth) = arguments.get(index + 1) else {
                    return Some("find maxdepth requires a value".into());
                };
                let Ok(depth) = depth.parse::<u8>() else {
                    return Some("find maxdepth must be a small positive integer".into());
                };
                if !(1..=5).contains(&depth) {
                    return Some("find maxdepth must be between 1 and 5".into());
                }
                index += 2;
            }
            "-not" => {
                if arguments.get(index + 1).map(String::as_str) != Some("-path")
                    || arguments
                        .get(index + 2)
                        .is_none_or(|pattern| !safe_find_exclusion(pattern))
                {
                    return Some(
                        "find only permits `-not -path` exclusions for repository subtrees".into(),
                    );
                }
                index += 3;
            }
            other => {
                return Some(format!(
                    "find expression `{other}` is not an allowed bounded repository read"
                ));
            }
        }
    }
    if !arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-maxdepth" | "--maxdepth"))
    {
        return Some("find requires maxdepth between 1 and 5".into());
    }
    None
}

fn safe_find_exclusion(pattern: &str) -> bool {
    pattern.starts_with("*/")
        && pattern.ends_with("/*")
        && pattern[2..pattern.len() - 2].chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn path_is_within_repository(path: &Path, repository: &Path) -> bool {
    if path.is_absolute() {
        path == repository || path.strip_prefix(repository).is_ok()
    } else {
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
    }
}
fn default_approval_programs() -> BTreeSet<String> {
    ["npm", "pnpm", "python", "python3"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
fn default_denied_fragments() -> BTreeSet<String> {
    ["reset --hard", "clean -fd", "push --force"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
fn default_timeout() -> u64 {
    120
}
fn default_output() -> usize {
    1_048_576
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("could not read policy: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not parse policy: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not encode signed policy payload: {0}")]
    Json(#[from] serde_json::Error),
    #[error("signed policy pack is invalid: {0}")]
    InvalidSignedPack(String),
}

pub fn resolve_policy_path(repository: &Path) -> PathBuf {
    repository.join("policies/default.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use purrcode_runtime_core::CommandAction;
    use std::collections::BTreeMap;

    fn action(args: &[&str]) -> ProposedAction {
        command("git", args)
    }

    fn command(program: &str, args: &[&str]) -> ProposedAction {
        ProposedAction::Command(CommandAction {
            program: program.into(),
            arguments: args.iter().map(|s| (*s).to_owned()).collect(),
            working_directory: "/repo".into(),
            environment: BTreeMap::new(),
        })
    }

    #[test]
    fn hard_deny_wins_over_allowlisted_program() {
        assert!(matches!(
            Policy::default().evaluate(&action(&["reset", "--hard"]), Path::new("/repo")),
            JudgmentDecision::Deny { .. }
        ));
    }

    fn signed_pack() -> (SignedPolicyPack, String) {
        let signing = SigningKey::from_bytes(&[7_u8; 32]);
        let mut policy = Policy::default();
        policy.denied_argument_fragments.insert("publish".into());
        policy.timeout_seconds = 30;
        let mut pack = SignedPolicyPack {
            version: "2026.1".into(),
            issuer: "example-security".into(),
            expires_at: Utc::now() + chrono::Duration::days(30),
            allowed_overrides: BTreeSet::new(),
            payload_hash: String::new(),
            signature: String::new(),
            policy,
        };
        let payload = pack.payload_bytes().unwrap();
        pack.payload_hash = blake3::hash(&payload).to_hex().to_string();
        pack.signature =
            base64::engine::general_purpose::STANDARD.encode(signing.sign(&payload).to_bytes());
        (pack, hex::encode(signing.verifying_key().to_bytes()))
    }

    #[test]
    fn signed_organization_policy_cannot_be_weakened_locally() {
        let (pack, key) = signed_pack();
        pack.verify(&key).unwrap();
        let mut local = Policy::default();
        local.denied_argument_fragments.clear();
        local.timeout_seconds = 600;
        local.auto_allow_worktree_writes = true;
        let effective = pack.restrict(local);
        assert!(effective.denied_argument_fragments.contains("publish"));
        assert_eq!(effective.timeout_seconds, 30);
        assert!(!effective.auto_allow_worktree_writes);
    }

    #[test]
    fn signed_policy_tampering_and_wrong_keys_fail_closed() {
        let (mut pack, key) = signed_pack();
        pack.policy.timeout_seconds = 999;
        assert!(pack.verify(&key).is_err());
        let other = SigningKey::from_bytes(&[8_u8; 32]);
        let (pack, _) = signed_pack();
        assert!(pack
            .verify(&hex::encode(other.verifying_key().to_bytes()))
            .is_err());
    }

    #[test]
    fn mutating_git_subcommand_is_denied() {
        assert!(matches!(
            Policy::default().evaluate(&action(&["commit", "-m", "no"]), Path::new("/repo")),
            JudgmentDecision::Deny { .. }
        ));
    }

    #[test]
    fn bounded_repository_listings_are_allowed_without_approval() {
        for proposed in [
            command("ls", &["-la", "/repo"]),
            command(
                "find",
                &[
                    "/repo",
                    "-maxdepth",
                    "3",
                    "-not",
                    "-path",
                    "*/node_modules/*",
                    "-not",
                    "-path",
                    "*/.git/*",
                ],
            ),
        ] {
            assert!(matches!(
                Policy::default().evaluate(&proposed, Path::new("/repo")),
                JudgmentDecision::AllowWithConstraints(_)
            ));
        }
    }

    #[test]
    fn unsafe_repository_listings_are_denied() {
        for proposed in [
            command("ls", &["-R", "/repo"]),
            command("ls", &["/etc"]),
            command("ls", &["../outside"]),
            command("find", &["/repo", "-maxdepth", "99"]),
            command("find", &["/tmp", "-maxdepth", "2"]),
            command("find", &["/repo", "-maxdepth", "2", "-exec", "sh"]),
            command("find", &["/repo", "-type", "d"]),
            command(
                "find",
                &["/repo", "-maxdepth", "2", "-not", "-path", "/tmp/*"],
            ),
        ] {
            assert!(matches!(
                Policy::default().evaluate(&proposed, Path::new("/repo")),
                JudgmentDecision::Deny { .. }
            ));
        }
    }

    #[test]
    fn executable_path_cannot_impersonate_allowlisted_program() {
        let mut command = match action(&["status"]) {
            ProposedAction::Command(command) => command,
            other => panic!("test helper returned unexpected action: {other:?}"),
        };
        command.program = "/tmp/git".into();
        assert!(matches!(
            Policy::default().evaluate(&ProposedAction::Command(command), Path::new("/repo")),
            JudgmentDecision::Deny { .. }
        ));
    }

    #[test]
    fn external_tools_always_require_human_approval() {
        let action = ProposedAction::ExternalTool(purrcode_runtime_core::ExternalToolAction {
            server_id: "docs".into(),
            tool_name: "search".into(),
            arguments: serde_json::json!({"query":"safe"}),
            working_directory: "/repo".into(),
        });
        assert!(matches!(
            Policy::default().evaluate(&action, Path::new("/repo")),
            JudgmentDecision::RequireApproval { .. }
        ));
    }
}
