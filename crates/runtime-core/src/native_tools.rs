//! The built-in native tool table (v1.3 §4.1 / §8 PR1).
//!
//! The 10 `RepositoryReadAction` kinds plus `WriteFile`, `DeleteFile` and
//! `Command` each map to a [`ToolDescriptorProposal`]. These are the *claims*;
//! they still pass through `CapabilityRegistry::admit_tool` like any other
//! provider, so the built-in table is subject to the same ceiling (which, for
//! `Builtin` origins, is the policy itself).
//!
//! A `const` array of `ToolDescriptor` is impossible by design: descriptors
//! have no public constructor and a digest field, and the only mint is the
//! registry. This module is the single declaration site the architecture doc's
//! `native_tools.rs` describes, expressed as proposal builders.

use super::tool::{
    ApprovalPolicy, DescriptorOrigin, FilesystemScope, NetworkScope, SideEffectClass,
    ToolDescriptorProposal, ToolId, ToolProvider,
};
use std::collections::BTreeSet;

fn read_proposal(name: &str, description: &str) -> ToolDescriptorProposal {
    ToolDescriptorProposal {
        id: ToolId::native(name),
        provider: ToolProvider::Native,
        display_name: name.to_owned(),
        description: description.to_owned(),
        schema: serde_json::json!({ "type": "object" }),
        capabilities: BTreeSet::new(),
        side_effect_class: SideEffectClass::Read,
        network_scope: NetworkScope::None,
        filesystem_scope: FilesystemScope::WorktreeRead,
        approval_policy: ApprovalPolicy::ByClass,
        origin: DescriptorOrigin::Builtin,
    }
}

/// Every built-in native tool proposal. In declaration order of
/// `RepositoryReadAction` plus the write/delete/command trio.
pub fn builtin_native_proposals() -> Vec<ToolDescriptorProposal> {
    vec![
        read_proposal("git_status", "Read git working-tree status"),
        read_proposal("git_rev_parse", "Resolve a git revision"),
        read_proposal("git_log", "Read git commit history"),
        read_proposal("git_diff", "Read the working-tree diff"),
        read_proposal("git_show", "Show the content of a git object"),
        read_proposal("git_ls_files", "List tracked files"),
        read_proposal("repository_grep", "Grep the repository"),
        read_proposal("find", "Find files by name"),
        read_proposal("list", "List a directory"),
        read_proposal("read_file", "Read a bounded file"),
        // ── Write / delete / command ──
        ToolDescriptorProposal {
            id: ToolId::native("write_file"),
            provider: ToolProvider::Native,
            display_name: "write_file".into(),
            description: "Write a file inside the session worktree".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            capabilities: BTreeSet::new(),
            side_effect_class: SideEffectClass::Write,
            network_scope: NetworkScope::None,
            filesystem_scope: FilesystemScope::maximum(),
            approval_policy: ApprovalPolicy::ByClass,
            origin: DescriptorOrigin::Builtin,
        },
        ToolDescriptorProposal {
            id: ToolId::native("delete_file"),
            provider: ToolProvider::Native,
            display_name: "delete_file".into(),
            description: "Delete a file inside the session worktree".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            capabilities: BTreeSet::new(),
            side_effect_class: SideEffectClass::Destructive,
            network_scope: NetworkScope::None,
            filesystem_scope: FilesystemScope::maximum(),
            approval_policy: ApprovalPolicy::AlwaysAsk,
            origin: DescriptorOrigin::Builtin,
        },
        ToolDescriptorProposal {
            id: ToolId::native("command"),
            provider: ToolProvider::Native,
            display_name: "command".into(),
            description: "Run a program in the sandbox".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "program": { "type": "string" },
                    "arguments": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["program"]
            }),
            capabilities: BTreeSet::new(),
            side_effect_class: SideEffectClass::Execute,
            network_scope: NetworkScope::None,
            filesystem_scope: FilesystemScope::maximum(),
            approval_policy: ApprovalPolicy::ByClass,
            origin: DescriptorOrigin::Builtin,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_covers_all_read_kinds_and_the_trio() {
        let proposals = builtin_native_proposals();
        assert_eq!(proposals.len(), 13, "10 reads + write + delete + command");
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
            "write_file",
            "delete_file",
            "command",
        ] {
            let id = ToolId::native(name);
            assert!(
                proposals.iter().any(|p| p.id == id),
                "missing builtin {name}"
            );
        }
    }

    #[test]
    fn read_kinds_are_read_and_command_is_execute() {
        let proposals = builtin_native_proposals();
        let by_name = |name: &str| {
            proposals
                .iter()
                .find(|p| p.id == ToolId::native(name))
                .unwrap_or_else(|| panic!("missing builtin {name}"))
        };
        assert_eq!(
            by_name("read_file").side_effect_class,
            SideEffectClass::Read
        );
        assert_eq!(
            by_name("write_file").side_effect_class,
            SideEffectClass::Write
        );
        assert_eq!(
            by_name("delete_file").side_effect_class,
            SideEffectClass::Destructive
        );
        assert_eq!(
            by_name("command").side_effect_class,
            SideEffectClass::Execute
        );
    }
}
