# Research benchmark artifacts

This directory contains two separate forms of evidence for the FSE research
direction. The fixture labels are human-authored synthetic annotations; they
are not results from live coding tasks or executed repository tests.

## Deterministic mechanism benchmark

`crates/runtime-core/tests/revision_benchmark.rs` feeds durable events to the
real `SessionState::reduce_event` reducer and evaluates the delivery gate. It
covers fresh evidence, revised requirements, artifact edits, scope
preservation, redundant corrections, and a static control. Its expected
decisions are hand-written scenario oracles.

Run it from the repository root with an optional new JSON output path:

```bash
PURRCODE_REVISION_BENCHMARK_JSON=target/research/revision-mechanism.json \
  cargo test -p purrcode-runtime-core --test revision_benchmark -- --nocapture
```

This is mechanism regression evidence, not a live-agent or A/B/C experiment.
A zero exit status means the deterministic assertions and known-gap checks
were reproduced; it does not mean every expected mechanism is implemented.
The report records unsupported cases and observed failures explicitly.

## Local review probe

`review-traces.json` and `scripts/local_review_probe.py` define a bounded
Ollama probe over 12 repeated synthetic tasks, two prompt conditions, and
seeds 17 and 29. It sends only the current request, artifact, and observed
evidence to the local model. It is not PurrCode provider qualification, live
coding, or an independent-task effect experiment.

Run it with a new output directory; an existing directory is rejected:

```bash
python3 scripts/local_review_probe.py \
  --model llama3.2:1b \
  --out target/research/local-review-probe-<new-directory>
```

The Ollama server must be reachable at the default localhost URL, or an
explicit localhost-only `--base-url` may be supplied. No probe result is
claimed here until the run produces its manifest, JSONL event log, results,
and summary.
