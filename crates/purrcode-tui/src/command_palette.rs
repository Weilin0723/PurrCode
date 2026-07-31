//! Slash command dispatch. All commands call daemon API; no second execution path.

use crate::app::{App, AppMode, PendingModelPull};
use crate::provider_setup::ProviderSetup;
use crate::skill_browser::SkillBrowser;
use crate::status_bar::{PermissionMode, TaskMode};
use crate::ui_actions::UiActionDefinition;
use serde_json::Value;
use std::fmt::Write as _;

pub struct CommandPalette;

/// Every verb `CommandPalette::execute` serves, including the aliases resolved
/// by argument normalization.
///
/// This constant is load-bearing in both directions. The dispatcher's fallback
/// arm consults it, so a verb listed here without a match arm reports an
/// internal inconsistency instead of a plain "unknown command"; and
/// `ui_actions::coverage` / `ui_actions::orphan_commands` check it against the
/// action registry, so neither list can drift silently.
pub const DISPATCH_COMMANDS: &[&str] = &[
    "approve",
    "ask",
    "build",
    "cancel",
    "capability",
    "compact",
    "connect",
    "deny",
    "diff",
    "help",
    "history",
    "mcp",
    "model",
    "models",
    "new",
    "pause",
    "plan",
    "privacy",
    "provider",
    "providers",
    "quit",
    "research",
    "research-approve",
    "resume",
    "review",
    "role",
    "rollback",
    "session",
    "sessions",
    "mode",
    "permission",
    "settings",
    "status",
    "studio",
    "terminal",
    "terminal-return",
    "terminal-take",
    "skill-block",
    "skill-download",
    "skill-download-approve",
    "skill-install",
    "skill-install-approve",
    "skill-search",
    "skill-search-approve",
    "skills",
];

