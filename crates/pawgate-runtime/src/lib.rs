//! Deterministic, model-independent pre-execution policy.

use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use purrcode_runtime_core::{
    ActionConstraints, ApprovalPolicy, FilesystemScope, JudgmentDecision, NetworkScope,
    ProposedAction, SideEffectClass, ToolCeiling, ToolDescriptor,
};
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

    /// v1.3 precedence. Three tiers instead of two:
    ///   `Policy::default()`              ← the floor
    ///     ↓ restrict_local(base, user)   ← `~/.purrcode/policy.toml` (or `config.toml [policy]`)
    ///     ↓ restrict_local(base, project)← `<repo>/policies/default.toml` — RESTRICTED, not replacing
    ///     ↓ SignedPolicyPack::restrict   ← org pack, unchanged, still the outermost authority
    ///
    /// The project tier gets no delegation: `restrict_local` has an empty
    /// `allowed_overrides`, so a project file can never widen the user's
    /// machine-level bounds.
    pub fn load_effective_v3(
        user_policy: Option<&Path>,
        repository_policy: Option<&Path>,
        organization: Option<(&Path, &str)>,
    ) -> Result<Self, PolicyError> {
        let mut base = Policy::default();
        if let Some(user) = user_policy.filter(|p| p.exists()) {
            base = restrict_local(base, Self::load(user)?);
        }
        if let Some(project) = repository_policy.filter(|p| p.exists()) {
            base = restrict_local(base, Self::load(project)?);
        }
        if let Some((pack, key)) = organization {
            let pack = SignedPolicyPack::load_verified(pack, key)?;
            base = pack.restrict(base);
        }
        Ok(base)
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
            ProposedAction::RepositoryRead(read) => {
                if repository.as_os_str().is_empty() {
                    return JudgmentDecision::Deny {
                        reason: "repository read requires a bounded worktree path".into(),
                    };
                }
                if let Some(reason) = unsafe_repository_read(read, repository) {
                    return JudgmentDecision::Deny { reason };
                }
                JudgmentDecision::AllowWithConstraints(ActionConstraints {
                    working_directory: repository.to_path_buf(),
                    network: false,
                    timeout_seconds: self.timeout_seconds,
                    maximum_output_bytes: self.maximum_output_bytes,
                    allowed_write_globs: Vec::new(),
                    maximum_changed_files: 0,
                })
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
            // v1.3: registry-admitted tool invocations are judged by
            // `Policy::evaluate_tool` against their descriptor. This arm is a
            // conservative fallback so the old `evaluate` path can never
            // auto-allow a registry tool; PR2 replaces it with the real
            // descriptor-driven decision.
            ProposedAction::Tool(invocation) => {
                if invocation.working_directory != repository {
                    return JudgmentDecision::Deny {
                        reason: "tool working directory does not match the session worktree".into(),
                    };
                }
                JudgmentDecision::RequireApproval {
                    reason: format!(
                        "tool `{}` requires explicit authorization",
                        invocation.tool_id
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

    /// Provider-blind judgment of a registry-admitted tool invocation (v1.3).
    ///
    /// Reads the four descriptor fields that PR1's lattice already restricted
    /// against the workspace ceiling:
    /// - `schema` is validated FIRST, so a malformed call is denied before any
    ///   authorization could be minted (the failure precedes authorization);
    /// - `side_effect_class` selects the read fast-path vs approval;
    /// - `network_scope` becomes `ActionConstraints.network`;
    /// - `filesystem_scope` becomes the write globs / changed-file budget;
    /// - `approval_policy` selects the decision class.
    ///
    /// This is the single decision point for EVERY provider. There is no
    /// `if provider == Mcp` here — the descriptor carries everything PawGate
    /// needs.
    pub fn evaluate_tool(
        &self,
        action: &ProposedAction,
        descriptor: &ToolDescriptor,
        repository: &Path,
    ) -> JudgmentDecision {
        // 1. Working-directory containment: a tool runs against the session
        // worktree, never against an arbitrary path.
        let working_directory = match action {
            ProposedAction::Tool(invocation) => &invocation.working_directory,
            ProposedAction::ExternalTool(external) => &external.working_directory,
            _ => {
                return JudgmentDecision::Deny {
                    reason: "evaluate_tool only judges Tool/ExternalTool invocations".into(),
                };
            }
        };
        if working_directory != repository {
            return JudgmentDecision::Deny {
                reason: "tool working directory does not match the session worktree".into(),
            };
        }

        // 2. Argument schema validation BEFORE any allow/approval. A malformed
        // call must be denied, never routed to a human for approval that the
        // sandbox would then fail.
        let arguments = match action {
            ProposedAction::Tool(invocation) => &invocation.arguments,
            ProposedAction::ExternalTool(external) => &external.arguments,
            _ => unreachable!("guarded above"),
        };
        if let Err(reason) = validate_arguments(descriptor.schema(), arguments) {
            return JudgmentDecision::Deny { reason };
        }

        // 3. Forbidden by the ceiling (PR1 admitted it Forbidden). This is a
        // hard deny that no permission mode may override.
        if descriptor.approval_policy() == ApprovalPolicy::Forbidden {
            return JudgmentDecision::Deny {
                reason: format!("tool `{}` is forbidden in this workspace", descriptor.id()),
            };
        }

        // 4. Build the constraints envelope from the descriptor.
        let (network, filesystem) = descriptor_scope(descriptor);

        // 5. Decide.
        match descriptor.approval_policy() {
            ApprovalPolicy::PreAuthorized => {
                JudgmentDecision::AllowWithConstraints(constraints_for(
                    repository,
                    self.timeout_seconds,
                    self.maximum_output_bytes,
                    network,
                    filesystem,
                ))
            }
            ApprovalPolicy::ByClass => {
                if descriptor.side_effect_class() == SideEffectClass::Read
                    && matches!(descriptor.network_scope(), NetworkScope::None)
                {
                    JudgmentDecision::AllowWithConstraints(constraints_for(
                        repository,
                        self.timeout_seconds,
                        self.maximum_output_bytes,
                        false,
                        Vec::new(),
                    ))
                } else {
                    JudgmentDecision::RequireApproval {
                        reason: format!(
                            "tool `{}` may mutate repository or external state",
                            descriptor.id()
                        ),
                        constraints: constraints_for(
                            repository,
                            self.timeout_seconds,
                            self.maximum_output_bytes,
                            network,
                            filesystem,
                        ),
                    }
                }
            }
            ApprovalPolicy::AlwaysAsk => JudgmentDecision::RequireApproval {
                reason: format!("tool `{}` requires explicit authorization", descriptor.id()),
                constraints: constraints_for(
                    repository,
                    self.timeout_seconds,
                    self.maximum_output_bytes,
                    network,
                    filesystem,
                ),
            },
            ApprovalPolicy::Forbidden => unreachable!("handled above"),
        }
    }

    /// The workspace ceiling derived from this policy (v1.3 §9.2). Project
    /// config never participates in producing this; it is only ever restricted
    /// against it.
    pub fn tool_ceiling(&self, _repository: &Path) -> ToolCeiling {
        let maximum_filesystem = if self.auto_allow_worktree_writes {
            FilesystemScope::Worktree {
                write_globs: vec!["**".into()],
                maximum_changed_files: usize::MAX,
            }
        } else {
            FilesystemScope::WorktreeRead
        };
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::None,
            maximum_filesystem,
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::new(),
        }
    }

    /// Judge one action proposed by a **delegated worker** (v1.4 §5.3, §9
    /// "Scope Escape").
    ///
    /// The delegation's scope is applied *before* the ordinary policy, and the
    /// resulting constraints are intersected with it afterwards. Both halves
    /// matter: the pre-check is what turns "worker delegated `src/auth/**`
    /// writes `src/payments/billing.rs`" into a PawGate deny rather than a
    /// deny discovered later at result validation, and the post-intersection is
    /// what stops a policy that would have allowed a wider write from widening
    /// what the delegation authorized.
    ///
    /// This lives on `Policy` rather than beside the delegation runtime on
    /// purpose. PawGate is the authority for "may this action run"; a second
    /// gate elsewhere would be a second answer to the same question, and the
    /// two would eventually disagree.
    pub fn evaluate_delegated(
        &self,
        action: &ProposedAction,
        worktree: &Path,
        delegation: &purrcode_runtime_core::delegation::Delegation,
    ) -> JudgmentDecision {
        use purrcode_runtime_core::delegation::WorkspaceAccess;

        // 1. A read-only worker may not mutate anything, whatever the policy
        //    would have said.
        let mutated = mutated_path(action);
        if let Some(path) = mutated {
            if delegation.access() == WorkspaceAccess::ReadOnly {
                return JudgmentDecision::Deny {
                    reason: format!(
                        "delegation {} is read-only; it cannot modify `{}`",
                        delegation.id().short(),
                        path.display()
                    ),
                };
            }
            // 2. The delegated path scope. An empty allowlist permits nothing.
            if !delegation.permits_path(path) {
                return JudgmentDecision::Deny {
                    reason: format!(
                        "`{}` is outside the paths delegated to {} ({})",
                        path.display(),
                        delegation.id().short(),
                        delegation
                            .allowed_paths()
                            .iter()
                            .map(|pattern| pattern.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
            }
        }

        // 3. A worker whose effective ceiling forbids this class of effect is
        //    denied before the policy is asked.
        let ceiling = delegation.effective_ceiling();
        if side_effect_of(action) > ceiling.maximum_side_effect {
            return JudgmentDecision::Deny {
                reason: format!(
                    "delegation {} may not perform a {:?} action",
                    delegation.id().short(),
                    side_effect_of(action)
                ),
            };
        }

        // 4. Ordinary policy, then narrow whatever it granted.
        match self.evaluate(action, worktree) {
            JudgmentDecision::AllowWithConstraints(constraints) => {
                JudgmentDecision::AllowWithConstraints(narrow_constraints(constraints, delegation))
            }
            JudgmentDecision::RequireApproval {
                reason,
                constraints,
            } => JudgmentDecision::RequireApproval {
                reason,
                constraints: narrow_constraints(constraints, delegation),
            },
            deny => deny,
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

/// The repository-relative path an action would modify, if any.
fn mutated_path(action: &ProposedAction) -> Option<&Path> {
    match action {
        ProposedAction::WriteFile(write) => Some(&write.path),
        ProposedAction::DeleteFile(delete) => Some(&delete.path),
        // A command's effects are not enumerable from its arguments, so its
        // scope is bounded by the constraints it runs under, not by a path
        // check here.
        _ => None,
    }
}

/// The side-effect class an action belongs to, for the delegation ceiling
/// check. Deliberately pessimistic: anything that spawns a process or reaches
/// an external tool counts as `Execute` even when it might turn out to be
/// harmless.
fn side_effect_of(action: &ProposedAction) -> SideEffectClass {
    match action {
        ProposedAction::RepositoryRead(_) => SideEffectClass::Read,
        ProposedAction::WriteFile(_) => SideEffectClass::Write,
        ProposedAction::DeleteFile(_) => SideEffectClass::Destructive,
        ProposedAction::Command(_) | ProposedAction::ExternalTool(_) | ProposedAction::Tool(_) => {
            SideEffectClass::Execute
        }
    }
}

/// Intersect granted constraints with what the delegation authorized.
///
/// Write globs are replaced by the delegation's own patterns when the policy
/// granted something wider, and the changed-file budget takes the minimum. This
/// can only narrow: there is no branch that adds a glob the delegation did not
/// already carry.
fn narrow_constraints(
    constraints: ActionConstraints,
    delegation: &purrcode_runtime_core::delegation::Delegation,
) -> ActionConstraints {
    let delegated: Vec<String> = delegation
        .allowed_paths()
        .iter()
        .map(|pattern| pattern.as_str().to_owned())
        .collect();
    let allowed_write_globs: Vec<String> = constraints
        .allowed_write_globs
        .into_iter()
        .filter(|granted| {
            delegated
                .iter()
                .any(|permitted| purrcode_runtime_core::tool::glob_covers(permitted, granted))
        })
        .collect();
    ActionConstraints {
        maximum_changed_files: constraints
            .maximum_changed_files
            .min(delegation.budget().maximum_changed_files),
        allowed_write_globs,
        ..constraints
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

    /// Restrict `local` against this signed pack's policy. `allowed_overrides`
    /// (which is inside the signed payload, hence tamper-evident) may delegate
    /// a field to the local value verbatim; every other field takes the
    /// restrictive lattice operator.
    pub fn restrict(&self, local: Policy) -> Policy {
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

/// Restrict a base policy against a proposal using the same lattice as
/// `SignedPolicyPack::restrict`, but with **no** `allowed_overrides` escape
/// hatch: every field takes the restrictive operator, so the proposal can only
/// narrow the base. This is the v1.3 non-signed tier operator — the project
/// tier gets no delegation because (unlike a signed org pack) a project file's
/// delegation list would not be tamper-evident.
///
/// Lattice: allow-sets intersect, deny-sets union, numeric budgets take the
/// min, permission booleans AND.
pub fn restrict_local(base: Policy, proposal: Policy) -> Policy {
    // A synthesized pack with an empty allowlist selects the restrictive
    // branch of `field_override` for every field, with `proposal` as the
    // restrictive source.
    let pack = SignedPolicyPack {
        version: String::new(),
        issuer: String::new(),
        expires_at: chrono::Utc::now(),
        allowed_overrides: BTreeSet::new(),
        payload_hash: String::new(),
        signature: String::new(),
        policy: proposal,
    };
    pack.restrict(base)
}

fn field_override<T>(allowed: &BTreeSet<String>, field: &str, local: T, restrictive: T) -> T {
    if allowed.contains(field) {
        local
    } else {
        restrictive
    }
}

/// Validate `arguments` against a JSON-Schema `schema`. Returns a deny reason
/// on failure. This runs BEFORE any authorization is minted, so a malformed
/// call is denied rather than approved-then-failed.
fn validate_arguments(
    schema: &serde_json::Value,
    arguments: &serde_json::Value,
) -> Result<(), String> {
    // The schema is an unrestricted JSON object; a "type":"object" contract
    // with properties/required is the descriptor's declaration. We enforce
    // the declared `required` array and, when a `type` is declared at the top,
    // that the arguments match it. Full JSON-Schema evaluation is not
    // available here (no jsonschema dependency); the descriptor lattice is the
    // authoritative bound and the sandbox re-checks constraints at execution.
    let Some(obj) = schema.as_object() else {
        return Ok(()); // no object contract declared
    };
    if let Some(required) = obj.get("required").and_then(serde_json::Value::as_array) {
        let args = arguments
            .as_object()
            .ok_or_else(|| "arguments must be a JSON object".to_string())?;
        for key in required {
            let Some(key) = key.as_str() else { continue };
            if !args.contains_key(key) {
                return Err(format!("missing required argument `{key}`"));
            }
        }
    }
    if let Some(expected) = obj.get("type").and_then(serde_json::Value::as_str) {
        let actual = match arguments {
            serde_json::Value::Object(_) => "object",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::Bool(_) => "boolean",
            serde_json::Value::Null => "null",
        };
        if expected != actual {
            return Err(format!("arguments must be a JSON {expected}, got {actual}"));
        }
    }
    Ok(())
}

/// Map a descriptor's network/filesystem scope onto the two constraint fields
/// that `ActionConstraints` carries. Returns `(network, allowed_write_globs)`.
fn descriptor_scope(descriptor: &ToolDescriptor) -> (bool, Vec<String>) {
    let network = match descriptor.network_scope() {
        NetworkScope::Any | NetworkScope::Hosts { .. } => true,
        NetworkScope::None => false,
    };
    let write_globs = match descriptor.filesystem_scope() {
        FilesystemScope::Worktree {
            write_globs,
            maximum_changed_files,
        } if *maximum_changed_files > 0 && !write_globs.is_empty() => write_globs.clone(),
        _ => Vec::new(),
    };
    (network, write_globs)
}

fn constraints_for(
    repository: &Path,
    timeout_seconds: u64,
    maximum_output_bytes: usize,
    network: bool,
    allowed_write_globs: Vec<String>,
) -> ActionConstraints {
    ActionConstraints {
        working_directory: repository.to_path_buf(),
        network,
        timeout_seconds,
        maximum_output_bytes,
        maximum_changed_files: if allowed_write_globs.is_empty() {
            0
        } else {
            allowed_write_globs.len()
        },
        allowed_write_globs,
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

fn unsafe_repository_read(
    read: &purrcode_runtime_core::RepositoryReadAction,
    repository: &Path,
) -> Option<String> {
    use purrcode_runtime_core::{RepositoryReadAction, canonicalize_repository_path};
    match read {
        RepositoryReadAction::GitStatus | RepositoryReadAction::GitLog { .. } => None,
        RepositoryReadAction::GitRevParse { revision } => {
            if revision.is_empty()
                || revision.starts_with('-')
                || revision.chars().any(char::is_whitespace)
                || revision.contains("..")
            {
                Some("git rev-parse revision must reference one safe revision".into())
            } else {
                None
            }
        }
        RepositoryReadAction::GitLsFiles { pathspec } => {
            // Gap 4: GitLsFiles pathspec previously bypassed path validation.
            // Treat each pathspec entry as a repository-relative path and pass
            // it through the same containment check applied to other variants.
            let specs: Vec<PathBuf> = pathspec.to_vec();
            unsafe_paths(&specs, repository, "git ls-files")
        }
        RepositoryReadAction::GitDiff { paths } => unsafe_paths(paths, repository, "git diff"),
        RepositoryReadAction::GitShow { revision, path } => {
            if revision.is_empty()
                || revision.chars().any(char::is_whitespace)
                || revision.contains("..")
            {
                return Some("git_show revision must reference a single revision".into());
            }
            if path.as_os_str().is_empty() {
                None
            } else {
                unsafe_paths(std::slice::from_ref(path), repository, "git show")
            }
        }
        RepositoryReadAction::RepositoryGrep {
            pattern,
            paths,
            case_insensitive: _,
            max_results,
            max_bytes,
        } => {
            if pattern.is_empty() || pattern.contains('\n') {
                return Some("repository_grep pattern must not be empty or multi-line".into());
            }
            if let Err(reason) = read.validate_bounds() {
                return Some(reason.to_string());
            }
            if *max_results == 0 {
                return Some("repository_grep max_results must be greater than zero".into());
            }
            if *max_bytes == 0 {
                return Some("repository_grep max_bytes must be greater than zero".into());
            }
            unsafe_paths(paths, repository, "repository grep")
        }
        RepositoryReadAction::Find {
            paths,
            max_depth,
            max_entries,
        } => {
            if let Err(reason) = read.validate_bounds() {
                return Some(reason.to_string());
            }
            if *max_depth == 0 {
                return Some("find max_depth must be between 1 and 5".into());
            }
            if *max_depth > purrcode_runtime_core::DEFAULT_FIND_MAX_DEPTH {
                return Some(format!(
                    "find max_depth must be at most {}",
                    purrcode_runtime_core::DEFAULT_FIND_MAX_DEPTH
                ));
            }
            if *max_entries == 0 {
                return Some("find max_entries must be greater than zero".into());
            }
            unsafe_paths(paths, repository, "find")
        }
        RepositoryReadAction::List { paths, max_entries } => {
            if *max_entries == 0 {
                return Some("list max_entries must be greater than zero".into());
            }
            unsafe_paths(paths, repository, "list")
        }
        RepositoryReadAction::ReadFile { path, max_bytes } => {
            if *max_bytes == 0 {
                return Some("read_file max_bytes must be greater than zero".into());
            }
            if *max_bytes > purrcode_runtime_core::MAX_READ_FILE_BYTES {
                return Some(format!(
                    "read_file max_bytes must be at most {}",
                    purrcode_runtime_core::MAX_READ_FILE_BYTES
                ));
            }
            let canonical = canonicalize_repository_path(path)?;
            if canonical.as_os_str().is_empty() {
                return Some("read_file path must name a file, not the repository root".into());
            }
            unsafe_paths(std::slice::from_ref(path), repository, "read_file")
        }
    }
}

fn unsafe_paths(paths: &[PathBuf], repository: &Path, verb: &str) -> Option<String> {
    use std::path::Component;
    for path in paths {
        // The canonical repository root is represented by an empty relative
        // path. Single-file reads reject it separately above.
        if path.as_os_str().is_empty() {
            continue;
        }
        if path.is_absolute() {
            return Some(format!("{verb} path must be repository-relative"));
        }
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Some(format!("{verb} path may not traverse out of the worktree"));
        }
        if path == Path::new("..") || path.starts_with("../") {
            return Some(format!("{verb} path may not escape the worktree"));
        }
        let joined = repository.join(path);
        if !joined.starts_with(repository) {
            return Some(format!("{verb} path resolves outside the worktree"));
        }
        // Resolve symlinks for existing paths to detect symlink-based escapes.
        // Non-existent paths are not checked here because they will be created
        // (and execution-level containment still applies), but if a symlink
        // chain redirects outside the repository it must be rejected.
        if let Ok(canonical) = joined.canonicalize()
            && !canonical.starts_with(repository)
        {
            return Some(format!(
                "{verb} path follows a symlink outside the worktree: {}",
                path.display()
            ));
        }
    }
    None
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

/// The machine-level user policy tier: `~/.purrcode/policy.toml`. This is the
/// "user" tier in `load_effective_v3`'s Default → User → Project → Org order.
/// It is deliberately NOT loadable from a repository, so a repository can never
/// widen the machine's bounds — only restrict them.
pub fn resolve_user_policy_path() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".purrcode/policy.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use purrcode_runtime_core::{
        CapabilityRegistry, CommandAction, ToolCeiling, ToolDescriptorProposal, ToolId,
        ToolInvocation, ToolProvider, builtin_native_proposals,
    };
    use std::collections::BTreeMap;

    #[cfg(not(windows))]
    const TEST_REPOSITORY: &str = "/repo";
    #[cfg(windows)]
    const TEST_REPOSITORY: &str = r"C:\repo";

    fn action(args: &[&str]) -> ProposedAction {
        command("git", args)
    }

    fn command(program: &str, args: &[&str]) -> ProposedAction {
        ProposedAction::Command(CommandAction {
            program: program.into(),
            arguments: args.iter().map(|s| (*s).to_owned()).collect(),
            working_directory: TEST_REPOSITORY.into(),
            environment: BTreeMap::new(),
        })
    }

    /// Admit a proposal through the ONLY mint and return the descriptor.
    fn admit(proposal: ToolDescriptorProposal) -> ToolDescriptor {
        let mut registry = CapabilityRegistry::new();
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::PreAuthorized,
            denied_tool_ids: BTreeSet::new(),
        };
        registry.admit_tool(proposal, &ceiling).clone()
    }

    fn native_descriptor(name: &str) -> ToolDescriptor {
        admit(
            builtin_native_proposals()
                .into_iter()
                .find(|p| p.id == ToolId::native(name))
                .unwrap_or_else(|| panic!("missing builtin {name}")),
        )
    }

    #[test]
    fn known_overrides_cover_every_policy_field() {
        // The six-string allowlist at the top of `verify` is maintained
        // separately from the struct; assert they never drift apart.
        let value = serde_json::to_value(Policy::default()).unwrap();
        let fields: BTreeSet<String> = value.as_object().unwrap().keys().cloned().collect();
        let known: BTreeSet<String> = [
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
        assert_eq!(
            fields, known,
            "the signed-pack `allowed_overrides` allowlist must cover every Policy field"
        );
    }

    #[test]
    fn evaluate_tool_native_read_equivalent_to_evaluate() {
        // Zero-behaviour-change proof: the descriptor-driven path for a native
        // read must match the classic `evaluate` path exactly.
        let policy = Policy::default();
        let repository = Path::new(TEST_REPOSITORY);

        for name in [
            "git_status",
            "git_rev_parse",
            "git_log",
            "git_diff",
            "git_show",
            "git_ls_files",
            "repository_grep",
            "find",
            "list",
            "read_file",
        ] {
            let descriptor = native_descriptor(name);
            // A representative action for this read kind. The action payload is
            // incidental — the descriptor carries the decision fields.
            let action = ProposedAction::Tool(ToolInvocation {
                tool_id: ToolId::native(name),
                arguments: serde_json::json!({}),
                working_directory: repository.to_path_buf(),
                descriptor_digest: "abc".into(),
            });
            let via_tool = policy.evaluate_tool(&action, &descriptor, repository);
            assert!(
                matches!(via_tool, JudgmentDecision::AllowWithConstraints(_)),
                "{name}: a native read via descriptor should be allowed with constraints"
            );
            assert!(
                !matches!(via_tool, JudgmentDecision::RequireApproval { .. }),
                "{name}: a native read must not require approval"
            );
        }
    }

    #[test]
    fn evaluate_tool_malformed_arguments_are_denied_not_approved() {
        let policy = Policy::default();
        let repository = Path::new(TEST_REPOSITORY);
        let descriptor = admit(ToolDescriptorProposal {
            id: ToolId::mcp("github", "create_issue"),
            provider: ToolProvider::Mcp,
            display_name: "create_issue".into(),
            description: "create a github issue".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"]
            }),
            capabilities: BTreeSet::new(),
            side_effect_class: SideEffectClass::Write,
            network_scope: NetworkScope::Any,
            filesystem_scope: FilesystemScope::WorktreeRead,
            approval_policy: ApprovalPolicy::PreAuthorized,
            origin: purrcode_runtime_core::DescriptorOrigin::RemoteDiscovery,
        });
        let action = ProposedAction::Tool(ToolInvocation {
            tool_id: descriptor.id().clone(),
            arguments: serde_json::json!({ "body": "missing title" }),
            working_directory: repository.to_path_buf(),
            descriptor_digest: "abc".into(),
        });
        let decision = policy.evaluate_tool(&action, &descriptor, repository);
        assert!(
            matches!(decision, JudgmentDecision::Deny { .. }),
            "a malformed call must be denied BEFORE it could be authorized or approved"
        );
    }

    #[test]
    fn evaluate_tool_networked_preauthorized_carries_network_constraint() {
        let policy = Policy::default();
        let repository = Path::new(TEST_REPOSITORY);
        let descriptor = admit(ToolDescriptorProposal {
            id: ToolId::mcp("github", "get_issue"),
            provider: ToolProvider::Mcp,
            display_name: "get_issue".into(),
            description: "fetch a github issue".into(),
            schema: serde_json::json!({ "type": "object" }),
            capabilities: BTreeSet::new(),
            side_effect_class: SideEffectClass::Read,
            network_scope: NetworkScope::Any,
            filesystem_scope: FilesystemScope::WorktreeRead,
            approval_policy: ApprovalPolicy::PreAuthorized,
            origin: purrcode_runtime_core::DescriptorOrigin::RemoteDiscovery,
        });
        let action = ProposedAction::Tool(ToolInvocation {
            tool_id: descriptor.id().clone(),
            arguments: serde_json::json!({}),
            working_directory: repository.to_path_buf(),
            descriptor_digest: "abc".into(),
        });
        let JudgmentDecision::AllowWithConstraints(constraints) =
            policy.evaluate_tool(&action, &descriptor, repository)
        else {
            panic!("PreAuthorized networked read should be allowed with constraints");
        };
        assert!(
            constraints.network,
            "a Hosts/Any network scope must surface as network: true in the envelope"
        );
    }

    #[test]
    fn restrict_local_only_narrows() {
        let base = Policy::default();
        let looser = Policy {
            auto_allow_worktree_writes: true,
            ..Policy::default()
        };
        // Proposal can only narrow: a looser proposal leaves the base intact.
        let narrowed = restrict_local(base.clone(), looser.clone());
        assert!(!narrowed.auto_allow_worktree_writes);
        // A stricter proposal narrows.
        let strict = Policy {
            timeout_seconds: 5,
            ..Policy::default()
        };
        let narrowed = restrict_local(base, strict);
        assert_eq!(narrowed.timeout_seconds, 5);
    }

    #[test]
    fn hard_deny_wins_over_allowlisted_program() {
        assert!(matches!(
            Policy::default().evaluate(&action(&["reset", "--hard"]), Path::new(TEST_REPOSITORY)),
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
        assert!(
            pack.verify(&hex::encode(other.verifying_key().to_bytes()))
                .is_err()
        );
    }

    #[test]
    fn mutating_git_subcommand_is_denied() {
        assert!(matches!(
            Policy::default()
                .evaluate(&action(&["commit", "-m", "no"]), Path::new(TEST_REPOSITORY)),
            JudgmentDecision::Deny { .. }
        ));
    }

    #[test]
    fn bounded_repository_listings_are_allowed_without_approval() {
        for proposed in [
            command("ls", &["-la", TEST_REPOSITORY]),
            command(
                "find",
                &[
                    TEST_REPOSITORY,
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
                Policy::default().evaluate(&proposed, Path::new(TEST_REPOSITORY)),
                JudgmentDecision::AllowWithConstraints(_)
            ));
        }
    }

    #[test]
    fn unsafe_repository_listings_are_denied() {
        for proposed in [
            command("ls", &["-R", TEST_REPOSITORY]),
            command("ls", &["/etc"]),
            command("ls", &["../outside"]),
            command("find", &[TEST_REPOSITORY, "-maxdepth", "99"]),
            command("find", &["/tmp", "-maxdepth", "2"]),
            command("find", &[TEST_REPOSITORY, "-maxdepth", "2", "-exec", "sh"]),
            command("find", &[TEST_REPOSITORY, "-type", "d"]),
            command(
                "find",
                &[TEST_REPOSITORY, "-maxdepth", "2", "-not", "-path", "/tmp/*"],
            ),
        ] {
            assert!(matches!(
                Policy::default().evaluate(&proposed, Path::new(TEST_REPOSITORY)),
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
            Policy::default().evaluate(
                &ProposedAction::Command(command),
                Path::new(TEST_REPOSITORY)
            ),
            JudgmentDecision::Deny { .. }
        ));
    }

    #[test]
    fn external_tools_always_require_human_approval() {
        let action = ProposedAction::ExternalTool(purrcode_runtime_core::ExternalToolAction {
            server_id: "docs".into(),
            tool_name: "search".into(),
            arguments: serde_json::json!({"query":"safe"}),
            working_directory: TEST_REPOSITORY.into(),
        });
        assert!(matches!(
            Policy::default().evaluate(&action, Path::new(TEST_REPOSITORY)),
            JudgmentDecision::RequireApproval { .. }
        ));
    }

    #[test]
    fn typed_repository_reads_with_relative_or_canonical_root_are_allowed() {
        use purrcode_runtime_core::RepositoryReadAction;
        let proposed = [
            RepositoryReadAction::GitStatus,
            RepositoryReadAction::GitLog {
                max_count: Some(5),
                oneline: true,
            },
            RepositoryReadAction::GitLsFiles { pathspec: vec![] },
            RepositoryReadAction::Find {
                paths: vec![PathBuf::from(".")],
                max_depth: 3,
                max_entries: 64,
            },
            RepositoryReadAction::Find {
                paths: vec![PathBuf::from("./")],
                max_depth: 3,
                max_entries: 64,
            },
            RepositoryReadAction::Find {
                paths: vec![PathBuf::from("src")],
                max_depth: 3,
                max_entries: 64,
            },
            RepositoryReadAction::List {
                paths: vec![PathBuf::from(".")],
                max_entries: 32,
            },
            RepositoryReadAction::List {
                paths: vec![PathBuf::from("./src")],
                max_entries: 32,
            },
            RepositoryReadAction::RepositoryGrep {
                pattern: "TODO".into(),
                paths: vec![PathBuf::from("src")],
                case_insensitive: false,
                max_results: 64,
                max_bytes: 4096,
            },
        ];
        for read in proposed {
            let action = ProposedAction::RepositoryRead(read);
            assert!(
                matches!(
                    Policy::default().evaluate(&action, Path::new(TEST_REPOSITORY)),
                    JudgmentDecision::AllowWithConstraints(_)
                ),
                "expected typed repository read to be allowed: {action:?}"
            );
        }
    }

    #[test]
    fn typed_repository_reads_with_unsafe_paths_are_denied() {
        use purrcode_runtime_core::RepositoryReadAction;
        let proposed = [
            RepositoryReadAction::Find {
                paths: vec![PathBuf::from("..")],
                max_depth: 3,
                max_entries: 64,
            },
            RepositoryReadAction::Find {
                paths: vec![PathBuf::from("../outside")],
                max_depth: 3,
                max_entries: 64,
            },
            RepositoryReadAction::Find {
                paths: vec![PathBuf::from("/etc")],
                max_depth: 3,
                max_entries: 64,
            },
            RepositoryReadAction::List {
                paths: vec![PathBuf::from("/etc")],
                max_entries: 32,
            },
            RepositoryReadAction::GitDiff {
                paths: vec![PathBuf::from("../sibling.txt")],
            },
            RepositoryReadAction::GitShow {
                revision: "HEAD^".into(),
                path: PathBuf::from("../outside.txt"),
            },
            RepositoryReadAction::RepositoryGrep {
                pattern: "TODO\nmore".into(),
                paths: vec![PathBuf::from("src")],
                case_insensitive: false,
                max_results: 64,
                max_bytes: 4096,
            },
            RepositoryReadAction::GitShow {
                revision: "HEAD bad".into(),
                path: PathBuf::from("src/file.rs"),
            },
        ];
        for read in proposed {
            let action = ProposedAction::RepositoryRead(read);
            assert!(
                matches!(
                    Policy::default().evaluate(&action, Path::new(TEST_REPOSITORY)),
                    JudgmentDecision::Deny { .. }
                ),
                "expected typed repository read to be denied: {action:?}"
            );
        }
    }

    #[test]
    fn typed_repository_read_with_empty_root_is_denied() {
        use purrcode_runtime_core::RepositoryReadAction;
        let action = ProposedAction::RepositoryRead(RepositoryReadAction::Find {
            paths: vec![PathBuf::new()],
            max_depth: 3,
            max_entries: 64,
        });
        assert!(matches!(
            Policy::default().evaluate(&action, Path::new("")),
            JudgmentDecision::Deny { .. }
        ));
    }

    // ── v1.4 delegated judgment (§5.3, §9 "Scope Escape") ────────────────

    fn delegated_worktree() -> PathBuf {
        PathBuf::from(TEST_REPOSITORY)
            .join(".purrcode")
            .join("worktrees")
            .join("worker-a")
    }

    fn delegation(
        paths: &[&str],
        expected: purrcode_runtime_core::delegation::ExpectedOutput,
    ) -> purrcode_runtime_core::delegation::Delegation {
        use purrcode_runtime_core::delegation::{
            AuthorityInputs, DelegationBudget, DelegationRequest, PathPattern,
        };
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::None,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::new(),
        };
        let remaining = DelegationBudget::modest();
        DelegationRequest {
            parent_session_id: purrcode_runtime_core::SessionId::new(),
            parent_turn_id: purrcode_runtime_core::TurnId::new(),
            objective: "implement token exchange".into(),
            capability: purrcode_runtime_core::CapabilityId::parse("implement_backend").unwrap(),
            acceptance_criteria: Vec::new(),
            context_refs: Vec::new(),
            allowed_paths: paths
                .iter()
                .map(|p| PathPattern::parse(p).unwrap())
                .collect(),
            expected_output: expected,
            dependencies: Vec::new(),
            budget: DelegationBudget::modest(),
        }
        .admit(AuthorityInputs {
            workspace: &ceiling,
            parent: &ceiling,
            profile: &ceiling,
            parent_remaining_budget: &remaining,
            depth: 1,
        })
        .unwrap()
    }

    fn write(path: &str) -> ProposedAction {
        ProposedAction::WriteFile(purrcode_runtime_core::WriteFileAction {
            path: PathBuf::from(path),
            content: "contents".into(),
            expected_digest: None,
        })
    }

    #[test]
    fn a_worker_writing_outside_its_delegated_paths_is_denied_by_pawgate() {
        // §9 "Scope Escape": delegated `src/auth/**`, tried
        // `src/payments/billing.rs`. Expected: PawGate deny.
        let policy = Policy {
            auto_allow_worktree_writes: true,
            ..Policy::default()
        };
        let delegation = delegation(
            &["src/auth/**"],
            purrcode_runtime_core::delegation::ExpectedOutput::Patch,
        );
        let decision = policy.evaluate_delegated(
            &write("src/payments/billing.rs"),
            &delegated_worktree(),
            &delegation,
        );
        match decision {
            JudgmentDecision::Deny { reason } => {
                assert!(reason.contains("outside the paths delegated"), "{reason}");
                assert!(reason.contains("src/auth/**"), "{reason}");
            }
            other => panic!("expected a deny, got {other:?}"),
        }

        // …and the in-scope write is still allowed.
        assert!(matches!(
            policy.evaluate_delegated(
                &write("src/auth/token.rs"),
                &delegated_worktree(),
                &delegation
            ),
            JudgmentDecision::AllowWithConstraints(_)
        ));
    }

    #[test]
    fn a_read_only_worker_cannot_write_even_where_policy_would_allow_it() {
        let policy = Policy {
            auto_allow_worktree_writes: true,
            ..Policy::default()
        };
        let review = delegation(
            &["src/auth/**"],
            purrcode_runtime_core::delegation::ExpectedOutput::Review,
        );
        match policy.evaluate_delegated(&write("src/auth/token.rs"), &delegated_worktree(), &review)
        {
            JudgmentDecision::Deny { reason } => assert!(reason.contains("read-only"), "{reason}"),
            other => panic!("expected a deny, got {other:?}"),
        }
    }

    #[test]
    fn granted_constraints_are_narrowed_to_the_delegated_scope() {
        let policy = Policy {
            auto_allow_worktree_writes: true,
            ..Policy::default()
        };
        let delegation = delegation(
            &["src/auth/**"],
            purrcode_runtime_core::delegation::ExpectedOutput::Patch,
        );
        match policy.evaluate_delegated(
            &write("src/auth/token.rs"),
            &delegated_worktree(),
            &delegation,
        ) {
            JudgmentDecision::AllowWithConstraints(constraints) => {
                assert_eq!(constraints.allowed_write_globs, ["src/auth/token.rs"]);
                assert!(
                    constraints.maximum_changed_files <= delegation.budget().maximum_changed_files
                );
            }
            other => panic!("expected an allow, got {other:?}"),
        }
    }

    #[test]
    fn a_delegated_command_still_goes_through_the_ordinary_policy() {
        // A worker does not get a private command allowlist: `git status` is
        // read-only for everyone, and an unlisted program is denied for
        // everyone.
        let policy = Policy::default();
        let delegation = delegation(
            &["src/auth/**"],
            purrcode_runtime_core::delegation::ExpectedOutput::Patch,
        );
        let repository = PathBuf::from(TEST_REPOSITORY);
        assert!(matches!(
            policy.evaluate_delegated(&action(&["status"]), &repository, &delegation),
            JudgmentDecision::AllowWithConstraints(_)
        ));
        assert!(matches!(
            policy.evaluate_delegated(&command("curl", &["https://x"]), &repository, &delegation),
            JudgmentDecision::Deny { .. }
        ));
    }

    #[test]
    fn a_delegation_with_no_paths_can_write_nothing() {
        let policy = Policy {
            auto_allow_worktree_writes: true,
            ..Policy::default()
        };
        // A review delegation carries no write authority at all.
        let review = delegation(
            &[],
            purrcode_runtime_core::delegation::ExpectedOutput::Review,
        );
        assert!(matches!(
            policy.evaluate_delegated(&write("anything.rs"), &delegated_worktree(), &review),
            JudgmentDecision::Deny { .. }
        ));
    }
}
