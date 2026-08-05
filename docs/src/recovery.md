<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# Recovery

NineLives maintains durable session events and checkpoints.

Recovery behavior includes:

- restart reconciliation;
- uncertain-action detection;
- preserved isolated worktrees;
- session resume;
- manual checkpoints;
- isolated rollback;
- partial-output preservation after cancellation;
- lease-conflict handling.

Interrupted actions are not automatically replayed. When the runtime cannot prove whether an action completed, it marks the result as uncertain and requires review.

## Resume

```bash
purrcode resume
```

Resumes a paused plan in the same session. `purrcode resume --tui` reattaches the terminal Workbench.

## Rollback

```bash
purrcode rollback
```

Rolls back isolated work. Rollback requires an exact preview digest and explicit acknowledgement of any unattributed effects; invalid event append or replay fails loudly.

## Recovery evidence

The repository's [recovery documentation](https://github.com/Weilin0723/PurrCode/blob/main/docs/recovery.md) and [troubleshooting guide](https://github.com/Weilin0723/PurrCode/blob/main/docs/troubleshooting.md) cover durable state and failure behavior in detail.
