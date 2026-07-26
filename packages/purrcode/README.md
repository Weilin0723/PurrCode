# PurrCode npm launcher

This package installs the native `purrcode` and `purrcoded` binaries from the matching signed
[PurrCode release](https://github.com/Weilin0723/PurrCode/releases).

```bash
npm install --global purrcode
purrcode init
purrcode
```

Until the package is published to the public npm registry, install the release tarball directly:

```bash
npm install --global https://github.com/Weilin0723/PurrCode/releases/download/v0.2.1/purrcode-0.2.1.tgz
```

The installer supports macOS ARM64/x64, Linux ARM64/x64, and Windows x64. It downloads over HTTPS,
accepts redirects only to GitHub release hosts, verifies a checksum pinned in the npm package, and
then exposes both commands through npm's normal binary directory.

PurrCode is licensed under Apache-2.0. Source, documentation, release signatures, and provenance are
available in the [main repository](https://github.com/Weilin0723/PurrCode).
