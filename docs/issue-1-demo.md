# Issue #1 end-to-end demo evidence

Date: 2026-07-26  
Platform: macOS  
Real provider: local Ollama, `llama3.2:1b`

## Exact run

```bash
PURRCODE_LIVE_OLLAMA_MODEL='llama3.2:1b' cargo test -p purrcode-daemon \
  live_ollama_connect_and_provider_backed_multiturn_streaming -- --ignored --nocapture
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm test --prefix sdk/typescript
npm test --prefix apps/vscode-extension
PYTHONPATH=sdk/python/src python3 -m unittest discover -s sdk/python/tests -v
```

The live provider test passed discovery, configuration through the authenticated daemon API,
provider health, and two streamed turns with the second request containing the first assistant
turn. It then unloaded the model and verified that `/api/ps` returned `{"models":[]}`. No fixture
provider was used for this test.

## Eighteen-step product path

1. Launch the daemon-backed conversation workspace.
2. Open `/connect` and select local Ollama.
3. Discover installed Ollama models without requesting a credential.
4. Persist the selected provider through the authenticated daemon API.
5. Run provider health and display only redacted status.
6. Select the discovered coding model.
7. Send a repository conversation turn and consume streamed deltas.
8. Send a second turn with the durable prior conversation as context.
9. Detect a missing Terraform capability.
10. Search configured skill sources only after approval.
11. Inspect provenance, manifest, permissions, signature state, and package files.
12. Bind installation to the exact inspected digest and explicit scope.
13. Dynamically qualify the declared entrypoint inside Claw.
14. Deny missing or mismatched per-invocation PawGate authorization.
15. Allow the exact invocation once and deny its replay.
16. Reopen daemon-owned skill storage and resolve the installed skill first.
17. Record matched, reused, and external-search-avoided events without starting external search.
18. Export durable original timestamps with private fields removed and identifiers pseudonymized.

The real-provider test covers steps 2–8. Workspace integration tests cover steps 1 and 9–18,
including negative paths. The repository-wide checks above are the executable evidence; skipped or
unavailable checks are not represented as passing.
