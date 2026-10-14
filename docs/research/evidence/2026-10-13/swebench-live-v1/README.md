# SWE-bench-Live E2E evidence

This directory contains the frozen, model-external 30-task real-repository study artifacts.
The run bundle contains only public problem statements, the controlled P0/P1 revision, and the
120-cell schedule. Hidden test patches, solution patches, hints, and test lists remain in the
operator-side dataset/oracle boundary.

- `qualification.json` is the operator-side DeepSeek qualification output.
- `model-freeze.md` freezes model, resource, and stop/retry rules.
- `config.toml` contains no secret; it names `keychain:deepseek` as the credential reference.
- `runner-lock.json` binds the E2E/oracle scripts and the committed run bundle.
- `dependency-lock.json` binds the official harness commit, Python runtime, and Docker wrapper policy.
- `analysis-plan.json`, `freeze.json`, and `receipt.json` are the pre-call analysis freeze.
- `gold-oracle-preflight.json` is populated only after the x86_64 oracle audit passes.

The local Apple Silicon host is not used for final oracle evidence: running the public x86_64
images under Rosetta can terminate native extension tests with `Illegal instruction`. The final
run is executed on the x86_64 GitHub Actions matrix in `.github/workflows/research-swebench-live.yml`.
