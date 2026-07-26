# Recovery and rollback

Sessions and authorizations are event-sourced in SQLite WAL mode. Start/resume/approval, model
requests, execution, validation, plans, compaction, and dispositions are durable.

## After interruption

```bash
purrcode sessions
purrcode resume [SESSION_ID]
purrcode review [SESSION_ID]
```

An action durably marked started without a finish becomes `uncertain` at daemon startup and is
never retried automatically. An interrupted model request becomes an explicit recovery review.
Failed and cancelled worktrees remain inspectable.

## Checkpoint and rollback

The initial worktree boundary is always checkpointed. TUI/daemon clients can add manual
checkpoints. `purrcode rollback` resets only the isolated session worktree and never discards
pre-existing changes in the source tree.

Reviewed hunks use an optimistic patch digest. Applying a hunk first runs `git apply --check`
against the current source tree. Rejecting a hunk reverses it only inside the isolated result.

## Database backup

```bash
purrcode database migration-preview
purrcode database backup /safe/location/purrcode.db
```

Backups use SQLite’s online backup API, refuse overwrite, and run an integrity check.

## Configuration migration

```bash
purrcode config migration-preview
purrcode config migrate
```

Migration validates the complete result before replacement and preserves the prior file as a
non-overwritten versioned backup. PurrCode supports the immediately preceding legacy schema;
newer unknown schemas fail before daemon startup.
