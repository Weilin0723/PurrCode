//! SSE streaming controller for assistant responses.

use tokio::sync::mpsc;

pub enum StreamEvent {
    Delta(String),
    ToolCall(serde_json::Value),
    Usage { input: u64, output: u64 },
    Done,
    Error(String),
}

pub struct StreamController {
    pub active: bool,
    pub receiver: Option<mpsc::Receiver<StreamEvent>>,
}

impl StreamController {
    pub fn new() -> Self {
        Self {
            active: false,
            receiver: None,
        }
    }

    pub fn start(&mut self) -> mpsc::Sender<StreamEvent> {
        let (tx, rx) = mpsc::channel(128);
        self.active = true;
        self.receiver = Some(rx);
        tx
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.receiver = None;
    }
}
