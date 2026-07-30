use super::*;

#[test]
fn rejected_response_preview_is_bounded_and_removes_terminal_controls() {
    let input = format!(
        "before\u{1b}[2J{}",
        "x".repeat(MAX_REJECTED_RESPONSE_PREVIEW_CHARS + 10)
    );
    let preview = safe_rejected_response_preview(&input, 2);
    assert!(preview.contains("attempt 2"));
    assert!(!preview.contains('\u{1b}'));
    assert!(preview.contains("output truncated"));
}

use async_trait::async_trait;
use futures::stream;
use futures::StreamExt;
use purrcode_ninelives::SessionStore;
use purrcode_pawgate::Policy;
use purrcode_provider_gateway::{
    ModelCapabilities, ModelEvent, ModelEventStream, ModelId, ModelProvider, ModelRequest,
    ProviderError, ProviderEventStream, ProviderHealth, ProviderStreamEvent, StreamPhase,
    TokenEstimate,
};
use purrcode_runtime_core::{
    ActionId, ApprovalAuthority, ConversationMessage, ProposedAction, SessionEvent, SessionId,
    SessionStatus, ValidationStatus, WriteFileAction,
};
use purrcode_whisker::{
    IndexLifecycleStage, IndexPauseReason, IndexStopReason, IndexingSignals, MemoryPressure,
    RetrievalBudget, Tier0Budget, Tier1Budget, Tier2Policy, Tier2Status,
};
use schemars::schema::RootSchema;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::context::{
    task_related_paths, task_tier1_request, AgentContextIndex, AgentContextIndexError,
    AgentContextPolicy, MAX_TASK_CONTEXT_FILENAME_TERMS, MAX_TASK_CONTEXT_PATH_HINTS,
};
use crate::stream::{
    bounded_agent_stream_channel, is_unsafe_terminal_control, AgentStreamEvent,
    AgentStreamObserverError, AgentStreamReceiver, RationaleStreamExtractor, TopLevelJsonState,
    MAX_STREAMED_RATIONALE_BYTES, MAX_STREAM_OBSERVER_CAPACITY,
};
use crate::AgentError;
struct MockProvider {
    responses: Mutex<Vec<Value>>,
}

struct StreamingProvider {
    streams: Mutex<Vec<Vec<Result<ProviderStreamEvent, ProviderError>>>>,
    remain_pending: bool,
}

#[async_trait]
impl ModelProvider for MockProvider {
    async fn capabilities(&self, _model: &ModelId) -> Result<ModelCapabilities, ProviderError> {
        Ok(ModelCapabilities::unknown(true))
    }
    async fn stream(&self, _request: ModelRequest) -> Result<ModelEventStream, ProviderError> {
        Ok(Box::pin(stream::empty()))
    }
    async fn structured(
        &self,
        _request: ModelRequest,
        _schema: RootSchema,
    ) -> Result<Value, ProviderError> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ProviderError::InvalidResponse("mock exhausted".into()))
    }
    async fn count_tokens(&self, _request: &ModelRequest) -> Result<TokenEstimate, ProviderError> {
        Ok(TokenEstimate {
            tokens: 1,
            exact: true,
        })
    }
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth {
            available: true,
            detail: "mock".into(),
        })
    }
}

#[async_trait]
impl ModelProvider for StreamingProvider {
    async fn capabilities(&self, _model: &ModelId) -> Result<ModelCapabilities, ProviderError> {
        Ok(ModelCapabilities::unknown(true))
    }

    async fn stream(&self, _request: ModelRequest) -> Result<ModelEventStream, ProviderError> {
        Ok(Box::pin(stream::empty()))
    }

    async fn structured(
        &self,
        _request: ModelRequest,
        _schema: RootSchema,
    ) -> Result<Value, ProviderError> {
        Err(ProviderError::InvalidResponse(
            "non-stream structured path was called".into(),
        ))
    }

