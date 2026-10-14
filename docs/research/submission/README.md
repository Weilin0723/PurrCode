# Repository-completion submission bundle

The double-anonymous supplementary archive is
`anonymous-repository-completion-study.tar.gz`.

- SHA-256: `2b4e950fac68ffebaec4a522d1a0f8a28b352f69cbc6b7798adde0385eeb90f3`.
- Contents: 87 manifest-listed payload files plus `ARTIFACT-MANIFEST.json`, covering frozen
  protocols, fixture/oracles, complete 288-call local evidence, both DeepSeek follow-up runs,
  independent analyses, audit outputs, and reproduction tests.
- Validation: extraction and all internal checksums passed; 28 packaged offline tests passed; the
  identity, local-path, and credential-pattern scan had no findings.
- Anonymization: prototype/package names and local paths are replaced in the derivative archive.
  The frozen hashes continue to identify the immutable pre-sanitization originals, as disclosed in
  the archive README; counts, labels, outputs, schedule indices, statistics, and model digests are
  unchanged.

Rebuild deterministically from the working repository with:

```sh
python3 scripts/build_anonymous_research_artifact.py
```

At submission time, upload the archive as supplementary material. A persistent public archive URL
should be added only after acceptance so that the review copy remains double-anonymous.
