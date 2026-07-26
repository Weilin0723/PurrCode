# Troubleshooting

## The TUI cannot reach the daemon

Run `purrcode init`, or start `purrcode serve`. Verify the configured URL is loopback and that
the daemon token file exists with owner-only permissions.

## A provider is unavailable

Run:

```bash
purrcode provider doctor
purrcode model list
purrcode model qualify PROVIDER/MODEL
```

For keychain-backed providers, re-enter the credential with `purrcode credential set PROVIDER`.
Local Ollama/LM Studio processes must be running and listening on the configured loopback port.

## An action needs approval

Inspect the exact action, deterministic and contextual judgment, diff, and validation evidence.
Approve, reject, or edit the proposed command. Scheduled and parallel workers never approve
themselves.

## Sandbox is degraded

Run `purrcode sandbox doctor`. Install bubblewrap on Linux where practical. A degraded result
means worktree boundaries and command filtering remain active, but network/process isolation is not
equivalent to the supported OS sandbox.

## A session is uncertain

Do not rerun the action blindly. Inspect its worktree and durable events, then cancel, roll back, or
start a new task. See [recovery](recovery.md).

## Validation is unavailable

The final report identifies the missing executable or undetected validation. Install the required
toolchain and resume; unavailable/skipped checks are never counted as passed.