    async fn structured_stream(
        &self,
        _request: ModelRequest,
        _schema: RootSchema,
    ) -> Result<ProviderEventStream, ProviderError> {
        let events = self
            .streams
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ProviderError::InvalidResponse("stream mock exhausted".into()))?;
        if self.remain_pending {
            Ok(Box::pin(stream::iter(events).chain(stream::pending())))
        } else {
            Ok(Box::pin(stream::iter(events)))
        }
    }

    async fn count_tokens(&self, _request: &ModelRequest) -> Result<TokenEstimate, ProviderError> {
        Ok(TokenEstimate {
            tokens: 1,
            exact: true,
        })
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth {
            available: true,
            detail: "stream mock".into(),
        })
    }
}

fn observed_turn_json() -> String {
    serde_json::json!({
        "plan": ["write isolated file", "validate"],
        "current_step_index": 0,
        "expected_postconditions": ["new.txt exists"],
        "rationale": "implement objective",
        "action": {
            "type": "write_file",
            "path": "new.txt",
            "content": "created",
            "expected_digest": null
        },
        "complete": false
    })
    .to_string()
}

fn successful_observed_stream(output: &str) -> Vec<Result<ProviderStreamEvent, ProviderError>> {
    let split = output.len() / 2;
    vec![
        Ok(ProviderStreamEvent::Connected),
        Ok(ProviderStreamEvent::BytesReceived {
            byte_count: output.len(),
        }),
        Ok(ProviderStreamEvent::Model(ModelEvent::TextDelta(
            output[..split].into(),
        ))),
        Ok(ProviderStreamEvent::Model(ModelEvent::TextDelta(
            output[split..].into(),
        ))),
        Ok(ProviderStreamEvent::Model(ModelEvent::Usage {
            input_tokens: 17,
            output_tokens: 9,
        })),
        Ok(ProviderStreamEvent::Model(ModelEvent::Finished)),
    ]
}

fn drain_observer(receiver: &mut AgentStreamReceiver) -> Vec<AgentStreamEvent> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    events
}

fn repository() -> tempfile::TempDir {
    let repository = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(repository.path())
        .status()
        .unwrap()
        .success());
    std::fs::write(repository.path().join("README.md"), "base").unwrap();
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repository.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-c",
            "user.name=PurrCode",
            "-c",
            "user.email=test@local.invalid",
            "commit",
            "-q",
            "-m",
            "base",
        ])
        .current_dir(repository.path())
        .status()
        .unwrap()
        .success());
    repository
}

#[test]
fn startup_prepares_only_tier0_then_task_indexes_relevant_paths_once() {
    let repository = tempfile::tempdir().unwrap();
    std::fs::write(repository.path().join("Cargo.toml"), "manifest_only_token").unwrap();
    std::fs::create_dir_all(repository.path().join("src")).unwrap();
    std::fs::write(
        repository.path().join("src/relevant.rs"),
        "pub fn relevant_task_token() {}",
    )
    .unwrap();
    std::fs::write(
        repository.path().join("src/unrelated.rs"),
        "pub fn unrelated_task_token() {}",
    )
    .unwrap();
    let database = repository.path().join(".purrcode").join("context.db");
    let mut context = AgentContextIndex::open(repository.path(), &database).unwrap();

    let startup = context.prepare_startup(&Tier0Budget::default()).unwrap();
    assert!(startup.rebuilt);
    assert_eq!(
        context.lifecycle_stage().unwrap(),
        IndexLifecycleStage::Tier0Ready
    );
    assert!(context
        .retrieve("manifest_only_token", &RetrievalBudget::default())
        .unwrap()
        .iter()
        .any(|hit| hit.path == Path::new("Cargo.toml")));
    assert!(context
        .retrieve("relevant_task_token", &RetrievalBudget::default())
        .unwrap()
        .is_empty());
    assert!(matches!(
        context.begin_tier2(Tier2Policy::default()),
        Err(AgentContextIndexError::TaskRequiredForTier2)
    ));

    let task = context
        .submit_task(
            "Update `src/relevant.rs` for the relevant behavior.",
            &[],
            &AgentContextPolicy::default(),
        )
        .unwrap();
    assert!(!task.tier0_rebuilt);
    assert!(task
        .tier1
        .selected_paths
        .contains(&PathBuf::from("src/relevant.rs")));
    assert_eq!(
        context.lifecycle_stage().unwrap(),
        IndexLifecycleStage::TaskReady
    );
    assert!(context
        .retrieve("relevant_task_token", &RetrievalBudget::default())
        .unwrap()
        .iter()
        .any(|hit| hit.path == Path::new("src/relevant.rs")));
    assert!(context
        .retrieve("unrelated_task_token", &RetrievalBudget::default())
        .unwrap()
        .is_empty());

    drop(context);
    let mut reopened = AgentContextIndex::open(repository.path(), &database).unwrap();
    let preserved = reopened.prepare_startup(&Tier0Budget::default()).unwrap();
    assert!(!preserved.rebuilt);
    assert_eq!(preserved.stage, IndexLifecycleStage::TaskReady);
    assert!(reopened
        .retrieve("relevant_task_token", &RetrievalBudget::default())
        .unwrap()
        .iter()
        .any(|hit| hit.path == Path::new("src/relevant.rs")));
}

