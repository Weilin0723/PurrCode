//! Conversation state, message types, and event polling.

use chrono::{DateTime, Utc};
use purrcode_runtime_core::ConversationMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub model: Option<String>,
}

pub struct Conversation {
    pub messages: Vec<Message>,
    pub streaming_message: Option<Message>,
    pub pending_action: Option<Value>,
    pub mode: ConversationMode,
    pub scroll: usize,
    pub evidence: Vec<String>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            streaming_message: None,
            pending_action: None,
            mode: ConversationMode::Build,
            scroll: 0,
            evidence: Vec::new(),
        }
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: "user".into(),
            content: content.to_string(),
            timestamp: Utc::now(),
            model: None,
        });
    }

    pub fn start_streaming(&mut self, model: Option<String>) {
        self.streaming_message = Some(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: "assistant".into(),
            content: String::new(),
            timestamp: Utc::now(),
            model,
        });
    }

    pub fn append_streaming(&mut self, delta: &str) {
        if let Some(msg) = &mut self.streaming_message {
            msg.content.push_str(delta);
        }
    }

    pub fn finalize_streaming(&mut self) {
        if let Some(msg) = self.streaming_message.take() {
            if !msg.content.is_empty()
                && !self
                    .messages
                    .iter()
                    .any(|existing| existing.role == msg.role && existing.content == msg.content)
            {
                self.messages.push(msg);
            }
        }
    }

    pub fn cancel_streaming(&mut self) {
        self.streaming_message = None;
    }

    pub async fn refresh_events(
        &mut self,
        daemon_url: &str,
        token: &str,
        session_id: Option<String>,
    ) {
        let Some(session_id) = session_id else {
            return;
        };
        let url = format!(
            "{}/v1/sessions/{session_id}/messages",
            daemon_url.trim_end_matches('/')
        );
        if let Ok(response) = reqwest::Client::new()
            .get(url)
            .bearer_auth(token)
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(messages) = response.json::<Vec<Message>>().await {
                    self.messages = messages;
                }
            }
        }
        let events_url = format!(
            "{}/v1/sessions/{session_id}/events",
            daemon_url.trim_end_matches('/')
        );
        if let Ok(response) = reqwest::Client::new()
            .get(events_url)
            .bearer_auth(token)
            .send()
            .await
        {
            if let Ok(events) = response.json::<Vec<Value>>().await {
                self.pending_action = events.iter().rev().find_map(|event| {
                    (event["event"] == "action_proposed")
                        .then(|| event.pointer("/data/action").cloned())
                        .flatten()
                });
                self.evidence = events
                    .iter()
                    .filter_map(|event| match event["event"].as_str() {
                        Some("judgment_recorded") => Some(format!(
                            "Judgment: {}",
                            event.pointer("/data/decision").unwrap_or(&Value::Null)
                        )),
                        Some("validation_recorded") => Some(format!(
                            "Validation: {} — {}",
                            event
                                .pointer("/data/status")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown"),
                            event
                                .pointer("/data/evidence")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                        )),
                        _ => None,
                    })
                    .rev()
                    .take(3)
                    .collect();
            }
        }
    }

    pub fn current_objective(&self) -> String {
        self.messages
            .first()
            .map(|message| message.content.clone())
            .unwrap_or_default()
    }
}
