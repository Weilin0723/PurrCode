# v0.4 product redesign acceptance

Audited: 2026-07-26 against Issue #5 and PRD v1.1.

| Phase | Result | Evidence |
|---|---|---|
| 0 — baseline and fixtures | Implemented | Provider fixtures, deterministic render snapshots, 409 recovery fixture path, and pre/post workspace checks. |
| 1 — multiline composer | Implemented | PR #6; Unicode graphemes, multiline paste, history, selection, paging, undo/redo, and 256 KB performance test. |
| 2 — content and secret guard | Implemented | PR #7; pre-submit guard plus daemon rejection and durable-secret regression tests. |
| 3 — provider import engine | Implemented | PR #8; parse-only Python, JavaScript, cURL, dotenv, JSON, YAML, TOML, malformed/dynamic fixtures, confidence, warnings, and redaction. |
| 4 — provider setup | Implemented | PR #9; discovery, editable import review, secure credential reference, real health test, model discovery, role assignment, and saved-profile management. |
| 5 — repository workspace | Implemented | PR #10; responsive status/workspace/timeline/composer layout and bounded metadata-only repository inspection. |
| 6 — runtime cards | Implemented | PR #11; semantic event cards, bounded expandable output, validation states, exact approval keys, and daemon-backed full diff. |
| 7 — recovery and polish | Implemented | PR #12; actionable 409 recovery, redacted draft/session restoration, searchable command palette, `NO_COLOR`, ASCII fallback, snapshots, and performance tests. |

CI run 30194463045 passed clients plus Rust formatting, Clippy, and workspace tests on macOS,
Ubuntu, and Windows. The real local Ollama connect and provider-backed multi-turn streaming test
passed. `qwen2.5-coder:7b` separately failed full model capability qualification at 2/7; this is
recorded in the provider report and is not represented as a product acceptance pass.

The signed v0.4.0 workflow is run 30194842815. Release assets and installation smoke evidence must
be checked after the workflow finishes.