#[test]
fn caller_owned_tier2_pauses_and_cancels_without_unbounded_steps() {
    let repository = tempfile::tempdir().unwrap();
    std::fs::write(repository.path().join("Cargo.toml"), "manifest").unwrap();
    for file in 0..12 {
        std::fs::write(
            repository.path().join(format!("source-{file}.rs")),
            format!("pub fn background_{file}() {{}}"),
        )
        .unwrap();
    }
    let database = repository.path().join(".purrcode").join("context.db");
    let mut context = AgentContextIndex::open(repository.path(), &database).unwrap();
    context
        .submit_task("Inspect source-0.rs", &[], &AgentContextPolicy::default())
        .unwrap();
    let policy = Tier2Policy {
        maximum_entries_per_step: 4,
        maximum_files_per_step: 1,
        maximum_bytes_per_step: 1024,
        maximum_total_entries: 64,
        maximum_total_files: 16,
        maximum_total_bytes: 16 * 1024,
        maximum_file_bytes: 1024,
        pause_at_input_latency_millis: 50,
    };
    let mut work = context.begin_tier2(policy.clone()).unwrap();

    for (signals, expected) in [
        (
            IndexingSignals {
                memory_pressure: MemoryPressure::High,
                ..IndexingSignals::default()
            },
            IndexPauseReason::HighMemoryPressure,
        ),
        (
            IndexingSignals {
                generation_active: true,
                ..IndexingSignals::default()
            },
            IndexPauseReason::GenerationActive,
        ),
        (
            IndexingSignals {
                input_latency_millis: policy.pause_at_input_latency_millis,
                ..IndexingSignals::default()
            },
            IndexPauseReason::InputLatency,
        ),
    ] {
        let paused = context.drive_tier2(&mut work, signals).unwrap();
        assert_eq!(paused.status, Tier2Status::Paused(expected));
        assert_eq!(paused.examined_entries, 0);
        assert_eq!(paused.indexed_files, 0);
    }

    let step = context
        .drive_tier2(&mut work, IndexingSignals::default())
        .unwrap();
    assert!(step.examined_entries <= policy.maximum_entries_per_step);
    assert!(step.indexed_files <= policy.maximum_files_per_step);
    assert!(step.indexed_bytes <= policy.maximum_bytes_per_step);
    let cancelled = context
        .drive_tier2(
            &mut work,
            IndexingSignals {
                cancel_requested: true,
                ..IndexingSignals::default()
            },
        )
        .unwrap();
    assert_eq!(cancelled.status, Tier2Status::Cancelled);
    assert_eq!(
        context
            .drive_tier2(&mut work, IndexingSignals::default())
            .unwrap()
            .status,
        Tier2Status::Cancelled
    );

    let mut critical_work = context.begin_tier2(policy).unwrap();
    let stopped = context
        .drive_tier2(
            &mut critical_work,
            IndexingSignals {
                memory_pressure: MemoryPressure::Critical,
                ..IndexingSignals::default()
            },
        )
        .unwrap();
    assert_eq!(
        stopped.status,
        Tier2Status::Stopped(IndexStopReason::CriticalMemoryPressure)
    );
}

