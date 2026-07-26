# Security model

PurrCode fails closed when authorization is absent, mismatched, or already consumed. The digest
covers both the proposed action and its constraints. The executor checks the digest through the
store and checks constraints again before spawning.

Tool processes receive only `PATH`, temporary-directory, locale, and terminal variables plus an
explicitly authorized custom environment. Provider keys are therefore not inherited accidentally.
Current policy requires human approval for any custom environment; the CLI does not yet implement
that approval flow.

The initial backend is process isolation, not a complete sandbox. Network prohibition and
filesystem write globs need an OS-specific enforcement adapter plus post-execution effect
reconciliation. The current policy compensates by allowing only narrow read-only command forms.

