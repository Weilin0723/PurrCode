# PurrCode RC 0.1 Release Pipeline Report

Date: 2026-07-25

## Outcome

The repository-level release pipeline is statically validated and locally testable. It is not yet
proven in GitHub's hosted environment, and no release artifact is represented as published or
installable.

## CI workflow

`.github/workflows/ci.yml` runs on pushes and pull requests. Its Rust matrix covers current GitHub
Ubuntu, macOS, and Windows runners and executes formatting, Clippy with warnings denied, and all
workspace tests. A separate Linux job installs locked Node dependencies, tests both TypeScript
clients, and runs the Python SDK unit suite.

## Signed release workflow

`.github/workflows/release.yml` accepts `v*` tag pushes and manual dispatch. Build and publish jobs
are explicitly tag-guarded, so dispatching from a branch validates without publishing a release.
The validation job runs formatting, Clippy, and workspace tests before any matrix build.

The native matrix contains:

| Platform | Target | Archive |
|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| macOS x86_64 | `x86_64-apple-darwin` | `.tar.gz` |
| macOS ARM64 | `aarch64-apple-darwin` | `.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `.zip` |

Each build uses the lockfile and packages both PurrCode binaries, `purrcode` and `purrcoded`, with
the README and license. Uploads fail when the expected archive is absent. The
publish job merges artifacts, creates `SHA256SUMS`, signs every archive and the manifest through
Sigstore keyless OIDC, requests GitHub build provenance, and uploads all distribution files to the
GitHub Release. Write, OIDC, and attestation permissions are restricted to that publish job.

## Package-manager templates

- The Homebrew formula selects ARM64 or x86_64 macOS archives and installs both binaries.
- The winget singleton manifest selects the Windows x86_64 archive and exposes both portable
  commands.
- Both intentionally contain `RELEASE_AUTOMATION_REPLACES_THIS_VALUE`; they are templates and must
  not be submitted until a release process substitutes real SHA-256 values.
- Repository, upgrade, formula, and winget URLs point to `Weilin0723/PurrCode`.

## External gates

The following were not run because this checkout has no authenticated GitHub session and neither
`act` nor `actionlint` is installed:

1. Hosted execution of CI on all three operating systems.
2. A tag-triggered signed release with GitHub OIDC, attestations, and artifact upload.
3. Substitution and publication of real Homebrew and winget checksums.
4. Installation and rollback smoke tests from the produced macOS release archive.

These remain external gates. Static inspection or a skipped tool is not counted as execution
success.

## Local follow-up after Actions run 30183639751

The Ubuntu job failed at `cargo fmt --all --check` on commit `687acc8`. The formatting diff has been
applied locally, and `cargo fmt --all --check`, Clippy with warnings denied, and the full workspace
test suite pass in this checkout. GitHub-hosted rerun evidence remains external until this checkout
is authenticated and the fix is pushed.
