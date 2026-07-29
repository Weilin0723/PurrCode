# Security model

PurrCode fails closed when authorization is absent, mismatched, or already consumed. The digest
covers both the proposed action and its constraints. The executor checks the digest through the
store and checks constraints again before spawning.

Tool processes receive only `PATH`, temporary-directory, locale, and terminal variables plus an
explicitly authorized custom environment. Provider keys are therefore not inherited accidentally.
Current policy requires human approval for any custom environment; the daemon's `/approve` and
`/deny` exact-action commands gate that flow.

## Sandbox reality

The initial backend is process isolation, not a complete sandbox. Network prohibition and
filesystem write globs need an OS-specific enforcement adapter plus post-execution effect
reconciliation. The current policy compensates by allowing only narrow typed read-only forms.

The `SandboxLevel` reported by `purrcode-claw::sandbox_capability` is determined by the host:

| Host | Backend string | Reported level |
|---|---|---|
| macOS with `/usr/bin/sandbox-exec` | `macos-sandbox-exec` | `RestrictedProcessNoNetwork` |
| Linux with `bwrap` on `PATH` | `linux-bubblewrap` | `RestrictedProcessNoNetwork` |
| Other Unix / Windows | `process-filter-fallback` | `WorktreeWriteNoShell` |

Hosts that fall back to `process-filter-fallback` do not have OS-level isolation and the
`network_isolation` field on `SandboxCapability` is `false`. Nothing in PurrCode labels that
fallback as a full sandbox; the architecture summary and `docs/implementation-status.md` describe
the result as process isolation. `purrcode-claw` tests cover the credential scrub, process-group
timeout (`timeout_terminates_the_entire_process_group`), and exact-action digest recheck
(`authorized_write_is_atomic_and_single_use`,
`authorized_repository_read_executes_through_sandbox`).

## Read commands are typed

Repository reads are no longer shell-string commands. The model emits `RepositoryReadAction`
variants (`GitStatus`, `GitLog`, `GitDiff`, `GitShow`, `GitLsFiles`, `RepositoryGrep`, `Find`,
`List`) and the runtime synthesizes a safe invocation. Claw cannot receive `-exec`, `-delete`,
external roots, or shell metacharacters through the typed path. `unsafe_repository_read` rejects
parent traversal, absolute paths, and patterns that resolve outside the worktree. After
validation the policy returns `AllowWithConstraints` with `network = false`,
`allowed_write_globs = []`, and `maximum_changed_files = 0`; reads never trigger contextual
judgment and never require human approval. Unsafe predicates remain denied by the policy layer
(`typed_repository_reads_with_unsafe_paths_are_denied` in `purrcode-pawgate`) and never reach
Claw.

## State machine

`SessionState::reduce_event` is the authoritative reducer. Approvals must reference the
currently pending `ActionId`; an approval for any other id returns `DomainError::UnexpectedApproval`.
Terminal statuses (`Completed`, `Cancelled`, `Failed`) reject every event except explicit recovery.
The full transition matrix lives in `purrcode-runtime-core::is_valid_transition`.

## Approval words

Typing `approve`, `deny`, or `reject` as bare composer text when an approval card is visible
routes to the matching exact-action command (`/approve`, `/deny <reason>`). When no approval card
is visible the same words are rejected locally with a clear explanation; the session never
transitions to `Failed` from a stray approval word. The durable PawGate boundary and Claw
digest recheck are preserved in both paths.

## Test references

- `purrcode-runtime-core::tests::reducer_accepts_typed_read_proposal_and_executes_through_active`
- `purrcode-runtime-core::tests::reducer_rejects_approval_for_unknown_action`
- `purrcode-runtime-core::tests::reducer_rejects_approval_for_wrong_action_id`
- `purrcode-runtime-core::tests::reducer_rejects_transitions_from_terminal_states`
- `purrcode-pawgate::tests::typed_repository_reads_with_relative_or_canonical_root_are_allowed`
- `purrcode-pawgate::tests::typed_repository_reads_with_unsafe_paths_are_denied`
- `purrcode-pawgate::tests::typed_repository_read_with_empty_root_is_denied`
- `purrcode-claw::tests::authorized_repository_read_executes_through_sandbox`
- `purrcode-claw::tests::authorized_write_is_atomic_and_single_use`
- `purrcode-claw::tests::timeout_terminates_the_entire_process_group`
- `purrcode-tui::keybindings::tests::bare_approval_words_are_detected_exactly`
- `purrcode-tui::keybindings::tests::bare_approval_command_maps_to_exact_action`