#[test]
fn task_hint_extraction_is_strictly_bounded() {
    let objective = (0..5_000)
        .map(|index| format!("src/file-{index}.rs"))
        .collect::<Vec<_>>()
        .join(" ");
    let (request, hints) = task_tier1_request(&objective, &[], &Tier1Budget::default());
    assert!(hints.objective_truncated);
    assert!(hints.mentioned_paths.len() <= MAX_TASK_CONTEXT_PATH_HINTS);
    assert!(hints.filename_terms.len() <= MAX_TASK_CONTEXT_FILENAME_TERMS);
    assert_eq!(
        request.budget.maximum_examined_entries,
        Tier1Budget::default().maximum_examined_entries
    );
}

#[test]
fn rejected_absolute_action_paths_never_enter_task_context() {
    let mut state = purrcode_runtime_core::SessionState::empty(SessionId::new());
    state.proposed_actions.insert(
        ActionId::new(),
        ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("/untrusted/README.md"),
            content: "ignored".into(),
            expected_digest: None,
        }),
    );
    state.proposed_actions.insert(
        ActionId::new(),
        ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("README.md"),
            content: "safe hint".into(),
            expected_digest: None,
        }),
    );

    assert_eq!(task_related_paths(&state), vec![PathBuf::from("README.md")]);
}

#[test]
fn rationale_stream_extracts_only_target_string_across_frames_and_escapes() {
    let mut extractor = RationaleStreamExtractor::default();
    let frames = [
        "{\"action\":{\"content\":\"must-not-leak\"},\"complete\":false,\"rati",
        "onale\":\"Line 1\\nquote: \\\"ok\\\" emoji: \\uD83D",
        "\\uDE3A and 汉字\",\"plan\":[\"also-not-visible\"]}",
    ];
    let visible = frames
        .into_iter()
        .filter_map(|frame| extractor.push(frame))
        .collect::<String>();
    assert_eq!(visible, "Line 1\nquote: \"ok\" emoji: 😺 and 汉字");
    assert!(!visible.contains("must-not-leak"));
    assert!(!visible.contains("also-not-visible"));
    assert!(!visible.contains("\"rationale\""));
    assert!(!visible.contains('{'));
}

#[test]
fn rationale_stream_disables_before_exceeding_its_byte_bound() {
    let mut extractor = RationaleStreamExtractor {
        emitted_bytes: MAX_STREAMED_RATIONALE_BYTES - 1,
        ..RationaleStreamExtractor::default()
    };
    assert!(extractor.push("{\"rationale\":\"é\"}").is_none());
    assert!(matches!(extractor.state, TopLevelJsonState::Disabled));
}

#[tokio::test]
async fn real_structured_stream_observes_transport_semantics_with_bounded_backpressure() {
    let output = observed_turn_json();
    let provider = StreamingProvider {
        streams: Mutex::new(vec![successful_observed_stream(&output)]),
        remain_pending: false,
    };
    let (observer, mut receiver) = bounded_agent_stream_channel(1).unwrap();
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    )
    .with_stream_observer(observer);
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();

    let run = agent.start(&mut store, repository.path(), "create new.txt");
    let observe = async {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            let completed = matches!(
                event,
                AgentStreamEvent::Phase {
                    phase: StreamPhase::Completed,
                    ..
                }
            );
            events.push(event);
            if completed {
                break;
            }
        }
        events
    };
    let (outcome, observations) = tokio::join!(run, observe);
    let outcome = outcome.unwrap();
    let AgentOutcome::AwaitingApproval { session_id, .. } = outcome else {
        panic!("streamed turn did not reach its expected approval boundary");
    };

    let receiving_timing = observations
        .iter()
        .find_map(|event| match event {
            AgentStreamEvent::Phase {
                phase: StreamPhase::Receiving,
                timing,
                ..
            } => Some(timing),
            _ => None,
        })
        .unwrap();
    assert!(receiving_timing.connected_ms.is_some());
    assert!(receiving_timing.first_byte_ms.is_some());
    assert!(receiving_timing.first_semantic_event_ms.is_some());
    assert!(receiving_timing.first_semantic_delta_ms.is_some());
    assert!(
        receiving_timing.first_byte_ms.unwrap()
            <= receiving_timing.first_semantic_delta_ms.unwrap()
    );
    let rendered: String = observations
        .iter()
        .filter_map(|event| match event {
            AgentStreamEvent::ContentDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(rendered, "implement objective");
    assert!(!rendered.contains('{'));
    assert!(!rendered.contains("\"action\""));
    assert!(observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::Phase {
            phase: StreamPhase::Finalizing,
            ..
        }
    )));
    assert!(observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::Phase {
            phase: StreamPhase::Completed,
            ..
        }
    )));

    let durable = store.events(session_id).unwrap();
    assert!(durable.iter().any(|event| matches!(
        event,
        SessionEvent::ModelRequestFinished {
            input_tokens: Some(17),
            output_tokens: Some(9),
            ..
        }
    )));
    assert!(durable.iter().any(|event| matches!(
        event,
        SessionEvent::ConversationMessageAdded {
            message: ConversationMessage { content, .. }
        } if content == "implement objective"
    )));
    assert!(!durable.iter().any(|event| matches!(
        event,
        SessionEvent::ConversationMessageAdded {
            message: ConversationMessage { content, .. }
        } if content == &output
    )));
}

