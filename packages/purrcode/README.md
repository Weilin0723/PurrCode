# PurrCode npm launcher

This package installs the native `purrcode` and `purrcoded` binaries from the matching signed
[PurrCode release](https://github.com/Weilin0723/PurrCode/releases).

```bash
npm install --global @minaovo/purrcode
purrcode init
purrcode
```

The npm package installs PurrCode v0.6.0 and exposes both `purrcode` and `purrcoded`. Provider
profiles support Ollama, NVIDIA NIM, LM Studio, and OpenAI-compatible endpoints; credentials remain
in the operating-system secret store.

You can also install the signed release tarball directly:

```bash
npm install --global https://github.com/Weilin0723/PurrCode/releases/download/v0.6.0/purrcode-0.6.0.tgz
```

The installer supports macOS ARM64/x64, Linux ARM64/x64, and Windows x64. It downloads over HTTPS,
accepts redirects only to GitHub release hosts, verifies a checksum pinned in the npm package, and
then exposes both commands through npm's normal binary directory.

PurrCode is licensed under Apache-2.0. Source, documentation, release signatures, and provenance are
available in the [main repository](https://github.com/Weilin0723/PurrCode).
