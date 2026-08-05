<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# Installation

## macOS and Linux installer

```bash
curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.9.0/scripts/install.sh | sh
```

The installer detects the host platform, downloads the release archive, verifies it against the release `SHA256SUMS`, and installs into `~/.local/bin`. Use a custom installation directory:

```bash
PURRCODE_INSTALL_DIR="$HOME/bin" \
  curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.9.0/scripts/install.sh | sh
```

## npm

Node.js 18 or newer:

```bash
npm install --global @minaovo/purrcode
```

The package selects the correct macOS, Linux, or Windows binary, verifies its pinned SHA-256 digest, and exposes both `purrcode` and `purrcoded`.

You can also install the signed-release launcher directly from GitHub:

```bash
npm install --global https://github.com/Weilin0723/PurrCode/releases/latest/download/purrcode-0.9.0.tgz
```

## Windows

Download and extract `purrcode-x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/Weilin0723/PurrCode/releases/latest).

## Build from source

Requirements:

- Rust 1.88 or newer;
- Git;
- platform build tools.

```bash
git clone https://github.com/Weilin0723/PurrCode.git
cd PurrCode

cargo install --locked --path crates/purrcode-cli
cargo install --locked --path crates/purrcode-daemon
```

## Verify the install

```bash
purrcode --version
purrcoded --version
```

## Upgrade and rollback

```bash
purrcode upgrade check --channel stable
purrcode upgrade download /tmp/purrcode-release.tar.gz
purrcode upgrade install --channel stable
purrcode upgrade rollback
```

`install` verifies the checksum manifest and both Sigstore bundles before extraction, rejects unsafe archive paths, atomically rotates `purrcode` and `purrcoded`, and preserves the prior pair for rollback.

## Optional isolation dependencies

- macOS: `/usr/bin/sandbox-exec` when available.
- Linux: `bubblewrap` (`bwrap`).
- Windows: process filtering and job controls; inspect with `purrcode sandbox doctor`.

PurrCode reports degraded isolation explicitly and does not reinterpret it as a full sandbox.