#[tokio::test]
async fn partial_provider_cancellation_preserves_delta_without_completed_or_repair() {
    let partial = "{\"plan\":[],\"rationale\":\"part";
    let visible_partial = "part";
    let provider = StreamingProvider {
        streams: Mutex::new(vec![vec![
            Ok(ProviderStreamEvent::Connected),
            Ok(ProviderStreamEvent::BytesReceived {
                byte_count: partial.len(),
            }),
            Ok(ProviderStreamEvent::Model(ModelEvent::TextDelta(
                partial.into(),
            ))),
        ]]),
        remain_pending: true,
    };
    let (observer, mut receiver) = bounded_agent_stream_channel(32).unwrap();
    let cancellation = AgentCancellation::new();
    let cancel_after_delta = cancellation.clone();
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    )
    .with_stream_observer(observer)
    .with_cancellation(cancellation);
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();
    let session_id = SessionId::new();

    let run = agent.start_with_session_id(
        &mut store,
        repository.path(),
        "cancel after a partial response",
        session_id,
    );
    let observe = async {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            if matches!(
                &event,
                AgentStreamEvent::ContentDelta { delta, .. } if delta == visible_partial
            ) {
                cancel_after_delta.cancel();
            }
            let cancelled = matches!(
                event,
                AgentStreamEvent::Phase {
                    phase: StreamPhase::Cancelled,
                    ..
                }
            );
            events.push(event);
            if cancelled {
                break;
            }
        }
        events
    };
    let (result, observations) = tokio::join!(run, observe);
    let error = result.unwrap_err();
    assert!(error.is_cancelled());
    assert!(observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::ContentDelta { delta, .. } if delta == visible_partial
    )));
    assert!(observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::Phase {
            phase: StreamPhase::Cancelled,
            ..
        }
    )));
    assert!(!observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::Phase {
            phase: StreamPhase::Completed,
            ..
        }
    )));
    let durable = store.events(session_id).unwrap();
    assert_eq!(
        durable
            .iter()
            .filter(|event| matches!(event, SessionEvent::ModelRequestStarted { .. }))
            .count(),
        1
    );
    assert!(!durable
        .iter()
        .any(|event| matches!(event, SessionEvent::ModelRequestFinished { .. })));
    assert!(!durable
        .iter()
        .any(|event| matches!(event, SessionEvent::ConversationMessageAdded { .. })));
}