/// Palette entries for a query, generated from the action registry. There is no
/// separate palette list to keep in sync.
pub fn filtered_actions(query: &str) -> Vec<&'static UiActionDefinition> {
    crate::ui_actions::filtered(query)
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPalette {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(&self, app: &mut App, input: &str) {
        let input = input.trim().to_string();
        let (parsed_cmd, parsed_args) = input
            .strip_prefix('/')
            .map(|s| {
                let parts: Vec<&str> = s.splitn(2, ' ').collect();
                (parts[0].to_lowercase(), parts.get(1).copied().unwrap_or(""))
            })
            .unwrap_or_default();
        let (cmd, args) = match (parsed_cmd.as_str(), parsed_args.split_once(' ')) {
            ("skills", Some(("search", query))) => ("skill-search".to_owned(), query.to_owned()),
            ("mcp", Some(("search", query))) => (
                "skill-search".to_owned(),
                format!("MCP server capability: {query}"),
            ),
            ("capability", Some(("add", description))) => {
                ("skill-search".to_owned(), description.to_owned())
            }
            _ => (parsed_cmd, parsed_args.to_owned()),
        };
        let args = args.as_str();

        match cmd.as_str() {
            "help" => {
                app.switch_mode(AppMode::Help);
            }
            "connect" => {
                app.provider_setup = Some(if args.trim() == "import" {
                    ProviderSetup::import_mode()
                } else {
                    ProviderSetup::new()
                });
                app.switch_mode(AppMode::ProviderSetup);
                app.message_bar = "Setting up provider...".into();
            }
            "providers" => {
                match app
                    .request(reqwest::Method::GET, "/v1/providers", None)
                    .await
                {
                    Ok(val) => app.message_bar = format!("Providers: {val}"),
                    Err(e) => app.message_bar = format!("Error: {e}"),
                }
            }
            "provider" => {
                let mut parts = args.split_whitespace();
                let action = parts.next().unwrap_or("list");
                let name = parts.next();
                if parts.next().is_some() {
                    app.message_bar = "Usage: /provider [list|edit|test|remove] [name]".into();
                    return;
                }
                match (action, name) {
                    ("list", None) => match app
                        .request(reqwest::Method::GET, "/v1/providers", None)
                        .await
                    {
                        Ok(value) => app.message_bar = format!("Providers: {value}"),
                        Err(error) => app.message_bar = format!("Error: {error}"),
                    },
                    ("test", Some(name)) => match app
                        .request(
                            reqwest::Method::POST,
                            "/v1/providers/test",
                            Some(serde_json::json!({"provider": name})),
                        )
                        .await
                    {
                        Ok(value) => {
                            app.message_bar = format!(
                                "{name}: verified in {} ms — {}",
                                value["latency_ms"].as_u64().unwrap_or_default(),
                                value["detail"].as_str().unwrap_or("healthy response")
                            )
                        }
                        Err(error) => app.message_bar = format!("Provider test failed: {error}"),
                    },
                    ("edit", Some(name)) => match app
                        .request(reqwest::Method::GET, &format!("/v1/providers/{name}"), None)
                        .await
                    {
                        Ok(value) => match ProviderSetup::from_saved(&value) {
                            Ok(setup) => {
                                app.provider_setup = Some(setup);
                                app.switch_mode(AppMode::ProviderSetup);
                            }
                            Err(error) => app.message_bar = error,
                        },
                        Err(error) => app.message_bar = format!("Error: {error}"),
                    },
                    ("remove", Some(name)) => match app
                        .request(
                            reqwest::Method::DELETE,
                            &format!("/v1/providers/{name}"),
                            None,
                        )
                        .await
                    {
                        Ok(_) => app.message_bar = format!("Provider {name} removed."),
                        Err(error) => app.message_bar = format!("Error: {error}"),
                    },
                    _ => app.message_bar = "Usage: /provider [list|edit|test|remove] [name]".into(),
                }
            }
            "models" | "model" => {
                let mut parts = args.split_whitespace();
                let action = parts.next().unwrap_or_default();
                if cmd == "model" && action == "recommend" {
                    if parts.next().is_some() {
                        app.message_bar = "Usage: /model recommend".into();
                        return;
                    }
                    app.message_bar =
                        "Inspecting Ollama metadata, resources, and qualification evidence..."
                            .into();
                    let recommendations = app
                        .request(
                            reqwest::Method::GET,
                            "/v1/local-models/recommendations",
                            None,
                        )
                        .await;
                    let local_status = app
                        .request(reqwest::Method::GET, "/v1/local-models", None)
                        .await;
                    match recommendations {
                        Ok(recommendations) => {
                            let resources = local_status.unwrap_or_else(|error| {
                                serde_json::json!({
                                    "resource_error": error.to_string(),
                                })
                            });
                            app.message_bar =
                                format_recommendation_report(&recommendations, &resources);
                        }
                        Err(error) => {
                            app.message_bar = format!("Model recommendation failed: {error}")
                        }
                    }
                    return;
                }
                if cmd == "model" && action == "qualify" {
                    let (Some(provider), Some(model), None) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        app.message_bar =
                            "Usage: /model qualify <ollama-provider> <installed-model>".into();
                        return;
                    };
                    app.message_bar =
                        format!("Qualifying {provider}/{model} with the real provider...");
                    match app
                        .request_with_timeout(
                            reqwest::Method::POST,
                            "/v1/local-models/qualify",
                            Some(serde_json::json!({
                                "provider": provider,
                                "model": model,
                            })),
                            std::time::Duration::from_secs(10 * 60),
                        )
                        .await
                    {
                        Ok(value) => {
                            app.message_bar = format_qualification_report(&value);
                        }
                        Err(error) => {
                            app.message_bar = format!("Model qualification failed: {error}")
                        }
                    }
                    return;
                }
                if cmd == "model" && action == "pull" {
                    let (Some(model), None) = (parts.next(), parts.next()) else {
                        app.message_bar = "Usage: /model pull <model>".into();
                        return;
                    };
                    if app.active_pull_action.is_some() {
                        app.message_bar =
                            "A model pull is already active. Cancel or finish it first.".into();
                        return;
                    }
                    if let Some(pending) = &app.pending_model_pull {
                        app.message_bar = format!(
                            "Pull action {} for `{}` is still awaiting approval. Press P or run /model pull-approve.",
                            pending.action_id, pending.model
                        );
                        return;
                    }
                    match app
                        .request(
                            reqwest::Method::POST,
                            "/v1/local-models/pull/propose",
                            Some(serde_json::json!({
                                "repository": app.config.repository,
                                "model": model,
                            })),
                        )
                        .await
                    {
                        Ok(value) => {
                            let fields = (
                                value["action_id"].as_str(),
                                value["action_digest"].as_str(),
                                value["session_id"].as_str(),
                                value["model"].as_str(),
                            );
                            let (
                                Some(action_id),
                                Some(action_digest),
                                Some(session_id),
                                Some(model),
                            ) = fields
                            else {
                                app.message_bar =
                                    "Pull proposal failed: daemon returned an incomplete authorization record."
                                        .into();
                                return;
                            };
                            app.pending_model_pull = Some(PendingModelPull {
                                action_id: action_id.to_owned(),
                                action_digest: action_digest.to_owned(),
                                session_id: session_id.to_owned(),
                                model: model.to_owned(),
                                approved: false,
                            });
                            app.message_bar = format!(
                                "Approval required — no pull has started.\nExact action: ollama pull {model}\nAction ID: {action_id}\nDigest: {action_digest}\nNetwork: allowed · external Ollama model store may change\nPress P or run /model pull-approve to approve this exact action."
                            );
                        }
                        Err(error) => {
                            app.message_bar = format!("Pull proposal failed: {error}");
                        }
                    }
                    return;
                }
                if cmd == "model" && action == "pull-approve" {
                    if parts.next().is_some() {
                        app.message_bar = "Usage: /model pull-approve".into();
                        return;
                    }
                    let Some(mut pending) = app.pending_model_pull.clone() else {
                        app.message_bar =
                            "No exact Ollama pull action is awaiting approval.".into();
                        return;
                    };
                    if !pending.approved {
                        match app
                            .request(
                                reqwest::Method::POST,
                                &format!("/v1/local-models/pull/{}/approve", pending.action_id),
                                Some(serde_json::json!({
                                    "session_id": pending.session_id,
                                })),
                            )
                            .await
                        {
                            Ok(value)
                                if value["action_digest"].as_str()
                                    == Some(pending.action_digest.as_str()) =>
                            {
                                pending.approved = true;
                                app.pending_model_pull = Some(pending.clone());
                            }
                            Ok(_) => {
                                app.message_bar =
                                    "Pull approval rejected: daemon digest did not match the displayed exact action."
                                        .into();
                                return;
                            }
                            Err(error) => {
                                app.message_bar = format!("Pull approval failed: {error}");
                                return;
                            }
                        }
                    }
                    match app
                        .request(
                            reqwest::Method::POST,
                            &format!("/v1/local-models/pull/{}/start", pending.action_id),
                            Some(serde_json::json!({
                                "session_id": pending.session_id,
                            })),
                        )
                        .await
                    {
                        Ok(progress) => {
                            app.active_pull_action = Some(pending.action_id.clone());
                            app.active_pull_session = Some(pending.session_id.clone());
                            app.pending_model_pull = None;
                            app.message_bar = format!(
                                "Approved exact pull started for `{}` · {}.\nCtrl+C cancels. Completion is accepted only after daemon rediscovery.",
                                pending.model,
                                progress["phase"].as_str().unwrap_or("queued")
                            );
                        }
                        Err(error) => {
                            app.pending_model_pull = Some(pending);
                            app.message_bar = format!(
                                "Exact pull was approved but could not start: {error}\nRun /model pull-approve to retry the same authorized action."
                            );
                        }
                    }
                    return;
                }
                if cmd == "model" && action == "pull-cancel" {
                    if parts.next().is_some() {
                        app.message_bar = "Usage: /model pull-cancel".into();
                        return;
                    }
                    let (Some(action_id), Some(session_id)) = (
                        app.active_pull_action.clone(),
                        app.active_pull_session.clone(),
                    ) else {
                        app.message_bar = "No active Ollama pull to cancel.".into();
                        return;
                    };
                    match app
                        .request(
                            reqwest::Method::POST,
                            &format!("/v1/local-models/pull/{action_id}/cancel"),
                            Some(serde_json::json!({
                                "session_id": session_id,
                            })),
                        )
                        .await
                    {
                        Ok(_) => {
                            app.message_bar =
                                "Model pull cancellation requested; waiting for terminal evidence."
                                    .into();
                        }
                        Err(error) => {
                            app.message_bar = format!("Model pull cancellation failed: {error}");
                        }
                    }
                    return;
                }
                if cmd == "model" && action == "loaded" && parts.next().is_none() {
                    match app
                        .request(reqwest::Method::GET, "/v1/local-models", None)
                        .await
                    {
                        Ok(value) => {
                            let loaded = value["loaded"]
                                .as_array()
                                .map(|models| {
                                    models
                                        .iter()
                                        .filter_map(|model| model["name"].as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .filter(|models| !models.is_empty())
                                .unwrap_or_else(|| "none".into());
                            app.message_bar = format!(
                                "Loaded: {loaded}\nMemory pressure: {} · local concurrency: {}",
                                value["resources"]["memory_pressure"]
                                    .as_str()
                                    .unwrap_or("unknown"),
                                value["resources"]["maximum_local_inference_requests"]
                                    .as_u64()
                                    .unwrap_or(1)
                            );
                        }
                        Err(error) => {
                            app.message_bar = format!("Local model status failed: {error}")
                        }
                    }
                    return;
                }
                if cmd == "model" && action == "unload-all" && parts.next().is_none() {
                    match app
                        .request(
                            reqwest::Method::POST,
                            "/v1/local-models/unload",
                            Some(serde_json::json!({"all": true})),
                        )
                        .await
                    {
                        Ok(value) => {
                            app.message_bar =
                                format!("Unloaded and verified: {}", value["unloaded"])
                        }
                        Err(error) => app.message_bar = format!("Unload failed: {error}"),
                    }
                    return;
                }
                if cmd == "model" && action == "unload" {
                    let Some(model) = parts.next() else {
                        app.message_bar = "Usage: /model unload <model>".into();
                        return;
                    };
                    if parts.next().is_some() {
                        app.message_bar = "Usage: /model unload <model>".into();
                        return;
                    }
                    match app
                        .request(
                            reqwest::Method::POST,
                            "/v1/local-models/unload",
                            Some(serde_json::json!({"model": model})),
                        )
                        .await
                    {
                        Ok(_) => {
                            app.message_bar = format!("Unloaded and verified: {model}");
                        }
                        Err(error) => app.message_bar = format!("Unload failed: {error}"),
                    }
                    return;
                }
                match app.request(reqwest::Method::GET, "/v1/models", None).await {
                    Ok(val) => {
                        let choices = val
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|model| model["id"].as_str().map(str::to_owned))
                            .collect::<Vec<_>>();
                        if cmd == "models" || args.is_empty() {
                            app.model_selected = choices
                                .iter()
                                .position(|model| model == &app.status_bar.model)
                                .unwrap_or(0);
                            app.model_choices = choices;
                            app.switch_mode(AppMode::ModelBrowse);
                            app.message_bar.clear();
                            return;
                        }
                        let Some(selected) = choices.iter().find(|model| model.as_str() == args)
                        else {
                            app.message_bar = format!(
                                "Unknown model `{args}`. Run /models and select a configured model."
                            );
                            return;
                        };
                        let update = if let Some(id) = app.session_id.clone() {
                            app.request(
                                reqwest::Method::POST,
                                &format!("/v1/sessions/{id}/model"),
                                Some(serde_json::json!({"model": selected})),
                            )
                            .await
                        } else {
                            app.request(
                                reqwest::Method::POST,
                                "/v1/models/roles",
                                Some(serde_json::json!({
                                    "role": "coding_worker",
                                    "model": selected,
                                })),
                            )
                            .await
                        };
                        match update {
                            Ok(_) => {
                                app.status_bar.set_model(selected);
                                app.status_bar.local = val
                                    .as_array()
                                    .and_then(|models| {
                                        models.iter().find(|model| model["id"] == selected.as_str())
                                    })
                                    .and_then(|model| model["local"].as_bool())
                                    .unwrap_or(false);
                                app.message_bar = format!("Switched model to `{selected}`.");
                            }
                            Err(error) => {
                                app.message_bar = format!("Model switch failed: {error}");
                            }
                        }
                    }
                    Err(e) => app.message_bar = format!("Error: {e}"),
                }
            }
            // `/mcp search <q>` and `/capability add <text>` normalize to
            // skill-search above. Reaching here means the argument form was
            // wrong, so state the usage rather than falling through to
            // "unknown command".
            "mcp" => {
                app.message_bar = "Usage: /mcp search <capability description>".into();
            }
            "capability" => {
                app.message_bar = "Usage: /capability add <capability description>".into();
            }
            "skills" => {
                let token = app.token.clone();
                let daemon_url = app.daemon_url().to_string();
                let client = reqwest::Client::new();
                app.skill_browser = Some(SkillBrowser::new());
                app.switch_mode(AppMode::SkillBrowse);
                if let Some(ref mut browser) = app.skill_browser {
                    browser.load(&client, &daemon_url, &token).await;
                }
            }
            "role" => {
                let mut parts = args.split_whitespace();
                let (Some(role), Some(model), None) = (parts.next(), parts.next(), parts.next())
                else {
                    app.message_bar =
                        "Usage: /role <coding_worker|judge|planner|reviewer> <provider/model>"
                            .into();
                    return;
                };
                match app
                    .request(
                        reqwest::Method::POST,
                        "/v1/models/roles",
                        Some(serde_json::json!({"role": role, "model": model})),
                    )
                    .await
                {
                    Ok(_) => app.message_bar = format!("Assigned {model} to {role}"),
                    Err(error) => app.message_bar = format!("Error: {error}"),
                }
            }
            "skill-search" | "skill-search-approve" => {
                let query = if args.is_empty() { "all" } else { args };
                app.message_bar = format!("Searching skills for: {query}...");
                let token = app.token.clone();
                let daemon_url = app.daemon_url().to_string();
                let session_id = app.session_id.clone();
                let client = reqwest::Client::new();
                if cmd == "skill-search" || app.skill_browser.is_none() {
                    app.skill_browser = Some(SkillBrowser::new());
                }
                app.switch_mode(AppMode::SkillBrowse);
                if let Some(ref mut browser) = app.skill_browser {
                    browser
                        .search(
                            &client,
                            &daemon_url,
                            &token,
                            query,
                            session_id.as_deref(),
                            cmd == "skill-search-approve",
                        )
                        .await;
                }
            }
            // Review reads the daemon's own effect evidence; it does not build a
            // second view of what changed.
            "diff" => app.load_review().await,
            // PRD §14: the header deliberately omits paths, SHAs, session ids
            // and endpoints. They have to remain reachable, or hiding them is
            // just withholding them.
            "status" => {
                let mut lines = vec![
                    format!("Repository: {}", app.config.repository.display()),
                    format!(
                        "Branch: {} ({})",
                        app.workspace.branch, app.workspace.source_state
                    ),
                    format!("Model: {}", app.status_bar.model),
                    format!("Task mode: {}", app.status_bar.task_mode.label()),
                    format!("Permission: {}", app.status_bar.permission.label()),
                    format!("Privacy: {}", app.status_bar.privacy),
                    format!(
                        "Daemon: {} ({})",
                        app.config.daemon_url, app.workspace.daemon_health
                    ),
                    format!("Sandbox: {}", app.workspace.sandbox),
                ];
                match app.session_id.as_deref() {
                    Some(session) => lines.push(format!("Session: {session}")),
                    None => lines.push("Session: none started".into()),
                }
                app.message_bar = lines.join("\n");
            }
            "terminal" => app.open_terminal().await,
            "terminal-take" => app.set_terminal_owner(true).await,
            "terminal-return" => app.set_terminal_owner(false).await,
            "studio" => app.open_studio().await,
            "privacy" => {
                if app.status_bar.privacy == "local-only" {
                    app.status_bar.set_privacy("mixed");
                    app.message_bar = "Privacy mode: mixed (web search allowed)".into();
                } else {
                    app.status_bar.set_privacy("local-only");
                    app.message_bar = "Privacy mode: local-only".into();
                }
            }
            "research" | "research-approve" => {
                let Some(session_id) = app.session_id.clone() else {
                    app.message_bar = "Start a conversation before web research.".into();
                    return;
                };
                if args.is_empty() {
                    app.message_bar = format!("Usage: /{cmd} <https-url>");
                    return;
                }
                let action_id = if cmd == "research-approve" {
                    match app.pending_research.as_ref() {
                        Some(pending) if pending.url == args => Some(pending.action_id.clone()),
                        _ => {
                            app.message_bar = "No exact pending research action matches this URL. Run /research <https-url> first.".into();
                            return;
                        }
                    }
                } else {
                    None
                };
                match app
                    .request(
                        reqwest::Method::POST,
                        "/v1/research/fetch",
                        Some(serde_json::json!({
                            "session_id": session_id,
                            "url": args,
                            "approved": cmd == "research-approve",
                            "domain_approved": cmd == "research-approve",
                            "action_id": action_id,
                        })),
                    )
                    .await
                {
                    Ok(value) if value["requires_approval"].as_bool() == Some(true) => {
                        app.pending_research = value["action_id"].as_str().map(|action_id| {
                            crate::app::PendingResearch {
                                action_id: action_id.to_owned(),
                                url: args.to_owned(),
                            }
                        });
                        app.message_bar = format!(
                            "Research fetch requires approval. Run /research-approve {args}"
                        );
                    }
                    Ok(value) => {
                        app.pending_research = None;
                        app.message_bar = format!(
                            "Evidence {}: {}",
                            value["content_digest"].as_str().unwrap_or("unknown"),
                            value["content"].as_str().unwrap_or("")
                        )
                    }
                    Err(error) => app.message_bar = format!("Research failed: {error}"),
                }
            }
            "ask" => app.set_task_mode(TaskMode::Ask),
            "plan" => app.set_task_mode(TaskMode::Plan),
            "build" => app.set_task_mode(TaskMode::Build),
            "review" => app.set_task_mode(TaskMode::Review),
            // `/mode` with no argument cycles; with one it sets that mode.
            "mode" => match TaskMode::parse(args) {
                Some(mode) => app.set_task_mode(mode),
                None if args.trim().is_empty() => {
                    app.set_task_mode(app.status_bar.task_mode.next())
                }
                None => {
                    app.message_bar =
                        format!("`{args}` is not a task mode. Choose Ask, Plan, Build or Review.")
                }
            },
            "permission" => match PermissionMode::parse(args) {
                Some(mode) => app.set_permission_mode(mode),
                None if args.trim().is_empty() => {
                    app.set_permission_mode(app.status_bar.permission.next())
                }
                None => {
                    app.message_bar = format!(
                        "`{args}` is not a permission mode. Choose Ask, Auto or Full Access."
                    )
                }
            },
            "new" => {
                app.session_id = None;
                app.conversation = crate::conversation::Conversation::new();
                app.message_bar = "New session started.".into();
            }
            "session" => {
                if !args.is_empty() {
                    match app
                        .request(reqwest::Method::GET, &format!("/v1/sessions/{args}"), None)
                        .await
                    {
                        Ok(_) => {
                            app.session_id = Some(args.to_owned());
                            app.refresh().await;
                            app.message_bar = format!("Reconnected to durable session {args}");
                        }
                        Err(error) => app.message_bar = format!("Error: {error}"),
                    }
                } else if let Some(id) = &app.session_id {
                    match app
                        .request(reqwest::Method::GET, &format!("/v1/sessions/{id}"), None)
                        .await
                    {
                        Ok(value) => {
                            app.message_bar =
                                serde_json::to_string_pretty(&value).unwrap_or_default()
                        }
                        Err(error) => app.message_bar = format!("Error: {error}"),
                    }
                } else {
                    app.message_bar = "No active session.".into();
                }
            }
            "sessions" | "history" => match app
                .request(reqwest::Method::GET, "/v1/sessions", None)
                .await
            {
                Ok(value) => {
                    app.message_bar = serde_json::to_string_pretty(&value).unwrap_or_default()
                }
                Err(error) => app.message_bar = format!("Error: {error}"),
            },
            "settings" => {
                let mut parts = args.split_whitespace();
                if parts.next() == Some("local-model-lifecycle") {
                    let policy = parts.next();
                    let timeout = parts.next();
                    if parts.next().is_some() {
                        app.message_bar = "Usage: /settings local-model-lifecycle [unload_after_request|idle_timeout|keep_loaded|external] [seconds]".into();
                        return;
                    }
                    let result = if let Some(policy) = policy {
                        let idle_timeout_seconds = match timeout {
                            Some(value) => match value.parse::<u64>() {
                                Ok(value) => value,
                                Err(_) => {
                                    app.message_bar =
                                        "Idle timeout must be a whole number of seconds.".into();
                                    return;
                                }
                            },
                            None => 300,
                        };
                        app.request(
                            reqwest::Method::POST,
                            "/v1/local-models/settings",
                            Some(serde_json::json!({
                                "policy": policy,
                                "idle_timeout_seconds": idle_timeout_seconds,
                            })),
                        )
                        .await
                    } else {
                        app.request(reqwest::Method::GET, "/v1/local-models/settings", None)
                            .await
                    };
                    match result {
                        Ok(value) => {
                            app.message_bar = format!(
                                "Local model lifecycle: {} · idle timeout: {}s",
                                value["policy"].as_str().unwrap_or("unknown"),
                                value["idle_timeout_seconds"].as_u64().unwrap_or(300)
                            );
                        }
                        Err(error) => {
                            app.message_bar = format!("Lifecycle settings failed: {error}")
                        }
                    }
                    return;
                }
                let providers = app
                    .request(reqwest::Method::GET, "/v1/providers", None)
                    .await;
                let models = app.request(reqwest::Method::GET, "/v1/models", None).await;
                app.message_bar = format!(
                    "Providers: {}\nModels: {}",
                    providers.map_or_else(|e| format!("error: {e}"), |v| v.to_string()),
                    models.map_or_else(|e| format!("error: {e}"), |v| v.to_string())
                );
            }
            "skill-block" => {
                if args.is_empty() {
                    app.message_bar = "Usage: /skill-block <publisher>".into();
                } else {
                    match app.request(reqwest::Method::POST, "/v1/skills/publishers/block", Some(serde_json::json!({"publisher": args, "reason": "blocked from TUI"}))).await {
                        Ok(_) => app.message_bar = format!("Blocked skill publisher: {args}"),
                        Err(error) => app.message_bar = format!("Error: {error}"),
                    }
                }
            }
            "skill-download" | "skill-download-approve" => {
                let Some(session_id) = app.session_id.clone() else {
                    app.message_bar = "Start a conversation before downloading a skill.".into();
                    return;
                };
                let mut parts = args.split_whitespace();
                let (Some(candidate_id), Some(commit), None) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    app.message_bar =
                        format!("Usage: /{cmd} <github:owner/repository> <40-character-commit>");
                    return;
                };
                let action_id = if cmd == "skill-download-approve" {
                    match app.pending_skill_download.as_ref() {
                        Some(pending)
                            if pending.candidate_id == candidate_id && pending.commit == commit =>
                        {
                            Some(pending.action_id.clone())
                        }
                        _ => {
                            app.message_bar = "No exact pending download matches this repository and commit. Run /skill-download first.".into();
                            return;
                        }
                    }
                } else {
                    None
                };
                match app
                    .request(
                        reqwest::Method::POST,
                        "/v1/skills/download",
                        Some(serde_json::json!({
                            "session_id": session_id,
                            "candidate_id": candidate_id,
                            "commit": commit,
                            "approved": cmd == "skill-download-approve",
                            "action_id": action_id,
                        })),
                    )
                    .await
                {
                    Ok(value) if value["requires_approval"].as_bool() == Some(true) => {
                        app.pending_skill_download = value["action_id"].as_str().map(|action_id| {
                            crate::app::PendingSkillDownload {
                                action_id: action_id.to_owned(),
                                candidate_id: candidate_id.to_owned(),
                                commit: commit.to_owned(),
                            }
                        });
                        app.message_bar = format!("Download requires separate approval. Run /skill-download-approve {candidate_id} {commit}");
                    }
                    Ok(value) => {
                        app.pending_skill_download = None;
                        app.message_bar = format!("Downloaded and inspected {} v{}. Review it, then run /skill-install <scope>.", value["name"].as_str().unwrap_or("skill"), value["version"].as_str().unwrap_or("?"));
                        app.downloaded_skill = Some(value);
                        app.switch_mode(AppMode::Conversation);
                    }
                    Err(error) => app.message_bar = format!("Download failed: {error}"),
                }
            }
            "skill-install" => {
                let Some(session_id) = app.session_id.clone() else {
                    app.message_bar = "No active session.".into();
                    return;
                };
                let scope = if args.is_empty() { "repository" } else { args };
                if !matches!(scope, "user" | "repository" | "session") {
                    app.message_bar = "Usage: /skill-install <user|repository|session>".into();
                    return;
                }
                let Some(skill) = app.downloaded_skill.clone() else {
                    app.message_bar = "Download and inspect a skill first.".into();
                    return;
                };
                match app
                    .request(
                        reqwest::Method::POST,
                        "/v1/skills/install/propose",
                        Some(serde_json::json!({
                            "session_id": session_id,
                            "candidate_id": skill["name"],
                            "version": skill["version"],
                            "scope": scope,
                            "source_path": skill["source_path"],
                            "content_digest": skill["content_digest"],
                            "publisher": skill["publisher"],
                            "approved_permissions": {},
                            "signature": null,
                            "publisher_public_key": null,
                        })),
                    )
                    .await
                {
                    Ok(value) => {
                        app.pending_skill_install_action =
                            value["action_id"].as_str().map(str::to_owned);
                        app.message_bar = "Install action inspected and awaiting approval. Run /skill-install-approve to authorize this exact digest.".into();
                    }
                    Err(error) => app.message_bar = format!("Install proposal failed: {error}"),
                }
            }
            "skill-install-approve" => {
                let (Some(session_id), Some(action_id)) = (
                    app.session_id.clone(),
                    app.pending_skill_install_action.clone(),
                ) else {
                    app.message_bar = "No pending skill install action.".into();
                    return;
                };
                let approval = app
                    .request(
                        reqwest::Method::POST,
                        &format!("/v1/skills/install/{action_id}/approve"),
                        Some(serde_json::json!({"session_id": session_id.clone()})),
                    )
                    .await;
                if let Err(error) = approval {
                    app.message_bar = format!("Install approval failed: {error}");
                    return;
                }
                match app
                    .request(
                        reqwest::Method::POST,
                        "/v1/skills/install",
                        Some(serde_json::json!({"session_id": session_id, "action_id": action_id})),
                    )
                    .await
                {
                    Ok(value) => {
                        app.message_bar = format!(
                            "Installed and qualified {} v{}.",
                            value["skill_id"].as_str().unwrap_or("skill"),
                            value["version"].as_str().unwrap_or("?")
                        );
                        app.pending_skill_install_action = None;
                    }
                    Err(error) => app.message_bar = format!("Install failed: {error}"),
                }
            }
            "compact" => {
                if let Some(id) = &app.session_id {
                    match app
                        .request(
                            reqwest::Method::POST,
                            &format!("/v1/sessions/{id}/compact"),
                            Some(serde_json::json!({})),
                        )
                        .await
                    {
                        Ok(_) => app.message_bar = "Context compacted.".into(),
                        Err(e) => app.message_bar = format!("Error: {e}"),
                    }
                } else {
                    app.message_bar = "No active session.".into();
                }
            }
            // Approval and rejection are the only commands that grant or refuse
            // execution authority. They never submit an action the user has not
            // been shown: from anywhere else they open the focused decision
            // surface first, and only submit once it is on screen.
            "approve" | "deny" => {
                let approve = cmd == "approve";
                if app.mode == AppMode::Approval {
                    app.submit_decision(approve).await;
                } else if app.open_approval() && !approve {
                    // Rejection from outside the surface still requires seeing it.
                    app.message_bar = "Review the action below, then press R to reject it.".into();
                }
            }
            "resume" | "rollback" | "pause" => {
                let Some(id) = app.session_id.clone() else {
                    app.message_bar = "No active session.".into();
                    return;
                };
                let (endpoint, body) = match cmd.as_str() {
                    "resume" => ("resume", serde_json::json!({})),
                    "rollback" => ("rollback", serde_json::json!({})),
                    _ => ("pause", serde_json::json!({"reason": "paused from TUI"})),
                };
                match app
                    .request(
                        reqwest::Method::POST,
                        &format!("/v1/sessions/{id}/{endpoint}"),
                        Some(body),
                    )
                    .await
                {
                    Ok(_) => {
                        app.message_bar = match endpoint {
                            "resume" => "Session resumed.".into(),
                            "rollback" => {
                                "Agent-owned changes were discarded; your working tree is untouched."
                                    .to_owned()
                            }
                            _ => "Session paused; all evidence is preserved.".into(),
                        };
                        app.refresh().await;
                        app.refresh_approval();
                    }
                    Err(error) => app.message_bar = format!("Error: {error}"),
                }
            }
            "cancel" => {
                if !app.stream.active {
                    app.message_bar = "No active session stream to cancel.".into();
                    return;
                }
                let Some(session_id) = app.session_id.clone() else {
                    app.message_bar = "No active session.".into();
                    return;
                };
                app.message_bar = "Requesting cooperative cancellation...".into();
                match app
                    .request(
                        reqwest::Method::POST,
                        &format!("/v1/sessions/{session_id}/cancel"),
                        Some(serde_json::json!({
                            "reason": "cancelled from TUI with Ctrl+C"
                        })),
                    )
                    .await
                {
                    Ok(_) => {
                        if let Some(delta) = app.stream.finish_cancelled() {
                            app.conversation.append_streaming(&delta);
                        }
                        app.conversation.cancel_streaming();
                        app.workspace.session_phase = "cancelled".into();
                        app.message_bar = "Cancelled; partial output was preserved.".into();
                    }
                    Err(error) => {
                        app.message_bar = format!(
                            "Cancellation was not accepted; the live stream remains attached: {error}"
                        );
                    }
                }
            }
            "quit" => {
                app.message_bar = "Goodbye.".into();
                app.quit_requested = true;
            }
            // A verb listed in DISPATCH_COMMANDS that reaches this arm is an
            // internal inconsistency, not a user error: the discovery surfaces
            // advertise it but nothing serves it. Report that distinctly so the
            // failure is visible instead of looking like a typo.
            registered if DISPATCH_COMMANDS.contains(&registered) => {
                app.message_bar = format!(
                    "/{registered} is registered but has no handler in this build. Please report this; no action was taken."
                );
            }
            _ => {
                app.message_bar = format!("Unknown command: /{cmd}. Type /help for commands.");
            }
        }
    }
}

