//! Background filesystem watcher for session worktrees.
//!
//! The daemon owns the isolated worktrees, so it is the natural place to
//! detect external edits — a human editing a file outside PurrCode, or a
//! build tool regenerating files. When an external change lands while the
//! session is not executing, the watcher records an `ExternalChangeDetected`
//! audit event and pushes a live-stream message so clients refresh instead of
//! polling.

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use purrcode_ninelives::SessionStore;
use purrcode_runtime_core::{SessionEvent, SessionId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::{AppState, LiveStreamHub};

/// How often the watcher re-scans sessions for newly created worktrees.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);

/// Serializes the set of paths being watched. Each `RecommendedWatcher` must
/// stay alive for the watch to remain active, so it is stored here and only
/// dropped when the session's worktree is dropped.
#[derive(Default)]
struct WatchRegistry {
    active: BTreeMap<SessionId, (PathBuf, RecommendedWatcher)>,
}

pub async fn run_worktree_watcher(state: AppState) {
    let registry = Arc::new(Mutex::new(WatchRegistry::default()));
    let mut interval = tokio::time::interval(RESCAN_INTERVAL);
    loop {
        interval.tick().await;
        reconcile_watches(&state, &registry).await;
    }
}

/// Ensures every session with an isolated worktree is watched, starting new
/// watches and dropping finished ones.
async fn reconcile_watches(state: &AppState, registry: &Arc<Mutex<WatchRegistry>>) {
    let active: BTreeMap<SessionId, PathBuf> = {
        let store = match SessionStore::open(&state.database) {
            Ok(store) => store,
            Err(_) => return,
        };
        let mut map = BTreeMap::new();
        let session_ids = match store.list_session_ids() {
            Ok(ids) => ids,
            Err(_) => return,
        };
        for session_id in session_ids {
            let session = match store.load(session_id) {
                Ok(session) => session,
                Err(_) => continue,
            };
            let Some(worktree) = session.worktree.as_ref() else {
                continue;
            };
            if worktree.is_dir() {
                map.insert(session_id, worktree.clone());
            }
        }
        map
    };
    let mut registry = registry.lock().await;
    // Start watches for new worktrees.
    for (session_id, path) in &active {
        if registry
            .active
            .get(session_id)
            .is_some_and(|(watched, _)| watched == path)
        {
            continue;
        }
        if let Some((watcher, rx)) = spawn_watch(path) {
            let watch_state = state.clone();
            let watch_session = *session_id;
            let watch_path = path.clone();
            tokio::spawn(async move {
                consume_events(watch_state, watch_session, watch_path, rx).await;
            });
            registry.active.insert(*session_id, (path.clone(), watcher));
        }
    }
    // Drop finished sessions' watches.
    registry
        .active
        .retain(|session_id, _| active.contains_key(session_id));
}

fn spawn_watch(path: &Path) -> Option<(RecommendedWatcher, tokio::sync::mpsc::Receiver<Event>)> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(256);
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            if let Ok(event) = result {
                let _ = tx.try_send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )
    .ok()?;
    watcher.watch(path, RecursiveMode::Recursive).ok()?;
    Some((watcher, rx))
}

/// Consumes filesystem events for one session worktree and reports external
/// changes. Change events caused by the agent's own execution are not
/// distinguishable at this layer, so the watcher only reports when the
/// session is not executing (no active lease).
async fn consume_events(
    state: AppState,
    session_id: SessionId,
    worktree: PathBuf,
    mut rx: tokio::sync::mpsc::Receiver<Event>,
) {
    // Accumulate events over a fixed debounce window so a burst (a build
    // regenerating a directory) collapses into one ExternalChangeDetected.
    let mut pending: Vec<PathBuf> = Vec::new();
    loop {
        let Some(event) = rx.recv().await else {
            return;
        };
        let relevant = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        );
        if relevant {
            for path in event.paths {
                if let Ok(relative) = path.strip_prefix(&worktree) {
                    if relative.components().count() > 0
                        && !pending.contains(&relative.to_path_buf())
                    {
                        pending.push(relative.to_path_buf());
                    }
                }
            }
        }
        // Wait out the debounce window. A second event resets it.
        match tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
            Ok(Some(_)) => continue,
            Ok(None) => {
                report_external_change(&state, session_id, &mut pending).await;
                return;
            }
            Err(_) => {
                report_external_change(&state, session_id, &mut pending).await;
            }
        }
    }
}

