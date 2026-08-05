<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# Development and Testing

## Build

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

## Repository checks

Run from the project root:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

npm test --prefix packages/purrcode
npm test --prefix sdk/typescript

PYTHONPATH=sdk/python/src \
  python3 -m unittest discover -s sdk/python/tests -v
```

## Implementation status

PurrCode is under active development. See the repository's [implementation status](https://github.com/Weilin0723/PurrCode/blob/main/docs/implementation-status.md) and the [PurrCode v1.0 Master PRD](https://github.com/Weilin0723/PurrCode/blob/main/docs/prd/PurrCode_v1.0_Codex_Master_PRD.md) for milestone status, verified capabilities, and remaining platform-specific gates.

## Contributing

Contributions are welcome. Before opening a pull request:

1. review the architecture and security documentation;
2. preserve the existing trust boundaries;
3. do not allow model providers to authorize actions;
4. keep external content and downloaded skills untrusted;
5. add deterministic tests;
6. run formatting, Clippy, and workspace tests;
7. document unsupported or skipped validation honestly.

Suggested contribution areas:

- provider compatibility;
- terminal usability;
- qualification fixtures;
- cross-platform sandboxing;
- documentation;
- accessibility;
- performance;
- skill and MCP catalogs;
- model recommendation evidence;
- integration testing.