fn format_recommendation_report(recommendations: &Value, local_status: &Value) -> String {
    let report = &recommendations["report"];
    let resources = &local_status["resources"];
    let mut output = String::new();
    push_line(
        &mut output,
        format_args!(
            "Local model evidence · provider {} · observed {}",
            text_or(&recommendations["provider"], "unknown"),
            text_or(&recommendations["observed_at"], "unknown")
        ),
    );
    if resources.is_object() {
        push_line(
            &mut output,
            format_args!(
                "Resources · physical {} · available {} · loaded {} · swap used {} · pressure {}",
                optional_bytes(&resources["total_memory_bytes"]),
                optional_bytes(&resources["available_memory_bytes"]),
                optional_bytes(&resources["loaded_model_bytes"]),
                optional_bytes(&resources["used_swap_bytes"]),
                text_or(&resources["memory_pressure"], "unknown")
            ),
        );
        push_line(
            &mut output,
            format_args!(
                "Governor · local requests {} · separate local judge {}",
                optional_u64(&resources["maximum_local_inference_requests"]),
                optional_bool(&resources["allow_separate_local_judge"])
            ),
        );
    } else if let Some(error) = local_status["resource_error"].as_str() {
        push_line(&mut output, format_args!("Resources unavailable · {error}"));
    }

    match report["outcome"]["status"].as_str() {
        Some("recommended") => push_line(
            &mut output,
            format_args!(
                "Primary · {} · evidence score {}",
                text_or(&report["outcome"]["model"], "unknown"),
                optional_u64(&report["outcome"]["score"])
            ),
        ),
        Some("no_recommendation") => push_line(
            &mut output,
            format_args!(
                "Primary · no recommendation ({})",
                enum_array(&report["outcome"]["reasons"])
            ),
        ),
        _ => push_line(&mut output, format_args!("Primary · evidence unavailable")),
    }

    let constraints = &report["constraints"];
    push_line(
        &mut output,
        format_args!(
            "Constraints · context {} tokens · parallel {} · single model {} · lifecycle {}{}",
            optional_u64(&constraints["context_limit_tokens"]),
            optional_u64(&constraints["maximum_parallel_requests"]),
            optional_bool(&constraints["single_model_mode"]),
            pretty_enum(text_or(&constraints["lifecycle"], "unknown")),
            constraints["idle_timeout_seconds"]
                .as_u64()
                .map(|seconds| format!(" ({seconds}s)"))
                .unwrap_or_default()
        ),
    );
    let resource_risks = notice_array(&report["resource_risks"]);
    if !resource_risks.is_empty() {
        push_line(
            &mut output,
            format_args!("Resource risks · {resource_risks}"),
        );
    }

    let Some(cards) = report["cards"].as_array() else {
        push_line(
            &mut output,
            format_args!("Qualification cards · unavailable"),
        );
        return output.trim_end().to_owned();
    };
    if cards.is_empty() {
        push_line(
            &mut output,
            format_args!("Qualification cards · no installed Ollama models observed"),
        );
    }
    for card in cards {
        push_line(
            &mut output,
            format_args!(
                "\n{} · {}{}",
                text_or(&card["model"], "unnamed model"),
                pretty_enum(text_or(&card["status"], "unknown")),
                if card["currently_loaded"].as_bool() == Some(true) {
                    " · loaded"
                } else {
                    ""
                }
            ),
        );
        push_line(
            &mut output,
            format_args!(
                "  Metadata · parameters {} · quantization {} · context {} · observed size {}",
                compact_count(&card["parameter_count"]),
                text_or(&card["quantization"], "not observed"),
                optional_u64(&card["metadata_context_tokens"]),
                optional_bytes(&card["observed_size_bytes"])
            ),
        );
        push_line(
            &mut output,
            format_args!(
                "  Qualification · {} · coding {} · cases {}/{} · accuracy {} · JSON {} · tools {}",
                pretty_enum(text_or(&card["qualification"]["assessment"], "missing")),
                pretty_enum(text_or(&card["qualification"]["coding"], "unverified")),
                optional_u64(&card["qualification"]["passed_cases"]),
                optional_u64(&card["qualification"]["total_cases"]),
                basis_points(&card["qualification"]["accuracy_basis_points"]),
                pretty_enum(text_or(
                    &card["qualification"]["structured_json"],
                    "not_tested"
                )),
                pretty_enum(text_or(
                    &card["qualification"]["tool_calling"],
                    "not_tested"
                ))
            ),
        );
        push_line(
            &mut output,
            format_args!(
                "  Runtime · estimated RAM {} · recommended context {} · latency {} · throughput {} · reliable context {} · score {}",
                optional_bytes(&card["estimated_runtime_memory_bytes"]),
                optional_u64(&card["recommended_context_tokens"]),
                optional_milliseconds(&card["qualification"]["mean_latency_ms"]),
                optional_tokens_per_second(
                    &card["qualification"]["estimated_tokens_per_second_milli"]
                ),
                optional_u64(&card["qualification"]["maximum_reliable_context_tokens"]),
                optional_u64(&card["score"])
            ),
        );
        for (label, key) in [
            ("  Risks", "risks"),
            ("  Missing evidence", "missing_evidence"),
            ("  Not recommended", "not_recommended_reasons"),
        ] {
            let details = notice_array(&card[key]);
            if !details.is_empty() {
                push_line(&mut output, format_args!("{label} · {details}"));
            }
        }
    }
    output.trim_end().to_owned()
}

