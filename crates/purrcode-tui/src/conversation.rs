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
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            streaming_message: None,
            pending_action: None,
            mode: ConversationMode::Build,
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
            if !msg.content.is_empty() {
                self.messages.push(msg);
            }
        }
    }

    pub fn cancel_streaming(&mut self) {
        self.streaming_message = None;
    }

    pub async fn refresh_events(
        &mut self,
        _daemon_url: &str,
        _token: &str,
        _session_id: Option<String>,
    ) {
        // TODO: poll /v1/sessions/{id}/events and update pending_action, etc.
    }

    pub fn current_objective(&self) -> String {
        self.messages
            .first()
            .map(|m| m.content.chars().take(80).collect())
            .unwrap_or_default()
    }
}
