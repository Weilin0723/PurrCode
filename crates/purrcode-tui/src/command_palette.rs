//! Slash command dispatch. All commands call daemon API; no second execution path.

use crate::app::{App, AppMode};
use crate::provider_setup::ProviderSetup;
use crate::skill_browser::SkillBrowser;

pub struct CommandPalette;

impl CommandPalette {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(&self, app: &mut App, input: &str) {
        let input = input.trim().to_string();
        let (cmd, args) = input
            .strip_prefix('/')
            .map(|s| {
                let parts: Vec<&str> = s.splitn(2, ' ').collect();
                (parts[0].to_lowercase(), parts.get(1).copied().unwrap_or(""))
            })
            .unwrap_or_default();

        match cmd.as_str() {
            "help" => {
                app.message_bar = "/help /connect /providers /models /model <id> /role <role> <provider/model> /privacy /plan /build /review /diff /approve /deny <reason> /pause /resume /rollback /skills /skill-search <query> /skill-block <publisher> /settings /session /new /compact /cancel /quit".into();
            }
            "connect" => {
                app.provider_setup = Some(ProviderSetup::new());
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
            "models" | "model" => {
                match app.request(reqwest::Method::GET, "/v1/models", None).await {
                    Ok(val) => {
                        if cmd == "model" && !args.is_empty() {
                            app.status_bar.set_model(args);
                            if let Some(id) = &app.session_id {
                                let _ = app
                                    .request(
                                        reqwest::Method::POST,
                                        &format!("/v1/sessions/{id}/model"),
                                        Some(serde_json::json!({"model": args})),
                                    )
                                    .await;
                            }
                        }
                        app.message_bar = format!("Models: {val}");
                    }
                    Err(e) => app.message_bar = format!("Error: {e}"),
                }
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
            "skill-search" => {
                let query = if args.is_empty() { "all" } else { args };
                app.message_bar = format!("Searching skills for: {query}...");
                let token = app.token.clone();
                let daemon_url = app.daemon_url().to_string();
                let session_id = app.session_id.clone();
                let client = reqwest::Client::new();
                app.skill_browser = Some(SkillBrowser::new());
                app.switch_mode(AppMode::SkillBrowse);
                if let Some(ref mut browser) = app.skill_browser {
                    browser
                        .search(&client, &daemon_url, &token, query, session_id.as_deref())
                        .await;
                }
            }
            "diff" => {
                if let Some(id) = &app.session_id {
                    match app
                        .request(reqwest::Method::GET, &format!("/v1/sessions/{id}"), None)
                        .await
                    {
                        Ok(session) => {
                            app.diff_view = Some(crate::diff_view::DiffView {
                                content: serde_json::to_string_pretty(&session).unwrap_or_default(),
                            });
                            app.switch_mode(AppMode::DiffView);
                        }
                        Err(e) => app.message_bar = format!("Error: {e}"),
                    }
                } else {
                    app.message_bar = "No active session.".into();
                }
            }
            "privacy" => {
                if app.status_bar.privacy == "local-only" {
                    app.status_bar.set_privacy("mixed");
                    app.message_bar = "Privacy mode: mixed (web search allowed)".into();
                } else {
                    app.status_bar.set_privacy("local-only");
                    app.message_bar = "Privacy mode: local-only".into();
                }
            }
            "plan" => {
                app.conversation.mode = purrcode_runtime_core::ConversationMode::Plan;
                app.status_bar.set_mode("plan");
                app.message_bar = "Switched to plan mode.".into();
            }
            "build" => {
                app.conversation.mode = purrcode_runtime_core::ConversationMode::Build;
                app.status_bar.set_mode("build");
                app.message_bar = "Switched to build mode.".into();
            }
            "review" => {
                app.conversation.mode = purrcode_runtime_core::ConversationMode::Review;
                app.status_bar.set_mode("review");
                app.message_bar = "Switched to review mode.".into();
            }
            "new" => {
                app.session_id = None;
                app.conversation = crate::conversation::Conversation::new();
                app.message_bar = "New session started.".into();
            }
            "session" => {
                if let Some(id) = &app.session_id {
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
            "settings" => {
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
            "approve" | "resume" | "rollback" | "pause" | "deny" => {
                let Some(id) = app.session_id.clone() else {
                    app.message_bar = "No active session.".into();
                    return;
                };
                let (endpoint, body) = match cmd.as_str() {
                    "approve" => ("approve", serde_json::json!({})),
                    "resume" => ("resume", serde_json::json!({})),
                    "rollback" => ("rollback", serde_json::json!({})),
                    "pause" => ("pause", serde_json::json!({"reason": "paused from TUI"})),
                    _ => (
                        "reject",
                        serde_json::json!({"reason": if args.is_empty() { "denied from TUI" } else { args }}),
                    ),
                };
                match app
                    .request(
                        reqwest::Method::POST,
                        &format!("/v1/sessions/{id}/{endpoint}"),
                        Some(body),
                    )
                    .await
                {
                    Ok(_) => app.message_bar = format!("Session command accepted: {cmd}"),
                    Err(error) => app.message_bar = format!("Error: {error}"),
                }
            }
            "cancel" => {
                app.conversation.cancel_streaming();
                app.stream.stop();
                app.message_bar = "Cancelled.".into();
            }
            "quit" => {
                app.message_bar = "Goodbye.".into();
                app.quit_requested = true;
            }
            _ => {
                app.message_bar = format!("Unknown command: /{cmd}. Type /help for commands.");
            }
        }
    }
}
