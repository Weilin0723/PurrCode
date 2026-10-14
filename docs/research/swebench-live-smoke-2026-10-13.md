# SWE-bench-Live E2E runner smoke — 2026-10-13 local / 2026-10-14 UTC

Status: harness-only smoke evidence. These runs use generated temporary repositories and are not
study observations and are not pooled with the 30-task SWE-bench-Live schedule.

## B arm — static completion

The alignment-disabled research gate was enabled on an isolated daemon. A fresh temporary Git
repository was given the objective:

> Create a file named `bmarker.txt` containing exactly: bmarker

The daemon created an isolated worktree, the agent wrote the file, the session reached
`completed`, no expectation contract was observed, and the extracted patch contained exactly
`bmarker.txt`. This verifies the B-arm API path, worktree handling, claim extraction, and patch
extraction.

## C arm — two-phase correction and fresh review

A second fresh temporary repository received the P0 objective:

> Create a new file `marker.txt` in the repository root containing exactly: marker

After the first phase settled with a successful `native:write_file` event, the runner sent the P1
follow-up on the same durable session:

> Create one additional new file `followup.txt` containing exactly: followup. Preserve marker.txt.

The daemon recorded a `requirement_revision_ack`; the session then completed. The extracted patch
contained both `marker.txt` and `followup.txt`, and excluded `.purrcode/`. This verifies that the
correction is delivered in a second phase rather than racing an active turn, that the same worktree
is preserved, and that runtime context databases never leak into the scored patch.

The one earlier smoke attempt proved the race the final runner avoids: an active-turn follow-up
could be durably recorded yet complete before the model consumed it. The final runner waits for the
first phase to settle and then resumes the session for P1. That failed attempt is not reported as
evidence.

## Local oracle boundary

The local Apple Silicon host can pull and start the official x86_64 images through Rosetta, but
running native extension tests can terminate with `Illegal instruction`. The runner therefore does
not treat local Rosetta output as final hidden-oracle evidence. The frozen x86_64 GitHub Actions
matrix performs the gold preflight and candidate scoring natively.

Machine-readable study evidence is stored in
`docs/research/evidence/2026-10-13/swebench-live-v1/`; this note records only smoke plumbing.