fn format_qualification_report(value: &Value) -> String {
    let report = &value["qualification"];
    let model = format!(
        "{}/{}",
        text_or(&report["model"]["provider"], "unknown"),
        text_or(&report["model"]["model"], "unknown")
    );
    let mut output = String::new();
    push_line(
        &mut output,
        format_args!("Real provider qualification · {model}"),
    );
    push_line(
        &mut output,
        format_args!(
            "Accuracy {:.1}% · mean latency {:.0} ms · throughput {} · reliable context {} tokens",
            report["accuracy"].as_f64().unwrap_or_default() * 100.0,
            report["mean_latency_ms"].as_f64().unwrap_or_default(),
            report["estimated_tokens_per_second"]
                .as_f64()
                .map(|value| format!("{value:.1} tok/s"))
                .unwrap_or_else(|| "not observed".into()),
            optional_u64(&report["maximum_reliable_context_tokens"])
        ),
    );
    if let Some(cases) = report["cases"].as_array() {
        for case in cases {
            push_line(
                &mut output,
                format_args!(
                    "  {} · {} · {} ms",
                    text_or(&case["name"], "unnamed case"),
                    if case["passed"].as_bool() == Some(true) {
                        "passed"
                    } else {
                        "failed"
                    },
                    optional_u64(&case["latency_ms"])
                ),
            );
        }
    }
    push_line(
        &mut output,
        format_args!(
            "Recommended roles · {}",
            text_array(&report["recommended_roles"])
        ),
    );
    push_line(
        &mut output,
        format_args!(
            "Not recommended roles · {}",
            text_array(&report["not_recommended_roles"])
        ),
    );
    match value["recommendations"]["outcome"]["status"].as_str() {
        Some("recommended") => push_line(
            &mut output,
            format_args!(
                "Updated recommendation · {} · score {}",
                text_or(&value["recommendations"]["outcome"]["model"], "unknown"),
                optional_u64(&value["recommendations"]["outcome"]["score"])
            ),
        ),
        Some("no_recommendation") => push_line(
            &mut output,
            format_args!(
                "Updated recommendation · none ({})",
                enum_array(&value["recommendations"]["outcome"]["reasons"])
            ),
        ),
        _ => {}
    }
    output.trim_end().to_owned()
}