#[tokio::test]
async fn invalid_streamed_json_fails_closed_after_one_repair_without_completed() {
    let invalid_stream = || {
        vec![
            Ok(ProviderStreamEvent::Connected),
            Ok(ProviderStreamEvent::BytesReceived { byte_count: 8 }),
            Ok(ProviderStreamEvent::Model(ModelEvent::TextDelta(
                "not-json".into(),
            ))),
            Ok(ProviderStreamEvent::Model(ModelEvent::Finished)),
        ]
    };
    let provider = StreamingProvider {
        streams: Mutex::new(vec![invalid_stream(), invalid_stream()]),
        remain_pending: false,
    };
    let (observer, mut receiver) = bounded_agent_stream_channel(32).unwrap();
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    )
    .with_stream_observer(observer);
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();
    let session_id = SessionId::new();

    let error = agent
        .start_with_session_id(
            &mut store,
            repository.path(),
            "reject invalid provider JSON",
            session_id,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::Structured(_)));
    let observations = drain_observer(&mut receiver);
    for attempt in [1, 2] {
        assert!(observations.iter().any(|event| matches!(
            event,
            AgentStreamEvent::Phase {
                attempt: observed,
                phase: StreamPhase::Failed,
                ..
            } if *observed == attempt
        )));
    }
    assert!(!observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::Phase {
            phase: StreamPhase::Completed,
            ..
        }
    )));
    let durable = store.events(session_id).unwrap();
    assert!(!durable
        .iter()
        .any(|event| matches!(event, SessionEvent::ModelRequestFinished { .. })));
    assert!(!durable
        .iter()
        .any(|event| matches!(event, SessionEvent::ConversationMessageAdded { .. })));
}