async fn report_external_change(
    state: &AppState,
    session_id: SessionId,
    pending: &mut Vec<PathBuf>,
) {
    if pending.is_empty() {
        return;
    }
    // Only report when the agent is not currently executing.
    if state.leases.lock().await.contains_key(&session_id) {
        pending.clear();
        return;
    }
    let changed = std::mem::take(pending);
    let mut store = match SessionStore::open(&state.database) {
        Ok(store) => store,
        Err(_) => {
            *pending = changed;
            return;
        }
    };
    if store
        .append(
            session_id,
            &SessionEvent::ExternalChangeDetected {
                changed_files: changed.clone(),
            },
        )
        .is_ok()
    {
        if let Some(hub) = state.live_streams.lock().await.get(&session_id).cloned() {
            hub.publish_external_change(&changed).await;
        }
    }
}

impl LiveStreamHub {
    /// Publishes an external-change message to live SSE subscribers.
    pub async fn publish_external_change(&self, changed_files: &[PathBuf]) {
        let value = serde_json::json!({
            "kind": "external_change",
            "changed_files": changed_files.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
        });
        let _ = self.sender.send(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tokio::sync::Mutex as TokioMutex;

    fn test_state(database: &Path) -> AppState {
        AppState {
            store: std::sync::Arc::new(Mutex::new(SessionStore::open(database).unwrap())),
            unavailable_sessions: std::sync::Arc::new(BTreeMap::new()),
            bearer_token: std::sync::Arc::from("test"),
            database: database.to_path_buf(),
            app_config: database.with_file_name("config.toml"),
            leases: std::sync::Arc::new(TokioMutex::new(BTreeMap::new())),
            lifecycle_epochs: std::sync::Arc::new(TokioMutex::new(BTreeMap::new())),
            lifecycle_gate: std::sync::Arc::new(TokioMutex::new(())),
            active_models: std::sync::Arc::new(TokioMutex::new(BTreeMap::new())),
            local_inference_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            local_inference_limit: 1,
            interrupting_sessions: std::sync::Arc::new(TokioMutex::new(BTreeMap::new())),
            pull_jobs: std::sync::Arc::new(TokioMutex::new(BTreeMap::new())),
            live_streams: std::sync::Arc::new(TokioMutex::new(BTreeMap::new())),
            supervisor_runs: std::sync::Arc::new(TokioMutex::new(BTreeMap::new())),
            lsp: std::sync::Arc::new(TokioMutex::new(
                purrcode_lsp::LspManager::new(purrcode_lsp::default_server_commands()),
            )),
            terminals: crate::TerminalRuntime::default(),
        }
    }

    #[tokio::test]
    async fn external_change_is_recorded_as_an_audit_event() {
        let temporary = tempfile::tempdir().unwrap();
        let state = test_state(&temporary.path().join("sessions.db"));
        let session_id = SessionId::new();
        state
            .store
            .lock()
            .await
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "watch me".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();

        // Not executing → the change is recorded as an audit event.
        let mut pending = vec![PathBuf::from("src/auth.rs")];
        report_external_change(&state, session_id, &mut pending).await;
        assert!(pending.is_empty());
        let session = state.store.lock().await.load(session_id).unwrap();
        assert!(
            session.recent_context_ledger.is_empty() && session.event_count >= 2,
            "an ExternalChangeDetected event should have been appended"
        );

        // An empty pending set is a no-op (debounce produced no paths).
        let mut pending = Vec::new();
        report_external_change(&state, session_id, &mut pending).await;
        let session = state.store.lock().await.load(session_id).unwrap();
        assert_eq!(session.event_count, 2);
    }
}