fn push_line(output: &mut String, arguments: std::fmt::Arguments<'_>) {
    output
        .write_fmt(arguments)
        .expect("writing formatted evidence to a String cannot fail");
    output.push('\n');
}

fn text_or<'a>(value: &'a Value, fallback: &'a str) -> &'a str {
    value.as_str().unwrap_or(fallback)
}

fn pretty_enum(value: &str) -> String {
    value.replace('_', " ")
}

fn optional_u64(value: &Value) -> String {
    value
        .as_u64()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "not observed".into())
}

fn optional_bool(value: &Value) -> &'static str {
    match value.as_bool() {
        Some(true) => "yes",
        Some(false) => "no",
        None => "not observed",
    }
}

fn optional_bytes(value: &Value) -> String {
    value.as_u64().map_or_else(
        || "not observed".into(),
        |bytes| {
            if bytes >= 1024 * 1024 * 1024 {
                format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
            } else {
                format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
            }
        },
    )
}

fn compact_count(value: &Value) -> String {
    value.as_u64().map_or_else(
        || "not observed".into(),
        |count| {
            if count >= 1_000_000_000 {
                format!("{:.1}B", count as f64 / 1_000_000_000.0)
            } else if count >= 1_000_000 {
                format!("{:.1}M", count as f64 / 1_000_000.0)
            } else {
                count.to_string()
            }
        },
    )
}