#[tokio::test]
async fn terminal_escape_in_rationale_never_reaches_content_and_attempt_is_failed() {
    let valid = observed_turn_json();
    let unsafe_output = valid.replace("implement objective", "safe\\u001b[31m");
    let provider = StreamingProvider {
        streams: Mutex::new(vec![
            successful_observed_stream(&valid),
            successful_observed_stream(&unsafe_output),
        ]),
        remain_pending: false,
    };
    let (observer, mut receiver) = bounded_agent_stream_channel(64).unwrap();
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    )
    .with_stream_observer(observer);
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();

    let outcome = agent
        .start(&mut store, repository.path(), "reject terminal injection")
        .await
        .unwrap();
    let AgentOutcome::AwaitingApproval { session_id, .. } = outcome else {
        panic!("safe repair did not reach its approval boundary");
    };
    let observations = drain_observer(&mut receiver);
    assert!(observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::Phase {
            attempt: 1,
            phase: StreamPhase::Failed,
            ..
        }
    )));
    assert!(observations.iter().any(|event| matches!(
        event,
        AgentStreamEvent::Phase {
            attempt: 2,
            phase: StreamPhase::Completed,
            ..
        }
    )));
    let first_attempt = observations
        .iter()
        .filter_map(|event| match event {
            AgentStreamEvent::ContentDelta {
                attempt: 1, delta, ..
            } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(first_attempt, "safe");
    let repaired = observations
        .iter()
        .filter_map(|event| match event {
            AgentStreamEvent::ContentDelta {
                attempt: 2, delta, ..
            } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(repaired, "implement objective");
    assert!(observations.iter().all(|event| match event {
        AgentStreamEvent::ContentDelta { delta, .. } => {
            !delta.chars().any(is_unsafe_terminal_control) && !delta.contains("[31m")
        }
        AgentStreamEvent::Phase { .. } => true,
    }));
    let durable = store.events(session_id).unwrap();
    assert!(durable.iter().any(|event| matches!(
        event,
        SessionEvent::ConversationMessageAdded {
            message: ConversationMessage { content, .. }
        } if content == "implement objective"
    )));
    assert!(!durable.iter().any(|event| matches!(
        event,
        SessionEvent::ConversationMessageAdded {
            message: ConversationMessage { content, .. }
        } if content.contains('\u{001b}')
    )));
}

#[test]
fn observer_channel_rejects_unbounded_or_zero_capacity() {
    assert!(bounded_agent_stream_channel(1).is_ok());
    assert_eq!(
        bounded_agent_stream_channel(0).unwrap_err(),
        AgentStreamObserverError::InvalidCapacity {
            requested: 0,
            maximum: MAX_STREAM_OBSERVER_CAPACITY,
        }
    );
    assert_eq!(
        bounded_agent_stream_channel(MAX_STREAM_OBSERVER_CAPACITY + 1).unwrap_err(),
        AgentStreamObserverError::InvalidCapacity {
            requested: MAX_STREAM_OBSERVER_CAPACITY + 1,
            maximum: MAX_STREAM_OBSERVER_CAPACITY,
        }
    );
}

#[tokio::test]
async fn write_action_pauses_for_durable_human_approval() {
    let provider = MockProvider {
        responses: Mutex::new(vec![serde_json::json!({
            "plan": ["write isolated file", "validate"],
            "rationale": "implement objective",
            "action": {
                "type": "write_file",
                "path": "new.txt",
                "content": "created",
                "expected_digest": null
            },
            "complete": false
        })]),
    };
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    );
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();
    let outcome = agent
        .start(&mut store, repository.path(), "create new.txt")
        .await
        .unwrap();
    let AgentOutcome::AwaitingApproval {
        session_id,
        action_id,
        ..
    } = outcome
    else {
        panic!("agent did not pause for approval");
    };
    assert_eq!(
        store.load(session_id).unwrap().status,
        SessionStatus::AwaitingApproval(action_id)
    );
    let executed = agent.approve(&mut store, session_id).await.unwrap();
    assert!(matches!(executed, AgentOutcome::ActionExecuted { .. }));
    let state = store.load(session_id).unwrap();
    assert_eq!(
        std::fs::read_to_string(state.worktree.unwrap().join("new.txt")).unwrap(),
        "created"
    );
    assert!(store
        .events(session_id)
        .unwrap()
        .iter()
        .any(|event| matches!(
            event,
            SessionEvent::ApprovalRecorded {
                authority: ApprovalAuthority::Human,
                ..
            }
        )));
}

#[tokio::test]
async fn repeated_deterministic_denials_pause_before_more_provider_calls() {
    let denied_turn = || {
        serde_json::json!({
            "plan": ["inspect repository"],
            "rationale": "try an action that the policy deterministically denies",
            "action": {
                "type": "write_file",
                "path": ".",
                "content": "value",
                "expected_digest": null
            },
            "complete": false,
            "current_step_index": 0,
            "expected_postconditions": []
        })
    };
    let provider = MockProvider {
        responses: Mutex::new(vec![denied_turn(), denied_turn(), denied_turn()]),
    };
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    );
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();

    let outcome = agent
        .start(&mut store, repository.path(), "inspect the repository")
        .await
        .unwrap();

    let AgentOutcome::IterationLimit { session_id } = outcome else {
        panic!("repeated policy denials did not pause the session");
    };
    let state = store.load(session_id).unwrap();
    assert_eq!(state.status, SessionStatus::Paused);
    assert!(store.events(session_id).unwrap().iter().any(|event| {
        matches!(
            event,
            SessionEvent::SessionPaused { reason }
                if reason.contains("rejected 3 consecutive proposed actions")
        )
    }));
    assert!(
        provider.responses.lock().unwrap().is_empty(),
        "the circuit breaker must stop after exactly three denied turns"
    );
}

#[tokio::test]
async fn approval_then_resume_completes_full_turn_with_next_action() {
    let provider = MockProvider {
        responses: Mutex::new(vec![
            serde_json::json!({
                "plan": ["write isolated file", "validate"],
                "rationale": "task done",
                "complete": true
            }),
            serde_json::json!({
                "plan": ["write isolated file", "validate"],
                "rationale": "implement objective step 1",
                "action": {
                    "type": "write_file",
                    "path": "step1.txt",
                    "content": "first step",
                    "expected_digest": null
                },
                "complete": false
            }),
        ]),
    };
    let (observer, _receiver) = bounded_agent_stream_channel(64).unwrap();
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    )
    .with_stream_observer(observer);
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();
    let session_id = SessionId::new();

    let outcome = agent
        .start_with_session_id(
            &mut store,
            repository.path(),
            "write step1.txt and finish",
            session_id,
        )
        .await
        .unwrap();
    let AgentOutcome::AwaitingApproval {
        action_id, reason, ..
    } = outcome
    else {
        panic!("agent did not pause for approval");
    };
    assert!(reason.contains("requires human approval"));
    assert_eq!(
        store.load(session_id).unwrap().status,
        SessionStatus::AwaitingApproval(action_id)
    );

    let executed = agent.approve(&mut store, session_id).await.unwrap();
    let AgentOutcome::ActionExecuted { result, .. } = executed else {
        panic!("agent did not report action executed");
    };
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        std::fs::read_to_string(
            store
                .load(session_id)
                .unwrap()
                .worktree
                .unwrap()
                .join("step1.txt")
        )
        .unwrap(),
        "first step"
    );

    let state = store.load(session_id).unwrap();
    assert_eq!(state.status, SessionStatus::Active);
    let final_outcome = agent.resume(&mut store, session_id).await.unwrap();
    let AgentOutcome::Completed { .. } = final_outcome else {
        panic!("agent did not complete after resuming");
    };
    let durable = store.events(session_id).unwrap();
    assert!(durable
        .iter()
        .any(|event| matches!(event, SessionEvent::SessionCompleted)));
}

