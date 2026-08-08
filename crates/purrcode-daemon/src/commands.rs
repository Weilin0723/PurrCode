//! The composer command registry, and what each command actually does.
//!
//! Before this module the registry published names and descriptions only, and
//! the submit path forwarded `/undo` to the language model as ordinary prose.
//! The model would reply "Sure, I'll undo that" and nothing would be undone —
//! a worse failure than a disabled button, because it looks like it worked.
//!
//! So every command declares its [`CommandExecution`]. A command is either
//! executed deterministically by the daemon at a named route, executed by the
//! client's own UI, or — declared explicitly, never by omission — expanded into
//! an instruction for the agent. A client that reads this registry cannot
//! accidentally present a deterministic operation as chat, and the daemon
//! refuses to accept a command as a conversation message at all.

use serde::Serialize;

/// How a command runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CommandExecution {
    /// The daemon performs it, deterministically, at `path` with `{id}`
    /// replaced by the session id. Never reaches a model.
    Daemon {
        method: &'static str,
        path: &'static str,
    },
    /// The client performs it in its own UI — opening a panel, focusing a
    /// surface, starting a picker. Never reaches a model.
    Client,
    /// The command is a shorthand for an instruction, and `prompt` is the text
    /// the agent receives. Declared so that "this one does go to the model" is
    /// something the client is told rather than something it assumes.
    Prompt { prompt: &'static str },
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct CommandDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub group: &'static str,
    pub execution: CommandExecution,
}

/// The canonical built-in commands.
pub(crate) fn builtin_commands() -> Vec<CommandDescriptor> {
    use CommandExecution::{Client, Daemon, Prompt};
    vec![
        CommandDescriptor {
            name: "/context",
            description: "Show the current context summary and token budget",
            group: "context",
            execution: Client,
        },
        CommandDescriptor {
            name: "/compact",
            description: "Compact the session context into a checkpoint",
            group: "context",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/compact",
            },
        },
        CommandDescriptor {
            name: "/undo",
            description: "Restore the worktree to the previous checkpoint",
            group: "session",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/undo",
            },
        },
        CommandDescriptor {
            name: "/redo",
            description: "Re-apply the changes undone by the last restore",
            group: "session",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/redo",
            },
        },
        CommandDescriptor {
            name: "/fork",
            description: "Fork this session at a conversation message",
            group: "session",
            // A fork needs an anchor message, so the client opens the picker
            // rather than the daemon guessing which message was meant.
            execution: Client,
        },
        CommandDescriptor {
            name: "/checkpoint",
            description: "Capture a restorable checkpoint",
            group: "session",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/checkpoint",
            },
        },
        CommandDescriptor {
            name: "/diff",
            description: "Review the current session diff",
            group: "review",
            execution: Client,
        },
        CommandDescriptor {
            name: "/test",
            description: "Ask the agent to run the validation suite",
            group: "review",
            execution: Prompt {
                prompt: "Run this project's validation suite and report exactly what it output, \
                         including any failures.",
            },
        },
        CommandDescriptor {
            name: "/review",
            description: "Ask the agent to review the proposed changes",
            group: "review",
            execution: Prompt {
                prompt: "Review the changes currently in this session's worktree and report what \
                         is correct, what is risky, and what is wrong.",
            },
        },
        CommandDescriptor {
            name: "/model",
            description: "Select a model for this session",
            group: "settings",
            execution: Client,
        },
        CommandDescriptor {
            name: "/agent",
            description: "Inspect or control the running agent",
            group: "settings",
            execution: Client,
        },
        CommandDescriptor {
            name: "/mcp",
            description: "Manage MCP servers",
            group: "settings",
            execution: Client,
        },
        CommandDescriptor {
            name: "/skills",
            description: "Search, install, and manage skills",
            group: "settings",
            execution: Client,
        },
        CommandDescriptor {
            name: "/memory",
            description: "Inspect and edit project memory",
            group: "settings",
            execution: Client,
        },
        CommandDescriptor {
            name: "/approve",
            description: "Approve the awaiting action, or the plan under review",
            group: "authority",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/approve",
            },
        },
        CommandDescriptor {
            // Deliberately narrower than `/approve`. There is no "reject the
            // plan" operation — revising a plan is done by saying what is wrong
            // with it, which is an ordinary message — so claiming the symmetry
            // would promise something the runtime does not have.
            name: "/reject",
            description: "Reject the action awaiting approval",
            group: "authority",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/reject",
            },
        },
        CommandDescriptor {
            name: "/pause",
            description: "Pause the current session",
            group: "session",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/pause",
            },
        },
        CommandDescriptor {
            name: "/resume",
            description: "Resume the current session",
            group: "session",
            execution: Daemon {
                method: "POST",
                path: "/v1/sessions/{id}/resume",
            },
        },
    ]
}

/// The command a message text invokes, if any.
///
/// A command is only a command when it is the first thing in the message: a
/// sentence that happens to mention `/undo` is prose about undo, and treating
/// it as an invocation would make quoting a command impossible.
pub(crate) fn command_for(text: &str) -> Option<CommandDescriptor> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }
    let name = trimmed
        .split_whitespace()
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    builtin_commands()
        .into_iter()
        .find(|command| command.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_command_is_recognised() {
        assert_eq!(command_for("/undo").map(|c| c.name), Some("/undo"));
        assert_eq!(command_for("  /undo  ").map(|c| c.name), Some("/undo"));
        assert_eq!(command_for("/UNDO").map(|c| c.name), Some("/undo"));
    }

    #[test]
    fn prose_that_mentions_a_command_is_not_an_invocation() {
        assert!(command_for("what does /undo do?").is_none());
        assert!(command_for("explain the /undo command").is_none());
    }

    #[test]
    fn an_unknown_slash_word_is_not_a_command() {
        // Left to the ordinary message path: it may well be a file path or a
        // regex the user is asking about.
        assert!(command_for("/usr/local/bin is on PATH").is_none());
        assert!(command_for("/nonsense").is_none());
    }

    #[test]
    fn the_destructive_session_commands_are_deterministic() {
        // These four are the reason this module exists. If any of them ever
        // becomes a `Prompt`, a user asking to undo their work gets a model
        // agreeing to undo their work, which is not the same thing.
        for name in ["/undo", "/redo", "/compact", "/checkpoint"] {
            let command = command_for(name).expect("registered");
            assert!(
                matches!(command.execution, CommandExecution::Daemon { .. }),
                "{name} must execute deterministically, not as a prompt"
            );
        }
    }

    #[test]
    fn every_command_declares_how_it_runs() {
        // The registry is the client's only source of truth for this. A command
        // whose execution is unstated is one a client will guess about.
        for command in builtin_commands() {
            match command.execution {
                CommandExecution::Daemon { path, .. } => assert!(
                    path.starts_with("/v1/"),
                    "{} must name a real route",
                    command.name
                ),
                CommandExecution::Prompt { prompt } => assert!(
                    !prompt.trim().is_empty(),
                    "{} must carry the instruction it expands to",
                    command.name
                ),
                CommandExecution::Client => {}
            }
        }
    }

    #[test]
    fn command_names_are_unique() {
        let mut names: Vec<&str> = builtin_commands()
            .iter()
            .map(|command| command.name)
            .collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "a duplicate name shadows a command");
    }
}