fn basis_points(value: &Value) -> String {
    value
        .as_u64()
        .map(|basis_points| format!("{:.1}%", basis_points as f64 / 100.0))
        .unwrap_or_else(|| "not observed".into())
}

fn optional_milliseconds(value: &Value) -> String {
    value
        .as_u64()
        .map(|milliseconds| format!("{milliseconds} ms"))
        .unwrap_or_else(|| "not observed".into())
}

fn optional_tokens_per_second(value: &Value) -> String {
    value
        .as_u64()
        .map(|milli| format!("{:.1} tok/s", milli as f64 / 1_000.0))
        .unwrap_or_else(|| "not observed".into())
}

fn text_array(value: &Value) -> String {
    value.as_array().map_or_else(
        || "none observed".into(),
        |items| {
            let values = items.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            if values.is_empty() {
                "none".into()
            } else {
                values.join(", ")
            }
        },
    )
}

fn enum_array(value: &Value) -> String {
    value.as_array().map_or_else(
        || "not observed".into(),
        |items| {
            let values = items
                .iter()
                .filter_map(Value::as_str)
                .map(pretty_enum)
                .collect::<Vec<_>>();
            if values.is_empty() {
                "none".into()
            } else {
                values.join(", ")
            }
        },
    )
}

fn notice_array(value: &Value) -> String {
    value.as_array().map_or_else(String::new, |items| {
        items
            .iter()
            .map(|notice| {
                let code = pretty_enum(text_or(&notice["code"], "unclassified"));
                match notice["detail"].as_str() {
                    Some(detail) => format!("{code}: {detail}"),
                    None => code,
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_palette_searches_labels_descriptions_and_commands() {
        assert_eq!(filtered_actions("/diff")[0].label, "Review diff");
        assert_eq!(
            filtered_actions("/model recommend")[0].label,
            "Recommend local model"
        );
        assert_eq!(
            filtered_actions("/model qualify")[0].label,
            "Qualify local model"
        );
        assert!(filtered_actions("provider")
            .iter()
            .all(|action| action.matches("provider")));
        assert!(filtered_actions("exact pending")
            .iter()
            .any(|action| action.id.as_str() == "approval.approve"));
    }

    /// Every verb the discovery surfaces advertise must reach a real match arm.
    /// The dispatcher's fallback reports registered-but-unhandled verbs
    /// distinctly, which is exactly what this asserts against.
    #[tokio::test]
    async fn every_registered_verb_reaches_a_handler() {
        for verb in DISPATCH_COMMANDS {
            let mut app = crate::test_fixtures::offline_app();
            CommandPalette::new()
                .execute(&mut app, &format!("/{verb}"))
                .await;
            assert!(
                !app.message_bar.contains("registered but has no handler"),
                "/{verb} is advertised but has no dispatcher arm"
            );
            assert!(
                !app.message_bar.contains("Unknown command"),
                "/{verb} was rejected as unknown: {}",
                app.message_bar
            );
        }
    }

    #[test]
    fn recommendation_card_renders_only_observed_evidence_and_gaps() {
        let recommendations = serde_json::json!({
            "provider": "ollama",
            "observed_at": "2026-07-26T10:00:00Z",
            "report": {
                "outcome": {
                    "status": "no_recommendation",
                    "reasons": ["evidence_incomplete"]
                },
                "constraints": {
                    "single_model_mode": true,
                    "context_limit_tokens": 8192,
                    "maximum_parallel_requests": 1,
                    "allow_separate_local_judge": false,
                    "lifecycle": "unload_after_request",
                    "idle_timeout_seconds": null
                },
                "resource_risks": [{
                    "code": "low_memory_system",
                    "detail": "physical memory is constrained"
                }],
                "cards": [{
                    "model": "misleading-super-coder-name:latest",
                    "installed": true,
                    "currently_loaded": false,
                    "parameter_count": 7_000_000_000_u64,
                    "quantization": "Q4_K_M",
                    "quantization_bits": 4,
                    "metadata_context_tokens": 32768,
                    "observed_size_bytes": 4_500_000_000_u64,
                    "estimated_runtime_memory_bytes": 6_000_000_000_u64,
                    "recommended_context_tokens": 8192,
                    "qualification": {
                        "assessment": "missing",
                        "coding": "unverified",
                        "passed_cases": null,
                        "total_cases": null,
                        "accuracy_basis_points": null,
                        "structured_json": "not_tested",
                        "tool_calling": "not_tested",
                        "mean_latency_ms": null,
                        "estimated_tokens_per_second_milli": null,
                        "maximum_reliable_context_tokens": null
                    },
                    "score": null,
                    "status": "not_recommended",
                    "risks": [],
                    "missing_evidence": [{
                        "code": "qualification",
                        "detail": "real qualification has not been run"
                    }],
                    "not_recommended_reasons": [{
                        "code": "qualification_missing",
                        "detail": "a verified recommendation requires evidence"
                    }]
                }]
            }
        });
        let status = serde_json::json!({
            "resources": {
                "total_memory_bytes": 8_589_934_592_u64,
                "available_memory_bytes": 2_147_483_648_u64,
                "loaded_model_bytes": 0,
                "used_swap_bytes": 536_870_912_u64,
                "memory_pressure": "elevated",
                "maximum_local_inference_requests": 1,
                "allow_separate_local_judge": false
            }
        });

        let rendered = format_recommendation_report(&recommendations, &status);
        assert!(rendered.contains("physical 8.0 GiB"));
        assert!(rendered.contains("coding unverified"));
        assert!(rendered.contains("Missing evidence · qualification"));
        assert!(rendered.contains("Not recommended · qualification missing"));
        assert!(!rendered.contains("coding strong"));
    }

    #[test]
    fn qualification_card_shows_real_case_results_and_updated_outcome() {
        let value = serde_json::json!({
            "qualification": {
                "model": {"provider": "ollama", "model": "coder:7b"},
                "cases": [{
                    "name": "structured-output",
                    "passed": true,
                    "latency_ms": 42,
                    "detail": "observed"
                }],
                "accuracy": 1.0,
                "mean_latency_ms": 42.0,
                "estimated_tokens_per_second": 12.5,
                "maximum_reliable_context_tokens": 4096,
                "recommended_roles": ["coding_worker"],
                "not_recommended_roles": ["judge"]
            },
            "recommendations": {
                "outcome": {
                    "status": "recommended",
                    "model": "coder:7b",
                    "score": 812
                }
            }
        });

        let rendered = format_qualification_report(&value);
        assert!(rendered.contains("Real provider qualification · ollama/coder:7b"));
        assert!(rendered.contains("structured-output · passed · 42 ms"));
        assert!(rendered.contains("Updated recommendation · coder:7b · score 812"));
    }
}