#[tokio::test]
async fn failed_validation_routes_a_repair_then_reruns_focused_and_full_checks() {
    let repository = repository();
    std::fs::write(
        repository.path().join("Makefile"),
        "test:\n\t@test -f repaired.marker\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["add", "Makefile"])
        .current_dir(repository.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-c",
            "user.name=PurrCode",
            "-c",
            "user.email=test@local.invalid",
            "commit",
            "-q",
            "-m",
            "validation fixture",
        ])
        .current_dir(repository.path())
        .status()
        .unwrap()
        .success());
    let provider = MockProvider {
        responses: Mutex::new(vec![
            serde_json::json!({
                "plan": ["repair the failed test", "validate"],
                "rationale": "validation now passes",
                "complete": true
            }),
            serde_json::json!({
                "plan": ["repair the failed test", "validate"],
                "rationale": "create the fixture required by the failed test",
                "action": {
                    "type": "write_file",
                    "path": "repaired.marker",
                    "content": "repaired\n",
                    "expected_digest": null
                },
                "complete": false
            }),
            serde_json::json!({
                "plan": ["run validation", "repair the failed test"],
                "rationale": "the implementation is ready for validation",
                "complete": true
            }),
        ]),
    };
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    );
    let mut store = SessionStore::in_memory().unwrap();
    let outcome = agent
        .start(
            &mut store,
            repository.path(),
            "make the validation fixture pass",
        )
        .await
        .unwrap();
    let AgentOutcome::AwaitingApproval { session_id, .. } = outcome else {
        panic!("agent did not propose a repair after validation failed");
    };
    agent.approve(&mut store, session_id).await.unwrap();
    let outcome = agent.resume(&mut store, session_id).await.unwrap();
    assert!(matches!(outcome, AgentOutcome::Completed { .. }));
    let validation_statuses = store
        .events(session_id)
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::ValidationRecorded { status, .. } => Some(status),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(validation_statuses.contains(&ValidationStatus::Failed));
    assert!(
        validation_statuses
            .iter()
            .filter(|status| **status == ValidationStatus::Passed)
            .count()
            >= 2
    );
}

#[tokio::test]
async fn plan_only_session_is_durable_and_never_mutates_source_repository() {
    let provider = MockProvider {
        responses: Mutex::new(vec![serde_json::json!({
            "steps": ["inspect the implementation", "make a bounded change", "run tests"],
            "assumptions": ["existing tests describe expected behavior"],
            "risks": ["avoid changing public interfaces"]
        })]),
    };
    let (observer, mut receiver) = bounded_agent_stream_channel(64).unwrap();
    let agent = NativeAgent::new(
        &provider,
        ModelId::parse("local/test").unwrap(),
        Policy::default(),
    )
    .with_stream_observer(observer);
    let repository = repository();
    let mut store = SessionStore::in_memory().unwrap();
    let session_id = SessionId::new();
    store
        .append(
            session_id,
            &SessionEvent::SessionCreated {
                objective: "plan a safe change".into(),
                repository: repository.path().canonicalize().unwrap(),
            },
        )
        .unwrap();
    let plan = agent
        .plan_initialized(&mut store, session_id)
        .await
        .unwrap();
    assert_eq!(plan.steps.len(), 3);
    let state = store.load(session_id).unwrap();
    assert_eq!(state.status, SessionStatus::Paused);
    assert_eq!(state.plan_steps, plan.steps);
    let observations = drain_observer(&mut receiver);
    assert!(!observations
        .iter()
        .any(|event| matches!(event, AgentStreamEvent::ContentDelta { .. })));
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(status.stdout.is_empty());
}
